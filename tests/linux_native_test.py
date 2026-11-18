#!/usr/bin/env python3
"""Failure-path regressions for native release acceptance; ELF bytes are fixtures."""
import contextlib
import ctypes
import errno
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import struct
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
from unittest import mock

import linux_native
import workspaces


class ClosedIdentityTests(unittest.TestCase):
    def test_already_closed_process_disappearing_during_read_is_gone(self):
        for code in (errno.ENOENT, errno.ESRCH):
            with self.subTest(errno=code):
                read_start = mock.Mock(side_effect=OSError(code, os.strerror(code)))
                self.assertTrue(workspaces.closed_identity_gone(41, 900, read_start))
                read_start.assert_called_once_with(41)

    def test_same_live_identity_still_requires_waiting(self):
        read_start = mock.Mock(return_value=900)
        self.assertFalse(workspaces.closed_identity_gone(41, 900, read_start))
        read_start.assert_called_once_with(41)

    def test_reused_pid_does_not_keep_old_identity_alive(self):
        read_start = mock.Mock(return_value=901)
        self.assertTrue(workspaces.closed_identity_gone(41, 900, read_start))
        read_start.assert_called_once_with(41)

    def test_permission_and_other_read_errors_remain_failures(self):
        for code in (errno.EACCES, errno.EIO):
            with self.subTest(errno=code):
                failure = OSError(code, os.strerror(code))
                read_start = mock.Mock(side_effect=failure)
                with self.assertRaises(OSError) as raised:
                    workspaces.closed_identity_gone(41, 900, read_start)
                self.assertIs(raised.exception, failure)
                read_start.assert_called_once_with(41)


class ArchitectureTests(unittest.TestCase):
    def header(self, machine, elf_class=2):
        header = bytearray(64)
        header[:7] = b"\x7fELF" + bytes((elf_class, 1, 1))
        struct.pack_into("<HHI", header, 16, 3, machine, 1)
        return header

    def inspect(self, data, architecture):
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / "fixture.elf"
            binary.write_bytes(data)
            return linux_native.inspect_elf(binary, architecture)

    def test_headers_identify_each_architecture(self):
        for architecture, machine in (("x86_64", 62), ("aarch64", 183)):
            with self.subTest(architecture=architecture):
                self.assertEqual(self.inspect(self.header(machine), architecture)["machine"], machine)

    def test_wrong_architecture_is_rejected_before_execution(self):
        with self.assertRaisesRegex(ValueError, "does not match"):
            self.inspect(self.header(62), "aarch64")
        with self.assertRaisesRegex(ValueError, "does not match"):
            self.inspect(self.header(183), "x86_64")

    def test_32_bit_and_truncated_headers_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "64-bit"):
            self.inspect(self.header(62, elf_class=1), "x86_64")
        with self.assertRaisesRegex(ValueError, "complete ELF64"):
            self.inspect(self.header(62)[:20], "x86_64")


class SourceTests(unittest.TestCase):
    def test_pinned_checkout_rejects_wrong_commit_and_both_dirty_states(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "main.rs").write_text("fn main() {}\n")
            for command in (["git", "init", "--quiet"], ["git", "add", "main.rs"],
                            ["git", "-c", "user.name=Release Gate Test", "-c", "user.email=gate@example.invalid",
                             "commit", "--quiet", "-m", "fixture"]):
                subprocess.run(command, cwd=root, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            clean = linux_native.inspect_source(root)
            linux_native.require_source(clean, clean["commit"])
            with self.assertRaisesRegex(ValueError, "pinned source"):
                linux_native.require_source(clean, "0" * 40)
            (root / "main.rs").write_text("fn main() { panic!(); }\n")
            with self.assertRaisesRegex(ValueError, "clean tracked"):
                linux_native.require_source(linux_native.inspect_source(root), clean["commit"])
            subprocess.run(["git", "add", "main.rs"], cwd=root, check=True)
            with self.assertRaisesRegex(ValueError, "clean tracked"):
                linux_native.require_source(linux_native.inspect_source(root), clean["commit"])


def workflow_python_step(name):
    lines = (Path(__file__).resolve().parents[1] / ".github/workflows/release.yml").read_text().splitlines()
    start = next(index for index, line in enumerate(lines) if line.strip() == "- name: " + name)
    start = next(index for index in range(start + 1, len(lines)) if lines[index].strip() == "run: |")
    body = []
    for line in lines[start + 1:]:
        if line and not line.startswith("          "):
            break
        body.append(line[10:])
    return "\n".join(body)


class EvidenceArchiveTests(unittest.TestCase):
    def test_nested_gui_evidence_obeys_actual_flat_release_contract(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            out = root / "dist"
            stem = "ztty-linux-x86_64"
            evidence = out / (stem + ".native")
            files = {}
            for case in linux_native.CASES:
                png = "screenshots/frame.png" if case in ("daily_workflow", "repaint_gui", "session_switcher") else "screenshot.png"
                for name, payload in (("report.json", b'{"ok":true}\n'),
                                      ("stdout.log", b"accepted\n"), ("stderr.log", b""),
                                      (png, b"\x89PNG\r\n\x1a\nsynthetic packaging fixture")):
                    path = evidence / case / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(payload)
                    files[(Path(stem + ".native") / case / name).as_posix()] = payload
            neighbor = root / "unrelated"
            neighbor.mkdir()
            (neighbor / "keep").write_text("untouched")
            asset = out / (stem + ".native.tar.gz")
            archived = linux_native.archive_evidence(evidence, asset)
            self.assertEqual(archived["evidence_files"], len(files))
            self.assertFalse(evidence.exists())
            self.assertEqual((neighbor / "keep").read_text(), "untouched")
            with tarfile.open(asset, "r:gz") as archive:
                self.assertEqual(set(archive.getnames()), set(files))
                for name, payload in files.items():
                    self.assertEqual(archive.extractfile(name).read(), payload)
            source, version = "1" * 40, "0.0.0-fixture"
            metadata = {"source_sha": source, "version": version, "candidate_platform": "all"}
            (out / (stem + ".build.json")).write_text(json.dumps(metadata))
            (out / (stem + ".native.json")).write_text(json.dumps({"source_sha": source, "passed": True}))
            environment = {"ARTIFACT_STEM": stem, "SOURCE_SHA": source, "VERSION": version,
                           "RUNNER_TEMP": str(root), **{"OUTCOME_" + name.upper(): "success"
                            for name in ("toolchain", "build", "native", "package", "evidence")}}
            with mock.patch.dict(os.environ, environment), contextlib.chdir(root):
                exec(compile(workflow_python_step("Finalize source-bound artifact report"), "<linux-finalizer>", "exec"), {})
                report = json.loads((out / (stem + ".build.json")).read_text())
                self.assertTrue(report["passed"])
                self.assertIn(asset.name, report["files"])
                self.assertTrue(all(Path(name).name == name for name in report["files"]))
                self.assertEqual(report["files"][asset.name]["sha256"], hashlib.sha256(asset.read_bytes()).hexdigest())
                folder = root / "ztty-release/bin"
                shutil.copytree(out, folder)
                for platform_name in ("linux-aarch64", "macos-x86_64", "macos-aarch64", "windows-x86_64", "windows-aarch64"):
                    other = "ztty-" + platform_name
                    data = dict(metadata, passed=True, outcomes={"fixture": "success"}, files={})
                    (folder / (other + ".build.json")).write_text(json.dumps(data))
                    (folder / (other + ".native.json")).write_text(json.dumps({"source_sha": source, "passed": True}))
                exec(compile(workflow_python_step("Verify every platform artifact belongs to the accepted source"), "<release-verifier>", "exec"), {})
                self.assertTrue(all(path.is_file() for path in folder.iterdir()), "release glob would include a directory")

    def test_existing_archive_is_preserved_without_removing_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            evidence, archive = root / "evidence", root / "evidence.tar.gz"
            evidence.mkdir()
            (evidence / "report.json").write_text("{}")
            archive.write_bytes(b"prior accepted asset")
            with self.assertRaises(FileExistsError):
                linux_native.archive_evidence(evidence, archive)
            self.assertEqual(archive.read_bytes(), b"prior accepted asset")
            self.assertEqual((evidence / "report.json").read_text(), "{}")

    def test_linked_evidence_is_rejected_without_removing_other_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            evidence, archive = root / "evidence", root / "evidence.tar.gz"
            evidence.mkdir()
            outside = root / "outside.txt"
            outside.write_text("preserved")
            (evidence / "linked.txt").symlink_to(outside)
            with self.assertRaisesRegex(ValueError, "non-regular"):
                linux_native.archive_evidence(evidence, archive)
            self.assertTrue(evidence.exists())
            self.assertFalse(archive.exists())
            self.assertEqual(outside.read_text(), "preserved")


@unittest.skipUnless(sys.platform.startswith("linux"), "Linux process ownership gate")
class ProcessTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
            raise OSError("cannot adopt owned test descendants")

    def test_nonzero_exit_is_not_accepted(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            result = linux_native.run_owned([sys.executable, "-c", "raise SystemExit(9)"],
                                            root / "exit", root, 5)
            self.assertEqual(result["exit_code"], 9)
            self.assertFalse(linux_native.process_passed(result))
            self.assertFalse(result["cleanup"]["remaining_owned_pids"])

    @unittest.skipUnless(shutil.which("Xvfb"), "requires actual Xvfb for orphan cleanup")
    def test_timeout_reaps_owned_xvfb_and_preserves_unrelated_process(self):
        helper_root = Path(__file__).resolve().parents[1] / "tools"
        child = "\n".join((
            "import json,subprocess,sys,time", f"sys.path.insert(0, {str(helper_root)!r})",
            "from isolated_xvfb import start_xvfb",
            'server=start_xvfb(screen="640x480x24", stderr=subprocess.DEVNULL)',
            'print(json.dumps({"pid":server.process.pid,"display":server.number}),flush=True)',
            "time.sleep(300)",
        ))
        unrelated = subprocess.Popen([sys.executable, "-c", "import time;time.sleep(30)"], start_new_session=True)
        try:
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                result = linux_native.run_owned([sys.executable, "-c", child], root / "timeout", root, 3)
                created = json.loads((root / "timeout/stdout.log").read_text())
                self.assertTrue(result["timed_out"])
                self.assertFalse(linux_native.process_passed(result))
                self.assertIn(created["pid"], result["cleanup"]["leftover_owned_pids"])
                self.assertFalse(result["cleanup"]["remaining_owned_pids"])
                self.assertIsNone(linux_native.process_identity(created["pid"]))
                self.assertFalse(Path(f"/tmp/ztty-xvfb-{created['display']}.lease").exists())
                self.assertIsNone(unrelated.poll(), "cleanup stopped an unrelated process")
        finally:
            unrelated.terminate()
            with contextlib.suppress(subprocess.TimeoutExpired):
                unrelated.wait(timeout=3)
            if unrelated.poll() is None:
                unrelated.kill()
                unrelated.wait(timeout=3)

    @unittest.skipUnless(shutil.which("Xvfb"), "requires actual Xvfb for interruption cleanup")
    def test_external_sigterm_unwinds_cleanup_before_driver_exits(self):
        tests = Path(__file__).resolve().parent
        helper_root = tests.parent / "tools"
        child = "\n".join((
            "import json,os,subprocess,sys,time", f"sys.path.insert(0, {str(helper_root)!r})",
            "from isolated_xvfb import start_xvfb",
            'server=start_xvfb(screen="640x480x24", stderr=subprocess.DEVNULL)',
            'print(json.dumps({"pid":server.process.pid,"display":server.number,',
            ' "marker":os.environ["ZTTY_LINUX_NATIVE_RUN"],"harness_pid":os.getpid()}),flush=True)',
            "time.sleep(300)",
        ))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            driver = "\n".join((
                "import ctypes,json,sys", "from pathlib import Path",
                f"sys.path.insert(0, {str(tests)!r})",
                "from linux_native import NativeGateInterrupted,run_owned,termination_scope",
                "ctypes.CDLL(None).prctl(36,1,0,0,0)", f"root=Path({str(root)!r})",
                "with termination_scope():",
                " try:",
                f"  run_owned([sys.executable,'-c',{child!r}],root/'child',root,60)",
                " except NativeGateInterrupted as error:",
                "  (root/'interrupted.json').write_text(json.dumps({'signal':error.signum}))",
                "  raise SystemExit(7)",
            ))
            started_ns = time.time_ns()
            created = None
            with (root / "driver.log").open("wb") as log:
                process = subprocess.Popen([sys.executable, "-c", driver], stdout=log, stderr=log,
                                           start_new_session=True)
                try:
                    deadline = time.monotonic() + 8
                    while time.monotonic() < deadline and created is None:
                        try:
                            created = json.loads((root / "child/stdout.log").read_text())
                        except (FileNotFoundError, json.JSONDecodeError):
                            time.sleep(0.02)
                    self.assertIsNotNone(created, (root / "driver.log").read_text())
                    process.terminate()
                    self.assertEqual(process.wait(timeout=20), 7, (root / "driver.log").read_text())
                    self.assertEqual(json.loads((root / "interrupted.json").read_text())["signal"], signal.SIGTERM)
                    self.assertIsNone(linux_native.process_identity(created["pid"]))
                    self.assertFalse(linux_native.marked_processes(created["marker"]))
                    self.assertFalse(Path(f"/tmp/ztty-xvfb-{created['display']}.lease").exists())
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait(timeout=3)
                    if created:
                        linux_native.cleanup_marker(created["marker"], created["harness_pid"], started_ns)


if __name__ == "__main__":
    unittest.main()
