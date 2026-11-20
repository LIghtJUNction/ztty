#!/usr/bin/env python3
"""Exercise same-PID runtime reload on owned PTYs, X11 windows and sockets.

No installed binary or existing terminal is replaced. With --target, the test
uses two supplied runtime builds. Without it, two ELF copies with distinct
ignored suffixes exercise executable replacement using one reload-capable build.
--expect-unsupported with --client NEW_ELF checks that an older broker refuses
reload while its live command remains usable.
"""
import argparse
import base64
import contextlib
import ctypes
import fcntl
import hashlib
import json
import os
from pathlib import Path
import select
import secrets
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import time
import traceback
import tty

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from isolated_xvfb import start_xvfb
from measure_runtime import process_stat, same_process
from measure_multiwindow import cleanup


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def send(channel, value):
    channel.sendall(json.dumps(value, ensure_ascii=False).encode() + b"\n")


def receive(stream):
    data = stream.readline(1024 * 1024 + 1)
    if not data or not data.endswith(b"\n") or len(data) > 1024 * 1024:
        raise RuntimeError("missing or oversized control reply")
    return json.loads(data)


def wait(predicate, label, seconds=10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.01)
    raise TimeoutError(label)


def child(path, name):
    """Keep mutable progress and the same controlling PTY across parent exec."""
    tty.setraw(0)
    channel = socket.socket(socket.AF_UNIX)
    channel.connect(path)
    stream = channel.makefile("rb")
    ledger, received = bytearray(), bytearray()
    nonce = secrets.token_hex(16)
    sequence, remaining, period, next_write = 0, 0, 0.0, 0.0

    def write(data):
        pending = memoryview(data)
        while pending:
            count = os.write(1, pending)
            if count <= 0:
                raise OSError("child PTY write made no progress")
            ledger.extend(pending[:count])
            pending = pending[count:]

    def state():
        st = os.fstat(0)
        size = os.get_terminal_size(0)
        return {"ok": True, "name": name, "pid": os.getpid(),
                "parent_pid": os.getppid(), "nonce": nonce,
                "start_ticks": process_stat(os.getpid())["start_ticks"],
                "tty": os.ttyname(0), "tty_rdev": st.st_rdev,
                "cols": size.columns, "rows": size.lines,
                "sequence": sequence, "remaining": remaining,
                "input_hex": received.hex(), "output_bytes": len(ledger)}

    send(channel, state())
    write(("\x1b[?25lHELLO " + name + "\r\n").encode())
    while True:
        timeout = max(0, next_write - time.monotonic()) if remaining else None
        ready, _, _ = select.select([0, channel], [], [], timeout)
        if 0 in ready:
            data = os.read(0, 4096)
            if not data:
                return
            received.extend(data)
        if channel in ready:
            request = receive(stream)
            op = request["op"]
            if op == "emit":
                write(base64.b64decode(request["base64"], validate=True))
                send(channel, state())
            elif op == "stream":
                remaining = int(request["count"])
                period = float(request["period"])
                next_write = time.monotonic()
                send(channel, state())
            elif op == "stop-stream":
                remaining = 0
                send(channel, state())
            elif op == "status":
                send(channel, state())
            elif op == "ledger":
                send(channel, dict(state(), base64=base64.b64encode(ledger).decode()))
            elif op == "quit":
                send(channel, state())
                return
            else:
                raise ValueError(op)
        if remaining and time.monotonic() >= next_write:
            sequence += 1
            write(f"STREAM {sequence:06d} mutable-progress\r\n".encode())
            remaining -= 1
            next_write = time.monotonic() + period


def main():
    if len(sys.argv) == 4 and sys.argv[1] == "--child":
        child(sys.argv[2], sys.argv[3])
        return 0
    from Xlib import X, XK, display
    from Xlib.ext import xtest
    from PIL import Image

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--target", type=Path)
    parser.add_argument("--renderer", choices=("software", "gpu"), default="software")
    parser.add_argument("--client", type=Path,
                        help="reload-capable CLI, useful when checking an old broker")
    parser.add_argument("--expect-unsupported", action="store_true")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    target = args.target.resolve(strict=True) if args.target else binary
    client_source = args.client.resolve(strict=True) if args.client else binary
    if args.expect_unsupported and not args.client:
        parser.error("--expect-unsupported requires a reload-capable --client")
    output = args.output.resolve()
    if output.exists():
        parser.error("refusing to overwrite a previous reload result")
    output.parent.mkdir(parents=True, exist_ok=True)
    screenshots = (args.screenshots or output.with_suffix("")).resolve()
    if screenshots.exists():
        parser.error("refusing to reuse a screenshot directory")
    screenshots.mkdir(parents=True)
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("PR_SET_CHILD_SUBREAPER failed")
    report = {"ok": False, "checks": [], "reloads": [], "failures": [], "partial_parser_cases": [],
              "rejected_target_cases": [],
              "screenshots": {}, "scope": "Linux same-PID exec, real PTY and private X11 pixels",
              "binary_sha256": digest(binary), "target_sha256": digest(target),
              "test_script_sha256": digest(__file__),
              "identity_fixture_suffixes": args.target is None,
              "expected_unsupported": args.expect_unsupported, "renderer": args.renderer,
              "helper_sha256": {name: digest(ROOT / "tools" / name) for name in
                  ("isolated_xvfb.py", "measure_runtime.py", "measure_multiwindow.py")}}
    identities, launchers, channels, streams = {}, [], {}, {}
    children, cursors, collected = {}, {}, {}
    connection = server = listener = None
    broker = None

    with tempfile.TemporaryDirectory(prefix="ztty-runtime-reload-") as temporary:
        directory = Path(temporary)
        log_path = directory / "terminal.log"
        with log_path.open("wb") as log:
            try:
                before = directory / "before-ztty"
                after = directory / "after-ztty"
                if args.target is None:
                    before.write_bytes(binary.read_bytes() + b"\nztty-reload-fixture-before\n")
                    after.write_bytes(binary.read_bytes() + b"\nztty-reload-fixture-after\n")
                else:
                    before.write_bytes(binary.read_bytes())
                    after.write_bytes(target.read_bytes())
                before.chmod(0o755)
                after.chmod(0o755)
                assert digest(before) != digest(after), "supply distinct builds or omit --target"
                report["images"] = {"before": digest(before), "after": digest(after)}
                client = client_source if args.client else before
                server = start_xvfb(screen="1800x1100x24", dpi=96, stderr=log)
                connection = display.Display(server.display)
                report["display_ownership"] = server.evidence
                env = dict(os.environ, DISPLAY=server.display, XMODIFIERS="@im=none",
                           LC_ALL="C.UTF-8", LIBGL_ALWAYS_SOFTWARE="1",
                           XDG_RUNTIME_DIR=temporary, XDG_CONFIG_HOME=str(directory / "config-home"),
                           ZTTY_SOCKET="@" + directory.name, ZTTY_RELOAD_TEST=directory.name)
                for name in ("WAYLAND_DISPLAY", "LD_PRELOAD", "ZTTY_SESSION"):
                    env.pop(name, None)
                config = directory / "appearance.conf"
                config.write_text("font = monospace:size=11\nbackground = #000000\n"
                                  "background-opacity = 100\nbackground-blur = false\n"
                                  "cursor-blink = false\nwindow-padding = 0\n"
                                  + ("" if args.expect_unsupported else f"renderer = {args.renderer}\n"))
                control = directory / "children.sock"
                listener = socket.socket(socket.AF_UNIX)
                listener.bind(str(control))
                listener.listen(4)
                listener.settimeout(15)

                def rpc(request):
                    with socket.socket(socket.AF_UNIX) as channel:
                        channel.settimeout(15)
                        channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                        send(channel, request)
                        reply = receive(channel.makefile("rb"))
                    assert reply.get("ok") is True, reply
                    return reply["result"]

                def info(name):
                    return rpc({"op": "info", "session": name})

                def command(name, op, **fields):
                    send(channels[name], {"op": op, **fields})
                    result = receive(streams[name])
                    assert result.get("ok") is True, result
                    return result

                def emit(name, data):
                    return command(name, "emit", base64=base64.b64encode(data).decode())

                def read(name):
                    return rpc({"op": "read", "session": name,
                                "cursor": cursors[name], "max_bytes": 65536})

                def drain(name):
                    for _ in range(16):
                        page = read(name)
                        assert not page["truncated"], (name, page)
                        block = base64.b64decode(page["output_base64"], validate=True)
                        assert page["next_cursor"] == cursors[name] + len(block), (name, page)
                        collected[name].extend(block)
                        cursors[name] = page["next_cursor"]
                        if not page["has_more"] or not block:
                            return page
                    raise AssertionError("unexpected unbounded output stream")

                def verify_output(name):
                    expected = base64.b64decode(command(name, "ledger")["base64"], validate=True)
                    wait(lambda: (drain(name), len(collected[name]) >= len(expected))[1],
                         name + " output drain")
                    assert bytes(collected[name]) == expected, (name, len(collected[name]), len(expected))
                    return {"bytes": len(expected), "sha256": hashlib.sha256(expected).hexdigest(),
                            "cursor": cursors[name], "truncated": False}

                def start(name, extra=()):
                    nonlocal broker
                    process = subprocess.Popen(
                        [str(before), "--name", name, "--config", str(config),
                         "--geometry", "80x20", "--scrollback", "500", *extra,
                         "-e", sys.executable, str(Path(__file__).resolve()),
                         "--child", str(control), name],
                        env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
                    launchers.append(process)
                    identities[process.pid] = process_stat(process.pid)["start_ticks"]
                    channel, _ = listener.accept()
                    channel.settimeout(15)
                    stream = channel.makefile("rb")
                    hello = receive(stream)
                    assert hello["name"] == name
                    channels[name], streams[name], children[name] = channel, stream, hello
                    cursors[name], collected[name] = 0, bytearray()
                    identities[hello["pid"]] = hello["start_ticks"]
                    current = wait(lambda: next((s for s in rpc({"op": "list"})["sessions"]
                                                if s["name"] == name), None), name + " session")
                    if broker is None:
                        broker = current["broker_pid"]
                        identities[broker] = process_stat(broker)["start_ticks"]
                        assert digest(f"/proc/{broker}/exe") == digest(before)
                    assert current["broker_pid"] == broker == hello["parent_pid"]
                    assert current["pid"] == hello["pid"]
                    wait(lambda: name in drain(name)["screen"], name + " initial screen")
                    return current

                def kernel_identity(name):
                    value = info(name)
                    pid = value["pid"]
                    fd = os.open(f"/proc/{pid}/fd/0", os.O_RDONLY | os.O_NOCTTY | os.O_CLOEXEC)
                    try:
                        size = struct.unpack("HHHH", fcntl.ioctl(fd, termios.TIOCGWINSZ, b"\0" * 8))
                        device = os.fstat(fd).st_rdev
                    finally:
                        os.close(fd)
                    return {"metadata": {key: value[key] for key in
                                ("id", "created_at", "uri", "name", "origin", "pid", "broker_pid",
                                 "cols", "rows", "input_owner", "read_only", "attachment", "visible")},
                            "child_start_ticks": process_stat(pid)["start_ticks"],
                            "tty_rdev": device, "tty_size": list(size),
                            "nonce": command(name, "status")["nonce"]}

                def same_children(expected):
                    assert same_process(broker, identities[broker]), "broker PID was replaced"
                    for name, identity in expected.items():
                        assert kernel_identity(name) == identity, (name, identity, kernel_identity(name))
                    assert all(process.poll() is None for process in launchers), "a launcher lost its wait peer"

                def cli_reload(path, expected_success=True, *, error_contains=None, timeout=30):
                    started = time.monotonic()
                    result = subprocess.run([str(client), "reload", "--binary", str(path)], env=env,
                                            capture_output=True, text=True, timeout=timeout)
                    entry = {"target_sha256": digest(path) if Path(path).is_file() else None,
                             "exit_code": result.returncode, "stdout": result.stdout,
                             "stderr": result.stderr, "elapsed_ms": (time.monotonic() - started) * 1000}
                    accepted = result.returncode == 0
                    entry["accepted"] = accepted
                    if expected_success is True or (expected_success is None and accepted):
                        report["reloads"].append(entry)
                        assert result.returncode == 0, entry
                        value = json.loads(result.stdout)
                        if value.get("ok") is True and "result" in value:
                            value = value["result"]
                        assert value.get("reloaded") is True and value["broker_pid"] == broker, value
                        assert value["sessions"] == len(children), value
                        assert digest(f"/proc/{broker}/exe") == digest(path)
                    else:
                        report["failures"].append(entry)
                        # A timeout, signal or unrelated client crash is not a
                        # successful refusal from the still-running broker.
                        assert result.returncode == 1, entry
                        assert not result.stdout.strip(), entry
                        assert result.stderr.startswith("ztty: "), entry
                        if error_contains is not None:
                            assert error_contains in result.stderr, entry
                    return entry

                def key(name, names):
                    view = connection.create_resource_object("window", info(name)["window_id"])
                    view.set_input_focus(X.RevertToParent, X.CurrentTime)
                    connection.sync()
                    codes = [connection.keysym_to_keycode(XK.string_to_keysym(value)) for value in names]
                    assert all(codes)
                    for code in codes:
                        xtest.fake_input(connection, X.KeyPress, code)
                    for code in reversed(codes):
                        xtest.fake_input(connection, X.KeyRelease, code)
                    connection.sync()

                def picture(name):
                    view = connection.create_resource_object("window", info(name)["window_id"])
                    assert view.get_attributes().map_state == X.IsViewable
                    connection.sync()
                    geometry = view.get_geometry()
                    pixels = view.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xFFFFFFFF)
                    return Image.frombytes("RGB", (geometry.width, geometry.height), pixels.data, "raw", "BGRX")

                def capture(name, label, expected=None):
                    if expected is None:
                        image = picture(name)
                    else:
                        def matching():
                            current = picture(name)
                            return current if current.size == expected.size and current.tobytes() == expected.tobytes() else None
                        image = wait(matching, label + " restored pixels")
                    path = screenshots / (label + ".png")
                    image.save(path)
                    report["screenshots"][label] = {"path": str(path), "sha256": digest(path),
                                                     "width": image.width, "height": image.height}
                    return image

                alpha = "reload-alpha"
                beta = "reload-beta"
                gamma = "reload-gamma"
                start(alpha)
                report["broker_pid"] = broker
                report["broker_start_ticks"] = identities[broker]

                if args.expect_unsupported:
                    expected = {alpha: kernel_identity(alpha)}
                    old_image = digest(f"/proc/{broker}/exe")
                    cli_reload(after, False)
                    assert digest(f"/proc/{broker}/exe") == old_image
                    same_children(expected)
                    rpc({"op": "write", "session": alpha, "data": "LEGACY-STILL-LIVE"})
                    wait(lambda: b"LEGACY-STILL-LIVE" in bytes.fromhex(command(alpha, "status")["input_hex"]),
                         "legacy broker still accepts input")
                    emit(alpha, b"LEGACY-OUTPUT-AFTER-REFUSAL\r\n")
                    report["output"] = {alpha: verify_output(alpha)}
                    report["checks"].append("older broker rejects runtime reload and retains the exact command, PTY and executable")
                    report["ok"] = True
                else:
                    start(beta, ["--split-right", alpha])
                    start(gamma, ["--tab", alpha])
                    wait(lambda: info(gamma)["visible"] and not info(beta)["visible"], "two-tab split layout")
                    history = b"\x1b[?25l" + b"".join(f"HISTORY {i:04d} retained-line\r\n".encode() for i in range(140))
                    emit(beta, history)
                    emit(gamma, b"\x1b[?25l\x1b[2J\x1b[HPRIMARY-BEFORE-ALT\r\n")
                    wait(lambda: "PRIMARY-BEFORE-ALT" in drain(gamma)["screen"], "primary screen preparation")
                    primary = drain(gamma)["screen"]
                    emit(gamma, ("\x1b[?1049h\x1b[?25l\x1b[?1h\x1b[?2004h"
                                 "\x1b[48;2;15;28;39m\x1b[2J\x1b[HALTERNATE-SURVIVES\r\n"
                                 "\x1b[31m中文 e\u0301 👩🏽‍💻\x1b[0m").encode())
                    wait(lambda: "ALTERNATE-SURVIVES" in drain(gamma)["screen"], "alternate screen preparation")
                    alt_screen = drain(gamma)["screen"]
                    # Wait on actual distinctive pixels, not a parser response.
                    def alternate_pixels_ready():
                        colors = picture(gamma).getcolors(10_000) or []
                        return any(count > 100 and color == (15, 28, 39) for count, color in colors)
                    wait(alternate_pixels_ready, "alternate framebuffer")
                    alt_image = capture(gamma, "alternate-before")
                    for name in children:
                        verify_output(name)
                    layout = rpc({"op": "save-layout", "session": alpha})
                    assert len(layout["tabs"]) == 2 and len(layout["nodes"]) == 4, layout
                    expected = {name: kernel_identity(name) for name in children}
                    report["initial_identities"] = expected
                    report["initial_layout"] = layout

                    bad = directory / "invalid-executable"
                    bad.write_bytes(b"not an ELF runtime\n")
                    bad.chmod(0o755)
                    noexec = directory / "not-executable"
                    noexec.write_bytes(after.read_bytes())
                    noexec.chmod(0o600)
                    zero_exit = Path("/bin/true").resolve(strict=True)
                    assert zero_exit.read_bytes().startswith(b"\x7fELF"), "use a real ELF /bin/true"
                    assert subprocess.run([str(zero_exit)], capture_output=True, timeout=5).returncode == 0
                    fifo = directory / "fifo-runtime"
                    os.mkfifo(fifo, 0o600)
                    rejected_targets = [
                        ("missing", directory / "missing-runtime", "cannot open reload executable"),
                        ("not-executable", noexec, None),
                        ("invalid-elf", bad, "reload target must be a native ztty executable"),
                        ("zero-exit-without-protocol", zero_exit,
                         "new executable does not support validated runtime reload"),
                        ("fifo", fifo, "reload target must be a native ztty executable"),
                    ]
                    for label, path, error_contains in rejected_targets:
                        old_image = digest(f"/proc/{broker}/exe")
                        output_before = {name: verify_output(name) for name in children}
                        screens_before = {name: drain(name)["screen"] for name in children}
                        progress_before = {name: command(name, "status") for name in children}
                        failure = cli_reload(path, False, error_contains=error_contains, timeout=10)
                        assert digest(f"/proc/{broker}/exe") == old_image
                        same_children(expected)
                        assert rpc({"op": "save-layout", "session": alpha}) == layout
                        for name in children:
                            assert drain(name)["screen"] == screens_before[name], (label, name)
                            progress_after = command(name, "status")
                            for field in ("sequence", "remaining", "input_hex", "output_bytes"):
                                assert progress_after[field] == progress_before[name][field], (label, name, field)
                        # Exercise both PTY directions after every refusal,
                        # including the valid ELF that exits 0 without an ACK.
                        marker = ("INPUT-AFTER-REFUSAL-" + label).encode()
                        expected_input = bytes.fromhex(progress_before[alpha]["input_hex"]) + marker
                        rpc({"op": "write", "session": alpha, "data": marker.decode()})
                        wait(lambda: bytes.fromhex(command(alpha, "status")["input_hex"]) == expected_input,
                             label + " refusal retains input delivery")
                        emit(alpha, ("OUTPUT-AFTER-REFUSAL-" + label + "\r\n").encode())
                        output_after = {name: verify_output(name) for name in children}
                        same_children(expected)
                        report["rejected_target_cases"].append({
                            "case": label, "target": str(path), "elapsed_ms": failure["elapsed_ms"],
                            "outcome": "rejected_without_state_loss", "broker_pid": broker,
                            "unchanged_image_sha256": old_image, "input_continuity": True,
                            "screen_and_layout_unchanged": True,
                            "output_before": output_before, "output_after": output_after,
                        })
                    capture(gamma, "alternate-after-refusals", alt_image)
                    rpc({"op": "write", "session": alpha, "data": "INPUT-BEFORE-RELOAD"})
                    wait(lambda: b"INPUT-BEFORE-RELOAD" in bytes.fromhex(command(alpha, "status")["input_hex"]), "pre-reload input")
                    # Keep emitting past the reload response. A short finite
                    # stream could finish before exec and falsely pass this
                    # check; this bounded stream is stopped explicitly below.
                    command(alpha, "stream", count=1024, period=0.05)
                    wait(lambda: command(alpha, "status")["sequence"] > 0, "long command progress")
                    progress_at_reload = command(alpha, "status")
                    # A genuinely parked broker connection must survive exec,
                    # not merely a new RPC opened after restoration.
                    parked_cursor = cursors[gamma]
                    with socket.socket(socket.AF_UNIX) as parked:
                        parked.settimeout(15)
                        parked.connect("\0" + env["ZTTY_SOCKET"][1:])
                        send(parked, {"op": "read", "session": gamma,
                                      "cursor": parked_cursor, "max_bytes": 65536,
                                      "wait_ms": 30000})
                        assert not select.select([parked], [], [], 0.03)[0], "read was not parked"
                        cli_reload(after)
                        assert not select.select([parked], [], [], 0)[0], "parked read closed or completed during exec"
                        marker = b"\x1b[?25l"
                        emit(gamma, marker)
                        with parked.makefile("rb") as reply_stream:
                            reply = receive(reply_stream)
                        assert reply.get("ok") is True, reply
                        page = reply["result"]
                        assert page["timed_out"] is False and not page["truncated"], page
                        assert base64.b64decode(page["output_base64"], validate=True) == marker, page
                        assert page["next_cursor"] == parked_cursor + len(marker), page
                    report["checks"].append("an already parked read socket survives exec and replies with the exact later output and cursor")
                    progress_on_return = command(alpha, "status")
                    assert progress_on_return["remaining"] > 0, "output finished before reload completed"
                    same_children(expected)
                    assert rpc({"op": "save-layout", "session": alpha}) == layout
                    assert drain(gamma)["screen"] == alt_screen
                    capture(gamma, "alternate-after", alt_image)
                    rpc({"op": "write", "session": alpha, "data": "INPUT-AFTER-RELOAD"})
                    wait(lambda: b"INPUT-AFTER-RELOAD" in bytes.fromhex(command(alpha, "status")["input_hex"]), "post-reload input")
                    wait(lambda: command(alpha, "status")["sequence"] > progress_on_return["sequence"],
                         "the same output stream progresses after exec")
                    stopped = command(alpha, "stop-stream")
                    assert stopped["sequence"] > progress_at_reload["sequence"] and stopped["remaining"] == 0
                    report["stream_progress"] = {
                        "before_reload": progress_at_reload["sequence"],
                        "on_reload_return": progress_on_return["sequence"],
                        "still_pending_on_return": progress_on_return["remaining"],
                        "stopped_after_reload": stopped["sequence"],
                    }
                    report["checks"].append("new executable runs in the same broker PID with every PTY, child nonce, session identity and launcher wait peer retained")
                    report["checks"].append("long-running command progress and raw output cursors continue through exec without loss or duplication")
                    report["checks"].append("two tabs, split topology, alternate screen text and actual framebuffer pixels survive")

                    before_keys = bytes.fromhex(command(gamma, "status")["input_hex"])
                    key(gamma, ["Up"])
                    wait(lambda: bytes.fromhex(command(gamma, "status")["input_hex"])[len(before_keys):] == b"\x1bOA",
                         "application cursor mode after reload")
                    report["checks"].append("application cursor mode remains active for real XTEST keyboard input")

                    key(gamma, ["Control_L", "Shift_L", "Prior"])
                    wait(lambda: info(alpha)["visible"] and info(beta)["visible"] and not info(gamma)["visible"], "restored split tab")
                    view = connection.create_resource_object("window", info(beta)["window_id"])
                    view.set_input_focus(X.RevertToParent, X.CurrentTime)
                    view.warp_pointer(20, 20)
                    connection.sync()
                    live_image = picture(beta)
                    for _ in range(3):
                        xtest.fake_input(connection, X.ButtonPress, 4)
                        xtest.fake_input(connection, X.ButtonRelease, 4)
                    connection.sync()
                    wait(lambda: picture(beta).tobytes() != live_image.tobytes(), "real scrollback movement")
                    time.sleep(0.05)
                    history_image = capture(beta, "history-before")
                    history_layout = rpc({"op": "save-layout", "session": alpha})
                    expected = {name: kernel_identity(name) for name in children}
                    cli_reload(before)
                    same_children(expected)
                    assert rpc({"op": "save-layout", "session": alpha}) == history_layout
                    capture(beta, "history-after", history_image)
                    report["checks"].append("scrollback contents and the user's scrolled viewport are retained across reload")

                    current = before
                    partials = [
                        ("utf8", b"\x1b[2J\x1b[HUTF8:" + b"\xe4\xb8", b"\xad\r\n", "UTF8:中"),
                        ("csi", b"\x1b[2J\x1b[H\x1b[3", b"1mCSI-RED\x1b[0m\r\n", "CSI-RED"),
                        ("osc", b"\x1b]2;reload-fragmented-", b"title\x1b\\", None),
                    ]
                    for label, prefix, suffix, expected_text in partials:
                        verify_output(beta)
                        emitted = emit(beta, prefix)
                        def prefix_consumed():
                            page = drain(beta)
                            return (page["has_more"] and not page["output_base64"]) if label == "utf8" else cursors[beta] == emitted["output_bytes"]
                        wait(prefix_consumed, label + " prefix consumed without an in-band barrier")
                        expected = {name: kernel_identity(name) for name in children}
                        requested = after if current == before else before
                        old_image = digest(f"/proc/{broker}/exe")
                        outcome = cli_reload(requested, expected_success=None)
                        if outcome["accepted"]:
                            current = requested
                        else:
                            assert digest(f"/proc/{broker}/exe") == old_image
                        same_children(expected)
                        emit(beta, suffix)
                        verify_output(beta)
                        if expected_text:
                            wait(lambda: expected_text in drain(beta)["screen"], label + " parser continuation")
                        else:
                            wait(lambda: info(beta)["title"] == "reload-fragmented-title", "OSC parser continuation")
                        report["partial_parser_cases"].append({
                            "parser": label,
                            "outcome": "reloaded_and_continued" if outcome["accepted"] else "rejected_without_state_loss",
                            "suffix_verified": True,
                            "before_image_sha256": old_image,
                            "after_image_sha256": digest(f"/proc/{broker}/exe"),
                        })
                    report["checks"].append("unfinished UTF-8, CSI and OSC either migrate or explicitly refuse without state loss; each actual outcome is recorded separately")

                    emit(gamma, b"\x1b[?1049l")
                    wait(lambda: drain(gamma)["screen"] == primary, "primary buffer restored after alternate exit")
                    report["checks"].append("exiting the restored alternate screen returns the original primary buffer")
                    report["output"] = {name: verify_output(name) for name in children}
                    report["final_identities"] = {name: kernel_identity(name) for name in children}
                    report["final_running_image_sha256"] = digest(f"/proc/{broker}/exe")
                    report["checks"].append("missing, non-executable, invalid ELF, zero-exit /bin/true and FIFO targets are explicitly refused while the old executable, PID, PTYs, screen, layout and exact output remain intact")
                    report["ok"] = True
            except Exception as error:
                report["error"] = repr(error)
                report["traceback"] = traceback.format_exc()
            finally:
                for name, channel in channels.items():
                    with contextlib.suppress(Exception):
                        send(channel, {"op": "quit"})
                        receive(streams[name])
                    with contextlib.suppress(Exception):
                        streams[name].close()
                    channel.close()
                for name in children:
                    if "rpc" in locals():
                        with contextlib.suppress(Exception):
                            rpc({"op": "close", "session": name})
                for process in launchers:
                    with contextlib.suppress(subprocess.TimeoutExpired):
                        process.wait(timeout=2)
                if connection is not None:
                    connection.close()
                if listener is not None:
                    listener.close()
                # Discover only processes carrying this fixture's private marker.
                # Known identities remain tracked after an exec changes environ.
                marker = ("ZTTY_RELOAD_TEST=" + directory.name).encode()
                for entry in Path("/proc").iterdir():
                    if entry.name.isdigit():
                        with contextlib.suppress(OSError, ProcessLookupError, KeyError, ValueError):
                            if marker in (entry / "environ").read_bytes().split(b"\0"):
                                pid = int(entry.name)
                                identities.setdefault(pid, process_stat(pid)["start_ticks"])
                if server is not None:
                    identities.pop(server.process.pid, None)
                report["remaining_owned_pids"] = cleanup(identities)
                if server is not None:
                    server.close()
                    report["xserver_exit_code"] = server.process.returncode
                if report["remaining_owned_pids"]:
                    report["ok"] = False
                log.flush()
                report["terminal_log"] = log_path.read_text(errors="replace")
                output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "checks": len(report["checks"]),
                      "reloads": len(report["reloads"]), "output": str(output),
                      "error": report.get("error")}, ensure_ascii=False))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
