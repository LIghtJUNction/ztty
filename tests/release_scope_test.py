#!/usr/bin/env python3
"""Execute the actual release version/scope shell gate with isolated Git/version fixtures."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SOURCE = "0123456789abcdef0123456789abcdef01234567"


def version_script():
    lines = (ROOT / ".github/workflows/release.yml").read_text().splitlines()
    start = lines.index("      - name: Verify release version and pin the source commit")
    start = next(i for i in range(start, len(lines)) if lines[i] == "        run: |") + 1
    script = []
    for line in lines[start:]:
        if line and not line.startswith("          "):
            break
        script.append(line[10:] if line else "")
    return "\n".join(script)


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


if __name__ == "__main__":
    unittest.main()
