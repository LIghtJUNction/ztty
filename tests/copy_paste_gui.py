#!/usr/bin/env python3
"""Check selection, the temporary clipboard menu and paste feedback on owned Xvfb.

Uses real XTEST, native window pixels, selection requests and a raw PTY. No
user desktop, clipboard, broker or session is accessed. Requires python-xlib
and Pillow, as do the existing GUI regressions.
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
import struct
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from isolated_xvfb import start_xvfb

CHILD = r'''import base64,json,os,sys,threading,time,tty
tty.setraw(0)
out=open(sys.argv[1],"wb",buffering=0)
def output():
    previous=0
    while True:
        try:
            request=json.load(open(sys.argv[2]))
            if request["sequence"] != previous:
                previous=request["sequence"]
                os.write(1,base64.b64decode(request["data"]))
                os.write(1,("\x1b]2;copy-"+str(previous)+"\x07").encode())
        except FileNotFoundError: pass
        time.sleep(.01)
threading.Thread(target=output,daemon=True).start()
while True:
    data=os.read(0,65536)
    if not data: break
    out.write(data)
'''

CONFIG = """font = monospace:size=11
background = #0c1218
foreground = #e1e7f0
background-opacity = 100
background-blur = false
cursor-blink = false
window-padding = 16
selection-background = #355577
selection-foreground = #ffffff
"""
LONG_LINE = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnop"
SCENE = ("\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l"
         "\x1b[?25l\x1b[2J\x1b[Halpha beta /tmp/report.txt"
         "\x1b[2;1Hone 中文e\u0301 next\x1b[3;1H" + LONG_LINE +
         "\x1b[6;1Hmouse area\x1b[9;1HREADY")


def process_token(pid):
    return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]


def main():
    from Xlib import X, XK, Xatom, display, protocol
    from Xlib.ext import xtest
    from PIL import Image, ImageChops

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, default=Path("artifacts/copy-paste-gui"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    args.artifacts.mkdir(parents=True, exist_ok=True)
    repository = Path(__file__).resolve().parents[1]
    report = {"ok": False, "checks": [], "screenshots": {},
              "binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "source_sha256": {}, "scope": "Owned Xvfb, XTEST, clipboard and raw PTY"}
    report["source_commit"] = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repository, text=True).strip()
    report["source_changes"] = subprocess.check_output(["git", "status", "--porcelain"], cwd=repository, text=True).splitlines()
    for name in ("src/session.rs", "src/selection.rs", "src/x11.rs", "src/main.rs", "tests/copy_paste_gui.py"):
        report["source_sha256"][name] = hashlib.sha256((repository / name).read_bytes()).hexdigest()
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("PR_SET_CHILD_SUBREAPER failed")
    connection = xvfb = launcher = None
    owned = {}
    clipboard_value = b""
    bad_clipboard_type = False

    def record(name, **details):
        report["checks"].append({"name": name, "ok": True, **details})

    with tempfile.TemporaryDirectory(prefix="ztty-copy-paste-") as temporary:
        root = Path(temporary)
        log = (root / "stderr.log").open("wb")
        try:
            xvfb = start_xvfb(screen="1280x800x24", stderr=log)
            report["display_ownership"] = xvfb.evidence
            connection = display.Display(xvfb.display)
            env = dict(os.environ, DISPLAY=xvfb.display, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                       XDG_RUNTIME_DIR=temporary, XDG_CONFIG_HOME=str(root / "config-home"),
                       ZTTY_SOCKET="@ztty-copy-" + root.name)
            env.pop("WAYLAND_DISPLAY", None)
            config = root / "config"
            config.write_text(CONFIG)
            input_path, request_path = root / "input.bin", root / "output.json"
            clipboard = connection.intern_atom("CLIPBOARD")
            utf8 = connection.intern_atom("UTF8_STRING")
            targets = connection.intern_atom("TARGETS")
            transfer = connection.intern_atom("ZTTY_COPY_TEST")
            owner = connection.screen().root.create_window(0, 0, 1, 1, 0, X.CopyFromParent,
                X.InputOutput, X.CopyFromParent, event_mask=X.PropertyChangeMask)

            def pump():
                while connection.pending_events():
                    event = connection.next_event()
                    if event.type != X.SelectionRequest:
                        continue
                    property_atom = event.property or event.target
                    target = event.target
                    if target == targets:
                        event.requestor.change_property(property_atom, Xatom.ATOM, 32, [utf8])
                    elif target == utf8:
                        if bad_clipboard_type:
                            event.requestor.change_property(property_atom, Xatom.ATOM, 32, [utf8])
                        else:
                            event.requestor.change_property(property_atom, utf8, 8, clipboard_value)
                    else:
                        property_atom = X.NONE
                    event.requestor.send_event(protocol.event.SelectionNotify(
                        time=event.time, requestor=event.requestor, selection=event.selection,
                        target=event.target, property=property_atom), propagate=False)
                    connection.flush()

            def wait(predicate, label, timeout=6):
                deadline = time.monotonic() + timeout
                while time.monotonic() < deadline:
                    pump()
                    try:
                        value = predicate()
                        if value:
                            return value
                    except (FileNotFoundError, ConnectionRefusedError):
                        pass
                    time.sleep(.01)
                raise AssertionError(label)

            def rpc(request):
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                    channel.settimeout(3)
                    channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                    pid, uid, _ = struct.unpack("3i", channel.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
                    assert uid == os.getuid()
                    owned.setdefault(pid, process_token(pid))
                    channel.sendall(json.dumps(request).encode() + b"\n")
                    data = bytearray()
                    while b"\n" not in data:
                        part = channel.recv(8192)
                        if not part:
                            raise RuntimeError("private broker disconnected")
                        data.extend(part)
                        assert len(data) < 1048576
                reply = json.loads(data.split(b"\n", 1)[0])
                assert reply["ok"], reply
                return reply["result"]

            def info():
                return rpc({"op": "info", "session": "copy-test"})

            launcher = subprocess.Popen([str(binary), "--name", "copy-test", "--config", str(config),
                "--geometry", "40x12", "--scrollback", "100", "-e", "/usr/bin/python3", "-c", CHILD,
                str(input_path), str(request_path)], env=env, stdout=subprocess.DEVNULL, stderr=log,
                start_new_session=True)
            owned[launcher.pid] = process_token(launcher.pid)
            wait(lambda: rpc({"op": "list"})["sessions"], "test session starts", 15)
            initial = info()
            owned[initial["pid"]] = process_token(initial["pid"])
            view = connection.create_resource_object("window", initial["window_id"])
            view.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            geometry = view.get_geometry()
            cw, ch = (geometry.width - 32) // 40, (geometry.height - 32) // 12
            report["grid"] = {"cols": 40, "rows": 12, "cell_width": cw, "cell_height": ch}
            sequence = 0

            def scene(extra=""):
                nonlocal sequence
                sequence += 1
                replacement = root / "output.new"
                replacement.write_text(json.dumps({"sequence": sequence,
                    "data": base64.b64encode((SCENE + extra).encode()).decode()}))
                replacement.replace(request_path)
                wait(lambda: view.get_wm_name() == f"copy-{sequence}", "new output painted")

            def image():
                connection.sync()
                pixels = view.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xffffffff)
                return Image.frombytes("RGB", (geometry.width, geometry.height), pixels.data, "raw", "BGRX")

            def capture(name):
                path = args.artifacts / (name + ".png")
                image().save(path)
                report["screenshots"][name] = {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}

            def menu_box():
                masks = [channel.point(lambda value, target=target: 255 if value == target else 0)
                         for channel, target in zip(image().split(), (95, 114, 135))]
                return ImageChops.multiply(ImageChops.multiply(masks[0], masks[1]), masks[2]).getbbox()

            def footer():
                return image().crop((0, geometry.height - 16 - ch, geometry.width, geometry.height)).tobytes()

            def key(*names):
                codes = [connection.keysym_to_keycode(XK.string_to_keysym(name)) for name in names]
                assert all(codes), names
                for code in codes:
                    xtest.fake_input(connection, X.KeyPress, code)
                for code in reversed(codes):
                    xtest.fake_input(connection, X.KeyRelease, code)
                connection.sync()

            def move(col, row):
                view.warp_pointer(16 + col * cw + cw // 2, 16 + row * ch + ch // 2)
                connection.sync()

            def button(value, pressed):
                xtest.fake_input(connection, X.ButtonPress if pressed else X.ButtonRelease, value)
                connection.sync()

            def clicks(col, row, count):
                time.sleep(.45)
                move(col, row)
                for _ in range(count):
                    button(1, True)
                    button(1, False)
                    time.sleep(.04)

            def drag(start, end, word=False, line=False, shift_release=False):
                time.sleep(.45)
                move(*start)
                for _ in range(2 if line else 1 if word else 0):
                    button(1, True)
                    button(1, False)
                    time.sleep(.04)
                button(1, True)
                if shift_release:
                    xtest.fake_input(connection, X.KeyRelease, connection.keysym_to_keycode(XK.XK_Shift_L))
                move(*end)
                button(1, False)

            def open_menu(col=24, row=5, shift=False):
                move(col, row)
                code = connection.keysym_to_keycode(XK.XK_Shift_L)
                if shift:
                    xtest.fake_input(connection, X.KeyPress, code)
                button(3, True)
                if shift:
                    xtest.fake_input(connection, X.KeyRelease, code)
                button(3, False)
                return wait(menu_box, "clipboard menu appears")

            def choose(row):
                box = wait(menu_box, "clipboard menu before choosing")
                view.warp_pointer((box[0] + box[2]) // 2, box[1] + 1 + row * (ch + 8) + (ch + 8) // 2)
                connection.sync()
                button(1, True)
                button(1, False)
                wait(lambda: menu_box() is None, "menu closes after choosing")

            def set_clipboard(text, bad=False):
                nonlocal clipboard_value, bad_clipboard_type
                clipboard_value = text.encode()
                bad_clipboard_type = bad
                owner.set_selection_owner(clipboard, X.CurrentTime)
                connection.sync()
                assert connection.get_selection_owner(clipboard).id == owner.id

            def selection(atom):
                owner.delete_property(transfer)
                owner.convert_selection(atom, utf8, transfer, X.CurrentTime)
                connection.flush()
                value = wait(lambda: owner.get_full_property(transfer, utf8), "selection UTF-8 response")
                return bytes(value.value).decode()

            def expect_selection(atom, expected):
                wait(lambda: selection(atom) == expected, f"expected selection {expected!r}")

            def received():
                return input_path.read_bytes() if input_path.exists() else b""

            def expect_input(expected):
                wait(lambda: received() == expected, f"PTY bytes {expected!r}")

            scene()
            clean_footer = footer()
            set_clipboard("preserved clipboard")
            drag((4, 0), (0, 0))
            expect_selection(Xatom.PRIMARY, "alpha")
            expect_selection(clipboard, "preserved clipboard")
            key("Control_L", "Shift_L", "c")
            expect_selection(clipboard, "alpha")
            wait(lambda: footer() != clean_footer, "copy notice visible")
            capture("copied")
            wait(lambda: footer() == clean_footer, "copy notice expires")
            assert received() == b""
            record("reverse drag keeps PRIMARY separate; explicit copy writes clipboard and transient feedback expires")

            clicks(16, 0, 2)
            expect_selection(Xatom.PRIMARY, "/tmp/report.txt")
            box = open_menu()
            menu_pixels = image()
            assert menu_pixels.getpixel((box[0] + 2, box[1] + 2)) == (53, 85, 119), "menu uses configured selection background"
            highlighted = menu_pixels.crop((box[0] + 1, box[1] + 1, box[2] - 1, box[1] + ch + 9))
            assert any(rgb == (255, 255, 255) for _, rgb in highlighted.getcolors(highlighted.width * highlighted.height)), "menu uses configured selection foreground"
            capture("copy-paste-menu")
            assert received() == b""
            choose(0)
            expect_selection(clipboard, "/tmp/report.txt")
            record("double-click selects a path; right-click preserves it and only an explicit Copy item copies", selected_background="#355577", selected_foreground="#ffffff")

            clicks(5, 1, 2)
            expect_selection(Xatom.PRIMARY, "中文e\u0301")
            key("Control_L", "Shift_L", "c")
            expect_selection(clipboard, "中文e\u0301")
            capture("unicode-word")
            record("wide-character continuation and combining mark are copied as whole Unicode text")

            clicks(5, 3, 3)
            expect_selection(Xatom.PRIMARY, LONG_LINE)
            key("Control_L", "Shift_L", "c")
            expect_selection(clipboard, LONG_LINE)
            capture("wrapped-line")
            record("triple-click joins a soft-wrapped logical line without adding a newline")

            drag((7, 0), (23, 0), word=True)
            expect_selection(Xatom.PRIMARY, "beta /tmp/report.txt")
            drag((18, 0), (7, 0), word=True)
            expect_selection(Xatom.PRIMARY, "beta /tmp/report.txt")
            record("double-click then drag expands by whole words in either direction")

            drag((3, 0), (8, 1), line=True)
            expect_selection(Xatom.PRIMARY, "alpha beta /tmp/report.txt\none 中文e\u0301 next")
            record("triple-click then drag expands by complete lines and preserves a hard newline")

            scene("\x1b[?1000h\x1b[?1006h")
            move(3, 5)
            button(3, True)
            button(3, False)
            expected = b"\x1b[<2;4;6M\x1b[<2;4;6m"
            expect_input(expected)
            assert menu_box() is None
            shift = connection.keysym_to_keycode(XK.XK_Shift_L)
            xtest.fake_input(connection, X.KeyPress, shift)
            drag((0, 0), (4, 0), shift_release=True)
            expect_selection(Xatom.PRIMARY, "alpha")
            open_menu(shift=True)
            key("Escape")
            wait(lambda: menu_box() is None, "Escape closes Shift menu")
            assert received() == expected
            record("TUI right-click remains an SGR press/release; Shift bypass owns drag/menu releases after Shift lifts")

            scene("\x1b[?2004h")
            set_clipboard("first\n第二行\tend")
            open_menu()
            assert received() == expected
            choose(1)
            expected += "\x1b[200~first\n第二行\tend\x1b[201~".encode()
            expect_input(expected)
            capture("pasted")
            record("right-click alone sends nothing; Paste delivers exact multiline UTF-8 bracketed bytes")

            set_clipboard("keyboard paste")
            key("Control_L", "Shift_L", "v")
            expected += b"\x1b[200~keyboard paste\x1b[201~"
            expect_input(expected)
            move(4, 5)
            button(2, True)
            button(2, False)
            expected += b"\x1b[200~alpha\x1b[201~"
            expect_input(expected)
            record("clipboard shortcut and middle-button PRIMARY paste keep their existing meanings")

            clicks(2, 0, 2)
            set_clipboard("menu keyboard")
            open_menu()
            key("Down")
            key("Return")
            expected += b"\x1b[200~menu keyboard\x1b[201~"
            expect_input(expected)
            assert menu_box() is None
            record("arrow and Enter operate the temporary menu without leaking navigation keys")

            wait(lambda: footer() == clean_footer, "paste notice expires")
            set_clipboard("")
            key("Control_L", "Shift_L", "v")
            wait(lambda: footer() != clean_footer, "empty clipboard notice")
            capture("empty-clipboard")
            assert received() == expected
            wait(lambda: footer() == clean_footer, "empty notice expires")
            set_clipboard("invalid clipboard", bad=True)
            key("Control_L", "Shift_L", "v")
            wait(lambda: footer() != clean_footer, "transfer error notice")
            capture("paste-error")
            assert received() == expected
            record("empty and rejected transfers show feedback without sending a paste bracket or partial text")

            scene()
            open_menu(39, 11)
            box = menu_box()
            assert 0 <= box[0] < box[2] <= geometry.width and 0 <= box[1] < box[3] <= geometry.height
            view.warp_pointer(1, 1)
            connection.sync()
            button(1, True)
            button(1, False)
            wait(lambda: menu_box() is None, "outside click dismisses menu")
            assert received() == expected
            record("menu clamps to window edges; outside click dismisses without reaching the PTY")

            clicks(2, 0, 2)
            rpc({"op": "attach", "session": "copy-test", "observe": True})
            assert info()["read_only"]
            set_clipboard("must not reach observer")
            open_menu()
            capture("read-only-menu")
            choose(1)
            assert received() == expected and info()["read_only"]
            key("Control_L", "Shift_L", "c")
            expect_selection(clipboard, "alpha")
            record("read-only view can copy while its Paste item stays disabled and cannot acquire input")

            report["ok"] = True
        except Exception as error:
            import traceback
            report["error"] = str(error)
            report["traceback"] = traceback.format_exc()
            try:
                report["received_hex"] = received().hex()
                capture("failure")
            except Exception:
                pass
        finally:
            if connection is not None:
                connection.close()
            for pid, token in reversed(list(owned.items())):
                try:
                    if process_token(pid) == token:
                        os.kill(pid, signal.SIGTERM)
                except (FileNotFoundError, ProcessLookupError):
                    pass
            if launcher is not None:
                try:
                    launcher.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    launcher.kill()
                    launcher.wait(timeout=3)
            if xvfb is not None:
                xvfb.close()
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                try:
                    pid, _ = os.waitpid(-1, os.WNOHANG)
                except ChildProcessError:
                    break
                if pid == 0:
                    time.sleep(.02)
            report["remaining_owned_pids"] = []
            for pid, token in owned.items():
                try:
                    if process_token(pid) == token:
                        report["remaining_owned_pids"].append(pid)
                except FileNotFoundError:
                    pass
            if report["remaining_owned_pids"]:
                report["ok"] = False
            log.close()
            report["stderr_tail"] = (root / "stderr.log").read_text(errors="replace")[-3000:]
            (args.artifacts / "acceptance.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
