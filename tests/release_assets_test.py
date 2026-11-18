#!/usr/bin/env python3
"""Pure filesystem contracts for release evidence; no network or publication."""
import json
from pathlib import Path
import runpy
import shutil
import tempfile
import unittest
import zipfile

MODULE = runpy.run_path(str(Path(__file__).resolve().parents[1] / "tools" / "release_assets.py"))
PREPARE = MODULE["prepare"]
VERIFY = MODULE["verify_and_restore_empty"]
VERSION = "1.2.3"
SOURCE = "a" * 40


class EvidenceContracts(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.folder = self.root / "original"
        self.folder.mkdir()
        (self.folder / "payload.bin").write_bytes(b"\x00accepted payload\xff")
        (self.folder / "application.stderr.log").write_bytes(b"")
        (self.folder / "nested").mkdir()
        (self.folder / "nested" / "empty.log").write_bytes(b"")
        (self.folder / "nested" / "trace.log").write_bytes(b"trace\n")
        self.archive = self.root / "evidence.zip"
        self.assets = self.root / "assets.bin"
        self.readback = self.root / "readback"
        self.readback.mkdir()

    def stage(self):
        result = PREPARE(self.folder, self.archive, self.assets, VERSION, SOURCE)
        for raw in self.assets.read_bytes().split(b"\x00"):
            if raw:
                path = Path(raw.decode("utf-8"))
                shutil.copyfile(path, self.readback / path.name)
        return result

    def verify(self, version=VERSION, source=SOURCE):
        return VERIFY(self.readback / self.archive.name, self.readback,
                      version, source, self.archive)

    def test_empty_and_nested_evidence_survive_without_empty_uploads(self):
        result = self.stage()
        self.assertEqual(result["files_preserved"], 4)
        self.assertEqual(result["empty_files_preserved"], 2)
        uploads = [Path(raw.decode("utf-8")) for raw in self.assets.read_bytes().split(b"\x00") if raw]
        self.assertTrue(all(path.stat().st_size > 0 for path in uploads))
        self.assertNotIn(self.folder / "application.stderr.log", uploads)
        with zipfile.ZipFile(self.archive) as archive:
            self.assertEqual(archive.read("files/application.stderr.log"), b"")
            self.assertEqual(archive.read("files/nested/empty.log"), b"")
            manifest = json.loads(archive.read("manifest.json"))
            self.assertEqual(manifest["files"]["nested/empty.log"]["bytes"], 0)
        audit = self.verify()
        self.assertTrue(audit["passed"])
        self.assertEqual(audit["empty_files_restored"], ["application.stderr.log"])
        self.assertEqual((self.readback / "application.stderr.log").read_bytes(), b"")

    def test_archive_is_deterministic_and_readback_is_idempotent(self):
        self.stage()
        original = self.archive.read_bytes()
        PREPARE(self.folder, self.archive, self.assets, VERSION, SOURCE)
        self.assertEqual(self.archive.read_bytes(), original)
        self.verify()
        self.assertEqual(self.verify()["empty_files_restored"], [])

    def test_changed_nonempty_asset_stops_before_empty_materialization(self):
        self.stage()
        (self.readback / "payload.bin").write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "standalone release asset"):
            self.verify()
        self.assertFalse((self.readback / "application.stderr.log").exists())

    def test_existing_conflicting_empty_asset_is_never_overwritten(self):
        self.stage()
        target = self.readback / "application.stderr.log"
        target.write_bytes(b"keep this content")
        with self.assertRaisesRegex(ValueError, "empty evidence conflicts"):
            self.verify()
        self.assertEqual(target.read_bytes(), b"keep this content")

    def test_wrong_source_or_version_fails(self):
        self.stage()
        for version, source in [("1.2.4", SOURCE), (VERSION, "b" * 40)]:
            with self.subTest(version=version, source=source):
                with self.assertRaisesRegex(ValueError, "identity mismatch"):
                    self.verify(version, source)
        self.assertFalse((self.readback / "application.stderr.log").exists())

    def test_modified_archive_fails_even_if_its_embedded_manifest_is_changed(self):
        self.stage()
        downloaded = self.readback / self.archive.name
        with zipfile.ZipFile(downloaded, "a") as archive:
            archive.writestr("unexpected", b"")
        with self.assertRaisesRegex(ValueError, "differs from the accepted upload"):
            self.verify()
        self.assertFalse((self.readback / "application.stderr.log").exists())

    def test_source_symlinks_and_recursive_outputs_are_rejected(self):
        link = self.folder / "linked.bin"
        try:
            link.symlink_to(self.folder / "payload.bin")
        except OSError:
            self.skipTest("symlinks unavailable")
        with self.assertRaisesRegex(ValueError, "symlink"):
            PREPARE(self.folder, self.archive, self.assets, VERSION, SOURCE)
        link.unlink()
        with self.assertRaisesRegex(ValueError, "outside"):
            PREPARE(self.folder, self.folder / "recursive.zip", self.assets, VERSION, SOURCE)


if __name__ == "__main__":
    unittest.main()
