#!/usr/bin/env python3
"""Native AppKit/PTY, Sixel, WindowServer pixel and window recovery acceptance.

The Swift helper must be compiled with the native Apple SDK. A decoded image or
a PTY reply never substitutes for a real WindowServer capture. Permission
failures remain blocked, with JSON/log evidence and a nonzero exit status.
"""
from __future__ import annotations

import argparse
import base64
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import tty

from macos_smoke import child as protocol_child, receive
# This deterministic encoder has no Win32 calls; use the exact Windows vectors.
from windows_native import sixel


def write_json(path: Path, value: object) -> None:
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


def wait_for(predicate, description: str, timeout: float = 10):
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


def pty_size(fd: int) -> dict:
    rows, cols, width, height = struct.unpack(
        "HHHH", fcntl.ioctl(fd, termios.TIOCGWINSZ, b"\0" * 8))
    return {"cols": cols, "rows": rows, "pixel_width": width, "pixel_height": height}


def child(directory: Path) -> int:
    """Keep the original eight protocol checks, then serve bounded PTY vectors."""
    protocol_path = directory / "protocol.json"
    status = protocol_child(protocol_path)
    if status:
        return status
    fd = os.open("/dev/tty", os.O_RDWR | os.O_NOCTTY)
    previous = termios.tcgetattr(fd)
    try:
        tty.setraw(fd)
        write_json(directory / "ready.json", {
            "pid": os.getpid(), "cwd": os.getcwd(), "pwd": os.environ.get("PWD"),
            "pty": pty_size(fd),
            "protocol": json.loads(protocol_path.read_text(encoding="utf-8")),
        })
        for sequence in range(64):
            request_path = directory / f"command-{sequence}.json"
            wait_for(request_path.exists, "parent PTY command", timeout=45)
            request = json.loads(request_path.read_text(encoding="utf-8"))
            reply = {"passed": False}
            try:
                payload = base64.b64decode(request.get("payload", ""), validate=True)
                view = memoryview(payload)
                while view:
                    written = os.write(fd, view)
                    if not written:
                        raise OSError("zero-byte PTY write")
                    view = view[written:]
                if request.get("reply"):
                    received = receive(fd, base64.b64decode(request["reply"], validate=True))
                    reply["reply_hex"] = received.hex()
                if request.get("size"):
                    expected = request["size"]
                    wait_for(lambda: pty_size(fd) == expected, f"Darwin PTY resize to {expected}")
                reply["pty"] = pty_size(fd)
                reply["passed"] = True
            except Exception as error:
                reply["error"] = repr(error)
            write_json(directory / f"done-{sequence}.json", reply)
            if not reply["passed"]:
                return 1
            if request.get("exit"):
                return 0
        raise AssertionError("native probe exceeded its PTY command budget")
    finally:
        termios.tcsetattr(fd, termios.TCSANOW, previous)
        os.close(fd)


class NativeFailure(AssertionError):
    def __init__(self, message: str, blocked: bool = False):
        super().__init__(message)
        self.blocked = blocked


def validate_helper_result(value: dict, returncode: int) -> None:
    if value.get("passed") is not True or returncode != 0:
        raise NativeFailure(
            value.get("error", f"native helper exit {returncode}"),
            blocked=value.get("status") == "blocked",
        )
    if value.get("status") != "passed":
        raise NativeFailure("native helper returned inconsistent acceptance status")


def screenshot_metadata(path: Path) -> dict:
    data = path.read_bytes()
    if len(data) < 33 or data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        raise AssertionError(f"native capture is not a PNG: {path.name}")
    width, height = struct.unpack(">II", data[16:24])
    if not (0 < width <= 8192 and 0 < height <= 8192 and width * height <= 16 * 1024 * 1024):
        raise AssertionError(f"invalid native PNG dimensions: {width}x{height}")
    return {"file": path.name, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
            "width": width, "height": height}


def run(binary: Path, helper: Path, prefix: Path) -> tuple[dict, int]:
    report = {
        "passed": False, "status": "failed", "checks": {}, "stages": [],
        "pixel_readback": "not_performed", "ime_keyboard": "not_performed",
        "host": platform.platform(), "architecture": platform.machine(),
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "helper_sha256": hashlib.sha256(helper.read_bytes()).hexdigest(),
        "remaining_owned_processes": [],
    }
    checks = report["checks"]
    prefix.parent.mkdir(parents=True, exist_ok=True)
    process = None
    child_pid = None
    window_id = None
    capture_permitted = False
    sequence = 0

    with tempfile.TemporaryDirectory(prefix="ztty-macos-native-") as temporary:
        directory = Path(temporary).resolve()
        window_title = "ztty-native-" + directory.name
        report["window_title"] = window_title
        cwd = directory / "cwd with 空格与Unicode"
        cwd.mkdir()
        config = directory / "ztty.conf"
        config.write_text(
            "font = Menlo:size=12\nbackground = #101820\nwindow-padding = 0\n"
            "background-opacity = 100\nbackground-blur = false\ncursor-blink = false\n",
            encoding="utf-8",
        )
        # This path is relative to the launcher's original cwd, not --cwd.
        config_argument = os.path.relpath(config, Path.cwd())

        def invoke(name: str, action: str = "capture", **values) -> dict:
            nonlocal window_id
            report["active_check"] = name
            png = prefix.with_name(prefix.name + f".{name}.png")
            request = {"action": action, "pid": process.pid if process else 0,
                       "window_id": window_id, "window_title": window_title,
                       "png": str(png), "timeout": 10, **values}
            request_path = directory / f"helper-{name}.json"
            write_json(request_path, request)
            result = None
            value = {"passed": False, "status": "failed", "action": action}
            stdout = stderr = ""
            try:
                result = subprocess.run(
                    [str(helper), str(request_path)], capture_output=True, text=True,
                    timeout=float(request["timeout"]) + 8,
                )
                stdout, stderr = result.stdout, result.stderr
                # Persist process status before decoding: an abort can leave no
                # JSON at all, while stderr still contains the actual assertion.
                value["exit_code"] = result.returncode
                if result.returncode < 0:
                    value["termination_signal"] = -result.returncode
                    try:
                        value["termination_signal_name"] = signal.Signals(-result.returncode).name
                    except ValueError:
                        value["termination_signal_name"] = "UNKNOWN"
                try:
                    value.update(json.loads(stdout))
                except (ValueError, TypeError) as error:
                    value["json_error"] = repr(error)
                    detail = value.get("termination_signal_name", "no signal")
                    value["error"] = (
                        f"native helper exited {result.returncode} ({detail}) without a valid JSON report"
                    )
                    raise NativeFailure(value["error"]) from error
                value["exit_code"] = result.returncode
                if png.exists():
                    value["png"] = screenshot_metadata(png)
                if action in ("capture", "resize", "restore") and value.get("passed"):
                    window = value.get("window", {})
                    if (window.get("pid") != process.pid or not window.get("window_id")
                            or window.get("title") != window_title):
                        raise AssertionError("native capture does not match the launched PID and unique terminal title")
                    if window_id is not None and window["window_id"] != window_id:
                        raise AssertionError("native capture changed its owned WindowID")
                    window_id = window["window_id"]
                    if "png" not in value or not value.get("screenshot"):
                        raise AssertionError("native helper accepted pixels without saving a real PNG")
                    screenshot = value["screenshot"]
                    dimensions = [value["png"]["width"], value["png"]["height"]]
                    if (screenshot.get("pid") != process.pid
                            or screenshot.get("window_id") != window_id
                            or Path(screenshot.get("path", "")).resolve() != png
                            or dimensions != [screenshot.get("width"), screenshot.get("height")]
                            or dimensions != [value.get("image", {}).get("width"),
                                              value.get("image", {}).get("height")]):
                        raise AssertionError("saved PNG ownership, path or geometry disagrees with native capture")
                    report["pixel_readback"] = "WindowServer window via ScreenCaptureKit"
                validate_helper_result(value, result.returncode)
            except Exception as error:
                value["passed"] = False
                if not isinstance(error, NativeFailure):
                    value["error"] = repr(error)
                if isinstance(error, subprocess.TimeoutExpired):
                    value["timed_out"] = True
                    stdout = error.stdout.decode(errors="replace") if isinstance(error.stdout, bytes) else error.stdout or ""
                    stderr = error.stderr.decode(errors="replace") if isinstance(error.stderr, bytes) else error.stderr or ""
                raise
            finally:
                value["stdout_bytes"] = len(stdout.encode("utf-8"))
                value["stderr_tail"] = stderr[-4000:]
                if result is not None:
                    value["exit_code"] = result.returncode
                if png.exists() and "png" not in value:
                    try:
                        value["png"] = screenshot_metadata(png)
                    except Exception as error:
                        value["png_error"] = repr(error)
                value["name"] = name
                report["stages"].append(value)
                write_json(prefix.with_name(prefix.name + f".{name}.json"), value)
                prefix.with_name(prefix.name + f".{name}.stdout.log").write_text(stdout, encoding="utf-8")
                prefix.with_name(prefix.name + f".{name}.stderr.log").write_text(stderr, encoding="utf-8")
            return value

        def command(payload=b"", reply=b"", expected_size=None, stop=False) -> dict:
            nonlocal sequence
            index = sequence
            sequence += 1
            write_json(directory / f"command-{index}.json", {
                "payload": base64.b64encode(payload).decode("ascii"),
                "reply": base64.b64encode(reply).decode("ascii"),
                "size": expected_size, "exit": stop,
            })
            path = directory / f"done-{index}.json"

            def completed():
                if path.exists():
                    return json.loads(path.read_text(encoding="utf-8"))
                if process.poll() is not None:
                    # A successful final command can race with child exit.
                    if path.exists():
                        return json.loads(path.read_text(encoding="utf-8"))
                    raise NativeFailure(f"ztty exited during PTY command {index}: {process.returncode}")
                return None

            value = wait_for(completed, f"PTY command {index}", timeout=12)
            report.setdefault("pty_commands", []).append(value)
            if not value.get("passed"):
                raise NativeFailure(value.get("error", "PTY child rejected a command"))
            return value

        stdout_path = prefix.with_name(prefix.name + ".application.stdout.log")
        stderr_path = prefix.with_name(prefix.name + ".application.stderr.log")
        with stdout_path.open("wb") as stdout_file, stderr_path.open("wb") as stderr_file:
            try:
                report["active_check"] = "original_protocol_checks"
                process = subprocess.Popen(
                    [str(binary), "--config", config_argument, "--cwd", str(cwd),
                     "--geometry", "40x12", "-e", sys.executable,
                     str(Path(__file__).resolve()), "--child-directory", str(directory)],
                    stdout=stdout_file, stderr=stderr_file,
                )
                report["application_pid"] = process.pid
                ready_path = directory / "ready.json"

                def ready():
                    if ready_path.exists():
                        return json.loads(ready_path.read_text(encoding="utf-8"))
                    if process.poll() is not None:
                        path = directory / "protocol.json"
                        if path.exists():
                            report["protocol"] = json.loads(path.read_text(encoding="utf-8"))
                        raise NativeFailure("original macOS protocol probe exited before pixel testing")
                    return None

                state = wait_for(ready, "original eight AppKit/PTY protocol checks", timeout=35)
                child_pid = state["pid"]
                report["child_pid"] = child_pid
                report["protocol"] = state["protocol"]
                checks.update(state["protocol"]["checks"])
                if state["protocol"].get("passed") is not True or len(state["protocol"]["checks"]) != 8:
                    raise NativeFailure("the original eight macOS protocol checks did not all pass")
                report["cwd"] = {
                    "requested": str(cwd), "child": state["cwd"], "pwd": state.get("pwd"),
                    "config_argument": config_argument, "launch_directory": str(Path.cwd()),
                }
                report["active_check"] = "unicode_cwd_reaches_native_pty_child"
                pwd = state.get("pwd")
                if (not Path(state["cwd"]).samefile(cwd) or not isinstance(pwd, str)
                        or not Path(pwd).is_absolute() or not Path(pwd).samefile(cwd)):
                    raise NativeFailure("Darwin PTY cwd/PWD do not identify the requested Unicode directory")
                checks["unicode_cwd_reaches_native_pty_child"] = True
                checks["relative_config_resolves_before_cwd"] = True
                initial = state["pty"]
                report["pty_size"] = initial
                if (initial["cols"], initial["rows"]) != (40, 12):
                    raise NativeFailure(f"unexpected initial PTY size: {initial}")
                if initial["pixel_width"] % 40 or initial["pixel_height"] % 12:
                    raise NativeFailure("PTY physical cell geometry is not integral")
                cw, ch = initial["pixel_width"] // 40, initial["pixel_height"] // 12
                client = [initial["pixel_width"], initial["pixel_height"]]
                background = [16, 24, 32]

                preflight = invoke("permissions", "preflight")
                if (preflight.get("permissions") != {"screen_capture": True, "accessibility": True}
                        or preflight.get("pixel_layout_vector") != "passed"
                        or preflight.get("window_selection_vector") != "passed"):
                    raise NativeFailure("native helper did not verify both permissions, capture pixel layout and window selection")
                if preflight.get("architecture") != platform.machine() or preflight.get("process_translated") is not False:
                    raise NativeFailure("helper architecture is not the native Python/runner architecture")
                architectures = subprocess.check_output(
                    ["xcrun", "lipo", "-archs", str(binary)], text=True, timeout=10).strip().split()
                if architectures != [platform.machine()]:
                    raise NativeFailure(f"unexpected Mach-O architecture: {architectures}")
                report["binary_architectures"] = architectures
                checks["native_helper_and_binary_architecture"] = True
                capture_permitted = True
                invalid = invoke(
                    "invalid_cwd_fails_before_window", "invalid_cwd",
                    binary=str(binary),
                    arguments=["--cwd", str(directory / "missing cwd 不存在"),
                               "--config", config_argument, "-e", "/usr/bin/true"],
                )
                if not invalid.get("launch", {}).get("window_observations"):
                    raise NativeFailure("invalid-cwd probe did not observe WindowServer")
                checks["invalid_cwd_fails_before_window"] = True

                def capture(name, samples, action="capture", expected=client, **values):
                    result = invoke(
                        name, action, expected_client_pixels=expected,
                        samples=[{"x": x, "y": y, "rgb": rgb} for x, y, rgb in samples],
                        **values,
                    )
                    checks[name] = True
                    return result

                width, height = 48, 24
                x0, y0 = cw, 2 * ch
                position = b"\x1b[3;2H"
                rgb = [1 + x // 16 for _ in range(height) for x in range(width)]
                image = sixel(width, height, rgb, {1: (100, 0, 0), 2: (0, 100, 0), 3: (0, 0, 100)})
                rgb_samples = [(x0 + 8, y0 + 8, [255, 0, 0]),
                               (x0 + 24, y0 + 8, [0, 255, 0]),
                               (x0 + 40, y0 + 16, [0, 0, 255])]
                command(b"\x1b]2;" + window_title.encode("ascii") + b"\x07"
                        + b"\x1b[0m\x1b[2J\x1b[H\x1b[?25l" + position + image)
                capture("sixel_rgb_native_pixels", rgb_samples)
                green = sixel(width, height, [2] * (width * height), {2: (0, 100, 0)})
                holes = [1 if x < width // 2 else 0 for _ in range(height) for x in range(width)]
                transparent = sixel(width, height, holes, {1: (100, 0, 0)})
                command(b"\x1b[2J" + position + green + position + transparent)
                capture("sixel_transparent_preserves_underlying_image", [
                    (x0 + 8, y0 + 8, [255, 0, 0]), (x0 + 40, y0 + 8, [0, 255, 0])])
                opaque = sixel(width, height, holes, {1: (100, 0, 0)}, transparent=False)
                command(b"\x1b[48;2;16;24;32m" + position + opaque)
                capture("sixel_opaque_uses_current_background", [
                    (x0 + 8, y0 + 8, [255, 0, 0]), (x0 + 40, y0 + 8, background)])
                blank = [(x0 + x, y0 + y, background) for x, y in ((8, 8), (24, 8), (40, 16))]
                command(b"\x1b[2J\x1b[H")
                capture("sixel_erase_display_repaints", blank)
                command(position + image[:-2] + b"\x18\x1b[7;9H\x1b[6n", b"\x1b[7;9R")
                capture("sixel_cancel_and_pty_recovery", blank)
                command(b'\x1bP7;1q"1;1;4294967295;4294967295#1!99~\x1b\\'
                        + position + image + b"\x1b[9;3H\x1b[6n", b"\x1b[9;3R")
                capture("sixel_recovers_after_oversized_raster", rgb_samples)

                # A right-edge Sixel survives a real AppKit -> Darwin PTY resize.
                edge = sixel(2 * cw, ch, [1] * (2 * cw * ch), {1: (100, 0, 0)})
                command(b"\x1b[2J\x1b[2;38H" + edge)
                edge_samples = [(38 * cw, ch + ch // 2, [255, 0, 0])]
                capture("sixel_before_resize", edge_samples)
                shrunk = [32 * cw, 12 * ch]
                capture("sixel_resize_clips_without_ghosts",
                        [(31 * cw + cw // 2, ch + ch // 2, background)],
                        action="resize", expected=shrunk, target_client_pixels=shrunk)
                changed = dict(initial, cols=32, pixel_width=32 * cw)
                command(expected_size=changed)
                capture("sixel_resize_restores_visible_image", edge_samples,
                        action="resize", target_client_pixels=client)
                command(expected_size=initial)
                invoke("window_minimized", "minimize")
                checks["window_minimized"] = True
                capture("sixel_minimize_restore_visible_image", edge_samples, action="restore")
                command(b"\x1b[9;3H\x1b[6n", b"\x1b[9;3R", expected_size=initial)
                checks["pty_recovers_after_window_restore"] = True
                command(stop=True)
                process.wait(timeout=10)
                report["application_exit"] = process.returncode
                if process.returncode != 0:
                    raise NativeFailure(f"native application exited with {process.returncode}")
                report["passed"] = True
                report["status"] = "passed"
            except Exception as error:
                active = report.get("active_check", "initialization")
                checks[active] = False
                report["error"] = repr(error)
                report["status"] = "blocked" if isinstance(error, NativeFailure) and error.blocked else "failed"
                if capture_permitted and process is not None and process.poll() is None:
                    try:
                        invoke("failure_window", timeout=3)
                    except Exception as diagnostic_error:
                        report["failure_capture_error"] = repr(diagnostic_error)
                report["active_check"] = active
            finally:
                if process is not None:
                    if process.poll() is None:
                        process.terminate()
                        try:
                            process.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            process.kill()
                            process.wait(timeout=5)
                    report["application_exit"] = process.returncode
                if child_pid is not None:
                    # The PTY closes on application exit. Confirm its owned child
                    # also exited; never signal an unverified/reused process ID.
                    deadline = time.monotonic() + 2
                    while time.monotonic() < deadline:
                        state = subprocess.run(
                            ["/bin/ps", "-p", str(child_pid), "-o", "stat=", "-o", "command="],
                            capture_output=True, text=True, timeout=2,
                        ).stdout.strip()
                        if not state or state.startswith("Z") or str(directory) not in state:
                            break
                        time.sleep(0.02)
                    else:
                        report["remaining_owned_processes"].append(child_pid)
                        report["passed"] = False
                        report["status"] = "failed"
                        report["cleanup_error"] = "the owned Darwin PTY child remained alive"
        report["application_logs"] = [stdout_path.name, stderr_path.name]
    return report, 0 if report["passed"] else (2 if report["status"] == "blocked" else 1)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--helper", type=Path)
    parser.add_argument("--evidence-prefix", type=Path)
    parser.add_argument("--child-directory", type=Path)
    args = parser.parse_args()
    if args.child_directory:
        return child(args.child_directory.resolve(strict=True))
    if sys.platform != "darwin":
        parser.error("this gate requires a logged-in native macOS desktop")
    if not args.binary or not args.helper or not args.evidence_prefix:
        parser.error("--binary, --helper and --evidence-prefix are required")
    report, code = run(args.binary.resolve(strict=True), args.helper.resolve(strict=True),
                       args.evidence_prefix.resolve())
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
