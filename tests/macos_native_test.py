#!/usr/bin/env python3
"""Portable failure and evidence contracts for the native macOS gate.

Run: python3 tests/macos_native_test.py
These ten tests do not claim native window, permission or rendering acceptance.
The separate macos_native.py gate must run on each actual macOS architecture.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import platform
import sys
import tempfile
import unittest

import macos_native as native
from windows_native import png_rgba


class GateContract(unittest.TestCase):
    def make_binary(self, directory):
        # A process fixture for independent initialization. It creates no window
        # and never supplies pixel evidence; permission failures must still fail.
        names = (
            "native_pty_and_pixel_geometry", "cursor_reply", "kitty_capability_query",
            "chunked_rgba", "png", "alternate_screen_image_isolation",
            "malformed_image_text_recovery", "image_deletion",
        )
        binary = directory / "ztty"
        lines = [
            "#!" + sys.executable,
            "import json, os, signal, sys",
            "from pathlib import Path",
            "directory = Path(sys.argv[sys.argv.index('--child-directory') + 1])",
            "assert Path(sys.argv[sys.argv.index('--config') + 1]).is_file()",
            "os.chdir(sys.argv[sys.argv.index('--cwd') + 1])",
            "os.environ['PWD'] = os.getcwd()",
            "state = {'pid': os.getpid(), 'cwd': os.getcwd(), 'pwd': os.environ['PWD'], "
            "'pty': {'cols': 40, 'rows': 12, 'pixel_width': 320, 'pixel_height': 192}, "
            "'protocol': {'passed': True, 'checks': " + repr(dict.fromkeys(names, True)) + "}}",
            "temporary = directory / 'ready.json.tmp'",
            "temporary.write_text(json.dumps(state), encoding='utf-8')",
            "temporary.replace(directory / 'ready.json')",
            "signal.pause()",
        ]
        binary.write_text("\n".join(lines) + "\n", encoding="utf-8")
        binary.chmod(0o700)
        return binary

    def test_good_status(self):
        native.validate_helper_result({"passed": True, "status": "passed"}, 0)

    def test_nonzero_never_passes(self):
        with self.assertRaises(native.NativeFailure):
            native.validate_helper_result({"passed": True, "status": "passed"}, 1)

    def test_false_never_passes(self):
        with self.assertRaises(native.NativeFailure):
            native.validate_helper_result({"passed": False, "status": "failed"}, 0)

    def test_contradictory_status(self):
        with self.assertRaises(native.NativeFailure):
            native.validate_helper_result({"passed": True, "status": "blocked"}, 0)

    def pipeline(self, stdout, exitcode, expected):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            binary = self.make_binary(directory)
            helper = directory / "helper"
            helper.write_text(
                "#!" + sys.executable + "\nimport sys\nsys.stdout.write("
                + repr(stdout) + ")\nsys.exit(" + str(exitcode) + ")\n",
                encoding="utf-8",
            )
            helper.chmod(0o700)
            report, code = native.run(binary, helper, directory / "evidence")
            self.assertEqual(code, expected)
            self.assertFalse(report["passed"])
            self.assertTrue(report["protocol"]["passed"])
            self.assertEqual(len(report["protocol"]["checks"]), 8)
            self.assertTrue(all(report["protocol"]["checks"].values()))
            self.assertTrue(report["checks"]["unicode_cwd_reaches_native_pty_child"])
            self.assertTrue(report["checks"]["relative_config_resolves_before_cwd"])
            self.assertEqual(report["remaining_owned_processes"], [])
            self.assertEqual(report["pixel_readback"], "not_performed")
            saved = json.loads((directory / "evidence.permissions.json").read_text(encoding="utf-8"))
            self.assertFalse(saved["passed"])
            self.assertEqual(
                (directory / "evidence.permissions.stdout.log").read_text(encoding="utf-8"),
                stdout,
            )
            return report

    def test_permission_block_preserves_evidence(self):
        report = self.pipeline(json.dumps({
            "passed": False, "status": "blocked",
            "error": "fixture: Screen Recording is unavailable",
        }), 2, 2)
        self.assertEqual(report["status"], "blocked")

    def test_malformed_helper_json_preserves_evidence(self):
        report = self.pipeline("this is not JSON\n", 0, 1)
        self.assertEqual(report["status"], "failed")

    def test_forged_permission_success_rejected(self):
        value = {
            "passed": True, "status": "passed", "architecture": platform.machine(),
            "process_translated": False,
            "permissions": {"screen_capture": False, "accessibility": True},
            "pixel_layout_vector": "passed",
        }
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            binary = self.make_binary(directory)
            helper = directory / "helper"
            helper.write_text(
                "#!" + sys.executable + "\nprint(" + repr(json.dumps(value)) + ")\n",
                encoding="utf-8",
            )
            helper.chmod(0o700)
            report, code = native.run(binary, helper, directory / "evidence")
            self.assertEqual(code, 1)
            self.assertFalse(report["passed"])
            self.assertTrue(report["protocol"]["passed"])
            self.assertEqual(len(report["protocol"]["checks"]), 8)
            self.assertTrue(all(report["protocol"]["checks"].values()))
            self.assertTrue(report["checks"]["unicode_cwd_reaches_native_pty_child"])
            self.assertTrue(report["checks"]["relative_config_resolves_before_cwd"])
            self.assertEqual(report["remaining_owned_processes"], [])
            self.assertIn("both permissions", report["error"])

    def test_png_dimensions_and_hash(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "window.png"
            blob = png_rgba(2, 2, bytes([
                255, 0, 0, 255, 0, 255, 0, 255,
                0, 0, 255, 255, 255, 255, 255, 255,
            ]))
            path.write_bytes(blob)
            actual = native.screenshot_metadata(path)
            self.assertEqual([actual["width"], actual["height"]], [2, 2])
            self.assertEqual(actual["sha256"], hashlib.sha256(blob).hexdigest())

    def test_missing_png_signature_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "bad.png"
            path.write_bytes(b"not a PNG")
            with self.assertRaises(AssertionError):
                native.screenshot_metadata(path)

    def test_unicode_atomic_control_record(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "含 空格.json"
            value = {"cwd": "含 空格与Unicode", "size": [40, 12]}
            native.write_json(path, value)
            self.assertEqual(json.loads(path.read_text(encoding="utf-8")), value)
            self.assertFalse(path.with_name(path.name + ".tmp").exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
