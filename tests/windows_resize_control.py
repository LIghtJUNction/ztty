#!/usr/bin/env python3
"""Diagnostic only: independent standard Win32 retained-DIB resize control.

Build windows_resize_control.c with -municode -O2 -Wall -Wextra -Werror
-luser32 -lgdi32. This experiment never changes the ztty native gate result.
"""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import traceback
import uuid

import windows_desktop as desktop
import windows_native as native
from windows_workspace_native import Window


REQUIRED = 16
CLASS_NAME = "Win32ResizeControl"
BACKGROUND, RED, BLUE = [16, 24, 32], [255, 0, 0], [0, 0, 255]


def identity(binary):
    result = desktop.source_identity(binary)
    run = os.environ.get("GITHUB_RUN_ID", "")
    if not re.fullmatch(r"[0-9]+", run):
        raise ValueError("control evidence requires GITHUB_RUN_ID")
    result.update({"run_id": run, "c_source_sha256": hashlib.sha256(
        Path(__file__).with_suffix(".c").read_bytes()).hexdigest()})
    return result


class ControlWindow(Window):
    def windows(self):
        if self.process.poll() is not None:
            raise RuntimeError(f"control exited before observation: {self.process.returncode}")
        result = []
        @self.enum_proc
        def visit(hwnd, _):
            name = ctypes.create_unicode_buffer(256)
            self.user.GetClassNameW(hwnd, name, len(name))
            if self.pid(hwnd) == self.process.pid and name.value == CLASS_NAME:
                result.append(hwnd)
            return True
        self.user.EnumWindows(visit, 0)
        return result

    def require_owned(self):
        if (self.process.poll() is not None or not self.user.IsWindow(self.hwnd)
                or self.pid(self.hwnd) != self.process.pid):
            raise RuntimeError("control window no longer belongs to the live owned process")

    def client_size(self):
        self.require_owned()
        rect = native.NativeRect()
        if not self.user.GetClientRect(self.hwnd, ctypes.byref(rect)):
            raise OSError("GetClientRect(control) failed")
        return [rect.right, rect.bottom]

    def resize(self, width):
        self.require_owned()
        # Keep the original cross-process caller's ignored x/y and flags.
        if not self.user.SetWindowPos(self.hwnd, None, 0, 0, width, 229, 0x16):
            raise OSError("SetWindowPos(control) failed")

    def stage(self, output, name):
        self.require_owned()
        state = self.geometry()
        samples = [(139, 28, BACKGROUND)]
        if state["client"][2] == 180:
            samples += [(175, 28, RED), (175, 45, BLUE), (178, 95, BACKGROUND)]
        self.pixels(samples)  # Record actual pixels; this is not a new acceptance wait.
        state.update({"samples": list(self.last_samples), "monotonic_seconds": time.monotonic(),
                      "screenshot": self.capture(output, name)})
        origin = self.screen_point(0, 0)
        state["client_screen_origin"] = [origin.x, origin.y]
        self.report["resize_stages"][name] = state


def run_one(args, output, number, expected_identity):
    report = {"diagnostic_only": True, "number": number, "passed": False, "status": "running",
              "checks": [], "resize_stages": {}, "architecture": args.arch,
              "resize_flags": 0x16, "resize_xy": [0, 0], "pixel_timeout_seconds": 10,
              "expected_outer_before": [32, 32, 228, 261], **expected_identity}
    process = window = None
    stdout_path = output.with_suffix(".stdout.log")
    stderr_path = output.with_suffix(".stderr.log")
    try:
        assert identity(args.binary) == expected_identity
        with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
            try:
                process = subprocess.Popen([str(args.binary)], stdin=subprocess.DEVNULL,
                    stdout=stdout, stderr=stderr, creationflags=subprocess.CREATE_NO_WINDOW)
                report["pid"] = process.pid
                report["process_architecture"] = native.process_architecture(process.pid)
                if report["process_architecture"]["emulated"]:
                    raise AssertionError("control process must be native")
                window = ControlWindow(process, report)
                report["hwnd"] = int(window.hwnd)
                window.require_owned()
                if not window.user.SetWindowPos(window.hwnd, ctypes.c_void_p(-1), 32, 32, 0, 0, 1):
                    raise OSError("placing the owned control window failed")
                window.user.SetForegroundWindow(window.hwnd)
                native.wait_for(lambda: window.user.GetForegroundWindow() == window.hwnd,
                                "owned control foreground")
                state = window.geometry()
                if state["outer"] != [32, 32, 228, 261] or state["client"] != [0, 0, 180, 190] or state["dpi"] != 96:
                    raise native.DesktopBlocked(f"control geometry differs from the resize fixture: {state}")
                window.expect("before", [(175, 28, RED)])
                report["checks"].append({"name": "before", "passed": True})
                window.stage(output, "before")
                window.resize(160)
                native.wait_for(lambda: window.client_size() == [144, 190], "control shrunk client")
                window.expect("shrunk", [(139, 28, BACKGROUND)])
                report["checks"].append({"name": "shrunk", "passed": True})
                window.stage(output, "shrunk")
                window.resize(196)
                native.wait_for(lambda: window.client_size() == [180, 190], "control restored client")
                try:
                    window.expect("restored", [(175, 28, RED)])
                    report["checks"].append({"name": "restored", "passed": True})
                finally:
                    window.stage(output, "restored")
                report["passed"] = True
            except BaseException as error:
                report["error"] = repr(error)
                if window is not None:
                    try:
                        window.require_owned()
                        report["failure_screenshot"] = window.capture(output, "failure")
                    except Exception as capture_error:
                        report["capture_error"] = repr(capture_error)
            finally:
                def close_owned():
                    if window is not None and process.poll() is None:
                        window.require_owned()
                        if not window.user.PostMessageW(window.hwnd, 0x10, 0, 0):
                            raise OSError("PostMessage(WM_CLOSE control) failed")
                native.cleanup_owned(process, close_owned, None, report)
        report["startup"] = json.loads(stdout_path.read_text(encoding="utf-8"))
        startup = report["startup"]
        if (startup.get("pid") != report.get("pid") or startup.get("dpi_awareness") != -4
                or startup.get("class_style") != 3 or startup.get("window_style") != 0x00CF0000
                or startup.get("background_brush_null") is not True
                or startup.get("dark_attribute") != 20 or startup.get("dark_value") != 1
                or startup.get("dark_hresult") != 0 or startup.get("valid_rect_policy") != "default"
                or startup.get("nocopybits_added") is not False):
            raise AssertionError(f"unexpected control setup: {startup}")
        if report["cleanup"].get("returncode") != 0:
            report["passed"] = False
        assert identity(args.binary) == expected_identity
    except BaseException as error:
        report["passed"] = False
        report.setdefault("error", repr(error))
    finally:
        report["status"] = "passed" if report["passed"] else "failed"
        for key, path in [("stdout", stdout_path), ("stderr", stderr_path)]:
            if path.exists():
                report[key] = {"path": path.name, "bytes": path.stat().st_size,
                               "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        desktop.write_json(output, report)
    return report


def validate_runs(report, expected_identity, directory):
    if report.get("diagnostic_only") is not True or report.get("required") != REQUIRED:
        raise ValueError("missing fixed diagnostic run requirement")
    entries = report.get("runs", [])
    if not 1 <= len(entries) <= REQUIRED:
        raise ValueError("missing bounded control reports")
    completed = 0
    for number, entry in enumerate(entries, 1):
        name = entry["report"]
        if Path(name).name != name or not name.endswith(f".cold-{number:02d}.json"):
            raise ValueError("invalid control report name/order")
        raw = (directory / name).read_bytes()
        if hashlib.sha256(raw).hexdigest() != entry["report_sha256"]:
            raise ValueError("control report changed after collection")
        run = json.loads(raw)
        if any(run.get(key) != value for key, value in expected_identity.items()):
            raise ValueError("control source/binary identity mismatch")
        cleanup = run.get("cleanup", {})
        success = (run.get("passed") is True and run.get("diagnostic_only") is True
                   and run.get("number") == number and cleanup.get("process_exited") is True
                   and cleanup.get("returncode") == 0 and not run.get("shutdown_timeout")
                   and run.get("checks") == [{"name": name, "passed": True}
                                              for name in ("before", "shrunk", "restored")])
        if entry.get("passed") is not success:
            raise ValueError("control result disagrees with its raw evidence")
        if not success and number != len(entries):
            raise ValueError("control continued after the first failure")
        completed += int(success)
    if report.get("completed") != completed:
        raise ValueError("control completion count mismatch")
    return completed == REQUIRED and len(entries) == REQUIRED


def worker(args, expected_identity, state_path):
    context = desktop.WindowsAPI().context()
    desktop.require_context(context, args.worker_desktop)
    state = {"passed": True, "pid": os.getpid(), "desktop": args.worker_desktop,
             "context": context, **expected_identity}
    desktop.write_json(state_path, state)
    machine = 0x8664 if args.arch == "x64" else 0xAA64
    python = native.process_architecture()
    if python["emulated"] or python["native_machine"] != machine or native.executable_machine(args.binary) != machine:
        raise AssertionError("control binary and Python must match the native runner architecture")
    report = {"diagnostic_only": True, "passed": False, "status": "running", "required": REQUIRED,
              "completed": 0, "runs": [], "desktop": args.worker_desktop,
              "python_process": python, "binary_machine": machine, **expected_identity}
    for number in range(1, REQUIRED + 1):
        output = args.output.with_suffix(f".cold-{number:02d}.json")
        run = run_one(args, output, number, expected_identity)
        report["runs"].append({"number": number, "report": output.name, "passed": run["passed"],
            "report_sha256": hashlib.sha256(output.read_bytes()).hexdigest()})
        if run["passed"]:
            report["completed"] += 1
        else:
            report["error"] = f"control {number}/{REQUIRED} failed: {run.get('error', run.get('cleanup'))}"
        desktop.write_json(args.output, report)
        if not run["passed"]:
            break
    report["passed"] = validate_runs(report, expected_identity, args.output.parent)
    report["status"] = "passed" if report["passed"] else "failed"
    desktop.write_json(args.output, report)
    return 0 if report["passed"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--arch", choices=["x64", "arm64"], required=True)
    parser.add_argument("--isolated-desktop", action="store_true")
    parser.add_argument("--worker-desktop", help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(), args.output.resolve()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    desktop_path = args.output.with_suffix(".desktop.json")
    state_path = args.output.with_suffix(".desktop.worker.json")
    try:
        desktop.require_ci(args.isolated_desktop)
        expected_identity = identity(args.binary)
        if args.worker_desktop:
            if not re.fullmatch(r"ztty-control-[0-9]+-[0-9a-f]{32}", args.worker_desktop):
                raise desktop.DesktopBlocked("invalid owned control desktop name")
            return worker(args, expected_identity, state_path)
        name = f"ztty-control-{os.getpid()}-{uuid.uuid4().hex}"
        command = [sys.executable, str(Path(__file__).resolve()), "--isolated-desktop",
                   "--worker-desktop", name, "--binary", str(args.binary),
                   "--output", str(args.output), "--arch", args.arch]
        desktop.write_json(args.output, {"diagnostic_only": True, "passed": False, "status": "not_run"})
        stdout, stderr = args.output.with_suffix(".desktop.stdout.log"), args.output.with_suffix(".desktop.stderr.log")
        isolated = desktop.run_isolated(desktop.WindowsAPI(), command, name, stdout, stderr, 150,
            checkpoint=lambda value: desktop.write_json(desktop_path, value))
        report = json.loads(args.output.read_text(encoding="utf-8"))
        state = json.loads(state_path.read_text(encoding="utf-8"))
        valid = validate_runs(report, expected_identity, args.output.parent)
        valid = (valid and state.get("passed") is True and state.get("desktop") == name
                 and state.get("pid") == isolated.get("worker_pid")
                 and all(state.get(key) == value for key, value in expected_identity.items())
                 and identity(args.binary) == expected_identity)
        report["desktop_isolation"] = isolated
        report["worker_state"] = state
        report["passed"] = valid and isolated.get("passed") is True and report.get("passed") is True
        report["status"] = "passed" if report["passed"] else "failed"
        desktop.write_json(args.output, report)
        print(json.dumps(report, indent=2))
        return 0 if report["passed"] else 1
    except BaseException as error:
        try:
            report = json.loads(args.output.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            report = {}
        report.update({"diagnostic_only": True, "passed": False, "status": "failed"})
        report.setdefault("error", repr(error))
        desktop.write_json(args.output, report)
        traceback.print_exc()
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
