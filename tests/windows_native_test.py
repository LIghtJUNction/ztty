#!/usr/bin/env python3
"""Contract checks for resource disposal, separate from native pixel acceptance."""
import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

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


class Clock:
    def __init__(self):
        self.now = 0.0
        self.sleeps = []

    def monotonic(self):
        return self.now

    def sleep(self, seconds):
        self.sleeps.append(seconds)
        self.now += seconds


class ExitedProcess:
    returncode = 0

    def wait(self, timeout):
        return self.returncode

    def poll(self):
        return self.returncode


def windows_error(code):
    # OSError exposes winerror only on Windows; keep portable contracts exact.
    error = PermissionError(13, "injected Windows file conflict")
    error.winerror = code
    return error


class ConflictedDirectory:
    def __init__(self, directory, error, failures=None):
        self.directory = directory
        self.name = directory.name
        self.error = error
        self.failures = failures
        self.attempts = 0

    def cleanup(self):
        self.attempts += 1
        if self.failures is None or self.attempts <= self.failures:
            raise self.error
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

    def cleanup_with_clock(self, directory, report, platform="win32", process=None):
        clock = Clock()
        with (mock.patch.object(native.sys, "platform", platform),
              mock.patch.object(native.time, "monotonic", clock.monotonic),
              mock.patch.object(native.time, "sleep", clock.sleep)):
            native.cleanup_owned(process or ExitedProcess(), lambda: None, directory, report)
        return clock

    def test_transient_windows_conflicts_recover_without_erasing_pixel_failure(self):
        for code in (32, 33):
            for passed in (True, False):
                with self.subTest(winerror=code, passed=passed), tempfile.TemporaryDirectory() as outer:
                    directory = tempfile.TemporaryDirectory(dir=outer)
                    conflict = ConflictedDirectory(directory, windows_error(code), failures=2)
                    report = {"passed": passed, "status": "passed" if passed else "failed"}
                    if not passed:
                        report.update(error="original pixel failure", failed_pixel={"actual": [20, 20, 20]})
                    clock = self.cleanup_with_clock(conflict, report)
                    cleanup = report["cleanup"]
                    removal = cleanup["directory_removal"]
                    self.assertFalse(Path(directory.name).exists())
                    self.assertTrue(cleanup["temporary_directory_removed"])
                    self.assertNotIn("directory_error", cleanup)
                    self.assertEqual(removal["attempts"], 3)
                    self.assertEqual(removal["first_error"]["winerror"], code)
                    self.assertEqual(removal["first_error"]["errno"], 13)
                    self.assertEqual(clock.sleeps, [0.05, 0.05])
                    self.assertAlmostEqual(removal["elapsed_seconds"], 0.1)
                    self.assertEqual(report["passed"], passed)
                    if not passed:
                        self.assertEqual(report["error"], "original pixel failure")
                        self.assertEqual(report["failed_pixel"], {"actual": [20, 20, 20]})
                        self.assertEqual(report["status"], "failed")

    def test_persistent_windows_lock_fails_at_fixed_deadline(self):
        with tempfile.TemporaryDirectory() as outer:
            directory = tempfile.TemporaryDirectory(dir=outer)
            conflict = ConflictedDirectory(directory, windows_error(32))
            report = {"passed": True}
            clock = self.cleanup_with_clock(conflict, report)
            cleanup = report["cleanup"]
            removal = cleanup["directory_removal"]
            self.assertFalse(report["passed"])
            self.assertFalse(cleanup["temporary_directory_removed"])
            self.assertTrue(Path(directory.name).exists())
            self.assertEqual(removal["first_error"]["winerror"], 32)
            self.assertEqual(removal["attempts"], conflict.attempts)
            self.assertGreater(removal["attempts"], 1)
            self.assertLessEqual(removal["attempts"], 41)
            self.assertAlmostEqual(removal["elapsed_seconds"], 2.0)
            self.assertAlmostEqual(sum(clock.sleeps), 2.0)
            self.assertTrue(all(0 < delay <= 0.05 for delay in clock.sleeps))
            self.assertIn("injected Windows file conflict", cleanup["directory_error"])
            directory.cleanup()

    def test_first_conflict_is_retained_when_a_later_error_is_not_retryable(self):
        class ChangingDirectory(ConflictedDirectory):
            def cleanup(self):
                self.error = windows_error(32 if self.attempts == 0 else 5)
                super().cleanup()

        with tempfile.TemporaryDirectory() as outer:
            directory = tempfile.TemporaryDirectory(dir=outer)
            conflict = ChangingDirectory(directory, None)
            report = {"passed": False, "error": "original pixel failure"}
            clock = self.cleanup_with_clock(conflict, report)
            removal = report["cleanup"]["directory_removal"]
            self.assertEqual(conflict.attempts, 2)
            self.assertEqual(removal["first_error"]["winerror"], 32)
            self.assertEqual(clock.sleeps, [0.05])
            self.assertFalse(report["passed"])
            self.assertEqual(report["error"], "original pixel failure")
            self.assertFalse(report["cleanup"]["temporary_directory_removed"])
            directory.cleanup()

    def test_other_errors_and_non_windows_conflicts_are_not_retried(self):
        cases = [("win32", windows_error(5)), ("win32", PermissionError(13, "denied")),
                 ("win32", RuntimeError("not an OS lock")), ("linux", windows_error(32))]
        for platform, error in cases:
            with self.subTest(platform=platform, error=repr(error)), tempfile.TemporaryDirectory() as outer:
                directory = tempfile.TemporaryDirectory(dir=outer)
                conflict = ConflictedDirectory(directory, error)
                report = {"passed": True}
                clock = self.cleanup_with_clock(conflict, report, platform)
                self.assertEqual(conflict.attempts, 1)
                self.assertEqual(clock.sleeps, [])
                self.assertFalse(report["passed"])
                self.assertFalse(report["cleanup"]["temporary_directory_removed"])
                self.assertEqual(report["cleanup"]["directory_removal"]["first_error"]["repr"], repr(error))
                directory.cleanup()

    def test_no_directory_attempt_without_confirmed_process_exit(self):
        class UnconfirmedProcess(ExitedProcess):
            returncode = None

            def wait(self, timeout):
                raise OSError("owned process wait failed")

        with tempfile.TemporaryDirectory() as outer:
            directory = tempfile.TemporaryDirectory(dir=outer)
            conflict = ConflictedDirectory(directory, windows_error(32), failures=0)
            report = {"passed": True}
            clock = self.cleanup_with_clock(conflict, report, process=UnconfirmedProcess())
            cleanup = report["cleanup"]
            self.assertEqual(conflict.attempts, 0)
            self.assertEqual(clock.sleeps, [])
            self.assertFalse(cleanup["process_exited"])
            self.assertFalse(cleanup["temporary_directory_removed"])
            self.assertEqual(cleanup["directory_removal"]["attempts"], 0)
            self.assertIn("not confirmed", cleanup["directory_removal"]["skipped"])
            self.assertFalse(report["passed"])
            directory.cleanup()

    def test_successful_cleanup_call_must_actually_remove_directory(self):
        with tempfile.TemporaryDirectory() as outer:
            class InertDirectory:
                name = outer

                def cleanup(self):
                    pass

            report = {"passed": True}
            clock = self.cleanup_with_clock(InertDirectory(), report)
            self.assertFalse(report["passed"])
            self.assertFalse(report["cleanup"]["temporary_directory_removed"])
            self.assertEqual(report["cleanup"]["directory_removal"]["attempts"], 1)
            self.assertEqual(clock.sleeps, [])


class ProbeFailureEvidenceContract(unittest.TestCase):
    def test_successful_child_operation_has_no_diagnostic_file(self):
        with tempfile.TemporaryDirectory() as directory:
            operation = mock.Mock(return_value=17)
            self.assertEqual(native.child_failure_guard(directory, "main", operation), 17)
            operation.assert_called_once_with()
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_main_and_reader_errors_are_bounded_and_reraise_the_original(self):
        with tempfile.TemporaryDirectory() as directory:
            for phase in ("main", "reader"):
                error = PermissionError(13, "\x00\U0001f980" * 20000)
                error.winerror = 32
                with self.subTest(phase=phase), self.assertRaises(PermissionError) as raised:
                    native.child_failure_guard(directory, phase, mock.Mock(side_effect=error))
                self.assertIs(raised.exception, error)
                raw = (Path(directory) / f"child-{phase}-error.json").read_bytes()
                self.assertLessEqual(len(raw), native.CHILD_ERROR_LIMIT)
                record = json.loads(raw)
                self.assertEqual(record["phase"], phase)
                self.assertEqual(record["type"], "PermissionError")
                self.assertEqual((record["errno"], record["winerror"]), (13, 32))
                self.assertTrue(record["truncated"])
                self.assertGreater(record["pid"], 0)
                self.assertGreater(record["parent_pid"], 0)
                self.assertIn("PermissionError", record["traceback"])

    def test_sidecar_write_failure_does_not_replace_the_original_exception(self):
        with tempfile.TemporaryDirectory() as directory:
            original = RuntimeError("original probe failure")
            with (mock.patch.object(Path, "write_bytes", side_effect=OSError("diagnostic disk failure")),
                  self.assertRaises(RuntimeError) as raised):
                native.child_failure_guard(directory, "main", mock.Mock(side_effect=original))
            self.assertIs(raised.exception, original)
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_sidecars_survive_fixture_cleanup_and_oversize_is_rejected(self):
        with tempfile.TemporaryDirectory() as outer:
            output = Path(outer) / "native.json"
            directory = tempfile.TemporaryDirectory(dir=outer)
            fixture = Path(directory.name)
            raw = b'{"phase":"reader","type":"OSError","message":"probe read failed"}\n'
            (fixture / "child-reader-error.json").write_bytes(raw)
            (fixture / "keyboard.bin").write_bytes(b"unrelated fixture bytes")
            report = {"passed": False, "error": "original PTY write timeout"}
            native.archive_child_failures(fixture, output, report)
            native.cleanup_owned(None, lambda: None, directory, report)
            self.assertFalse(fixture.exists())
            self.assertEqual(report["error"], "original PTY write timeout")
            self.assertFalse(report["passed"])
            self.assertEqual(report["child_errors"], [{"phase": "reader",
                "path": str(Path(outer) / "native.child-reader-error.json"),
                "bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}])
            self.assertEqual((Path(outer) / "native.child-reader-error.json").read_bytes(), raw)
            self.assertFalse((Path(outer) / "keyboard.bin").exists())
            with tempfile.TemporaryDirectory(dir=outer) as oversized:
                (Path(oversized) / "child-main-error.json").write_bytes(b"x" * (native.CHILD_ERROR_LIMIT + 1))
                report = {"passed": True}
                native.archive_child_failures(oversized, output, report)
                self.assertFalse(report["passed"])
                self.assertNotIn("child_errors", report)
                self.assertIn("fixed bound", report["child_error_capture_errors"][0]["error"])
                self.assertFalse((Path(outer) / "native.child-main-error.json").exists())

    def test_write_failure_snapshot_observes_once_without_modifying_fixture(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory)
            payload = b"\x1b_Gbroken-fixture\x1b\\"
            (fixture / "command-9.bin").write_bytes(payload)
            process = mock.Mock(pid=912)
            process.poll.return_value = 1
            report = {"passed": False, "error": "original write error"}
            native.record_output_write_failure(report, process, fixture, 9, payload, 913)
            process.poll.assert_called_once_with()
            self.assertEqual(report["output_write_failure"], {"sequence": 9,
                "payload_bytes": len(payload), "payload_sha256": hashlib.sha256(payload).hexdigest(),
                "child_pid": 913, "ztty_pid": 912, "command_exists": True,
                "staging_exists": False, "done_exists": False, "ztty_returncode": 1})
            self.assertEqual(report["error"], "original write error")
            self.assertEqual((fixture / "command-9.bin").read_bytes(), payload)
            self.assertEqual([path.name for path in fixture.iterdir()], ["command-9.bin"])


if __name__ == "__main__":
    unittest.main()
