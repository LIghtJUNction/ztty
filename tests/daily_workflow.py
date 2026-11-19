#!/usr/bin/env python3
"""Exercise search, command UI, configuration and ownership through real X11 input.

Requirements: Linux, Xvfb, python-xlib and Pillow. The test owns a private X
display, runtime directory, broker socket and PTY; it never uses the user's
desktop or kills processes by name. Screen captures are actual X11 pixels.
"""

import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from isolated_xvfb import start_xvfb


CHILD = r'''import os,sys,threading,time,tty
tty.setraw(0)
out=open(sys.argv[1],"wb",buffering=0)
def emit(text): os.write(1,text.encode())
emit("\x1b]2;daily workflow\x07\x1b]133;A\x07$ \x1b]133;B\x07echo first\r\n\x1b]133;C\x07")
emit("first-output 中文\r\n\x1b]133;D;7\x07")
emit("\x1b]133;A\x07$ \x1b]133;B\x07echo history\r\n\x1b]133;C\x07")
for n in range(100):
    emit(("history %03d  " % n) + ("needle" if n in (10,55) else "CaseTarget" if n == 15 else "casetarget" if n == 60 else "retained output") + "\r\n")
emit("\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07")
emit("\x1b[38;2;255;0;255mtruecolor-sentinel\x1b[0m default-sentinel\r\nREADY\r\n")
# Enable application mouse reporting to catch leaked bound button events.
emit("\x1b[?1000h\x1b[?1006h")
# A private file requests output without introducing application input. The
# daemon exits with this fixture's PTY process; it owns no additional process.
def updates():
    previous=""
    while True:
        try:
            with open(sys.argv[2]) as request: current=request.read().strip()
        except FileNotFoundError:
            current=""
        if current and current != previous:
            previous=current
            emit("\r\nlive-update-" + current + "\r\n")
        time.sleep(0.02)
threading.Thread(target=updates,daemon=True).start()
while True:
    data=os.read(0,4096)
    if not data: break
    out.write(data)
'''

BASE_CONFIG = """font = monospace:size=11
background = #000000
foreground = #e1e7f0
background-opacity = 100
background-blur = false
cursor-blink = false
window-padding = 20
selection-background = #355577
selection-foreground = #ffffff
keybind = ctrl+shift+x = search
mousebind = ctrl+button3 = palette
"""


def main():
    from Xlib import X, XK, display, protocol
    from Xlib.ext import xtest
    from PIL import Image, ImageChops

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("artifacts/daily-workflow.json"))
    parser.add_argument("--screenshots", type=Path, default=Path("artifacts/daily-workflow"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    repository = Path(__file__).resolve().parent.parent
    report = {
        "ok": False,
        "checks": [],
        "platform": "Linux X11 / private Xvfb",
        "binary": str(binary),
        "binary_sha256": hashlib.file_digest(binary.open("rb"), "sha256").hexdigest(),
        "source_sha256": {},
        "screenshots": {},
    }
    for name in ("src/session.rs", "src/session_control.rs", "src/output_tail.rs",
                 "src/session_core.rs", "src/input_queue.rs",
                 "src/search.rs", "src/scroll_prompt.rs", "src/actions.rs", "src/config_watch.rs",
                 "src/x11.rs", "src/appearance.rs", "src/main.rs", "tests/daily_workflow.py"):
        path = repository / name
        if path.is_file():
            report["source_sha256"][name] = hashlib.sha256(path.read_bytes()).hexdigest()

    launcher = xvfb = connection = broker_pid = child_pid = None
    child_start = None
    owned = []
    children = {}
    # Adopt this test's detached broker/PTY so failed tests do not leave zombies.
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("PR_SET_CHILD_SUBREAPER failed")

    def wait(predicate, label, seconds=12):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                value = predicate()
                if value:
                    return value
            except (FileNotFoundError, ConnectionRefusedError):
                pass
            time.sleep(0.02)
        raise TimeoutError(label)

    with tempfile.TemporaryDirectory(prefix="ztty-daily-workflow-") as temporary:
        root = Path(temporary)
        log = (root / "stderr.log").open("wb")
        try:
            xvfb = start_xvfb(screen="1600x1000x24", stderr=log, timeout=10)
            connection = display.Display(xvfb.display)
            report["display_ownership"] = xvfb.evidence
            env = dict(os.environ, DISPLAY=xvfb.display, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                       XDG_RUNTIME_DIR=temporary, XDG_CONFIG_HOME=str(root / "config-home"),
                       ZTTY_SOCKET="@ztty-daily-" + root.name)
            env.pop("WAYLAND_DISPLAY", None)
            config = root / "settings" / "ztty" / "config"
            config.parent.mkdir(parents=True)
            config.write_text(BASE_CONFIG)
            default_config = root / "config-home" / "ztty" / "config"
            default_config.parent.mkdir(parents=True)
            default_config.write_text(BASE_CONFIG)
            input_path = root / "input.bin"
            output_request = root / "output-request.txt"

            def rpc(request, success=True):
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                    channel.settimeout(5)
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
                response = json.loads(data.split(b"\n", 1)[0])
                assert response["ok"] is success, response
                return response["result"] if success else response

            def info():
                return rpc({"op": "info", "session": "daily"})

            def action(value, success=True):
                return rpc({"op": "action", "session": "daily", "action": value}, success)

            def input_bytes():
                return input_path.read_bytes() if input_path.exists() else b""

            def window():
                return connection.create_resource_object("window", info()["window_id"])

            def key(*keys):
                window().set_input_focus(X.RevertToParent, X.CurrentTime)
                connection.sync()
                codes = [connection.keysym_to_keycode(XK.string_to_keysym(name)) for name in keys]
                assert all(codes), keys
                for code in codes:
                    xtest.fake_input(connection, X.KeyPress, code)
                for code in reversed(codes):
                    xtest.fake_input(connection, X.KeyRelease, code)
                connection.sync()

            def text(value):
                names = {" ": "space", "-": "minus", "+": "plus"}
                for character in value:
                    key(names.get(character, character))

            def image():
                view = window()
                connection.sync()
                geometry = view.get_geometry()
                pixels = view.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xFFFFFFFF)
                return Image.frombytes("RGB", (geometry.width, geometry.height), pixels.data, "raw", "BGRX")

            def color_count(color):
                return sum(count for count, value in image().getcolors(2000000) if value == color)

            def capture(name):
                args.screenshots.mkdir(parents=True, exist_ok=True)
                path = args.screenshots / (name + ".png")
                image().save(path)
                report["screenshots"][name] = {
                    "path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                }

            def prompt_box(new_output=False):
                # The one-pixel border is an exact solid RGB color, independent
                # of font rasterization. Locate the actual painted target.
                color = (137, 180, 250) if new_output else (100, 116, 139)
                channels = image().split()
                masks = [channel.point(lambda value, target=target: 255 if value == target else 0)
                         for channel, target in zip(channels, color)]
                return ImageChops.multiply(ImageChops.multiply(masks[0], masks[1]), masks[2]).getbbox()

            def no_prompt():
                return prompt_box() is None and prompt_box(True) is None

            def click_prompt(new_output=False):
                bounds = wait(lambda: prompt_box(new_output), "painted scrollback return target")
                before = info()
                before_input = input_bytes()
                x, y = (bounds[0] + bounds[2]) // 2, (bounds[1] + bounds[3]) // 2
                view = window()
                view.warp_pointer(x, y)
                connection.sync()
                xtest.fake_input(connection, X.ButtonPress, 1)
                connection.sync()
                # The button disappears on press. Its later drag/release must
                # remain UI input even though the live terminal is now below it.
                view.warp_pointer(x + 2, y)
                xtest.fake_input(connection, X.ButtonRelease, 1)
                connection.sync()
                wait(no_prompt, "scrollback prompt disappears after returning live")
                after = info()
                for field in ("pid", "input_owner", "read_only", "cols", "rows"):
                    assert after[field] == before[field], (field, before, after)
                time.sleep(0.05)
                assert input_bytes() == before_input, (before_input, input_bytes())
                return bounds

            def new_output(token):
                marker = "live-update-" + token
                output_request.write_text(token)
                wait(lambda: marker in rpc({"op": "read", "session": "daily"})["screen"],
                     "new PTY output reaches the terminal model")
                wait(lambda: prompt_box(True), "new-output scrollback prompt")
                return marker

            clipboard_window = connection.screen().root.create_window(
                0, 0, 1, 1, 0, X.CopyFromParent, X.InputOutput, X.CopyFromParent,
                event_mask=X.PropertyChangeMask,
            )
            clipboard = connection.intern_atom("CLIPBOARD")
            utf8 = connection.intern_atom("UTF8_STRING")
            transfer = connection.intern_atom("ZTTY_TEST_CLIPBOARD")

            def clipboard_text():
                wait(lambda: connection.get_selection_owner(clipboard), "clipboard ownership")
                clipboard_window.delete_property(transfer)
                connection.sync()
                clipboard_window.convert_selection(clipboard, utf8, transfer, X.CurrentTime)
                connection.flush()
                value = wait(lambda: clipboard_window.get_full_property(transfer, utf8), "UTF-8 clipboard reply")
                return bytes(value.value).decode("utf-8")

            def expect_clipboard(expected):
                last = ""
                def matches():
                    nonlocal last
                    last = clipboard_text()
                    return last == expected
                try:
                    return wait(matches, "expected clipboard contents")
                except TimeoutError:
                    raise AssertionError({"expected_clipboard": expected, "actual_clipboard": last})

            def atomic_config(contents):
                replacement = root / "config.new"
                replacement.write_text(contents)
                replacement.replace(config)

            launcher = subprocess.Popen(
                [str(binary), "--name", "daily", "--config", str(config), "--geometry", "100x26",
                 "--scrollback", "300", "-e", "/usr/bin/python3", "-c", CHILD, str(input_path),
                 str(output_request)],
                env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True,
            )
            owned.append(launcher)
            wait(lambda: launcher.poll() is None and rpc({"op": "list"})["sessions"], "daily startup")
            initial = info()
            broker_pid, child_pid = initial["broker_pid"], initial["pid"]
            child_start = Path(f"/proc/{child_pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
            children[child_pid] = child_start
            wait(lambda: "READY" in rpc({"op": "read", "session": "daily"})["screen"], "initial terminal output")
            assert "needle" not in rpc({"op": "read", "session": "daily"})["screen"]
            assert input_bytes() == b""
            initial_geometry = (initial["cols"], initial["rows"])
            assert initial_geometry == (100, 26), initial
            wait(lambda: color_count((255, 0, 255)) > 10, "truecolor sentinel render")

            # OSC 133 records are checked independently through broker and CLI.
            commands = rpc({"op": "commands", "session": "daily"})["commands"]
            completed = [command for command in commands if command["has_output"] and command["complete"]]
            assert len(completed) == 2, commands
            first, second = completed
            assert first["exit_status"] == 7 and second["exit_status"] == 0, completed
            first_output = rpc({"op": "command-read", "session": "daily", "command_id": first["id"]})
            assert first_output["complete"] and not first_output["truncated"], first_output
            assert "first-output 中文" in first_output["text"], first_output
            limited = rpc({"op": "command-read", "session": "daily", "command_id": first["id"], "max_bytes": 3})
            assert limited["truncated"] and len(limited["text"].encode()) <= 3, limited
            command_input = rpc({"op": "command-read", "session": "daily", "command_id": first["id"], "part": "input"})
            assert "echo first" in command_input["text"], command_input
            cli_commands = subprocess.run([str(binary), "commands", "daily"], env=env, capture_output=True, check=True, timeout=10)
            assert json.loads(cli_commands.stdout)["commands"] == commands
            cli_output = subprocess.run([str(binary), "command-output", "daily", str(first["id"])], env=env,
                                        capture_output=True, check=True, timeout=10)
            assert json.loads(cli_output.stdout)["text"] == first_output["text"]
            report["checks"].append("OSC 133 metadata, exit codes, bounded command output and command CLI agree with the real PTY")

            # Search only old history, select a match, export selection through
            # the actual X clipboard, and navigate to the other retained match.
            key("Control_L", "Shift_L", "f")
            text("needle")
            wait(lambda: color_count((53, 85, 119)) > 100, "history search highlights")
            key("Control_L", "Shift_L", "c")
            expect_clipboard("needle")
            first_search = image()
            capture("history-search")
            key("Return")
            wait(lambda: ImageChops.difference(first_search, image()).getbbox(), "next search result changes retained viewport")
            key("Control_L", "Shift_L", "c")
            expect_clipboard("needle")
            key("Escape")
            assert input_bytes() == b"", input_bytes()
            report["checks"].append("real search keys find off-screen retained output, highlight it, navigate matches and copy UTF-8 selection without PTY input")

            # Search case mode is local UI state. The clipboard proves both
            # the selected original spelling and the navigation order.
            key("Control_L", "Shift_L", "f")
            key("Control_L", "u")
            text("casetarget")
            key("Control_L", "Shift_L", "c")
            expect_clipboard("casetarget")
            key("Return")
            key("Control_L", "Shift_L", "c")
            expect_clipboard("casetarget")
            key("Alt_L", "c")
            key("Return")
            key("Control_L", "Shift_L", "c")
            expect_clipboard("CaseTarget")
            capture("history-search-ignore-case")
            key("Shift_L", "Return")
            key("Control_L", "Shift_L", "c")
            expect_clipboard("casetarget")
            key("Alt_L", "c")
            key("Return")
            key("Control_L", "Shift_L", "c")
            expect_clipboard("casetarget")
            capture("history-search-match-case")
            key("Escape")
            assert input_bytes() == b"", input_bytes()
            report["checks"].append("Alt+C toggles search case; forward/backward navigation and clipboard preserve original spelling without PTY input")

            # Command navigation and clipboard use retained regions, never a
            # second copy of command stdout. The latest prompt may be incomplete.
            for _ in commands:
                key("Control_L", "Shift_L", "k")
            key("Control_L", "Shift_L", "y")
            expect_clipboard(first_output["text"])
            key("Control_L", "Shift_L", "j")
            key("Control_L", "Shift_L", "y")
            wait(lambda: "history 010  needle" in clipboard_text(), "second command clipboard output")
            assert input_bytes() == b"", input_bytes()
            report["checks"].append("command navigation copies the selected first and second command regions through the real clipboard")

            # Filter and execute a palette action using actual input. The PTY
            # geometry change proves execution rather than only overlay drawing.
            before_palette = image()
            key("Control_L", "Shift_L", "p")
            text("font-increase")
            wait(lambda: ImageChops.difference(before_palette, image()).getbbox(), "palette filter render")
            capture("command-palette")
            key("Return")
            wait(lambda: info()["cols"] < initial_geometry[0] or info()["rows"] < initial_geometry[1], "palette font action")
            assert info()["pid"] == child_pid
            key("Control_L", "0")
            wait(lambda: (info()["cols"], info()["rows"]) == initial_geometry, "font reset")
            assert input_bytes() == b"", input_bytes()
            report["checks"].append("filtered command palette executes a font action, font reset restores the grid and both preserve the running PTY")

            # Custom key and mouse bindings use the same view-action path.
            key("Control_L", "Shift_L", "x")
            key("Control_L", "u")
            text("needle")
            wait(lambda: color_count((53, 85, 119)) > 100, "custom search key binding")
            key("Escape")
            before_mouse = image()
            view = window()
            view.warp_pointer(100, 100)
            view.set_input_focus(X.RevertToParent, X.CurrentTime)
            ctrl = connection.keysym_to_keycode(XK.string_to_keysym("Control_L"))
            xtest.fake_input(connection, X.KeyPress, ctrl)
            xtest.fake_input(connection, X.ButtonPress, 3)
            xtest.fake_input(connection, X.ButtonRelease, 3)
            xtest.fake_input(connection, X.KeyRelease, ctrl)
            connection.sync()
            text("font-increase")
            wait(lambda: ImageChops.difference(before_mouse, image()).getbbox(), "custom mouse palette binding")
            key("Return")
            wait(lambda: info()["cols"] < initial_geometry[0] or info()["rows"] < initial_geometry[1], "custom mouse action execution")
            key("Control_L", "0")
            wait(lambda: (info()["cols"], info()["rows"]) == initial_geometry, "custom mouse font reset")
            assert input_bytes() == b"", input_bytes()
            report["checks"].append("custom key and mouse bindings execute actions; application mouse reporting receives neither bound press nor release")

            # A width change conservatively invalidates command regions that
            # overlap the live screen; navigation must skip those records.
            current_commands=rpc({"op":"commands","session":"daily"})["commands"]
            assert any(not c["available"] for c in current_commands),current_commands
            for _ in current_commands:
                key("Control_L","Shift_L","k")
            key("Control_L","Shift_L","y")
            expect_clipboard(first_output["text"])
            report["checks"].append("command navigation skips regions invalidated by live-screen width changes and retains intact historical output")

            # Return to the live screen, then atomically replace the watched
            # config. Pixel color, grid and PID jointly check the applied state.
            for _ in range(12):
                action("scroll-down")
            changed_config = BASE_CONFIG.replace("background = #000000", "background = #030609")
            changed_config = changed_config.replace("foreground = #e1e7f0", "foreground = #80ff80")
            changed_config = changed_config.replace("monospace:size=11", "monospace:size=13")
            atomic_config(changed_config)
            wait(lambda: image().getpixel((1, 1)) == (3, 6, 9), "atomic config background reload")
            wait(lambda: info()["cols"] < initial_geometry[0] and info()["rows"] < initial_geometry[1], "atomic font reload")
            wait(lambda: color_count((255, 0, 255)) > 10 and color_count((128, 255, 128)) > 10, "theme source preservation")
            assert info()["pid"] == child_pid
            applied_geometry = (info()["cols"], info()["rows"])
            before_invalid = image()
            atomic_config(changed_config + "unknown-option = rejected\n")
            action("reload", success=False)
            wait(lambda: ImageChops.difference(before_invalid,image()).getbbox(), "invalid watched config visible diagnostic")
            capture("invalid-config-diagnostic")
            assert image().getpixel((1, 1)) == (3, 6, 9)
            assert (info()["cols"], info()["rows"]) == applied_geometry
            assert info()["pid"] == child_pid
            atomic_config(BASE_CONFIG)
            wait(lambda: image().getpixel((1, 1)) == (0, 0, 0), "valid config recovery")
            wait(lambda: (info()["cols"], info()["rows"]) == initial_geometry, "valid font recovery")
            key("Control_L", "Shift_L", "r")
            assert input_bytes() == b"", input_bytes()
            capture("reloaded-terminal")
            report["checks"].append("atomic config saves update pixels/font without restarting; explicit truecolor survives theme change; malformed reload retains the previous valid state")

            # Editors/config managers also replace the containing directory.
            # Exercise both a moved parent and a deleted/recreated parent.
            moved_directory = config.parent.with_name("ztty.moved")
            config.parent.rename(moved_directory)
            config.parent.mkdir()
            atomic_config(changed_config)
            wait(lambda: image().getpixel((1, 1)) == (3, 6, 9), "config watcher after directory rename/recreation")
            (moved_directory / "config").unlink()
            moved_directory.rmdir()
            config.unlink()
            config.parent.rmdir()
            config.parent.mkdir()
            atomic_config(BASE_CONFIG)
            wait(lambda: image().getpixel((1, 1)) == (0, 0, 0), "config watcher after directory deletion/recreation")
            wait(lambda: (info()["cols"], info()["rows"]) == initial_geometry, "font after directory recovery")
            report["checks"].append("config watching recovers after the containing directory is renamed or deleted and recreated")

            # The return control is temporary, pins the inspected rows while
            # output arrives, and withholds its full mouse gesture from the PTY.
            wait(no_prompt, "no permanent return control at the live screen")
            key("Shift_L", "Page_Up")
            normal_box = wait(lambda: prompt_box(), "return control while browsing history")
            assert prompt_box(True) is None
            capture("scrollback-return")
            retained = image()
            marker = new_output("1")
            updated = image()
            top = (0, 0, retained.width, retained.height // 2)
            assert ImageChops.difference(retained.crop(top), updated.crop(top)).getbbox() is None
            unread_box = prompt_box(True)
            capture("scrollback-new-output")
            click_prompt(True)
            capture("scrollback-live")
            key("Shift_L", "Page_Up")
            wait(lambda: prompt_box(), "returning live clears the old new-output indication")
            assert prompt_box(True) is None
            click_prompt()
            report["scroll_prompt"] = {"normal_bounds": normal_box, "new_output_bounds": unread_box}
            report["checks"].append("history shows a temporary return control; new PTY output preserves the inspected rows and marks new output; clicking returns live, clears the indication and sends no application mouse bytes")

            # Search and the full command palette keep their query/input modes
            # when the return control above them is clicked.
            key("Shift_L", "Page_Up")
            wait(lambda: prompt_box(), "history before search coexistence")
            key("Control_L", "Shift_L", "f")
            key("Control_L", "u")
            text("needle")
            search_box = wait(lambda: (box if (box := prompt_box()) and box[1] < normal_box[1] else None),
                              "return control above the search row")
            capture("scrollback-search")
            click_prompt()
            key("Control_L", "u")
            text(marker)
            key("Control_L", "Shift_L", "c")
            expect_clipboard(marker)
            key("Escape")
            key("Shift_L", "Page_Up")
            wait(lambda: prompt_box(), "history before palette coexistence")
            key("Control_L", "Shift_L", "p")
            palette_box = wait(lambda: (box if (box := prompt_box()) and box[1] < search_box[1] else None),
                               "return control above the full palette")
            capture("scrollback-palette")
            click_prompt()
            text("search")
            key("Return")
            key("Control_L", "u")
            text("needle")
            key("Control_L", "Shift_L", "c")
            expect_clipboard("needle")
            key("Escape")
            assert input_bytes() == b"", input_bytes()
            report["scroll_prompt"].update(search_bounds=search_box, palette_bounds=palette_box)
            report["checks"].append("the return control avoids search and palette rows; clicking preserves both modal input flows, clipboard search and the unchanged PTY grid")

            # A separate trusted transport identity owns input. The GUI remains
            # useful for observation/search/copy, but keys and paste cannot write.
            rpc({"op": "control", "session": "daily", "action": "acquire", "takeover": True, "_actor": "workflow-owner"})
            rpc({"op": "write", "session": "daily", "data": "forbidden", "_actor": "workflow-observer"}, success=False)
            rpc({"op": "commands", "session": "daily", "_actor": "workflow-observer"})
            observed = rpc({"op": "command-read", "session": "daily", "command_id": first["id"], "_actor": "workflow-observer"})
            assert observed["text"] == first_output["text"]
            key("x")
            key("Control_L", "Shift_L", "v")
            key("Control_L", "Shift_L", "f")
            key("Control_L", "u")
            text("needle")
            key("Control_L", "Shift_L", "c")
            expect_clipboard("needle")
            key("Escape")
            # The clipboard response is an event-loop barrier for the preceding
            # input. Waiting briefly also catches a deferred paste notification.
            time.sleep(0.1)
            assert input_bytes() == b"", input_bytes()

            # Attach to an existing workspace as an observer: the companion
            # pane must retain X input focus. This uses the real attach handler.
            companion = subprocess.Popen(
                [str(binary), "--name", "companion", "--config", str(config), "--split-right", "daily",
                 "-e", "/usr/bin/python3", "-c", "import time; time.sleep(120)"],
                env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True,
            )
            owned.append(companion)
            wait(lambda: len(rpc({"op": "list"})["sessions"]) == 2, "companion pane startup")
            other = rpc({"op": "info", "session": "companion"})
            children[other["pid"]] = Path(f"/proc/{other['pid']}/stat").read_text().rsplit(")", 1)[1].split()[19]
            assert other["workspace"] == info()["workspace"] is not None
            other_window = connection.create_resource_object("window", other["window_id"])
            other_window.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            assert connection.get_input_focus().focus.id == other["window_id"]
            rpc({"op": "attach", "session": "daily", "observe": True})
            time.sleep(0.1)
            connection.sync()
            assert connection.get_input_focus().focus.id == other["window_id"]
            report["checks"].append("observing an already attached workspace pane preserves the other pane's actual X input focus")

            # Closing an observing view must not kill a local-created PTY whose
            # input is now owned by an agent. Check named close and WM_DELETE.
            action("close")
            wait(lambda: not info()["attached"], "observer action close detaches view")
            assert info()["running"] and info()["pid"] == child_pid
            rpc({"op": "write", "session": "daily", "data": "agent-after-close", "_actor": "workflow-owner"})
            wait(lambda: input_bytes() == b"agent-after-close", "PTY survives observer close")

            def observe_attach():
                attached = subprocess.Popen([str(binary), "attach", "--observe", "daily"], env=env,
                                            stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
                owned.append(attached)
                wait(lambda: info()["attached"], "observer CLI attach")
                assert info()["running"] and info()["pid"] == child_pid

            observe_attach()
            observing = info()
            assert observing["read_only"] and observing["input_owner"] == "workflow-owner", observing
            key("Shift_L", "Page_Up")
            wait(lambda: prompt_box(), "return control in a read-only observing view")
            new_output("2")
            capture("scrollback-observer")
            click_prompt(True)
            assert input_bytes() == b"agent-after-close", input_bytes()
            report["scroll_prompt"]["observer"] = {
                "input_owner": info()["input_owner"], "read_only": info()["read_only"],
                "pid_preserved": info()["pid"] == child_pid,
            }
            report["checks"].append("a read-only observer can return live after new output without acquiring input, resizing or replacing the PTY, or sending mouse input")
            close_message = protocol.event.ClientMessage(
                window=window(), client_type=connection.intern_atom("WM_PROTOCOLS"),
                data=(32, [connection.intern_atom("WM_DELETE_WINDOW"), X.CurrentTime, 0, 0, 0]),
            )
            window().send_event(close_message, event_mask=0)
            connection.flush()
            wait(lambda: not info()["attached"], "observer WM_DELETE detaches view")
            assert info()["running"] and info()["pid"] == child_pid
            observe_attach()
            report["checks"].append("closing an observer by action or WM_DELETE preserves the agent-owned PTY and permits reattachment")

            key("Control_L", "Shift_L", "g")
            wait(lambda: info()["input_owner"] == "local", "explicit local takeover")
            key("x")
            wait(lambda: input_bytes() == b"agent-after-closex", "input after explicit takeover")
            assert info()["pid"] == child_pid
            report["checks"].append("observers can inspect commands/search/copy while GUI typing and paste are blocked; explicit takeover restores input without restarting the PTY")

            # Reacquiring the same local owner is state-idempotent, but must
            # still leave explicit observation mode and permit GUI input.
            rpc({"op": "attach", "session": "daily", "observe": True})
            observed_local = info()
            assert observed_local["input_owner"] == "local" and observed_local["read_only"]
            assert observed_local["pid"] == child_pid
            rpc({"op": "control", "session": "daily", "action": "acquire"})
            reclaimed_local = info()
            assert reclaimed_local["input_owner"] == "local" and not reclaimed_local["read_only"]
            assert reclaimed_local["pid"] == child_pid
            key("y")
            wait(lambda: input_bytes() == b"agent-after-closexy", "same-owner acquire restores local input")
            report["checks"].append("same-owner local acquire leaves observation mode and restores real GUI input without replacing the PTY")


            # Attaching an observer to an already visible, locally owned view
            # must clear fresh-view ownership too. The original companion Ui
            # was created above; this exercises the existing-Ui attach branch.
            local_observer = subprocess.Popen(
                [str(binary), "attach", "--observe", "companion"], env=env,
                stdout=subprocess.DEVNULL, stderr=log, start_new_session=True,
            )
            owned.append(local_observer)
            wait(lambda: rpc({"op": "info", "session": "companion"})["read_only"],
                 "existing local view becomes an observer")
            local_before = rpc({"op": "info", "session": "companion"})
            assert local_before["input_owner"] == "local" and local_before["pid"] == other["pid"]
            local_window = connection.create_resource_object("window", local_before["window_id"])
            local_window.send_event(protocol.event.ClientMessage(
                window=local_window, client_type=connection.intern_atom("WM_PROTOCOLS"),
                data=(32, [connection.intern_atom("WM_DELETE_WINDOW"), X.CurrentTime, 0, 0, 0]),
            ), event_mask=0)
            connection.flush()
            wait(lambda: not rpc({"op": "info", "session": "companion"})["attached"],
                 "existing locally owned observer closes")
            local_after = rpc({"op": "info", "session": "companion"})
            assert local_after["running"] and local_after["pid"] == local_before["pid"]
            assert local_after["input_owner"] == "local"
            rpc({"op": "write", "session": "companion", "data": "local-after-observer-close\n"})
            wait(lambda: "local-after-observer-close" in rpc({"op": "read", "session": "companion"})["screen"],
                 "existing local PTY still echoes after observer close")
            report["checks"].append("attach --observe on an already attached local-owned view preserves the same live PTY when the observing window closes")

            fresh_observer = subprocess.Popen(
                [str(binary), "--observe", "--name", "fresh-observer", "--config", str(config),
                 "-e", "/usr/bin/python3", "-c", "import time; time.sleep(120)"],
                env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True,
            )
            owned.append(fresh_observer)
            wait(lambda: any(item["name"] == "fresh-observer" for item in rpc({"op": "list"})["sessions"]),
                 "fresh observer startup")
            fresh = rpc({"op": "info", "session": "fresh-observer"})
            assert fresh["attached"] and fresh["read_only"] and fresh["input_owner"] == "local"
            children[fresh["pid"]] = Path(f"/proc/{fresh['pid']}/stat").read_text().rsplit(")", 1)[1].split()[19]
            fresh_window = connection.create_resource_object("window", fresh["window_id"])
            fresh_window.send_event(protocol.event.ClientMessage(
                window=fresh_window, client_type=connection.intern_atom("WM_PROTOCOLS"),
                data=(32, [connection.intern_atom("WM_DELETE_WINDOW"), X.CurrentTime, 0, 0, 0]),
            ), event_mask=0)
            connection.flush()
            wait(lambda: not any(item["uri"] == fresh["uri"] for item in rpc({"op": "list"})["sessions"]),
                 "fresh observer retires its own session on window close")
            wait(lambda: not Path(f"/proc/{fresh['pid']}").exists(), "fresh observer child is reaped")
            fresh_observer.wait(timeout=5)
            report["checks"].append("a fresh --observe window owns and reaps only its newly created child when closed")

            rpc({"op": "close", "session": "daily"})
            rpc({"op": "close", "session": "companion"})
            wait(lambda: not rpc({"op": "list"})["sessions"], "session cleanup")
            for process in owned:
                process.wait(timeout=5)
            report["checks"].append("launch waiter exits and session/PTY resources are released")
            report["ok"] = True
        except Exception as error:
            import traceback
            report["error"] = str(error)
            report["traceback"] = traceback.format_exc()
            # Retain the live state before cleanup so an intermittent watcher
            # failure can be diagnosed without weakening the pixel assertion.
            try:
                report["failure_state"] = {"session": info(), "corner_pixel": image().getpixel((1, 1)),
                                           "config_sha256": hashlib.sha256(config.read_bytes()).hexdigest()}
                report["failure_state"]["paths"] = {str(path): {"inode": path.stat().st_ino, "device": path.stat().st_dev}
                    for path in (config, config.parent, config.parent.parent) if path.exists()}
                report["failure_state"]["inotify"] = {fd.name: Path(f"/proc/{broker_pid}/fdinfo/{fd.name}").read_text()
                    for fd in Path(f"/proc/{broker_pid}/fd").iterdir() if os.readlink(fd) == "anon_inode:inotify"}
                capture("failure")
            except Exception as detail:
                report["failure_diagnostic_error"] = str(detail)
            raise
        finally:
            if connection is not None:
                connection.close()
            # Eligible PIDs came from this private broker, or were created by
            # this script. Verify the PTY process birth token before signaling.
            for owned_pid, start_token in children.items():
                try:
                    current = Path(f"/proc/{owned_pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
                    if current == start_token:
                        os.kill(owned_pid, signal.SIGTERM)
                except (FileNotFoundError, ProcessLookupError):
                    pass
            if broker_pid is not None:
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
            if xvfb is not None:
                xvfb.close()
            # Reap only adopted children of this test. Nonblocking and bounded.
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                try:
                    reaped, _ = os.waitpid(-1, os.WNOHANG)
                except ChildProcessError:
                    break
                if reaped == 0:
                    time.sleep(0.02)
            log.close()
            report["stderr"] = (root / "stderr.log").read_text(errors="replace")[-12000:]
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
