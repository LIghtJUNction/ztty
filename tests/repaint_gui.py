#!/usr/bin/env python3
"""Validate repaint deadlines, interaction fairness and synchronized frames on a private Xvfb."""
import argparse
import contextlib
import ctypes
import hashlib
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import sys
import tempfile
import time
import tty

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from isolated_xvfb import start_xvfb
from measure_runtime import process_stat, same_process
from measure_multiwindow import cleanup
from benchmark_terminals import output_payload

COLS, ROWS, PADDING = 120, 32, 16
BASE = (11, 22, 33)
KEY_COLOR = (171, 41, 83)
RPC_COLOR = (23, 153, 97)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def frame(color, label=""):
    r, g, b = color
    return f"\x1b[?25l\x1b[48;2;{r};{g};{b}m\x1b[2J\x1b[H{label}\x1b[0m"


def write_all(data):
    data = memoryview(data)
    while data:
        count = os.write(1, data[:65536])
        if count <= 0:
            raise RuntimeError("PTY write made no progress")
        data = data[count:]


def send(channel, data):
    channel.sendall(json.dumps(data).encode() + b"\n")


def receive(channel):
    data = bytearray()
    while b"\n" not in data:
        chunk = channel.recv(65536)
        if not chunk:
            raise RuntimeError("control connection closed")
        data.extend(chunk)
        if len(data) > 4 * 1024 * 1024:
            raise RuntimeError("oversized control message")
    return json.loads(data.split(b"\n", 1)[0])


def child(path, title):
    tty.setraw(0)
    channel = socket.socket(socket.AF_UNIX)
    channel.connect(path)
    write_all((frame(BASE) + "\x1b]2;" + title + "\x07").encode())
    send(channel, {"pid": os.getpid()})
    incoming = bytearray()
    hold_echo = False
    pending_echo = None
    flood_until, flood_frames = 0.0, 0
    while True:
        ready, _, _ = select.select([0, channel], [], [], 0 if flood_until > time.monotonic() else None)
        if 0 in ready:
            data = os.read(0, 4096)
            if not data:
                return
            incoming.extend(data)
            if len(incoming) > 65536:
                raise RuntimeError("unexpected excessive test input")
            for key, color in ((b"q", KEY_COLOR), (b"z", RPC_COLOR)):
                if key in data:
                    if hold_echo:
                        pending_echo = color
                    else:
                        write_all(frame(color, "interaction echoed").encode())
        if channel in ready:
            request = receive(channel)
            op = request["op"]
            if op == "write":
                write_all(request["text"].encode())
            elif op == "flood":
                flood_until = time.monotonic() + request["seconds"]
            elif op == "stop":
                flood_until = 0.0
            elif op == "hold":
                hold_echo = True
            elif op == "release":
                hold_echo = False
                if pending_echo is not None:
                    write_all(frame(pending_echo, "delayed interaction echoed").encode())
                    pending_echo = None
            elif op == "status":
                pass
            elif op == "quit":
                send(channel, {"ok": True})
                return
            else:
                raise RuntimeError("unexpected child operation")
            send(channel, {"ok": True, "input_hex": incoming.hex(),
                           "flood_active": flood_until > time.monotonic(),
                           "flood_frames": flood_frames})
        if flood_until > time.monotonic():
            flood_frames += 1
            color = (32 + flood_frames * 17 % 192, 24 + flood_frames * 29 % 192, 48 + flood_frames * 7 % 160)
            # Full rows repeatedly change; the frontend must render during output,
            # not postpone every new frame until the producer becomes quiet.
            text = frame(color)
            text += "".join(f"\x1b[{row};1Hframe {flood_frames:08d} " + "X" * 95
                            for row in range(1, ROWS + 1))
            write_all(text.encode())


def wait(predicate, label, timeout=3.0, interval=0.001):
    started = time.monotonic()
    while time.monotonic() - started < timeout:
        result = predicate()
        if result:
            return result, time.monotonic() - started
        time.sleep(interval)
    raise AssertionError("timed out: " + label)


def switches(pid):
    values = {}
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        if line.startswith(("voluntary_ctxt_switches:", "nonvoluntary_ctxt_switches:")):
            key, value = line.split(":")
            values[key] = int(value)
    return values


def add_owned_children(identities, roots):
    pending, seen = list(roots), set()
    while pending:
        pid = pending.pop()
        if pid in seen:
            continue
        seen.add(pid)
        if pid != os.getpid() and not same_process(pid, identities[pid]):
            continue
        try:
            children = Path(f"/proc/{pid}/task/{pid}/children").read_text().split()
        except OSError:
            continue
        for value in children:
            child_pid = int(value)
            if child_pid not in identities:
                with contextlib.suppress(OSError, ValueError):
                    identities[child_pid] = process_stat(child_pid)["start_ticks"]
            if child_pid in identities:
                pending.append(child_pid)


def main():
    from PIL import Image
    from Xlib import X, XK, display
    from Xlib.ext import xtest

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("artifacts/repaint-gui.json"))
    parser.add_argument("--screenshots", type=Path, default=Path("artifacts/repaint-gui"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    report = {
        "ok": False, "binary": str(binary), "binary_sha256": digest(binary),
        "checks": [], "timings_ms": {}, "screenshots": {},
        "scope": "Real PTY/XTEST and X11 pixels on owned Xvfb, not physical display latency",
        "source_sha256": {name: digest(ROOT / name) for name in (
            "src/repaint.rs", "src/session.rs", "crates/zterm/src/terminal.rs",
            "tests/repaint_gui.py", "tools/isolated_xvfb.py", "tools/benchmark_terminals.py",
            "tools/measure_runtime.py", "tools/measure_multiwindow.py")},
    }
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("PR_SET_CHILD_SUBREAPER failed")
    xserver = connection = listener = None
    identities, views, launchers = {}, [], []
    with tempfile.TemporaryDirectory(prefix="ztty-repaint-gui-") as temporary:
        directory = Path(temporary)
        log_path = directory / "stderr.log"
        with log_path.open("wb") as log:
            try:
                xserver = start_xvfb(screen="2400x1000x24", dpi=96, stderr=log, timeout=10)
                report["display_ownership"] = xserver.evidence
                connection = display.Display(xserver.display)
                title_atom = connection.intern_atom("_NET_WM_NAME")
                pid_atom = connection.intern_atom("_NET_WM_PID")
                config = directory / "ztty.conf"
                config.write_text("font = monospace:size=11\nbackground = #000000\n"
                                  "background-opacity = 100\nbackground-blur = false\n"
                                  "cursor-blink = false\nwindow-padding = 16\n")
                env = dict(os.environ, DISPLAY=xserver.display, XMODIFIERS="@im=none",
                           LC_ALL="C.UTF-8", XDG_RUNTIME_DIR=temporary,
                           XDG_CONFIG_HOME=str(directory / "config"), XDG_CACHE_HOME=str(directory / "cache"),
                           XDG_STATE_HOME=str(directory / "state"), XDG_DATA_HOME=str(directory / "data"),
                           ZTTY_SOCKET="@ztty-repaint-" + directory.name,
                           ZTTY_REPAINT_TEST=directory.name)
                for key in ("WAYLAND_DISPLAY", "LD_PRELOAD", "ZTTY_X11_DIAGNOSTIC_CONTROL"):
                    env.pop(key, None)
                socket_path = directory / "control.sock"
                listener = socket.socket(socket.AF_UNIX)
                listener.bind(str(socket_path))
                listener.listen(2)
                listener.settimeout(10)
                for index, name in enumerate(("repaint-a", "repaint-b")):
                    title = directory.name + "-" + name
                    command = [str(binary), "--name", name, "--config", str(config),
                               "--geometry", "120x32", "-e", sys.executable, str(Path(__file__).resolve()),
                               "--child", str(socket_path), title]
                    launcher = subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL,
                                                stderr=log, start_new_session=True)
                    launchers.append(launcher)
                    identities[launcher.pid] = process_stat(launcher.pid)["start_ticks"]
                    channel, _ = listener.accept()
                    channel.settimeout(10)
                    hello = receive(channel)
                    child_pid = hello["pid"]
                    identities[child_pid] = process_stat(child_pid)["start_ticks"]

                    def find_window():
                        for window in connection.screen().root.query_tree().children:
                            value = window.get_full_property(title_atom, 0)
                            if value is not None and bytes(value.value) == title.encode():
                                return window
                        return None
                    window, _ = wait(find_window, "new owned window")
                    broker = int(window.get_full_property(pid_atom, 0).value[0])
                    marker = ("ZTTY_REPAINT_TEST=" + directory.name).encode()
                    if marker not in Path(f"/proc/{broker}/environ").read_bytes().split(b"\0"):
                        raise AssertionError("window PID does not belong to this test")
                    if Path(f"/proc/{broker}/exe").resolve() != binary or digest(Path(f"/proc/{broker}/exe")) != report["binary_sha256"]:
                        raise AssertionError("window owner binary mismatch")
                    identities[broker] = process_stat(broker)["start_ticks"]
                    window.configure(x=index * 1200, y=0)
                    geometry = window.get_geometry()
                    cw, ch = (geometry.width - PADDING * 2) // COLS, (geometry.height - PADDING * 2) // ROWS
                    views.append({"name": name, "window": window, "channel": channel,
                                  "broker": broker, "child": child_pid, "marker": 0,
                                  "point": (PADDING + (COLS - 2) * cw + 1,
                                            PADDING + (ROWS - 2) * ch + 1)})
                connection.sync()
                a, b = views
                assert a["broker"] == b["broker"], "sessions must share one broker"
                broker = a["broker"]
                report["broker"] = {**process_stat(broker), "exe_sha256": report["binary_sha256"]}
                report["owned_processes"] = identities.copy()

                def command(view, op, **fields):
                    send(view["channel"], {"op": op, **fields})
                    response = receive(view["channel"])
                    assert response["ok"], response
                    return response

                def pixel(view):
                    x, y = view["point"]
                    data = view["window"].get_image(x, y, 1, 1, X.ZPixmap, 0xffffffff).data
                    return Image.frombytes("RGB", (1, 1), data, "raw", "BGRX").getpixel((0, 0))

                def visible(view, color, label, timeout=0.2):
                    _, elapsed = wait(lambda: pixel(view) == color, label, timeout)
                    report["timings_ms"][label] = elapsed * 1000
                    return elapsed

                def emit(view, text):
                    view["marker"] += 1
                    title = directory.name + "-" + view["name"] + "-" + str(view["marker"])
                    command(view, "write", text=text + "\x1b]2;" + title + "\x07")
                    def parsed():
                        value = view["window"].get_full_property(title_atom, 0)
                        return value is not None and bytes(value.value) == title.encode()
                    wait(parsed, "parse marker (not a frame fence)")

                def capture(view, name):
                    geometry = view["window"].get_geometry()
                    data = view["window"].get_image(0, 0, geometry.width, geometry.height,
                                                   X.ZPixmap, 0xffffffff).data
                    image = Image.frombytes("RGB", (geometry.width, geometry.height), data, "raw", "BGRX")
                    args.screenshots.mkdir(parents=True, exist_ok=True)
                    path = args.screenshots / (name + ".png")
                    image.save(path)
                    report["screenshots"][name] = {"path": str(path), "sha256": digest(path),
                                                   "rgb_sha256": hashlib.sha256(image.tobytes()).hexdigest()}

                def rpc(request):
                    with socket.socket(socket.AF_UNIX) as channel:
                        channel.settimeout(0.5)
                        channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                        send(channel, request)
                        response = receive(channel)
                        assert response["ok"], response
                        return response["result"]

                visible(a, BASE, "initial-a")
                visible(b, BASE, "initial-b")
                payload = output_payload()
                report["payload"] = {"bytes": len(payload), "sha256": hashlib.sha256(payload).hexdigest()}
                final = (77, 99, 121)
                emit(a, payload.decode("ascii") + frame(final, "QUIET FINAL FRAME"))
                # Do not send PTY input, Expose or resize here. XGetImage alone
                # cannot wake the broker to repair a missing final-frame deadline.
                visible(a, final, "quiet-final-frame")
                capture(a, "quiet-final-frame")
                report["checks"].append("bulk output ending in silence publishes the final real pixels without an external wakeup")

                time.sleep(0.05)
                before = switches(broker)
                time.sleep(0.35)
                after = switches(broker)
                delta = {key: after[key] - before[key] for key in before}
                report["idle_context_switches"] = {"seconds": 0.35, "before": before, "after": after, "delta": delta}
                assert delta["voluntary_ctxt_switches"] <= 4, delta
                report["checks"].append("idle hidden-cursor windows have no recurring 8 ms repaint wakeup")

                b["window"].set_input_focus(X.RevertToParent, X.CurrentTime)
                connection.sync()
                time.sleep(0.03)
                command(b, "hold")
                before_flood = pixel(a)
                command(a, "flood", seconds=2.0)
                def first_flood_frame():
                    color = pixel(a)
                    return color if color != before_flood else None
                first_color, _ = wait(first_flood_frame, "first visible flood frame", 0.2)
                observed = {first_color}
                started = time.monotonic()
                while time.monotonic() - started < 0.1:
                    observed.add(pixel(a))
                    time.sleep(0.003)
                keycode = connection.keysym_to_keycode(XK.string_to_keysym("q"))
                assert keycode
                xtest.fake_input(connection, X.KeyPress, keycode)
                xtest.fake_input(connection, X.KeyRelease, keycode)
                connection.sync()
                wait(lambda: b"q" in bytes.fromhex(command(b, "status")["input_hex"]), "real XTEST input", 0.3)
                # Force a repaint of the old screen before allowing the child to
                # echo: interaction priority must survive that old-frame paint.
                x, y = b["point"]
                b["window"].clear_area(x=x, y=y, width=1, height=1, exposures=True)
                connection.flush()
                visible(b, BASE, "old-frame-expose", 0.1)
                command(b, "release")
                visible(b, KEY_COLOR, "xtest-visible-echo")
                rpc_started = time.monotonic()
                result = rpc({"op": "write", "session": "repaint-b", "data": "z"})
                report["timings_ms"]["broker-write-during-flood"] = (time.monotonic() - rpc_started) * 1000
                assert result["written"] == 1
                visible(b, RPC_COLOR, "rpc-visible-echo")
                assert len(rpc({"op": "list"})["sessions"]) == 2
                observed.add(pixel(a))
                assert command(a, "status")["flood_active"], "interactive/RPC work finished only after flood ended"
                until = time.monotonic() + 0.08
                while time.monotonic() < until:
                    observed.add(pixel(a))
                    time.sleep(0.003)
                assert len(observed) >= 2, "continuous output postponed its own repaint forever"
                report["flood_visible_colors"] = [list(color) for color in sorted(observed)]
                command(a, "stop")
                capture(a, "fairness-flood")
                capture(b, "fairness-interaction")
                report["checks"].append("one shared broker updates the flooding view while XTEST echo and broker input render in the other view before flood ends")
                report["checks"].append("a force/expose paint of the old screen does not lose the subsequent interactive response")

                emit(a, frame(BASE, "COMMITTED"))
                visible(a, BASE, "sync-initial")
                sync_color = (147, 31, 61)
                sync_started = time.monotonic()
                emit(a, "\x1b[?2026h" + frame(sync_color, "HELD"))
                assert pixel(a) == BASE, "synchronized partial frame leaked"
                x, y = a["point"]
                a["window"].clear_area(x=x, y=y, width=1, height=1, exposures=True)
                connection.flush()
                visible(a, BASE, "sync-expose-retained", 0.08)
                assert time.monotonic() - sync_started < 0.14, "sync observation missed its held interval"
                emit(a, "\x1b[?2026l")
                visible(a, sync_color, "sync-explicit-release")
                capture(a, "sync-release")
                report["checks"].append("synchronized output and Expose retain the committed pixels until explicit release")

                timeout_color = (41, 111, 173)
                sync_started = time.monotonic()
                emit(a, "\x1b[?2026h" + frame(timeout_color, "TIMEOUT"))
                assert pixel(a) == sync_color
                while time.monotonic() - sync_started < 0.10:
                    assert pixel(a) == sync_color, "synchronized timeout ended too early"
                    time.sleep(0.002)
                emit(a, "\x1b[?2026h")
                remaining = 0.22 - (time.monotonic() - sync_started)
                visible(a, timeout_color, "sync-quiet-timeout", max(0.001, remaining))
                elapsed = time.monotonic() - sync_started
                report["timings_ms"]["sync-total-from-first-begin"] = elapsed * 1000
                assert 0.12 <= elapsed < 0.22, elapsed
                capture(a, "sync-timeout")
                report["checks"].append("a quiet synchronized frame commits at the 150 ms timeout; repeated begin does not restart it")

                same_feed = (181, 103, 43)
                emit(a, "\x1b[?2026h" + frame(same_feed, "SAME FEED") + "\x1b[?2026l")
                visible(a, same_feed, "sync-same-feed")
                capture(a, "sync-same-feed")
                report["checks"].append("a synchronized begin/end in one small write publishes its complete final pixels")
                assert digest(binary) == report["binary_sha256"]
                report["ok"] = True
            except Exception as error:
                report["error"] = repr(error)
                import traceback
                report["traceback"] = traceback.format_exc()
                if "capture" in locals():
                    for view in views:
                        with contextlib.suppress(Exception):
                            capture(view, "failure-" + view["name"])
            finally:
                for view in views:
                    with contextlib.suppress(Exception):
                        send(view["channel"], {"op": "quit"})
                    view["channel"].close()
                if listener is not None:
                    listener.close()
                # This test is a child subreaper; traverse only our known process
                # trees and our own adopted children, never scan other processes.
                add_owned_children(identities, [os.getpid(), *identities])
                if xserver is not None:
                    identities.pop(xserver.process.pid, None)
                remaining = cleanup(identities)
                for launcher in launchers:
                    with contextlib.suppress(subprocess.TimeoutExpired):
                        launcher.wait(timeout=1)
                if connection is not None:
                    connection.close()
                if xserver is not None:
                    number = xserver.number
                    xserver.close()
                    report["display_cleanup"] = {
                        "exit_code": xserver.process.returncode,
                        "pid_lock_exists": Path(f"/tmp/.X{number}-lock").exists(),
                        "socket_exists": Path(f"/tmp/.X11-unix/X{number}").exists(),
                        "lease_exists": Path(f"/tmp/ztty-xvfb-{number}.lease").exists()}
                report["remaining_owned_pids"] = remaining
                if remaining:
                    report["ok"] = False
        report["stderr_tail"] = log_path.read_text(errors="replace")[-4000:]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "checks": len(report["checks"]),
                      "error": report.get("error"), "output": str(args.output)}))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--child":
        child(sys.argv[2], sys.argv[3])
    else:
        raise SystemExit(main())
