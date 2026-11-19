#!/usr/bin/env python3
"""Exercise scrollback anchoring through real PTY output and XTEST on private Xvfb."""
import argparse
import contextlib
import ctypes
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from isolated_xvfb import start_xvfb
from measure_runtime import process_stat
from measure_multiwindow import cleanup
from repaint_gui import add_owned_children, receive, send, wait

COLS, ROWS, HISTORY, PADDING = 20, 6, 8, 16


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def color(index):
    return (40 + index * 5, 30, 50)


def row(index):
    r, g, b = color(index)
    return f"\x1b[48;2;{r};{g};{b}m" + " " * COLS + "\r\n"


def main():
    from PIL import Image
    from Xlib import X, XK, display
    from Xlib.ext import xtest

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("artifacts/scrollback-gui.json"))
    parser.add_argument("--screenshots", type=Path, default=Path("artifacts/scrollback-gui"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    report = {
        "ok": False,
        "binary": str(binary),
        "binary_sha256": digest(binary),
        "scope": "Real PTY output and XTEST with private Xvfb; no physical-display latency measurement",
        "grid": [COLS, ROWS],
        "history_limit": HISTORY,
        "checks": [],
        "pixels": {},
        "screenshots": {},
        "source_sha256": {
            name: digest(ROOT / name) for name in (
                "src/session.rs", "src/session_core.rs", "crates/zterm/src/terminal.rs",
                "tests/scrollback_gui.py", "tests/repaint_gui.py", "tools/isolated_xvfb.py",
            )
        },
    }
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("PR_SET_CHILD_SUBREAPER failed")
    identities = {}
    xserver = connection = listener = channel = launcher = window = None
    with tempfile.TemporaryDirectory(prefix="ztty-scrollback-gui-") as temporary:
        directory = Path(temporary)
        log_path = directory / "stderr.log"
        with log_path.open("wb") as log:
            try:
                xserver = start_xvfb(screen="800x600x24", dpi=96, stderr=log, timeout=10)
                report["display_ownership"] = xserver.evidence
                connection = display.Display(xserver.display)
                config = directory / "ztty.conf"
                config.write_text(
                    "font = monospace:size=11\nbackground = #000000\n"
                    "background-opacity = 100\nbackground-blur = false\n"
                    "cursor-blink = false\nwindow-padding = 16\n"
                )
                env = dict(
                    os.environ, DISPLAY=xserver.display, XMODIFIERS="@im=none",
                    LC_ALL="C.UTF-8", XDG_RUNTIME_DIR=temporary,
                    XDG_CONFIG_HOME=str(directory / "config"), XDG_CACHE_HOME=str(directory / "cache"),
                    XDG_STATE_HOME=str(directory / "state"), XDG_DATA_HOME=str(directory / "data"),
                    ZTTY_SOCKET="@" + directory.name, ZTTY_SCROLLBACK_TEST=directory.name,
                )
                for key in ("WAYLAND_DISPLAY", "LD_PRELOAD", "ZTTY_X11_DIAGNOSTIC_CONTROL"):
                    env.pop(key, None)
                path = directory / "control.sock"
                listener = socket.socket(socket.AF_UNIX)
                listener.bind(str(path))
                listener.listen(1)
                listener.settimeout(10)
                title = directory.name
                launcher = subprocess.Popen(
                    [str(binary), "--config", str(config), "--geometry", f"{COLS}x{ROWS}",
                     "--scrollback", str(HISTORY), "-e", sys.executable,
                     str(ROOT / "tests/repaint_gui.py"), "--child", str(path), title],
                    env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True,
                )
                identities[launcher.pid] = process_stat(launcher.pid)["start_ticks"]
                channel, _ = listener.accept()
                channel.settimeout(5)
                hello = receive(channel)
                identities[hello["pid"]] = process_stat(hello["pid"])["start_ticks"]
                title_atom = connection.intern_atom("_NET_WM_NAME")

                def find_window():
                    for candidate in connection.screen().root.query_tree().children:
                        value = candidate.get_full_property(title_atom, 0)
                        if value is not None and bytes(value.value) == title.encode():
                            return candidate
                    return None

                window, _ = wait(find_window, "owned scrollback test window")
                pid = int(window.get_full_property(connection.intern_atom("_NET_WM_PID"), 0).value[0])
                marker = ("ZTTY_SCROLLBACK_TEST=" + directory.name).encode()
                if marker not in Path(f"/proc/{pid}/environ").read_bytes().split(b"\0"):
                    raise AssertionError("window PID is not owned by this test")
                if Path(f"/proc/{pid}/exe").resolve() != binary or digest(Path(f"/proc/{pid}/exe")) != report["binary_sha256"]:
                    raise AssertionError("window binary does not match candidate")
                identities[pid] = process_stat(pid)["start_ticks"]
                window.configure(x=0, y=0)
                window.set_input_focus(X.RevertToParent, X.CurrentTime)
                connection.sync()
                geometry = window.get_geometry()
                cw = (geometry.width - PADDING * 2) // COLS
                ch = (geometry.height - PADDING * 2) // ROWS
                sequence = 0

                def command(op, **fields):
                    send(channel, {"op": op, **fields})
                    result = receive(channel)
                    assert result["ok"], result
                    return result

                def emit(text):
                    nonlocal sequence
                    sequence += 1
                    expected = title + "-" + str(sequence)
                    command("write", text=text + "\x1b]2;" + expected + "\x07")

                    def parsed():
                        value = window.get_full_property(title_atom, 0)
                        return value is not None and bytes(value.value) == expected.encode()

                    wait(parsed, "PTY parse marker")
                    # The title is a parse marker, not a display fence. Allow the
                    # bounded output-repaint deadline before testing unchanged pixels.
                    time.sleep(0.08)
                    connection.sync()

                def pixel():
                    data = window.get_image(
                        PADDING + cw * 10 + 1, PADDING + ch // 2,
                        1, 1, X.ZPixmap, 0xffffffff,
                    ).data
                    return Image.frombytes("RGB", (1, 1), data, "raw", "BGRX").getpixel((0, 0))

                def capture(name):
                    data = window.get_image(
                        0, 0, geometry.width, geometry.height, X.ZPixmap, 0xffffffff,
                    ).data
                    target = args.screenshots / (name + ".png")
                    target.parent.mkdir(parents=True, exist_ok=True)
                    Image.frombytes(
                        "RGB", (geometry.width, geometry.height), data, "raw", "BGRX",
                    ).save(target)
                    report["screenshots"][name] = str(target)

                def expect(expected, label):
                    actual = pixel()
                    report["pixels"][label] = {"expected": expected, "actual": actual}
                    capture(label)
                    assert actual == expected, f"{label}: expected {expected}, received {actual}"
                    report["checks"].append(label)

                def scroll_up():
                    xtest.fake_input(connection, X.MotionNotify, x=20, y=20)
                    xtest.fake_input(connection, X.ButtonPress, 4)
                    xtest.fake_input(connection, X.ButtonRelease, 4)
                    connection.sync()
                    time.sleep(0.08)

                emit("".join(row(i) for i in range(20)))
                expect(color(15), "full-history-live")
                scroll_up()
                expect(color(12), "browse-three-rows-back")
                emit(row(20) + row(21))
                expect(color(12), "retained-anchor-stays-fixed")
                emit("".join(row(i) for i in range(22, 32)))
                expect(color(19), "evicted-anchor-clamps-to-oldest")

                keycode = connection.keysym_to_keycode(XK.string_to_keysym("x"))
                xtest.fake_input(connection, X.KeyPress, keycode)
                xtest.fake_input(connection, X.KeyRelease, keycode)
                connection.sync()
                time.sleep(0.08)
                expect(color(27), "typing-returns-to-live")
                assert "78" in command("status")["input_hex"], "typing did not reach the PTY"

                scroll_up()
                expect(color(24), "browse-before-clear")
                emit("\x1b[3J")
                expect(color(27), "clear-history-returns-to-live")
                emit("".join(row(i) for i in range(32, 36)))
                scroll_up()
                expect(color(28), "browse-before-alternate")
                alternate = (55, 120, 90)
                emit("\x1b[?1049h\x1b[?25l\x1b[48;2;55;120;90m\x1b[2J\x1b[H")
                expect(alternate, "alternate-screen-starts-live")
                emit("\x1b[?1049l")
                expect(color(31), "alternate-exit-restores-primary-live")
                assert digest(binary) == report["binary_sha256"], "candidate changed during acceptance"
                assert all(digest(ROOT / name) == value for name, value in report["source_sha256"].items()), "source changed during acceptance"
                report["ok"] = True
            except Exception as error:
                report["error"] = str(error)
                if window is not None:
                    with contextlib.suppress(Exception):
                        capture("failure")
            finally:
                if channel is not None:
                    with contextlib.suppress(Exception):
                        send(channel, {"op": "quit"})
                    channel.close()
                if listener is not None:
                    listener.close()
                add_owned_children(identities, [os.getpid(), *identities])
                if xserver is not None:
                    identities.pop(xserver.process.pid, None)
                report["remaining_owned_pids"] = cleanup(identities)
                if launcher is not None:
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
                        "lease_exists": Path(f"/tmp/ztty-xvfb-{number}.lease").exists(),
                    }
                if report["remaining_owned_pids"]:
                    report["ok"] = False
        report["stderr_tail"] = log_path.read_text(errors="replace")[-4000:]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "checks": len(report["checks"]),
                      "error": report.get("error"), "output": str(args.output)}))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
