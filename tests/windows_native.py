#!/usr/bin/env python3
"""Native Windows acceptance gate (requires an interactive Windows desktop).

python tests/windows_native.py --binary target/release/ztty.exe \
    --output artifacts/windows-native.json

Uses an actual Python child behind ConPTY, Win32 window messages, and GetPixel
on the client DC. It deliberately fails if the installed ConPTY filters Kitty
APC traffic; a cross compilation is not a substitute for this runtime gate.
"""

import argparse
import base64
import binascii
import ctypes
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


def run(binary, output):
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
    user.SetProcessDPIAware()
    checks = []
    process = None
    hwnd = None
    report = {"platform": sys.platform, "binary": str(binary.resolve()),
              "transport": "real ConPTY", "renderer": "Win32 retained DIB",
              "checks": checks, "passed": False}
    try:
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
            user.SetForegroundWindow(hwnd)
            wait_for(lambda: (directory / "ready").exists(), "Unicode command arguments and ConPTY child")
            checks.append({"name": "unicode_arguments_and_native_child", "passed": True})
            rect = wintypes.RECT()
            assert user.GetClientRect(hwnd, ctypes.byref(rect))
            cw, ch = rect.right // 20, rect.bottom // 10
            assert cw > 0 and ch > 0
            report["client_pixels"] = [rect.right, rect.bottom]
            report["cell_pixels"] = [cw, ch]
            sequence = 0

            def send(data):
                nonlocal sequence
                path = directory / f"command-{sequence}.bin"
                stage = path.with_suffix(".tmp")
                stage.write_bytes(data)
                stage.replace(path)
                wait_for(lambda: (directory / f"done-{sequence}").exists(), "PTY output write")
                sequence += 1

            def pixel(col, row):
                dc = user.GetDC(hwnd)
                assert dc
                try:
                    rgb = int(gdi.GetPixel(dc, int(col * cw), int(row * ch)))
                    if rgb == 0xFFFFFFFF:
                        raise AssertionError("GetPixel failed")
                    return [rgb & 255, (rgb >> 8) & 255, (rgb >> 16) & 255]
                finally:
                    user.ReleaseDC(hwnd, dc)

            def expect(name, col, row, expected):
                actual = None

                def matches():
                    nonlocal actual
                    actual = pixel(col, row)
                    return all(abs(a - b) <= 1 for a, b in zip(actual, expected))

                wait_for(matches, f"{name}: expected {expected}, last {actual}")
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
            outer = wintypes.RECT()
            assert user.GetWindowRect(hwnd, ctypes.byref(outer))
            old_width, old_height = outer.right - outer.left, outer.bottom - outer.top
            assert user.SetWindowPos(hwnd, None, 0, 0, old_width - 4 * cw, old_height, 0x16)

            def resized():
                current = wintypes.RECT()
                user.GetClientRect(hwnd, ctypes.byref(current))
                return current.right == 16 * cw

            wait_for(resized, "native client resize")
            expect("resize_clips_without_ghosts", 15.5, 1.5, [16, 24, 32])
            assert user.SetWindowPos(hwnd, None, 0, 0, old_width, old_height, 0x16)
            expect("resize_restores_visible_image", 19.5, 1.5, [255, 0, 0])
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
            assert process.poll() is None, "ztty exited unexpectedly"
            report["passed"] = True
    except BaseException as error:
        report["error"] = str(error)
        raise
    finally:
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/ztty.exe"))
    parser.add_argument("--output", type=Path, default=Path("artifacts/windows-native.json"))
    parser.add_argument("--child")
    args = parser.parse_args()
    if sys.platform != "win32":
        parser.error("this acceptance gate must run on native Windows")
    if args.child:
        child(args.child)
    else:
        run(args.binary, args.output)


if __name__ == "__main__":
    main()
