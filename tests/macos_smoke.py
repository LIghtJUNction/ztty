#!/usr/bin/env python3
"""Native AppKit + Darwin PTY integration probe; run on a logged-in Mac.

    python3 tests/macos_smoke.py target/release/ztty

The probe verifies real PTY replies, Retina pixel geometry, chunked RGBA, PNG,
image deletion, alternate-screen isolation, and recovery after malformed input.
It deliberately reports pixel/IME checks as unperformed: a protocol ACK is not
proof that a native window contains the right pixels. The Linux pixel harness
is a separate acceptance gate.
"""
from __future__ import annotations

import argparse
import base64
import fcntl
import json
import os
from pathlib import Path
import platform
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time
import tty
import zlib


def receive(fd: int, expected: bytes, timeout: float = 3.0) -> bytes:
    deadline = time.monotonic() + timeout
    data = bytearray()
    while time.monotonic() < deadline:
        ready, _, _ = select.select([fd], [], [], max(0.0, deadline - time.monotonic()))
        if not ready:
            break
        chunk = os.read(fd, 65536)
        if not chunk:
            break
        data.extend(chunk)
        if expected in data:
            return bytes(data)
    raise AssertionError(f"missing {expected!r}; got {bytes(data)!r}")


def send_graphics(fd: int, header: bytes, payload: bytes = b"") -> None:
    packet = b"\x1b_G" + header + b";" + base64.b64encode(payload) + b"\x1b\\"
    view = memoryview(packet)
    while view:
        view = view[os.write(fd, view):]


def png() -> bytes:
    def chunk(kind: bytes, payload: bytes) -> bytes:
        return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", zlib.crc32(kind + payload))
    rows = b"".join(b"\0" + bytes((40, 150, 235, x * 255 // 31)) * 32 for x in range(32))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 32, 32, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")


def child(report_path: Path) -> int:
    fd = os.open("/dev/tty", os.O_RDWR | os.O_NOCTTY)
    previous = termios.tcgetattr(fd)
    checks: dict[str, bool] = {}
    report: dict[str, object] = {"host": platform.platform(), "checks": checks, "pixel_readback": "not_performed", "ime_keyboard": "not_performed"}
    try:
        tty.setraw(fd)
        for _ in range(100):
            rows, cols, pixel_w, pixel_h = struct.unpack("HHHH", fcntl.ioctl(fd, termios.TIOCGWINSZ, b"\0" * 8))
            if pixel_w and pixel_h:
                break
            time.sleep(0.02)
        assert (cols, rows) == (40, 12), (cols, rows)
        assert pixel_w >= cols and pixel_h >= rows, (pixel_w, pixel_h)
        report["pty_size"] = {"cols": cols, "rows": rows, "pixel_width": pixel_w, "pixel_height": pixel_h}
        checks["native_pty_and_pixel_geometry"] = True
        os.write(fd, b"\x1b[2J\x1b[H\x1b[?25l\x1b[6n")
        receive(fd, b"\x1b[1;1R")
        checks["cursor_reply"] = True
        send_graphics(fd, b"a=q,t=d,f=32,s=1,v=1,i=9101", bytes((255, 0, 0, 255)))
        receive(fd, b"i=9101;OK")
        checks["kitty_capability_query"] = True
        rgba = bytes((255, 64, 32, 128)) * (64 * 64)
        encoded = base64.b64encode(rgba)
        for offset in range(0, len(encoded), 4096):
            block = encoded[offset:offset + 4096]
            more = int(offset + len(block) < len(encoded))
            header = (b"a=T,t=d,f=32,s=64,v=64,i=9102,c=8,r=4,C=1," if offset == 0 else b"") + f"m={more}".encode()
            os.write(fd, b"\x1b_G" + header + b";" + block + b"\x1b\\")
        receive(fd, b"i=9102;OK")
        checks["chunked_rgba"] = True
        os.write(fd, b"\x1b[1;10H")
        send_graphics(fd, b"a=T,t=d,f=100,i=9103,c=8,r=4,C=1", png())
        receive(fd, b"i=9103;OK")
        checks["png"] = True
        os.write(fd, b"\x1b[?1049h")
        send_graphics(fd, b"a=p,i=9102,C=1")
        receive(fd, b"ENOENT")
        os.write(fd, b"\x1b[?1049l")
        send_graphics(fd, b"a=p,i=9102,C=1")
        receive(fd, b"i=9102;OK")
        checks["alternate_screen_image_isolation"] = True
        send_graphics(fd, b"a=T,t=d,f=100,i=9104", b"not-a-png")
        malformed_reply = receive(fd, b"i=9104;")
        assert b";OK" not in malformed_reply, malformed_reply
        os.write(fd, b"\x1b[H\x1b[6n")
        receive(fd, b"\x1b[1;1R")
        checks["malformed_image_text_recovery"] = True
        send_graphics(fd, b"a=d,d=I,i=9102")
        send_graphics(fd, b"a=p,i=9102,C=1")
        receive(fd, b"ENOENT")
        checks["image_deletion"] = True
        os.write(fd, "\x1b[7;1HAppKit + Darwin PTY: PASS\r\n中文 UTF-8 · PNG / RGBA\r\n".encode())
        report["passed"] = True
        time.sleep(0.25)
        return 0
    except Exception as exc:
        report["passed"] = False
        report["error"] = repr(exc)
        return 1
    finally:
        report_path.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
        termios.tcsetattr(fd, termios.TCSANOW, previous)
        os.close(fd)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="?", default="target/release/ztty")
    parser.add_argument("--child-report", type=Path)
    args = parser.parse_args()
    if args.child_report:
        return child(args.child_report)
    if sys.platform != "darwin":
        parser.error("this probe requires a logged-in native macOS desktop")
    binary = Path(args.binary).resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="ztty-macos-") as temporary:
        directory = Path(temporary)
        report_path = directory / "probe.json"
        config = directory / "ztty.conf"
        config.write_text("font = Menlo:size=12\nbackground-opacity = 100\nbackground-blur = false\ncursor-blink = false\n")
        process = subprocess.Popen([str(binary), "--config", str(config), "--geometry", "40x12", "-e", sys.executable, str(Path(__file__).resolve()), "--child-report", str(report_path)], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            stdout, stderr = process.communicate(timeout=30)
        except subprocess.TimeoutExpired:
            process.terminate()
            try:
                stdout, stderr = process.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                stdout, stderr = process.communicate()
            raise AssertionError("native ztty integration timed out")
        if not report_path.exists():
            raise AssertionError(f"child never completed; status={process.returncode}, stdout={stdout!r}, stderr={stderr!r}")
        report = json.loads(report_path.read_text())
        report["application_exit"] = process.returncode
        print(json.dumps(report, ensure_ascii=False, indent=2))
        return 0 if report.get("passed") and process.returncode == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
