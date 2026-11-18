#!/usr/bin/env python3
"""Offline Linux installer regression tests using real native executable fixtures.

Run: python3 tests/install_test.py
All downloads, homes, launchers, and package roots are temporary. No user
installation is read or modified, and no window or network request is created.
"""
from __future__ import annotations

import hashlib
import os
from pathlib import Path
import platform
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT / "install.sh"
C_SOURCE = r"""
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "--version") == 0) {
        const char *probe = getenv("ZTTY_FIXTURE_PROBE");
        if (probe) {
            FILE *out = fopen(probe, "w");
            if (!out) return 80;
            fputs("version executed\n", out);
            fclose(out);
        }
        if (getenv("ZTTY_FIXTURE_FAIL")) return 81;
        const char *override = getenv("ZTTY_FIXTURE_VERSION");
        puts(override ? override : "ztty " ZTTY_TEST_VERSION);
        return 0;
    }
    if (argc == 2 && strcmp(argv[1], "--hold") == 0) {
        puts("ready");
        fflush(stdout);
        for (;;) pause();
    }
    const char *launch = getenv("ZTTY_FIXTURE_LAUNCH");
    if (!launch) return 82;
    FILE *out = fopen(launch, "w");
    if (!out) return 83;
    fprintf(out, "%s\n%s\n", ZTTY_TEST_VERSION, argv[0]);
    fclose(out);
    return 0;
}
"""


@unittest.skipUnless(platform.system() == "Linux", "Linux installer regression")
class InstallerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.fixtures = tempfile.TemporaryDirectory(prefix="ztty-native-fixtures-")
        cls.fixture_root = Path(cls.fixtures.name)
        source = cls.fixture_root / "fixture.c"
        source.write_text(C_SOURCE, encoding="utf-8")
        cls.old = cls.fixture_root / "old-ztty"
        cls.new = cls.fixture_root / "new-ztty"
        compiler = shutil.which("cc")
        if not compiler:
            cls.fixtures.cleanup()
            raise unittest.SkipTest("a C compiler is required for real ELF fixtures")
        for target, version in [(cls.old, "0.1.2"), (cls.new, "0.2.1")]:
            subprocess.run(
                [compiler, "-O2", f'-DZTTY_TEST_VERSION="{version}"', str(source),
                 "-o", str(target)], check=True, capture_output=True,
            )

    @classmethod
    def tearDownClass(cls):
        cls.fixtures.cleanup()

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="ztty-install-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.home = self.root / "home"
        self.home.mkdir()
        self.binary_dir = self.home / ".local/bin"
        self.target = self.binary_dir / "ztty"
        self.desktop = self.home / ".local/share/applications/ztty.desktop"
        self.assets = self.root / "assets"
        self.assets.mkdir()
        self.download_tmp = self.root / "downloads"
        self.download_tmp.mkdir()
        self.mock_bin = self.root / "mock-bin"
        self.mock_bin.mkdir()
        self.curl_log = self.root / "curl.log"
        mock_curl = self.mock_bin / "curl"
        mock_curl.write_text(
            "#!" + sys.executable + "\n"
            "import os, shutil, sys\n"
            "from pathlib import Path\n"
            "args = sys.argv[1:]\n"
            "assert len(args) == 6 and args[:3] == ['-fsSL', '--retry', '3']\n"
            "assert args[4] == '-o'\n"
            "url = args[3]\n"
            "assert url.startswith('https://github.com/LIghtJUNction/ztty/releases/')\n"
            "with open(os.environ['MOCK_CURL_LOG'], 'a') as log: log.write(url + '\\n')\n"
            "shutil.copyfile(Path(os.environ['MOCK_ASSETS']) / url.rsplit('/', 1)[-1], args[5])\n",
            encoding="utf-8",
        )
        mock_curl.chmod(0o755)
        self.env = {
            key: value for key, value in os.environ.items()
            if not key.startswith("ZTTY_") and key not in {
                "HOME", "DESTDIR", "XDG_DATA_HOME", "PREFIX", "BINARY",
                "MAKEFLAGS", "MFLAGS", "MAKEOVERRIDES",
            }
        }
        self.env.update({
            "HOME": str(self.home), "PATH": str(self.mock_bin) + os.pathsep + os.environ["PATH"],
            "TMPDIR": str(self.download_tmp), "MOCK_ASSETS": str(self.assets),
            "MOCK_CURL_LOG": str(self.curl_log),
        })
        architecture = {"amd64": "x86_64", "arm64": "aarch64"}.get(
            platform.machine(), platform.machine(),
        )
        self.asset = f"ztty-linux-{architecture}.tar.gz"
        self.make_archive()

    def make_archive(self, *, symlink=None):
        archive = self.assets / self.asset
        with tarfile.open(archive, "w:gz") as package:
            if symlink is None:
                package.add(self.new, arcname="ztty")
            else:
                member = tarfile.TarInfo("ztty")
                member.type = tarfile.SYMTYPE
                member.linkname = str(symlink)
                package.addfile(member)
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        (self.assets / (self.asset + ".sha256")).write_text(
            f"{digest}  {self.asset}\n", encoding="ascii",
        )

    def run_install(self, *args, success=True, **environment):
        result = subprocess.run(
            ["sh", str(INSTALLER), *map(str, args)],
            env=self.env | {key: str(value) for key, value in environment.items()},
            cwd=ROOT, capture_output=True, text=True, timeout=20,
        )
        if success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(list(self.download_tmp.iterdir()), [])
        self.assertEqual(list(self.home.rglob(".ztty-*")), [])
        return result

    def put_old(self):
        self.binary_dir.mkdir(parents=True)
        shutil.copy2(self.old, self.target)

    def assert_installed(self, target=None):
        target = target or self.target
        self.assertFalse(target.is_symlink())
        self.assertEqual(target.read_bytes(), self.new.read_bytes())
        self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o755)

    def test_verified_latest_download_and_desktop_entry(self):
        self.run_install()
        self.assert_installed()
        self.assertEqual(
            self.curl_log.read_text().splitlines(),
            [f"https://github.com/LIghtJUNction/ztty/releases/latest/download/{name}"
             for name in (self.asset, self.asset + ".sha256")],
        )
        text = self.desktop.read_text()
        self.assertIn(f'Exec="{self.target}"\n', text)
        self.assertIn('StartupWMClass=Ztty\n', text)
        template = (ROOT / "assets/ztty.desktop").read_text().splitlines()
        self.assertEqual(
            [line for line in text.splitlines() if not line.startswith("Exec=")],
            [line for line in template if not line.startswith("Exec=")],
        )
        self.assertEqual(stat.S_IMODE(self.desktop.stat().st_mode), 0o644)
        self.assertFalse((self.binary_dir / "ztty.previous").exists())

    def test_pinned_download_uses_requested_tag(self):
        self.run_install(ZTTY_VERSION="v0.2.1")
        self.assert_installed()
        self.assertTrue(all("/download/v0.2.1/" in line
                            for line in self.curl_log.read_text().splitlines()))

    def test_checksum_failure_preserves_installed_files_without_execution(self):
        self.put_old()
        self.desktop.parent.mkdir(parents=True)
        self.desktop.write_text("old launcher\n")
        marker = self.root / "executed"
        (self.assets / (self.asset + ".sha256")).write_text(
            "0" * 64 + f"  {self.asset}\n", encoding="ascii",
        )
        result = self.run_install(success=False, ZTTY_FIXTURE_PROBE=marker)
        self.assertIn("checksum mismatch", result.stderr)
        self.assertFalse(marker.exists())
        self.assertEqual(self.target.read_bytes(), self.old.read_bytes())
        self.assertEqual(self.desktop.read_text(), "old launcher\n")
        self.assertFalse((self.binary_dir / "ztty.previous").exists())

    def test_invalid_requested_tag_does_not_download(self):
        for version in ("v0.2.1/other", "v0.2.1\nextra"):
            with self.subTest(version=version):
                self.run_install(success=False, ZTTY_VERSION=version)
        self.assertFalse(self.curl_log.exists())
        self.assertFalse(self.target.exists())

    def test_checksum_tool_failure_does_not_allow_local_install(self):
        self.put_old()
        checksum = self.mock_bin / "sha256sum"
        checksum.write_text("#!/bin/sh\nexit 1\n")
        checksum.chmod(0o755)
        marker = self.root / "executed"
        self.run_install("--from-binary", self.new, success=False, ZTTY_FIXTURE_PROBE=marker)
        self.assertFalse(marker.exists())
        self.assertEqual(self.target.read_bytes(), self.old.read_bytes())

    def test_control_characters_in_installation_path_are_rejected(self):
        for suffix in ("with\nnewline", "with\ttab", "with\rcarriage-return"):
            with self.subTest(suffix=suffix):
                destination = self.home / suffix
                self.run_install("--from-binary", self.new, success=False,
                                 ZTTY_INSTALL_DIR=destination)
                self.assertFalse(destination.exists())

    def test_pinned_binary_version_must_match_before_replacement(self):
        self.put_old()
        result = self.run_install(success=False, ZTTY_VERSION="v0.3.0")
        self.assertIn("does not match", result.stderr)
        self.assertEqual(self.target.read_bytes(), self.old.read_bytes())
        self.assertFalse((self.binary_dir / "ztty.previous").exists())

    def test_archive_symlink_is_not_executed(self):
        marker = self.root / "executed"
        self.make_archive(symlink=self.new)
        result = self.run_install(success=False, ZTTY_FIXTURE_PROBE=marker)
        self.assertIn("regular native executable", result.stderr)
        self.assertFalse(marker.exists())
        self.assertFalse(self.target.exists())

    def test_local_input_requires_regular_native_binary(self):
        marker = self.root / "executed"
        script = self.root / "fake-ztty"
        script.write_text(f"#!/bin/sh\ntouch '{marker}'\nprintf 'ztty 0.2.1\\n'\n")
        script.chmod(0o755)
        link = self.root / "linked-ztty"
        link.symlink_to(self.new)
        for candidate in (self.root, script, link):
            with self.subTest(candidate=candidate.name):
                self.run_install("--from-binary", candidate, success=False)
        self.assertFalse(marker.exists())
        self.assertFalse(self.target.exists())
        self.assertFalse(self.curl_log.exists())

    def test_local_runtime_and_version_failures_preserve_old_binary(self):
        self.put_old()
        for env in [
            {"ZTTY_FIXTURE_FAIL": "1"},
            {"ZTTY_FIXTURE_VERSION": "another-program 0.2.1"},
            {"ZTTY_FIXTURE_VERSION": "ztty 0.2.1\nextra output"},
        ]:
            with self.subTest(environment=env):
                self.run_install("--from-binary", self.new, success=False, **env)
                self.assertEqual(self.target.read_bytes(), self.old.read_bytes())
                self.assertFalse((self.binary_dir / "ztty.previous").exists())

    def test_same_source_target_is_rejected(self):
        self.put_old()
        before = self.target.stat().st_ino
        result = self.run_install("--from-binary", self.target, success=False)
        self.assertIn("already the installation target", result.stderr)
        self.assertEqual(self.target.stat().st_ino, before)
        self.assertFalse((self.binary_dir / "ztty.previous").exists())

    def test_directory_target_is_rejected(self):
        self.target.mkdir(parents=True)
        sentinel = self.target / "keep"
        sentinel.write_text("untouched")
        self.run_install("--from-binary", self.new, success=False)
        self.assertEqual(sentinel.read_text(), "untouched")

    def test_running_elf_upgrade_keeps_process_and_verified_rollback(self):
        self.put_old()
        old_inode = self.target.stat().st_ino
        process = subprocess.Popen(
            [str(self.target), "--hold"], stdout=subprocess.PIPE, text=True, env=self.env,
        )
        try:
            self.assertEqual(process.stdout.readline(), "ready\n")
            self.run_install("--from-binary", self.new)
            self.assert_installed()
            self.assertIsNone(process.poll(), "the existing process must keep running")
            self.assertEqual(Path(f"/proc/{process.pid}/exe").stat().st_ino, old_inode)
            backup = self.binary_dir / "ztty.previous"
            self.assertEqual(backup.read_bytes(), self.old.read_bytes())
            subprocess.run(
                ["sha256sum", "-c", "ztty.previous.sha256"], cwd=self.binary_dir,
                check=True, capture_output=True,
            )
            # An identical reinstall must not discard the useful rollback copy.
            self.run_install("--from-binary", self.new)
            self.assertEqual(backup.read_bytes(), self.old.read_bytes())
            # Rollback uses the same checked, atomic local-install path.
            self.run_install("--from-binary", backup)
            self.assertEqual(self.target.read_bytes(), self.old.read_bytes())
            self.assertEqual(backup.read_bytes(), self.new.read_bytes())
            self.assertIsNone(process.poll())
        finally:
            process.terminate()
            process.wait(timeout=5)
            process.stdout.close()

    def test_symlink_targets_preserve_link_and_referent(self):
        for kind in ("file", "directory", "dangling"):
            with self.subTest(kind=kind):
                root = self.root / kind
                root.mkdir()
                referent = root / "referent"
                if kind == "file":
                    referent.write_bytes(b"protected unrelated file")
                elif kind == "directory":
                    referent.mkdir()
                    (referent / "keep").write_text("protected directory")
                destination = root / "bin"
                destination.mkdir()
                target = destination / "ztty"
                target.symlink_to("../referent")
                self.run_install(
                    "--from-binary", self.new, ZTTY_INSTALL_DIR=destination,
                    ZTTY_DESKTOP_DIR=root / "applications",
                )
                self.assert_installed(target)
                backup = destination / "ztty.previous"
                self.assertTrue(backup.is_symlink())
                self.assertEqual(os.readlink(backup), "../referent")
                self.assertFalse((destination / "ztty.previous.sha256").exists())
                if kind == "file":
                    self.assertEqual(referent.read_bytes(), b"protected unrelated file")
                elif kind == "directory":
                    self.assertEqual((referent / "keep").read_text(), "protected directory")
                    self.assertEqual(list(referent.iterdir()), [referent / "keep"])
                else:
                    self.assertFalse(referent.exists())

    def test_backup_and_desktop_symlinks_do_not_overwrite_referents(self):
        self.put_old()
        protected = self.root / "protected"
        protected.write_text("unchanged")
        self.desktop.parent.mkdir(parents=True)
        for target in (
            self.desktop, self.binary_dir / "ztty.previous",
            self.binary_dir / "ztty.previous.sha256",
        ):
            target.symlink_to(protected)
        self.run_install("--from-binary", self.new)
        self.assert_installed()
        self.assertEqual(protected.read_text(), "unchanged")
        self.assertFalse(self.desktop.is_symlink())
        self.assertFalse((self.binary_dir / "ztty.previous").is_symlink())
        self.assertFalse((self.binary_dir / "ztty.previous.sha256").is_symlink())

    @unittest.skipUnless(shutil.which("gio") and shutil.which("desktop-file-validate"),
                         "gio and desktop-file-validate are needed for launcher acceptance")
    def test_launcher_runs_installed_binary_with_escaped_path_and_old_path_entry(self):
        destination = self.home / 'bin 空 格 " $ ` % = \\'
        data = self.home / "application data"
        launch = self.root / "launched"
        shutil.copy2(self.old, self.mock_bin / "ztty")
        self.run_install(
            "--from-binary", self.new, ZTTY_INSTALL_DIR=destination,
            XDG_DATA_HOME=data,
        )
        desktop = data / "applications/ztty.desktop"
        subprocess.run(["desktop-file-validate", str(desktop)], check=True, capture_output=True)
        result = subprocess.run(
            ["gio", "launch", str(desktop)], env=self.env | {"ZTTY_FIXTURE_LAUNCH": str(launch)},
            capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        deadline = time.monotonic() + 3
        while not launch.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertEqual(launch.read_text().splitlines(), ["0.2.1", str(destination / "ztty")])

    def test_make_install_and_uninstall_respect_prefix_and_destdir(self):
        prefix = self.root / "final prefix"
        staging = self.root / "package root"
        command = [
            "make", "-o", "build", f"PREFIX={prefix}", f"DESTDIR={staging}",
            f"BINARY={self.new}",
        ]
        result = subprocess.run(
            [*command, "install"], cwd=ROOT, env=self.env,
            capture_output=True, text=True, timeout=20,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        installed_prefix = staging / str(prefix).lstrip("/")
        target = installed_prefix / "bin/ztty"
        desktop = installed_prefix / "share/applications/ztty.desktop"
        self.assert_installed(target)
        self.assertIn(f'Exec="{prefix}/bin/ztty"\n', desktop.read_text())
        self.assertNotIn(str(staging), desktop.read_text())
        self.assertFalse(prefix.exists())
        neighbor = installed_prefix / "bin/keep"
        neighbor.write_text("unrelated")
        result = subprocess.run(
            [*command, "uninstall"], cwd=ROOT, env=self.env,
            capture_output=True, text=True, timeout=20,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(target.exists())
        self.assertFalse(desktop.exists())
        self.assertEqual(neighbor.read_text(), "unrelated")


if __name__ == "__main__":
    unittest.main(verbosity=2)
