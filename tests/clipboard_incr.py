#!/usr/bin/env python3
"""Deterministic private-Xvfb regression for a double INCR header deletion.

The test-only preload gate pauses after XGetWindowProperty already deleted the
header. A separate owner installs the first chunk and waits for its deletion.
It then pauses until the first actual payload read is observed. This forces the
old extra XDeleteProperty to erase that unread chunk; fixed code reads it intact.
These intentional gates test ordering and integrity, never paste performance.
"""
import argparse
import contextlib
import ctypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import sys
import tempfile
import time
import traceback

DSR = b"\x1b[1;1R"
CHUNK = 16363  # The first boundary splits a UTF-8 code point in SEED.
SEED = "block{中文😀}\x00\x01\x07\x1b[201~\x7f\u0085\t\r\n".encode()


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def payload(size):
    raw = (SEED * (size // len(SEED) + 1))[:size]
    valid = raw.decode("utf8", "ignore").encode()
    return valid + b"x" * (size - len(valid))


def helper(path):
    spec = importlib.util.spec_from_file_location("paste_measure", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def send(channel, value):
    channel.sendall(json.dumps(value).encode() + b"\n")


def receive(stream):
    line = stream.readline(1024 * 1024 + 1)
    if not line or len(line) > 1024 * 1024 or not line.endswith(b"\n"):
        raise RuntimeError("missing or oversized fixture message")
    return json.loads(line)


def receiver(measure_path, path):
    import tty
    measure = helper(measure_path)
    tty.setraw(0)
    with measure.listener(path) as server:
        channel, _ = server.accept()
        with channel, channel.makefile("rb") as stream:
            send(channel, {"event": "hello", **measure.identity()})
            state, capture, trailer = "idle", None, bytearray()
            while True:
                ready, _, _ = select.select([0, channel], [], [])
                if channel in ready:
                    request = receive(stream)
                    op = request["op"]
                    if op == "quit":
                        return
                    if op == "arm":
                        assert state == "idle"
                        capture = measure.Capture(measure.paste_wire(payload(request["size"]), request["bracketed"]))
                        state = "arming"
                        trailer.clear()
                        mode = b"\x1b[?2004h" if request["bracketed"] else b"\x1b[?2004l"
                        os.write(1, b"\x1b[2J\x1b[H" + mode + b"\x1b[6n")
                    elif op == "finish":
                        assert state == "receiving"
                        state = "barrier"
                        trailer.clear()
                        os.write(1, b"\x1b[6n")
                    else:
                        raise ValueError(op)
                if 0 not in ready:
                    continue
                data = os.read(0, 65536)
                if not data:
                    return
                if state in ("arming", "barrier"):
                    trailer.extend(data)
                    if state == "arming":
                        assert DSR.startswith(trailer), trailer.hex()
                    if trailer.endswith(DSR):
                        if state == "arming":
                            assert trailer == DSR
                            state = "receiving"
                            send(channel, {"event": "armed"})
                        else:
                            capture.feed(trailer[:-len(DSR)], time.monotonic_ns())
                            send(channel, {"event": "finished", **capture.report()})
                            state = "idle"
                        trailer.clear()
                elif state == "receiving":
                    capture.feed(data, time.monotonic_ns())
                else:
                    raise RuntimeError("unexpected input outside this fixture's transfer")


def owner(display_name, measure_path, path):
    from Xlib import X, Xatom, display, protocol
    measure = helper(measure_path)
    connection = display.Display(display_name)
    window = connection.screen().root.create_window(0, 0, 1, 1, 0,
        connection.screen().root_depth, X.InputOutput, X.CopyFromParent,
        event_mask=X.PropertyChangeMask)
    atoms = {name: connection.intern_atom(name) for name in ("CLIPBOARD", "TARGETS", "UTF8_STRING", "INCR")}
    errors = []
    connection.set_error_handler(lambda error, _request: errors.append(type(error).__name__))
    source, transfer, stats = b"", None, {}
    released = paused = waiting = False

    with measure.listener(path) as server:
        channel, _ = server.accept()
        with channel, channel.makefile("rb") as stream:
            send(channel, {"event": "hello", **measure.identity(), "atoms": atoms})

            def notify(event, prop):
                event.requestor.send_event(protocol.event.SelectionNotify(time=event.time,
                    requestor=event.requestor, selection=event.selection,
                    target=event.target, property=prop))
                connection.flush()

            def piece():
                at = transfer["offset"]
                data = source[at:at + CHUNK]
                transfer["window"].change_property(transfer["property"], atoms["UTF8_STRING"], 8, data)
                connection.flush()
                if data:
                    transfer["offset"] += len(data)
                    stats["chunks"] += 1
                    stats["bytes_sent"] += len(data)
                    if at == 0:
                        # This round trip proves the first chunk exists before
                        # allowing XGetWindowProperty(header) to return in ztty.
                        connection.sync()
                        send(channel, {"event": "first_chunk_installed", "bytes": len(data),
                            "window": transfer["window"].id, "property": transfer["property"],
                            "sha256": hashlib.sha256(data).hexdigest()})
                else:
                    transfer["zero"] = True

            while True:
                while connection.pending_events():
                    event = connection.next_event()
                    if event.type == X.SelectionRequest and event.selection == atoms["CLIPBOARD"]:
                        prop = event.property or event.target
                        if event.target == atoms["TARGETS"]:
                            event.requestor.change_property(prop, Xatom.ATOM, 32,
                                                            [atoms["TARGETS"], atoms["UTF8_STRING"]])
                            notify(event, prop)
                        elif event.target == atoms["UTF8_STRING"]:
                            event.requestor.change_attributes(event_mask=X.PropertyChangeMask)
                            transfer = {"window": event.requestor, "property": prop, "offset": 0, "zero": False}
                            event.requestor.change_property(prop, atoms["INCR"], 32, [len(source)])
                            notify(event, prop)
                        else:
                            notify(event, X.NONE)
                    elif event.type == X.PropertyNotify and event.state == X.PropertyDelete and transfer:
                        if (event.window.id, event.atom) != (transfer["window"].id, transfer["property"]):
                            continue
                        stats["deletes"] += 1
                        if transfer["zero"]:
                            stats["complete"] = True
                            transfer = None
                            if waiting:
                                send(channel, {"event": "complete", **stats, "x_errors": errors})
                                waiting = False
                        elif transfer["offset"] == CHUNK and not released:
                            # No timer or per-chunk sleep. Keep the property
                            # absent until the first data read is observed.
                            paused = True
                            stats["paused_after_first_ack"] = True
                        else:
                            piece()
                ready, _, _ = select.select([connection.fileno(), channel], [], [])
                if channel not in ready:
                    continue
                request = receive(stream)
                op = request["op"]
                if op == "quit":
                    break
                if op == "prepare":
                    assert transfer is None
                    source = payload(request["size"])
                    released = paused = waiting = False
                    stats = {"chunks": 0, "bytes_sent": 0, "deletes": 0, "complete": False,
                             "paused_after_first_ack": False,
                             "utf8_split_boundaries": sum(source[at] & 0xc0 == 0x80
                                 for at in range(CHUNK, len(source), CHUNK))}
                    window.set_selection_owner(atoms["CLIPBOARD"], X.CurrentTime)
                    connection.sync()
                    assert connection.get_selection_owner(atoms["CLIPBOARD"]).id == window.id
                    send(channel, {"event": "prepared"})
                elif op == "continue":
                    released = True
                    if paused:
                        paused = False
                        piece()
                    send(channel, {"event": "continued"})
                elif op == "finish":
                    if stats["complete"]:
                        send(channel, {"event": "complete", **stats, "x_errors": errors})
                    else:
                        waiting = True
                elif op == "abort":
                    transfer = None
                    waiting = False
                    send(channel, {"event": "aborted", **stats, "x_errors": errors})
                else:
                    raise ValueError(op)
    window.destroy()
    connection.close()


class Events:
    def __init__(self, path):
        self.fd = os.open(path, os.O_RDWR | os.O_NONBLOCK)
        self.buffer = bytearray()

    def next(self, timeout=10):
        end = time.monotonic() + timeout
        while b"\n" not in self.buffer:
            left = end - time.monotonic()
            if left <= 0 or not select.select([self.fd], [], [], left)[0]:
                raise TimeoutError("test-only Xlib scheduling gate did not respond")
            self.buffer.extend(os.read(self.fd, 8192))
            if len(self.buffer) > 65536:
                raise RuntimeError("oversized gate record")
        line, _, rest = self.buffer.partition(b"\n")
        self.buffer = bytearray(rest)
        return json.loads(line)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, required=True)
    parser.add_argument("--measure-script", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--hook", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--sizes", default="65536,262144")
    parser.add_argument("--expect-race", action="store_true")
    args = parser.parse_args()
    repo, binary = args.repo_root.resolve(strict=True), args.binary.resolve(strict=True)
    measure_path, hook = args.measure_script.resolve(strict=True), args.hook.resolve(strict=True)
    output = args.output.resolve()
    if output.exists():
        parser.error("refusing to overwrite an existing result directory")
    output.mkdir(parents=True)
    sizes = [int(value) for value in args.sizes.split(",")]
    if not sizes or any(not CHUNK < size <= 4 * 1024 * 1024 for size in sizes):
        parser.error("sizes must be greater than one chunk and at most 4 MiB")
    measure = helper(measure_path)
    sys.path.insert(0, str(repo / "tools"))
    from isolated_xvfb import start_xvfb
    from measure_runtime import process_stat, same_process, descendants
    from measure_multiwindow import cleanup
    assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0
    report = {"passed": False, "expected_race": args.expect_race,
              "scope": "intentional ordering regression on private Xvfb and owned PTYs",
              "performance_benchmark": False, "binary_sha256": digest(binary),
              "test_sha256": digest(__file__), "hook_sha256": digest(hook),
              "measure_script_sha256": digest(measure_path), "cases": []}
    server = None
    try:
        with tempfile.TemporaryDirectory(prefix="ztty-incr-order-") as temporary:
            root = Path(temporary)
            env_base = dict(os.environ, LC_ALL="C.UTF-8", XMODIFIERS="@im=none",
                            XDG_RUNTIME_DIR=str(root), XDG_CONFIG_HOME=str(root / "config"),
                            LIBGL_ALWAYS_SOFTWARE="1", LP_NUM_THREADS="2")
            for key in ("WAYLAND_DISPLAY", "WAYLAND_SOCKET", "LD_PRELOAD", "XAUTHORITY", "ZTTY_SESSION"):
                env_base.pop(key, None)
            with (output / "xvfb.log").open("wb") as xlog:
                server = start_xvfb(screen="1280x800x24", env=env_base, stderr=xlog)
                report["display_ownership"] = server.evidence
                env_base["DISPLAY"] = server.display
                for size in sizes:
                    for bracketed in (False, True):
                        label = f"{size}-bracketed-{int(bracketed)}"
                        work = root / label
                        work.mkdir(mode=0o700)
                        events_path, release_path = work / "events", work / "release"
                        os.mkfifo(events_path, 0o600)
                        os.mkfifo(release_path, 0o600)
                        events = Events(events_path)
                        release = os.open(release_path, os.O_RDWR | os.O_NONBLOCK)
                        identities, processes, controls = {}, [], []
                        row = {"case": label, "passed": False, "size": size, "bracketed": bracketed,
                               "hook_events": [], "payload_sha256": hashlib.sha256(payload(size)).hexdigest()}
                        report["cases"].append(row)
                        tag = root.name + "-" + label
                        env = dict(env_base, ZTTY_SOCKET="@" + tag, ZTTY_INCR_TEST_TAG=tag)

                        def spawn(command, process_env, log):
                            process = subprocess.Popen(command, env=process_env, stdout=log, stderr=log,
                                                       start_new_session=True)
                            processes.append(process)
                            identities[process.pid] = process_stat(process.pid)["start_ticks"]
                            return process

                        def connect(path):
                            measure.wait(path.exists, "fixture socket did not appear")
                            channel = socket.socket(socket.AF_UNIX)
                            channel.settimeout(15)
                            channel.connect(str(path))
                            stream = channel.makefile("rb")
                            controls.append((channel, stream))
                            hello = receive(stream)
                            identities[hello["pid"]] = hello["start_ticks"]
                            return channel, stream, hello

                        def gate_until(kind):
                            while True:
                                event = events.next()
                                row["hook_events"].append(event)
                                assert event["pid"] == broker, event
                                assert event["event"] != "gate_failed", event
                                if event["event"] == kind:
                                    return event

                        with (output / (label + ".log")).open("wb") as log:
                            try:
                                owner_path, sink_path = work / "owner.sock", work / "sink.sock"
                                spawn([sys.executable, str(Path(__file__).resolve()), "--owner", server.display,
                                       str(measure_path), str(owner_path)], env, log)
                                owner_channel, owner_stream, owner_identity = connect(owner_path)
                                preload_env = dict(env, LD_PRELOAD=str(hook),
                                    ZTTY_INCR_TEST_EVENTS=str(events_path), ZTTY_INCR_TEST_RELEASE=str(release_path),
                                    ZTTY_INCR_TEST_ATOM=str(owner_identity["atoms"]["INCR"]))
                                config = work / "appearance.conf"
                                config.write_text("font=monospace:pixelsize=16\nrenderer=software\nwindow-padding=0\ncursor-blink=false\n")
                                spawn([str(binary), "--backend", "x11", "--name", "incr-order", "--config", str(config),
                                       "--geometry", "80x24", "-e", "/usr/bin/env", "-u", "LD_PRELOAD", sys.executable,
                                       str(Path(__file__).resolve()), "--receiver", str(measure_path), str(sink_path)], preload_env, log)
                                sink, sink_stream, sink_identity = connect(sink_path)
                                info = measure.rpc(env["ZTTY_SOCKET"], {"op": "info", "session": "incr-order"})
                                broker = info["broker_pid"]
                                assert info["pid"] == sink_identity["pid"]
                                assert Path(f"/proc/{broker}/exe").resolve() == binary
                                assert digest(f"/proc/{broker}/exe") == report["binary_sha256"]
                                assert ("ZTTY_INCR_TEST_TAG=" + tag).encode() in Path(f"/proc/{broker}/environ").read_bytes().split(b"\0")
                                identities[broker] = process_stat(broker)["start_ticks"]
                                row["identities"] = {"broker": broker, "receiver": sink_identity, "owner": owner_identity}
                                send(owner_channel, {"op": "prepare", "size": size})
                                assert receive(owner_stream)["event"] == "prepared"
                                send(sink, {"op": "arm", "size": size, "bracketed": bracketed})
                                assert receive(sink_stream)["event"] == "armed"
                                measure.rpc(env["ZTTY_SOCKET"], {"op": "action", "session": "incr-order", "action": "paste"})
                                header = gate_until("header_deleted_by_get")
                                first = receive(owner_stream)
                                assert first["event"] == "first_chunk_installed"
                                assert first["window"] == header["window"] and first["property"] == header["property"]
                                assert first["bytes"] == CHUNK and first["sha256"] == hashlib.sha256(payload(size)[:CHUNK]).hexdigest()
                                row["first_chunk_before_header_return"] = first
                                assert os.write(release, b"R") == 1
                                data = gate_until("first_data_read")
                                duplicate = any(item["event"] == "delete_before_first_data_read" for item in row["hook_events"])
                                row["duplicate_delete_observed"] = duplicate
                                if args.expect_race:
                                    assert duplicate and data["type"] == 0 and data["count"] == 0, row
                                    send(owner_channel, {"op": "abort"})
                                    row["owner"] = receive(owner_stream)
                                else:
                                    assert not duplicate and data["type"] == owner_identity["atoms"]["UTF8_STRING"]
                                    assert data["count"] == CHUNK and data["code"] == 0
                                    send(owner_channel, {"op": "continue"})
                                    assert receive(owner_stream)["event"] == "continued"
                                    send(owner_channel, {"op": "finish"})
                                    row["owner"] = receive(owner_stream)
                                    assert row["owner"]["event"] == "complete" and not row["owner"]["x_errors"]
                                    assert row["owner"]["chunks"] > 1 and row["owner"]["bytes_sent"] == size
                                    assert row["owner"]["utf8_split_boundaries"] > 0
                                send(sink, {"op": "finish"})
                                result = receive(sink_stream)
                                assert result["event"] == "finished"
                                row["receiver"] = result
                                expected = measure.paste_wire(payload(size), bracketed)
                                row["filtered_payload_bytes"] = len(measure.paste_wire(payload(size), False))
                                row["control_bytes_removed"] = size - row["filtered_payload_bytes"]
                                assert row["control_bytes_removed"] > 0
                                if args.expect_race:
                                    assert result["received_bytes"] == 0 and not result["byte_exact"]
                                    row["outcome"] = "first chunk deleted before read; original paste rejected without PTY bytes"
                                else:
                                    assert result["byte_exact"] and result["actual_sha256"] == hashlib.sha256(expected).hexdigest()
                                    assert result["has_start_bracket"] == bracketed and result["has_end_bracket"] == bracketed
                                    row["outcome"] = "all chunks preserved, UTF-8 complete, controls filtered and framing exact"
                                row["passed"] = True
                            except Exception as error:
                                row["error"] = repr(error)
                                row["traceback"] = traceback.format_exc()
                            finally:
                                for channel, stream in controls:
                                    with contextlib.suppress(Exception):
                                        send(channel, {"op": "quit"})
                                    stream.close()
                                    channel.close()
                                # A broker can double-fork before its info reply
                                # is recorded. PR_SET_CHILD_SUBREAPER adopts that
                                # orphan, so it may no longer be a descendant of
                                # the tracked launcher. Admit only this fixture's
                                # exact random environment tag, then retain its
                                # start ticks for cleanup's PID-reuse protection.
                                marker = ("ZTTY_INCR_TEST_TAG=" + tag).encode()
                                for entry in Path("/proc").iterdir():
                                    if entry.name.isdigit():
                                        with contextlib.suppress(OSError, KeyError, ValueError):
                                            pid = int(entry.name)
                                            before = process_stat(pid)["start_ticks"]
                                            if marker not in (entry / "environ").read_bytes().split(b"\0"):
                                                continue
                                            after = process_stat(pid)["start_ticks"]
                                            if before == after:
                                                identities.setdefault(pid, before)
                                for pid, started in list(identities.items()):
                                    if same_process(pid, started):
                                        identities.update(descendants(pid))
                                row["remaining_owned_pids"] = cleanup(identities)
                                for process in processes:
                                    with contextlib.suppress(subprocess.TimeoutExpired):
                                        process.wait(timeout=1)
                                os.close(events.fd)
                                os.close(release)
                                row["passed"] = row["passed"] and not row["remaining_owned_pids"]
                        print(json.dumps({key: row[key] for key in ("case", "passed", "outcome", "error") if key in row}), flush=True)
                report["passed"] = all(row["passed"] for row in report["cases"])
    except Exception as error:
        report["error"] = repr(error)
        report["traceback"] = traceback.format_exc()
    finally:
        if server is not None:
            server.close()
            report["xserver_exit_code"] = server.process.returncode
    report["binary_unchanged"] = digest(binary) == report["binary_sha256"]
    report["passed"] = report["passed"] and report["binary_unchanged"]
    (output / "validation.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--receiver":
        receiver(sys.argv[2], sys.argv[3])
    elif len(sys.argv) == 5 and sys.argv[1] == "--owner":
        owner(sys.argv[2], sys.argv[3], sys.argv[4])
    else:
        raise SystemExit(main())
