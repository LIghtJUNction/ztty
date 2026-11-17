#!/usr/bin/env python3
"""Native Windows acceptance gate (requires an interactive Windows desktop).

python tests/windows_native.py --binary target/release/ztty.exe \
    --output artifacts/windows-native.json

Uses an actual Python child behind ConPTY, Win32 window messages, and GetPixel
on the client DC, plus owned-window BitBlt/GetDIBits PNG captures. Kitty and
Sixel packets must traverse ConPTY and produce the expected actual pixels;
a cross compilation is not a substitute for this runtime gate.
"""

import argparse
import base64
import binascii
import ctypes
import hashlib
from ctypes import wintypes
import json
import os
from pathlib import Path
import subprocess
import struct
import sys
import tempfile
import threading
import time
import zlib


def kitty(header, payload=b""):
    return b"\x1b_G" + header.encode("ascii") + b";" + base64.b64encode(payload) + b"\x1b\\"


def png_rgba(width, height, pixels):
    def chunk(kind, payload):
        body = kind + payload
        return struct.pack(">I", len(payload)) + body + struct.pack(">I", binascii.crc32(body) & 0xFFFFFFFF)

    rows = b"".join(b"\0" + pixels[row * width * 4:(row + 1) * width * 4] for row in range(height))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def sixel(width, height, indices, palette, transparent=True):
    """Small deterministic Sixel vectors, in native pixels with a 1:1 aspect."""
    assert len(indices) == width * height
    body = bytearray(f'"1;1;{width};{height}'.encode("ascii"))
    for index, rgb in palette.items():
        body.extend((f"#{index};2;" + ";".join(map(str, rgb))).encode("ascii"))
    for row in range(0, height, 6):
        for index in palette:
            body.extend(f"#{index}".encode("ascii"))
            body.extend(63 + sum(1 << bit for bit in range(min(6, height - row))
                                if indices[(row + bit) * width + col] == index)
                        for col in range(width))
            body.extend(b"$")
        if row + 6 < height:
            body.extend(b"-")
    return b"\x1bP7;" + (b"1" if transparent else b"0") + b"q" + body + b"\x1b\\"


class NativePoint(ctypes.Structure):
    _fields_ = [("x", ctypes.c_int32), ("y", ctypes.c_int32)]


class NativeRect(ctypes.Structure):
    _fields_ = [(name, ctypes.c_int32) for name in ("left", "top", "right", "bottom")]


class MonitorInfo(ctypes.Structure):
    _fields_ = [("size", ctypes.c_uint32), ("monitor", NativeRect),
                ("work", NativeRect), ("flags", ctypes.c_uint32)]


def rect_values(rect):
    return [rect.left, rect.top, rect.right, rect.bottom]


class BitmapInfoHeader(ctypes.Structure):
    _fields_ = [("size", ctypes.c_uint32), ("width", ctypes.c_int32),
                ("height", ctypes.c_int32), ("planes", ctypes.c_uint16),
                ("bit_count", ctypes.c_uint16), ("compression", ctypes.c_uint32),
                ("image_size", ctypes.c_uint32), ("x_pixels_per_meter", ctypes.c_int32),
                ("y_pixels_per_meter", ctypes.c_int32), ("colors_used", ctypes.c_uint32),
                ("important_colors", ctypes.c_uint32)]


class BitmapInfo(ctypes.Structure):
    _fields_ = [("header", BitmapInfoHeader), ("colors", ctypes.c_uint32 * 1)]


def capture_window(user, gdi, hwnd, expected_pid, path):
    """Capture only our client DC; save the actual BitBlt/GetDIBits pixels."""
    owner = wintypes.DWORD()
    user.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
    if owner.value != expected_pid:
        raise AssertionError("refusing to capture a window owned by another process")
    rect = wintypes.RECT()
    if not user.GetClientRect(hwnd, ctypes.byref(rect)):
        raise OSError("GetClientRect failed during window capture")
    width, height = rect.right - rect.left, rect.bottom - rect.top
    if not (0 < width <= 8192 and 0 < height <= 8192 and width * height <= 16 * 1024 * 1024):
        raise AssertionError(f"invalid native capture dimensions: {width}x{height}")
    gdi.CreateCompatibleDC.argtypes = [wintypes.HDC]
    gdi.CreateCompatibleDC.restype = wintypes.HDC
    gdi.CreateCompatibleBitmap.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int]
    gdi.CreateCompatibleBitmap.restype = wintypes.HANDLE
    gdi.SelectObject.argtypes = [wintypes.HDC, wintypes.HANDLE]
    gdi.SelectObject.restype = wintypes.HANDLE
    gdi.BitBlt.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                         wintypes.HDC, ctypes.c_int, ctypes.c_int, wintypes.DWORD]
    gdi.BitBlt.restype = wintypes.BOOL
    gdi.PatBlt.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.DWORD]
    gdi.PatBlt.restype = wintypes.BOOL
    gdi.GetDIBits.argtypes = [wintypes.HDC, wintypes.HANDLE, wintypes.UINT, wintypes.UINT,
                             ctypes.c_void_p, ctypes.POINTER(BitmapInfo), wintypes.UINT]
    gdi.GetDIBits.restype = ctypes.c_int
    gdi.DeleteObject.argtypes = [wintypes.HANDLE]
    gdi.DeleteObject.restype = wintypes.BOOL
    gdi.DeleteDC.argtypes = [wintypes.HDC]
    gdi.DeleteDC.restype = wintypes.BOOL
    dc = user.GetDC(hwnd)
    if not dc:
        raise OSError("GetDC failed during window capture")
    memory = bitmap = previous = None
    try:
        memory = gdi.CreateCompatibleDC(dc)
        if not memory:
            raise OSError("CreateCompatibleDC failed")
        bitmap = gdi.CreateCompatibleBitmap(dc, width, height)
        if not bitmap:
            raise OSError("CreateCompatibleBitmap failed")
        previous = gdi.SelectObject(memory, bitmap)
        if not previous or previous == ctypes.c_void_p(-1).value:
            previous = None
            raise OSError("SelectObject failed")
        # A clipped source must not expose uninitialized compatible-bitmap
        # memory in failure evidence. Pixel assertions independently require
        # each sample to be visible and owned by this window.
        if not gdi.PatBlt(memory, 0, 0, width, height, 0x00000042):
            raise OSError("clearing the capture bitmap failed")
        # SRCCOPY excludes other layered windows; the source is this HWND's DC.
        if not gdi.BitBlt(memory, 0, 0, width, height, dc, 0, 0, 0x00CC0020):
            raise OSError(f"BitBlt failed: Win32 error {ctypes.get_last_error()}")
        # GetDIBits requires the bitmap not to be selected into any DC.
        restored = gdi.SelectObject(memory, previous)
        if not restored or restored == ctypes.c_void_p(-1).value:
            raise OSError("restoring the capture DC's bitmap failed")
        previous = None
        info = BitmapInfo()
        info.header = BitmapInfoHeader(ctypes.sizeof(BitmapInfoHeader), width, -height,
                                       1, 32, 0, width * height * 4, 0, 0, 0, 0)
        bits = ctypes.create_string_buffer(width * height * 4)
        if gdi.GetDIBits(dc, bitmap, 0, height, bits, ctypes.byref(info), 0) != height:
            raise OSError(f"GetDIBits did not return all rows: Win32 error {ctypes.get_last_error()}")
        bgra = bits.raw
        rgba = bytearray(bgra)
        rgba[0::4], rgba[2::4] = bgra[2::4], bgra[0::4]
        rgba[3::4] = b"\xff" * (width * height)
        encoded = png_rgba(width, height, rgba)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(encoded)
        return {"path": str(path), "width": width, "height": height, "window_pid": owner.value,
                "sha256": hashlib.sha256(encoded).hexdigest(),
                "capture": "owned Win32 client DC via BitBlt/GetDIBits"}
    finally:
        if previous and memory:
            gdi.SelectObject(memory, previous)
        if bitmap:
            gdi.DeleteObject(bitmap)
        if memory:
            gdi.DeleteDC(memory)
        user.ReleaseDC(hwnd, dc)


def child(directory):
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.GetStdHandle.restype = wintypes.HANDLE
    kernel.GetStdHandle.argtypes = [wintypes.DWORD]
    kernel.SetConsoleMode.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel.ReadFile.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD,
                               ctypes.POINTER(wintypes.DWORD), ctypes.c_void_p]
    stdin = kernel.GetStdHandle(-10 & 0xFFFFFFFF)
    stdout = kernel.GetStdHandle(-11 & 0xFFFFFFFF)
    if not kernel.SetConsoleMode(stdin, 0x200):
        raise ctypes.WinError(ctypes.get_last_error())
    if not kernel.SetConsoleMode(stdout, 0x1 | 0x4 | 0x8):
        raise ctypes.WinError(ctypes.get_last_error())
    directory = Path(directory)
    received = bytearray()

    def read_input():
        buffer = ctypes.create_string_buffer(4096)
        while True:
            count = wintypes.DWORD()
            if not kernel.ReadFile(stdin, buffer, len(buffer), ctypes.byref(count), None):
                return
            if not count.value:
                return
            received.extend(buffer.raw[:count.value])
            if len(received) > 65536:
                del received[:-65536]
            (directory / "keyboard.bin").write_bytes(received)

    threading.Thread(target=read_input, daemon=True).start()
    sys.stdout.buffer.write(b"\x1b]2;ztty native acceptance\x07\x1b[?25l\x1b[2J\x1b[H")
    sys.stdout.buffer.flush()
    (directory / "ready").write_text("ready", encoding="utf-8")
    sequence = 0
    while True:
        path = directory / f"command-{sequence}.bin"
        if path.exists():
            payload = path.read_bytes()
            sys.stdout.buffer.write(payload)
            sys.stdout.buffer.flush()
            (directory / f"done-{sequence}").write_text("sent", encoding="utf-8")
            sequence += 1
        time.sleep(0.01)


def wait_for(predicate, description, timeout=10):
    deadline = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < deadline:
        try:
            result = predicate()
            if result:
                return result
        except (OSError, ValueError) as error:
            last_error = error
        time.sleep(0.02)
    raise AssertionError(f"timeout: {description}; last error: {last_error}")


def process_architecture(pid=None):
    """Record the native machine and reject emulated acceptance processes."""
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.GetCurrentProcess.restype = wintypes.HANDLE
    kernel.GetCurrentProcess.argtypes = []
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.IsWow64Process2.restype = wintypes.BOOL
    kernel.IsWow64Process2.argtypes = [wintypes.HANDLE, ctypes.POINTER(ctypes.c_ushort),
                                      ctypes.POINTER(ctypes.c_ushort)]
    handle = kernel.GetCurrentProcess() if pid is None else kernel.OpenProcess(0x1000, False, pid)
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    process_machine = ctypes.c_ushort()
    native_machine = ctypes.c_ushort()
    try:
        if not kernel.IsWow64Process2(handle, ctypes.byref(process_machine), ctypes.byref(native_machine)):
            raise ctypes.WinError(ctypes.get_last_error())
    finally:
        if pid is not None:
            kernel.CloseHandle(handle)
    return {"process_machine": process_machine.value, "native_machine": native_machine.value,
            "emulated": process_machine.value != 0}


def executable_machine(binary):
    with binary.open("rb") as stream:
        header = stream.read(64)
        if len(header) != 64 or header[:2] != b"MZ":
            raise AssertionError(f"{binary}: expected a PE executable")
        stream.seek(struct.unpack_from("<I", header, 0x3C)[0])
        pe = stream.read(6)
    if len(pe) != 6 or pe[:4] != b"PE\0\0":
        raise AssertionError(f"{binary}: invalid PE header")
    return struct.unpack_from("<H", pe, 4)[0]


def run(binary, output, expected_arch=None):
    user = ctypes.WinDLL("user32", use_last_error=True)
    gdi = ctypes.WinDLL("gdi32", use_last_error=True)
    enum_proc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user.EnumWindows.argtypes = [enum_proc, wintypes.LPARAM]
    user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    user.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    user.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int,
                                 ctypes.c_int, ctypes.c_int, wintypes.UINT]
    user.GetDC.argtypes = [wintypes.HWND]
    user.GetDC.restype = wintypes.HDC
    user.ReleaseDC.argtypes = [wintypes.HWND, wintypes.HDC]
    user.SendMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    user.SendMessageW.restype = wintypes.LPARAM
    user.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    user.SetForegroundWindow.argtypes = [wintypes.HWND]
    gdi.GetPixel.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int]
    gdi.GetPixel.restype = wintypes.DWORD
    user.ClientToScreen.argtypes = [wintypes.HWND, ctypes.POINTER(NativePoint)]
    user.ClientToScreen.restype = wintypes.BOOL
    user.MonitorFromWindow.argtypes = [wintypes.HWND, wintypes.DWORD]
    user.MonitorFromWindow.restype = wintypes.HANDLE
    user.GetMonitorInfoW.argtypes = [wintypes.HANDLE, ctypes.POINTER(MonitorInfo)]
    user.GetMonitorInfoW.restype = wintypes.BOOL
    user.WindowFromPoint.argtypes = [NativePoint]
    user.WindowFromPoint.restype = wintypes.HWND
    user.GetAncestor.argtypes = [wintypes.HWND, wintypes.UINT]
    user.GetAncestor.restype = wintypes.HWND
    user.GetForegroundWindow.argtypes = []
    user.GetForegroundWindow.restype = wintypes.HWND
    user.IsWindowVisible.argtypes = [wintypes.HWND]
    user.IsWindowVisible.restype = wintypes.BOOL
    user.IsIconic.argtypes = [wintypes.HWND]
    user.IsIconic.restype = wintypes.BOOL
    user.GetDpiForWindow.argtypes = [wintypes.HWND]
    user.GetDpiForWindow.restype = wintypes.UINT
    gdi.GetClipBox.argtypes = [wintypes.HDC, ctypes.POINTER(wintypes.RECT)]
    gdi.GetClipBox.restype = ctypes.c_int
    gdi.PtVisible.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int]
    gdi.PtVisible.restype = wintypes.BOOL
    user.SetProcessDPIAware()
    checks = []
    process = None
    hwnd = None
    report = {"platform": sys.platform, "binary": str(binary.resolve()),
              "transport": "real ConPTY", "renderer": "Win32 retained DIB",
              "checks": checks, "passed": False}
    def window_pid(window):
        pid = wintypes.DWORD()
        if window:
            user.GetWindowThreadProcessId(window, ctypes.byref(pid))
        return pid.value

    def screen_point(x, y):
        point = NativePoint(int(x), int(y))
        if not user.ClientToScreen(hwnd, ctypes.byref(point)):
            raise OSError("ClientToScreen failed")
        return point

    def window_geometry(samples=()):
        client, outer, clip = wintypes.RECT(), wintypes.RECT(), wintypes.RECT()
        if not user.GetClientRect(hwnd, ctypes.byref(client)) or not user.GetWindowRect(hwnd, ctypes.byref(outer)):
            raise OSError("cannot read native window geometry")
        top_left = screen_point(0, 0)
        bottom_right = screen_point(client.right, client.bottom)
        monitor = user.MonitorFromWindow(hwnd, 2)
        info = MonitorInfo()
        info.size = ctypes.sizeof(MonitorInfo)
        if not monitor or not user.GetMonitorInfoW(monitor, ctypes.byref(info)):
            raise OSError("cannot read native monitor work area")
        dc = user.GetDC(hwnd)
        if not dc:
            raise OSError("cannot inspect native window DC")
        try:
            clip_type = gdi.GetClipBox(dc, ctypes.byref(clip))
            points = []
            for x, y in samples:
                screen = screen_point(x, y)
                hit = user.WindowFromPoint(screen)
                root_window = user.GetAncestor(hit, 2) if hit else None
                color = int(gdi.GetPixel(dc, int(x), int(y)))
                points.append({"client": [int(x), int(y)], "screen": [screen.x, screen.y],
                               "point_visible": bool(gdi.PtVisible(dc, int(x), int(y))),
                               "window_from_point": int(hit or 0), "root_window": int(root_window or 0),
                               "window_from_point_pid": window_pid(hit),
                               "owned_window_at_point": root_window == hwnd and window_pid(hit) == process.pid,
                               "rgb": None if color == 0xFFFFFFFF else
                               [color & 255, (color >> 8) & 255, (color >> 16) & 255]})
        finally:
            user.ReleaseDC(hwnd, dc)
        return {"client": rect_values(client), "outer": rect_values(outer),
                "client_screen": [top_left.x, top_left.y, bottom_right.x, bottom_right.y],
                "dpi": int(user.GetDpiForWindow(hwnd)), "visible": bool(user.IsWindowVisible(hwnd)),
                "minimized": bool(user.IsIconic(hwnd)), "window_pid": window_pid(hwnd),
                "foreground_pid": window_pid(user.GetForegroundWindow()),
                "monitor": {"rect": rect_values(info.monitor), "work": rect_values(info.work),
                            "primary": bool(info.flags & 1)},
                "clip_type": clip_type, "clip_box": rect_values(clip), "samples": points}

    def place_owned_window():
        before = window_geometry()
        if before["window_pid"] != process.pid:
            raise AssertionError("native test window changed ownership")
        left, top, right, bottom = before["monitor"]["work"]
        outer = before["outer"]
        width, height = outer[2] - outer[0], outer[3] - outer[1]
        if width > right - left or height > bottom - top:
            raise AssertionError("test window does not fit the monitor work area")
        x = left + min(32, (right - left - width) // 2)
        y = top + min(32, (bottom - top - height) // 2)
        # Move only this new test window, without resizing or activating another
        # process. Topmost placement keeps GetPixel away from unrelated windows.
        if not user.SetWindowPos(hwnd, ctypes.c_void_p(-1), x, y, 0, 0, 0x0011):
            raise OSError("cannot place the owned test window inside its monitor")

        def visible_inside_work_area():
            state = window_geometry()
            bounds = state["client_screen"]
            return (state["visible"] and not state["minimized"] and
                    left <= bounds[0] < bounds[2] <= right and top <= bounds[1] < bounds[3] <= bottom)

        wait_for(visible_inside_work_area, "owned native window visible inside monitor work area")
        report["window_placement"] = {"before": before, "placed": window_geometry(),
                                      "requested_topmost": True, "position": [x, y]}

    try:
        interpreter = process_architecture()
        machine = interpreter["native_machine"]
        architecture = {0x8664: "x64", 0xAA64: "arm64"}.get(machine)
        report["architecture"] = architecture
        report["python_process"] = interpreter
        report["binary_machine"] = executable_machine(binary)
        assert architecture is not None, f"unsupported native machine: 0x{machine:04x}"
        assert not interpreter["emulated"], f"Python acceptance process is emulated: {interpreter}"
        assert report["binary_machine"] == machine, "ztty executable does not match the native machine"
        if expected_arch is not None:
            assert architecture == expected_arch, f"expected {expected_arch}; runner is {architecture}"
        checks.append({"name": "native_python_and_binary_architecture", "passed": True})
        with tempfile.TemporaryDirectory(prefix="ztty native 图像 ") as temporary:
            directory = Path(temporary)
            clean_environment = os.environ.copy()
            clean_environment["APPDATA"] = str(directory / "missing-parent" / "config")
            shell = os.environ.get("COMSPEC", str(Path(os.environ["SystemRoot"]) / "System32" / "cmd.exe"))
            startup = subprocess.run([str(binary.resolve()), "-e", shell, "/D", "/C", "exit 0"],
                                     env=clean_environment, timeout=15, check=False)
            assert startup.returncode == 0, f"fresh-config shell startup/exit failed: {startup.returncode}"
            checks.append({"name": "absent_default_config_and_cmd_exit", "passed": True})
            config = directory / "appearance.conf"
            config.write_text("font = Consolas:size=12\nwindow-padding = 0\n"
                              "background = #101820\ncursor-blink = false\n"
                              "background-opacity = 100\n", encoding="utf-8")
            command = [str(binary.resolve()), "--config", str(config), "--geometry", "20x10",
                       "-e", sys.executable, str(Path(__file__).resolve()), "--child", str(directory)]
            process = subprocess.Popen(command)
            report["ztty_process"] = process_architecture(process.pid)
            assert report["ztty_process"] == interpreter, "ztty must run natively on the test machine"
            checks.append({"name": "native_ztty_process_architecture", "passed": True})

            def find_window():
                matches = []

                @enum_proc
                def visit(window, _):
                    pid = wintypes.DWORD()
                    user.GetWindowThreadProcessId(window, ctypes.byref(pid))
                    if pid.value == process.pid:
                        matches.append(window)
                    return True

                user.EnumWindows(visit, 0)
                return matches[0] if matches else None

            hwnd = wait_for(find_window, "native ztty window")
            place_owned_window()
            user.SetForegroundWindow(hwnd)
            wait_for(lambda: (directory / "ready").exists(), "Unicode command arguments and ConPTY child")
            checks.append({"name": "unicode_arguments_and_native_child", "passed": True})
            rect = wintypes.RECT()
            assert user.GetClientRect(hwnd, ctypes.byref(rect))
            cw, ch = rect.right // 20, rect.bottom // 10
            assert cw > 0 and ch > 0
            report["client_pixels"] = [rect.right, rect.bottom]
            report["cell_pixels"] = [cw, ch]
            def record_resize(stage):
                state = {"monotonic_seconds": time.monotonic()}
                try:
                    state.update(window_geometry([(19.5 * cw, 1.5 * ch), (15.5 * cw, 1.5 * ch)]))
                    state["screenshot"] = capture_window(
                        user, gdi, hwnd, process.pid,
                        output.with_name(output.stem + f".resize-{stage}.png"))
                except Exception as error:
                    state["diagnostic_error"] = repr(error)
                report.setdefault("resize_stages", {})[stage] = state

            sequence = 0

            def send(data):
                nonlocal sequence
                path = directory / f"command-{sequence}.bin"
                stage = path.with_suffix(".tmp")
                stage.write_bytes(data)
                stage.replace(path)
                wait_for(lambda: (directory / f"done-{sequence}").exists(), "PTY output write")
                sequence += 1

            def pixel_at(x, y):
                dc = user.GetDC(hwnd)
                assert dc
                try:
                    screen = screen_point(x, y)
                    hit = user.WindowFromPoint(screen)
                    root_window = user.GetAncestor(hit, 2) if hit else None
                    if (root_window != hwnd or window_pid(hit) != process.pid or
                            not gdi.PtVisible(dc, int(x), int(y))):
                        raise OSError(f"sample ({int(x)}, {int(y)}) is clipped or covered by another window")
                    rgb = int(gdi.GetPixel(dc, int(x), int(y)))
                    if rgb == 0xFFFFFFFF:
                        raise AssertionError("GetPixel failed")
                    return [rgb & 255, (rgb >> 8) & 255, (rgb >> 16) & 255]
                finally:
                    user.ReleaseDC(hwnd, dc)

            def pixel(col, row):
                return pixel_at(col * cw, row * ch)

            def expect_pixels(name, samples):
                actual = []
                report["active_check"] = name

                def matches():
                    nonlocal actual
                    actual = [pixel_at(x, y) for x, y, _ in samples]
                    return all(all(abs(a - b) <= 1 for a, b in zip(rgb, expected))
                               for rgb, (_, _, expected) in zip(actual, samples))

                try:
                    wait_for(matches, name)
                except AssertionError as error:
                    raise AssertionError(f"{name}: expected {samples}; observed {actual}") from error
                checks.append({"name": name, "passed": True, "pixels": [
                    {"x": x, "y": y, "rgb": rgb} for (x, y, _), rgb in zip(samples, actual)]})

            def expect(name, col, row, expected):
                actual = None

                def matches():
                    nonlocal actual
                    actual = pixel(col, row)
                    return all(abs(a - b) <= 1 for a, b in zip(actual, expected))

                try:
                    wait_for(matches, name)
                except AssertionError as error:
                    failure = {"name": name, "expected": expected, "actual": actual,
                               "client_sample": [int(col * cw), int(row * ch)]}
                    try:
                        failure["window"] = window_geometry([(col * cw, row * ch)])
                    except Exception as diagnostic_error:
                        failure["diagnostic_error"] = repr(diagnostic_error)
                    report["failed_pixel"] = failure
                    raise AssertionError(f"{name}: expected {expected}, last RGB {actual}; {error}") from error
                checks.append({"name": name, "passed": True, "pixel": actual})

            rgba = bytes([255, 0, 0, 255, 0, 255, 0, 128,
                          0, 0, 255, 255, 255, 255, 255, 0])
            send(kitty("a=q,t=d,f=24,s=1,v=1,i=9847", bytes([1, 2, 3])))
            wait_for(lambda: (directory / "keyboard.bin").exists()
                     and b"i=9847;OK" in (directory / "keyboard.bin").read_bytes(),
                     "graphics capability response reaches the ConPTY child")
            checks.append({"name": "capability_query_roundtrip", "passed": True})
            send(b"\x1b[3;2H" + kitty("a=T,f=32,s=2,v=2,i=41,c=4,r=2,C=1,q=2", rgba))
            expect("opaque_red", 1.5, 2.5, [255, 0, 0])
            expect("half_alpha_green", 3.5, 2.5, [8, 140, 16])
            expect("fully_transparent", 3.5, 3.5, [16, 24, 32])
            send(b"\x1b[10;1H\n")
            expect("image_scrolls_with_content", 1.5, 1.5, [255, 0, 0])
            expect("old_image_position_repainted", 1.5, 3.5, [16, 24, 32])
            send(b"\x1b[?1049h\x1b[?25l")
            expect("alternate_hides_primary_image", 1.5, 1.5, [16, 24, 32])
            send(b"\x1b[?1049l\x1b[?25l")
            expect("primary_image_restored", 1.5, 1.5, [255, 0, 0])
            send(kitty("a=d,d=I,i=41,q=2"))
            expect("deletion_repaints_image", 1.5, 1.5, [16, 24, 32])
            send(b"\x1b[3;2H" + kitty("a=T,f=100,i=46,c=4,r=2,C=1,q=2", png_rgba(2, 2, rgba)))
            expect("png_decode_and_alpha", 3.5, 2.5, [8, 140, 16])
            send(kitty("a=d,d=I,i=46,q=2"))
            send(b"\x1b[3;2H" + kitty("a=T,f=24,s=1,v=1,i=47,c=2,r=1,C=1,q=2", bytes([12, 34, 56])))
            expect("rgb24_decode", 1.5, 2.5, [12, 34, 56])
            send(kitty("a=d,d=I,i=47,q=2"))
            send(b"\x1b[2;20H" + kitty("a=T,f=32,s=2,v=2,i=42,c=4,r=2,C=1,q=2", rgba))
            expect("right_edge_clips_image", 19.5, 1.5, [255, 0, 0])
            record_resize("before")
            outer = wintypes.RECT()
            assert user.GetWindowRect(hwnd, ctypes.byref(outer))
            old_width, old_height = outer.right - outer.left, outer.bottom - outer.top
            assert user.SetWindowPos(hwnd, None, 0, 0, old_width - 4 * cw, old_height, 0x16)

            def resized(columns):
                current = wintypes.RECT()
                if not user.GetClientRect(hwnd, ctypes.byref(current)):
                    raise OSError("cannot read client geometry after resize")
                return current.right == columns * cw and current.bottom == 10 * ch

            try:
                wait_for(lambda: resized(16), "native client resize")
                expect("resize_clips_without_ghosts", 15.5, 1.5, [16, 24, 32])
            finally:
                record_resize("shrunk")
            assert user.SetWindowPos(hwnd, None, 0, 0, old_width, old_height, 0x16)
            try:
                wait_for(lambda: resized(20), "native client restore to original geometry")
                expect("resize_restores_visible_image", 19.5, 1.5, [255, 0, 0])
            finally:
                record_resize("restored")
            # Sixel must traverse the actual ConPTY and draw native pixels.
            # A child "done" file only acknowledges its write; each vector also
            # waits for pixels read from our HWND rather than assuming a delay.
            width, height = 48, 24
            x0, y0 = cw, 2 * ch
            background = [16, 24, 32]
            rgb = [1 + x // 16 for _ in range(height) for x in range(width)]
            image = sixel(width, height, rgb, {1: (100, 0, 0), 2: (0, 100, 0), 3: (0, 0, 100)})
            position = b"\x1b[3;2H"
            send(b"\x1b[0m\x1b[2J\x1b[H" + position + image)
            expect_pixels("sixel_rgb_native_pixels", [
                (x0 + 8, y0 + 8, [255, 0, 0]),
                (x0 + 24, y0 + 8, [0, 255, 0]),
                (x0 + 40, y0 + 16, [0, 0, 255])])

            green = sixel(width, height, [2] * (width * height), {2: (0, 100, 0)})
            holes = [1 if x < width // 2 else 0 for _ in range(height) for x in range(width)]
            transparent = sixel(width, height, holes, {1: (100, 0, 0)})
            send(b"\x1b[2J" + position + green + position + transparent)
            expect_pixels("sixel_transparent_preserves_underlying_image", [
                (x0 + 8, y0 + 8, [255, 0, 0]), (x0 + 40, y0 + 8, [0, 255, 0])])

            opaque = sixel(width, height, holes, {1: (100, 0, 0)}, transparent=False)
            send(b"\x1b[48;2;16;24;32m" + position + opaque)
            expect_pixels("sixel_opaque_uses_current_background", [
                (x0 + 8, y0 + 8, [255, 0, 0]), (x0 + 40, y0 + 8, background)])

            send(b"\x1b[2J\x1b[H")
            blank_samples = [(x0 + x, y0 + y, background)
                             for x, y in ((8, 8), (24, 8), (40, 16))]
            expect_pixels("sixel_erase_display_repaints", blank_samples)

            # No ST is sent for this image. CAN must discard it, and the child
            # must still receive both a cursor response and real window input.
            send(position + image[:-2])
            send(b"\x18\x1b[7;9H\x1b[6n")
            wait_for(lambda: (directory / "keyboard.bin").exists()
                     and b"\x1b[7;9R" in (directory / "keyboard.bin").read_bytes(),
                     "cursor response after cancelled Sixel")
            cancelled_marker = b"ztty-sixel-cancel-43f2"
            for byte in cancelled_marker:
                user.SendMessageW(hwnd, 0x0102, byte, 1)
            user.SendMessageW(hwnd, 0x0102, 13, 1)
            wait_for(lambda: cancelled_marker in (directory / "keyboard.bin").read_bytes(),
                     "native keyboard input after cancelled Sixel")
            expect_pixels("sixel_cancel_and_input_recovery", blank_samples)

            send(b'\x1bP7;1q"1;1;4294967295;4294967295#1!99~\x1b\\'
                 + position + image + b"\x1b[9;3H\x1b[6n")
            wait_for(lambda: b"\x1b[9;3R" in (directory / "keyboard.bin").read_bytes(),
                     "cursor response after oversized Sixel and a valid image")
            expect_pixels("sixel_recovers_after_oversized_raster", [
                (x0 + 8, y0 + 8, [255, 0, 0]),
                (x0 + 24, y0 + 8, [0, 255, 0]),
                (x0 + 40, y0 + 16, [0, 0, 255])])
            report["sixel_screenshot"] = capture_window(
                user, gdi, hwnd, process.pid, output.with_name(output.stem + ".sixel.png"))
            report["active_check"] = "keyboard_recovers_after_malformed_graphics"

            send(kitty("a=d,d=A,q=2") + kitty("a=T,f=100,i=43,q=2", b"broken PNG")
                 + kitty("a=T,f=32,s=4294967295,v=4294967295,i=44,q=2", b"bad")
                 + kitty("a=t,f=32,s=2,v=2,i=45,m=1,q=2", rgba[:3])
                 + b"\x18\x1b[HRECOVERED")
            keyboard_marker = b"ztty-native-key-7a2f"
            for byte in keyboard_marker:
                user.SendMessageW(hwnd, 0x0102, byte, 1)
            user.SendMessageW(hwnd, 0x0102, 13, 1)
            wait_for(lambda: (directory / "keyboard.bin").exists()
                     and keyboard_marker in (directory / "keyboard.bin").read_bytes(),
                     "keyboard input after malformed and incomplete images")
            checks.append({"name": "keyboard_recovers_after_malformed_graphics", "passed": True})
            assert all("screenshot" in report.get("resize_stages", {}).get(stage, {})
                       for stage in ("before", "shrunk", "restored")), "resize evidence capture failed"
            assert process.poll() is None, "ztty exited unexpectedly"
            report["passed"] = True
    except BaseException as error:
        report["error"] = str(error)
        raise
    finally:
        output.parent.mkdir(parents=True, exist_ok=True)
        if hwnd and process:
            try:
                report["screenshot"] = capture_window(user, gdi, hwnd, process.pid, output.with_suffix(".png"))
                report["pixel_readback"] = "GetPixel checks and owned client DC BitBlt/GetDIBits PNG capture"
            except Exception as error:
                report["screenshot_error"] = repr(error)
                report["passed"] = False
        if hwnd:
            user.PostMessageW(hwnd, 0x0010, 0, 0)
        if process:
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait(timeout=5)
                report["shutdown_timeout"] = True
                report["passed"] = False
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(report, indent=2))
    return report["passed"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/ztty.exe"))
    parser.add_argument("--output", type=Path, default=Path("artifacts/windows-native.json"))
    parser.add_argument("--arch", choices=["x64", "arm64"],
                        help="require this native OS, Python and executable architecture")
    parser.add_argument("--child")
    args = parser.parse_args()
    if sys.platform != "win32":
        parser.error("this acceptance gate must run on native Windows")
    if args.child:
        child(args.child)
    else:
        return 0 if run(args.binary, args.output, args.arch) else 1


if __name__ == "__main__":
    raise SystemExit(main())
