#!/usr/bin/env python3
"""Pure filesystem contracts for release evidence; no network or publication."""
import hashlib
import json
from pathlib import Path
import runpy
import shutil
import tempfile
import unittest
from unittest import mock
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
        self.package = "ztty-linux-x86_64.tar.gz"
        package = b"\x00accepted distribution fixture\xff"
        self.standalone = {
            self.package: package,
            self.package + ".sha256": (hashlib.sha256(package).hexdigest() + "  " + self.package + "\n").encode(),
            "install.sh": b"#!/bin/sh\nexit 0\n",
        }
        self.evidence = {
            "application.stderr.log": b"",
            "application.stdout.log": b"native acceptance completed\n",
            "native-capture.png": b"\x89PNG\r\n\x1a\nscreenshot fixture",
            "nested/empty.log": b"",
            "nested/trace.log": b"trace\n",
        }
        for name, payload in self.standalone.items() | self.evidence.items():
            path = self.folder / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(payload)
        self.archive = self.root / "evidence.zip"
        self.assets = self.root / "assets.bin"
        self.readback = self.root / "readback"
        self.readback.mkdir()

    def uploads(self):
        return [Path(raw.decode("utf-8")) for raw in self.assets.read_bytes().split(b"\x00") if raw]

    def stage(self):
        result = PREPARE(self.folder, self.archive, self.assets, VERSION, SOURCE)
        for path in self.uploads():
            shutil.copyfile(path, self.readback / path.name)
        return result

    def verify(self, version=VERSION, source=SOURCE):
        return VERIFY(self.readback / self.archive.name, self.readback,
                      version, source, self.archive)

    def rewrite_archives(self, changed):
        with zipfile.ZipFile(self.archive) as archive:
            entries = {name: archive.read(name) for name in archive.namelist()}
        entries.update(changed)
        for path in (self.archive, self.readback / self.archive.name):
            with zipfile.ZipFile(path, "w") as archive:
                for name, payload in entries.items():
                    archive.writestr(name, payload)

    def test_empty_and_nested_evidence_survive_without_raw_uploads(self):
        result = self.stage()
        self.assertEqual(result["files_preserved"], len(self.standalone) + len(self.evidence))
        self.assertEqual(result["empty_files_preserved"], 2)
        uploads = self.uploads()
        self.assertTrue(all(path.stat().st_size > 0 for path in uploads))
        self.assertEqual({path.name for path in uploads},
                         set(self.standalone) | {"evidence.zip", "evidence.zip.sha256"})
        for name in self.evidence:
            self.assertFalse((self.readback / name).exists())
        with zipfile.ZipFile(self.archive) as archive:
            manifest = json.loads(archive.read("manifest.json"))
            for name, payload in self.standalone.items() | self.evidence.items():
                self.assertEqual(archive.read("files/" + name), payload)
                self.assertEqual(manifest["files"][name],
                                 {"bytes": len(payload), "sha256": hashlib.sha256(payload).hexdigest()})
        audit = self.verify()
        self.assertTrue(audit["passed"])
        self.assertEqual(audit["files_verified"], len(self.standalone) + len(self.evidence))
        self.assertEqual(audit["empty_files_restored"], ["application.stderr.log"])
        self.assertEqual(set(audit["restored_files"]),
                         {"application.stderr.log", "application.stdout.log", "native-capture.png"})
        for name, payload in self.evidence.items():
            if "/" not in name:
                self.assertEqual((self.readback / name).read_bytes(), payload)
        self.assertFalse((self.readback / "nested").exists())

    def test_hundreds_of_raw_files_do_not_increase_standalone_uploads(self):
        for index in range(1000):
            (self.folder / f"capture-{index:04}.png").write_bytes(f"frame {index}".encode())
        (self.folder / "ztty-windows-x86_64.resize-control.zip").write_bytes(b"diagnostic bundle")
        (self.folder / "ztty-linux-x86_64.native.tar.gz").write_bytes(b"native evidence bundle")
        result = self.stage()
        self.assertEqual(result["files_preserved"], 1002 + len(self.standalone) + len(self.evidence))
        self.assertEqual(result["standalone_assets"], len(self.standalone) + 2)
        self.assertEqual(len(self.uploads()), len(self.standalone) + 2)
        audit = self.verify()
        self.assertEqual(audit["files_verified"], result["files_preserved"])
        self.assertEqual((self.readback / "capture-0999.png").read_bytes(), b"frame 999")
        self.assertEqual((self.readback / "ztty-windows-x86_64.resize-control.zip").read_bytes(),
                         b"diagnostic bundle")

    def test_all_six_platform_packages_and_provenance_stay_standalone(self):
        expected = {"install.sh"}
        for platform, formats in (("linux", ("tar.gz",)), ("macos", ("zip", "tar.gz")),
                                  ("windows", ("zip",))):
            for arch in ("x86_64", "aarch64"):
                stem = f"ztty-{platform}-{arch}"
                suffixes = [".build.json", ".build.json.sha256", ".native.json"]
                for extension in formats:
                    suffixes.extend(("." + extension, "." + extension + ".sha256"))
                if platform != "linux":
                    suffixes.extend((".manifest.json", ".manifest.json.sha256"))
                for suffix in suffixes:
                    name = stem + suffix
                    (self.folder / name).write_bytes(("fixture " + name).encode())
                    expected.add(name)
        result = self.stage()
        self.assertEqual({path.name for path in self.uploads()},
                         expected | {"evidence.zip", "evidence.zip.sha256"})
        self.assertEqual(result["standalone_assets"] + MODULE["SOURCE_ASSET_COUNT"], 47)
        self.assertTrue(self.verify()["passed"])

    def test_asset_bound_includes_generated_and_source_assets_before_writing(self):
        needed = len(self.standalone) + 2 + MODULE["SOURCE_ASSET_COUNT"]
        with mock.patch.dict(PREPARE.__globals__, {"MAX_STANDALONE_ASSETS": needed - 1}):
            with self.assertRaisesRegex(ValueError, "asset count exceeds"):
                PREPARE(self.folder, self.archive, self.assets, VERSION, SOURCE)
        self.assertFalse(self.archive.exists())
        self.assertFalse(self.assets.exists())
        with mock.patch.dict(PREPARE.__globals__, {"MAX_STANDALONE_ASSETS": needed}):
            self.stage()

    def test_empty_distribution_is_rejected_before_writing(self):
        (self.folder / self.package).write_bytes(b"")
        with self.assertRaisesRegex(ValueError, "standalone release asset is empty"):
            PREPARE(self.folder, self.archive, self.assets, VERSION, SOURCE)
        self.assertFalse(self.archive.exists())
        self.assertFalse(self.assets.exists())

    def test_archive_is_deterministic_and_readback_is_idempotent(self):
        self.stage()
        original = self.archive.read_bytes()
        PREPARE(self.folder, self.archive, self.assets, VERSION, SOURCE)
        self.assertEqual(self.archive.read_bytes(), original)
        self.verify()
        audit = self.verify()
        self.assertEqual(audit["empty_files_restored"], [])
        self.assertEqual(audit["restored_files"], [])

    def test_missing_or_changed_distribution_stops_before_evidence_restoration(self):
        self.stage()
        target = self.readback / self.package
        for changed in (None, b"changed"):
            with self.subTest(changed=changed):
                if changed is None:
                    target.unlink()
                else:
                    target.write_bytes(changed)
                with self.assertRaisesRegex(ValueError, "standalone release asset"):
                    self.verify()
                self.assertFalse((self.readback / "application.stderr.log").exists())
                self.assertFalse((self.readback / "application.stdout.log").exists())

    def test_existing_conflicting_archive_evidence_is_never_overwritten(self):
        self.stage()
        for name in ("application.stderr.log", "application.stdout.log"):
            with self.subTest(name=name):
                target = self.readback / name
                target.write_bytes(b"keep this content")
                with self.assertRaisesRegex(ValueError, "evidence conflicts"):
                    self.verify()
                self.assertEqual(target.read_bytes(), b"keep this content")
                self.assertFalse((self.readback / "native-capture.png").exists())
                target.unlink()

    def test_identical_existing_archive_evidence_is_accepted(self):
        self.stage()
        for name, payload in self.evidence.items():
            if "/" not in name:
                (self.readback / name).write_bytes(payload)
        self.assertEqual(self.verify()["restored_files"], [])

    def test_archive_evidence_readback_cannot_follow_symlinks(self):
        self.stage()
        outside = self.root / "outside.log"
        outside.write_bytes(self.evidence["application.stdout.log"])
        try:
            (self.readback / "application.stdout.log").symlink_to(outside)
        except OSError:
            self.skipTest("symlinks unavailable")
        with self.assertRaisesRegex(ValueError, "symlinks"):
            self.verify()
        self.assertEqual(outside.read_bytes(), self.evidence["application.stdout.log"])
        self.assertFalse((self.readback / "application.stderr.log").exists())

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

    def test_nested_archive_payload_is_verified_before_any_restoration(self):
        self.stage()
        self.rewrite_archives({"files/nested/trace.log": b"corrupted"})
        with self.assertRaisesRegex(ValueError, "evidence payload mismatch"):
            self.verify()
        self.assertFalse((self.readback / "application.stderr.log").exists())
        self.assertFalse((self.readback / "application.stdout.log").exists())

    def test_archive_manifest_requires_the_complete_file_set(self):
        self.stage()
        self.rewrite_archives({"files/unlisted.log": b"not in manifest"})
        with self.assertRaisesRegex(ValueError, "manifest and archive entries differ"):
            self.verify()
        self.assertFalse((self.readback / "application.stderr.log").exists())

    def test_source_symlinks_and_recursive_outputs_are_rejected(self):
        link = self.folder / "linked.bin"
        try:
            link.symlink_to(self.folder / self.package)
        except OSError:
            self.skipTest("symlinks unavailable")
        with self.assertRaisesRegex(ValueError, "symlink"):
            PREPARE(self.folder, self.archive, self.assets, VERSION, SOURCE)
        link.unlink()
        with self.assertRaisesRegex(ValueError, "outside"):
            PREPARE(self.folder, self.folder / "recursive.zip", self.assets, VERSION, SOURCE)


if __name__ == "__main__":
    unittest.main()
