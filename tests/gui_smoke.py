#!/usr/bin/env python3
"""Exercise the real ztty GUI in an owned Xvfb, without the user's display.

Requires Python Pillow/python-Xlib and Xvfb. No clipboard command or window
manager is needed. Every keyboard and pointer event enters through XTEST; every
asserted byte is read by a real raw-mode child over ztty's PTY.
"""
import argparse
import contextlib
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]
RESET = "\x1bc\x1b[?25l"


class Gui:
    def __init__(self, binary, font, directory, fcitx=False):
        self.terminal = self.fcitx = self.xvfb = self.control = self.d = None
        self.xlog = self.tlog = None
        from PIL import Image
        from Xlib import X, XK, Xatom, display, protocol
        from Xlib.ext import xtest

        self.Image, self.X, self.XK, self.Xatom = Image, X, XK, Xatom
        self.protocol, self.xtest = protocol, xtest
        self.directory = Path(directory)
        self.stderr = self.directory / "ztty.stderr"
        self.xlog = open(self.directory / "xvfb.stderr", "wb")
        self.tlog = open(self.stderr, "wb")
        self.marker = 0
        self.receive_buffer = bytearray()
        self.selection_result = None
        self.clipboard_bytes = b""
        self.clipboard_owned = False
        self.key_events = []
        self.down = set()
        self.mouse_down = set()
        read_fd, write_fd = os.pipe()
        try:
            self.xvfb = subprocess.Popen(
                ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1600x1000x24", "-nolisten", "tcp"],
                pass_fds=(write_fd,), stdout=subprocess.DEVNULL, stderr=self.xlog,
                start_new_session=True,
            )
        finally:
            os.close(write_fd)
        try:
            ready, _, _ = select.select([read_fd], [], [], 10)
            if not ready:
                raise RuntimeError("Xvfb did not provide a display within 10 seconds")
            number = os.read(read_fd, 64).strip().decode("ascii")
            if not number.isdecimal():
                raise RuntimeError("Xvfb failed to allocate an isolated display")
        finally:
            os.close(read_fd)
        self.display_name = ":" + number
        self.d = display.Display(self.display_name)
        self.root = self.d.screen().root
        self.helper = self.root.create_window(
            0, 0, 1, 1, 0, self.d.screen().root_depth, X.InputOutput, X.CopyFromParent,
            event_mask=X.PropertyChangeMask,
        )
        self.atoms = {name: self.d.intern_atom(name) for name in
                      ["CLIPBOARD", "UTF8_STRING", "TARGETS", "ZTTY_GUI_REPLY", "_NET_WM_NAME"]}
        executable_dir = self.directory / "bin"
        executable_dir.mkdir()
        opener = executable_dir / "xdg-open"
        opener.write_text(
            "#!/usr/bin/env python3\nimport json, os, sys\n"
            "with open(os.environ['ZTTY_GUI_OPEN_LOG'], 'a', encoding='utf-8') as f:\n"
            "    f.write(json.dumps(sys.argv[1:]) + '\\n')\n",
            encoding="utf-8",
        )
        opener.chmod(0o755)
        environment = os.environ.copy()
        environment["ZTTY_SOCKET"] = "@ztty-gui-" + str(os.getpid())
        environment.update(DISPLAY=self.display_name, LC_ALL="C.UTF-8", XMODIFIERS="@im=none")
        environment["PATH"] = str(executable_dir) + os.pathsep + environment.get("PATH", "")
        environment["ZTTY_GUI_OPEN_LOG"] = str(self.directory / "opened.jsonl")
        socket_path = self.directory / "control.sock"
        config = self.directory / "ztty.conf"
        config.write_text(
            "window-padding = 12\nbackground = #203040\nbackground-opacity = 75\n"
            "palette = 1=#c04060\ncursor-color = #e0c080\n"
            "cursor-shape = bar\ncursor-blink-interval = 150\n",
            encoding="utf-8",
        )
        if fcitx:
            self.start_fcitx(environment)
        self.terminal = subprocess.Popen(
            [str(binary), "--config", str(config), "--font", font, "--geometry", "120x32", "-e", sys.executable,
             str(ROOT / "tests/gui_child.py"), "--socket", str(socket_path),
             "--log", str(self.directory / "child.jsonl")],
            env=environment, stdout=subprocess.DEVNULL, stderr=self.tlog,
            start_new_session=True,
        )

        def find_window():
            for window in self.root.query_tree().children:
                try:
                    name = window.get_wm_class()
                    if name and any(part.lower() == "ztty" for part in name):
                        self.window = window
                        return True
                except Exception:
                    pass
            return False

        self.wait(find_window, "ztty window", 10)
        self.wait(socket_path.exists, "raw PTY child", 10)
        self.control = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.control.settimeout(5)
        self.control.connect(str(socket_path))
        self.window.configure(x=50, y=40)
        self.window.set_input_focus(X.RevertToParent, X.CurrentTime)
        self.d.sync()
        geometry = self.window.get_geometry()
        self.cw = (geometry.width - 24) // 120
        self.ch = (geometry.height - 24) // 32
        if self.cw < 1 or self.ch < 1:
            raise AssertionError("invalid terminal cell dimensions")
        self.emit(RESET)

    def start_fcitx(self, environment):
        config = self.directory / "fcitx-config"
        profile = config / "fcitx5" / "profile"
        profile.parent.mkdir(parents=True)
        profile.write_text(
            "[Groups/0]\nName=Default\nDefault Layout=us\nDefaultIM=pinyin\n"
            "[Groups/0/Items/0]\nName=keyboard-us\n"
            "[Groups/0/Items/1]\nName=pinyin\n[GroupOrder]\n0=Default\n",
            encoding="utf-8",
        )
        environment.update(XMODIFIERS="@im=fcitx", XDG_CONFIG_HOME=str(config))
        self.fcitx_environment = {
            key: value for key, value in environment.items()
            if key not in ("WAYLAND_DISPLAY", "WAYLAND_SOCKET")
        }
        self.fcitx_environment.update(
            XDG_CACHE_HOME=str(config / "cache"),
            XDG_DATA_HOME=str(config / "data"),
        )
        with (self.directory / "fcitx.stderr").open("wb") as log:
            self.fcitx = subprocess.Popen(
                ["fcitx5", "--disable=wayland,waylandim,notifications,classicui"],
                env=self.fcitx_environment, stdout=log, stderr=log,
            )
        selection = self.d.intern_atom("@server=fcitx")
        self.wait(lambda: self.d.get_selection_owner(selection), "Fcitx XIM server", 10)

    def fcitx_remote(self, *args):
        return subprocess.check_output(
            ["fcitx5-remote", *args], env=self.fcitx_environment, text=True, timeout=5
        ).strip()

    def pump(self):
        if self.d is None:
            return
        X = self.X
        while self.d.pending_events():
            event = self.d.next_event()
            if event.type == X.SelectionRequest and self.clipboard_owned:
                requestor = event.requestor
                property_atom = event.property or event.target
                returned_property = X.NONE
                if event.target == self.atoms["TARGETS"]:
                    requestor.change_property(property_atom, self.Xatom.ATOM, 32,
                                              [self.atoms["TARGETS"], self.atoms["UTF8_STRING"], self.Xatom.STRING])
                    returned_property = property_atom
                elif event.target in (self.atoms["UTF8_STRING"], self.Xatom.STRING):
                    data = self.clipboard_bytes
                    if event.target == self.Xatom.STRING:
                        data = data.decode("utf-8").encode("latin-1", "replace")
                    requestor.change_property(property_atom, event.target, 8, data)
                    returned_property = property_atom
                notify = self.protocol.event.SelectionNotify(
                    time=event.time, requestor=requestor, selection=event.selection,
                    target=event.target, property=returned_property,
                )
                requestor.send_event(notify, propagate=False)
                self.d.flush()
            elif event.type == X.SelectionClear:
                self.clipboard_owned = False
            elif event.type == X.SelectionNotify and event.requestor.id == self.helper.id:
                if event.property == X.NONE:
                    self.selection_result = False
                else:
                    prop = self.helper.get_full_property(event.property, X.AnyPropertyType)
                    self.selection_result = bytes(prop.value) if prop is not None else False
                    self.helper.delete_property(event.property)
            elif event.type in (X.KeyPress, X.KeyRelease):
                self.key_events.append((event.type, event.detail, event.time))

    def wait(self, predicate, description, timeout=5):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.pump()
            if self.terminal is not None and self.terminal.poll() is not None:
                raise RuntimeError("ztty exited while waiting for " + description)
            result = predicate()
            if result:
                return result
            time.sleep(0.004)
        raise AssertionError("timed out waiting for " + description)

    def command(self, action, **data):
        self.control.sendall(json.dumps({"action": action, **data}).encode("utf-8") + b"\n")
        while b"\n" not in self.receive_buffer:
            chunk = self.control.recv(65536)
            if not chunk:
                raise RuntimeError("PTY child control connection closed")
            self.receive_buffer.extend(chunk)
        line, _, rest = self.receive_buffer.partition(b"\n")
        self.receive_buffer = bytearray(rest)
        return json.loads(line)

    def emit(self, text):
        self.marker += 1
        title = "ztty-gui-" + str(self.marker)
        self.command("write", text=text + "\x1b]2;" + title + "\x1b\\")

        def title_arrived():
            value = self.window.get_full_property(self.atoms["_NET_WM_NAME"], self.atoms["UTF8_STRING"])
            return value is not None and bytes(value.value) == title.encode()

        self.wait(title_arrived, "terminal output barrier")

    def snapshot(self):
        return self.command("snapshot")

    def clear_input(self):
        self.command("clear")

    def input(self):
        return bytes.fromhex(self.snapshot()["input_hex"])

    def expect_input(self, expected, timeout=5):
        actual = b""

        def complete():
            nonlocal actual
            actual = self.input()
            if not expected.startswith(actual):
                raise AssertionError("unexpected PTY input: " + repr(actual[:240]) + "; expected " + repr(expected[:240]))
            return actual == expected

        self.wait(complete, "expected PTY input", timeout)
        # A release event can follow its press in a separate server/PTY batch.
        until = time.monotonic() + 0.035
        while time.monotonic() < until:
            self.pump()
            time.sleep(0.003)
        actual = self.input()
        if actual != expected:
            raise AssertionError("unexpected trailing PTY input: " + repr(actual[:240]))
        return len(actual)

    def key(self, name, pressed):
        code = self.d.keysym_to_keycode(self.XK.string_to_keysym(name))
        if not code:
            raise AssertionError("Xvfb has no keycode for " + name)
        self.xtest.fake_input(self.d, self.X.KeyPress if pressed else self.X.KeyRelease, code)
        if pressed:
            self.down.add(name)
        else:
            self.down.discard(name)
        self.d.sync()

    def tap(self, name):
        self.key(name, True)
        self.key(name, False)

    def configure_repeat(self, name, delay_ms, interval_ms):
        """Configure and read back repeat on this harness's private X server."""
        import ctypes

        code = self.d.keysym_to_keycode(self.XK.string_to_keysym(name))
        self.d.change_keyboard_control(auto_repeat_mode=self.X.AutoRepeatModeOn)
        self.d.change_keyboard_control(key=code, auto_repeat_mode=self.X.AutoRepeatModeOn)
        self.d.sync()
        # python-Xlib does not expose XKB in all packaged versions. libX11 is
        # already required by ztty; use its documented narrow XKB API here.
        x11 = ctypes.CDLL("libX11.so.6")
        x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
        x11.XOpenDisplay.restype = ctypes.c_void_p
        x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
        x11.XCloseDisplay.restype = ctypes.c_int
        x11.XkbSetAutoRepeatRate.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_uint, ctypes.c_uint]
        x11.XkbSetAutoRepeatRate.restype = ctypes.c_int
        x11.XkbGetAutoRepeatRate.argtypes = [ctypes.c_void_p, ctypes.c_uint,
                                           ctypes.POINTER(ctypes.c_uint), ctypes.POINTER(ctypes.c_uint)]
        x11.XkbGetAutoRepeatRate.restype = ctypes.c_int
        display = x11.XOpenDisplay(self.display_name.encode("ascii"))
        if not display:
            raise AssertionError("could not open the private display for XKB repeat control")
        try:
            core_keyboard = 0x0100  # XkbUseCoreKbd, from X11/extensions/XKB.h.
            if not x11.XkbSetAutoRepeatRate(display, core_keyboard, delay_ms, interval_ms):
                raise AssertionError("XKB could not configure the repeat rate")
            actual_delay, actual_interval = ctypes.c_uint(), ctypes.c_uint()
            if not x11.XkbGetAutoRepeatRate(display, core_keyboard,
                                          ctypes.byref(actual_delay), ctypes.byref(actual_interval)):
                raise AssertionError("XKB could not read back the repeat rate")
            actual = (actual_delay.value, actual_interval.value)
            assert actual == (delay_ms, interval_ms), "XKB repeat settings were not applied: " + repr(actual)
            return {"delay_ms": actual[0], "interval_ms": actual[1]}
        finally:
            x11.XCloseDisplay(display)

    def chord(self, *names):
        try:
            for name in names[:-1]:
                self.key(name, True)
            self.tap(names[-1])
        finally:
            for name in reversed(names[:-1]):
                self.key(name, False)

    def motion(self, col, row):
        geometry = self.window.get_geometry()
        self.xtest.fake_input(self.d, self.X.MotionNotify,
                              x=geometry.x + 12 + col * self.cw + self.cw // 2,
                              y=geometry.y + 12 + row * self.ch + self.ch // 2)
        self.d.sync()

    def button(self, number, pressed):
        self.xtest.fake_input(self.d, self.X.ButtonPress if pressed else self.X.ButtonRelease, number)
        if pressed:
            self.mouse_down.add(number)
        else:
            self.mouse_down.discard(number)
        self.d.sync()

    def drag(self, start, end):
        self.motion(*start)
        self.button(1, True)
        self.motion(*end)
        self.button(1, False)

    def clipboard(self):
        self.wait(lambda: getattr(self.d.get_selection_owner(self.atoms["CLIPBOARD"]), "id", 0) == self.window.id,
                  "terminal clipboard ownership")
        self.selection_result = None
        self.helper.convert_selection(self.atoms["CLIPBOARD"], self.atoms["UTF8_STRING"],
                                      self.atoms["ZTTY_GUI_REPLY"], self.X.CurrentTime)
        self.d.flush()
        self.wait(lambda: self.selection_result is not None, "clipboard conversion")
        if self.selection_result is False:
            raise AssertionError("terminal refused UTF-8 clipboard conversion")
        return self.selection_result

    def own_clipboard(self, text):
        self.clipboard_bytes = text.encode("utf-8")
        self.clipboard_owned = True
        self.helper.set_selection_owner(self.atoms["CLIPBOARD"], self.X.CurrentTime)
        self.d.sync()

    def image(self, rect=None):
        geometry = self.window.get_geometry()
        x, y, width, height = rect or (0, 0, geometry.width, geometry.height)
        data = self.window.get_image(x, y, width, height, self.X.ZPixmap, 0xffffffff).data
        # This harness owns the Xvfb and selects its 24-bit TrueColor screen.
        return self.Image.frombytes("RGB", (width, height), data, "raw", "BGRX")

    def stable_image(self, rect=None, timeout=3):
        previous = None
        stable_since = time.monotonic()
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.pump()
            image = self.image(rect)
            current = image.tobytes()
            if current != previous:
                previous = current
                stable_since = time.monotonic()
            elif time.monotonic() - stable_since >= 0.05:
                return image
            time.sleep(0.01)
        raise AssertionError("window did not reach a stable frame")

    def reset(self):
        for number in list(self.mouse_down):
            self.button(number, False)
        for name in list(self.down):
            self.key(name, False)
        self.emit(RESET)
        self.clear_input()
        # RIS resets application modes; an ordinary key also clears any local
        # selection remaining from the preceding test, through the real UI.
        self.tap("Escape")
        self.expect_input(b"\x1b")
        self.clear_input()

    def close(self):
        if self.control is not None:
            with contextlib.suppress(Exception):
                self.command("quit")
            self.control.close()
        if self.terminal is not None:
            try:
                self.terminal.wait(timeout=3)
            except subprocess.TimeoutExpired:
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(self.terminal.pid, signal.SIGTERM)
                try:
                    self.terminal.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    with contextlib.suppress(ProcessLookupError):
                        os.killpg(self.terminal.pid, signal.SIGKILL)
                    self.terminal.wait(timeout=2)
        if self.fcitx is not None and self.fcitx.poll() is None:
            self.fcitx.terminate()
            self.fcitx.wait(timeout=3)
        if self.d is not None:
            with contextlib.suppress(Exception):
                self.d.close()
        if self.xvfb is not None:
            self.xvfb.terminate()
            try:
                self.xvfb.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.xvfb.kill()
                self.xvfb.wait(timeout=2)
        if self.xlog is not None:
            self.xlog.close()
        if self.tlog is not None:
            self.tlog.close()


def legacy_keyboard(gui):
    gui.reset()
    gui.chord("Control_L", "c")
    gui.tap("Up")
    gui.tap("Return")
    gui.chord("Shift_L", "Tab")
    return {"received_bytes": gui.expect_input(b"\x03\x1b[A\r\x1b[Z")}


def kitty_disambiguation(gui):
    gui.reset()
    gui.emit("\x1b[=1u")
    gui.chord("Control_L", "Shift_L", "a")
    gui.tap("Escape")
    gui.tap("Return")
    gui.tap("F3")
    return {"received_bytes": gui.expect_input(b"\x1b[97;6u\x1b[27u\r\x1b[13~")}


def kitty_events_and_text(gui):
    gui.reset()
    gui.emit("\x1b[=31u")
    gui.tap("a")
    gui.expect_input(b"\x1b[97;1:1;97u\x1b[97;1:3u")
    gui.clear_input()
    gui.chord("Shift_L", "a")
    gui.expect_input(b"\x1b[57441;2:1u\x1b[97:65;2:1;65u\x1b[97:65;2:3u\x1b[57441;1:3u")
    gui.emit("\x1b[=3u")
    gui.clear_input()
    # Event selection belongs to this connection and does not grab or replace
    # ztty's events. Observe whether duplicate XTEST down requests actually
    # reach clients; they are not a reliable way to generate auto-repeat.
    gui.window.change_attributes(event_mask=gui.X.KeyPressMask | gui.X.KeyReleaseMask)
    gui.d.sync()
    gui.key_events.clear()
    gui.key("Up", True)
    gui.key("Up", True)
    gui.key("Up", False)
    gui.emit("\x1b[=3u")  # Output barrier also lets ztty finish the key events.
    gui.pump()
    duplicate_probe = {
        "requested_presses": 2,
        "observed_presses": sum(kind == gui.X.KeyPress for kind, _, _ in gui.key_events),
        "observed_releases": sum(kind == gui.X.KeyRelease for kind, _, _ in gui.key_events),
        "pty_hex": gui.input().hex(),
    }
    settings = gui.configure_repeat("Up", 120, 40)
    gui.clear_input()
    gui.key_events.clear()
    press, repeat, release = b"\x1b[1;1:1A", b"\x1b[1;1:2A", b"\x1b[1;1:3A"

    def repeat_arrived():
        actual = gui.input()
        if len(actual) < len(press):
            assert press.startswith(actual), "unexpected initial key event: " + repr(actual)
            return False
        assert actual.startswith(press), "hold did not start with type=1: " + repr(actual)
        following = actual[len(press):]
        possible = repeat * (len(following) // len(repeat) + 1)
        assert possible.startswith(following), "held key produced something other than type=2: " + repr(actual)
        return len(following) >= len(repeat)

    gui.key("Up", True)
    try:
        gui.wait(repeat_arrived, "XKB-generated auto-repeat while Up remains held", 3)
    except Exception as error:
        raise AssertionError(f"{error}; X11 observer={gui.key_events!r}; duplicate probe={duplicate_probe!r}") from error
    finally:
        gui.key("Up", False)
    gui.wait(lambda: gui.input().endswith(release), "the physical Up release after auto-repeat")
    # Scheduler latency may allow another repeat before the release reaches
    # the server. Accept its count, but require every intermediate event to
    # be type=2, with exactly one type=1 and one final type=3.
    actual = gui.input()
    middle = actual[len(press):-len(release)]
    repeats = len(middle) // len(repeat)
    assert repeats >= 1 and middle == repeat * repeats, "invalid repeat/release sequence: " + repr(actual)
    gui.expect_input(press + repeat * repeats + release)
    return {"received_bytes": len(actual), "repeat_events": repeats,
            "repeat_settings": settings, "duplicate_xtest_probe": duplicate_probe}


def resize_signal(gui):
    gui.reset()
    before = gui.snapshot()["winches"]
    gui.window.configure(width=80 * gui.cw + 24, height=24 * gui.ch + 24)
    gui.d.sync()

    def resized():
        state = gui.snapshot()
        return state if state["cols"] == 80 and state["rows"] == 24 and state["winches"] > before else None

    state = gui.wait(resized, "TIOCSWINSZ and child SIGWINCH")
    return {"cols": state["cols"], "rows": state["rows"], "sigwinch": state["winches"] - before}


def selection_clipboard(gui):
    gui.reset()
    gui.emit("\x1b[HCOPY-ME\r\n汉字Z")
    gui.drag((0, 0), (6, 0))
    gui.chord("Control_L", "Shift_L", "c")
    first = gui.clipboard()
    assert first == b"COPY-ME", repr(first)
    gui.drag((0, 1), (4, 1))
    gui.chord("Control_L", "Shift_L", "c")
    second = gui.clipboard()
    assert second == "汉字Z".encode(), repr(second)
    return {"ascii": first.decode(), "wide_text": second.decode()}


def clipboard_bracketed_paste(gui):
    gui.reset()
    gui.emit("\x1b[?2004h")
    text = "paste 中文\nsecond\tline\x1b[201~tail"
    gui.own_clipboard(text)
    gui.chord("Control_L", "Shift_L", "v")
    expected = b"\x1b[200~" + text.replace("\x1b", "").encode() + b"\x1b[201~"
    return {"received_bytes": gui.expect_input(expected)}


def large_paste_backpressure(gui):
    gui.reset()
    gui.emit("\x1b[?2004h")
    text = "0123456789abcdef\n" * 8192
    gui.own_clipboard(text)
    gui.chord("Control_L", "Shift_L", "v")
    expected = b"\x1b[200~" + text.encode() + b"\x1b[201~"
    return {"received_bytes": gui.expect_input(expected, 10), "payload_bytes": len(text)}


def sgr_mouse_drag(gui):
    gui.reset()
    gui.emit("\x1b[?1002h\x1b[?1006h")
    gui.drag((2, 3), (4, 3))
    return {"received_bytes": gui.expect_input(b"\x1b[<0;3;4M\x1b[<32;5;4M\x1b[<0;5;4m")}


def shift_selection_release(gui):
    gui.reset()
    gui.emit("\x1b[2;1HSELECT-STAYS\x1b[?1002h\x1b[?1006h")
    gui.key("Shift_L", True)
    gui.motion(0, 1)
    gui.button(1, True)
    gui.motion(3, 1)
    gui.key("Shift_L", False)
    gui.button(1, False)
    gui.motion(8, 1)
    gui.expect_input(b"")
    gui.chord("Control_L", "Shift_L", "c")
    copied = gui.clipboard()
    assert copied == b"SELE", "selection continued after mouse release: " + repr(copied)
    return {"selection": copied.decode(), "application_mouse_bytes": 0}


def osc8_open(gui):
    gui.reset()
    uri = "https://example.invalid/ztty-gui-test?literal=1&value=two"
    gui.emit("\x1b[Hprefix \x1b]8;;" + uri + "\x1b\\OPEN-LINK\x1b]8;;\x1b\\\x1b[?1002h\x1b[?1006h")
    gui.key("Control_L", True)
    gui.motion(9, 0)
    gui.button(1, True)
    gui.button(1, False)
    gui.key("Control_L", False)
    log_path = gui.directory / "opened.jsonl"
    gui.wait(lambda: log_path.exists() and log_path.stat().st_size > 0, "OSC8 opener invocation")
    opened = [json.loads(line) for line in log_path.read_text().splitlines()]
    assert opened == [[uri]], opened
    gui.expect_input(b"")
    return {"opened_uri": uri, "opener": "isolated recording stub; no browser or network"}


def synchronized_output(gui):
    gui.reset()
    gui.emit("\x1b[4;5HCOMMITTED FRAME")
    rect = (12 + 4 * gui.cw, 12 + 3 * gui.ch, 26 * gui.cw, gui.ch)
    before = gui.stable_image(rect).tobytes()
    start = time.monotonic()
    gui.emit("\x1b[?2026h")
    gui.emit("\x1b[4;5HUNCOMMITTED DATA")
    assert gui.image(rect).tobytes() == before, "a partial synchronized frame became visible"
    gui.window.clear_area(x=rect[0], y=rect[1], width=rect[2], height=rect[3], exposures=True)
    gui.d.flush()
    gui.wait(lambda: gui.image(rect).tobytes() == before, "Expose restoring the committed frame", 0.10)
    elapsed = time.monotonic() - start
    assert elapsed < 0.14, f"sync observation exceeded the 150 ms safety timeout ({elapsed:.3f}s)"
    gui.emit("\x1b[?2026l")
    gui.wait(lambda: gui.image(rect).tobytes() != before, "committing the synchronized frame")
    return {"separate_pty_writes": 3, "expose_retained_previous_frame": True}


def appearance_and_cursor(gui):
    gui.reset()
    assert gui.window.get_geometry().depth == 32
    hint = gui.d.intern_atom("_KDE_NET_WM_BLUR_BEHIND_REGION")
    prop = gui.window.get_full_property(hint, gui.Xatom.CARDINAL)
    assert prop is not None and prop.format == 32 and len(prop.value) == 0

    def pixel(x, y):
        data = gui.window.get_image(x, y, 1, 1, gui.X.ZPixmap, 0xffffffff).data
        return tuple(data[2::-1]) + (data[3],)

    x, y = 12, 12
    background = (24, 36, 48, 191)
    cursor = (224, 192, 128, 255)
    gui.emit("\x1b[?25h")
    gui.wait(lambda: pixel(x, y) == cursor, "configured bar cursor appears")
    assert pixel(x + gui.cw - 1, y) == background
    gui.wait(lambda: pixel(x, y) == background, "cursor blinks off")
    gui.wait(lambda: pixel(x, y) == cursor, "cursor blinks on again")

    gui.emit("\x1b[2 q")
    gui.wait(lambda: pixel(x + gui.cw - 1, y) == cursor, "DECSCUSR steady block")
    deadline = time.monotonic() + 0.35
    while time.monotonic() < deadline:
        assert pixel(x, y) == cursor, "steady cursor blinked"
        time.sleep(0.02)
    gui.emit("\x1b[?25l\x1b[41m \x1b[49m \x1b[48;2;32;48;64m \x1b[44m \x1b[0m")
    gui.wait(lambda: pixel(x, y) == (192, 64, 96, 255), "opaque customized ANSI background")
    assert pixel(x + gui.cw, y) == background
    assert pixel(x + 2 * gui.cw, y) == (32, 48, 64, 255)
    assert pixel(x + 3 * gui.cw, y) == (145, 181, 241, 255)
    return {"argb_depth": 32, "background_alpha": 191, "cursor_phases_verified": True}


def fcitx_input(gui):
    gui.fcitx_remote("-c")
    gui.tap("a")
    gui.expect_input(b"a")
    gui.clear_input()
    gui.key("Control_L", True)
    gui.tap("space")
    gui.key("Control_L", False)
    gui.wait(lambda: gui.fcitx_remote() == "2" and gui.fcitx_remote("-n") == "pinyin",
             "Pinyin activation")
    for char in "nihao":
        gui.tap(char)
    gui.tap("space")
    committed = "你好".encode()
    gui.expect_input(committed)
    gui.clear_input()
    gui.fcitx_remote("-c")
    gui.tap("Return")
    gui.expect_input(b"\r")
    return {"committed_text": committed.decode()}


def preview(gui, output):
    gui.reset()
    gui.window.configure(width=112 * gui.cw + 24, height=28 * gui.ch + 24)
    gui.d.sync()
    gui.wait(lambda: gui.snapshot()["cols"] == 112, "preview resize")
    text = RESET

    def at(row, col, value):
        return f"\x1b[{row};{col}H" + value

    text += at(2, 4, "\x1b[1;38;2;233;238;245mztty\x1b[0m")
    text += at(2, 14, "\x1b[38;2;127;151;176mRUST  /  NO_STD  /  LINUX\x1b[0m")
    text += at(5, 4, "\x1b[1mA terminal, built from small pieces.\x1b[0m")
    text += at(7, 4, "\x1b[38;2;150;166;184mSTREAM          SCREEN           INPUT\x1b[0m")
    text += at(8, 4, "zvte            zterm            zinput")
    text += at(10, 4, "UTF-8   你好，终端。   日本語   한글   combining: e\u0301")
    text += at(13, 4, "\x1b[38;2;150;166;184mANSI PALETTE\x1b[0m")
    text += at(14, 4, "".join(f"\x1b[48;5;{index}m    " for index in range(16)) + "\x1b[0m")
    text += at(17, 4, "\x1b[38;2;150;166;184m24-BIT COLOR\x1b[0m")
    ramp = "".join(f"\x1b[48;2;{40 + index};{90 + index * 2};{180 - index}m " for index in range(64))
    text += at(18, 4, ramp + "\x1b[0m")
    text += at(21, 4, "\x1b]8;;https://github.com/LIghtJUNction/ztty\x1b\\\x1b[38;2;138;180;248mgithub.com/LIghtJUNction/ztty\x1b[0m\x1b]8;;\x1b\\")
    text += at(24, 4, "\x1b[38;2;150;166;184mPTY  ·  keyboard  ·  clipboard  ·  resize  ·  synchronized output\x1b[0m")
    text += at(26, 4, "\x1b[38;2;110;133;157mLive protocol preview\x1b[0m")
    gui.emit(text)
    time.sleep(0.15)
    image = gui.stable_image()
    assert len(image.getcolors(image.width * image.height) or []) > 64, "preview has insufficient rendered color detail"
    image.save(output)
    return {"path": str(output), "width": image.width, "height": image.height}


def main(fcitx=False):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/ztty")
    parser.add_argument("--fcitx", action="store_true",
                        help="test real XIM via Fcitx 5 and Pinyin using an isolated D-Bus session")
    parser.add_argument("--font", default="monospace:size=11")
    parser.add_argument("--artifacts", type=Path, default=ROOT / "artifacts")
    args = parser.parse_args()
    if args.fcitx:
        # Launch a dedicated bus regardless of the caller's desktop environment.
        os.execvp("dbus-run-session", ["dbus-run-session", "--", sys.executable, "-c",
            "import sys; sys.path.insert(0,sys.argv[1]); import gui_smoke; "
            "sys.argv = [gui_smoke.__file__, *sys.argv[2:]]; sys.exit(gui_smoke.main(fcitx=True))",
            str(ROOT / "tests"), *[arg for arg in sys.argv[1:] if arg != "--fcitx"]])
    args.artifacts.mkdir(parents=True, exist_ok=True)
    result = {"schema": 1, "ok": False, "binary": str(args.binary.resolve()), "display": "owned Xvfb", "checks": []}
    tests = [legacy_keyboard, kitty_disambiguation, kitty_events_and_text, resize_signal,
             selection_clipboard, clipboard_bracketed_paste, large_paste_backpressure,
             sgr_mouse_drag, shift_selection_release, osc8_open, synchronized_output, appearance_and_cursor]
    if fcitx:
        tests = [fcitx_input]
    gui = None
    with tempfile.TemporaryDirectory(prefix="ztty-gui-") as directory:
        try:
            if not args.binary.is_file():
                raise FileNotFoundError(args.binary)
            # Assign before construction so even a failed setup is cleaned up.
            gui = Gui.__new__(Gui)
            gui.__init__(args.binary.resolve(), args.font, directory, fcitx)
            for test in tests:
                try:
                    detail = test(gui)
                    result["checks"].append({"name": test.__name__, "ok": True, **detail})
                except Exception as error:
                    result["checks"].append({"name": test.__name__, "ok": False, "error": f"{type(error).__name__}: {error}"})
                    if gui.terminal.poll() is not None:
                        break
            if not fcitx:
                try:
                    result["preview"] = preview(gui, args.artifacts / "ztty-preview.png")
                except Exception as error:
                    result["checks"].append({"name": "preview", "ok": False, "error": f"{type(error).__name__}: {error}"})
            result["ok"] = len(result["checks"]) == len(tests) and all(check["ok"] for check in result["checks"])
        except Exception as error:
            result["error"] = f"{type(error).__name__}: {error}"
        finally:
            if gui is not None:
                with contextlib.suppress(Exception):
                    gui.close()
            fcitx_log = Path(directory) / "fcitx.stderr"
            if fcitx and not result["ok"] and fcitx_log.exists():
                result["fcitx_stderr_tail"] = fcitx_log.read_text(errors="replace")[-3000:]
            stderr = Path(directory) / "ztty.stderr"
            if stderr.exists() and stderr.stat().st_size:
                result["stderr_tail"] = stderr.read_text(errors="replace")[-2000:]
            if not result["ok"]:
                xlog = Path(directory) / "xvfb.stderr"
                if xlog.exists() and xlog.stat().st_size:
                    result["xvfb_stderr_tail"] = xlog.read_text(errors="replace")[-2000:]
    result["passed"] = sum(check["ok"] for check in result["checks"])
    result["failed"] = sum(not check["ok"] for check in result["checks"])
    (args.artifacts / "gui-smoke.json").write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
