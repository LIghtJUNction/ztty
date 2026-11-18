#!/usr/bin/env python3
"""Keep registry consumers tied to release manifests without invoking Cargo."""
from pathlib import Path
import runpy
import shutil
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
                {"zinput": "0.7.1", "zgraphics": "0.8.2", "zpty": "0.9.3", "zterm": "0.10.4"},
                {"zinput": "0.7.2", "zgraphics": "0.8.3", "zpty": "0.9.4", "zterm": "0.11.0"},
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
                "zinput": "0.7.1", "zgraphics": "0.8.2", "zpty": "0.9.3", "zterm": "0.10.4",
            }, inherited=False)
            with self.assertRaises((KeyError, ValueError)):
                runpy.run_path(str(root / "tools/verify_registry.py"))


if __name__ == "__main__":
    unittest.main()
