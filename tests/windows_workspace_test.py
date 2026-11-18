#!/usr/bin/env python3
"""Portable failure contracts, never substitutes for native workspace pixels."""
import copy
import ctypes
import hashlib
import json
from pathlib import Path
import tempfile
import types
import unittest
from unittest.mock import patch

import windows_desktop as desktop
import windows_workspace_native as workspace


def complete_report():
    report = {"passed": True, "status": "passed",
            "checks": [{"name": name, "passed": True} for name in workspace.REQUIRED_CHECKS],
            "visible_pixel_checks_completed": 1,
            "cleanup": {"process_exited": True, "temporary_directory_removed": True,
                        "all_probe_processes_exited": True, "returncode": 0},
            "screenshots": {name: {"sha256": "a" * 64} for name in
                            ("split-right", "hold-focus", "hold-resize", "three-panes", "resized", "tab-restored", "surviving-tab")}}
    report["checks"][2]["pixels"] = [{"owned": True, "visible": True, "rgb": [255, 0, 0], "expected": [255, 0, 0]}]
    report["checks"][2]["synchronized_layout"] = {
        name: {"elapsed_seconds": 0.050, "pixels": copy.deepcopy(report["checks"][2]["pixels"])}
        for name in ("focus", "resize")}
    return report


def complete_native_report(name):
    return {"passed": True, "status": "passed",
            "checks": [{"name": str(index), "passed": True} for index in range(31)],
            "runtime_trace_enabled": False, "resize_path": "rapid", "visible_pixel_checks_completed": 19,
            "rapid_resize_repetitions": {"required": 10, "completed": 10, "passed": True,
                "iterations": [{"number": number, "passed": True} for number in range(1, 11)]},
            "back_to_back_resize_repetitions": {"required": 10, "completed": 10, "passed": True,
                "iterations": [{"number": number, "passed": True} for number in range(1, 11)]},
            "cleanup": {"process_exited": True, "temporary_directory_removed": True, "returncode": 0},
            "desktop": {"thread": {"name": name}, "input": {"name": name}}}


def complete_cold_runs(directory, identity):
    aggregate = {"required": 8, "completed": 8, "passed": True, "runs": [], **identity}
    for number in range(1, 9):
        path = directory / f"native.cold-{number:02d}.json"
        desktop.write_json(path, complete_native_report(identity["desktop"]))
        aggregate["runs"].append({"number": number, "report": path.name, "passed": True, **identity,
                                  "report_sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
    return aggregate


class WorkspaceContract(unittest.TestCase):
    def test_input_abi_and_unicode_surrogate_events(self):
        self.assertEqual(ctypes.sizeof(workspace.KeyboardInput), 24)
        self.assertEqual(ctypes.sizeof(workspace.MouseInput), 32)
        self.assertEqual(ctypes.sizeof(workspace.Input), 40)
        self.assertEqual(workspace.Input.data.offset, 8)
        calls = []
        window = workspace.Window.__new__(workspace.Window)
        window.hwnd = 123
        def send(count, array, size):
            self.assertEqual(size, 40)
            calls.extend((item.data.keyboard.vk, item.data.keyboard.scan, item.data.keyboard.flags) for item in array)
            return count
        window.user = types.SimpleNamespace(GetForegroundWindow=lambda: 123, SendInput=send,
                                           GetAsyncKeyState=lambda key: 0)
        window.text("A🦀")
        self.assertEqual(calls, [(0, unit, flag) for unit in (65, 0xD83E, 0xDD80) for flag in (4, 6)])
        calls.clear()
        window.chord(0x0D, control=False, shift=False)
        self.assertEqual(calls, [(0x0D, 0, 0), (0x0D, 0, 2)])
        calls.clear()
        window.chord(0x21, control=False)
        self.assertEqual(calls, [(0x10, 0, 0), (0x21, 0, 0), (0x21, 0, 2), (0x10, 0, 2)])
        calls.clear()
        window.chord(0x45)
        self.assertEqual(calls, [(0x11, 0, 0), (0x10, 0, 0), (0x45, 0, 0),
                                 (0x45, 0, 2), (0x10, 0, 2), (0x11, 0, 2)])
        window.user.GetForegroundWindow = lambda: 999
        with self.assertRaises(workspace.native.DesktopBlocked):
            window.text("must not reach another window")

    def test_all_native_checks_images_and_cleanup_are_required(self):
        report = complete_report()
        self.assertTrue(workspace.validate_report(report))
        for field, value in (("passed", False), ("status", "blocked"), ("checks", []),
                             ("visible_pixel_checks_completed", 0), ("screenshots", {}),
                             ("checks", report["checks"][:-1]), ("checks", None)):
            with self.subTest(field=field, value=value):
                self.assertFalse(workspace.validate_report(report | {field: value}))
        for key in ("process_exited", "temporary_directory_removed", "all_probe_processes_exited", "returncode"):
            changed = copy.deepcopy(report)
            changed["cleanup"][key] = 1 if key == "returncode" else False
            self.assertFalse(workspace.validate_report(changed))
        changed = copy.deepcopy(report)
        changed["checks"][4]["passed"] = False
        self.assertFalse(workspace.validate_report(changed))
        for field, value in (("owned", False), ("visible", False), ("rgb", [240, 0, 0])):
            changed = copy.deepcopy(report)
            changed["checks"][2]["pixels"][0][field] = value
            self.assertFalse(workspace.validate_report(changed))
        changed = copy.deepcopy(report)
        changed["checks"][2]["synchronized_layout"]["resize"]["elapsed_seconds"] = 0.151
        self.assertFalse(workspace.validate_report(changed))

    def test_merge_preserves_original_thirty_checks_and_first_error(self):
        original = {"passed": True, "status": "passed",
                    "checks": [{"name": str(index), "passed": True} for index in range(30)]}
        checks = copy.deepcopy(original["checks"])
        failed = complete_report() | {"passed": False, "status": "blocked", "error": "owned window covered"}
        merged = workspace.merge_reports(copy.deepcopy(original), failed)
        self.assertFalse(merged["passed"])
        self.assertEqual(merged["status"], "blocked")
        self.assertEqual(merged["error"], "owned window covered")
        self.assertEqual(merged["checks"], checks)
        original.update({"passed": False, "status": "failed", "error": "original image rejection"})
        merged = workspace.merge_reports(original, complete_report())
        self.assertFalse(merged["passed"])
        self.assertEqual(merged["error"], "original image rejection")
        self.assertEqual(merged["checks"], checks)

    def test_visible_dc_guard_rejects_correct_color_from_a_covered_window(self):
        window = workspace.Window.__new__(workspace.Window)
        window.hwnd, window.process = 123, types.SimpleNamespace(pid=45)
        released = []
        window.user = types.SimpleNamespace(GetDC=lambda hwnd: 10, WindowFromPoint=lambda point: 123,
                                           GetAncestor=lambda hwnd, flag: hwnd,
                                           ReleaseDC=lambda hwnd, dc: released.append((hwnd, dc)))
        window.pid = lambda hwnd: 45 if hwnd == 123 else 99
        window.screen_point = lambda x, y: workspace.native.NativePoint(x + 10, y + 20)
        window.gdi = types.SimpleNamespace(PtVisible=lambda dc, x, y: True,
                                          GetPixel=lambda dc, x, y: 255)
        sample = [(4, 5, [255, 0, 0])]
        self.assertTrue(window.pixels(sample))
        self.assertFalse(window.pixels([(4, 5, [250, 0, 0])]))
        window.user.WindowFromPoint = lambda point: 999
        with self.assertRaises(OSError):
            window.pixels(sample)
        self.assertEqual(window.last_samples[0]["rgb"], [255, 0, 0])
        self.assertFalse(window.last_samples[0]["owned"])
        window.user.WindowFromPoint = lambda point: 123
        window.gdi.PtVisible = lambda dc, x, y: False
        with self.assertRaises(OSError):
            window.pixels(sample)
        self.assertEqual(len(released), 4)

    def test_worker_workspace_failure_keeps_legacy_evidence(self):
        name = "ztty-native-1-" + "a" * 32
        api = types.SimpleNamespace(context=lambda: {
            "station": "WinSta0", "visible": True, "thread": name, "input": name})
        for crash in (False, True):
            with self.subTest(crash=crash), tempfile.TemporaryDirectory() as temporary:
                output = Path(temporary) / "native.json"
                state = Path(temporary) / "worker.json"
                original = complete_native_report(name)
                calls = []
                def legacy(binary, report, arch):
                    calls.append(report)
                    desktop.write_json(report, original)
                    return True
                def fail_workspace(binary, report, arch):
                    self.assertEqual(len(calls), 8)
                    desktop.write_json(report, {"passed": False, "status": "failed",
                                               "checks": [{"name": "partial", "passed": True}],
                                               "error": "actual workspace failure"})
                    if crash:
                        raise RuntimeError("secondary exception")
                    return False
                args = types.SimpleNamespace(worker_desktop=name, binary=Path("ztty.exe"), output=output, arch="arm64")
                with patch.object(desktop, "WindowsAPI", return_value=api), \
                        patch.object(desktop, "source_identity", return_value={"source_sha": "1" * 40, "binary_sha256": "2" * 64}), \
                        patch.dict("sys.modules", {"windows_native": types.SimpleNamespace(run=legacy)}), \
                        patch.object(workspace, "run", side_effect=fail_workspace), \
                        patch.object(desktop.traceback, "print_exc"):
                    self.assertEqual(desktop.worker(args, state), 1)
                result = json.loads(output.read_text(encoding="utf-8"))
                self.assertFalse(result["passed"])
                self.assertTrue(result["cold_start_runs"]["passed"])
                self.assertEqual(len(set(calls)), 8)
                self.assertEqual(result["checks"], original["checks"])
                self.assertEqual(result["workspace"]["error"], "actual workspace failure")
                self.assertEqual(result["error"], "actual workspace failure")

    def test_cold_failure_or_incomplete_cleanup_stops_without_retry_or_workspace(self):
        name = "ztty-native-1-" + "c" * 32
        api = types.SimpleNamespace(context=lambda: {
            "station": "WinSta0", "visible": True, "thread": name, "input": name})
        for fault in ("pixel", "cleanup", "burst"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temporary:
                output, state = Path(temporary) / "native.json", Path(temporary) / "worker.json"
                calls, originals = [], {}
                def native(binary, report, arch):
                    calls.append(report)
                    evidence = complete_native_report(name)
                    if len(calls) == 4:
                        if fault == "pixel":
                            evidence.update({"passed": False, "status": "failed", "error": "cold restore stayed black",
                                             "failed_pixel": {"name": "resize_restores_visible_image"},
                                             "post_failure_snapshot": {"before": {"surface": {"size": [144, 190]}}}})
                        elif fault == "cleanup":
                            evidence["cleanup"]["process_exited"] = False
                        else:
                            evidence["back_to_back_resize_repetitions"]["completed"] = 9
                    desktop.write_json(report, evidence)
                    originals[len(calls)] = report.read_bytes()
                    if len(calls) == 4 and fault == "pixel":
                        raise AssertionError(evidence["error"])
                    return evidence["passed"]
                args = types.SimpleNamespace(worker_desktop=name, binary=Path("ztty.exe"), output=output, arch="arm64")
                with patch.object(desktop, "WindowsAPI", return_value=api), \
                        patch.object(desktop, "source_identity", return_value={"source_sha": "1" * 40, "binary_sha256": "2" * 64}), \
                        patch.dict("sys.modules", {"windows_native": types.SimpleNamespace(run=native)}), \
                        patch.object(workspace, "run") as workspace_run, patch.object(desktop.traceback, "print_exc"):
                    self.assertEqual(desktop.worker(args, state), 1)
                    workspace_run.assert_not_called()
                result = json.loads(output.read_text(encoding="utf-8"))
                self.assertFalse(result["passed"])
                self.assertEqual(result["checks"], complete_native_report(name)["checks"])
                aggregate = result["cold_start_runs"]
                self.assertEqual((aggregate["required"], aggregate["completed"], len(aggregate["runs"])), (8, 3, 4))
                self.assertFalse(aggregate["passed"])
                self.assertFalse(aggregate["runs"][-1]["passed"])
                self.assertEqual(calls, [output] + [output.with_suffix(f".cold-{n:02d}.json") for n in range(2, 5)])
                self.assertEqual(output.with_suffix(".cold-01.json").read_bytes(), originals[1])
                self.assertEqual(calls[-1].read_bytes(), originals[4])
                self.assertEqual(aggregate["runs"][-1]["report_sha256"], hashlib.sha256(originals[4]).hexdigest())
                self.assertFalse(output.with_suffix(".cold-05.json").exists())

    def test_atomic_probe_evidence_retries_only_transient_sharing_failure(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "state.json"
            original = Path.replace
            calls = []
            def replace(source, destination):
                calls.append((source, destination))
                if len(calls) == 1:
                    raise PermissionError("reader temporarily holds the old file")
                return original(source, destination)
            with patch.object(Path, "replace", replace), patch.object(workspace.time, "sleep"):
                workspace.write_json(path, {"pid": 123, "cwd": "会话"})
            self.assertEqual(json.loads(path.read_text(encoding="utf-8")), {"pid": 123, "cwd": "会话"})
            self.assertEqual(len(calls), 2)
            self.assertFalse(path.with_suffix(".json.tmp").exists())

    def test_resize_diagnostic_success_cannot_accept_original_failure(self):
        name = "ztty-native-1-" + "b" * 32
        api = types.SimpleNamespace(context=lambda: {
            "station": "WinSta0", "visible": True, "thread": name, "input": name})
        for cleaned in (True, False):
            with self.subTest(cleaned=cleaned), tempfile.TemporaryDirectory() as temporary:
                output = Path(temporary) / "native.json"
                state = Path(temporary) / "worker.json"
                original = {"passed": False, "status": "failed", "error": "pixel stayed black",
                            "checks": [{"name": "prior", "passed": True}],
                            "failed_pixel": {"name": "resize_restores_visible_image", "actual": [0, 0, 0]},
                            "cleanup": {"process_exited": True, "temporary_directory_removed": True}}
                calls = []
                def legacy(binary, report, arch, trace_runtime=False, staged_resize=False):
                    calls.append((report, trace_runtime, staged_resize))
                    evidence = {"passed": True, "cleanup": {
                        "process_exited": cleaned, "temporary_directory_removed": cleaned}}
                    desktop.write_json(report, evidence if trace_runtime else original)
                    if not trace_runtime:
                        raise AssertionError(original["error"])
                    return True
                args = types.SimpleNamespace(worker_desktop=name, binary=Path("ztty.exe"), output=output, arch="x64")
                with patch.object(desktop, "WindowsAPI", return_value=api), \
                        patch.object(desktop, "source_identity", return_value={"source_sha": "1" * 40, "binary_sha256": "2" * 64}), \
                        patch.dict("sys.modules", {"windows_native": types.SimpleNamespace(run=legacy)}), \
                        patch.object(desktop.traceback, "print_exc"):
                    self.assertEqual(desktop.worker(args, state), 1)
                result = json.loads(output.read_text(encoding="utf-8"))
                self.assertFalse(result["passed"])
                self.assertEqual(result["error"], original["error"])
                self.assertEqual(result["checks"], original["checks"])
                self.assertEqual(result["failed_pixel"], original["failed_pixel"])
                rapid = output.with_suffix(".resize-rapid-diagnostic.json")
                staged = output.with_suffix(".resize-staged-diagnostic.json")
                expected = [(output, False, False), (rapid, True, False)]
                if cleaned:
                    expected.append((staged, True, True))
                self.assertEqual(calls, expected)
                self.assertEqual(len(result["diagnostic_retries"]), 2 if cleaned else 1)
                self.assertTrue(json.loads(rapid.read_text(encoding="utf-8"))["passed"])


if __name__ == "__main__":
    unittest.main()
