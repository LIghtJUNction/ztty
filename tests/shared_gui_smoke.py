#!/usr/bin/env python3
"""Verify event routing and shared resource lifetimes on an owned X server."""
import argparse
import contextlib
import ctypes
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from measure_multiwindow import cleanup, owned_processes, terminal_windows
from measure_runtime import descendants, process_stat

CHILD = """import os,sys,tty
tty.setraw(0)
out=open(sys.argv[1],"wb",buffering=0)
os.write(1,("\\x1b]2;"+sys.argv[2]+"\\x07"+sys.argv[2]+"  中文 é\\r\\n").encode())
while True:
 data=os.read(0,256)
 if not data: break
 out.write(data)
 os.write(1,data)
"""


def main():
    from Xlib import X, XK, display, protocol
    from Xlib.ext import xtest
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("artifacts/shared-gui-smoke.json"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    result = {"ok": False, "binary": str(binary), "checks": [], "remaining_owned_pids": []}
    ctypes.CDLL(None).prctl(36, 1, 0, 0, 0)
    xvfb = d = None
    processes, identities = [], {}
    with tempfile.TemporaryDirectory(prefix="ztty-shared-gui-") as temporary:
        directory = Path(temporary)
        tag = directory.name
        with (directory / "terminal.log").open("wb") as tlog:
            try:
                rfd, wfd = os.pipe()
                try:
                    xvfb = subprocess.Popen(
                        ["Xvfb", "-displayfd", str(wfd), "-screen", "0", "1920x1200x24", "-nolisten", "tcp"],
                        pass_fds=(wfd,), stdout=subprocess.DEVNULL, stderr=tlog, start_new_session=True)
                finally:
                    os.close(wfd)
                try:
                    if not select.select([rfd], [], [], 10)[0]:
                        raise TimeoutError("Xvfb did not start")
                    number = os.read(rfd, 64).strip().decode()
                finally:
                    os.close(rfd)
                if not number.isdecimal():
                    raise RuntimeError("invalid display number")
                d = display.Display(":" + number)
                env = os.environ.copy()
                env.pop("WAYLAND_DISPLAY", None)
                env.update(DISPLAY=":" + number, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                           ZTTY_MEMORY_TAG=temporary, ZTTY_SOCKET="@ztty-shared-gui-" + tag,
                           XDG_RUNTIME_DIR=temporary)

                def wait(predicate, description):
                    deadline = time.monotonic() + 10
                    while time.monotonic() < deadline:
                        value = predicate()
                        if value:
                            return value
                        time.sleep(0.02)
                    raise TimeoutError(description)

                def named(title):
                    for value in terminal_windows(d).values():
                        if value["window"].get_wm_name() == title:
                            return value
                    return None

                def start_window(index, font_size):
                    title = f"shared-window-{index}"
                    command = [str(binary), "--name", title, "--font", f"monospace:size={font_size}",
                               "--geometry", "80x24", "-e", "/usr/bin/python3", "-c", CHILD,
                               str(directory / f"input-{index}"), title]
                    processes.append(subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL,
                                                      stderr=tlog, start_new_session=True))
                    return title

                def open_window(index, font_size):
                    title = start_window(index, font_size)
                    return wait(lambda: named(title), "window " + str(index))

                def send(window, text):
                    window.set_input_focus(X.RevertToParent, X.CurrentTime)
                    d.sync()
                    for letter in text:
                        code = d.keysym_to_keycode(XK.string_to_keysym(letter))
                        if not code:
                            raise RuntimeError("no keycode for " + letter)
                        xtest.fake_input(d, X.KeyPress, code)
                        xtest.fake_input(d, X.KeyRelease, code)
                    d.sync()

                def received(index, expected):
                    path = directory / f"input-{index}"
                    wait(lambda: path.exists() and path.read_bytes() == expected,
                         f"PTY {index} expected {expected!r}")

                def close(window, expected_count):
                    message = protocol.event.ClientMessage(
                        window=window, client_type=d.intern_atom("WM_PROTOCOLS"),
                        data=(32, [d.intern_atom("WM_DELETE_WINDOW"), X.CurrentTime, 0, 0, 0]))
                    window.send_event(message, event_mask=0)
                    d.flush()
                    wait(lambda: len(terminal_windows(d)) == expected_count, "window close")

                titles = [start_window(1, 11), start_window(2, 11), start_window(3, 13)]
                first, second, third = [
                    wait(lambda title=title: named(title), "concurrent window " + title) for title in titles
                ]
                owners = {entry["pid"] for entry in (first, second, third)}
                if len(owners) != 1:
                    raise AssertionError("multiple GUI owners: " + repr(owners))
                owner = owners.pop()
                for item, letter, index in ((first, "a", 1), (second, "b", 2), (third, "c", 3)):
                    send(item["window"], letter)
                    received(index, letter.encode())
                result["checks"].append({"name": "concurrent_start_and_independent_pty_input", "ok": True, "windows": 3,
                                         "gui_owner_pid": owner, "different_font_sizes": [11, 13]})

                attached = subprocess.Popen([str(binary), "attach", "shared-window-2"],
                                            env=env, stdout=subprocess.DEVNULL, stderr=tlog,
                                            start_new_session=True)
                processes.append(attached)
                wait(lambda: d.get_input_focus().focus.id == second["window"].id, "attach focus")
                if len(terminal_windows(d)) != 3:
                    raise AssertionError("attach created an extra window")
                result["checks"].append({"name": "attach_reuses_window", "ok": True})

                close(first["window"], 2)
                send(second["window"], "z")
                received(2, b"bz")
                received(3, b"c")
                result["checks"].append({"name": "close_one_preserves_shared_font_peer", "ok": True})

                close(second["window"], 1)
                wait(lambda: attached.poll() is not None, "attach launcher completion")
                fourth = open_window(4, 11)
                if fourth["pid"] != owner:
                    raise AssertionError("host changed while a window remained")
                send(fourth["window"], "d")
                received(4, b"d")
                send(third["window"], "y")
                received(3, b"cy")
                result["checks"].append({"name": "reload_font_group_with_other_windows_alive", "ok": True})

                close(third["window"], 1)
                close(fourth["window"], 0)
                result["checks"].append({"name": "close_all_windows", "ok": True})
                result["ok"] = True
            except Exception as error:
                result["error"] = repr(error)
            finally:
                owned = owned_processes(binary, temporary)
                for pid in owned:
                    identities.update(descendants(pid))
                identities.update(owned)
                for process in processes:
                    with contextlib.suppress(OSError, ValueError):
                        identities[process.pid] = process_stat(process.pid)["start_ticks"]
                        identities.update(descendants(process.pid))
                result["remaining_owned_pids"] = cleanup(identities)
                if result["remaining_owned_pids"]:
                    result["ok"] = False
                for process in processes:
                    with contextlib.suppress(subprocess.TimeoutExpired):
                        process.wait(timeout=1)
                if d is not None:
                    d.close()
                if xvfb is not None:
                    xvfb.terminate()
                    xvfb.wait(timeout=2)
        result["terminal_stderr"] = (directory / "terminal.log").read_text(errors="replace")[-4096:]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
