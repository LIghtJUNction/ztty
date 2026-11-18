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


def configure_nc_rendering_diagnostic(user, hwnd, process, report):
    """Opt-in control for this owned HWND; never part of release acceptance."""
    if os.environ.get("ZTTY_NATIVE_NC_RENDERING_DIAGNOSTICS") != "1":
        return
    record = report["nc_rendering_diagnostic"] = {
        "diagnostic_only": True, "requested_policy": 1, "attribute": 2,
        "hwnd": int(hwnd), "pid": process.pid}

    def require_owned():
        owner = wintypes.DWORD()
        user.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        assert process.poll() is None and owner.value == process.pid and user.IsWindow(hwnd), \
            "NC diagnostic window ownership changed"

    require_owned()
    dwm = ctypes.WinDLL("dwmapi", use_last_error=True)
    dwm.DwmGetWindowAttribute.argtypes = [wintypes.HWND, wintypes.DWORD,
                                        ctypes.c_void_p, wintypes.DWORD]
    dwm.DwmGetWindowAttribute.restype = ctypes.c_int32
    dwm.DwmSetWindowAttribute.argtypes = [wintypes.HWND, wintypes.DWORD,
                                        ctypes.c_void_p, wintypes.DWORD]
    dwm.DwmSetWindowAttribute.restype = ctypes.c_int32
    before, after, policy = wintypes.BOOL(), wintypes.BOOL(), ctypes.c_int32(1)
    before_status = dwm.DwmGetWindowAttribute(hwnd, 1, ctypes.byref(before), ctypes.sizeof(before))
    record.update(before_hresult=before_status, before_enabled=bool(before.value))
    assert before_status == 0 and before.value, "NC control requires initially enabled rendering"
    require_owned()
    set_status = dwm.DwmSetWindowAttribute(hwnd, 2, ctypes.byref(policy), ctypes.sizeof(policy))
    record["set_hresult"] = set_status
    assert set_status == 0, "disabling owned-window NC rendering failed"
    after_status = dwm.DwmGetWindowAttribute(hwnd, 1, ctypes.byref(after), ctypes.sizeof(after))
    record.update(after_hresult=after_status, after_enabled=bool(after.value))
    require_owned()
    assert after_status == 0 and not after.value, "NC rendering remained enabled"


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


def child(directory, stdio_only=False):
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.GetStdHandle.restype = wintypes.HANDLE
    kernel.GetStdHandle.argtypes = [wintypes.DWORD]
    kernel.SetConsoleMode.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel.GetConsoleMode.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
    kernel.GetConsoleMode.restype = wintypes.BOOL
    kernel.GetFileType.argtypes = [wintypes.HANDLE]
    kernel.GetFileType.restype = wintypes.DWORD
    kernel.ReadFile.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.DWORD,
                               ctypes.POINTER(wintypes.DWORD), ctypes.c_void_p]
    stdin = kernel.GetStdHandle(-10 & 0xFFFFFFFF)
    stdout = kernel.GetStdHandle(-11 & 0xFFFFFFFF)
    if stdio_only:
        handles = {}
        for name, identifier in (("stdin", -10), ("stdout", -11), ("stderr", -12)):
            handle = kernel.GetStdHandle(identifier & 0xFFFFFFFF)
            mode = wintypes.DWORD()
            valid = bool(kernel.GetConsoleMode(handle, ctypes.byref(mode)))
            error = 0 if valid else ctypes.get_last_error()
            handles[name] = {"handle": int(handle or 0), "console": valid,
                             "console_mode": mode.value if valid else None,
                             "file_type": int(kernel.GetFileType(handle)), "error": error}
        report = {"pid": os.getpid(), "parent_pid": os.getppid(), "handles": handles}
        path = Path(directory) / "stdio.json"
        path.write_text(json.dumps(report, indent=2), encoding="utf-8")
        assert all(value["console"] and value["file_type"] == 2 for value in handles.values()), report
        size = os.get_terminal_size(sys.stdout.fileno())
        report["columns"], report["rows"] = size.columns, size.lines
        path.write_text(json.dumps(report, indent=2), encoding="utf-8")
        sys.stdout.buffer.write(b"ztty-conpty-stdout-only\n")
        sys.stdout.buffer.flush()
        sys.stderr.buffer.write(b"ztty-conpty-stderr-only\n")
        sys.stderr.buffer.flush()
        return
    if not kernel.SetConsoleMode(stdin, 0x200):
        raise ctypes.WinError(ctypes.get_last_error())
    if not kernel.SetConsoleMode(stdout, 0x1 | 0x4 | 0x8):
        raise ctypes.WinError(ctypes.get_last_error())
    directory = Path(directory)
    (directory / "startup.json").write_text(
        json.dumps({"cwd": os.getcwd(), "pid": os.getpid(), "parent_pid": os.getppid()}), encoding="utf-8")
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
    size_before = None
    size_sequence = 0
    record_geometry = os.environ.get("ZTTY_NATIVE_GEOMETRY") == "1"
    while True:
        size = os.get_terminal_size(sys.stdout.fileno()) if record_geometry else None
        if record_geometry and size != size_before:
            state = {"pid": os.getpid(), "sequence": size_sequence,
                     "columns": size.columns, "rows": size.lines,
                     "monotonic_seconds": time.monotonic()}
            target = directory / f"geometry-{size_sequence}.json"
            pending = target.with_suffix(".tmp")
            pending.write_text(json.dumps(state), encoding="utf-8")
            pending.replace(target)
            size_before = size
            size_sequence += 1
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


class DesktopBlocked(RuntimeError):
    """The native window cannot be sampled on this runner's visible desktop."""


def process_image_name(pid):
    """Read only the executable basename; never inspect process arguments."""
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.QueryFullProcessImageNameW.argtypes = [
        wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD)]
    kernel.QueryFullProcessImageNameW.restype = wintypes.BOOL
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = kernel.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not handle:
        return {"error": ctypes.get_last_error()}
    try:
        name = ctypes.create_unicode_buffer(32768)
        length = wintypes.DWORD(len(name))
        if not kernel.QueryFullProcessImageNameW(handle, 0, name, ctypes.byref(length)):
            return {"error": ctypes.get_last_error()}
        return {"name": Path(name.value).name}
    finally:
        kernel.CloseHandle(handle)


def desktop_state(user):
    """Inspect names and access errors without switching or unlocking desktops."""
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.GetCurrentThreadId.argtypes = []
    kernel.GetCurrentThreadId.restype = wintypes.DWORD
    user.GetThreadDesktop.argtypes = [wintypes.DWORD]
    user.GetThreadDesktop.restype = wintypes.HANDLE
    user.OpenInputDesktop.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    user.OpenInputDesktop.restype = wintypes.HANDLE
    user.GetUserObjectInformationW.argtypes = [
        wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD,
        ctypes.POINTER(wintypes.DWORD)]
    user.GetUserObjectInformationW.restype = wintypes.BOOL
    user.CloseDesktop.argtypes = [wintypes.HANDLE]

    def name(handle):
        if not handle:
            return {"error": ctypes.get_last_error()}
        value = ctypes.create_unicode_buffer(256)
        needed = wintypes.DWORD()
        if not user.GetUserObjectInformationW(
                handle, 2, value, ctypes.sizeof(value), ctypes.byref(needed)):  # UOI_NAME
            return {"error": ctypes.get_last_error()}
        return {"name": value.value}

    current = name(user.GetThreadDesktop(kernel.GetCurrentThreadId()))
    handle = user.OpenInputDesktop(0, False, 1)  # DESKTOP_READOBJECTS only
    try:
        return {"thread": current, "input": name(handle)}
    finally:
        if handle:
            user.CloseDesktop(handle)


def remove_owned_directory(temporary_directory, cleanup):
    """Retry only Windows sharing/lock violations within one fixed deadline."""
    started = time.monotonic()
    deadline = started + 2.0
    removal = {"attempts": 0, "elapsed_seconds": 0.0, "retry_timeout_seconds": 2.0}
    cleanup["directory_removal"] = removal
    last_error = None
    try:
        while True:
            # A delayed wake must not start another attempt after the budget.
            if removal["attempts"] and time.monotonic() >= deadline:
                cleanup["directory_error"] = repr(last_error)
                return
            removal["attempts"] += 1
            try:
                temporary_directory.cleanup()
                cleanup["temporary_directory_removed"] = not Path(temporary_directory.name).exists()
                if not cleanup["temporary_directory_removed"]:
                    cleanup["directory_error"] = "temporary directory still exists after cleanup"
                return
            except Exception as error:
                last_error = error
                removal.setdefault("first_error", {
                    "type": type(error).__name__, "repr": repr(error),
                    "errno": getattr(error, "errno", None),
                    "winerror": getattr(error, "winerror", None),
                })
                remaining = deadline - time.monotonic()
                if (sys.platform != "win32" or not isinstance(error, OSError)
                        or getattr(error, "winerror", None) not in (32, 33) or remaining <= 0):
                    cleanup["directory_error"] = repr(error)
                    return
                time.sleep(min(0.05, remaining))
    finally:
        removal["elapsed_seconds"] = time.monotonic() - started


def cleanup_owned(process, close_window, temporary_directory, report):
    """Finish owned processes before deleting paths, retaining the first error."""
    cleanup = {"process_exited": process is None, "temporary_directory_removed": temporary_directory is None}
    report["cleanup"] = cleanup
    if process:
        try:
            close_window()
        except Exception as error:
            cleanup["window_close_error"] = repr(error)
            report["passed"] = False
        try:
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait(timeout=5)
                report["shutdown_timeout"] = True
                report["passed"] = False
            cleanup["process_exited"] = process.poll() is not None
            cleanup["returncode"] = process.returncode
        except Exception as error:
            cleanup["process_error"] = repr(error)
            report["passed"] = False
    # Windows may briefly retain a sharing lock after the owned process exits.
    # Never delete its directory until that exit has actually been observed.
    if temporary_directory:
        if cleanup["process_exited"]:
            try:
                remove_owned_directory(temporary_directory, cleanup)
            except Exception as error:
                cleanup["directory_error"] = repr(error)
                report["passed"] = False
        else:
            cleanup["directory_removal"] = {
                "attempts": 0, "elapsed_seconds": 0.0, "retry_timeout_seconds": 2.0,
                "skipped": "owned process exit was not confirmed",
            }
    if not cleanup["process_exited"] or not cleanup["temporary_directory_removed"]:
        report["passed"] = False


def run(binary, output, expected_arch=None, trace_runtime=False, staged_resize=False):
    user = ctypes.WinDLL("user32", use_last_error=True)
    gdi = ctypes.WinDLL("gdi32", use_last_error=True)
    enum_proc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user.EnumWindows.argtypes = [enum_proc, wintypes.LPARAM]
    user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    user.GetUpdateRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT), wintypes.BOOL]
    user.GetUpdateRect.restype = wintypes.BOOL
    user.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    user.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int,
                                 ctypes.c_int, ctypes.c_int, wintypes.UINT]
    user.GetDC.argtypes = [wintypes.HWND]
    user.GetDC.restype = wintypes.HDC
    user.ReleaseDC.argtypes = [wintypes.HWND, wintypes.HDC]
    user.SendMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    user.SendMessageW.restype = wintypes.LPARAM
    user.SendMessageTimeoutW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM,
        wintypes.LPARAM, wintypes.UINT, wintypes.UINT, ctypes.POINTER(ctypes.c_size_t)]
    user.SendMessageTimeoutW.restype = ctypes.c_ssize_t
    user.InvalidateRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT), wintypes.BOOL]
    user.InvalidateRect.restype = wintypes.BOOL
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
    user.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    user.GetClassNameW.restype = ctypes.c_int
    user.GetWindowLongPtrW.argtypes = [wintypes.HWND, ctypes.c_int]
    user.GetWindowLongPtrW.restype = ctypes.c_ssize_t
    gdi.GetClipBox.argtypes = [wintypes.HDC, ctypes.POINTER(wintypes.RECT)]
    gdi.GetClipBox.restype = ctypes.c_int
    gdi.GetDCOrgEx.argtypes = [wintypes.HDC, ctypes.POINTER(NativePoint)]
    gdi.GetDCOrgEx.restype = wintypes.BOOL
    gdi.PtVisible.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int]
    gdi.PtVisible.restype = wintypes.BOOL
    user.SetProcessDPIAware()
    checks = []
    process = None
    hwnd = None
    temporary_directory = None
    runtime_stream = None
    runtime_path = output.with_name(output.stem + ".runtime.log")
    report = {"platform": sys.platform, "binary": str(binary.resolve()),
              "transport": "real ConPTY", "renderer": "Win32 retained DIB",
              "runtime_trace_enabled": trace_runtime,
              "paint_diagnostics_enabled": os.environ.get("ZTTY_NATIVE_PAINT_DIAGNOSTICS") == "1",
              "nc_rendering_diagnostics_enabled": os.environ.get("ZTTY_NATIVE_NC_RENDERING_DIAGNOSTICS") == "1",
              "resize_path": "staged" if staged_resize else "rapid",
              "rapid_resize_repetitions": {"required": 10, "completed": 0, "passed": False, "iterations": []},
              "back_to_back_resize_repetitions": {"required": 10, "completed": 0, "passed": False, "iterations": []},
              "checks": checks, "passed": False, "status": "running"}
    def window_pid(window):
        pid = wintypes.DWORD()
        if window:
            user.GetWindowThreadProcessId(window, ctypes.byref(pid))
        return pid.value

    def window_identity(window):
        if not window:
            return {"hwnd": 0, "pid": 0}
        rect = wintypes.RECT()
        bounds = rect_values(rect) if user.GetWindowRect(window, ctypes.byref(rect)) else None
        name = ctypes.create_unicode_buffer(256)
        count = user.GetClassNameW(window, name, len(name))
        style = int(user.GetWindowLongPtrW(window, -20))  # GWL_EXSTYLE
        return {"hwnd": int(window), "pid": window_pid(window), "rect": bounds,
                "class": name.value if count else None,
                "process_image": process_image_name(window_pid(window)),
                "topmost": bool(style & 8), "visible": bool(user.IsWindowVisible(window)),
                "root": int(user.GetAncestor(window, 2) or 0)}

    def screen_point(x, y):
        point = NativePoint(int(x), int(y))
        if not user.ClientToScreen(hwnd, ctypes.byref(point)):
            raise OSError("ClientToScreen failed")
        return point

    def dwm_state():
        state = {}
        try:
            dwm = ctypes.WinDLL("dwmapi", use_last_error=True)
            dwm.DwmGetWindowAttribute.argtypes = [wintypes.HWND, wintypes.DWORD,
                                                ctypes.c_void_p, wintypes.DWORD]
            dwm.DwmGetWindowAttribute.restype = ctypes.c_int32
            dwm.DwmIsCompositionEnabled.argtypes = [ctypes.POINTER(wintypes.BOOL)]
            dwm.DwmIsCompositionEnabled.restype = ctypes.c_int32
            enabled = wintypes.BOOL()
            status = dwm.DwmIsCompositionEnabled(ctypes.byref(enabled))
            state["composition"] = {"hresult": status, "reported_enabled": bool(enabled.value)}
            for key, attribute, value in (("extended_frame_bounds", 9, wintypes.RECT()),
                                          ("cloaked", 14, wintypes.DWORD()),
                                          ("nonclient_rendering", 1, wintypes.BOOL())):
                status = dwm.DwmGetWindowAttribute(hwnd, attribute, ctypes.byref(value), ctypes.sizeof(value))
                state[key] = {"hresult": status, "value":
                              rect_values(value) if isinstance(value, wintypes.RECT) else value.value}
        except Exception as error:
            state["error"] = repr(error)
        return state

    def window_geometry(samples=(), diagnostics=False):
        diagnostics = diagnostics or trace_runtime
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
        screen_dc = user.GetDC(None) if diagnostics else None
        client_origin, screen_origin = NativePoint(), NativePoint()
        screen_clip = wintypes.RECT()
        origins = {}
        try:
            clip_type = gdi.GetClipBox(dc, ctypes.byref(clip))
            if diagnostics and gdi.GetDCOrgEx(dc, ctypes.byref(client_origin)):
                origins["client_dc_origin"] = [client_origin.x, client_origin.y]
            if screen_dc:
                if gdi.GetDCOrgEx(screen_dc, ctypes.byref(screen_origin)):
                    origins["screen_dc_origin"] = [screen_origin.x, screen_origin.y]
                origins["screen_clip_type"] = gdi.GetClipBox(screen_dc, ctypes.byref(screen_clip))
                origins["screen_clip_box"] = rect_values(screen_clip)
            points = []
            for x, y in samples:
                screen = screen_point(x, y)
                hit = user.WindowFromPoint(screen)
                root_window = user.GetAncestor(hit, 2) if hit else None
                color = int(gdi.GetPixel(dc, int(x), int(y)))
                owned = root_window == hwnd and window_pid(hit) == process.pid
                visible = bool(gdi.PtVisible(dc, int(x), int(y)))
                screen_color = int(gdi.GetPixel(screen_dc, screen.x, screen.y)) if screen_dc and owned and visible else 0xFFFFFFFF
                points.append({"client": [int(x), int(y)], "screen": [screen.x, screen.y],
                               "point_visible": visible,
                               "window_from_point": int(hit or 0), "root_window": int(root_window or 0),
                               "window_from_point_pid": window_pid(hit),
                               "hit_window": window_identity(hit),
                               "owned_window_at_point": owned,
                               "screen_rgb": None if screen_color == 0xFFFFFFFF else
                               [screen_color & 255, (screen_color >> 8) & 255, (screen_color >> 16) & 255],
                               "rgb": None if color == 0xFFFFFFFF else
                               [color & 255, (color >> 8) & 255, (color >> 16) & 255]})
        finally:
            if screen_dc:
                user.ReleaseDC(None, screen_dc)
            user.ReleaseDC(hwnd, dc)
        if diagnostics:
            client_after, update = wintypes.RECT(), wintypes.RECT()
            client_after_read = user.GetClientRect(hwnd, ctypes.byref(client_after))
            origins.update({"client_after_samples": rect_values(client_after) if client_after_read else None,
                            "update_pending": bool(user.GetUpdateRect(hwnd, ctypes.byref(update), False)),
                            "update_rect": rect_values(update), "dwm": dwm_state()})
        return {"client": rect_values(client), "outer": rect_values(outer),
                "client_screen": [top_left.x, top_left.y, bottom_right.x, bottom_right.y],
                "dpi": int(user.GetDpiForWindow(hwnd)), "visible": bool(user.IsWindowVisible(hwnd)),
                "minimized": bool(user.IsIconic(hwnd)), "window_pid": window_pid(hwnd),
                "foreground_pid": window_pid(user.GetForegroundWindow()),
                "foreground_window": window_identity(user.GetForegroundWindow()),
                "owned_window": window_identity(hwnd),
                "monitor": {"rect": rect_values(info.monitor), "work": rect_values(info.work),
                            "primary": bool(info.flags & 1)},
                "clip_type": clip_type, "clip_box": rect_values(clip), "samples": points,
                **origins}

    def place_owned_window():
        before = window_geometry()
        if before["window_pid"] != process.pid:
            raise AssertionError("native test window changed ownership")
        left, top, right, bottom = before["monitor"]["work"]
        outer = before["outer"]
        width, height = outer[2] - outer[0], outer[3] - outer[1]
        if width > right - left or height > bottom - top:
            raise AssertionError("test window does not fit the monitor work area")
        client = before["client"]
        cw, ch = client[2], client[3]
        points = [(1, 1), (cw - 2, 1), (1, ch - 2), (cw - 2, ch - 2), (cw // 2, ch // 2)]
        xs = [left + min(32, (right - left - width) // 2),
              left + (right - left - width) // 2, right - width]
        ys = [top + min(32, (bottom - top - height) // 2),
              top + (bottom - top - height) // 2, bottom - height]
        positions = list(dict.fromkeys((x, y) for y in ys for x in xs))
        placement = {"before": before, "requested_topmost": True, "attempts": []}
        report["window_placement"] = placement

        def unobstructed():
            state = window_geometry(points)
            bounds = state["client_screen"]
            if (state["visible"] and not state["minimized"] and
                    left <= bounds[0] < bounds[2] <= right and top <= bounds[1] < bounds[3] <= bottom and
                    all(point["owned_window_at_point"] and point["point_visible"] for point in state["samples"])):
                return state
            return None

        # Keep the test window inside its own work area and in front of other
        # topmost windows. If a runner overlay cannot be covered, try another
        # location for our window; never move, hide or close the other window.
        deadline = time.monotonic() + 10
        for x, y in positions:
            if time.monotonic() >= deadline:
                break
            if not user.SetWindowPos(hwnd, ctypes.c_void_p(-1), x, y, 0, 0, 0x0001):
                raise OSError("cannot place the owned test window inside its monitor")
            user.SetForegroundWindow(hwnd)
            try:
                placed = wait_for(unobstructed, "owned window has five visible client points",
                                  timeout=min(0.5, max(0, deadline - time.monotonic())))
                placement.update({"placed": placed, "position": [x, y]})
                placement["attempts"].append({"position": [x, y], "visible": True})
                return
            except AssertionError as error:
                placement["attempts"].append({"position": [x, y], "visible": False,
                                               "diagnostic": window_geometry(points),
                                               "error": str(error)})
        raise DesktopBlocked("runner desktop has no verified visible location for the owned test window")

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
        # Only cleanup_owned may delete it, after confirming the owned exit.
        # A finalizer must not try again behind a still-running failed child.
        temporary_directory = tempfile.TemporaryDirectory(prefix="ztty native 图像 ", delete=False)
        directory = Path(temporary_directory.name)
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
        working_directory = directory / "工作 目录 🦀"
        working_directory.mkdir()
        marker = directory / "invalid-cwd-started"
        child_code = f"from pathlib import Path; Path({str(marker)!r}).write_text('unexpected')"
        for kind, invalid in (("missing", directory / "missing cwd"),
                              ("regular_file", config), ("empty", "")):
            rejected = subprocess.run(
                [str(binary.resolve()), "--config", "appearance.conf", "--cwd", str(invalid),
                 "-e", sys.executable, "-c", child_code],
                cwd=directory, env=clean_environment, capture_output=True, timeout=10, check=False)
            assert rejected.returncode == 1, (kind, rejected.returncode, rejected.stderr)
            assert b"cannot enter command working directory (OS error " in rejected.stderr
            assert not marker.exists(), "invalid cwd must not launch a child"
            checks.append({"name": f"invalid_cwd_{kind}_rejected", "passed": True,
                           "exit_code": rejected.returncode,
                           "stderr": rejected.stderr.decode("utf-8", errors="replace").strip()})
        # A copied native --version executable proves relative program
        # resolution occurs inside the requested directory.
        relative_program = working_directory / "relative-probe.exe"
        relative_program.write_bytes(binary.read_bytes())
        relative = subprocess.run(
            [str(binary.resolve()), "--cwd", str(working_directory),
             "-e", ".\\relative-probe.exe", "--version"],
            env=clean_environment, capture_output=True, timeout=15, check=False)
        assert relative.returncode == 0, (relative.returncode, relative.stderr)
        checks.append({"name": "relative_program_uses_requested_cwd", "passed": True})
        stdio_directory = directory / "redirected host stdio"
        stdio_directory.mkdir()
        stdio_host = subprocess.run(
            [str(binary.resolve()), "--config", "appearance.conf", "--geometry", "20x10",
             "-e", sys.executable, str(Path(__file__).resolve()), "--stdio-child", str(stdio_directory)],
            cwd=directory, env=clean_environment, stdin=subprocess.DEVNULL,
            capture_output=True, timeout=15, check=False)
        stdio_path = stdio_directory / "stdio.json"
        stdio_evidence = json.loads(stdio_path.read_text(encoding="utf-8")) if stdio_path.exists() else {}
        report["redirected_host_stdio"] = {"returncode": stdio_host.returncode,
                                           "child": stdio_evidence,
                                           "stdout_bytes": len(stdio_host.stdout),
                                           "stderr": stdio_host.stderr.decode("utf-8", errors="replace")}
        assert stdio_host.returncode == 0, report["redirected_host_stdio"]
        handles = stdio_evidence.get("handles", {})
        assert set(handles) == {"stdin", "stdout", "stderr"}, stdio_evidence
        assert all(value["console"] and value["file_type"] == 2 for value in handles.values()), stdio_evidence
        assert (stdio_evidence["columns"], stdio_evidence["rows"]) == (20, 10), stdio_evidence
        assert not stdio_host.stdout and not stdio_host.stderr, report["redirected_host_stdio"]
        checks.append({"name": "redirected_host_uses_conpty_standard_handles", "passed": True})
        command = [str(binary.resolve()), "--config", "appearance.conf",
                   "--cwd", str(working_directory), "--geometry", "20x10",
                   "-e", sys.executable, str(Path(__file__).resolve()), "--child", str(directory)]
        if trace_runtime:
            runtime_path.parent.mkdir(parents=True, exist_ok=True)
            runtime_stream = runtime_path.open("wb")
        environment = dict(os.environ, ZTTY_NATIVE_TRACE="1" if trace_runtime else "0",
                           ZTTY_NATIVE_GEOMETRY="1" if trace_runtime or staged_resize else "0",
                           ZTTY_NATIVE_SNAPSHOT=str(directory / "failure-state.json"))
        process = subprocess.Popen(command, cwd=directory, env=environment, stderr=runtime_stream)
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
        wait_for(lambda: (directory / "ready").exists(), "Unicode command arguments and ConPTY child")
        checks.append({"name": "unicode_arguments_and_native_child", "passed": True})
        startup_state = json.loads((directory / "startup.json").read_text(encoding="utf-8"))
        assert Path(startup_state["cwd"]).samefile(working_directory), startup_state
        assert Path.cwd() != working_directory, "the harness cwd must remain unchanged"
        report["startup_directory"] = {"requested": str(working_directory),
                                       "actual": startup_state["cwd"],
                                       "relative_config": "appearance.conf"}
        checks.append({"name": "unicode_cwd_and_relative_config", "passed": True})
        configure_nc_rendering_diagnostic(user, hwnd, process, report)
        rect = wintypes.RECT()
        assert user.GetClientRect(hwnd, ctypes.byref(rect))
        cw, ch = rect.right // 20, rect.bottom // 10
        assert cw > 0 and ch > 0
        report["client_pixels"] = [rect.right, rect.bottom]
        report["cell_pixels"] = [cw, ch]

        def client_sample(col, row):
            return [int(col * cw), int(row * ch)]

        if report["paint_diagnostics_enabled"]:
            x, y = client_sample(19.5, 1.5)
            assert 0 <= x <= 65535 and 0 <= y <= 65535
            assert process.poll() is None and window_pid(hwnd) == process.pid, "paint sample window ownership changed"
            result = ctypes.c_size_t()
            ctypes.set_last_error(0)
            delivered = user.SendMessageTimeoutW(hwnd, 0x85A2, 0x5A545459, x | (y << 16),
                                                  0x0003, 2000, ctypes.byref(result))
            report["paint_sample_configuration"] = {"client_sample": [x, y],
                "delivered": bool(delivered), "result": result.value,
                "last_error": ctypes.get_last_error(), "timeout_ms": 2000}
            assert delivered and result.value == 1, "paint sample configuration was unavailable or timed out"

        def record_resize(stage, stages=None, prefix="resize"):
            if stages is None:
                stages = report.setdefault("resize_stages", {})
            state = stages.setdefault(stage, {})
            state["monotonic_seconds"] = time.monotonic()
            try:
                state.update(window_geometry([client_sample(19.5, 1.5), client_sample(15.5, 1.5)]))
                state["screenshot"] = capture_window(
                    user, gdi, hwnd, process.pid,
                    output.with_name(output.stem + f".{prefix}-{stage}.png"))
            except Exception as error:
                state["diagnostic_error"] = repr(error)

        sequence = 0

        def send(data):
            nonlocal sequence
            path = directory / f"command-{sequence}.bin"
            stage = path.with_suffix(".tmp")
            stage.write_bytes(data)
            stage.replace(path)
            wait_for(lambda: (directory / f"done-{sequence}").exists(), "PTY output write")
            sequence += 1

        def resize_ack(stage, columns, query_id):
            state = report.setdefault("resize_stages", {}).setdefault(stage, {})
            def child_size_matches():
                paths = list(directory.glob("geometry-*.json"))
                if not paths:
                    return False
                path = max(paths, key=lambda value: int(value.stem.split("-")[1]))
                actual = json.loads(path.read_text(encoding="utf-8"))
                state["child_geometry"] = actual
                return (actual["pid"] == startup_state["pid"] and
                        actual["columns"] == columns and actual["rows"] == 10)
            wait_for(child_size_matches, f"actual ConPTY child resize to {columns} columns")
            marker = f"i={query_id};OK".encode("ascii")
            send(kitty(f"a=q,t=d,f=24,s=1,v=1,i={query_id}", bytes([1, 2, 3])))
            wait_for(lambda: marker in (directory / "keyboard.bin").read_bytes(),
                     "frontend roundtrip after " + stage + " resize")
            state["frontend_roundtrip"] = {"query_id": query_id, "passed": True,
                                             "monotonic_seconds": time.monotonic()}

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
            return pixel_at(*client_sample(col, row))

        def post_failure_snapshot():
            # This path is reached only after the original pixel assertion
            # failed. Preserve its image before any message can flush/repaint.
            diagnostic = report["post_failure_snapshot"] = {"original_gate_remains_failed": True}
            try:
                x, y = report["failed_pixel"]["client_sample"]
                assert 0 <= x <= 65535 and 0 <= y <= 65535
                assert process.poll() is None and window_pid(hwnd) == process.pid, "snapshot window ownership changed"
                report["screenshot"] = capture_window(user, gdi, hwnd, process.pid, output.with_suffix(".png"))
                diagnostic["before"] = window_geometry([(x, y)], diagnostics=True)
                result = ctypes.c_size_t()
                ctypes.set_last_error(0)
                delivered = user.SendMessageTimeoutW(hwnd, 0x85A1, 0x5A545459, x | (y << 16),
                                                      0x0003, 2000, ctypes.byref(result))
                diagnostic["message"] = {"delivered": bool(delivered), "result": result.value,
                                         "last_error": ctypes.get_last_error(), "timeout_ms": 2000}
                assert delivered and result.value == 1, "failure snapshot was unavailable, busy or timed out"
                source = directory / "failure-state.json"
                assert source.stat().st_size <= 8192, "failure snapshot exceeded its bound"
                raw = source.read_bytes()
                state = json.loads(raw)
                assert (state["schema"] == 1 and state["pid"] == process.pid
                        and state["hwnd"] == int(hwnd) and state["sample"] == [x, y]), state
                saved = output.with_name(output.stem + ".post-failure.json")
                saved.write_bytes(raw)
                diagnostic["record"] = {"path": str(saved), "bytes": len(raw),
                                         "sha256": hashlib.sha256(raw).hexdigest()}
                diagnostic["state"] = state
                diagnostic["after_flush"] = window_geometry([(x, y)], diagnostics=True)
                diagnostic["after_flush_screenshot"] = capture_window(user, gdi, hwnd, process.pid,
                    output.with_name(output.stem + ".post-failure.png"))
                # A separate bounded repaint distinguishes a missing update
                # from a flush effect. It cannot change the failed assertion.
                assert process.poll() is None and window_pid(hwnd) == process.pid, "repaint window ownership changed"
                assert user.InvalidateRect(hwnd, None, False)
                update = wintypes.RECT()
                def repaint_processed():
                    assert process.poll() is None and window_pid(hwnd) == process.pid, "repaint window ownership changed"
                    return not user.GetUpdateRect(hwnd, ctypes.byref(update), False)
                wait_for(repaint_processed, "diagnostic repaint update region", timeout=2)
                diagnostic["after_repaint"] = window_geometry([(x, y)], diagnostics=True)
                diagnostic["after_repaint_screenshot"] = capture_window(user, gdi, hwnd, process.pid,
                    output.with_name(output.stem + ".post-repaint.png"))
                # Compare probe coordinates only after all original/flush/repaint
                # evidence is saved. This never changes the failed pixel result.
                alignment = diagnostic["paint_sample_alignment"] = {"failed_client_sample": [x, y]}
                for label, field in (("before_end_paint", "target_after_blit"),
                                     ("after_end_paint", "target_after_end_paint")):
                    sample = (state["before"]["last_paint"].get(field) or {}).get("sample")
                    alignment[label] = {"client_sample": sample,
                        "matches_failed_pixel": None if sample is None else sample == [x, y]}
            except Exception as error:
                diagnostic["error"] = repr(error)

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

        def expect(name, col, row, expected, record=True):
            actual = None

            def matches():
                nonlocal actual
                actual = pixel(col, row)
                return all(abs(a - b) <= 1 for a, b in zip(actual, expected))

            try:
                wait_for(matches, name)
            except AssertionError as error:
                failure = {"name": name, "expected": expected, "actual": actual,
                           "client_sample": client_sample(col, row)}
                try:
                    failure["window"] = window_geometry([client_sample(col, row)], diagnostics=True)
                except Exception as diagnostic_error:
                    failure["diagnostic_error"] = repr(diagnostic_error)
                report["failed_pixel"] = failure
                raise AssertionError(f"{name}: expected {expected}, last RGB {actual}; {error}") from error
            if record:
                checks.append({"name": name, "passed": True, "pixel": actual})
            return actual

        rgba = bytes([255, 0, 0, 255, 0, 255, 0, 128,
                      0, 0, 255, 255, 255, 255, 255, 0])
        send(kitty("a=q,t=d,f=24,s=1,v=1,i=9847", bytes([1, 2, 3])))
        wait_for(lambda: (directory / "keyboard.bin").exists()
                 and b"i=9847;OK" in (directory / "keyboard.bin").read_bytes(),
                 "graphics capability response reaches the ConPTY child")
        checks.append({"name": "capability_query_roundtrip", "passed": True})
        report["desktop"] = desktop_state(user)
        place_owned_window()
        user.SetForegroundWindow(hwnd)
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
        if staged_resize:
            resize_ack("before", 20, 9848)
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
            if staged_resize:
                resize_ack("shrunk", 16, 9849)
            expect("resize_clips_without_ghosts", 15.5, 1.5, [16, 24, 32])
        finally:
            record_resize("shrunk")
        assert user.SetWindowPos(hwnd, None, 0, 0, old_width, old_height, 0x16)
        try:
            wait_for(lambda: resized(20), "native client restore to original geometry")
            if staged_resize:
                resize_ack("restored", 20, 9850)
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
        # Preserve the original 31 checks, then independently require ten
        # repetitions of their rapid resize path. Later Sixel/recovery vectors
        # removed the earlier edge image, so recreate that exact fixture first.
        repetitions = report["rapid_resize_repetitions"]
        send(b"\x1b[0m\x1b[2J\x1b[?25l\x1b[2;20H"
             + kitty("a=T,f=32,s=2,v=2,i=42,c=4,r=2,C=1,q=2", rgba))
        expect("rapid_resize_fixture", 19.5, 1.5, [255, 0, 0], record=False)
        for number in range(1, repetitions["required"] + 1):
            iteration = {"number": number, "passed": False, "resize_stages": {}}
            repetitions["iterations"].append(iteration)
            prefix = f"rapid-{number:02d}"
            record_resize("before", iteration["resize_stages"], prefix)
            assert user.SetWindowPos(hwnd, None, 0, 0, old_width - 4 * cw, old_height, 0x16)
            try:
                wait_for(lambda: resized(16), "native client resize")
                iteration["shrunk_pixel"] = expect("resize_clips_without_ghosts", 15.5, 1.5,
                                                      [16, 24, 32], record=False)
            finally:
                record_resize("shrunk", iteration["resize_stages"], prefix)
            assert user.SetWindowPos(hwnd, None, 0, 0, old_width, old_height, 0x16)
            try:
                wait_for(lambda: resized(20), "native client restore to original geometry")
                iteration["restored_pixel"] = expect("resize_restores_visible_image", 19.5, 1.5,
                                                        [255, 0, 0], record=False)
            finally:
                record_resize("restored", iteration["resize_stages"], prefix)
            assert all("screenshot" in iteration["resize_stages"].get(stage, {})
                       for stage in ("before", "shrunk", "restored")), "rapid resize evidence capture failed"
            iteration["passed"] = True
            repetitions["completed"] += 1
        assert repetitions["completed"] == repetitions["required"] == 10
        repetitions["passed"] = True
        assert all("screenshot" in report.get("resize_stages", {}).get(stage, {})
                   for stage in ("before", "shrunk", "restored")), "resize evidence capture failed"
        # Observing the shrunk background above can wait for a repaint. This
        # separate gate makes both size requests without any observation,
        # screenshot, PTY roundtrip or wait between them. Keep the same flags.
        burst = report["back_to_back_resize_repetitions"]
        report["active_check"] = "back_to_back_resize_restores_visible_image"
        for number in range(1, burst["required"] + 1):
            iteration = {"number": number, "passed": False, "resize_stages": {}}
            burst["iterations"].append(iteration)
            try:
                shrunk = user.SetWindowPos(hwnd, None, 0, 0, old_width - 4 * cw, old_height, 0x16)
                restored = user.SetWindowPos(hwnd, None, 0, 0, old_width, old_height, 0x16)
                assert shrunk and restored, "back-to-back native resize requests failed"
                iteration["restored_pixel"] = expect("back_to_back_resize_restores_visible_image",
                                                     19.5, 1.5, [255, 0, 0], record=False)
            finally:
                record_resize("restored", iteration["resize_stages"], f"back-to-back-{number:02d}")
            assert "screenshot" in iteration["resize_stages"].get("restored", {}), "back-to-back resize evidence capture failed"
            iteration["passed"] = True
            burst["completed"] += 1
        assert burst["completed"] == burst["required"] == 10
        burst["passed"] = True
        assert process.poll() is None, "ztty exited unexpectedly"
        report["passed"] = True
    except BaseException as error:
        report["passed"] = False
        report["status"] = "blocked" if isinstance(error, DesktopBlocked) else "failed"
        report["error"] = str(error)
        if report.get("active_check") == "back_to_back_resize_restores_visible_image":
            # A failed size request/capture also retains this process's state;
            # no diagnostic rerun can replace the failed burst.
            report.setdefault("failed_pixel", {"name": "back_to_back_resize_restores_visible_image",
                "expected": [255, 0, 0], "actual": None, "client_sample": client_sample(19.5, 1.5)})
        if report.get("failed_pixel", {}).get("name") in (
                "resize_restores_visible_image", "back_to_back_resize_restores_visible_image"):
            post_failure_snapshot()
        raise
    finally:
        output.parent.mkdir(parents=True, exist_ok=True)
        if hwnd and process:
            try:
                if "screenshot" not in report:
                    report["screenshot"] = capture_window(user, gdi, hwnd, process.pid, output.with_suffix(".png"))
                visible_checks = sum("pixel" in check or "pixels" in check for check in checks)
                report["visible_pixel_checks_completed"] = visible_checks
                report["pixel_readback"] = (
                    "GetPixel assertions plus owned client DC BitBlt/GetDIBits PNG capture"
                    if visible_checks else "diagnostic owned client DC PNG only; visible pixel acceptance not completed")
            except Exception as error:
                report["screenshot_error"] = repr(error)
                report["passed"] = False
        def request_close():
            if hwnd and window_pid(hwnd) == process.pid:
                user.PostMessageW(hwnd, 0x0010, 0, 0)
        if temporary_directory and (trace_runtime or staged_resize):
            try:
                paths = sorted(directory.glob("geometry-*.json"),
                               key=lambda value: int(value.stem.split("-")[1]))
                report["child_geometry_history"] = [json.loads(path.read_text(encoding="utf-8"))
                                                    for path in paths[-64:]]
            except (OSError, ValueError) as error:
                report["child_geometry_error"] = repr(error)
        cleanup_owned(process, request_close, temporary_directory, report)
        if runtime_stream:
            runtime_stream.close()
            report["runtime_trace"] = {"path": str(runtime_path), "bytes": runtime_path.stat().st_size,
                                       "sha256": hashlib.sha256(runtime_path.read_bytes()).hexdigest()}
        report["status"] = "passed" if report["passed"] else ("blocked" if report["status"] == "blocked" else "failed")
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
    parser.add_argument("--stdio-child")
    args = parser.parse_args()
    if sys.platform != "win32":
        parser.error("this acceptance gate must run on native Windows")
    if args.stdio_child:
        child(args.stdio_child, stdio_only=True)
    elif args.child:
        child(args.child)
    else:
        return 0 if run(args.binary, args.output, args.arch) else 1


if __name__ == "__main__":
    raise SystemExit(main())
