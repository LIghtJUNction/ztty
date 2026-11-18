#!/usr/bin/env python3
"""Portable failure contracts, never substitutes for native workspace pixels."""
import copy
import ctypes
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
        window.text("A🦀\r")
        self.assertEqual(calls, [(0, unit, flag) for unit in (65, 0xD83E, 0xDD80, 13) for flag in (4, 6)])
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
                original = {"passed": True, "status": "passed",
                            "checks": [{"name": str(i), "passed": True} for i in range(30)]}
                def legacy(*args):
                    desktop.write_json(output, original)
                    return True
                def fail_workspace(binary, report, arch):
                    desktop.write_json(report, {"passed": False, "status": "failed",
                                               "checks": [{"name": "partial", "passed": True}],
                                               "error": "actual workspace failure"})
                    if crash:
                        raise RuntimeError("secondary exception")
                    return False
                args = types.SimpleNamespace(worker_desktop=name, binary=Path("ztty.exe"), output=output, arch="arm64")
                with patch.object(desktop, "WindowsAPI", return_value=api), \
                        patch.dict("sys.modules", {"windows_native": types.SimpleNamespace(run=legacy)}), \
                        patch.object(workspace, "run", side_effect=fail_workspace), \
                        patch.object(desktop.traceback, "print_exc"):
                    self.assertEqual(desktop.worker(args, state), 1)
                result = json.loads(output.read_text())
                self.assertFalse(result["passed"])
                self.assertEqual(result["checks"], original["checks"])
                self.assertEqual(result["workspace"]["error"], "actual workspace failure")
                self.assertEqual(result["error"], "actual workspace failure")

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


if __name__ == "__main__":
    unittest.main()
