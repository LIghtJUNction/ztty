#!/usr/bin/env python3
"""Portable failure contracts; native desktop/pixel evidence comes from the CI gate."""
import copy
import ctypes
import hashlib
import json
import os
from pathlib import Path
import tempfile
import types
import unittest
from unittest.mock import patch

import windows_desktop as desktop
from windows_workspace_test import complete_report, complete_native_report, complete_cold_runs


class FakeAPI:
    """Model only owned resources and make any unowned operation fail the test."""
    def __init__(self, fault=None, initial=None):
        self.fault, self.time, self.calls = fault, 0.0, []
        self.state = initial or {"station": "WinSta0", "visible": True, "thread": "Default", "input": "Default"}
        self.owned, self.assigned, self.running, self.resumed = None, False, False, False
        self.active, self.finished = [], False
        self.input_reads = 0

    def context(self):
        return dict(self.state)

    def name(self, handle):
        return {10: self.state["thread"], 11: "Default", 12: self.owned}[handle]

    def thread_desktop(self):
        return 10

    def open_input(self, switch=False):
        self.calls.append(("open_input", switch))
        return 11

    def input_name(self):
        if self.fault == "secure_mid_run" and self.resumed:
            self.state["input"] = "Winlogon"
        return self.state["input"]

    def create_desktop(self, name):
        self.calls.append(("create_desktop", name))
        self.owned = name
        return 12

    def create_job(self):
        self.calls.append(("create_job",))
        return 20

    def limit_job(self, job):
        assert job == 20
        self.calls.append(("limit_job", job))

    def create_suspended(self, command, name, stdout, stderr, process):
        assert name == self.owned
        self.calls.append(("create_suspended",))
        process.process, process.thread, process.pid, process.tid = 30, 31, 100, 101
        self.running = True

    def assign(self, job, process):
        assert job == 20 and process.process == 30
        self.calls.append(("assign",))
        if self.fault == "assign":
            raise OSError("injected nested Job refusal")
        self.assigned = True
        self.active = [100]

    def job_pids(self, job):
        assert job == 20
        return list(self.active)

    def switch(self, handle):
        assert handle in (11, 12), "cannot switch an unowned or secure desktop"
        self.calls.append(("switch", handle))
        if self.fault == "switch" and handle == 12:
            raise OSError("injected SwitchDesktop denial")
        if self.fault == "restore" and handle == 11:
            raise OSError("injected original desktop restoration denial")
        self.state["input"] = self.owned if handle == 12 else "Default"

    def restore_thread(self, handle):
        assert handle == 10
        self.calls.append(("restore_thread", handle))

    def resume(self, process):
        assert self.assigned and self.state["input"] == self.owned
        self.calls.append(("resume",))
        self.resumed = True

    def poll(self, process, milliseconds=0):
        assert process.process == 30
        self.time += milliseconds / 1000
        if not self.running:
            return 0
        if self.resumed and milliseconds and self.fault not in ("timeout", "secure_mid_run"):
            self.running = False
            self.active = [102] if self.fault == "lingering" else []
            return 7 if self.fault == "worker_failure" else 0
        return None

    def terminate_job(self, job):
        assert job == 20
        self.calls.append(("terminate_job", job))
        self.active, self.running = [], False

    def terminate_process(self, process):
        assert process.process == 30 and not self.assigned
        self.calls.append(("terminate_process", 30))
        self.running = False

    def close_handle(self, handle):
        assert handle in (20, 30, 31)
        self.calls.append(("close_handle", handle))

    def close_desktop(self, handle):
        assert handle in (11, 12), "GetThreadDesktop is a borrowed handle"
        if handle == 12:
            assert self.state["input"] == "Default", "restore before CloseDesktop"
        self.calls.append(("close_desktop", handle))

    def pause(self, amount):
        self.time += amount


class DesktopContract(unittest.TestCase):
    def execute(self, api):
        return desktop.run_isolated(
            api, ["owned-python", "windows_desktop.py"], "ztty-native-1-" + "a" * 32,
            Path("stdout.log"), Path("stderr.log"), 1,
            clock=lambda: api.time, pause=api.pause)

    def test_native_64_bit_abi_and_explicit_ci_opt_in(self):
        self.assertEqual(ctypes.sizeof(desktop.StartupInfo), 104)
        self.assertEqual(ctypes.sizeof(desktop.StartupInfoEx), 112)
        self.assertEqual(ctypes.sizeof(desktop.ProcessInfo), 24)
        self.assertEqual(ctypes.sizeof(desktop.JobBasicLimit), 64)
        self.assertEqual(ctypes.sizeof(desktop.JobExtendedLimit), 144)
        self.assertEqual(desktop.StartupInfo.desktop.offset, 16)
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(desktop.DesktopBlocked):
                desktop.require_ci(True)
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "RUNNER_OS": "Windows", "RUNNER_LABEL": "windows-11-arm"}):
            with self.assertRaises(desktop.DesktopBlocked):
                desktop.require_ci(False)
            desktop.require_ci(True)

    def test_secure_noninteractive_and_nonmatching_context_never_create_resources(self):
        baseline = {"station": "WinSta0", "visible": True, "thread": "Default", "input": "Default"}
        for field, value in (("station", "Service-0"), ("visible", False),
                             ("thread", "Other"), ("input", "Winlogon")):
            with self.subTest(field=field):
                api = FakeAPI(initial=baseline | {field: value})
                report = self.execute(api)
                self.assertFalse(report["passed"])
                self.assertEqual(report["status"], "blocked")
                self.assertEqual(api.calls, [])

    def test_success_assigns_before_resume_and_restores_before_close(self):
        api = FakeAPI()
        report = self.execute(api)
        self.assertTrue(report["passed"], report)
        events = [call[0] for call in api.calls]
        self.assertLess(events.index("create_suspended"), events.index("assign"))
        self.assertLess(events.index("assign"), events.index("resume"))
        self.assertLess(api.calls.index(("switch", 11)), api.calls.index(("close_desktop", 12)))
        self.assertNotIn("terminate_job", events)
        self.assertNotIn("terminate_process", events)
        self.assertEqual(report["cleanup"]["job_pids_after"], [])
        self.assertTrue(report["cleanup"]["input_restored"])
        self.assertTrue(report["cleanup"]["owned_desktop_closed"])
        self.assertEqual(report["inherited_handles"], ["NUL stdin", "stdout log", "stderr log"])

    def test_job_refusal_and_switch_denial_never_resume_and_reap_only_owned_process(self):
        for fault in ("assign", "switch"):
            with self.subTest(fault=fault):
                api = FakeAPI(fault)
                report = self.execute(api)
                self.assertFalse(report["passed"])
                self.assertEqual(report["status"], "blocked")
                self.assertFalse(report["worker_resumed"])
                self.assertNotIn(("resume",), api.calls)
                expected = ("terminate_process", 30) if fault == "assign" else ("terminate_job", 20)
                self.assertIn(expected, api.calls)
                self.assertTrue(report["cleanup"]["worker_exited"])
                self.assertTrue(report["cleanup"]["job_empty"])

    def test_timeout_and_lingering_children_fail_and_empty_the_owned_job(self):
        for fault in ("timeout", "lingering"):
            with self.subTest(fault=fault):
                api = FakeAPI(fault)
                report = self.execute(api)
                self.assertFalse(report["passed"])
                self.assertEqual(report["status"], "failed")
                self.assertIn(("terminate_job", 20), api.calls)
                self.assertTrue(report["cleanup"]["forced_termination"])
                self.assertTrue(report["cleanup"]["job_empty"])
                self.assertTrue(report["cleanup"]["input_restored"])

    def test_secure_desktop_change_is_not_switched_or_unlocked(self):
        api = FakeAPI("secure_mid_run")
        report = self.execute(api)
        self.assertFalse(report["passed"])
        self.assertEqual(report["status"], "blocked")
        self.assertNotIn(("switch", 11), api.calls)
        self.assertNotIn(("close_desktop", 12), api.calls)
        self.assertIn(("terminate_job", 20), api.calls)
        self.assertFalse(report["cleanup"]["input_restored"])
        self.assertTrue(report["cleanup"]["job_empty"])

    def test_restoration_failure_or_worker_nonzero_never_pass(self):
        for fault in ("restore", "worker_failure"):
            with self.subTest(fault=fault):
                api = FakeAPI(fault)
                report = self.execute(api)
                self.assertFalse(report["passed"])
                if fault == "restore":
                    self.assertEqual(report["status"], "blocked")
                    self.assertNotIn(("close_desktop", 12), api.calls)
                else:
                    self.assertEqual(report["worker_exit_code"], 7)
                    self.assertIn("exited with 7", report["error"])
                self.assertIn(("close_handle", 20), api.calls)

    def test_native_pixels_worker_identity_and_cleanup_must_all_pass(self):
        name = "ztty-native-1-" + "a" * 32
        identity = {"source_sha": "1" * 40, "binary_sha256": "2" * 64, "desktop": name}
        controller = {"passed": True, "status": "passed", "worker_pid": 100, **identity}
        worker = {"passed": True, "pid": 100, "desktop": name,
                  "source_sha": identity["source_sha"], "binary_sha256": identity["binary_sha256"],
                  "context": {"station": "WinSta0", "visible": True, "thread": name, "input": name}}
        native = complete_native_report(name)
        native["workspace"] = complete_report() | {"desktop": native["desktop"]}
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            native["cold_start_runs"] = complete_cold_runs(directory, identity)
            def combine(value=native, desktop_state=controller, worker_state=worker):
                return desktop.combine(copy.deepcopy(value), desktop_state, worker_state, directory)
            self.assertTrue(combine()["passed"])
            for changed in (native | {"passed": False}, native | {"checks": []},
                            native | {"visible_pixel_checks_completed": 0},
                            native | {"cleanup": {"process_exited": False}},
                            native | {"workspace": {}}, native | {"cold_start_runs": {}},
                            native | {"desktop": {"thread": {"name": "Default"}}}):
                self.assertFalse(combine(changed)["passed"])
            self.assertFalse(combine(desktop_state=controller | {"passed": False})["passed"])
            for changed in (worker | {"pid": 99}, worker | {"source_sha": "3" * 40},
                            worker | {"binary_sha256": "4" * 64}):
                self.assertFalse(combine(worker_state=changed)["passed"])
            failed = native | {"passed": False, "status": "blocked", "error": "original visible pixel failure"}
            result = combine(failed, controller | {"passed": False, "status": "failed"})
            self.assertEqual(result["error"], "original visible pixel failure")
            self.assertEqual(result["status"], "blocked")

    def test_cold_summary_cannot_accept_missing_changed_or_incomplete_raw_evidence(self):
        identity = {"source_sha": "1" * 40, "binary_sha256": "2" * 64, "desktop": "ztty-native-1-" + "a" * 32}
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            aggregate = complete_cold_runs(directory, identity)
            self.assertTrue(desktop.validate_cold_runs(aggregate, identity, directory))
            for changed in (aggregate | {"required": None}, aggregate | {"completed": 7},
                            aggregate | {"runs": aggregate["runs"][:-1]},
                            aggregate | {"source_sha": "3" * 40}):
                self.assertFalse(desktop.validate_cold_runs(changed, identity, directory))
            for key, value in (("source_sha", "3" * 40), ("binary_sha256", "4" * 64),
                               ("report_sha256", "5" * 64), ("passed", False),
                               ("report", aggregate["runs"][0]["report"])):
                changed = copy.deepcopy(aggregate)
                changed["runs"][1][key] = value
                self.assertFalse(desktop.validate_cold_runs(changed, identity, directory))
            path = directory / aggregate["runs"][-1]["report"]
            raw = path.read_bytes()
            path.write_bytes(raw + b" ")
            self.assertFalse(desktop.validate_cold_runs(aggregate, identity, directory))
            # Even a matching digest cannot accept a truncated resize gate,
            # traced run, wrong desktop, or unfinished process cleanup.
            for field, value in (("rapid_resize_repetitions", {"passed": True, "required": 10, "completed": 9}),
                                 ("back_to_back_resize_repetitions", None),
                                 ("back_to_back_resize_repetitions", {"passed": True, "required": 10, "completed": 9}),
                                 ("back_to_back_resize_repetitions", {"passed": True, "required": 10, "completed": 10,
                                    "iterations": [{"number": n, "passed": n != 7} for n in range(1, 11)]}),
                                 ("runtime_trace_enabled", True), ("checks", []),
                                 ("desktop", {"thread": {"name": "Default"}}),
                                 ("cleanup", {"process_exited": False, "temporary_directory_removed": True})):
                report = json.loads(raw)
                report[field] = value
                desktop.write_json(path, report)
                changed = copy.deepcopy(aggregate)
                changed["runs"][-1]["report_sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
                self.assertFalse(desktop.validate_cold_runs(changed, identity, directory))
            path.unlink()
            self.assertFalse(desktop.validate_cold_runs(aggregate, identity, directory))

    def test_worker_failure_preserves_original_native_checks_and_error(self):
        name = "ztty-native-1-" + "a" * 32
        api = FakeAPI(initial={"station": "WinSta0", "visible": True, "thread": name, "input": name})
        with tempfile.TemporaryDirectory() as temporary:
            output, state_path = Path(temporary) / "native.json", Path(temporary) / "worker.json"
            original = {"passed": False, "status": "blocked", "checks": [{"name": "protocol", "passed": True}],
                        "error": "original pixel rejection", "cleanup": {"process_exited": True}}
            def fail_native(*args):
                desktop.write_json(output, original)
                raise RuntimeError("original pixel rejection")
            args = types.SimpleNamespace(worker_desktop=name, binary=Path("ztty.exe"), output=output, arch="arm64")
            module = types.SimpleNamespace(run=fail_native)
            with patch.object(desktop, "WindowsAPI", return_value=api), \
                    patch.object(desktop, "source_identity", return_value={"source_sha": "1" * 40, "binary_sha256": "2" * 64}), \
                    patch.dict("sys.modules", {"windows_native": module}), patch.object(desktop.traceback, "print_exc"):
                self.assertEqual(desktop.worker(args, state_path), 1)
            self.assertEqual(json.loads(output.read_text()), original)
            self.assertTrue(json.loads(state_path.read_text())["passed"])


if __name__ == "__main__":
    unittest.main()
