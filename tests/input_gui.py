#!/usr/bin/env python3
"""Verify real X11 keyboard/mouse input against a running ztty binary.

Uses an owned Xvfb, private broker socket and raw PTY. XTEST drives every button
supported by the virtual pointer; higher buttons use explicitly labelled
XSendEvent coverage. A separate control socket changes PTY output modes, and
real cursor-position replies provide a barrier before each input assertion.
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


CHILD = r'''import fcntl,json,os,re,select,socket,struct,sys,termios,time,tty
tty.setraw(0)
record=open(sys.argv[1],"w+b",buffering=0)
control=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
control.bind(sys.argv[2]);control.listen(1)
os.write(1,b"\x1b]2;Input protocol verification\x07\x1b[?25l\r\n  Input protocol verification\r\n\r\n  Real X11 events -> raw PTY bytes\r\n  READY\r\n")
while True:
    ready,_,_=select.select([0,control],[],[])
    if 0 in ready:
        data=os.read(0,65536)
        if not data: break
        record.write(data)
    if control not in ready: continue
    connection,_=control.accept()
    with connection:
        connection.settimeout(8)
        request=bytearray()
        while b"\n" not in request:
            part=connection.recv(8192)
            if not part: break
            request.extend(part)
            if len(request)>65536: raise RuntimeError("oversized test control request")
        command=json.loads(request)
        os.write(1,command.get("output","").encode()+b"\x1b[6n")
        data=bytearray();reply=None;deadline=time.monotonic()+8
        while time.monotonic()<deadline:
            if not select.select([0],[],[],max(0,deadline-time.monotonic()))[0]: break
            block=os.read(0,65536)
            if not block: raise EOFError("PTY input closed")
            data.extend(block)
            reply=re.search(rb"\x1b\[[0-9]+;[0-9]+R",data)
            if reply:
                record.write(data[:reply.start()]+data[reply.end():])
                if command.get("clear"):
                    record.seek(0);record.truncate()
                size=list(struct.unpack("HHHH",fcntl.ioctl(0,termios.TIOCGWINSZ,bytes(8))))
                connection.sendall(json.dumps({"ok":True,"dsr":bytes(reply.group()).hex(),"winsize":size}).encode()+b"\n")
                break
        else: raise TimeoutError("cursor-position barrier")
        if not reply: raise TimeoutError("cursor-position barrier")
'''

RESET = "\x1b>\x1b[?1l\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b[=0u"
CONFIG = """font = monospace:size=11
background = #000000
background-opacity = 100
background-blur = false
cursor-blink = false
window-padding = 0
keybind = ctrl+shift+x = search
keybind = ctrl+shift+? = palette
keybind = ctrl+shift+: = palette
mousebind = ctrl+button3 = palette
mousebind = ctrl+button8 = palette
"""


def main():
    from Xlib import X, XK, display, protocol
    from Xlib.ext import xtest
    from PIL import Image

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--source-root", type=Path, required=True,
                        help="source tree used to build the supplied binary; recorded for evidence")
    parser.add_argument("--output", type=Path, default=Path("artifacts/input-gui.json"))
    parser.add_argument("--screenshot", type=Path, default=Path("artifacts/input-gui.png"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    source = args.source_root.resolve(strict=True)
    sys.path.insert(0, str(source / "tools"))
    from isolated_xvfb import start_xvfb
    with binary.open("rb") as file:
        binary_sha = hashlib.file_digest(file, "sha256").hexdigest()
    report = {
        "ok": False, "platform": "Linux X11 / private Xvfb", "binary": str(binary),
        "binary_sha256": binary_sha, "source_root": str(source), "source_sha256": {},
        "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True, timeout=5).strip(),
        "source_changes": subprocess.check_output(["git", "status", "--short"], cwd=source, text=True, timeout=5).splitlines(),
        "checks": [], "vectors": [], "button_injection": {},
    }
    for name in ("src/session.rs", "src/x11.rs", "src/main.rs", "src/actions.rs", "crates/zinput/src/lib.rs", "crates/zterm/src/lib.rs", "tools/isolated_xvfb.py"):
        path = source / name
        if path.is_file():
            report["source_sha256"][name] = hashlib.sha256(path.read_bytes()).hexdigest()
    report["test_sha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("PR_SET_CHILD_SUBREAPER failed")
    xvfb = connection = launcher = None
    owned_pids = {}
    original_mappings = {}

    def wait(predicate, label, seconds=12):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                result = predicate()
                if result:
                    return result
            except (FileNotFoundError, ConnectionRefusedError):
                pass
            time.sleep(0.02)
        raise TimeoutError(label)

    def birth(pid):
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]

    def descendants():
        # PR_SET_CHILD_SUBREAPER also covers a broker that detached before
        # startup failed. Walk only this test's process family, never names.
        result = {}
        pending = [os.getpid()]
        while pending:
            parent = pending.pop()
            try:
                children = Path(f"/proc/{parent}/task/{parent}/children").read_text().split()
            except FileNotFoundError:
                continue
            for child in children:
                pid = int(child)
                try:
                    stat = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
                except FileNotFoundError:
                    continue
                if int(stat[1]) == parent and pid not in result:
                    result[pid] = stat[19]
                    pending.append(pid)
        return result

    with tempfile.TemporaryDirectory(prefix="ztty-input-gui-") as temporary:
        root = Path(temporary)
        log = (root / "stderr.log").open("wb")
        try:
            xvfb = start_xvfb(screen="1400x900x24", stderr=log)
            report["xvfb"] = xvfb.evidence
            connection = display.Display(xvfb.display)
            assert connection.has_extension("XTEST"), "XTEST extension is required"
            # Keep a client connected so Xvfb does not reset this private keymap.
            # The PC option exercises Num Lock/Shift layer inversion; kpdl:dot
            # gives the physical decimal key its normal Delete/decimal pair.
            subprocess.run(
                ["setxkbmap", "-display", xvfb.display, "-layout", "us",
                 "-option", "", "-option", "numpad:pc", "-option", "kpdl:dot"],
                check=True, capture_output=True, timeout=10,
            )
            connection.sync()
            report["keyboard_layout"] = {"layout": "us", "options": ["numpad:pc", "kpdl:dot"]}
            pointer_mapping = list(connection.get_pointer_mapping())
            report["pointer_mapping"] = pointer_mapping
            # Resolve the physical dual-purpose keypad keys, not a preferred
            # keysym alias: Xvfb also has an always-decimal key at another code.
            minimum = connection.display.info.min_keycode
            maximum = connection.display.info.max_keycode
            keyboard_mapping = dict(enumerate(
                connection.get_keyboard_mapping(minimum, maximum - minimum + 1), minimum))
            # Num Lock off uses legacy navigation, or dedicated keypad identity
            # after Kitty disambiguation (functional-key table):
            # https://sw.kovidgoyal.net/kitty/keyboard-protocol/#functional-key-definitions
            keypad_cases = [
                ("KP_0", "KP_Insert", b"\x1b[2~", b"\x1b[57425u"),
                ("KP_1", "KP_End", b"\x1b[F", b"\x1b[57424u"),
                ("KP_2", "KP_Down", b"\x1b[B", b"\x1b[57420u"),
                ("KP_3", "KP_Next", b"\x1b[6~", b"\x1b[57422u"),
                ("KP_4", "KP_Left", b"\x1b[D", b"\x1b[57417u"),
                ("KP_5", "KP_Begin", b"\x1b[1E", b"\x1b[E"),
                ("KP_6", "KP_Right", b"\x1b[C", b"\x1b[57418u"),
                ("KP_7", "KP_Home", b"\x1b[H", b"\x1b[57423u"),
                ("KP_8", "KP_Up", b"\x1b[A", b"\x1b[57419u"),
                ("KP_9", "KP_Prior", b"\x1b[5~", b"\x1b[57421u"),
                ("KP_Decimal", "KP_Delete", b"\x1b[3~", b"\x1b[57426u"),
            ]
            physical_keypad = {}
            report["physical_keypad"] = {}
            for numeric, navigation, _, _ in keypad_cases:
                pair = (XK.string_to_keysym(navigation), XK.string_to_keysym(numeric))
                matches = [code for code, symbols in keyboard_mapping.items()
                           if tuple(symbols[:2]) == pair]
                assert len(matches) == 1, (numeric, navigation, matches)
                physical_keypad[numeric] = matches[0]
                physical_keypad[navigation] = matches[0]
                report["physical_keypad"][numeric] = {
                    "keycode": matches[0], "num_lock_off": navigation,
                    "symbols": list(keyboard_mapping[matches[0]]),
                    "keysym_lookup_keycode": connection.keysym_to_keycode(pair[1]),
                }
            num_lock_codes = [code for code, symbols in keyboard_mapping.items()
                              if symbols[0] == XK.string_to_keysym("Num_Lock")]
            assert len(num_lock_codes) == 1, num_lock_codes
            num_lock_code = num_lock_codes[0]
            num_lock_masks = [1 << index for index, row in enumerate(connection.get_modifier_mapping())
                              if num_lock_code in row]
            assert len(num_lock_masks) == 1, num_lock_masks
            num_lock_mask = num_lock_masks[0]
            report["num_lock_mapping"] = {"keycode": num_lock_code, "modifier_mask": num_lock_mask}
            # A non-US layout can have tilde as an unshifted key. Configure an
            # otherwise unused key in the private display before ztty connects.
            tilde_code = connection.display.info.max_keycode
            original_mappings[tilde_code] = tuple(connection.get_keyboard_mapping(tilde_code, 1)[0])
            connection.change_keyboard_mapping(tilde_code, [(XK.string_to_keysym("asciitilde"),)])
            connection.sync()
            assert connection.get_keyboard_mapping(tilde_code, 1)[0][0] == ord("~")
            report["tilde_keycode"] = tilde_code
            punctuation_code = tilde_code - 1
            original_mappings[punctuation_code] = tuple(connection.get_keyboard_mapping(punctuation_code, 1)[0])
            connection.change_keyboard_mapping(
                punctuation_code, [(ord(";"), ord("?"))])
            connection.sync()
            assert tuple(connection.get_keyboard_mapping(punctuation_code, 1)[0][:2]) == (ord(";"), ord("?"))
            report["punctuation_keycode"] = punctuation_code
            plus_code = tilde_code - 2
            original_mappings[plus_code] = tuple(connection.get_keyboard_mapping(plus_code, 1)[0])
            connection.change_keyboard_mapping(plus_code, [(ord("^"), ord("+"))])
            connection.sync()
            assert tuple(connection.get_keyboard_mapping(plus_code, 1)[0][:2]) == (ord("^"), ord("+"))
            report["non_us_plus_keycode"] = plus_code
            env = dict(os.environ, DISPLAY=xvfb.display, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                       XDG_RUNTIME_DIR=temporary, XDG_CONFIG_HOME=str(root / "xdg-config"),
                       ZTTY_SOCKET="@ztty-input-gui-" + root.name)
            env.pop("WAYLAND_DISPLAY", None)
            config = root / "config"
            config.write_text(CONFIG)
            input_path = root / "input.bin"
            control_path = root / "control.sock"

            def rpc(request):
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                    channel.settimeout(5)
                    channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                    channel.sendall(json.dumps(request).encode() + b"\n")
                    data = bytearray()
                    while b"\n" not in data:
                        part = channel.recv(8192)
                        if not part:
                            raise EOFError("broker disconnected")
                        data.extend(part)
                        if len(data) > 1048576:
                            raise RuntimeError("oversized broker response")
                response = json.loads(data.split(b"\n", 1)[0])
                assert response["ok"], response
                return response["result"]

            def info():
                return rpc({"op": "info", "session": "input-probe"})

            def control(output="", clear=False, path=None):
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                    channel.settimeout(10)
                    channel.connect(str(path or control_path))
                    channel.sendall(json.dumps({"output": output, "clear": clear}).encode() + b"\n")
                    data = bytearray()
                    while b"\n" not in data:
                        part = channel.recv(8192)
                        if not part:
                            raise EOFError("PTY control disconnected before barrier")
                        data.extend(part)
                    response = json.loads(data)
                    assert response["ok"], response
                    return response

            launcher = subprocess.Popen(
                [str(binary), "--name", "input-probe", "--config", str(config), "--geometry", "100x28",
                 "-e", "/usr/bin/python3", "-c", CHILD, str(input_path), str(control_path)],
                env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True,
            )
            wait(lambda: launcher.poll() is None and rpc({"op": "list"})["sessions"], "input session startup")
            session = info()
            for pid in (session["pid"], session["broker_pid"]):
                owned_pids[pid] = birth(pid)
            wait(lambda: control_path.exists() and "READY" in rpc({"op": "read", "session": "input-probe"})["screen"], "raw PTY ready")
            view = connection.create_resource_object("window", session["window_id"])
            view.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            geometry = view.get_geometry()
            cols, rows = session["cols"], session["rows"]
            assert geometry.width % cols == 0 and geometry.height % rows == 0
            cell_width, cell_height = geometry.width // cols, geometry.height // rows
            report["grid"] = {"cols": cols, "rows": rows, "cell_width": cell_width, "cell_height": cell_height}

            def codes(keys):
                values = [
                    key if isinstance(key, int) else physical_keypad.get(key)
                    or connection.keysym_to_keycode(XK.string_to_keysym(key))
                    for key in keys
                ]
                assert all(values), keys
                return values

            def key(*keys):
                values = codes(keys)
                for value in values:
                    xtest.fake_input(connection, X.KeyPress, value)
                for value in reversed(values):
                    xtest.fake_input(connection, X.KeyRelease, value)
                connection.sync()

            def type_text(text):
                for character in text:
                    key("space" if character == " " else character)

            def move(col=7, row=5):
                view.warp_pointer(col * cell_width + cell_width // 2, row * cell_height + cell_height // 2)
                connection.sync()

            def mouse_event(button, pressed, col=7, row=5, state=0):
                if button <= len(pointer_mapping):
                    report["button_injection"][str(button)] = "XTEST"
                    xtest.fake_input(connection, X.ButtonPress if pressed else X.ButtonRelease, button)
                else:
                    report["button_injection"][str(button)] = "XSendEvent (outside virtual pointer button count)"
                    x = col * cell_width + cell_width // 2
                    y = row * cell_height + cell_height // 2
                    event = (protocol.event.ButtonPress if pressed else protocol.event.ButtonRelease)(
                        time=X.CurrentTime, root=connection.screen().root, window=view, child=X.NONE,
                        root_x=x, root_y=y, event_x=x, event_y=y, state=state, same_screen=1, detail=button,
                    )
                    view.send_event(event, event_mask=X.ButtonPressMask if pressed else X.ButtonReleaseMask)

            def click(button, modifiers=()):
                for code in codes(modifiers):
                    xtest.fake_input(connection, X.KeyPress, code)
                mouse_event(button, True)
                mouse_event(button, False)
                for code in reversed(codes(modifiers)):
                    xtest.fake_input(connection, X.KeyRelease, code)
                connection.sync()

            def assert_input(label, expected, injection="XTEST", path=None, barrier_path=None):
                # XSync waits for the X server, not for the application to
                # consume its event queue. Allow the real PTY input to arrive
                # before taking the final output/reply barrier and sample.
                record_path = path or input_path
                deadline = time.monotonic() + 2
                quiet_since = time.monotonic()
                previous = record_path.read_bytes()
                while time.monotonic() < deadline:
                    actual = record_path.read_bytes()
                    if actual != previous:
                        quiet_since = time.monotonic()
                        previous = actual
                    if actual == expected and time.monotonic() - quiet_since >= 0.12:
                        break
                    time.sleep(0.01)
                control(path=barrier_path)
                actual = record_path.read_bytes()
                vector = {"name": label, "injection": injection, "expected_hex": expected.hex(), "actual_hex": actual.hex(), "ok": actual == expected}
                report["vectors"].append(vector)
                assert actual == expected, vector

            def begin(output=""):
                control(output, clear=True)

            begin(RESET)
            key("Control_L", "i")
            assert_input("legacy Ctrl+I", b"\t")
            begin()
            key("Control_L", "Shift_L", "i")
            assert_input("legacy Ctrl+Shift+I remains distinct", b"\x1b[105;6u")
            begin()
            key("Control_L", tilde_code)
            assert_input("Ctrl+unshifted tilde on private keyboard mapping", b"\x1e")
            report["checks"].append("real XTEST keyboard events distinguish Ctrl+Shift+I and retain Ctrl+I and Ctrl+tilde control mappings")

            # Read the real X server modifier state. This display is private,
            # so toggling locks cannot change the user's physical keyboard.
            def num_lock(enabled):
                if bool(connection.screen().root.query_pointer().mask & num_lock_mask) != enabled:
                    key(num_lock_code)
                wait(lambda: bool(connection.screen().root.query_pointer().mask & num_lock_mask) == enabled,
                     "private X server Num Lock state", 2)

            begin(RESET)
            num_lock(True)
            assert_input("Num Lock toggles without writing CSI-u into a legacy shell", b"")
            begin()
            for digit in range(10):
                key(f"KP_{digit}")
            key("KP_Decimal")
            assert_input("Num Lock on: keypad digits and decimal are text", b"0123456789.")
            for flags in range(1, 8):
                begin(RESET + f"\x1b[={flags}u")
                for digit in range(10):
                    key(f"KP_{digit}")
                for symbol in ("KP_Decimal", "KP_Add", "KP_Subtract", "KP_Multiply", "KP_Divide"):
                    key(symbol)
                assert_input(f"Kitty flags {flags}: printable keypad keys remain text",
                             b"0123456789.+-*/")
            begin(RESET + "\x1b[=8u")
            key("KP_1")
            assert_input("Kitty report-all retains keypad identity", b"\x1b[57400;129u")
            begin(RESET + "\x1b[=24u")
            key("KP_1")
            assert_input("Kitty report-all includes requested keypad text", b"\x1b[57400;129;49u")
            # Applications can enable DECKPAM without Kitty reporting.
            # Num Lock must still make the whole numeric keypad usable.
            numeric_keys = [(f"KP_{digit}", str(digit).encode()) for digit in range(10)] + [
                ("KP_Decimal", b"."), ("KP_Add", b"+"), ("KP_Subtract", b"-"),
                ("KP_Multiply", b"*"), ("KP_Divide", b"/"), ("KP_Enter", b"\r"),
            ]
            for symbol, expected in numeric_keys:
                begin(RESET + "\x1b=")
                key(symbol)
                assert_input(f"Num Lock overrides application keypad: {symbol}", expected)
            for flags in range(1, 8):
                begin(RESET + f"\x1b=\x1b[={flags}u")
                for symbol, _ in numeric_keys[:-1]:
                    key(symbol)
                assert_input(f"Kitty flags {flags}: Num Lock overrides application keypad text",
                             b"0123456789.+-*/")
            begin(RESET + "\x1b=")
            key("Alt_L", "KP_Enter")
            assert_input("Num Lock and application keypad: Alt+KP_Enter", b"\x1b\r")

            begin(RESET)
            num_lock(False)
            begin(RESET + "\x1b=\x1b[=1u")
            key("KP_Enter")
            assert_input("Num Lock off: application keypad keeps disambiguated identity",
                         b"\x1b[57414u")
            begin(RESET + "\x1b=\x1b[=3u")
            key("KP_Enter")
            assert_input("Num Lock off: application keypad keeps press and release events",
                         b"\x1b[57414;1:1u\x1b[57414;1:3u")
            begin(RESET)
            num_lock(False)
            assert_input("Num Lock off emits no legacy input", b"")
            for flags in (0, 1):
                for numeric, navigation, legacy, enhanced in keypad_cases:
                    begin(RESET + f"\x1b[={flags}u")
                    key(physical_keypad[numeric])
                    assert_input(f"Num Lock off, Kitty flags {flags}: physical {numeric} sends {navigation}",
                                 enhanced if flags else legacy)
            begin(RESET + "\x1b[=1u")
            num_lock(True)
            assert_input("explicit Kitty opt-in still reports Num Lock and its state", b"\x1b[57360;129u")
            begin(RESET)
            num_lock(False)
            assert_input("return to legacy mode stops reporting lock keys", b"")
            report["checks"].append("Num Lock stays silent in legacy mode; verified physical keypad digits/decimal use text while all eleven navigation keys retain legacy or requested Kitty identity, including Begin; lock reporting stays explicit")

            navigation_keys = [
                ("KP_0", b"\x1b[2~", b"\x1b[2;2~"),
                ("KP_1", b"\x1b[F", b"\x1b[1;2F"),
                ("KP_2", b"\x1b[B", b"\x1b[1;2B"),
                ("KP_3", b"\x1b[6~", b"\x1b[6;2~"),
                ("KP_4", b"\x1b[D", b"\x1b[1;2D"),
                ("KP_5", b"\x1b[1E", b"\x1b[1;2E"),
                ("KP_6", b"\x1b[C", b"\x1b[1;2C"),
                ("KP_7", b"\x1b[H", b"\x1b[1;2H"),
                ("KP_8", b"\x1b[A", b"\x1b[1;2A"),
                ("KP_9", b"\x1b[5~", b"\x1b[5;2~"),
                ("KP_Decimal", b"\x1b[3~", b"\x1b[3;2~"),
            ]
            for enabled in (False, True):
                begin(RESET)
                num_lock(enabled)
                for index, (symbol, navigation, shifted_navigation) in enumerate(navigation_keys):
                    for shifted in (False, True):
                        numeric = enabled != shifted  # the private numpad:pc map
                        expected = (b"." if index == 10 else str(index).encode()) if numeric else (
                            shifted_navigation if shifted else navigation)
                        begin(RESET)
                        key(*( ("Shift_L", symbol) if shifted else (symbol,) ))
                        assert_input(f"PC keypad NumLock={enabled} Shift={shifted}: {symbol}", expected)

            begin(RESET)
            num_lock(False)
            for symbol, suffix in [
                ("KP_1", b"F"), ("KP_2", b"B"), ("KP_4", b"D"),
                ("KP_6", b"C"), ("KP_7", b"H"), ("KP_8", b"A"),
            ]:
                begin(RESET + "\x1b[?1h")
                key(symbol)
                assert_input(f"Application cursor mode routes keypad navigation: {symbol}",
                             b"\x1bO" + suffix)
            application_keys = [(f"KP_{digit}", bytes([ord("p") + digit])) for digit in range(10)] + [
                ("KP_Decimal", b"n"), ("KP_Add", b"k"), ("KP_Subtract", b"m"),
                ("KP_Multiply", b"j"), ("KP_Divide", b"o"), ("KP_Enter", b"M"),
            ]
            for symbol, suffix in application_keys:
                begin(RESET + "\x1b=")
                key(symbol)
                assert_input(f"Num Lock off keeps application keypad: {symbol}", b"\x1bO" + suffix)

            begin(RESET)
            num_lock(True)
            begin(RESET + "\x1b[=3u")
            shift_code, end_code = codes(("Shift_L", "KP_1"))
            xtest.fake_input(connection, X.KeyPress, shift_code)
            xtest.fake_input(connection, X.KeyPress, end_code)
            xtest.fake_input(connection, X.KeyRelease, shift_code)
            xtest.fake_input(connection, X.KeyRelease, end_code)
            connection.sync()
            assert_input("Keypad release retains the pressed identity after Shift is released",
                         b"\x1b[57424;130:1u\x1b[57424;129:3u")

            begin(RESET + "\x1b[=3u")
            pressed = b"\x1b[57424;130:1u"
            repeated = b"\x1b[57424;129:2u"
            released = b"\x1b[57424;129:3u"
            try:
                xtest.fake_input(connection, X.KeyPress, shift_code)
                xtest.fake_input(connection, X.KeyPress, end_code)
                connection.sync()
                wait(lambda: input_path.read_bytes() == pressed, "held keypad press", 2)
                xtest.fake_input(connection, X.KeyRelease, shift_code)
                connection.sync()
                wait(lambda: repeated in input_path.read_bytes(),
                     "server keypad autorepeat retains the pressed identity", 3)
            finally:
                xtest.fake_input(connection, X.KeyRelease, end_code)
                xtest.fake_input(connection, X.KeyRelease, shift_code)
                connection.sync()
            wait(lambda: input_path.read_bytes().endswith(released),
                 "autorepeated keypad release", 2)
            repeat_count = input_path.read_bytes().count(repeated)
            assert repeat_count > 0
            assert_input("Keypad autorepeat and release retain identity after Shift is released",
                         pressed + repeated * repeat_count + released)
            report["keypad_autorepeat_count"] = repeat_count
            begin(RESET)
            num_lock(False)
            begin(RESET)
            report["checks"].append(
                "every physical keypad digit/decimal follows the XKB NumLock/Shift layer; "
                "application cursor/keypad modes, NumLock override and held-key release identity are verified")

            begin(RESET + "\x1b[?1000h\x1b[?1006h")
            move()
            for button, code in [(4, 64), (5, 65), (6, 66), (7, 67)]:
                begin()
                click(button)
                assert_input(f"wheel button {button}: press without release", f"\x1b[<{code};8;6M".encode(), report["button_injection"][str(button)])
            report["checks"].append("vertical and horizontal wheel input has distinct SGR press codes and no release reports")

            for button, code in [(8, 128), (9, 129), (10, 130), (11, 131)]:
                begin()
                click(button)
                expected = f"\x1b[<{code};8;6M\x1b[<{code};8;6m".encode()
                assert_input(f"extra button {button}: SGR press/release", expected, report["button_injection"][str(button)])
            report["checks"].append("extra button press/release reports retain button identity; report records physical XTEST versus synthetic XSendEvent coverage")

            begin(RESET + "\x1b[?1002h\x1b[?1006h")
            for button, code in [(8, 128), (9, 129), (10, 130), (11, 131)]:
                move()
                begin()
                mouse_event(button, True)
                connection.sync()
                # A native XTEST motion is also used for a synthetic high
                # button: it checks the frontend's independently held state.
                move(9, 6)
                mouse_event(button, False, 9, 6)
                connection.sync()
                expected = f"\x1b[<{code};8;6M\x1b[<{code + 32};10;7M\x1b[<{code};10;7m".encode()
                assert_input(f"extra button {button}: drag in mode 1002", expected, report["button_injection"][str(button)])
            report["checks"].append("held extra-button state survives motion and clears on release, including X11 buttons lacking core state-mask bits")

            begin(RESET + "\x1b[?1000h\x1b[?1006h")
            move()
            for button, code in [(1, 0), (8, 128)]:
                begin()
                mouse_event(button, True)
                xtest.fake_input(connection, X.KeyPress, codes(("Shift_L",))[0])
                mouse_event(button, False)
                xtest.fake_input(connection, X.KeyRelease, codes(("Shift_L",))[0])
                connection.sync()
                assert_input(f"button {button}: remote release survives Shift change",
                             f"\x1b[<{code};8;6M\x1b[<{code + 4};8;6m".encode())
                begin()
                xtest.fake_input(connection, X.KeyPress, codes(("Shift_L",))[0])
                mouse_event(button, True)
                xtest.fake_input(connection, X.KeyRelease, codes(("Shift_L",))[0])
                mouse_event(button, False)
                connection.sync()
                assert_input(f"button {button}: local Shift press cannot leak a remote release", b"")
            report["checks"].append("press ownership remains stable when Shift changes before a primary or extra-button release")

            begin(RESET + "\x1b[?1002h\x1b[?1006h\x1b[?1004h")
            move()
            begin()
            mouse_event(8, True)
            connection.sync()
            wait(lambda: input_path.read_bytes() == b"\x1b[<128;8;6M", "remote press before focus loss", 2)
            connection.screen().root.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            assert connection.get_input_focus().focus.id == connection.screen().root.id
            # Focus reports arrive through the real PTY only after the
            # frontend handles each transition, even under scheduling load.
            focus_out = b"\x1b[<128;8;6M\x1b[O"
            wait(lambda: input_path.read_bytes() == focus_out, "PTY focus-out acknowledgment", 2)
            view.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            focus_in = focus_out + b"\x1b[I"
            wait(lambda: input_path.read_bytes() == focus_in, "PTY focus-in acknowledgment", 2)
            report["focus_acknowledgments"] = {"focus_out_hex": focus_out.hex(), "focus_in_hex": focus_in.hex()}
            begin()
            move(9, 6)
            mouse_event(8, False, 9, 6)
            connection.sync()
            assert_input("focus loss clears extra-button drag and unmatched release", b"")
            report["checks"].append("focus loss clears held extra-button state before focus returns, motion, and release")

            begin(RESET + "\x1b[?9h")
            move()
            begin()
            click(1, ("Control_L", "Alt_L"))
            assert_input("X10 primary press ignores modifiers and release", bytes([27, 91, 77, 32, 40, 38]))
            begin()
            click(4)
            click(8)
            move(9, 6)
            assert_input("X10 suppresses wheel extra-button and motion", b"")
            begin("\x1b[?1006h")
            move()
            begin()
            click(1)
            assert_input("X10 tracking is independent from SGR encoding", b"\x1b[<0;8;6M")
            begin("\x1b[?1000h\x1b[?9l")
            click(1)
            assert_input("reset X10 does not cancel subsequently enabled normal tracking", b"\x1b[<0;8;6M\x1b[<0;8;6m")
            report["checks"].append("mode 9 filters input correctly, follows SGR selection, and its reset cannot cancel active mode 1000")

            begin()
            key("Control_L", "Shift_L", "x")
            type_text("query")
            key("Escape")
            assert_input("custom key binding and search input never leak to PTY", b"")
            begin()
            click(3, ("Control_L",))
            type_text("unmatched")
            key("Escape")
            assert_input("custom mouse binding and palette input never leak to PTY", b"")
            begin()
            click(8, ("Control_L",))
            type_text("unmatched")
            key("Escape")
            assert_input("custom extra-button binding and palette input never leak to PTY", b"")
            report["checks"].append("custom keyboard and mouse actions consume their bound events and overlay text while mouse reporting is enabled")

            for keycode, label in [
                ("slash", "US question mark"),
                ("semicolon", "US colon"),
                (punctuation_code, "question mark on a non-US key"),
            ]:
                begin(RESET + "\x1b[=3u")
                key("Control_L", "Shift_L", keycode)
                type_text("palette")
                key("Escape")
                assert_input(f"shifted punctuation binding: {label}", b"")
            report["checks"].append("shifted punctuation bindings consume their press, release and palette input using the active XKB layout")
            begin(RESET)

            # Default lookup must work without a custom binding table.
            base_config = "\n".join(line for line in CONFIG.splitlines()
                                    if line.split("=", 1)[0].strip() not in ("keybind", "mousebind")) + "\n"

            def reload_config(text):
                config.write_text(text)
                rpc({"op": "action", "session": "input-probe", "actor": "local", "action": "reload"})
                begin(RESET)

            def grid():
                current = info()
                return current["cols"], current["rows"]

            def font_metrics():
                reply = control()
                current = info()
                pty_rows, pty_cols, pixel_width, pixel_height = reply["winsize"]
                assert (pty_cols, pty_rows) == (current["cols"], current["rows"]), (reply, current)
                assert pixel_width and pixel_height and pixel_width % pty_cols == 0 and pixel_height % pty_rows == 0
                actual_geometry = view.get_geometry()
                return {"cols": pty_cols, "rows": pty_rows,
                        "cell_width": pixel_width // pty_cols, "cell_height": pixel_height // pty_rows,
                        "pty_winsize": reply["winsize"], "window_pixels": [actual_geometry.width, actual_geometry.height]}

            report["zoom"] = []
            reload_config(base_config)
            base_metrics = font_metrics()

            def zoom(label, chord, expected):
                before = font_metrics()
                begin(RESET + "\x1b[=3u")
                key(*chord)
                target = (base_metrics["cols"], base_metrics["rows"]) if expected == "reset" else None
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline:
                    current = grid()
                    if (current == target if target else current != (before["cols"], before["rows"])):
                        break
                    time.sleep(0.02)
                after = font_metrics()
                if expected == "reset":
                    ok = all(after[field] == base_metrics[field] for field in ("cols", "rows", "cell_width", "cell_height"))
                elif expected == "increase":
                    ok = after["cols"] <= before["cols"] and after["rows"] <= before["rows"] and (
                        after["cell_width"] > before["cell_width"] or after["cell_height"] > before["cell_height"])
                else:
                    ok = after["cols"] >= before["cols"] and after["rows"] >= before["rows"] and (
                        after["cell_width"] < before["cell_width"] or after["cell_height"] < before["cell_height"])
                item = {"name": label, "injection": "XTEST", "keys": list(chord), "expected": expected,
                        "before": before, "after": after, "ok": ok}
                report["zoom"].append(item)
                assert_input(label + ": shortcut press/release stay out of PTY", b"")
                assert after["window_pixels"] == before["window_pixels"], item
                assert ok, item

            # The installed pre-change binary fails this first new vector.
            zoom("Ctrl+Shift+Plus increases font", ("Control_L", "Shift_L", "equal"), "increase")
            zoom("Ctrl+Shift+0 restores configured font", ("Control_L", "Shift_L", "0"), "reset")
            zoom("Ctrl+Shift+Minus decreases font", ("Control_L", "Shift_L", "minus"), "decrease")
            zoom("Ctrl+0 restores configured font", ("Control_L", "0"), "reset")
            zoom("Ctrl+Equal retains font increase", ("Control_L", "equal"), "increase")
            zoom("Ctrl+Minus retains font decrease", ("Control_L", "minus"), "decrease")
            zoom("default plus on non-US physical key", ("Control_L", "Shift_L", plus_code), "increase")
            zoom("restore after non-US zoom", ("Control_L", "0"), "reset")
            begin(RESET)
            num_lock(False)
            zoom("Ctrl+keypad Add with Num Lock off", ("Control_L", "KP_Add"), "increase")
            zoom("Ctrl+keypad Subtract with Num Lock off", ("Control_L", "KP_Subtract"), "decrease")
            begin(RESET)
            num_lock(True)
            zoom("Ctrl+keypad Add with Num Lock on", ("Control_L", "KP_Add"), "increase")
            zoom("Ctrl+keypad 0 resets with Num Lock on", ("Control_L", "KP_0"), "reset")
            begin(RESET)
            num_lock(False)
            begin()
            key("Control_L", "KP_0")
            assert_input("Num Lock off: Ctrl+keypad Insert remains application input", b"\x1b[2;5~")
            assert font_metrics() == base_metrics

            reload_config(base_config + "keybind = ctrl+shift+equal = none\nkeybind = ctrl+kp-add = font-decrease\n")
            begin(RESET + "\x1b[=3u")
            key("Control_L", "Shift_L", "equal")
            assert_input("custom none disables shifted plus and consumes release", b"")
            assert font_metrics() == base_metrics
            zoom("custom keypad remap wins over font increase default", ("Control_L", "KP_Add"), "decrease")
            zoom("restore after custom keypad remap", ("Control_L", "0"), "reset")
            reload_config(base_config + "keybind = ctrl+shift+^ = none\n")
            begin(RESET + "\x1b[=3u")
            key("Control_L", "Shift_L", plus_code)
            assert_input("custom base-key none wins over shifted default plus", b"")
            assert font_metrics() == base_metrics
            reload_config(CONFIG)
            assert font_metrics() == base_metrics
            report["font_restored"] = True
            report["checks"].append("real zoom/reset shortcuts change glyph cell metrics and matching PTY grid, preserve pixel window size and emit no key press/release bytes; default non-US plus and custom overrides are covered")


            # Compare real search selection rendering with the main Enter key,
            # so an ignored keypad event cannot pass a no-PTY-output assertion.
            report["keypad_overlays"] = []

            def window_pixels():
                current = view.get_geometry()
                return view.get_image(0, 0, current.width, current.height,
                                      X.ZPixmap, 0xffffffff).data

            begin(RESET)
            num_lock(False)

            def search_selection(label, chord, expected=None):
                begin(RESET + "\x1b[=3u\x1b[2J\x1b[H"
                      "keypadprobe first\r\n\r\nkeypadprobe second\r\n\r\n"
                      "keypadprobe third\r\n")
                rpc({"op": "action", "session": "input-probe", "actor": "local",
                     "action": "search"})
                key("Control_L", "u")
                type_text("keypadprobe")
                # The PTY barrier emits DSR, which older terminal cores count
                # as a content revision and invalidate the search anchor.
                # Wait for the local search frame without writing PTY output.
                initial = window_pixels()
                unchanged_since = time.monotonic()
                deadline = unchanged_since + 3
                while time.monotonic() < deadline:
                    pixels = window_pixels()
                    if pixels != initial:
                        initial = pixels
                        unchanged_since = time.monotonic()
                    elif time.monotonic() - unchanged_since >= 0.12:
                        break
                    time.sleep(0.01)
                else:
                    raise TimeoutError(label + ": search frame did not settle")
                key(*chord)
                assert_input(label + ": navigation press/release stays local", b"")
                def selected_pixels():
                    pixels = window_pixels()
                    matches = pixels == expected if expected is not None else pixels != initial
                    return pixels if matches else None

                selected = wait(selected_pixels, label + " selects the expected search match", 3)
                assert selected != initial, label
                key("Shift_L")
                assert_input(label + ": bare Shift press/release stays local", b"")
                wait(lambda: window_pixels() == selected,
                     label + ": bare Shift preserves the selected search match", 3)
                key("Escape")
                assert_input(label + ": search exit stays local", b"")
                return selected

            next_match = search_selection("main Enter", ("Return",))
            previous_match = search_selection("main Shift+Enter", ("Shift_L", "Return"))
            assert next_match != previous_match, "forward and reverse search must select different matches"
            search_selection("search keypad Enter", ("KP_Enter",), next_match)
            search_selection("search Shift+keypad Enter", ("Shift_L", "KP_Enter"), previous_match)
            report["keypad_overlays"].append({
                "name": "keypad Enter and Shift+Enter navigate search matches",
                "injection": "XTEST", "num_lock": False, "kitty_flags": 3,
                "reference": "independently reopened main Enter and Shift+Enter searches",
                "fixture_matches": 3, "next_match_equal": True,
                "previous_match_equal": True, "directions_distinct": True,
                "modifier_preserves_selection": True, "ok": True,
            })

            def palette_font(label, navigation, expected):
                before = font_metrics()
                begin(RESET + "\x1b[=3u")
                key("Control_L", "Shift_L", "slash")
                type_text("font")
                for symbol in navigation:
                    key(symbol)
                key("KP_Enter")
                target = (base_metrics["cols"], base_metrics["rows"])
                wait(lambda: (grid() == target if expected == "reset" else
                              grid() != (before["cols"], before["rows"])),
                     label + " changes the PTY font grid", 3)
                after = font_metrics()
                if expected == "reset":
                    ok = all(after[field] == base_metrics[field]
                             for field in ("cols", "rows", "cell_width", "cell_height"))
                elif expected == "increase":
                    ok = after["cell_width"] > before["cell_width"] or after["cell_height"] > before["cell_height"]
                else:
                    ok = after["cell_width"] < before["cell_width"] or after["cell_height"] < before["cell_height"]
                item = {"name": label, "injection": "XTEST", "num_lock": False,
                        "kitty_flags": 3, "keys": [*navigation, "KP_Enter"],
                        "expected": expected, "before": before, "after": after, "ok": ok}
                report["keypad_overlays"].append(item)
                assert_input(label + ": press/release stay out of PTY", b"")
                assert after["window_pixels"] == before["window_pixels"], item
                assert ok, item

            def reset_palette_font():
                rpc({"op": "action", "session": "input-probe", "actor": "local",
                     "action": "font-reset"})
                wait(lambda: grid() == (base_metrics["cols"], base_metrics["rows"]),
                     "restore font after keypad palette command", 3)
                assert font_metrics() == base_metrics

            palette_font("palette keypad Enter executes font increase", (), "increase")
            reset_palette_font()
            palette_font("palette keypad Down selects font decrease", ("KP_Down",), "decrease")
            reset_palette_font()
            rpc({"op": "action", "session": "input-probe", "actor": "local",
                 "action": "font-increase"})
            wait(lambda: grid() != (base_metrics["cols"], base_metrics["rows"]),
                 "prepare enlarged font for keypad Up selection", 3)
            palette_font("palette keypad Up wraps to font reset", ("KP_Up",), "reset")
            assert font_metrics() == base_metrics
            report["checks"].append("keypad Enter and Shift+Enter reproduce search selection; keypad Up/Down choose and execute distinct palette actions without leaking press/release bytes")

            # Release after the press has already moved focus to another tab.
            peer_input = root / "peer-input.bin"
            peer_control = root / "peer-control.sock"
            peer_launcher = subprocess.Popen(
                [str(binary), "--name", "input-peer", "--tab", "input-probe", "--config", str(config),
                 "-e", "/usr/bin/python3", "-c", CHILD, str(peer_input), str(peer_control)],
                env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True,
            )
            wait(lambda: next((item for item in rpc({"op": "list"})["sessions"]
                               if item["name"] == "input-peer"), None), "second raw tab")
            peer = rpc({"op": "info", "session": "input-peer"})
            owned_pids[peer["pid"]] = birth(peer["pid"])
            wait(lambda: peer_control.exists() and peer_input.exists(), "second raw PTY")
            peer_view = connection.create_resource_object("window", peer["window_id"])
            peer_view.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            begin(RESET + "\x1b[=3u")
            control(RESET + "\x1b[=3u", clear=True, path=peer_control)

            def navigate(label, chord, destination):
                assert not rpc({"op": "info", "session": destination})["visible"], label
                values = codes(chord)
                try:
                    for code in values:
                        xtest.fake_input(connection, X.KeyPress, code)
                    connection.sync()
                    wait(lambda: rpc({"op": "info", "session": destination})["visible"], label, 3)
                finally:
                    for code in reversed(values):
                        xtest.fake_input(connection, X.KeyRelease, code)
                    connection.sync()
                assert_input(label + ": original PTY stays silent", b"")
                assert_input(label + ": peer PTY stays silent", b"", path=peer_input, barrier_path=peer_control)

            navigate("Ctrl+Shift+Tab selects previous tab", ("Control_L", "Shift_L", "Tab"), "input-probe")
            navigate("Ctrl+Tab selects next tab", ("Control_L", "Tab"), "input-peer")
            navigate("Ctrl+Shift+Tab returns to original tab", ("Control_L", "Shift_L", "Tab"), "input-probe")
            rpc({"op": "close", "session": "input-peer"})
            peer_launcher.wait(timeout=5)
            view.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            report["checks"].append("Ctrl+Tab and Ctrl+Shift+Tab move between real raw PTYs without leaking a consumed key release after focus changes")
            begin(RESET)
            geometry = view.get_geometry()

            assert info()["pid"] == session["pid"] and info()["running"]
            control(RESET + "\r\n  Keyboard, wheel, extra-button and mode tests passed.\r\n  Coverage details are recorded in input-gui.json.\r\n")
            connection.sync()
            time.sleep(0.1)
            pixels = view.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xFFFFFFFF)
            screenshot = Image.frombytes("RGB", (geometry.width, geometry.height), pixels.data, "raw", "BGRX")
            args.screenshot.parent.mkdir(parents=True, exist_ok=True)
            screenshot.save(args.screenshot)
            report["screenshot"] = {"path": str(args.screenshot), "sha256": hashlib.sha256(args.screenshot.read_bytes()).hexdigest()}
            rpc({"op": "close", "session": "input-probe"})
            wait(lambda: not rpc({"op": "list"})["sessions"], "input session cleanup")
            launcher.wait(timeout=5)
            report["ok"] = True
        except Exception as error:
            import traceback
            report["error"] = str(error)
            report["traceback"] = traceback.format_exc()
            if "input_path" in locals() and input_path.exists():
                report["last_input_hex_suffix"] = input_path.read_bytes().hex()[-16384:]
            if "rpc" in locals():
                try:
                    report["last_terminal"] = rpc({"op": "read", "session": "input-probe", "max_bytes": 8192})
                except Exception as detail:
                    report["last_terminal_error"] = str(detail)
            raise
        finally:
            if connection is not None:
                if "reload_config" in locals() and not report.get("font_restored"):
                    try:
                        reload_config(CONFIG)
                        report["font_restored"] = True
                    except Exception as error:
                        report["font_restore_error"] = str(error)
                try:
                    for keycode, mapping in original_mappings.items():
                        connection.change_keyboard_mapping(keycode, [mapping])
                    connection.sync()
                    report["keyboard_mapping_restored"] = True
                except Exception as error:
                    report["keyboard_restore_error"] = str(error)
                connection.close()
            for pid, token in owned_pids.items():
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
            if remaining:
                report["ok"] = False
            log.close()
            report["stderr"] = (root / "stderr.log").read_text(errors="replace")[-12000:]
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(report, ensure_ascii=False, indent=2))
    if not report["ok"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
