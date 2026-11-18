#!/usr/bin/env python3
"""Exercise real X11 panes, PTYs, detach and input arbitration in an owned display."""
import argparse
import ctypes
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from isolated_xvfb import start_xvfb

CHILD = r'''import os,sys,tty,urllib.parse
tty.setraw(0)
os.chdir(sys.argv[3])
out=open(sys.argv[1],"wb",buffering=0)
os.write(1,("\x1b]2;"+sys.argv[2]+"\x07\x1b]7;file://localhost"+urllib.parse.quote(sys.argv[3],safe="/")+"\x07\r\n  "+sys.argv[2]+"\r\n\r\n  One shared broker · independent PTY\r\n  中文  /  Rust  /  no_std\r\n").encode())
while True:
 data=os.read(0,4096)
 if not data: break
 out.write(data)
 os.write(1,data)
'''


def closed_identity_gone(pid, started, read_start):
    """Only an already-closed process may disappear during its identity read."""
    try:
        return read_start(pid) != started
    except OSError as error:
        if error.errno in (errno.ENOENT, errno.ESRCH):
            return True
        raise


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
    report = {"ok": False, "checks": [], "platform": "Linux X11 / private Xvfb", "binary": str(binary),
              "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "test_script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    owned = []
    broker_pid = None
    broker_start_ticks = None
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

    def start_ticks(pid):
        return int(Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19])

    def pty_size(pid):
        # Read only the PTY belonging to a child created by this test.
        fd = os.open(f"/proc/{pid}/fd/0", os.O_RDONLY | os.O_NOCTTY | os.O_CLOEXEC)
        try:
            rows, cols, _, _ = struct.unpack("HHHH", fcntl.ioctl(fd, termios.TIOCGWINSZ, b"\0" * 8))
            return cols, rows
        finally:
            os.close(fd)

    with tempfile.TemporaryDirectory(prefix="ztty-workspaces-") as temporary:
        root = Path(temporary)
        log = (root / "stderr.log").open("wb")
        try:
            xvfb = start_xvfb(screen="1600x1000x24", stderr=log)
            connection = display.Display(xvfb.display)
            report["display"] = xvfb.display
            report["xserver_pid"] = xvfb.process.pid
            report["xserver_ownership"] = xvfb.evidence
            env = dict(os.environ, DISPLAY=xvfb.display, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                       XDG_RUNTIME_DIR=temporary, ZTTY_SOCKET="@ztty-workspaces-" + root.name)
            env.pop("WAYLAND_DISPLAY", None)
            # Never alter the parent environment. Only our first launcher and
            # its broker subtree load this shim; its marker targets one PID.
            env.pop("LD_PRELOAD", None)
            fault_source = Path(__file__).with_name("ioctl_resize_fault.c")
            fault_library = root / "ioctl_resize_fault.so"
            fault_marker = root / "resize-failure.pid"
            fault_log = root / "resize-failure.log"
            compiler = shutil.which("cc")
            if compiler is None:
                raise RuntimeError("cc is required for the real ioctl failure regression")
            compile_command = [compiler, "-shared", "-fPIC", "-O2", "-Wall", "-Wextra", "-Werror",
                               "-o", str(fault_library), str(fault_source)]
            compiled = subprocess.run(compile_command, capture_output=True, text=True, timeout=30, check=True)
            report["fault_injection"] = {
                "source": str(fault_source),
                "source_sha256": hashlib.sha256(fault_source.read_bytes()).hexdigest(),
                "library_sha256": hashlib.sha256(fault_library.read_bytes()).hexdigest(),
                "compile_command": compile_command,
                "compiler_stdout": compiled.stdout,
                "compiler_stderr": compiled.stderr,
                "scope": "Private marker matches this test's broker PID; only TIOCSWINSZ fails with EIO. No global LD_PRELOAD.",
            }
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

            def start(name, extra=(), directory=temporary, shell=None):
                launch_env = env if shell is None else dict(env, SHELL=shell)
                if broker_pid is None:
                    launch_env = dict(launch_env, LD_PRELOAD=str(fault_library),
                                      ZTTY_TEST_RESIZE_FAILURE_MARKER=str(fault_marker),
                                      ZTTY_TEST_RESIZE_FAILURE_LOG=str(fault_log))
                process = subprocess.Popen([str(binary), "--name", name, "--config", str(config), "--geometry", "100x26", *extra,
                                            "-e", "/usr/bin/python3", "-c", CHILD, str(root / (name + ".input")), name, directory],
                                           env=launch_env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
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
            broker_start_ticks = start_ticks(broker_pid)
            assert Path(f"/proc/{broker_pid}/exe").resolve() == binary
            report["broker_pid"] = broker_pid
            report["broker_start_ticks"] = broker_start_ticks
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
            # A fixed delay can capture a blank surface under CPU pressure.
            # Wait for rendered glyph pixels in both panes, independently of
            # the terminal model's text response.
            def capture_split():
                connection.sync()
                geometry = parent.get_geometry()
                pixels = parent.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xFFFFFFFF)
                image = Image.frombytes("RGB", (geometry.width, geometry.height), pixels.data, "raw", "BGRX")
                counts = {}
                for name in ("alpha", "beta"):
                    pane = connection.create_resource_object("window", info(name)["window_id"]).get_geometry()
                    region = image.crop((pane.x + 12, pane.y + 12,
                                         pane.x + pane.width - 12, pane.y + pane.height - 12))
                    counts[name] = sum(region.convert("L").histogram()[128:])
                if all(count >= 100 for count in counts.values()):
                    report["screenshot_glyph_pixels"] = counts
                    return image
                return None
            screenshot = wait(capture_split, "rendered glyph pixels in both native split panes")
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

            # Save a real multi-tab split tree, move its windows independently,
            # then restore geometry without restarting or recreating any PTY.
            snapshot_path=root / "workspace.json"
            saved=subprocess.run([str(binary),"save-layout","alpha"],env=env,capture_output=True,check=True,timeout=10)
            snapshot=json.loads(saved.stdout)
            snapshot_path.write_bytes(saved.stdout)
            before_pids={name:info(name)["pid"] for name in ("alpha","beta","gamma")}
            assert len(snapshot["tabs"])==2 and len(snapshot["nodes"])==4,snapshot
            action("beta","move-window")
            action("gamma","move-window")
            restored=subprocess.run([str(binary),"restore-layout",str(snapshot_path)],env=env,capture_output=True,check=True,timeout=10)
            assert json.loads(restored.stdout)["attached"]
            after=json.loads(subprocess.check_output([str(binary),"save-layout","alpha"],env=env,timeout=10))
            assert after==snapshot,(snapshot,after)
            assert {name:info(name)["pid"] for name in before_pids}==before_pids
            assert info("alpha")["workspace"]==info("beta")["workspace"]==info("gamma")["workspace"]
            invalid=json.loads(saved.stdout)
            next(node for node in invalid["nodes"] if "pane" in node)["pane"]="ztty://local/missing-exited-session"
            unchanged=info("alpha")["workspace"]
            rpc({"op":"restore-layout","layout":invalid},success=False)
            rpc({"op":"restore-layout","layout":snapshot,"_actor":"agent-a"},success=False)
            assert info("alpha")["workspace"]==unchanged
            assert rpc({"op":"save-layout","session":"alpha"})==snapshot
            report["checks"].append("versioned CLI layout snapshots restore tabs/splits/geometry with unchanged PTY PIDs; unknown sessions and nonlocal restores fail before mutation")

            # Leave one pane behind in the old container. Its expansion is the
            # first fallible arrange after the selected panes were reparented.
            # Earlier code dropped the new parent at this error and destroyed
            # every selected view. The marker also checks all-view completion
            # and that retrying the same geometry repairs the actual PTY size.
            start("delta", ["--split-right", "alpha"])
            names = ("alpha", "beta", "gamma", "delta")
            before = {name: info(name) for name in names}
            identities = {name: start_ticks(view["pid"]) for name, view in before.items()}
            old_parent = before["delta"]["workspace"]
            sizes_before = {name: pty_size(view["pid"]) for name, view in before.items()}
            for name in names:
                assert sizes_before[name] == (before[name]["cols"], before[name]["rows"]), (name, before[name])
            fault_marker.write_text(f"{broker_pid}\n")
            failed = rpc({"op": "restore-layout", "layout": snapshot}, success=False)
            assert "cannot resize split PTY" in failed.get("error", ""), failed
            wait(lambda: fault_log.exists() and len(fault_log.read_text().splitlines()) >= 2,
                 "old and restored workspace resize errors")
            attempts = [json.loads(line) for line in fault_log.read_text().splitlines()]
            assert all(item["pid"] == broker_pid and item["errno"] == 5 for item in attempts), attempts
            assert len({item["fd"] for item in attempts}) >= 2, attempts
            report["fault_injection"].update({"attempts": attempts, "error_response": failed,
                                              "child_pids": {name: view["pid"] for name, view in before.items()},
                                              "pty_sizes_before": sizes_before})
            failed_views = {name: info(name) for name in names}
            restored_parent = failed_views["alpha"]["workspace"]
            assert restored_parent is not None and restored_parent != old_parent
            report["fault_injection"].update({"old_container": old_parent,
                                              "restored_container_after_error": restored_parent,
                                              "child_start_ticks": identities,
                                              "child_window_ids": {name: view["window_id"] for name, view in before.items()}})
            for name in names:
                view = failed_views[name]
                assert view["pid"] == before[name]["pid"] and start_ticks(view["pid"]) == identities[name]
                assert view["window_id"] == before[name]["window_id"] and view["attached"] and view["running"], view
                window = connection.create_resource_object("window", view["window_id"])
                attributes = window.get_attributes()
                expected_parent = old_parent if name == "delta" else restored_parent
                assert window.query_tree().parent.id == expected_parent, (name, view)
                assert view["visible"] == (name != "gamma"), (name, view)
                assert (attributes.map_state == X.IsViewable) == view["visible"], (name, attributes)
                assert pty_size(view["pid"]) == sizes_before[name], (name, "failed resize changed the PTY")
                assert (view["cols"], view["rows"]) == sizes_before[name], (name, "model advanced despite failed ioctl")
                token = f"fault-survived-{name}"
                rpc({"op": "write", "session": name, "data": token})
                wait(lambda name=name, token=token: token.encode() in input_bytes(name), name + " PTY after resize error")
                wait(lambda name=name, token=token: token in rpc({"op": "read", "session": name})["screen"],
                     name + " output after resize error")
            connection.sync()
            fault_marker.unlink()
            recovered = rpc({"op": "restore-layout", "layout": snapshot})
            assert recovered["attached"]
            recovered_views = {name: info(name) for name in names}
            for name, view in recovered_views.items():
                assert view["pid"] == before[name]["pid"] and start_ticks(view["pid"]) == identities[name]
                assert view["window_id"] == before[name]["window_id"]
                expected = (view["cols"], view["rows"])
                wait(lambda pid=view["pid"], expected=expected: pty_size(pid) == expected, name + " actual PTY resize recovery")
                window = connection.create_resource_object("window", view["window_id"])
                window.get_attributes()
                expected_parent = old_parent if name == "delta" else recovered_views["alpha"]["workspace"]
                assert window.query_tree().parent.id == expected_parent, (name, view)
            assert pty_size(recovered_views["delta"]["pid"]) != sizes_before["delta"], "left-behind pane did not expand"
            assert rpc({"op": "save-layout", "session": "alpha"}) == snapshot
            report["fault_injection"].update({"attempts": attempts, "error_response": failed,
                                              "child_pids": {name: view["pid"] for name, view in before.items()},
                                              "pty_sizes_before": sizes_before,
                                              "pty_sizes_recovered": {name: pty_size(view["pid"]) for name, view in recovered_views.items()}})
            report["checks"].append("injected ioctl EIO keeps both containers, every view, PTY identity and old dimensions alive; retrying the same snapshot repairs actual PTY sizes")

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

            # A canonical URI is reusable after close. A saved layout must not
            # silently adopt a different PTY later created with that same name.
            previous = start("incarnation")
            previous_started = start_ticks(previous["pid"])
            previous_waiter = owned[-1]
            stale_snapshot = rpc({"op": "save-layout", "session": "incarnation"})
            rpc({"op": "close", "session": "incarnation"})
            wait(lambda: not any(s["name"] == "incarnation" for s in rpc({"op": "list"})["sessions"]),
                 "old incarnation removed")
            previous_waiter.wait(timeout=5)
            wait(lambda: closed_identity_gone(previous["pid"], previous_started, start_ticks),
                 "old incarnation PTY reaped")
            replacement = start("incarnation")
            assert replacement["uri"] == previous["uri"], (previous, replacement)
            assert replacement["pid"] != previous["pid"] and replacement["id"] != previous["id"]
            fresh_snapshot = rpc({"op": "save-layout", "session": "incarnation"})
            current_window = connection.create_resource_object("window", replacement["window_id"])
            geometry = current_window.get_geometry()
            parent_id = current_window.query_tree().parent.id
            stale_error = rpc({"op": "restore-layout", "layout": stale_snapshot}, success=False)
            unchanged = info("incarnation")
            for field in ("id", "pid", "uri", "created_at", "window_id", "workspace", "cols", "rows", "input_owner"):
                assert unchanged[field] == replacement[field], (field, replacement, unchanged)
            assert rpc({"op": "save-layout", "session": "incarnation"}) == fresh_snapshot
            now_geometry = current_window.get_geometry()
            assert (now_geometry.width, now_geometry.height) == (geometry.width, geometry.height)
            assert current_window.query_tree().parent.id == parent_id
            assert current_window.get_attributes().map_state == X.IsViewable
            rpc({"op": "write", "session": "incarnation", "data": "replacement-still-live"})
            wait(lambda: "replacement-still-live" in rpc({"op": "read", "session": "incarnation"})["screen"],
                 "replacement PTY survives stale snapshot")
            report["incarnation_reuse"] = {"uri": replacement["uri"],
                                           "previous_id": previous["id"], "replacement_id": replacement["id"],
                                           "previous_pid": previous["pid"], "replacement_pid": replacement["pid"],
                                           "previous_created_at": previous["created_at"],
                                           "replacement_created_at": replacement["created_at"],
                                           "rejected_restore": stale_error}
            report["checks"].append("a stale layout cannot adopt a new PTY that reuses the closed session's origin/name; rejection preserves the replacement view and process")

            # A real shortcut must start a child in the local OSC 7 directory.
            # Verify the child's cwd and PWD, not just terminal metadata.
            from urllib.parse import quote
            fallback = root / "launch-directory"
            desired = root / "shell directory 中文 #?%+"
            records = root / "cwd-records"
            for directory in (fallback, desired, records):
                directory.mkdir()
            probe = root / "cwd-shell"
            probe.write_text(
                "#!/usr/bin/python3\n"
                "import json,os,pathlib,tty\n"
                "tty.setraw(0)\n"
                f"records=pathlib.Path({str(records)!r})\n"
                "value={'pid':os.getpid(),'cwd':os.getcwd(),'pwd':os.environ.get('PWD')}\n"
                "temporary=records/(str(os.getpid())+'.tmp')\n"
                "temporary.write_text(json.dumps(value,ensure_ascii=False))\n"
                "temporary.replace(records/(str(os.getpid())+'.json'))\n"
                "os.write(1,('CWD probe\\r\\n'+value['cwd']+'\\r\\n').encode())\n"
                "while os.read(0,4096): pass\n")
            probe.chmod(0o755)
            parent = start("cwd-parent", ["--cwd", str(fallback)],
                           directory=str(desired), shell=str(probe))
            report["cwd_inheritance"] = []

            def inherited_child(shortcut, expected, label):
                existing = {s["id"] for s in rpc({"op": "list"})["sessions"]}
                before_input = input_bytes("cwd-parent")
                key("cwd-parent", ["Control_L", "Shift_L", shortcut])
                created = wait(
                    lambda: [s for s in rpc({"op": "list"})["sessions"]
                    if s["id"] not in existing], label + " child")
                assert len(created) == 1, created
                child = created[0]
                record = records / (str(child["pid"]) + ".json")
                wait(record.exists, label + " actual child cwd")
                actual = json.loads(record.read_text())
                assert actual["cwd"] == str(expected), (label, actual, expected)
                assert actual["pwd"] == str(expected), (label, actual, expected)
                assert actual["pid"] == child["pid"] and child["broker_pid"] == broker_pid
                assert input_bytes("cwd-parent") == before_input, label
                wait(lambda: "CWD probe" in rpc({"op": "read", "session": child["uri"]})["screen"],
                     label + " real PTY output")
                report["cwd_inheritance"].append({"case": label, "child_id": child["id"],
                                                  "uri": child["uri"], **actual})
                if shortcut == "e":
                    view = info(child["uri"])
                    surface = connection.create_resource_object("window", view["workspace"])

                    def cwd_pixels():
                        connection.sync()
                        geometry = surface.get_geometry()
                        raw = surface.get_image(0, 0, geometry.width, geometry.height,
                                                X.ZPixmap, 0xFFFFFFFF)
                        picture = Image.frombytes("RGB", (geometry.width, geometry.height),
                                                  raw.data, "raw", "BGRX")
                        child_geometry = connection.create_resource_object(
                            "window", view["window_id"]).get_geometry()
                        pane = picture.crop((child_geometry.x, child_geometry.y,
                                             child_geometry.x + child_geometry.width,
                                             child_geometry.y + child_geometry.height))
                        return picture if sum(pane.convert("L").histogram()[128:]) >= 100 else None

                    picture = wait(cwd_pixels, "cwd probe pixels in actual split")
                    screenshot_path = args.screenshot.with_name("workspace-cwd.png")
                    screenshot_path.parent.mkdir(parents=True, exist_ok=True)
                    picture.save(screenshot_path)
                    report["cwd_screenshot"] = str(screenshot_path)
                child_started = start_ticks(child["pid"])
                rpc({"op": "close", "session": child["uri"]})
                wait(lambda: not Path(f"/proc/{child['pid']}").exists()
                     or start_ticks(child["pid"]) != child_started, label + " child reaped")
                wait(lambda: info("cwd-parent")["visible"], label + " parent restored")
                report["checks"].append(label + ": actual child cwd/PWD and shortcut isolation")

            for shortcut, label in [("t", "new tab"), ("e", "right split"),
                                    ("o", "lower split"), ("n", "new window")]:
                inherited_child(shortcut, desired, label + " inherits encoded local OSC 7")

            for suffix, uri in [
                ("remote", "file://remote-host.invalid" + quote(str(desired), safe="/")),
                ("bad-escape", "file://localhost" + quote(str(desired), safe="/") + "%ZZ"),
                ("nul", "file://localhost" + quote(str(desired), safe="/") + "%00"),
            ]:
                title = "cwd-guard-" + suffix
                rpc({"op": "write", "session": "cwd-parent",
                     "data": "\x1b]7;" + uri + "\x07\x1b]2;" + title + "\x07"})
                wait(lambda: info("cwd-parent")["title"] == title, title + " parsed")
                inherited_child("t", fallback, suffix + " OSC 7 uses initial launch directory")

            # Any local user can explicitly reclaim their own desktop session;
            # transport observers never get this action through MCP dispatch.
            for session in rpc({"op": "list"})["sessions"]:
                rpc({"op":"control", "session":session["uri"], "action":"acquire", "takeover":True})
                rpc({"op":"close", "session":session["uri"]})
            wait(lambda: not rpc({"op":"list"})["sessions"], "all sessions released")
            for process in owned:
                process.wait(timeout=5)
            report["checks"].append("all launch/attach waiters complete and sessions release their views and PTYs")
            assert hashlib.sha256(binary.read_bytes()).hexdigest() == report["binary_sha256"], "binary changed during verification"
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
                    if start_ticks(broker_pid) == broker_start_ticks and Path(f"/proc/{broker_pid}/exe").resolve() == binary:
                        os.kill(broker_pid, signal.SIGTERM)
                except (FileNotFoundError, ProcessLookupError):
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
                xvfb.close()
            log.close()
            report["stderr"] = (root / "stderr.log").read_text(errors="replace")[-8000:]
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
