#!/usr/bin/env python3
"""Release archives must contain exactly the tracked dependency source."""
from pathlib import Path
import hashlib
import importlib.util
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("source_archive", ROOT / "tools/source_archive.py")
archive = importlib.util.module_from_spec(spec)
spec.loader.exec_module(archive)


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], stderr=subprocess.PIPE).decode().strip()


class SourceArchive(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.dependency = self.root / "dependency"
        self.app = self.root / "app"
        for repo in (self.dependency, self.app):
            repo.mkdir()
            git(repo, "init", "-b", "main")
            git(repo, "config", "user.name", "Source archive test")
            git(repo, "config", "user.email", "test@example.invalid")
            (repo / "Cargo.toml").write_text('[package]\nname = "fixture"\nversion = "0.1.0"\n')
            git(repo, "add", "Cargo.toml")
            git(repo, "commit", "-m", "fixture")
        (self.dependency / "src").mkdir()
        (self.dependency / "src/lib.rs").write_text("#![no_std]\npub const VALUE: u8 = 7;\n")
        git(self.dependency, "add", "src/lib.rs")
        git(self.dependency, "commit", "-m", "source")
        git(self.app, "-c", "protocol.file.allow=always", "submodule", "add",
            str(self.dependency), "crates/zgpui")
        git(self.app, "commit", "-am", "pin dependency")
        self.output = self.root / "source.tar.gz"

    def test_submodule_is_complete_pinned_and_reproducible(self):
        archive.create(self.app, "HEAD", "ztty-test", self.output)
        first = hashlib.sha256(self.output.read_bytes()).hexdigest()
        with tarfile.open(self.output) as contents:
            data = contents.extractfile("ztty-test/crates/zgpui/src/lib.rs").read()
            self.assertIn(b"VALUE: u8 = 7", data)
            self.assertNotIn("ztty-test/crates/zgpui/.git", contents.getnames())
        archive.create(self.app, "HEAD", "ztty-test", self.output)
        self.assertEqual(first, hashlib.sha256(self.output.read_bytes()).hexdigest())

    def test_missing_submodule_does_not_publish_incomplete_archive(self):
        git(self.app, "submodule", "deinit", "-f", "--all")
        with self.assertRaisesRegex(RuntimeError, "not initialized"):
            archive.create(self.app, "HEAD", "ztty-test", self.output)
        self.assertFalse(self.output.exists())

    def test_dirty_or_different_dependency_is_rejected(self):
        module = self.app / "crates/zgpui"
        path = module / "src/lib.rs"
        path.write_text("changed\n")
        with self.assertRaisesRegex(RuntimeError, "tracked modifications"):
            archive.create(self.app, "HEAD", "ztty-test", self.output)
        git(module, "restore", "src/lib.rs")
        git(module, "checkout", "HEAD~1")
        with self.assertRaisesRegex(RuntimeError, "recorded commit"):
            archive.create(self.app, "HEAD", "ztty-test", self.output)
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
