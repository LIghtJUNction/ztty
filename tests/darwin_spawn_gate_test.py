#!/usr/bin/env python3
"""Contract checks for the actual workflow's Darwin spawn evidence gate."""
import contextlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SOURCES = ("crates/zpty/src/macos/spawn.rs", "crates/zpty/src/macos.rs",
           "crates/zpty/tests/darwin_spawn.rs", "crates/zpty/tests/darwin_spawn_abi.c")


def gate_script():
    lines = (ROOT / ".github/workflows/release.yml").read_text().splitlines()
    start = lines.index("      - name: Validate thread-safe Darwin PTY creation on the native SDK")
    start = next(i for i in range(start, len(lines)) if lines[i] == "        run: |") + 1
    body = []
    for line in lines[start:]:
        if line and not line.startswith("          "):
            break
        body.append(line[10:] if line else "")
    return "\n".join(body)


def valid_report():
    return {"suite": "darwin-threaded-spawn", "passed": True, "architecture": "aarch64",
            "checks": ["contract-fixture"] * 8, "worker_iterations": 5, "timeout_seconds": 5.01}


class DarwinSpawnGate(unittest.TestCase):
    def run_gate(self, stdout, code=0, *, host="darwin", error=None):
        script = gate_script()
        with tempfile.TemporaryDirectory(prefix="ztty-spawn-gate-") as temporary:
            root = Path(temporary)
            (root / "dist").mkdir()
            for name in SOURCES:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / name, path)
            previous = Path.cwd()
            env = {"ARTIFACT_STEM": "candidate", "RUST_TOOLCHAIN": "1.100.0",
                   "RUST_TARGET": "aarch64-apple-darwin", "HOST_ARCH": "arm64",
                   "SOURCE_SHA": "a" * 40, "ARCH": "aarch64"}
            result = subprocess.CompletedProcess(["cargo"], code, stdout, "retained stderr")
            try:
                os.chdir(root)
                with mock.patch.dict(os.environ, env), mock.patch("sys.platform", host), \
                     mock.patch("platform.machine", return_value="arm64"), \
                     mock.patch("subprocess.run", return_value=result, side_effect=error) as run, \
                     contextlib.redirect_stdout(io.StringIO()):
                    with self.assertRaises(SystemExit) as stopped:
                        exec(compile(script, "<actual-darwin-spawn-workflow-gate>", "exec"), {})
                report = json.loads((root / "dist/candidate.spawn.json").read_text())
                logs = ((root / "dist/candidate.spawn.stdout.log").read_text(),
                        (root / "dist/candidate.spawn.stderr.log").read_text())
                return stopped.exception.code, report, logs, run.call_count
            finally:
                os.chdir(previous)

    def test_valid_native_evidence(self):
        code, report, logs, calls = self.run_gate(json.dumps(valid_report()) + "\n")
        self.assertEqual(code, 0)
        self.assertIs(report["passed"], True)
        self.assertEqual(report["source_sha"], "a" * 40)
        self.assertEqual(set(report["source_files_sha256"]), set(SOURCES))
        self.assertEqual(logs[1], "retained stderr")
        self.assertEqual(calls, 1)

    def test_skip_and_ambiguous_output_cannot_pass(self):
        for output in ("SKIP darwin_spawn: requires native macOS\n", "",
                       json.dumps(valid_report()) + "\n" + json.dumps(valid_report())):
            with self.subTest(output=output[:40]):
                code, report, logs, _ = self.run_gate(output)
                self.assertNotEqual(code, 0)
                self.assertIs(report["passed"], False)
                self.assertEqual(logs[0], output)

    def test_invalid_native_results_cannot_pass(self):
        mutations = (("passed", "true"), ("passed", 1), ("passed", False),
                     ("architecture", "x86_64"), ("suite", "wrong"),
                     ("checks", ["one"]), ("worker_iterations", 0),
                     ("timeout_seconds", 0.1), ("timeout_seconds", 30.0))
        for key, value in mutations:
            with self.subTest(key=key, value=value):
                fixture = valid_report()
                fixture[key] = value
                code, report, _, _ = self.run_gate(json.dumps(fixture))
                self.assertNotEqual(code, 0)
                self.assertIs(report["passed"], False)

    def test_nonzero_process_cannot_pass(self):
        code, report, _, _ = self.run_gate(json.dumps(valid_report()), code=1)
        self.assertNotEqual(code, 0)
        self.assertIs(report["passed"], False)
        self.assertEqual(report["exit_code"], 1)

    def test_non_darwin_host_cannot_run_or_pass(self):
        code, report, _, calls = self.run_gate(json.dumps(valid_report()), host="linux")
        self.assertNotEqual(code, 0)
        self.assertIs(report["passed"], False)
        self.assertEqual(calls, 0)

    def test_timeout_preserves_partial_logs(self):
        error = subprocess.TimeoutExpired(["cargo"], 120, output=b"partial stdout", stderr=b"partial stderr")
        code, report, logs, _ = self.run_gate("", error=error)
        self.assertNotEqual(code, 0)
        self.assertIs(report["passed"], False)
        self.assertEqual(logs, ("partial stdout", "partial stderr"))
        self.assertIn("TimeoutExpired", report["error"])


if __name__ == "__main__":
    unittest.main()
