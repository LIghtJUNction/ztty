#!/usr/bin/env python3
"""Execute actual release scope and native regression gates with isolated fixtures."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[1]
SOURCE = "0123456789abcdef0123456789abcdef01234567"


def workflow_script(step):
    lines = (ROOT / ".github/workflows/release.yml").read_text().splitlines()
    start = lines.index("      - name: " + step)
    start = next(i for i in range(start, len(lines)) if lines[i] == "        run: |") + 1
    script = []
    for line in lines[start:]:
        if line and not line.startswith("          "):
            break
        script.append(line[10:] if line else "")
    return "\n".join(script)


def version_script():
    return workflow_script("Verify release version and pin the source commit")


class ReleaseScope(unittest.TestCase):
    def run_gate(self, build_only, scope, ref):
        with tempfile.TemporaryDirectory(prefix="ztty-release-scope-") as temporary:
            root = Path(temporary)
            (root / "tools").mkdir()
            (root / "bin").mkdir()
            # The version/archival implementation has its own acceptance. These
            # fixtures isolate only inputs to the real workflow shell guard.
            (root / "tools/release-source.sh").write_text("printf '0.2.0\\n'\n")
            git = root / "bin/git"
            git.write_text("#!/bin/sh\n[ \"$1\" = rev-parse ] && [ \"$2\" = HEAD ] || exit 2\nprintf '" + SOURCE + "\\n'\n")
            git.chmod(0o755)
            output = root / "output"
            env = os.environ.copy()
            env.update(BUILD_ONLY=build_only, CANDIDATE_PLATFORM=scope,
                       GITHUB_REF=ref, GITHUB_OUTPUT=str(output),
                       RUNNER_TEMP=str(root / "runner"), PATH=str(root / "bin") + os.pathsep + env["PATH"])
            result = subprocess.run(["bash", "-e", "-o", "pipefail", "-c", version_script()],
                                    cwd=root, env=env, text=True, capture_output=True, timeout=10)
            emitted = dict(line.split("=", 1) for line in output.read_text().splitlines()) if output.exists() else {}
            return result, emitted

    def test_build_only_accepts_all_four_explicit_scopes(self):
        for scope in ("all", "linux", "macos", "windows"):
            with self.subTest(scope=scope):
                result, output = self.run_gate("true", scope, "refs/heads/candidate")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(output, {"version": "0.2.0", "source_sha": SOURCE,
                                          "build_only": "true", "candidate_platform": scope})

    def test_full_tag_publish_remains_available(self):
        result, output = self.run_gate("false", "all", "refs/tags/v0.2.0")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output["candidate_platform"], "all")
        self.assertEqual(output["build_only"], "false")

    def test_partial_tag_publish_is_rejected(self):
        for scope in ("linux", "macos", "windows"):
            with self.subTest(scope=scope):
                result, output = self.run_gate("false", scope, "refs/tags/v0.2.0")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("Publishing requires all native platforms", result.stderr)
                self.assertEqual(output, {})

    def test_unknown_scope_is_rejected(self):
        for scope in ("", "darwin", "all,macos", "macos\nall"):
            with self.subTest(scope=scope):
                result, output = self.run_gate("true", scope, "refs/heads/candidate")
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(output, {})

    def test_branch_cannot_publish(self):
        result, output = self.run_gate("false", "all", "refs/heads/candidate")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(output, {})

    def test_wrong_version_tag_is_rejected(self):
        result, output = self.run_gate("false", "all", "refs/tags/v9.9.9")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(output, {})


class ReleaseArtifactAttempts(unittest.TestCase):
    def test_retry_collects_whole_latest_platform_artifact_and_preserves_other_jobs(self):
        with tempfile.TemporaryDirectory() as temporary:
            release = Path(temporary) / "ztty-release"
            attempts = release / "attempts"
            for name, files in {
                "ztty-windows-aarch64-2": {"arm.build.json": "failed", "stale.log": "old"},
                "ztty-windows-aarch64-10": {"arm.build.json": "passed", "latest.log": "new"},
                "ztty-linux-x86_64": {"linux.build.json": "passed"},
                "ztty-verify-ssh-diagnostics-2": {"ssh.json": "accepted"},
            }.items():
                directory = attempts / name
                directory.mkdir(parents=True)
                for filename, value in files.items():
                    (directory / filename).write_text(value)
            step = workflow_script("Collect the latest attempt for each platform and diagnostic")
            result = subprocess.run([sys.executable, "-c", step],
                                    env=dict(os.environ, RUNNER_TEMP=temporary), capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            actual = {file.name: file.read_text() for file in (release / "bin").iterdir()}
            self.assertEqual(actual, {"arm.build.json": "passed", "latest.log": "new",
                                      "linux.build.json": "passed", "ssh.json": "accepted"})


class NativeReleaseRegressions(unittest.TestCase):
    def native_script(self):
        return workflow_script("Run native Linux PTY and X11 GUI acceptance")

    def run_native_commands(self, arch, fail_script=""):
        with tempfile.TemporaryDirectory(prefix="ztty-release-native-") as temporary:
            root = Path(temporary)
            binary_dir = root / "bin"
            binary_dir.mkdir()
            (root / "dist").mkdir()
            cargo = binary_dir / "cargo"
            cargo.write_text("#!/bin/sh\nexit 0\n")
            cargo.chmod(0o755)
            python = binary_dir / "python"
            python.write_text("#!" + sys.executable + "\n" + textwrap.dedent("""
                import json, os
                from pathlib import Path
                import sys
                args = sys.argv[1:]
                with open(os.environ["CALL_LOG"], "a") as log:
                    log.write(json.dumps(args) + "\\n")
                if args[0] == "tests/linux_native.py":
                    Path(args[args.index("--evidence-dir") + 1]).mkdir(parents=True)
                if args[0] == "-":
                    sys.stdin.read()
                raise SystemExit(17 if args[0] == os.environ["FAIL_SCRIPT"] else 0)
            """))
            python.chmod(0o755)
            log = root / "calls.jsonl"
            env = dict(os.environ, PATH=str(binary_dir) + os.pathsep + os.environ["PATH"],
                       CALL_LOG=str(log), FAIL_SCRIPT=fail_script, ARCH=arch,
                       ARTIFACT_STEM="ztty-linux-" + arch, SOURCE_SHA=SOURCE,
                       GITHUB_WORKSPACE=str(root), VERSION="0.5.1", RUST_TOOLCHAIN="1.100.0")
            result = subprocess.run(["bash", "-e", "-o", "pipefail", "-c", self.native_script()],
                                    cwd=root, env=env, text=True, capture_output=True, timeout=20)
            calls = [json.loads(line) for line in log.read_text().splitlines()]
            return result, calls

    def test_linux_architectures_run_every_runtime_regression(self):
        for arch in ("x86_64", "aarch64"):
            with self.subTest(arch=arch):
                result, calls = self.run_native_commands(arch)
                self.assertEqual(result.returncode, 0, result.stderr)
                evidence = "dist/ztty-linux-" + arch + ".native/"
                guard = [call for call in calls if call[0] == "tests/clipboard_image_guard_gui.py"]
                self.assertEqual(guard, [["tests/clipboard_image_guard_gui.py",
                    "--binary", "target/release/ztty", "--expect", "reject",
                    "--output", evidence + "clipboard-image-guard.json"]])
                for script, stem, extra in (
                    ("reload_runtime.py", "reload", []),
                    ("tab_switch_pixels.py", "tab-switch", ["--scope", "all", "--expect", "clean"]),
                ):
                    actual = [call for call in calls if call[0] == "tests/" + script]
                    expected = [["tests/" + script, "--binary", "target/release/ztty",
                                 "--renderer", renderer, *extra,
                                 "--output", evidence + stem + "-" + renderer + ".json"]
                                for renderer in ("gpu", "software")]
                    self.assertEqual(actual, expected)
                self.assertEqual(calls[-1], ["-"])

    def test_runtime_regression_failure_stops_the_native_gate(self):
        for script in ("clipboard_image_guard_gui.py", "reload_runtime.py", "tab_switch_pixels.py"):
            with self.subTest(script=script):
                result, calls = self.run_native_commands("x86_64", "tests/" + script)
                self.assertEqual(result.returncode, 17, result.stderr)
                self.assertNotIn(["-"], calls)

    def run_readback(self, change=None):
        # Exercise the actual workflow readback, including its original checks.
        # These are report fixtures, not substitutes for running the GUI tests.
        with tempfile.TemporaryDirectory(prefix="ztty-release-readback-") as temporary:
            root = Path(temporary)
            binary = root / "target/release/ztty"
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b"immutable release ELF fixture")
            digest = hashlib.sha256(binary.read_bytes()).hexdigest()
            evidence = root / "dist/ztty-linux-x86_64.native"
            reports = {}
            for name, count, flag in (
                ("clipboard.json", 25, "ok"), ("clipboard-image-guard.json", 10, "ok"),
                ("wayland/validation.json", 65, "passed"),
            ):
                reports[name] = {flag: True, "binary_sha256": digest,
                                 "checks": [{flag: True} for _ in range(count)]}
            reports["clipboard-image-guard.json"].update(expect="reject", cleanup_errors=[])
            reports["wayland/validation.json"].update(
                source=SOURCE, owned_brokers_remaining=[], owned_children_remaining=[],
                owned_helpers_remaining=[])
            for renderer in ("gpu", "software"):
                for name, count in ((f"graphics-{renderer}/graphics-extensions.json", 8),
                                    (f"osc52-{renderer}/osc52.json", 13)):
                    reports[name] = {"ok": True, "binary_sha256": digest,
                                     "checks": [{"ok": True} for _ in range(count)]}
                reports[f"reload-{renderer}.json"] = {
                    "ok": True, "expected_unsupported": False, "renderer": renderer,
                    "binary_sha256": digest, "target_sha256": digest,
                    "checks": [f"completed reload check {number}" for number in range(9)],
                    "reloads": [{"accepted": True, "exit_code": 0} for _ in range(2)],
                    "remaining_owned_pids": [],
                }
                reports[f"tab-switch-{renderer}.json"] = {
                    "ok": True, "binary_sha256": digest, "renderer": renderer,
                    "requested_scope": "all", "expect": "clean",
                    "checks": ["tabs complete", "focus complete", "children remain live"],
                    "invalid_application_samples": 0,
                    "invalid_samples_by_scope": {"tabs": 0, "focus_raise": 0},
                    "remaining_owned_pids": [],
                }
            if change is not None:
                name, key, value = change
                reports[name][key] = value
            for name, report in reports.items():
                path = evidence / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(json.dumps(report))
            script = self.native_script().split("python - <<'PY'\n", 1)[1].rsplit("\nPY", 1)[0]
            env = dict(os.environ, ARTIFACT_STEM="ztty-linux-x86_64", SOURCE_SHA=SOURCE)
            return subprocess.run([sys.executable, "-c", script], cwd=root, env=env,
                                  text=True, capture_output=True, timeout=10)

    def test_complete_native_reports_pass_readback(self):
        result = self.run_readback()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_readback_rejects_stale_incomplete_or_failed_runtime_evidence(self):
        cases = (
            ("clipboard-image-guard.json", "binary_sha256", "0" * 64),
            ("clipboard-image-guard.json", "expect", "legacy-leak"),
            ("clipboard-image-guard.json", "cleanup_errors", [{"pid": 123}]),
            ("reload-gpu.json", "expected_unsupported", True),
            ("reload-gpu.json", "target_sha256", "0" * 64),
            ("reload-gpu.json", "reloads", []),
            ("reload-software.json", "remaining_owned_pids", [123]),
            ("reload-software.json", "checks", []),
            ("tab-switch-gpu.json", "binary_sha256", "0" * 64),
            ("tab-switch-gpu.json", "requested_scope", "tabs"),
            ("tab-switch-gpu.json", "expect", "reproduce"),
            ("tab-switch-software.json", "invalid_application_samples", 1),
            ("tab-switch-software.json", "invalid_samples_by_scope", {"tabs": 0}),
            ("tab-switch-software.json", "cleanup_errors", ["sampler remains live"]),
            ("tab-switch-software.json", "remaining_owned_pids", [123]),
        )
        for change in cases:
            with self.subTest(change=change):
                result = self.run_readback(change)
                self.assertNotEqual(result.returncode, 0, result.stdout)


if __name__ == "__main__":
    unittest.main()
