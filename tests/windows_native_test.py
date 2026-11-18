#!/usr/bin/env python3
"""Contract checks for resource disposal, separate from native pixel acceptance."""
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import windows_native as native


class LockedDirectory:
    """Model Windows' live-cwd lock while still deleting an actual directory."""
    def __init__(self, directory, process):
        self.directory = directory
        self.name = directory.name
        self.process = process
        self.attempted = False

    def cleanup(self):
        self.attempted = True
        if self.process.poll() is None:
            raise PermissionError("live process still owns this cwd")
        self.directory.cleanup()


class CleanupContract(unittest.TestCase):
    def test_live_child_exits_before_directory_removal_on_success_and_failure(self):
        for passed in (True, False):
            with self.subTest(passed=passed):
                directory = tempfile.TemporaryDirectory(prefix="ztty-cleanup-contract-")
                process = subprocess.Popen(
                    [sys.executable, "-c", "import sys; sys.stdin.readline()"],
                    cwd=directory.name, stdin=subprocess.PIPE,
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                locked = LockedDirectory(directory, process)
                report = {"passed": passed, "status": "running" if passed else "blocked"}
                if not passed:
                    report["error"] = "owned window is covered"
                def close():
                    process.stdin.write(b"\n")
                    process.stdin.close()
                try:
                    native.cleanup_owned(process, close, locked, report)
                    self.assertIsNotNone(process.poll())
                    self.assertTrue(locked.attempted)
                    self.assertFalse(Path(directory.name).exists())
                    self.assertEqual(report["passed"], passed)
                    self.assertTrue(report["cleanup"]["process_exited"])
                    self.assertTrue(report["cleanup"]["temporary_directory_removed"])
                    if not passed:
                        self.assertEqual(report["error"], "owned window is covered")
                        self.assertEqual(report["status"], "blocked")
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait(timeout=5)
                    if process.stdin and not process.stdin.closed:
                        process.stdin.close()
                    directory.cleanup()

    def test_forced_shutdown_is_recorded_and_cannot_pass(self):
        class UnresponsiveProcess:
            returncode = None
            terminated = False
            def wait(self, timeout):
                if not self.terminated:
                    raise subprocess.TimeoutExpired("owned-test-child", timeout)
                self.returncode = 1
            def terminate(self):
                self.terminated = True
            def poll(self):
                return self.returncode
        with tempfile.TemporaryDirectory() as outer:
            directory = tempfile.TemporaryDirectory(dir=outer)
            process = UnresponsiveProcess()
            locked = LockedDirectory(directory, process)
            report = {"passed": True}
            native.cleanup_owned(process, lambda: None, locked, report)
            self.assertTrue(process.terminated)
            self.assertFalse(report["passed"])
            self.assertTrue(report["shutdown_timeout"])
            self.assertTrue(report["cleanup"]["temporary_directory_removed"])

    def test_cleanup_failure_preserves_original_failure(self):
        class BrokenDirectory:
            name = "unused"
            def cleanup(self):
                raise PermissionError("injected disposal failure")
        report = {"passed": False, "error": "original pixel failure"}
        native.cleanup_owned(None, lambda: self.fail("no child to close"), BrokenDirectory(), report)
        self.assertEqual(report["error"], "original pixel failure")
        self.assertIn("injected disposal failure", report["cleanup"]["directory_error"])
        self.assertFalse(report["passed"])

    def test_window_close_error_still_waits_and_cleans_directory(self):
        calls = []
        class Process:
            returncode = None
            def wait(self, timeout):
                calls.append("wait")
                self.returncode = 0
            def poll(self):
                return self.returncode
        with tempfile.TemporaryDirectory() as outer:
            directory = tempfile.TemporaryDirectory(dir=outer)
            locked = LockedDirectory(directory, Process())
            def fail_close():
                calls.append("close")
                raise OSError("injected close failure")
            report = {"passed": True, "error": "preserved"}
            native.cleanup_owned(locked.process, fail_close, locked, report)
            self.assertEqual(calls, ["close", "wait"])
            self.assertTrue(locked.attempted)
            self.assertTrue(report["cleanup"]["temporary_directory_removed"])
            self.assertIn("injected close failure", report["cleanup"]["window_close_error"])
            self.assertEqual(report["error"], "preserved")
            self.assertFalse(report["passed"])

    def test_failure_before_process_creation_removes_directory(self):
        directory = tempfile.TemporaryDirectory()
        report = {"passed": False, "error": "startup failed"}
        native.cleanup_owned(None, lambda: self.fail("no process"), directory, report)
        self.assertTrue(report["cleanup"]["process_exited"])
        self.assertTrue(report["cleanup"]["temporary_directory_removed"])
        self.assertEqual(report["error"], "startup failed")


if __name__ == "__main__":
    unittest.main()
