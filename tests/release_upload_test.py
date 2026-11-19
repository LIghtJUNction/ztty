#!/usr/bin/env python3
"""Draft upload contracts with an in-memory gh runner; never access GitHub."""
import hashlib
import json
from pathlib import Path
import runpy
from types import SimpleNamespace
import tempfile
import unittest

MODULE = runpy.run_path(str(Path(__file__).resolve().parents[1] / "tools" / "release_upload.py"))
UPLOAD = MODULE["upload"]
REPO, TAG = "LIghtJUNction/ztty", "v0.4.1"


def metadata(path, **updates):
    payload = path.read_bytes()
    return {"name": path.name, "state": "uploaded", "size": len(payload),
            "digest": "sha256:" + hashlib.sha256(payload).hexdigest(), **updates}


class FakeGitHub:
    def __init__(self, *, exists=True, draft=True, assets=()):
        self.release = {"databaseId": 42, "isDraft": draft, "tagName": TAG} if exists else None
        self.assets = list(assets)
        self.lookup_override = None
        self.failure = None
        self.events = []
        self.commands = []
        self.raw_lookup = None

    def sleep(self, seconds):
        self.events.append(("sleep", seconds))

    def run(self, argv, **options):
        self.commands.append(argv)
        self.assert_options(options)
        if argv[1:3] == ["api", "graphql"]:
            operation = "lookup"
        elif argv[1] == "api":
            operation = "assets"
            if argv[2] != f"repos/{REPO}/releases/42/assets?per_page=100":
                raise AssertionError("asset lookup must use the fixed database ID")
        elif argv[1:3] == ["release", "create"]:
            operation = "create"
        elif argv[1:3] == ["release", "upload"]:
            operation = "upload:" + Path(argv[-1]).name
        else:
            raise AssertionError("unexpected GitHub command: " + repr(argv))
        self.events.append(("request", operation))
        if self.failure and self.failure[0] == operation:
            return SimpleNamespace(returncode=self.failure[1], stdout="",
                                   stderr=self.failure[2])
        if operation == "lookup":
            if self.raw_lookup is not None:
                return SimpleNamespace(returncode=0, stdout=self.raw_lookup, stderr="")
            payload = self.lookup_override if self.lookup_override is not None else {
                "data": {"repository": {"id": "repository-id", "release": self.release}}}
        elif operation == "assets":
            payload = self.assets
        else:
            if "--clobber" in argv:
                raise AssertionError("existing assets must never be clobbered")
            if operation == "create":
                self.release = {"databaseId": 42, "isDraft": True, "tagName": TAG}
            else:
                self.assets.append(metadata(Path(argv[-1])))
            payload = None
        return SimpleNamespace(returncode=0, stdout=json.dumps(payload), stderr="")

    @staticmethod
    def assert_options(options):
        if options != {"capture_output": True, "text": True, "check": False, "timeout": 300}:
            raise AssertionError(options)

    def writes(self):
        return [event[1] for event in self.events if event[0] == "request"
                and (event[1] == "create" or event[1].startswith("upload:"))]


class UploadContracts(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.a = self.root / "a.tar.gz"
        self.b = self.root / "b.json"
        self.a.write_bytes(b"binary package")
        self.b.write_bytes(b'{"source":"accepted"}\n')

    def run_upload(self, gh, files=None):
        return UPLOAD(REPO, TAG, "Raw evidence is retained in the ZIP.",
                      [self.a, self.b] if files is None else files,
                      runner=gh.run, sleeper=gh.sleep)

    def test_creates_empty_draft_then_uploads_serially(self):
        gh = FakeGitHub(exists=False)
        report = self.run_upload(gh)
        self.assertEqual(gh.writes(), ["create", "upload:a.tar.gz", "upload:b.json"])
        create = next(command for command in gh.commands if command[1:3] == ["release", "create"])
        for flag in ("--verify-tag", "--generate-notes", "--draft"):
            self.assertIn(flag, create)
        self.assertNotIn(str(self.a), create)
        self.assertNotIn(str(self.b), create)
        self.assertEqual(report["release_id"], 42)
        self.assertTrue(report["created"])
        self.assertTrue(report["readback_required"])
        self.assertEqual(sum(event == ("request", "lookup") for event in gh.events), 2)
        for index, event in enumerate(gh.events):
            if event[0] == "request" and (event[1] == "create" or event[1].startswith("upload:")):
                self.assertEqual(gh.events[index + 1], ("sleep", 1.0))
        self.assertFalse(any(command[1:3] == ["release", "edit"] for command in gh.commands))

    def test_lookup_failures_never_create_or_upload(self):
        for status in (401, 403, 404, 429, 500):
            with self.subTest(status=status):
                gh = FakeGitHub(exists=False)
                gh.failure = ("lookup", 1, f"HTTP {status}")
                with self.assertRaises(RuntimeError):
                    self.run_upload(gh)
                self.assertEqual(gh.writes(), [])

    def test_graphql_errors_null_repository_and_bad_identity_fail_closed(self):
        responses = [
            {"errors": [{"message": "rate limited"}], "data": {"repository": {"id": "r", "release": None}}},
            {"data": {"repository": None}},
            {"data": {"repository": {"id": "r"}}},
            {"data": {"repository": {"id": "r", "release": {"databaseId": 42, "isDraft": "true", "tagName": TAG}}}},
            {"data": {"repository": {"id": "r", "release": {"databaseId": True, "isDraft": True, "tagName": TAG}}}},
            {"data": {"repository": {"id": "r", "release": {"databaseId": 42, "isDraft": True, "tagName": "v9.9.9"}}}},
        ]
        for payload in responses:
            with self.subTest(payload=payload):
                gh = FakeGitHub()
                gh.lookup_override = payload
                with self.assertRaises(ValueError):
                    self.run_upload(gh)
                self.assertEqual(gh.writes(), [])
        gh = FakeGitHub()
        gh.raw_lookup = "not JSON"
        with self.assertRaisesRegex(ValueError, "invalid JSON"):
            self.run_upload(gh)
        self.assertEqual(gh.writes(), [])

    def test_resume_only_uploads_missing_assets(self):
        gh = FakeGitHub(assets=[metadata(self.a)])
        result = self.run_upload(gh)
        self.assertEqual(gh.writes(), ["upload:b.json"])
        self.assertEqual(result["existing"], ["a.tar.gz"])
        self.assertEqual(result["uploaded"], ["b.json"])
        self.assertEqual(sum(event == ("request", "lookup") for event in gh.events), 1)

    def test_incomplete_conflicting_or_duplicate_remote_assets_prevent_writes(self):
        cases = [
            [metadata(self.a, state="starter", size=0)],
            [metadata(self.a, size=100)],
            [metadata(self.a, digest="sha256:" + "0" * 64)],
            [metadata(self.a, digest="unrecognized")],
            [metadata(self.a), metadata(self.a)],
            [metadata(self.a, name="../a.tar.gz")],
        ]
        for assets in cases:
            with self.subTest(assets=assets):
                gh = FakeGitHub(assets=assets)
                with self.assertRaises(ValueError):
                    self.run_upload(gh)
                self.assertEqual(gh.writes(), [])

    def test_missing_digest_defers_content_verification_to_readback(self):
        gh = FakeGitHub(assets=[metadata(self.a, digest=None), metadata(self.b, digest=None)])
        result = self.run_upload(gh)
        self.assertEqual(gh.writes(), [])
        self.assertTrue(result["readback_required"])

    def test_published_release_never_writes_even_when_assets_are_missing(self):
        gh = FakeGitHub(draft=False, assets=[metadata(self.a)])
        result = self.run_upload(gh)
        self.assertEqual(gh.writes(), [])
        self.assertFalse(result["draft"])
        self.assertEqual(result["missing_from_published_release"], ["b.json"])
        self.assertTrue(result["readback_required"])

    def test_upload_failure_stops_before_next_asset_without_retry(self):
        gh = FakeGitHub()
        gh.failure = ("upload:a.tar.gz", 1, "HTTP 429: secondary rate limit")
        with self.assertRaisesRegex(RuntimeError, "429"):
            self.run_upload(gh)
        self.assertEqual(gh.writes(), ["upload:a.tar.gz"])
        self.assertEqual(gh.events[-1], ("sleep", 1.0))

    def test_create_failure_stops_without_lookup_retry_or_upload(self):
        gh = FakeGitHub(exists=False)
        gh.failure = ("create", 1, "HTTP 502")
        with self.assertRaises(RuntimeError):
            self.run_upload(gh)
        self.assertEqual(gh.writes(), ["create"])
        self.assertEqual(sum(event == ("request", "lookup") for event in gh.events), 1)

    def test_local_and_remote_asset_bounds_are_enforced(self):
        files = []
        for index in range(65):
            path = self.root / f"asset-{index:02}.json"
            path.write_bytes(b"data")
            files.append(path)
        gh = FakeGitHub()
        with self.assertRaisesRegex(ValueError, "1..64"):
            self.run_upload(gh, files)
        self.assertEqual(gh.commands, [])
        gh = FakeGitHub(assets=[metadata(path) for path in files])
        with self.assertRaisesRegex(ValueError, "too many assets"):
            self.run_upload(gh, [self.a])
        self.assertEqual(gh.writes(), [])
        gh = FakeGitHub(assets=[metadata(path) for path in files[:64]])
        with self.assertRaisesRegex(ValueError, "combined"):
            self.run_upload(gh, [self.a])
        self.assertEqual(gh.writes(), [])
        gh = FakeGitHub(assets=[metadata(path) for path in files[:64]])
        self.assertTrue(self.run_upload(gh, files[:64])["readback_required"])
        self.assertEqual(gh.writes(), [])

    def test_empty_duplicate_unsafe_and_symlink_local_paths_fail_before_network(self):
        empty = self.root / "empty.log"
        empty.write_bytes(b"")
        unsafe = self.root / "unsafe#label"
        unsafe.write_bytes(b"data")
        other = self.root / "nested"
        other.mkdir()
        duplicate = other / self.a.name
        duplicate.write_bytes(b"other")
        cases = [[], [empty], [unsafe], [self.a, duplicate], [self.root]]
        link = self.root / "linked.bin"
        try:
            link.symlink_to(self.a)
        except OSError:
            pass
        else:
            cases.append([link])
        for paths in cases:
            with self.subTest(paths=paths):
                gh = FakeGitHub()
                with self.assertRaises(ValueError):
                    self.run_upload(gh, paths)
                self.assertEqual(gh.commands, [])

    def test_asset_query_failure_stops_before_upload(self):
        gh = FakeGitHub()
        gh.failure = ("assets", 1, "HTTP 403")
        with self.assertRaises(RuntimeError):
            self.run_upload(gh)
        self.assertEqual(gh.writes(), [])


if __name__ == "__main__":
    unittest.main()
