#!/usr/bin/env python3
"""Exercise real X11 panes, PTYs, detach and input arbitration in an owned display."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import tempfile
import time

CHILD = r'''import os,sys,tty
tty.setraw(0)
out=open(sys.argv[1],"wb",buffering=0)
os.write(1,("\x1b]2;"+sys.argv[2]+"\x07\x1b]7;file://localhost"+sys.argv[3]+"\x07\r\n  "+sys.argv[2]+"\r\n\r\n  One shared broker · independent PTY\r\n  中文  /  Rust  /  no_std\r\n").encode())
while True:
 data=os.read(0,4096)
 if not data: break
 out.write(data)
 os.write(1,data)
'''


def main():
    from Xlib import X, XK, display
    from Xlib.ext import xtest
    from PIL import Image
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("artifacts/workspaces.json"))
    parser.add_argument("--screenshot", type=Path, default=Path("artifacts/workspaces.png"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    report = {"ok": False, "checks": [], "platform": "Linux X11 / private Xvfb", "binary": str(binary)}
    owned = []
    broker_pid = None
    xvfb = connection = None
    ctypes.CDLL(None).prctl(36, 1, 0, 0, 0)

    def wait(fn, label):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            try:
                result = fn()
                if result:
                    return result
            except (FileNotFoundError, ConnectionRefusedError):
                pass
            time.sleep(0.02)
        raise TimeoutError(label)

    with tempfile.TemporaryDirectory(prefix="ztty-workspaces-") as temporary:
        root = Path(temporary)
        log = (root / "stderr.log").open("wb")
        try:
            read_fd, write_fd = os.pipe()
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1600x1000x24", "-nolisten", "tcp"],
                                    pass_fds=(write_fd,), stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
            os.close(write_fd)
            try:
                if not select.select([read_fd], [], [], 10)[0]:
                    raise TimeoutError("Xvfb startup")
                number = os.read(read_fd, 64).strip().decode()
            finally:
                os.close(read_fd)
            assert number.isdecimal(), number
            connection = display.Display(":" + number)
            env = dict(os.environ, DISPLAY=":" + number, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                       XDG_RUNTIME_DIR=temporary, ZTTY_SOCKET="@ztty-workspaces-" + root.name)
            env.pop("WAYLAND_DISPLAY", None)
            config = root / "config"
            config.write_text("background = #000000\nbackground-opacity = 100\nbackground-blur = false\ncursor-blink = false\nwindow-padding = 20\n")

            def rpc(request, success=True):
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                    channel.settimeout(3)
                    channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                    channel.sendall(json.dumps(request).encode() + b"\n")
                    data = bytearray()
                    while b"\n" not in data:
                        part = channel.recv(8192)
                        if not part:
                            raise RuntimeError("broker disconnected")
                        data.extend(part)
                        if len(data) > 1048576:
                            raise RuntimeError("oversized broker response")
                value = json.loads(data.split(b"\n", 1)[0])
                assert value["ok"] is success, value
                return value["result"] if success else value

            def info(name):
                return rpc({"op": "info", "session": name})

            def start(name, extra=()):
                process = subprocess.Popen([str(binary), "--name", name, "--config", str(config), "--geometry", "100x26", *extra,
                                            "-e", "/usr/bin/python3", "-c", CHILD, str(root / (name + ".input")), name, temporary],
                                           env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
                owned.append(process)
                wait(lambda: process.poll() is None and any(s["name"] == name for s in rpc({"op": "list"})["sessions"]), name + " startup")
                wait(lambda: name in rpc({"op":"read", "session":name})["screen"], name + " screen")
                return info(name)

            def action(name, value):
                return rpc({"op": "action", "session": name, "action": value})

            def key(name, keys):
                view = info(name)
                window = connection.create_resource_object("window", view["window_id"])
                window.set_input_focus(X.RevertToParent, X.CurrentTime)
                connection.sync()
                codes = [connection.keysym_to_keycode(XK.string_to_keysym(k)) for k in keys]
                assert all(codes), keys
                for code in codes:
                    xtest.fake_input(connection, X.KeyPress, code)
                for code in reversed(codes):
                    xtest.fake_input(connection, X.KeyRelease, code)
                connection.sync()

            def input_bytes(name):
                path = root / (name + ".input")
                return path.read_bytes() if path.exists() else b""

            alpha = start("alpha")
            broker_pid = alpha["broker_pid"]
            assert alpha["id"] > 0 and abs(alpha["created_at"] / 1000 - time.time()) < 30
            beta = start("beta", ["--split-right", "alpha"])
            wait(lambda: info("alpha")["workspace"] == info("beta")["workspace"] != None, "split grouping")
            assert info("alpha")["broker_pid"] == beta["broker_pid"] == broker_pid
            assert info("alpha")["pid"] != beta["pid"]
            key("beta", ["b"])
            wait(lambda: input_bytes("beta") == b"b", "focused pane input")
            assert input_bytes("alpha") == b""
            report["checks"].append("two PTYs share one workspace and only the focused pane receives real keyboard input")

            workspace_id = info("alpha")["workspace"]
            parent = connection.create_resource_object("window", workspace_id)
            original = {name: connection.create_resource_object("window", info(name)["window_id"]).get_geometry() for name in ("alpha", "beta")}
            assert original["beta"].x > original["alpha"].x
            action("beta", "grow")
            wait(lambda: connection.create_resource_object("window", info("beta")["window_id"]).get_geometry().width > original["beta"].width, "split resize")
            action("beta", "shrink")
            action("beta", "swap-next")
            wait(lambda: connection.create_resource_object("window", info("beta")["window_id"]).get_geometry().x == 0, "split swap")
            action("beta", "swap-next")
            action("beta", "maximize")
            wait(lambda: not info("alpha")["visible"] and info("beta")["visible"], "maximize")
            action("beta", "maximize")
            wait(lambda: info("alpha")["visible"] and info("beta")["visible"], "restore splits")
            report["checks"].append("split resize, swap, maximize and restore change real window geometry")

            gamma = start("gamma", ["--tab", "alpha"])
            wait(lambda: info("gamma")["visible"] and not info("alpha")["visible"] and not info("beta")["visible"], "hidden tab")
            rpc({"op": "write", "session": "alpha", "data": "hidden-output"})
            wait(lambda: "hidden-output" in rpc({"op": "read", "session": "alpha"})["screen"], "hidden PTY drain")
            key("gamma", ["Control_L", "Shift_L", "Prior"])
            wait(lambda: info("alpha")["visible"] and info("beta")["visible"] and not info("gamma")["visible"], "real tab shortcut")
            assert input_bytes("gamma") == b"", input_bytes("gamma")
            report["checks"].append("tabs unmap hidden panes while their PTYs keep draining; shortcut bytes never reach the shell")

            # Capture the actual split surface, not a mock or screenshot of text.
            connection.sync()
            time.sleep(0.1)
            geometry = parent.get_geometry()
            pixels = parent.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xFFFFFFFF)
            screenshot = Image.frombytes("RGB", (geometry.width, geometry.height), pixels.data, "raw", "BGRX")
            args.screenshot.parent.mkdir(parents=True, exist_ok=True)
            screenshot.save(args.screenshot)
            report["screenshot"] = str(args.screenshot)

            old_pid = info("beta")["pid"]
            action("beta", "move-window")
            wait(lambda: info("beta")["workspace"] is None, "popout")
            rpc({"op": "move", "session": "beta", "target": "alpha", "layout": "down"})
            assert info("beta")["pid"] == old_pid
            assert info("alpha")["workspace"] == info("beta")["workspace"]
            report["checks"].append("moving panes between native windows and layouts preserves the PTY process")

            old_uri = info("beta")["uri"]
            rpc({"op": "rename", "session": "beta", "name": "renamed"})
            assert info("renamed")["uri"] == old_uri
            assert rpc({"op": "list", "filter": "renamed"})["sessions"][0]["id"] == beta["id"]
            rpc({"op": "detach", "session": "renamed"})
            assert info("renamed")["running"] and not info("renamed")["attached"]
            rpc({"op": "write", "session": "renamed", "data": "detached-output"})
            wait(lambda: b"detached-output" in input_bytes("beta"), "detached input")
            observer = subprocess.Popen([str(binary), "attach", "--observe", "renamed"], env=env,
                                        stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
            owned.append(observer)
            wait(lambda: info("renamed")["attached"], "reattach")
            assert info("renamed")["pid"] == old_pid
            before = input_bytes("beta")
            key("renamed", ["x"])
            time.sleep(0.1)
            assert input_bytes("beta") == before
            key("renamed", ["Control_L", "Shift_L", "g"])
            key("renamed", ["x"])
            wait(lambda: input_bytes("beta") == before + b"x", "explicit GUI takeover")
            report["checks"].append("rename keeps the URI stable; detach and observe reattach preserve PTY/output and enforce read-only input")

            # Separate trusted transport identities share observations but not
            # input control; clients must explicitly request a takeover.
            rpc({"op": "control", "session": "renamed", "action": "acquire", "takeover": True, "_actor": "agent-a"})
            rpc({"op": "write", "session": "renamed", "data": "forbidden", "_actor": "agent-b"}, success=False)
            rpc({"op": "resize", "session": "renamed", "cols": 80, "rows": 24, "_actor": "agent-b"}, success=False)
            rpc({"op": "close", "session": "renamed", "_actor": "agent-b"}, success=False)
            rpc({"op": "control", "session": "renamed", "action": "acquire", "_actor": "agent-b"}, success=False)
            rpc({"op": "read", "session": "renamed", "_actor": "agent-b"})
            rpc({"op": "control", "session": "renamed", "action": "acquire", "takeover": True, "_actor": "agent-b"})
            rpc({"op": "write", "session": "renamed", "data": "forbidden", "_actor": "agent-a"}, success=False)
            rpc({"op": "write", "session": "renamed", "data": "owner-b", "_actor": "agent-b"})
            wait(lambda: input_bytes("beta").endswith(b"owner-b"), "new input owner")
            assert b"forbidden" not in input_bytes("beta")
            report["checks"].append("per-request input ownership rejects old operators and permits multiple observers")

            # Any local user can explicitly reclaim their own desktop session;
            # transport observers never get this action through MCP dispatch.
            for session in rpc({"op": "list"})["sessions"]:
                rpc({"op":"control", "session":session["uri"], "action":"acquire", "takeover":True})
                rpc({"op":"close", "session":session["uri"]})
            wait(lambda: not rpc({"op":"list"})["sessions"], "all sessions released")
            for process in owned:
                process.wait(timeout=5)
            report["checks"].append("all launch/attach waiters complete and sessions release their views and PTYs")
            report["ok"] = True
        except Exception as error:
            import traceback
            report["error"] = str(error)
            report["traceback"] = traceback.format_exc()
            raise
        finally:
            if connection is not None:
                connection.close()
            # Only PIDs from this test's process ownership or broker reply are
            # eligible. No name-based kill and no shared user display cleanup.
            if broker_pid:
                try:
                    os.kill(broker_pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            for process in owned:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=3)
            if xvfb:
                xvfb.terminate()
                xvfb.wait(timeout=3)
            log.close()
            report["stderr"] = (root / "stderr.log").read_text(errors="replace")[-8000:]
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
