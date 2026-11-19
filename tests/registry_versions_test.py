#!/usr/bin/env python3
"""Exercise registry release selection and timeout policy without real Cargo."""
from pathlib import Path
import json
import os
import runpy
import shutil
import subprocess
import sys
import textwrap
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]


class RegistryVersions(unittest.TestCase):
    def fixture(self, root, versions, inherited=True):
        (root / "tools").mkdir(exist_ok=True)
        shutil.copyfile(ROOT / "tools/verify_registry.py", root / "tools/verify_registry.py")
        workspace = '[workspace.package]\n'
        if inherited:
            workspace += f'version = "{versions["zinput"]}"\n'
        (root / "Cargo.toml").write_text(workspace)
        for package, version in versions.items():
            folder = root / "crates" / package
            folder.mkdir(parents=True, exist_ok=True)
            version_line = 'version.workspace = true' if package == "zinput" else f'version = "{version}"'
            (folder / "Cargo.toml").write_text(
                f'[package]\nname = "{package}"\n{version_line}\n')

    def test_release_changes_reach_exact_registry_consumers(self):
        with tempfile.TemporaryDirectory(prefix="ztty-registry-versions-") as temporary:
            root = Path(temporary)
            for versions in [
                {"zinput": "0.7.1", "zgraphics": "0.8.2", "zpty": "0.9.3", "zgpui": "0.1.0", "zterm": "0.10.4", "ztty-plugin-api": "0.1.0"},
                {"zinput": "0.7.2", "zgraphics": "0.8.3", "zpty": "0.9.4", "zgpui": "0.2.0", "zterm": "0.11.0", "ztty-plugin-api": "0.2.0"},
            ]:
                with self.subTest(versions=versions):
                    self.fixture(root, versions)
                    script = runpy.run_path(str(root / "tools/verify_registry.py"))
                    consumer = tomllib.loads(script["manifest"](list(versions), "fixture"))
                    self.assertEqual(script["VERSIONS"], versions)
                    self.assertEqual(consumer["dependencies"], {
                        name: {"version": "=" + version, "default-features": False}
                        for name, version in versions.items()
                    })
                    subset = tomllib.loads(script["manifest"](["zterm"], "subset"))
                    self.assertEqual(list(subset["dependencies"]), ["zterm"])

    def test_missing_inherited_version_does_not_fall_back_to_an_old_release(self):
        with tempfile.TemporaryDirectory(prefix="ztty-registry-versions-") as temporary:
            root = Path(temporary)
            self.fixture(root, {
                "zinput": "0.7.1", "zgraphics": "0.8.2", "zpty": "0.9.3", "zgpui": "0.1.0", "zterm": "0.10.4", "ztty-plugin-api": "0.1.0",
            }, inherited=False)
            with self.assertRaises((KeyError, ValueError)):
                runpy.run_path(str(root / "tools/verify_registry.py"))


    def cargo_fixture(self, root, stage="", scope=""):
        self.fixture(root, {
            "zinput": "0.7.1", "zgraphics": "0.8.2", "zpty": "0.9.3", "zgpui": "0.1.0", "zterm": "0.10.4", "ztty-plugin-api": "0.1.0",
        })
        binary = root / "bin" / "cargo"
        binary.parent.mkdir()
        binary.write_text("#!" + sys.executable + "\n" + textwrap.dedent("""
            import json, os, pathlib, sys, time, tomllib
            stage = next(name for name in ("metadata", "check", "run") if name in sys.argv)
            root = pathlib.Path.cwd()
            if stage == os.environ.get("ZTTY_TEST_DELAY_STAGE") and (
                    not os.environ.get("ZTTY_TEST_DELAY_SCOPE") or
                    root.name == os.environ["ZTTY_TEST_DELAY_SCOPE"]):
                print("fixture waiting in Cargo " + stage, file=sys.stderr, flush=True)
                time.sleep(2)
            if stage == "metadata":
                dependencies = tomllib.loads((root / "Cargo.toml").read_text())["dependencies"]
                packages, lock = [], []
                for name, dependency in dependencies.items():
                    version = dependency["version"].removeprefix("=")
                    directory = root / ("registry-" + name)
                    directory.mkdir(exist_ok=True)
                    source = directory / "lib.rs"
                    source.write_text("#![no_std]\\n")
                    packages.append({
                        "name": name, "version": version,
                        "source": "registry+https://github.com/rust-lang/crates.io-index",
                        "manifest_path": str(directory / "Cargo.toml"),
                        "targets": [{"kind": ["lib"], "src_path": str(source)}],
                    })
                    lock.append(f'[[package]]\\nname = "{name}"\\nversion = "{version}"\\n')
                (root / "Cargo.lock").write_text("\\n".join(lock))
                print(json.dumps({"packages": packages}))
            elif stage == "run":
                print("registry consumer passed")
        """))
        binary.chmod(0o755)
        return dict(os.environ, PATH=str(binary.parent) + os.pathsep + os.environ["PATH"],
                    ZTTY_TEST_DELAY_STAGE=stage, ZTTY_TEST_DELAY_SCOPE=scope)

    def invoke_verifier(self, root, environment, arguments):
        report = root / "result.json"
        completed = subprocess.run(
            [sys.executable, str(root / "tools/verify_registry.py"),
             "--output", str(report), *arguments],
            env=environment, capture_output=True, text=True, timeout=15)
        result = json.loads(report.read_text()) if report.exists() else None
        return completed, result

    @unittest.skipUnless(os.name == "posix", "fake Cargo executable uses a POSIX shebang")
    def test_cli_budget_controls_each_cargo_stage_and_combined_consumer(self):
        stages = {"metadata": "metadata", "check": "no_std_library_check", "run": "consumer_run"}
        for stage, scope in [("metadata", ""), ("check", ""), ("run", ""), ("metadata", "combined")]:
            with self.subTest(stage=stage, scope=scope), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                environment = self.cargo_fixture(root, stage, scope)
                arguments = ["--cargo-timeout", "0.5"]
                if scope != "combined":
                    arguments.extend(["--packages", "zinput"])
                completed, result = self.invoke_verifier(root, environment, arguments)
                self.assertEqual(completed.returncode, 1, completed.stderr)
                checked = result["combined_check"] if scope else result["individual_checks"][0]
                self.assertFalse(checked["ok"])
                command = checked["commands"][-1]
                self.assertEqual(command["stage"], stages[stage])
                self.assertIn("exceeded 0.5 seconds", command["error"])
                self.assertIn("fixture waiting in Cargo", command["stderr"])

    @unittest.skipUnless(os.name == "posix", "fake Cargo executable uses a POSIX shebang")
    def test_cli_timeout_default_and_longer_budget(self):
        for timeout, expected, stage in [(None, 90, ""), ("3", 3, "metadata")]:
            with self.subTest(timeout=timeout), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                environment = self.cargo_fixture(root, stage)
                arguments = ["--packages", "zinput"]
                if timeout:
                    arguments.extend(["--cargo-timeout", timeout])
                completed, result = self.invoke_verifier(root, environment, arguments)
                self.assertEqual(completed.returncode, 0, completed.stderr)
                self.assertTrue(result["ok"])
                self.assertEqual(result["cargo_timeout_seconds"], expected)
                self.assertTrue(all(command["timeout_seconds"] == expected
                                    for command in result["individual_checks"][0]["commands"]))

    @unittest.skipUnless(os.name == "posix", "fake Cargo executable uses a POSIX shebang")
    def test_cli_rejects_nonpositive_or_unbounded_timeout(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            environment = self.cargo_fixture(root)
            for value in ("0", "-1", "nan", "inf"):
                with self.subTest(value=value):
                    completed, result = self.invoke_verifier(
                        root, environment, ["--cargo-timeout", value])
                    self.assertEqual(completed.returncode, 2, completed.stderr)
                    self.assertIn("positive finite number", completed.stderr)
                    self.assertIsNone(result)

if __name__ == "__main__":
    unittest.main()
