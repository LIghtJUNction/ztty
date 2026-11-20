#!/usr/bin/env python3
"""Real Windows multi-PTY acceptance, run inside the owned CI desktop.

Each pane runs an actual Python probe behind ConPTY. Win32 keyboard/mouse
input changes the shared layout; visible client pixels, independent child
responses, process identities and shutdown are the acceptance evidence.
"""
import argparse
import base64
import ctypes
import errno
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
import threading
import time
import traceback

import windows_native as native


PADDING = 12
INITIAL_GRID = (64, 20)
BLACK = [0, 0, 0]
COLORS = {"a": [255, 0, 0], "b": [0, 255, 0], "c": [0, 0, 255], "d": [255, 255, 0]}
REQUIRED_CHECKS = (
    "native_architecture_and_initial_pty", "split_right_distinct_pty", "split_viewport_pixels",
    "pane_keyboard_focus_isolation", "inactive_pane_mouse_tracking", "pane_history_isolation",
    "pane_selection_isolation", "split_down_distinct_pty", "three_pane_pixels_and_shared_window",
    "maximize_restore_keeps_pty_identity", "grow_shrink_updates_child_geometry", "swap_keeps_session_identity",
    "window_resize_coordinates_all_ptys", "new_tab_distinct_pty", "hidden_tab_output_and_response",
    "tab_switch_restores_pane_images", "font_increase_coordinates_all_ptys",
    "font_decrease_and_keypad_zoom", "font_reset_restores_configured_metrics",
    "natural_exit_keeps_siblings", "close_keeps_other_tab",
    "last_pane_closes_window",
)


def write_bytes(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_bytes(value)
    deadline = time.monotonic() + 1
    while True:
        try:
            temporary.replace(path)
            return
        except PermissionError:
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.005)


def write_json(path, value):
    write_bytes(path, (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8"))


def read_probe_bytes(path, report):
    """Bound sharing-conflict recovery to this probe's atomically replaced snapshots."""
    assert path.name in ("state.json", "keyboard.bin")
    try:
        return path.read_bytes()
    except PermissionError as error:
        if (sys.platform != "win32" or error.errno != errno.EACCES
                or getattr(error, "winerror", None) not in (None, 5, 32, 33)):
            raise
        started = time.monotonic()
        deadline = started + 1.0
        first = {"type": type(error).__name__, "errno": error.errno,
                 "winerror": getattr(error, "winerror", None)}
        last_error, attempts, outcome = error, 1, "failed"
        try:
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise last_error
                time.sleep(min(0.005, remaining))
                if time.monotonic() >= deadline:
                    raise last_error
                attempts += 1
                try:
                    value = path.read_bytes()
                except PermissionError as repeated:
                    if (repeated.errno != errno.EACCES
                            or getattr(repeated, "winerror", None) not in (None, 5, 32, 33)):
                        raise
                    last_error = repeated
                else:
                    outcome = "read"
                    return value
        finally:
            events = report.setdefault("probe_file_read_retries", [])
            if len(events) < 64:
                events.append({"path": str(path), "attempts": attempts, "retries": attempts - 1,
                    "elapsed_seconds": round(time.monotonic() - started, 6),
                    "retry_timeout_seconds": 1.0, "retry_interval_seconds": 0.005,
                    "first_error": first, "outcome": outcome})
            else:
                report["probe_file_read_retries_omitted"] = report.get("probe_file_read_retries_omitted", 0) + 1


def split_rect(rect, vertical, ratio=500):
    x, y, width, height = rect
    extent = width if vertical else height
    gap = 2 if extent >= 4 else 0
    available = max(2, extent - gap)
    first = min(available - 1, max(1, available * ratio // 1000))
    if vertical:
        return (x, y, first, height), (x + first + gap, y, available - first, height)
    return (x, y, width, first), (x, y + first + gap, width, available - first)


def cell_reply(data):
    """Read a CSI 16t reply without inferring font metrics from rounded grids."""
    replies = re.findall(rb"\x1b\[6;([0-9]+);([0-9]+)t", data)
    if len(replies) != 1:
        raise AssertionError("expected exactly one cell-size reply")
    height, width = map(int, replies[0])
    if not (0 < width <= 4096 and 0 < height <= 4096):
        raise AssertionError("invalid reported cell dimensions")
    return width, height


def without_probe_replies(data):
    """Keep possible leaked keys while removing only this probe's query replies."""
    data = re.sub(rb"\x1b\[6;[0-9]+;[0-9]+t", b"", data)
    return re.sub(rb"\x1b_Gi=[0-9]+;OK\x1b\\", b"", data)


def validate_report(report):
    if not isinstance(report, dict):
        return False
    checks = report.get("checks", [])
    screenshots = report.get("screenshots", {})
    if (not isinstance(checks, list) or any(not isinstance(check, dict) for check in checks)
            or not isinstance(screenshots, dict) or not isinstance(report.get("cleanup", {}), dict)):
        return False
    pixel_checks = [check["pixels"] for check in checks if "pixels" in check]
    def accepted_pixel(sample):
        if not isinstance(sample, dict) or sample.get("owned") is not True or sample.get("visible") is not True:
            return False
        rgb, expected = sample.get("rgb"), sample.get("expected")
        return (isinstance(rgb, list) and isinstance(expected, list) and len(rgb) == len(expected) == 3
                and all(isinstance(value, int) and 0 <= value <= 255 for value in rgb + expected)
                and all(abs(actual - wanted) <= 1 for actual, wanted in zip(rgb, expected)))
    holds = next((check.get("synchronized_layout", {}) for check in checks
                  if check.get("name") == "split_viewport_pixels"), {})
    def accepted_hold(name):
        if not isinstance(holds, dict) or not isinstance(holds.get(name), dict):
            return False
        hold = holds[name]
        elapsed, pixels = hold.get("elapsed_seconds"), hold.get("pixels")
        return (isinstance(elapsed, (int, float)) and 0 <= elapsed < 0.150
                and isinstance(pixels, list) and bool(pixels) and all(map(accepted_pixel, pixels)))
    return (report.get("passed") is True and report.get("status") == "passed"
            and [check.get("name") for check in checks] == list(REQUIRED_CHECKS)
            and all(check.get("passed") is True for check in checks)
            and bool(pixel_checks) and report.get("visible_pixel_checks_completed") == len(pixel_checks)
            and all(isinstance(samples, list) and bool(samples) and all(map(accepted_pixel, samples))
                    for samples in pixel_checks)
            and accepted_hold("focus") and accepted_hold("resize")
            and report.get("cleanup", {}).get("process_exited") is True
            and report.get("cleanup", {}).get("temporary_directory_removed") is True
            and report.get("cleanup", {}).get("all_probe_processes_exited") is True
            and report.get("cleanup", {}).get("returncode") == 0
            and all(screenshots.get(name, {}).get("sha256")
                    for name in ("split-right", "hold-focus", "hold-resize", "three-panes",
                                 "resized", "tab-restored", "surviving-tab")))


def merge_reports(original, workspace):
    """Retain every original assertion/error; workspace failure cannot erase it."""
    original["workspace"] = workspace
    passed = original.get("passed") is True and validate_report(workspace)
    original["passed"] = passed
    original["status"] = "passed" if passed else (
        "blocked" if "blocked" in (original.get("status"), workspace.get("status")) else "failed")
    if not passed:
        original.setdefault("error", workspace.get("error", "Windows workspace evidence did not pass"))
    return original


def child(directory):
    """An actual console child, with bounded input and atomic diagnostic files."""
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.GetStdHandle.argtypes, kernel.GetStdHandle.restype = [ctypes.c_uint32], ctypes.c_void_p
    kernel.SetConsoleMode.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
    kernel.ReadFile.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32,
                               ctypes.POINTER(ctypes.c_uint32), ctypes.c_void_p]
    stdin, stdout = (kernel.GetStdHandle(value & 0xFFFFFFFF) for value in (-10, -11))
    if not kernel.SetConsoleMode(stdin, 0x200) or not kernel.SetConsoleMode(stdout, 0x1 | 0x4 | 0x8):
        raise ctypes.WinError(ctypes.get_last_error())
    directory = Path(directory)
    received = bytearray()

    def read_input():
        buffer = ctypes.create_string_buffer(4096)
        while True:
            count = ctypes.c_uint32()
            if not kernel.ReadFile(stdin, buffer, len(buffer), ctypes.byref(count), None) or not count.value:
                return
            received.extend(buffer.raw[:count.value])
            del received[:-65536]
            write_bytes(directory / "keyboard.bin", received)

    threading.Thread(target=read_input, daemon=True).start()
    sys.stdout.buffer.write(b"\x1b]2;ztty workspace " + directory.name.encode("ascii")
                            + b"\x07\x1b[?25l\x1b[2J\x1b[H")
    sys.stdout.buffer.flush()
    sequence = 0
    while True:
        size = os.get_terminal_size()
        write_json(directory / "state.json", {"pid": os.getpid(), "parent_pid": os.getppid(),
                   "cwd": os.getcwd(), "cols": size.columns, "rows": size.lines,
                   "monotonic": time.monotonic()})
        path = directory / f"command-{sequence}.json"
        if path.exists():
            command = json.loads(path.read_text(encoding="utf-8"))
            if command.get("exit"):
                return
            sys.stdout.buffer.write(base64.b64decode(command["output"], validate=True))
            sys.stdout.buffer.flush()
            (directory / f"done-{sequence}").write_text("sent", encoding="utf-8")
            sequence += 1
        time.sleep(0.02)


class KeyboardInput(ctypes.Structure):
    _fields_ = [("vk", ctypes.c_uint16), ("scan", ctypes.c_uint16),
                ("flags", ctypes.c_uint32), ("time", ctypes.c_uint32), ("extra", ctypes.c_size_t)]


class MouseInput(ctypes.Structure):
    _fields_ = [("x", ctypes.c_int32), ("y", ctypes.c_int32), ("data", ctypes.c_uint32),
                ("flags", ctypes.c_uint32), ("time", ctypes.c_uint32), ("extra", ctypes.c_size_t)]


class InputData(ctypes.Union):
    _fields_ = [("keyboard", KeyboardInput), ("mouse", MouseInput)]


class Input(ctypes.Structure):
    _fields_ = [("kind", ctypes.c_uint32), ("data", InputData)]


class Window:
    def __init__(self, process, report):
        self.process, self.report, self.hwnd = process, report, None
        self.user = ctypes.WinDLL("user32", use_last_error=True)
        self.gdi = ctypes.WinDLL("gdi32", use_last_error=True)
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        handle, uint, integer = ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int32
        self.enum_proc = ctypes.WINFUNCTYPE(integer, handle, ctypes.c_ssize_t)
        for library, name, args, result in (
            (self.user, "EnumWindows", [self.enum_proc, ctypes.c_ssize_t], integer),
            (self.user, "GetWindowThreadProcessId", [handle, ctypes.c_void_p], uint),
            # The existing PNG helper uses wintypes.RECT/DWORD; accept either
            # pointer representation, with the same documented native layout.
            (self.user, "GetClientRect", [handle, ctypes.c_void_p], integer),
            (self.user, "GetWindowRect", [handle, ctypes.c_void_p], integer),
            (self.user, "IsWindow", [handle], integer),
            (self.user, "IsWindowVisible", [handle], integer),
            (self.user, "IsIconic", [handle], integer),
            (self.user, "GetWindowTextW", [handle, ctypes.c_wchar_p, integer], integer),
            (self.user, "GetClassNameW", [handle, ctypes.c_wchar_p, integer], integer),
            (self.user, "SetWindowPos", [handle, handle, integer, integer, integer, integer, uint], integer),
            (self.user, "ClientToScreen", [handle, ctypes.POINTER(native.NativePoint)], integer),
            (self.user, "MonitorFromWindow", [handle, uint], handle),
            (self.user, "GetMonitorInfoW", [handle, ctypes.POINTER(native.MonitorInfo)], integer),
            (self.user, "GetDpiForWindow", [handle], uint),
            (self.user, "SetForegroundWindow", [handle], integer),
            (self.user, "GetForegroundWindow", [], handle),
            (self.user, "GetAncestor", [handle, uint], handle),
            (self.user, "WindowFromPoint", [native.NativePoint], handle),
            (self.user, "GetDC", [handle], handle),
            (self.user, "ReleaseDC", [handle, handle], integer),
            (self.user, "SendMessageW", [handle, uint, ctypes.c_size_t, ctypes.c_ssize_t], ctypes.c_ssize_t),
            (self.user, "PostMessageW", [handle, uint, ctypes.c_size_t, ctypes.c_ssize_t], integer),
            (self.user, "SendInput", [uint, ctypes.POINTER(Input), integer], uint),
            (self.user, "GetAsyncKeyState", [integer], ctypes.c_int16),
            (self.user, "GetGuiResources", [handle, uint], uint),
            (self.gdi, "PtVisible", [handle, integer, integer], integer),
            (self.gdi, "GetPixel", [handle, integer, integer], uint),
            (self.kernel, "OpenProcess", [uint, integer, uint], handle),
            (self.kernel, "WaitForSingleObject", [handle, uint], uint),
            (self.kernel, "CloseHandle", [handle], integer),
        ):
            function = getattr(library, name)
            function.argtypes, function.restype = args, result
        self.last_samples = []
        self.user.SetProcessDPIAware()
        self.hwnd = native.wait_for(lambda: next(iter(self.windows()), None), "owned workspace window")
        if len(self.windows()) != 1:
            raise AssertionError("one workspace must have exactly one native ztty window")

    def pid(self, hwnd):
        value = ctypes.c_uint32()
        if hwnd:
            self.user.GetWindowThreadProcessId(hwnd, ctypes.byref(value))
        return value.value

    def windows(self):
        result = []
        @self.enum_proc
        def visit(hwnd, _):
            name = ctypes.create_unicode_buffer(256)
            self.user.GetClassNameW(hwnd, name, len(name))
            if self.pid(hwnd) == self.process.pid and name.value == "ztty.NativeWindow":
                result.append(hwnd)
            return True
        self.user.EnumWindows(visit, 0)
        return result

    def title(self):
        text = ctypes.create_unicode_buffer(512)
        self.user.GetWindowTextW(self.hwnd, text, len(text))
        return text.value

    def geometry(self):
        client, outer = native.NativeRect(), native.NativeRect()
        if (not self.user.GetClientRect(self.hwnd, ctypes.byref(client))
                or not self.user.GetWindowRect(self.hwnd, ctypes.byref(outer))):
            raise OSError("cannot read workspace window geometry")
        monitor = native.MonitorInfo()
        monitor.size = ctypes.sizeof(monitor)
        if not self.user.GetMonitorInfoW(self.user.MonitorFromWindow(self.hwnd, 2), ctypes.byref(monitor)):
            raise OSError("cannot read workspace monitor")
        return {"client": native.rect_values(client), "outer": native.rect_values(outer),
                "monitor": native.rect_values(monitor.monitor), "work": native.rect_values(monitor.work),
                "dpi": self.user.GetDpiForWindow(self.hwnd), "hwnd": int(self.hwnd),
                "pid": self.pid(self.hwnd), "foreground": int(self.user.GetForegroundWindow() or 0),
                "visible": bool(self.user.IsWindowVisible(self.hwnd)),
                "minimized": bool(self.user.IsIconic(self.hwnd))}

    def place(self, client_size=None):
        state = self.geometry()
        assert state["pid"] == self.process.pid
        left, top, right, bottom = state["work"]
        outer, client = state["outer"], state["client"]
        width, height = outer[2] - outer[0], outer[3] - outer[1]
        if client_size:
            width += client_size[0] - client[2]
            height += client_size[1] - client[3]
        if width > right - left or height > bottom - top:
            raise native.DesktopBlocked("workspace window does not fit its monitor work area")
        x = left + min(8, (right - left - width) // 2)
        y = top + min(8, (bottom - top - height) // 2)
        if not self.user.SetWindowPos(self.hwnd, ctypes.c_void_p(-1), x, y,
                                      width, height, 0):
            raise OSError("cannot position owned workspace window")
        if client_size:
            native.wait_for(lambda: self.geometry()["client"][2:] == list(client_size), "exact workspace client size")
        self.user.SetForegroundWindow(self.hwnd)
        try:
            native.wait_for(lambda: self.user.GetForegroundWindow() == self.hwnd, "owned foreground window")
        except AssertionError as error:
            raise native.DesktopBlocked(str(error)) from error
        self.report.setdefault("window_stages", []).append(self.geometry())

    def screen_point(self, x, y):
        point = native.NativePoint(int(x), int(y))
        if not self.user.ClientToScreen(self.hwnd, ctypes.byref(point)):
            raise OSError("ClientToScreen failed")
        return point

    def pixels(self, samples):
        dc = self.user.GetDC(self.hwnd)
        if not dc:
            raise OSError("cannot acquire the owned window DC")
        self.last_samples = []
        try:
            for x, y, expected in samples:
                point = self.screen_point(x, y)
                hit = self.user.WindowFromPoint(point)
                owned = self.user.GetAncestor(hit, 2) == self.hwnd and self.pid(hit) == self.process.pid
                visible = bool(self.gdi.PtVisible(dc, int(x), int(y)))
                color = self.gdi.GetPixel(dc, int(x), int(y))
                rgb = None if color == 0xFFFFFFFF else [color & 255, (color >> 8) & 255, (color >> 16) & 255]
                self.last_samples.append({"client": [x, y], "screen": [point.x, point.y], "rgb": rgb,
                                          "expected": expected, "window_from_point": int(hit or 0),
                                          "pid_at_point": self.pid(hit), "owned": owned, "visible": visible})
                if not owned or not visible:
                    raise OSError("workspace sample is covered or outside the visible client DC")
                if rgb is None:
                    raise OSError("GetPixel failed on the owned workspace DC")
            return all(all(abs(actual - expected) <= 1 for actual, expected in zip(point["rgb"], point["expected"]))
                       for point in self.last_samples)
        finally:
            self.user.ReleaseDC(self.hwnd, dc)

    def expect(self, name, samples):
        self.report["active_check"] = name
        try:
            native.wait_for(lambda: self.pixels(samples), name)
        except AssertionError as error:
            self.report["failed_pixels"] = {"name": name, "samples": self.last_samples, "window": self.geometry()}
            if any(not point["owned"] or not point["visible"] for point in self.last_samples):
                raise native.DesktopBlocked(str(error)) from error
            raise
        return list(self.last_samples)

    def capture(self, output, name):
        result = native.capture_window(self.user, self.gdi, self.hwnd, self.process.pid,
                                       output.with_name(output.stem + "." + name + ".png"))
        self.report.setdefault("screenshots", {})[name] = result
        return result

    def inputs(self, events):
        if self.user.GetForegroundWindow() != self.hwnd:
            raise native.DesktopBlocked("refusing to send keyboard input to an unowned foreground window")
        try:
            native.wait_for(lambda: all(not (self.user.GetAsyncKeyState(key) & 0x8000)
                                        for key in (0x10, 0x11, 0x12, 0x5B, 0x5C)),
                            "modifier release before owned input")
        except AssertionError as error:
            raise native.DesktopBlocked(str(error)) from error
        if self.user.GetForegroundWindow() != self.hwnd:
            raise native.DesktopBlocked("owned window lost foreground before SendInput")
        array = (Input * len(events))(*[Input(1, InputData(keyboard=KeyboardInput(*event, 0, 0)))
                                      for event in events])
        count = self.user.SendInput(len(array), array, ctypes.sizeof(Input))
        if count != len(array):
            raise native.DesktopBlocked(f"SendInput accepted {count}/{len(array)} owned-window events")

    def chord(self, vk, control=True, shift=True, alt=False):
        modifiers = ([0x11] if control else []) + ([0x12] if alt else []) + ([0x10] if shift else [])
        self.inputs([(key, 0, 0) for key in modifiers + [vk]]
                    + [(key, 0, 2) for key in [vk] + list(reversed(modifiers))])

    def text(self, value):
        units = struct.unpack("<" + "H" * (len(value.encode("utf-16-le")) // 2), value.encode("utf-16-le"))
        self.inputs([(0, unit, flags) for unit in units for flags in (4, 6)])

    def mouse(self, message, x, y, wparam=0, screen=False):
        if screen:
            point = self.screen_point(x, y)
            x, y = point.x, point.y
        self.user.SendMessageW(self.hwnd, message, wparam, (int(x) & 65535) | ((int(y) & 65535) << 16))


class Probe:
    def __init__(self, directory, name, window):
        self.name, self.path, self.window, self.sequence = name, directory / name, window, 0
        self.path.mkdir(exist_ok=True)
        self.handle = None
        self.queries = {}

    def ready(self):
        native.wait_for(lambda: (self.path / "state.json").exists(), "actual ConPTY child " + self.name)
        state = self.state()
        self.pid = state["pid"]
        self.handle = self.window.kernel.OpenProcess(0x100000 | 0x1000, 0, self.pid)
        if not self.handle:
            raise OSError("cannot retain the owned probe process handle")
        return state

    def state(self):
        return json.loads(read_probe_bytes(self.path / "state.json", self.window.report).decode("utf-8"))

    def input(self):
        path = self.path / "keyboard.bin"
        try:
            return read_probe_bytes(path, self.window.report)
        except FileNotFoundError:
            return b""

    def queue(self, payload):
        sequence = self.sequence
        query_id = 0x70000000 + ord(self.name) * 65536 + sequence
        marker = f"i={query_id};OK".encode("ascii")
        # WriteFile completion is not a frontend parse barrier. This query is
        # answered by ztty's graphics parser, including while output is held.
        payload += native.kitty(f"a=q,t=d,f=24,s=1,v=1,i={query_id}", bytes([1, 2, 3]))
        self.queries[sequence] = marker
        write_json(self.path / f"command-{sequence}.json", {"output": base64.b64encode(payload).decode("ascii")})
        self.sequence += 1
        return sequence

    def done(self, sequence):
        native.wait_for(lambda: (self.path / f"done-{sequence}").exists(), "PTY output from " + self.name)
        marker = self.queries.pop(sequence)
        native.wait_for(lambda: marker in self.input(), "frontend parse barrier for pane " + self.name)

    def send(self, payload):
        self.done(self.queue(payload))

    def exit(self):
        write_json(self.path / f"command-{self.sequence}.json", {"exit": True})

    def wait_grid(self, rect, cell):
        expected = [max(1, (rect[2] - 2 * PADDING) // cell[0]), max(1, (rect[3] - 2 * PADDING) // cell[1])]
        native.wait_for(lambda: [self.state()["cols"], self.state()["rows"]] == expected,
                        f"pane {self.name} real ConPTY size {expected}")
        return self.state()

    def cell_pixels(self):
        start = len(self.input())
        self.send(b"\x1b[16t")
        return cell_reply(self.input()[start:])

    def render(self):
        color = COLORS[self.name]
        palette = {1: tuple(value * 100 // 255 for value in color)}
        self.send(b"\x1b[0m\x1b[?25l\x1b[2J\x1b[H" + native.sixel(24, 18, [1] * (24 * 18), palette))


def run(binary, output, expected_arch=None):
    binary, output = binary.resolve(), output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    report = {"passed": False, "status": "running", "checks": [], "source_sha": os.environ.get("SOURCE_SHA"),
              "binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "transport": "real independent ConPTY children", "renderer": "one HWND and retained DIB",
              "report_file": output.name}
    process, window, directory = None, None, None
    probes = {}

    def check(name, **details):
        assert name == REQUIRED_CHECKS[len(report["checks"])], (name, report["checks"])
        report["checks"].append({"name": name, "passed": True, **details})
        write_json(output, report)

    def image_sample(rect, name):
        return rect[0] + PADDING + 4, rect[1] + PADDING + 4, COLORS[name]

    def focus(name, vk):
        window.chord(vk)
        native.wait_for(lambda: window.title().endswith("ztty workspace " + name), "focus pane " + name)

    def add(name, vk):
        probe = Probe(root, name, window)
        probes[name] = probe
        window.chord(vk)
        window.text(f'"{sys.executable}" "{Path(__file__).resolve()}" --child "{probe.path}"')
        # Unicode packets carry text; shell submission needs a real Enter key.
        window.chord(0x0D, control=False, shift=False)
        state = probe.ready()
        native.wait_for(lambda: window.title().endswith("ztty workspace " + name), "new pane title " + name)
        assert all(other.pid != probe.pid for key, other in probes.items() if key != name)
        assert Path(state["cwd"]).samefile(root)
        return state

    try:
        interpreter = native.process_architecture()
        arch = {0x8664: "x64", 0xAA64: "arm64"}.get(interpreter["native_machine"])
        assert arch and not interpreter["emulated"] and native.executable_machine(binary) == interpreter["native_machine"]
        assert expected_arch is None or arch == expected_arch
        report["architecture"], report["python_process"] = arch, interpreter
        directory = tempfile.TemporaryDirectory(prefix="ztty workspace 会话 ")
        root = Path(directory.name)
        (root / "a").mkdir()
        config = root / "appearance.conf"
        config.write_text("font = Consolas:size=12\nwindow-padding = 12\nbackground = #000000\n"
                          "cursor-blink = false\nbackground-opacity = 100\nselection-background = #224466\n"
                          "keybind = ctrl+shift+f6 = next-pane\n",
                          encoding="utf-8")
        environment = dict(os.environ)
        environment["COMSPEC"] = str(Path(os.environ["SystemRoot"]) / "System32" / "cmd.exe")
        process = subprocess.Popen([str(binary), "--config", str(config), "--cwd", str(root),
                                    "--geometry", f"{INITIAL_GRID[0]}x{INITIAL_GRID[1]}", "-e", sys.executable,
                                    str(Path(__file__).resolve()), "--child", str(root / "a")],
                                   cwd=root, env=environment)
        assert native.process_architecture(process.pid) == interpreter
        window = Window(process, report)
        window.place()
        report["desktop"] = native.desktop_state(window.user)
        initial = window.geometry()["client"][2:]
        cell = tuple((pixels - 2 * PADDING) // cells for pixels, cells in zip(initial, INITIAL_GRID))
        assert initial == [cells * pixels + 2 * PADDING for cells, pixels in zip(INITIAL_GRID, cell)] and min(cell) > 0
        report["cell_pixels"] = cell
        whole = (0, 0, *initial)
        probes["a"] = Probe(root, "a", window)
        a = probes["a"]
        state = a.ready()
        a.wait_grid(whole, cell)
        assert Path(state["cwd"]).samefile(root)
        initial_gdi = window.user.GetGuiResources(int(process._handle), 0)
        assert initial_gdi > 0
        check("native_architecture_and_initial_pty", pid=process.pid, child=state, gdi_objects=initial_gdi)

        state = add("b", 0x45)
        b = probes["b"]
        left, right = split_rect(whole, True)
        a.wait_grid(left, cell)
        b.wait_grid(right, cell)
        check("split_right_distinct_pty", child=state)
        a.render()
        b.render()
        cols = a.state()["cols"]
        edge = b"\x1b[3;" + str(cols).encode() + b"H" + native.sixel(5 * cell[0], 18, [1] * (5 * cell[0] * 18), {1: (100, 100, 100)})
        a.send(edge)
        samples = [image_sample(left, "a"), image_sample(right, "b"),
                   (PADDING + (cols - 1) * cell[0] + 2, PADDING + 2 * cell[1] + 3, [255, 255, 255]),
                   (PADDING + cols * cell[0] + 1, PADDING + 2 * cell[1] + 3, BLACK),
                   (left[2], PADDING + 2 * cell[1] + 3, BLACK),
                   (right[0] + 2, PADDING + 2 * cell[1] + 3, BLACK)]
        pixels = window.expect("split_viewport_pixels", samples)
        focus("a", 0x25)
        held = {}
        for kind in ("focus", "resize"):
            a.render()
            window.expect("committed frame before hold", [image_sample(left, "a")])
            start = time.monotonic()
            a.send(b"\x1b[?2026h\x1b[2J\x1b[H"
                   + native.sixel(24, 18, [1] * (24 * 18), {1: (0, 100, 0)}))
            if kind == "focus":
                focus("b", 0x27)
                held_rect = left
            else:
                window.chord(0xBB, alt=True)
                held_rect, grown_right = split_rect(whole, True, 550)
                a.wait_grid(held_rect, cell)
                b.wait_grid(grown_right, cell)
            held_pixels = window.expect("committed pixels survive hold " + kind, [image_sample(held_rect, "a")])
            window.capture(output, "hold-" + kind)
            elapsed = time.monotonic() - start
            assert elapsed < 0.150, f"hold observation missed the production 150ms deadline: {kind} {elapsed:.6f}s"
            held[kind] = {"elapsed_seconds": elapsed, "pixels": held_pixels}
            a.send(b"\x1b[?2026l")
            window.expect("held update becomes visible", [(held_rect[0] + PADDING + 4, held_rect[1] + PADDING + 4, [0, 255, 0])])
            if kind == "focus":
                focus("a", 0x25)
            else:
                window.chord(0xBD, alt=True)
                a.wait_grid(left, cell)
                b.wait_grid(right, cell)
        a.render()
        a.send(edge)
        focus("b", 0x27)
        check("split_viewport_pixels", pixels=pixels, synchronized_layout=held)
        window.capture(output, "split-right")

        marker_b, marker_a = b"workspace-b-input-74", b"workspace-a-input-92"
        window.text(marker_b.decode())
        native.wait_for(lambda: marker_b in b.input(), "focused pane B input")
        focus("a", 0x25)
        window.text(marker_a.decode())
        native.wait_for(lambda: marker_a in a.input(), "focused pane A input")
        assert marker_a not in b.input() and marker_b not in a.input()
        focus("b", 0x75)  # The configured F6 alias must use the same shared action.
        focus("a", 0x25)
        check("pane_keyboard_focus_isolation", received={"a": a.input().hex(), "b": b.input().hex()},
              custom_binding="ctrl+shift+f6 = next-pane")

        b.send(b"\x1b[?1000h\x1b[?1006h\x1b[6;5H\x1b[6n")
        native.wait_for(lambda: b"\x1b[6;5R" in b.input(), "inactive pane enables mouse tracking")
        before = a.input()
        window.mouse(0x020A, right[0] + PADDING + cell[0] * 2 + 2, PADDING + cell[1] * 2 + 2,
                     120 << 16, screen=True)
        native.wait_for(lambda: b"\x1b[<64;3;3M" in b.input(), "hover scroll reaches inactive pane B")
        assert a.input() == before
        a.send(b"\x1b[?1002h\x1b[?1006h")
        b.send(b"\x1b[?1000l\x1b[?1002h")
        a_start, b_start = len(a.input()), len(b.input())
        ax, by = PADDING + 2 * cell[0] + 2, PADDING + 2 * cell[1] + 2
        bx = right[0] + PADDING + 2 * cell[0] + 2
        window.mouse(0x0201, ax, by, 1)
        window.mouse(0x0200, bx, by, 1)
        window.mouse(0x0202, bx, by)
        a.send(b"")
        b.send(b"")
        a_events, b_events = a.input()[a_start:], b.input()[b_start:]
        for event in (b"\x1b[<0;3;3M", f"\x1b[<32;{cols};3M".encode(), f"\x1b[<0;{cols};3m".encode()):
            assert event in a_events, (event, a_events)
        assert b"\x1b[<" not in b_events, "drag crossed into a different pane's PTY"
        window.mouse(0x0201, ax, by, 1)
        focus("b", 0x27)
        b_start = len(b.input())
        window.mouse(0x0200, bx, by, 1)
        window.mouse(0x0202, bx, by)
        b.send(b"")
        assert b"\x1b[<" not in b.input()[b_start:], "cancelled drag released into the newly focused pane"
        a.send(b"\x1b[?1002l\x1b[?1006l")
        b.send(b"\x1b[?1002l\x1b[?1006l")
        focus("a", 0x25)
        check("inactive_pane_mouse_tracking", response=b.input().hex(), drag_events=a_events.hex(),
              cancelled_drag_recipient=b.input()[b_start:].hex(), active_title=window.title())

        a.send(b"\x1b[0m\x1b[2J\x1b[H\x1b[48;2;0;0;255m"
               + (b" " * cols + b"\r\n") * 48 + b"\x1b[0m")
        a.render()
        window.chord(0x21, control=False)
        history_sample = (PADDING + 5 * cell[0] + 2, PADDING + 3, [0, 0, 255])
        pixels = window.expect("pane A history", [history_sample, image_sample(right, "b")])
        before = a.input()
        # The complete shortcut includes Shift release. Further silent
        # modifier pairs must also preserve history and send no PTY input.
        window.inputs([(key, 0, flags) for key in (0x10, 0x11) for flags in (0, 2)])
        focus("b", 0x27)
        window.expect("history survives pane focus", [history_sample, image_sample(right, "b")])
        sequence = a.queue(b"")
        response = b"\x1b_G" + a.queries[sequence] + b"\x1b\\"
        a.done(sequence)
        assert a.input() == before + response, "silent keys or focus shortcut reached pane A's PTY"
        window.mouse(0x020A, PADDING + 5 * cell[0], PADDING + 2 * cell[1],
                     ((-120 * 50) & 65535) << 16, screen=True)
        window.expect("hover history return", [image_sample(left, "a"), image_sample(right, "b")])
        focus("a", 0x25)
        window.chord(0x21, control=False)
        window.expect("history before actual input", [history_sample, image_sample(right, "b")])
        before = a.input()
        window.chord(0x0D, control=False, shift=False)
        native.wait_for(lambda: a.input() == before + b"\r", "actual input from history")
        live_pixels = window.expect("actual input returns to live", [image_sample(left, "a"), image_sample(right, "b")])
        focus("b", 0x27)
        check("pane_history_isolation", pixels=pixels, silent_modifiers=["shift", "control"],
              actual_input="0d", live_pixels=live_pixels)

        selection_y = PADDING + 4 * cell[1] + 3
        start, end = right[0] + PADDING + 2 * cell[0] + 2, right[0] + PADDING + 6 * cell[0] + 2
        window.mouse(0x0201, start, selection_y, 1)
        window.mouse(0x0200, end, selection_y, 1)
        window.mouse(0x0202, end, selection_y)
        selected = (right[0] + PADDING + 4 * cell[0] + 2, selection_y, [34, 68, 102])
        pixels = window.expect("pane B selection", [selected, (PADDING + 4 * cell[0] + 2, selection_y, BLACK)])
        window.inputs([(key, 0, flags) for key in (0x10, 0x11) for flags in (0, 2)])
        focus("a", 0x25)
        window.expect("selection survives pane focus", [selected, image_sample(left, "a")])
        check("pane_selection_isolation", pixels=pixels)
        focus("b", 0x27)

        state = add("c", 0x4F)
        c = probes["c"]
        top, bottom = split_rect(right, False)
        b.wait_grid(top, cell)
        c.wait_grid(bottom, cell)
        check("split_down_distinct_pty", child=state)
        # Isolate geometry from history; render() above must preserve scrollback.
        for probe in probes.values():
            probe.send(b"\x1b[3J")
            probe.render()
        gdi = window.user.GetGuiResources(int(process._handle), 0)
        assert len(window.windows()) == 1 and 0 < gdi <= initial_gdi + 2, (initial_gdi, gdi, window.windows())
        pixels = window.expect("three_pane_pixels_and_shared_window", [image_sample(left, "a"), image_sample(top, "b"),
                              image_sample(bottom, "c"), (right[0] + 4, top[3], BLACK)])
        check("three_pane_pixels_and_shared_window", pixels=pixels, gdi_objects=gdi, windows=window.windows())
        window.capture(output, "three-panes")

        window.chord(0x0D)
        c.wait_grid(whole, cell)
        pixels = window.expect("maximized pane C", [image_sample(whole, "c"), (right[0] + PADDING + 4, PADDING + 4, BLACK)])
        window.chord(0x0D)
        c.wait_grid(bottom, cell)
        window.expect("restored three panes", [image_sample(left, "a"), image_sample(top, "b"), image_sample(bottom, "c")])
        assert all(probe.state()["pid"] == probe.pid for probe in probes.values())
        check("maximize_restore_keeps_pty_identity", pixels=pixels)

        window.chord(0xBB, alt=True)
        grown_top, grown_bottom = split_rect(right, False, 450)
        b.wait_grid(grown_top, cell)
        c.wait_grid(grown_bottom, cell)
        window.chord(0xBD, alt=True)
        b.wait_grid(top, cell)
        c.wait_grid(bottom, cell)
        check("grow_shrink_updates_child_geometry", restored={name: probe.state() for name, probe in probes.items()})

        window.chord(0x28)
        c.wait_grid(left, cell)
        a.wait_grid(bottom, cell)
        pixels = window.expect("swapped session pixels", [image_sample(left, "c"), image_sample(top, "b"), image_sample(bottom, "a")])
        window.chord(0x26)
        c.wait_grid(bottom, cell)
        a.wait_grid(left, cell)
        check("swap_keeps_session_identity", pixels=pixels)

        smaller = [initial[0] - 8 * cell[0], initial[1] - 4 * cell[1]]
        window.place(smaller)
        small_left, small_right = split_rect((0, 0, *smaller), True)
        small_top, small_bottom = split_rect(small_right, False)
        for name, rect in (("a", small_left), ("b", small_top), ("c", small_bottom)):
            probes[name].wait_grid(rect, cell)
        pixels = window.expect("resized workspace pixels", [image_sample(small_left, "a"), image_sample(small_top, "b"),
                                                            image_sample(small_bottom, "c")])
        window.capture(output, "resized")
        window.place(initial)
        for name, rect in (("a", left), ("b", top), ("c", bottom)):
            probes[name].wait_grid(rect, cell)
        check("window_resize_coordinates_all_ptys", pixels=pixels)

        state = add("d", 0x54)
        d = probes["d"]
        d.wait_grid(whole, cell)
        d.render()
        check("new_tab_distinct_pty", child=state, pixels=window.expect("new tab", [image_sample(whole, "d")]))
        pending = a.queue(b"\x1b[0m\x1b[H" + b"hidden-output\r\n" * 120000 + b"\x1b[7;9H\x1b[6n")
        window.text("visible-tab-still-responsive")
        native.wait_for(lambda: b"visible-tab-still-responsive" in d.input(), "foreground input with hidden output")
        a.done(pending)
        native.wait_for(lambda: b"\x1b[7;9R" in a.input(), "hidden tab output fully parsed and answered")
        a.render()
        check("hidden_tab_output_and_response", response=a.input()[-256:].hex(), pixels=window.expect("visible tab retained", [image_sample(whole, "d")]))
        focus("a", 0x21)
        pixels = window.expect("tab switch restores panes", [image_sample(left, "a"), image_sample(top, "b"), image_sample(bottom, "c")])
        window.capture(output, "tab-restored")
        focus("d", 0x22)
        window.expect("next tab image restored", [image_sample(whole, "d")])
        focus("a", 0x21)
        window.chord(0x09, shift=False)
        native.wait_for(lambda: window.title().endswith("ztty workspace d"), "Ctrl+Tab selects next tab")
        next_tab_pixels = window.expect("Ctrl+Tab preserves image", [image_sample(whole, "d")])
        window.chord(0x09)
        native.wait_for(lambda: window.title().endswith("ztty workspace a"), "Ctrl+Shift+Tab selects previous tab")
        previous_tab_pixels = window.expect("Ctrl+Shift+Tab preserves panes",
            [image_sample(left, "a"), image_sample(top, "b"), image_sample(bottom, "c")])
        assert all(probe.state()["pid"] == probe.pid for probe in probes.values())
        check("tab_switch_restores_pane_images", pixels=pixels,
              control_tab_pixels=next_tab_pixels, control_shift_tab_pixels=previous_tab_pixels)

        # Font shortcuts share one HWND/font set, including the hidden tab.
        # Query real cell metrics separately from the rounded ConPTY grid.
        font_window = window.geometry()
        font_pids = {name: probe.pid for name, probe in probes.items()}
        font_input_starts = {name: len(probe.input()) for name, probe in probes.items()}
        assert all(probe.cell_pixels() == cell for probe in probes.values())
        visible_panes = (("a", left), ("b", top), ("c", bottom))

        def font_trial(label, vk, accepted, shift=True):
            hidden_state = d.state()
            hidden_before = [hidden_state["cols"], hidden_state["rows"]]
            window.chord(vk, shift=shift)
            def ready():
                current = a.cell_pixels()
                return current if accepted(current) else None
            measured = native.wait_for(ready, "font metrics " + label)
            states = {}
            for name, probe in probes.items():
                assert probe.cell_pixels() == measured, f"font metrics differ in pane {name}"
                assert probe.state()["pid"] == font_pids[name], "font action replaced a PTY"
                states[name] = probe.state()
            for name, rect in visible_panes:
                states[name] = probes[name].wait_grid(rect, measured)
            assert [states["d"]["cols"], states["d"]["rows"]] == hidden_before, \
                "hidden tab grid changed before being shown"
            geometry = window.geometry()
            for field in ("hwnd", "pid", "client", "outer", "dpi"):
                assert geometry[field] == font_window[field], f"font action changed window {field}"
            assert window.title().endswith("ztty workspace a"), "font action changed pane focus"
            samples = []
            for name, rect in visible_panes:
                probe = probes[name]
                probe.render()
                red, green, blue = COLORS[name]
                probe.send(f"\x1b[4;4H\x1b[48;2;{red};{green};{blue}m \x1b[0m".encode("ascii"))
                x, y = rect[0] + PADDING, rect[1] + PADDING
                samples.extend((image_sample(rect, name),
                    (x + 3 * measured[0] + measured[0] // 2, y + 3 * measured[1] + measured[1] // 2, COLORS[name]),
                    (x + 4 * measured[0] + 1, y + 3 * measured[1] + measured[1] // 2, BLACK)))
            pixels = window.expect("font-sized cells " + label, samples)
            return {"shortcut_vk": vk, "control": True, "shift": shift,
                    "cell_pixels": list(measured), "panes": states, "window": geometry, "pixels": pixels}

        increase = font_trial("increase", 0xBB,
            lambda value: value != cell and all(new >= old for new, old in zip(value, cell)))
        enlarged = tuple(increase["cell_pixels"])
        window.capture(output, "font-increased")
        focus("d", 0x22)
        revealed = d.wait_grid(whole, enlarged)
        assert d.cell_pixels() == enlarged
        d.render()
        window.expect("zoomed hidden tab becomes visible", [image_sample(whole, "d")])
        focus("a", 0x21)
        for name, rect in visible_panes:
            probes[name].wait_grid(rect, enlarged)
        check("font_increase_coordinates_all_ptys", original_cell=list(cell), enlarged_cell=list(enlarged),
              trial=increase, revealed_hidden_tab=revealed, pixels=increase["pixels"])

        decreased = font_trial("decrease", 0xBD, lambda value: value == cell)
        smaller = font_trial("control-minus", 0xBD,
            lambda value: value != cell and all(new <= old for new, old in zip(value, cell)), shift=False)
        reduced = tuple(smaller["cell_pixels"])
        keypad_add = font_trial("keypad-add", 0x6B, lambda value: value == cell, shift=False)
        keypad_subtract = font_trial("keypad-subtract", 0x6D, lambda value: value == reduced, shift=False)
        check("font_decrease_and_keypad_zoom", original_cell=list(cell), smaller_cell=list(reduced),
              trials=[decreased, smaller, keypad_add, keypad_subtract], pixels=keypad_subtract["pixels"])

        # The configured size is 12, deliberately different from the default 11.
        reset = font_trial("configured-reset", 0x30, lambda value: value == cell, shift=False)
        raised_again = font_trial("keypad-increase-before-reset", 0x6B, lambda value: value == enlarged, shift=False)
        # Inject the logical VK_NUMPAD0, not a hardware scan code. This tests
        # that dispatch branch without changing or claiming NumLock state.
        keypad_reset = font_trial("logical-keypad-configured-reset", 0x60, lambda value: value == cell, shift=False)
        tails = {name: probe.input()[font_input_starts[name]:] for name, probe in probes.items()}
        assert all(not without_probe_replies(tail) for tail in tails.values()), \
            "a font shortcut leaked into a PTY"
        final_gdi = window.user.GetGuiResources(int(process._handle), 0)
        assert 0 < final_gdi <= initial_gdi + 2, (initial_gdi, final_gdi)
        window.capture(output, "font-reset")
        check("font_reset_restores_configured_metrics", configured_size=12, original_cell=list(cell),
              reset_shortcut="Ctrl+0", ctrl_shift_zero_native_verified=False,
              trials=[reset, raised_again, keypad_reset], input_kind="virtual-key", numlock_changed=False,
              physical_numlock_state_verified=False,
              shortcut_input_bytes={name: without_probe_replies(tail).hex() for name, tail in tails.items()},
              gdi_objects=final_gdi, pixels=keypad_reset["pixels"])

        a.exit()
        native.wait_for(lambda: window.kernel.WaitForSingleObject(a.handle, 0) == 0, "initial PTY child exits")
        full_top, full_bottom = split_rect(whole, False)
        b.wait_grid(full_top, cell)
        c.wait_grid(full_bottom, cell)
        native.wait_for(lambda: window.title().endswith("ztty workspace b"), "sibling receives focus")
        b.render()
        c.render()
        check("natural_exit_keeps_siblings", pixels=window.expect("siblings survive child exit", [image_sample(full_top, "b"), image_sample(full_bottom, "c")]))
        window.chord(0x57)
        c.wait_grid(whole, cell)
        window.chord(0x57)
        native.wait_for(lambda: window.title().endswith("ztty workspace d"), "other tab survives closing first tab")
        assert process.poll() is None
        check("close_keeps_other_tab", pixels=window.expect("other tab survives", [image_sample(whole, "d")]))
        window.capture(output, "surviving-tab")
        window.chord(0x57)
        native.wait_for(lambda: process.poll() is not None, "last pane closes the owning window")
        assert process.returncode == 0 and not window.windows()
        check("last_pane_closes_window", returncode=process.returncode)
        report["passed"] = True
    except BaseException as error:
        report["error"] = repr(error)
        try:
            stack = "".join(traceback.format_tb(error.__traceback__, limit=-32)).encode("utf-8", errors="replace")
            report["exception"] = {"type": type(error).__name__[:256],
                "errno": getattr(error, "errno", None), "winerror": getattr(error, "winerror", None),
                "traceback": stack[-16384:].decode("utf-8", errors="ignore"),
                "traceback_truncated": len(stack) > 16384}
            for name in ("filename", "filename2"):
                value = getattr(error, name, None)
                report["exception"][name] = (str(value).encode("utf-8", errors="replace")[:2048]
                    .decode("utf-8", errors="ignore")) if value is not None else None
        except Exception as diagnostic_error:
            report["exception_capture_error"] = repr(diagnostic_error)[:2048]
        report["status"] = "blocked" if isinstance(error, native.DesktopBlocked) else "failed"
        if window and window.user.IsWindow(window.hwnd):
            try:
                report["failure_window"] = window.geometry()
                window.capture(output, "failure")
            except Exception as diagnostic_error:
                report["capture_error"] = repr(diagnostic_error)
    finally:
        report["visible_pixel_checks_completed"] = sum("pixels" in check for check in report["checks"])
        report["pixel_readback"] = "GetPixel with WindowFromPoint/PtVisible guards; owned BitBlt/GetDIBits PNGs"
        report["probe_processes"] = {name: {"pid": getattr(probe, "pid", None)} for name, probe in probes.items()}
        def request_close():
            if window and window.pid(window.hwnd) == process.pid:
                window.user.PostMessageW(window.hwnd, 0x0010, 0, 0)
        native.cleanup_owned(process, request_close, directory, report)
        exited = True
        for name, probe in probes.items():
            if probe.handle:
                done = window.kernel.WaitForSingleObject(probe.handle, 2000) == 0
                report["probe_processes"][name]["exited"] = done
                exited &= done
                window.kernel.CloseHandle(probe.handle)
        report["cleanup"]["all_probe_processes_exited"] = exited
        report["passed"] &= exited
        report["status"] = "passed" if report["passed"] else ("blocked" if report["status"] == "blocked" else "failed")
        if report["passed"] and not validate_report(report):
            report.update({"passed": False, "status": "failed", "error": "incomplete Windows workspace evidence"})
        write_json(output, report)
    return report["passed"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--child", type=Path)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--arch", choices=("x64", "arm64"))
    args = parser.parse_args()
    if sys.platform != "win32":
        parser.error("native Windows is required")
    if args.child:
        child(args.child)
        return 0
    if args.binary is None or args.output is None:
        parser.error("--binary and --output are required")
    return 0 if run(args.binary, args.output, args.arch) else 1


if __name__ == "__main__":
    raise SystemExit(main())
