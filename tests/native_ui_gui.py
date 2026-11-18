#!/usr/bin/env python3
"""Exercise ZUI/1 through a real Linux window, private PTY and owned Xvfb.

Checks actual KGP-backed pixels, structured click/keyboard intents, complete
gesture consumption, stale revisions, bounds, resize, lifecycle and observers.
"""
import argparse
import base64
import ctypes
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import time

from input_gui import CHILD, CONFIG

PREFIX = b"\x1b_ZUI;1;"
END = b"\x1b\\"
IMAGE_ID = 7001
RED = (230, 70, 60)
BLUE = (60, 130, 230)


def frame(value):
    return (PREFIX + json.dumps(value, separators=(",", ":")).encode() + END).decode()


def scene(revision, events=True):
    return {
        "v": 1, "op": "replace", "surface": 9, "rev": revision, "root": 1, "events": events,
        "nodes": [
            {"id": 1, "type": "column", "children": [2, 3, 4, 7]},
            {"id": 2, "type": "text", "style": "heading", "text": "Native scene · measured input and image pixels"},
            {"id": 3, "type": "progress", "value": 5000, "label": "A bounded scene update"},
            {"id": 4, "type": "row", "children": [5, 6]},
            {"id": 5, "type": "image", "image_id": IMAGE_ID, "alt": "Image unavailable", "cols": 12, "rows": 4},
            {"id": 6, "type": "image", "image_id": IMAGE_ID, "alt": "Image unavailable", "cols": 12, "rows": 4},
            {"id": 7, "type": "button", "label": "Send structured action", "action": "confirm", "enabled": True},
        ],
    }


def image_frame(left=RED, right=BLUE):
    pixels = bytes((*left, 255)) * 16 + bytes((*right, 255)) * 16
    payload = base64.b64encode(pixels * 16).decode()
    return f"\x1b_Ga=t,f=32,s=32,v=16,i={IMAGE_ID},q=2;{payload}\x1b\\"


def main():
    from Xlib import X, XK, display
    from Xlib.ext import xtest
    from PIL import Image
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--source-root", required=True, type=Path)
    parser.add_argument("--output", type=Path, default=Path("artifacts/native-ui-gui.json"))
    parser.add_argument("--screenshot", type=Path, default=Path("artifacts/native-ui-gui.png"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    source = args.source_root.resolve(strict=True)
    sys.path.insert(0, str(source / "tools"))
    from isolated_xvfb import start_xvfb
    report = {
        "ok": False, "platform": "Linux X11 / private Xvfb",
        "binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "source_root": str(source),
        "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True).strip(),
        "source_changes": subprocess.check_output(["git", "status", "--short"], cwd=source, text=True).splitlines(),
        "source_sha256": {}, "checks": [], "actions": [],
    }
    for filename in ("src/native_ui.rs", "src/session.rs", "src/session_core.rs", "src/x11.rs",
                     "crates/ztty-ui/src/lib.rs", "crates/ztty-ui/src/wire.rs", "crates/zterm/src/terminal.rs",
                     "Cargo.toml", "Cargo.lock",
                     "tests/input_gui.py", "tests/native_ui_gui.py", "examples/native_ui.py", "tools/isolated_xvfb.py"):
        path = source / filename
        report["source_sha256"][filename] = hashlib.sha256(path.read_bytes()).hexdigest()
    assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0
    xvfb = connection = launcher = None
    owned = {}

    def wait(predicate, label, seconds=10):
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

    def birth(pid):
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]

    def descendants():
        result, pending = {}, [os.getpid()]
        while pending:
            parent = pending.pop()
            try:
                children = Path(f"/proc/{parent}/task/{parent}/children").read_text().split()
            except FileNotFoundError:
                continue
            for child in children:
                pid = int(child)
                try:
                    token = birth(pid)
                except FileNotFoundError:
                    continue
                if pid not in result:
                    result[pid] = token
                    pending.append(pid)
        return result

    with tempfile.TemporaryDirectory(prefix="ztty-native-ui-") as temporary:
        root = Path(temporary)
        log = (root / "stderr.log").open("wb")
        try:
            xvfb = start_xvfb(screen="1400x900x24", stderr=log)
            report["xvfb"] = xvfb.evidence
            connection = display.Display(xvfb.display)
            assert connection.has_extension("XTEST")
            env = dict(os.environ, DISPLAY=xvfb.display, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                       XDG_RUNTIME_DIR=temporary, XDG_CONFIG_HOME=str(root / "config-home"),
                       ZTTY_SOCKET="@ztty-native-ui-" + root.name)
            env.pop("WAYLAND_DISPLAY", None)
            config = root / "config"
            config.write_text(CONFIG)
            input_path, control_path = root / "input.bin", root / "control.sock"
            child = CHILD.replace("len(request)>65536", "len(request)>262144")
            child = child.replace("command=json.loads(request)", 'command=json.loads(request)\n        if command.get("exit"):\n            connection.sendall(b\'{"ok":true}\\n\');raise SystemExit(0)')

            def rpc(request):
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                    channel.settimeout(5)
                    channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                    channel.sendall(json.dumps(request).encode() + b"\n")
                    data = bytearray()
                    while b"\n" not in data:
                        block = channel.recv(8192)
                        if not block:
                            raise EOFError("broker disconnected")
                        data.extend(block)
                value = json.loads(data.split(b"\n", 1)[0])
                assert value["ok"], value
                return value["result"]

            def info():
                return rpc({"op": "info", "session": "ui-probe"})

            def control(output="", clear=False, exit=False):
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                    channel.settimeout(10)
                    channel.connect(str(control_path))
                    channel.sendall(json.dumps({"output": output, "clear": clear, "exit": exit}).encode() + b"\n")
                    data = bytearray()
                    while b"\n" not in data:
                        block = channel.recv(8192)
                        if not block:
                            raise EOFError("PTY control disconnected")
                        data.extend(block)
                value = json.loads(data)
                assert value["ok"], value

            launcher = subprocess.Popen([
                str(binary), "--name", "ui-probe", "--config", str(config), "--geometry", "100x28",
                "-e", "/usr/bin/python3", "-c", child, str(input_path), str(control_path),
            ], env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
            wait(lambda: launcher.poll() is None and rpc({"op": "list"})["sessions"], "session startup")
            session = info()
            for pid in (session["pid"], session["broker_pid"]):
                owned[pid] = birth(pid)
            wait(lambda: control_path.exists() and "READY" in rpc({"op": "read", "session": "ui-probe"})["screen"], "raw PTY ready")
            view = connection.create_resource_object("window", session["window_id"])
            view.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            geometry = view.get_geometry()
            cw, ch = geometry.width // session["cols"], geometry.height // session["rows"]
            report["grid"] = {"cell_width": cw, "cell_height": ch, "width": geometry.width, "height": geometry.height}
            panel_width = min(760, geometry.width - 24)
            inner_width = panel_width - 32
            image_y = 3 * ch + 70
            image_width = min(12 * cw, (inner_width - 12) // 2)
            image_height = min(4 * ch, image_width // 2)
            image_width = image_height * 2
            image_x = (28, 28 + (inner_width - 12) // 2 + 12)
            button = (28 + inner_width // 2, 7 * ch + 80 + (ch + 16) // 2)

            def screenshot():
                geo = view.get_geometry()
                pixels = view.get_image(0, 0, geo.width, geo.height, X.ZPixmap, 0xFFFFFFFF)
                return Image.frombytes("RGB", (geo.width, geo.height), pixels.data, "raw", "BGRX")

            def image_pixels(left=RED, right=BLUE):
                picture = screenshot()
                for x in image_x:
                    if picture.getpixel((x + image_width // 4, image_y + image_height // 2)) != left:
                        return False
                    if picture.getpixel((x + 3 * image_width // 4, image_y + image_height // 2)) != right:
                        return False
                return True

            def move(x, y):
                view.warp_pointer(x, y)
                connection.sync()

            def button_event(press):
                xtest.fake_input(connection, X.ButtonPress if press else X.ButtonRelease, 1)
                connection.sync()

            def click():
                move(*button)
                button_event(True)
                button_event(False)

            def key(name):
                code = connection.keysym_to_keycode(XK.string_to_keysym(name))
                assert code
                xtest.fake_input(connection, X.KeyPress, code)
                xtest.fake_input(connection, X.KeyRelease, code)
                connection.sync()

            def chord(*names):
                codes = [connection.keysym_to_keycode(XK.string_to_keysym(name)) for name in names]
                assert all(codes)
                for code in codes:
                    xtest.fake_input(connection, X.KeyPress, code)
                for code in reversed(codes):
                    xtest.fake_input(connection, X.KeyRelease, code)
                connection.sync()

            def sample():
                time.sleep(0.12)
                control()
                return input_path.read_bytes()

            def action(label, revision):
                wait(lambda: input_path.read_bytes(), label)
                raw = sample()
                assert raw.startswith(PREFIX) and raw.endswith(END), (label, raw.hex())
                packet = json.loads(raw[len(PREFIX):-len(END)])
                assert packet["type"] == "action" and packet["surface"] == 9 and packet["rev"] == revision
                assert packet["node"] == 7 and packet["action"] == "confirm", packet
                report["actions"].append(packet)
                report["checks"].append(label)
                control(clear=True)
                return packet

            control(clear=True)
            control(frame({"v": 1, "op": "capabilities"}))
            raw = sample()
            assert raw.startswith(PREFIX) and raw.endswith(END)
            capabilities = json.loads(raw[len(PREFIX):-len(END)])
            assert capabilities["supported"] is True and capabilities["events"] is True
            report["capabilities"] = capabilities
            def server_pixmap_bytes():
                if not connection.has_extension("X-Resource"):
                    return None
                result = connection.res_query_client_pixmap_bytes(view.id)
                return result.bytes + (result.bytes_overflow << 32)

            pixmaps_before = server_pixmap_bytes()
            control("\x1b[?1003h\x1b[?1006h" + image_frame() + frame(scene(1)), clear=True)
            wait(image_pixels, "two real image references")
            report["checks"].append("one transmitted KGP image renders correct red/blue pixels at two native scene references")
            pixmaps_after = server_pixmap_bytes()
            report["image_server_pixmap_bytes"] = {
                "before": pixmaps_before, "after": pixmaps_after,
                "delta": None if pixmaps_before is None else pixmaps_after - pixmaps_before,
                "rgba_image_bytes": 32 * 16 * 4,
                "references": 2,
            }
            if pixmaps_before is not None:
                assert pixmaps_after - pixmaps_before == 32 * 16 * 4, report["image_server_pixmap_bytes"]
                report["checks"].append("XRes measures one 2048-byte server pixmap for two image references")


            click()
            first = action("mouse click emits only a surface/revision/node-bound action, with no PTY mouse bytes", 1)
            key("x")
            key("Tab")
            key("Return")
            second = action("button focus consumes ordinary text and Tab; Enter emits one structured action", 1)
            assert second["event_id"] > first["event_id"]
            key("Escape")
            key("x")
            assert sample() == b"x"
            control(clear=True)
            report["checks"].append("Escape returns ordinary typing to the PTY and consumes its own gesture")

            move(*button)
            button_event(True)
            chord("Control_L", "equal")
            wait(lambda: info()["cols"] != session["cols"], "font zoom while native button focused")
            button_event(False)
            assert sample() == b""
            chord("Control_L", "0")
            wait(lambda: info()["cols"] == session["cols"], "font reset")
            wait(image_pixels, "image references after font reset")
            control(clear=True)
            report["checks"].append("view-only font zoom works during button focus, cancels captured geometry and consumes release")

            move(*button)
            button_event(True)
            move(geometry.width - 5, geometry.height - 5)
            button_event(False)
            assert sample() == b""
            control(clear=True)
            report["checks"].append("drag/release outside a button never activates or leaks a mouse gesture")

            move(*button)
            button_event(True)
            connection.screen().root.set_input_focus(X.RevertToNone, X.CurrentTime)
            connection.sync()
            time.sleep(0.1)
            button_event(False)
            assert sample() == b""
            view.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            control(clear=True)
            report["checks"].append("focus loss cancels a captured click while still consuming its release")

            move(*button)
            button_event(True)
            control(frame({"v": 1, "op": "update", "surface": 9, "base_rev": 1, "rev": 2,
                           "set": [{"id": 3, "value": 7500}]}), clear=True)
            button_event(False)
            assert sample() == b""
            report["checks"].append("a revision change between press and release cancels activation and consumes release")

            # A malformed or oversized packet must not alter the accepted scene,
            # contaminate ordinary terminal cells, or consume the following DSR.
            control("\x1b_ZUI;1;{broken\x1b\\", clear=True)
            control("\x1b_ZUI;1;" + " " * 65560 + "\x1b\\", clear=True)
            click()
            action("malformed and oversized payloads preserve revision 2 and recover for the following VT traffic", 2)
            cells = rpc({"op": "read", "session": "ui-probe"})["screen"]
            assert "ZUI" not in cells and "Native scene" not in cells
            wait(image_pixels, "images preserved after malformed payload")

            # A retransmission changes image generation without copying pixels
            # into the scene or resending the scene itself.
            control(image_frame(BLUE, RED), clear=True)
            wait(lambda: image_pixels(BLUE, RED), "image generation replacement")
            report["checks"].append("retransmitting the same KGP ID refreshes both scene references without a scene update")
            control(image_frame(), clear=True)
            wait(image_pixels, "image colors restored")

            # Use the stable image as a paint barrier for a separate content-only
            # benchmark. DSR measures parser/event-loop round trips, not display latency.
            timings = []
            revision = 2
            for value in range(24):
                started = time.perf_counter()
                control(frame({"v": 1, "op": "update", "surface": 9, "base_rev": revision,
                               "rev": revision + 1, "set": [{"id": 3, "value": value * 400}]}), clear=True)
                timings.append((time.perf_counter() - started) * 1000)
                revision += 1
            report["update_round_trip_ms"] = {
                "samples": len(timings), "median": statistics.median(timings), "max": max(timings),
                "scope": "PTY JSON update through DSR parser/event-loop barrier; excludes native display latency",
            }
            wait(image_pixels, "image pixels after updates")

            move(*button)
            button_event(True)
            view.configure(width=geometry.width - cw * 4, height=geometry.height - ch)
            connection.sync()
            wait(lambda: view.get_geometry().width != geometry.width, "pane resize")
            time.sleep(0.1)
            button_event(False)
            assert sample() == b""
            view.configure(width=geometry.width, height=geometry.height)
            connection.sync()
            wait(lambda: view.get_geometry().width == geometry.width, "pane restore")
            wait(image_pixels, "scene after pane restore")
            report["checks"].append("pane resize invalidates the captured hit and relayout restores actual image pixels")

            move(*button)
            button_event(True)
            control("\x1b[?1049h", clear=True)
            button_event(False)
            assert sample() == b""
            control("\x1b[?1049l" + frame(scene(revision + 1)), clear=True)
            revision += 1
            wait(image_pixels, "new scene after alternate-screen lifecycle")
            report["checks"].append("alternate-screen transition clears the surface and cancels a pending gesture")

            move(*button)
            button_event(True)
            control("\x1bc", clear=True)
            button_event(False)
            assert sample() == b""
            revision += 1
            control(image_frame() + frame(scene(revision)), clear=True)
            wait(image_pixels, "scene after RIS")
            report["checks"].append("RIS clears surface/actions; revision floor survives and a fresh scene can be drawn")

            move(*button)
            button_event(True)
            control(frame({"v": 1, "op": "clear", "surface": 9, "base_rev": revision, "rev": revision + 1}), clear=True)
            revision += 1
            button_event(False)
            assert sample() == b""
            wait(lambda: not image_pixels(), "explicit clear")
            report["checks"].append("explicit clear removes image UI and cancels the captured click")
            revision += 1
            control(frame(scene(revision, events=False)), clear=True)
            wait(image_pixels, "read-only scene preview")
            click()
            assert sample() == b""
            report["checks"].append("events:false previews the scene but never emits an action")
            revision += 1
            control(frame(scene(revision)), clear=True)
            wait(image_pixels, "interactive scene restored")

            # Preserve a useful final screenshot before checking observer/EOF.
            args.screenshot.parent.mkdir(parents=True, exist_ok=True)
            screenshot().save(args.screenshot)
            report["screenshot"] = {"path": str(args.screenshot),
                                    "sha256": hashlib.sha256(args.screenshot.read_bytes()).hexdigest()}
            rpc({"op": "attach", "session": "ui-probe", "observe": True})
            time.sleep(0.1)
            click()
            key("Return")
            assert sample() == b""
            report["checks"].append("an observer cannot emit action intents or ordinary PTY input")
            control(exit=True)
            wait(lambda: not rpc({"op": "list"})["sessions"] or not info()["running"], "PTY EOF")
            time.sleep(0.1)
            retained = bool(rpc({"op": "list"})["sessions"])
            if retained and info().get("window_id"):
                wait(lambda: not image_pixels(), "EOF surface cleared")
                click()
                time.sleep(0.1)
            assert input_path.read_bytes() == b""
            report["checks"].append("PTY EOF clears the surface and leaves no actionable retained UI")
            if retained:
                rpc({"op": "close", "session": "ui-probe"})
            wait(lambda: not rpc({"op": "list"})["sessions"], "session cleanup")
            launcher.wait(timeout=5)

            # Run the shipped producer too: an actual click must travel back
            # through its decoder and update the native progress pixels.
            launcher = subprocess.Popen([
                str(binary), "--name", "ui-demo", "--config", str(config), "--geometry", "100x28",
                "-e", "/usr/bin/python3", str(source / "examples/native_ui.py"),
            ], env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
            wait(lambda: launcher.poll() is None and rpc({"op": "list"})["sessions"], "demo startup")
            demo_session = rpc({"op": "info", "session": "ui-demo"})
            for pid in (demo_session["pid"], demo_session["broker_pid"]):
                owned[pid] = birth(pid)
            view = connection.create_resource_object("window", demo_session["window_id"])
            view.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            demo_geo = view.get_geometry()
            demo_cw = demo_geo.width // demo_session["cols"]
            demo_ch = demo_geo.height // demo_session["rows"]
            demo_width = min(760, demo_geo.width - 24) - 32
            progress_y = 4 * demo_ch + 62
            progress_x = 28 + demo_width * 9 // 10
            wait(lambda: screenshot().getpixel((progress_x, progress_y)) == (49, 65, 85), "demo initial progress")
            demo_image_y = 4 * demo_ch + 80
            demo_image_height = min(9 * demo_ch, 52 * demo_cw // 2)
            pixel = screenshot().getpixel((28 + demo_image_height // 2, demo_image_y + demo_image_height // 2))
            assert pixel[0] == 230 and pixel[2] == 60 and 99 <= pixel[1] <= 103, pixel
            report["checks"].append("shipped Python producer negotiates supported ZUI/1 and renders its actual generated image")
            move(28 + (demo_width - 12) // 4, 13 * demo_ch + 90 + (demo_ch + 16) // 2)
            button_event(True)
            button_event(False)
            wait(lambda: screenshot().getpixel((progress_x, progress_y)) == (138, 180, 248),
                 "demo action round trip updates progress")
            demo_shot = args.screenshot.with_name(args.screenshot.stem + "-demo.png")
            screenshot().save(demo_shot)
            report["demo_screenshot"] = {"path": str(demo_shot), "sha256": hashlib.sha256(demo_shot.read_bytes()).hexdigest()}
            report["checks"].append("real click -> producer action decoder -> revision update reaches 100% native progress pixels")
            key("Escape")
            key("q")
            wait(lambda: not rpc({"op": "list"})["sessions"], "demo clean exit")
            assert launcher.wait(timeout=5) == 0
            report["ok"] = True
        except Exception as error:
            import traceback
            report["error"], report["traceback"] = str(error), traceback.format_exc()
            if "input_path" in locals() and input_path.exists():
                report["last_input_hex"] = input_path.read_bytes().hex()[-8192:]
            raise
        finally:
            if connection is not None:
                connection.close()
            for pid, token in owned.items():
                try:
                    if birth(pid) == token:
                        os.kill(pid, signal.SIGTERM)
                except (FileNotFoundError, ProcessLookupError):
                    pass
            if launcher is not None:
                if launcher.poll() is None:
                    os.killpg(launcher.pid, signal.SIGTERM)
                try:
                    launcher.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(launcher.pid, signal.SIGKILL)
                    launcher.wait(timeout=3)
            if xvfb is not None:
                xvfb.close()
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                for pid, token in descendants().items():
                    try:
                        if birth(pid) == token:
                            os.kill(pid, signal.SIGKILL)
                    except (FileNotFoundError, ProcessLookupError):
                        pass
                try:
                    pid, _ = os.waitpid(-1, os.WNOHANG)
                except ChildProcessError:
                    break
                if pid == 0:
                    time.sleep(0.02)
            remaining = descendants()
            report["cleanup"] = {"owned_processes_remaining": list(remaining)}
            report["ok"] &= not remaining
            log.close()
            report["stderr"] = (root / "stderr.log").read_text(errors="replace")[-12000:]
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(report, ensure_ascii=False, indent=2))
    if not report["ok"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
