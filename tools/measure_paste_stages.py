#!/usr/bin/env python3
"""Measure owned X11 selection delivery into a real raw PTY, without echo.

The selection owner and PTY sink run in independent event-driven processes.
Neither sleeps between INCR chunks or PTY reads. All timestamps use the same
Linux monotonic clock. Owner-side deletion notifications are observations of
X server events, not timestamps inside ztty's finish_paste function.

Based on the selection protocol in tests/osc52_gui.py; intentionally does not
reuse Gui.wait(), whose polling sleep would affect incremental transfer timing.
"""
import argparse
import contextlib
import ctypes
import hashlib
import json
import os
from pathlib import Path
import secrets
import select
import socket
import statistics
import subprocess
import sys
import tempfile
import time


START, END, DSR = b"\x1b[200~", b"\x1b[201~", b"\x1b[1;1R"
MAX_BYTES = 4 * 1024 * 1024
SEED = "中文粘贴测试 abc123\t下一段\r\n".encode()


def stamp():
    return time.monotonic_ns()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def payload(size):
    if not 1 <= size <= MAX_BYTES:
        raise ValueError("selection size must be within 1..4 MiB")
    data = (SEED * (size // len(SEED) + 1))[:size]
    valid = data.decode("utf-8", "ignore").encode()
    return valid + b"x" * (size - len(valid))


def paste_wire(data, bracketed):
    # zinput 0.1.3 encode_paste: Rust char::is_control is Unicode Cc (C0/C1).
    # TAB, LF and CR are exceptions. There is no CR/LF normalization.
    text = data.decode("utf-8")
    encoded = "".join(c for c in text if c in "\t\n\r"
                      or not (ord(c) < 32 or 0x7f <= ord(c) <= 0x9f)).encode()
    return START + encoded + END if bracketed else encoded


def utf8_chunks(data, limit=65536):
    at = 0
    while at < len(data):
        end = min(len(data), at + limit)
        while end < len(data) and data[end] & 0xc0 == 0x80:
            end -= 1
        if end == at:
            raise ValueError("chunk limit cannot fit one UTF-8 character")
        yield data[at:end]
        at = end


def identity():
    fields = Path("/proc/self/stat").read_text().rsplit(")", 1)[1].split()
    return {"pid": os.getpid(), "start_ticks": int(fields[19])}


def send(channel, value):
    channel.sendall(json.dumps(value, ensure_ascii=False).encode() + b"\n")


def receive(channel):
    data = bytearray()
    while b"\n" not in data:
        part = channel.recv(65536)
        if not part:
            raise EOFError("fixture control connection closed")
        data.extend(part)
        if len(data) > 1024 * 1024:
            raise ValueError("oversized fixture message")
    line, tail = data.split(b"\n", 1)
    if tail:
        raise ValueError("fixture requests must not be pipelined")
    return json.loads(line)


def listener(path):
    channel = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    channel.bind(str(path))
    channel.listen(1)
    return channel


class Capture:
    def __init__(self, expected):
        self.expected = expected
        self.actual = hashlib.sha256()
        self.count = 0
        self.first = self.last = self.mismatch = None
        self.prefix, self.suffix, self.reads = b"", b"", []

    def feed(self, data, timestamp):
        if not data:
            return
        if self.first is None:
            self.first = timestamp
        self.last = timestamp
        expected = self.expected[self.count:self.count + len(data)]
        if self.mismatch is None and expected != data:
            self.mismatch = self.count + next((i for i, (a, b) in enumerate(zip(data, expected))
                                              if a != b), min(len(data), len(expected)))
        self.actual.update(data)
        self.count += len(data)
        self.prefix = (self.prefix + data)[:12]
        self.suffix = (self.suffix + data)[-12:]
        self.reads.append([timestamp, len(data), self.count])

    def report(self):
        return {"expected_bytes": len(self.expected), "received_bytes": self.count,
                "expected_sha256": digest(self.expected), "actual_sha256": self.actual.hexdigest(),
                "byte_exact": self.mismatch is None and self.count == len(self.expected),
                "first_mismatch_offset": self.mismatch, "first_read_ns": self.first,
                "last_payload_read_ns": self.last, "read_events": self.reads,
                "prefix_hex": self.prefix.hex(), "suffix_hex": self.suffix.hex(),
                "has_start_bracket": self.prefix.startswith(START),
                "has_end_bracket": self.suffix.endswith(END)}


def receiver(path):
    import termios
    import tty
    previous = termios.tcgetattr(0)
    server = listener(path)
    channel = None
    try:
        tty.setraw(0)
        channel, _ = server.accept()
        send(channel, {"event": "hello", **identity()})
        state, capture, pending, trailer = "idle", None, bytearray(), bytearray()
        while True:
            ready, _, _ = select.select([0, channel], [], [])
            if channel in ready:
                command = receive(channel)
                action = command["action"]
                if action == "quit":
                    return
                if action == "arm":
                    if state != "idle":
                        raise RuntimeError("previous transfer did not complete its DSR barrier")
                    expected = paste_wire(payload(command["bytes"]), command["bracketed"])
                    capture = Capture(expected)
                    pending.clear()
                    trailer.clear()
                    state = "arming"
                    mode = b"\x1b[?2004h" if command["bracketed"] else b"\x1b[?2004l"
                    os.write(1, b"\x1b[2J\x1b[H" + mode + b"\x1b[6n")
                elif action == "finish":
                    if state != "received":
                        raise RuntimeError("finish requested before expected byte count")
                    state = "barrier"
                    os.write(1, b"\x1b[6n")
                else:
                    raise ValueError("unknown receiver action")
            if 0 not in ready:
                continue
            data = os.read(0, 65536)
            observed = stamp()
            if not data:
                return
            if state == "arming":
                pending.extend(data)
                if pending == DSR:
                    state = "receiving"
                    send(channel, {"event": "armed", "timestamp_ns": observed})
                elif not DSR.startswith(pending):
                    raise RuntimeError("unexpected bytes before arm barrier: " + pending.hex())
            elif state == "receiving":
                remaining = len(capture.expected) - capture.count
                capture.feed(data[:remaining], observed)
                trailer.extend(data[remaining:])
                if capture.count == len(capture.expected):
                    state = "received"
                    send(channel, {"event": "received", "first_read_ns": capture.first,
                                   "last_payload_read_ns": capture.last, "bytes": capture.count})
            elif state in ("received", "barrier"):
                trailer.extend(data)
            else:
                raise RuntimeError("unexpected PTY bytes while receiver idle")
            if len(trailer) > MAX_BYTES + 1024:
                raise RuntimeError("unexpected excessive trailing input")
            if state == "barrier" and trailer.endswith(DSR):
                capture.feed(trailer[:-len(DSR)], observed)
                result = capture.report()
                result.update(event="finished", barrier_read_ns=observed,
                              barrier="fresh DSR response ordered after queued paste bytes")
                send(channel, result)
                state = "idle"
                trailer.clear()
    finally:
        if channel is not None:
            channel.close()
        server.close()
        with contextlib.suppress(OSError):
            termios.tcsetattr(0, termios.TCSANOW, previous)


def clipboard_owner(display_name, path):
    from Xlib import X, Xatom, display, protocol
    connection = display.Display(display_name)
    window = connection.screen().root.create_window(0, 0, 1, 1, 0,
        connection.screen().root_depth, X.InputOutput, X.CopyFromParent,
        event_mask=X.PropertyChangeMask)
    atoms = {name: connection.intern_atom(name) for name in ("CLIPBOARD", "TARGETS", "UTF8_STRING", "INCR")}
    server, channel = listener(path), None
    source, mode, chunk_bytes = b"", "direct", 16381
    transfer, stats, waiting = None, {}, False
    xerrors = []
    connection.set_error_handler(lambda error, _request: xerrors.append(type(error).__name__))

    def notify(event, prop):
        event.requestor.send_event(protocol.event.SelectionNotify(time=event.time,
            requestor=event.requestor, selection=event.selection, target=event.target, property=prop))
        connection.flush()

    def complete():
        nonlocal waiting
        if waiting and stats.get("final_property_delete_seen_ns"):
            stats["x_errors"] = xerrors[:]
            send(channel, {"event": "selection_finished", **stats})
            waiting = False

    try:
        channel, _ = server.accept()
        send(channel, {"event": "hello", **identity(), "window_id": window.id})
        while True:
            # Drain ready X events before blocking. No periodic polling or
            # inter-chunk delay is imposed by this clipboard owner.
            while connection.pending_events():
                event = connection.next_event()
                observed = stamp()
                if event.type == X.SelectionRequest and event.selection == atoms["CLIPBOARD"]:
                    prop = event.property or event.target
                    if event.target == atoms["TARGETS"]:
                        stats["targets_request_seen_ns"] = observed
                        event.requestor.change_property(prop, Xatom.ATOM, 32,
                                                        [atoms["TARGETS"], atoms["UTF8_STRING"]])
                        notify(event, prop)
                        stats["targets_notify_flushed_ns"] = stamp()
                        continue
                    if event.target != atoms["UTF8_STRING"]:
                        notify(event, X.NONE)
                        continue
                    stats["utf8_request_seen_ns"] = observed
                    event.requestor.change_attributes(event_mask=X.PropertyChangeMask)
                    transfer = {"window": event.requestor, "property": prop, "offset": 0, "zero_sent": False}
                    if mode == "incr":
                        event.requestor.change_property(prop, atoms["INCR"], 32, [len(source)])
                        notify(event, prop)
                        stats["incr_header_notify_flushed_ns"] = stamp()
                    else:
                        for index, at in enumerate(range(0, len(source), 32768)):
                            event.requestor.change_property(prop, atoms["UTF8_STRING"], 8,
                                source[at:at + 32768], X.PropModeReplace if index == 0 else X.PropModeAppend)
                        notify(event, prop)
                        stats["payload_notify_flushed_ns"] = stamp()
                        stats["direct_property_requests"] = (len(source) + 32767) // 32768
                elif event.type == X.PropertyNotify and event.state == X.PropertyDelete and transfer:
                    if (event.window.id, event.atom) != (transfer["window"].id, transfer["property"]):
                        continue
                    if mode == "direct" or transfer["zero_sent"]:
                        stats["final_property_delete_seen_ns"] = observed
                        transfer = None
                        complete()
                        continue
                    at = transfer["offset"]
                    piece = source[at:at + chunk_bytes]
                    transfer["window"].change_property(transfer["property"], atoms["UTF8_STRING"], 8, piece)
                    connection.flush()
                    sent = stamp()
                    if piece:
                        transfer["offset"] += len(piece)
                        stats["incr_chunks"].append({"delete_seen_ns": observed, "flushed_ns": sent,
                                                     "offset": at, "bytes": len(piece)})
                    else:
                        transfer["zero_sent"] = True
                        stats["incr_zero_flushed_ns"] = sent
            readable, _, _ = select.select([connection.fileno(), channel], [], [])
            if channel not in readable:
                continue
            command = receive(channel)
            action = command["action"]
            if action == "quit":
                return
            if action == "prepare":
                if transfer is not None or waiting:
                    raise RuntimeError("previous clipboard transfer was not acknowledged")
                source, mode = payload(command["bytes"]), command["mode"]
                chunk_bytes = command.get("chunk_bytes", 16381)
                if mode not in ("direct", "incr") or not 1 <= chunk_bytes <= 65536:
                    raise ValueError("invalid owner transfer mode")
                stats = {"mode": mode, "payload_bytes": len(source), "payload_sha256": digest(source),
                         "incr_chunks": [], "chunk_bytes": chunk_bytes,
                         "utf8_split_boundaries": sum(source[at] & 0xc0 == 0x80
                             for at in range(chunk_bytes, len(source), chunk_bytes))}
                xerrors.clear()
                window.set_selection_owner(atoms["CLIPBOARD"], X.CurrentTime)
                connection.sync()
                assert connection.get_selection_owner(atoms["CLIPBOARD"]).id == window.id
                send(channel, {"event": "prepared", "timestamp_ns": stamp()})
            elif action == "finish":
                waiting = True
                complete()
            else:
                raise ValueError("unknown owner action")
    finally:
        if channel is not None:
            channel.close()
        server.close()
        with contextlib.suppress(Exception):
            window.destroy()
            connection.close()


def rpc(address, request, timeout=10):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
        channel.settimeout(timeout)
        channel.connect("\0" + address[1:])
        send(channel, request)
        value = receive(channel)
    if not value.get("ok"):
        raise RuntimeError(value.get("error", "broker error"))
    return value["result"]


def wait(predicate, label, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.01)  # Startup only; never used by a timed transfer.
    raise TimeoutError(label)


def distribution(values):
    if not values:
        return None
    values = sorted(values)
    return {"count": len(values), "min": values[0], "median": statistics.median(values),
            "p95": values[min(len(values) - 1, int(len(values) * .95))], "max": values[-1]}


def self_test():
    controls = "a\x00\x01\x1b[201~\x7f\u0085\t\r\n中文😀".encode()
    filtered = "a[201~\t\r\n中文😀".encode()
    assert paste_wire(controls, False) == filtered
    assert paste_wire(controls, True) == START + filtered + END
    for size in (1, 2, 63, 65536, 262144, MAX_BYTES):
        data = payload(size)
        assert len(data) == size and data.decode().encode() == data
        wire = paste_wire(data, True)
        parts = list(utf8_chunks(wire))
        assert b"".join(parts) == wire and all(len(p) <= 65536 and p.decode().encode() == p for p in parts)
        capture = Capture(wire)
        for at in range(0, len(wire), 16381):
            capture.feed(wire[at:at + 16381], at + 1)
        assert capture.report()["byte_exact"] and capture.report()["actual_sha256"] == digest(wire)
    wrong = Capture(b"right")
    wrong.feed(b"rixht", 1)
    assert not wrong.report()["byte_exact"] and wrong.report()["first_mismatch_offset"] == 2
    extra = Capture(b"right")
    extra.feed(b"right-extra", 1)
    assert not extra.report()["byte_exact"] and extra.report()["first_mismatch_offset"] == 5
    print(json.dumps({"passed": True, "scope": "pure payload/framing/fragmentation oracle; no GUI or PTY run"}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--sizes", default="65536,262144")
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--trigger", choices=("key", "action"), default="key")
    parser.add_argument("--renderer", choices=("auto", "software", "gpu"), default="auto")
    parser.add_argument("--timeout", type=float, default=30)
    args = parser.parse_args()
    sizes = [int(n) for n in args.sizes.split(",")]
    if not sizes or not all(1 <= n <= MAX_BYTES for n in sizes) or not 1 <= args.repetitions <= 20:
        parser.error("invalid sizes/repetitions")
    binary, output = args.binary.resolve(strict=True), args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    sys.path.insert(0, str(args.repo_root.resolve() / "tools"))
    from isolated_xvfb import start_xvfb
    from measure_runtime import process_stat, same_process, descendants
    from measure_multiwindow import cleanup, owned_processes
    from Xlib import X, XK, display
    from Xlib.ext import xtest
    report = {"schema": 1, "passed": False, "binary_sha256": digest(binary.read_bytes()),
              "test_sha256": digest(Path(__file__).read_bytes()), "measurements": [],
              "clock": "time.monotonic_ns shared by Linux processes",
              "scope": "private Xvfb CLIPBOARD to raw PTY without echo; no shell/ZLE/render throughput claim",
              "limits": ["Selection final_property_delete_seen_ns is owner-observed X event, not internal finish_paste time",
                         "Broker control uses <=64 KiB UTF-8-safe RPC writes; request/JSON overhead is included",
                         "Synthetic Python owner and raw sink scheduling are included; this does not measure a particular desktop clipboard application",
                         "Ready/cleanup polling is outside timed transfers; owner and sink do not sleep per chunk"]}
    started = time.monotonic()
    processes, logs, identities, controls = [], [], {}, []
    server = connection = None
    tag = "ztty-paste-" + secrets.token_hex(12)
    try:
        assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0
        with tempfile.TemporaryDirectory(prefix="ztty-paste-") as temporary:
            root = Path(temporary)
            os.chmod(root, 0o700)
            env = os.environ.copy()
            for key in ("WAYLAND_DISPLAY", "WAYLAND_SOCKET", "LD_PRELOAD", "XAUTHORITY"):
                env.pop(key, None)
            env.update(ZTTY_SOCKET="@" + tag, ZTTY_MEMORY_TAG=tag, XDG_RUNTIME_DIR=str(root),
                       XDG_CONFIG_HOME=str(root / "config"), XDG_CACHE_HOME=str(root / "cache"),
                       XDG_STATE_HOME=str(root / "state"), XMODIFIERS="@im=none", LP_NUM_THREADS="2",
                       LIBGL_ALWAYS_SOFTWARE="1", LC_ALL="C.UTF-8")
            config = root / "ztty.conf"
            config.write_text("font=monospace:pixelsize=16\nwindow-padding=8\nbackground-opacity=100\n"
                              "cursor-blink=false\nrenderer=" + args.renderer + "\n")
            xlog = (output / "xvfb.log").open("wb")
            logs.append(xlog)
            server = start_xvfb(screen="1280x800x24", stderr=xlog, env=env)
            env["DISPLAY"] = server.display
            connection = display.Display(server.display)
            report["display_ownership"] = server.evidence

            def spawn(command, name):
                log = (output / (name + ".log")).open("wb")
                logs.append(log)
                process = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT,
                                           start_new_session=True)
                processes.append(process)
                identities[process.pid] = process_stat(process.pid)["start_ticks"]
                return process

            def connect(path):
                wait(path.exists, "fixture socket did not appear")
                channel = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                channel.settimeout(args.timeout)
                channel.connect(str(path))
                controls.append(channel)
                hello = receive(channel)
                assert hello["event"] == "hello"
                identities[hello["pid"]] = hello["start_ticks"]
                return channel, hello

            sink_path, owner_path = root / "receiver.sock", root / "owner.sock"
            spawn([str(binary), "--backend", "x11", "--name", "paste-stage", "--config", str(config),
                   "--geometry", "80x24", "-e", sys.executable, str(Path(__file__).resolve()),
                   "--receiver", str(sink_path)], "terminal")
            sink, sink_hello = connect(sink_path)
            info = rpc(env["ZTTY_SOCKET"], {"op": "info", "session": "paste-stage"})
            assert info["pid"] == sink_hello["pid"] and info["backend"] == "x11"
            broker = info["broker_pid"]
            assert Path(f"/proc/{broker}/exe").resolve() == binary
            assert ("ZTTY_MEMORY_TAG=" + tag).encode() in Path(f"/proc/{broker}/environ").read_bytes().split(b"\0")
            identities[broker] = process_stat(broker)["start_ticks"]
            window = connection.create_resource_object("window", info["window_id"])
            wait(lambda: window.get_attributes().map_state == X.IsViewable, "terminal window did not map")
            window.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            spawn([sys.executable, str(Path(__file__).resolve()), "--owner", server.display, str(owner_path)], "owner")
            owner, owner_hello = connect(owner_path)
            report["identities"] = {"session": info, "receiver": sink_hello, "selection_owner": owner_hello}

            for repetition in range(args.repetitions):
                for size in sizes:
                    for bracketed in (False, True):
                        source = payload(size)
                        expected = paste_wire(source, bracketed)
                        batches = [chunk.decode() for chunk in utf8_chunks(expected)]
                        for transport in ("direct", "incr", "broker"):
                            if transport != "broker":
                                send(owner, {"action": "prepare", "bytes": size, "mode": transport})
                                assert receive(owner)["event"] == "prepared"
                            send(sink, {"action": "arm", "bytes": size, "bracketed": bracketed})
                            assert receive(sink)["event"] == "armed"
                            row = {"repetition": repetition, "selection_bytes": size, "wire_bytes": len(expected),
                                   "bracketed": bracketed, "transport": transport, "trigger": args.trigger,
                                   "source_sha256": digest(source), "expected_wire_sha256": digest(expected)}
                            row["trigger_ns"] = stamp()
                            if transport == "broker":
                                requests = []
                                for text in batches:
                                    before = stamp()
                                    ack = rpc(env["ZTTY_SOCKET"], {"op": "write", "session": "paste-stage", "data": text})
                                    after = stamp()
                                    assert ack["written"] == len(text.encode())
                                    requests.append([before, after, ack["written"]])
                                row["broker_write_requests"] = requests
                            elif args.trigger == "action":
                                rpc(env["ZTTY_SOCKET"], {"op": "action", "session": "paste-stage", "action": "paste"})
                            else:
                                codes = [connection.keysym_to_keycode(XK.string_to_keysym(key))
                                         for key in ("Control_L", "Shift_L", "v")]
                                assert all(codes)
                                for code in codes:
                                    xtest.fake_input(connection, X.KeyPress, code)
                                for code in reversed(codes):
                                    xtest.fake_input(connection, X.KeyRelease, code)
                                connection.flush()
                            row["trigger_submission_finished_ns"] = stamp()
                            received = receive(sink)
                            assert received["event"] == "received", received
                            if transport != "broker":
                                send(owner, {"action": "finish"})
                                selection = receive(owner)
                                assert selection["event"] == "selection_finished" and not selection["x_errors"]
                                row["selection"] = selection
                                chunks = selection["incr_chunks"]
                                row["owner_chunk_interval_us"] = distribution(
                                    [(b["flushed_ns"] - a["flushed_ns"]) / 1000 for a, b in zip(chunks, chunks[1:])])
                                row["owner_chunk_service_us"] = distribution(
                                    [(chunk["flushed_ns"] - chunk["delete_seen_ns"]) / 1000 for chunk in chunks])
                                final_flush = selection.get("incr_zero_flushed_ns", selection.get("payload_notify_flushed_ns"))
                                row["selection_timing_ms"] = {
                                    "trigger_to_targets_request_seen": (selection["targets_request_seen_ns"] - row["trigger_ns"]) / 1e6,
                                    "utf8_request_seen_to_final_flush": (final_flush - selection["utf8_request_seen_ns"]) / 1e6,
                                    "trigger_to_final_delete_observation": (selection["final_property_delete_seen_ns"] - row["trigger_ns"]) / 1e6,
                                    "final_flush_to_first_pty_read": (received["first_read_ns"] - final_flush) / 1e6,
                                    "final_delete_observation_to_first_pty_read":
                                        (received["first_read_ns"] - selection["final_property_delete_seen_ns"]) / 1e6}
                            send(sink, {"action": "finish"})
                            result = receive(sink)
                            assert result["event"] == "finished"
                            row["receiver"] = result
                            row["passed"] = (result["byte_exact"] and result["actual_sha256"] == digest(expected)
                                             and result["has_start_bracket"] == bracketed
                                             and result["has_end_bracket"] == bracketed)
                            row["timing_ms"] = {
                                "trigger_to_first_read": (result["first_read_ns"] - row["trigger_ns"]) / 1e6,
                                "trigger_to_last_read": (result["last_payload_read_ns"] - row["trigger_ns"]) / 1e6,
                                "first_to_last_read": (result["last_payload_read_ns"] - result["first_read_ns"]) / 1e6}
                            report["measurements"].append(row)
                            if not row["passed"]:
                                raise AssertionError("raw PTY bytes differ from the paste policy")
            report["passed"] = True
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
    finally:
        for channel in controls:
            with contextlib.suppress(OSError):
                send(channel, {"action": "quit"})
            channel.close()
        found = owned_processes(binary, tag)
        for pid in found:
            identities.update(descendants(pid))
        identities.update(found)
        for process in processes:
            if process.pid in identities and same_process(process.pid, identities[process.pid]):
                identities.update(descendants(process.pid))
        report["remaining_owned_pids"] = cleanup(identities)
        for process in processes:
            with contextlib.suppress(subprocess.TimeoutExpired):
                process.wait(timeout=1)
        if connection is not None:
            connection.close()
        if server is not None:
            server.close()
        for log in logs:
            log.close()
    try:
        report["binary_sha256_after"] = digest(binary.read_bytes())
        report["binary_unchanged"] = report["binary_sha256_after"] == report["binary_sha256"]
    except OSError:
        report["binary_unchanged"] = False
    report["passed"] = report["passed"] and report["binary_unchanged"] and not report["remaining_owned_pids"]
    report["elapsed_seconds"] = time.monotonic() - started
    report["summary"] = []
    for size in sizes:
        for bracketed in (False, True):
            for transport in ("direct", "incr", "broker"):
                rows = [r for r in report["measurements"] if (r["selection_bytes"], r["bracketed"], r["transport"])
                        == (size, bracketed, transport)]
                report["summary"].append({"selection_bytes": size, "bracketed": bracketed, "transport": transport,
                    "runs": len(rows), "trigger_to_last_read_ms": distribution([r["timing_ms"]["trigger_to_last_read"] for r in rows])})
    (output / "validation.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "measurements": len(report["measurements"]),
                      "report": str(output / "validation.json")}))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        self_test()
    elif len(sys.argv) == 3 and sys.argv[1] == "--receiver":
        receiver(sys.argv[2])
    elif len(sys.argv) == 4 and sys.argv[1] == "--owner":
        clipboard_owner(sys.argv[2], sys.argv[3])
    else:
        raise SystemExit(main())
