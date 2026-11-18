#!/usr/bin/env python3
"""Recover the fixed v0.3.1 release from its original accepted artifacts.

The audit command is read-only with respect to GitHub and crates.io.
Only the explicit publish command can create/upload/publish a GitHub Release.
It never builds software, uploads to crates.io, changes tags, or overwrites assets.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import subprocess
import sys
import tarfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile

REPO = "LIghtJUNction/ztty"
REPO_ID = 1461427790
VERSION = "0.3.1"
TAG = "v" + VERSION
SOURCE = "cbcd480f861904a2265af52ab5ca8a5e19cebe9d"
TAG_OBJECT = "783178f40c4f732c3520c36e2690ba97b027b852"
RUN = 44375302476
ATTEMPT = 1
FAILED_PUBLISH_JOB = 135065712440
KNOWN_DRAFT_ID = 450445933
CRATE_SHA256 = "86d4bb0683f7809f7bec61783b50423554e9d29e4ceb3aca26e625055920716b"
BUNDLE = "ztty-0.3.1-native-evidence.zip"
ARTIFACTS = {
    "ztty-linux-x86_64": (14462525952, 1127914, "47fa4cefb1ddcf5baa735f3b45b62a77f063186c2e22f6647fd1f0baf2191c02"),
    "ztty-linux-aarch64": (14461920104, 1100008, "8a3b6eacc323fcb8be1a5299b1b4c58c974bf6728d95c990be05e519920e6af8"),
    "ztty-macos-aarch64": (14461647359, 442663, "9ae988efd691d3cba776b39bf884c4964e276cc5d18fe6b5db2cc08f1000eb2c"),
    "ztty-macos-x86_64": (14460743009, 476041, "b87078596154770ab8d991ebb6bf918467cf6b2d885012979ee1a9aa4f4400ae"),
    "ztty-windows-aarch64": (14461452592, 1075088, "68e856287f64f4433bdd4948b1ce47fc76cd82c59803602b9792bbda06316837"),
    "ztty-windows-x86_64": (14460907598, 1141037, "c06f1e66e7e8a76ec17892689e1ee17c4ae20b1e4b1c1fd33ca3b6d14a74e5c4"),
}
GATE_JOBS = {
    135052765903: "version",
    135052844126: "macos (macos-15-intel,",
    135052844191: "macos (macos-15,",
    135052844216: "windows (windows-11-arm,",
    135052844446: "windows (windows-2025,",
    135052844251: "linux (ubuntu-24.04,",
    135052844277: "linux (ubuntu-24.04-arm,",
    135052844405: "verify / checks",
}
OUTCOMES = {
    "linux": {"toolchain", "build", "native", "package", "evidence"},
    "macos": {"toolchain", "build", "spawn", "native", "package"},
    "windows": {"toolchain", "build", "native", "package"},
}
MAX_DOWNLOAD = 128 * 1024 * 1024
MAX_ZIP_TOTAL = 256 * 1024 * 1024


class Invalid(RuntimeError):
    pass


class Unavailable(RuntimeError):
    pass


class ApiError(Unavailable):
    def __init__(self, status: int, method: str, path: str):
        self.status = status
        super().__init__(f"GitHub {method} {path}: HTTP {status}")


def require(condition, message):
    if not condition:
        raise Invalid(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def encoded(value):
    return (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=True) + "\n").encode("utf-8")


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_bytes(encoded(value))
    temporary.replace(path)


def flat_name(name):
    require(isinstance(name, str) and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", name), "unsafe asset filename")
    return name


def zip_name(name):
    require(isinstance(name, str) and name and "\\" not in name and "\x00" not in name, "unsafe ZIP path")
    path = PurePosixPath(name)
    require(not path.is_absolute() and ".." not in path.parts, "unsafe ZIP traversal")
    require(str(path) == name.rstrip("/"), "noncanonical ZIP path")
    return path


def bounded_read(response, limit=MAX_DOWNLOAD):
    data = response.read(limit + 1)
    require(len(data) <= limit, "download exceeded its size bound")
    return data


def public_host(url):
    parts = urllib.parse.urlsplit(url)
    host = parts.hostname or ""
    require(parts.scheme == "https" and parts.username is None and parts.password is None,
            "non-HTTPS or credential-bearing download URL")
    require(host in {"github.com", "raw.githubusercontent.com", "static.crates.io"}
            or host.endswith(".githubusercontent.com")
            or host.endswith(".blob.core.windows.net"), "unexpected download host")
    return url


class PublicRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        public_host(new_url)
        # This opener never receives Authorization, including after redirects.
        return urllib.request.Request(new_url, headers={"User-Agent": "ztty-release-recovery/0.3.1"})


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        return None


def download_public(url):
    public_host(url)
    try:
        request = urllib.request.Request(url, headers={"User-Agent": "ztty-release-recovery/0.3.1"})
        with urllib.request.build_opener(PublicRedirect).open(request, timeout=30) as response:
            return bounded_read(response)
    except Invalid:
        raise
    except (urllib.error.URLError, TimeoutError, OSError) as error:
        # Signed download URLs and their query strings are never logged.
        status = getattr(error, "code", None)
        raise Unavailable(f"public download unavailable (HTTP {status or 'network error'})") from None


class GitHub:
    def __init__(self):
        self.token = os.environ.get("GH_TOKEN", "")
        require(bool(self.token), "GH_TOKEN must be the workflow's ephemeral token")
        self.opener = urllib.request.build_opener(NoRedirect)

    def request(self, method, path, payload=None, *, binary=False, upload=False):
        require(path.startswith("/repos/" + REPO + "/"), "API request outside the fixed repository")
        host = "uploads.github.com" if upload else "api.github.com"
        data = payload if isinstance(payload, bytes) else (encoded(payload) if payload is not None else None)
        headers = {
            "Authorization": "Bearer " + self.token,
            "User-Agent": "ztty-release-recovery/0.3.1",
            "X-GitHub-Api-Version": "2022-11-28",
            "Accept": "application/octet-stream" if binary else "application/vnd.github+json",
        }
        if data is not None:
            headers["Content-Type"] = "application/octet-stream" if upload else "application/json"
            headers["Content-Length"] = str(len(data))
        try:
            request = urllib.request.Request("https://" + host + path, data=data, method=method, headers=headers)
            with self.opener.open(request, timeout=30) as response:
                raw = bounded_read(response)
                return raw if binary else (json.loads(raw) if raw else None)
        except urllib.error.HTTPError as error:
            if method == "GET" and binary and error.code in (301, 302, 303, 307, 308):
                # Authorization remains on api.github.com. A fresh unauthenticated
                # request follows the signed artifact/asset/log redirect.
                location = error.headers.get("Location")
                require(bool(location), "binary response omitted its download location")
                return download_public(location)
            raise ApiError(error.code, method, path) from None
        except (urllib.error.URLError, TimeoutError, OSError):
            raise Unavailable("GitHub API network request failed") from None

    def get(self, suffix, **options):
        return self.request("GET", "/repos/" + REPO + "/" + suffix, **options)

    def pages(self, suffix, field=None):
        result = []
        separator = "&" if "?" in suffix else "?"
        for page in range(1, 21):
            value = self.get(f"{suffix}{separator}per_page=100&page={page}")
            entries = value[field] if field else value
            require(isinstance(entries, list), "unexpected paginated API response")
            result.extend(entries)
            if len(entries) < 100:
                return result
        raise Invalid("pagination exceeded its bound")


def audit_tag(api):
    ref = api.get("git/ref/tags/" + TAG)
    require(ref["object"]["type"] == "tag" and ref["object"]["sha"] == TAG_OBJECT,
            "the original annotated release tag changed")
    obj = api.get("git/tags/" + TAG_OBJECT)
    require(obj["tag"] == TAG and obj["object"] == {
        "sha": SOURCE, "type": "commit",
        "url": f"https://api.github.com/repos/{REPO}/git/commits/{SOURCE}",
    }, "release tag no longer identifies the accepted source")


def audit_gates(api):
    audit_tag(api)
    run = api.get(f"actions/runs/{RUN}/attempts/{ATTEMPT}")
    require(run["id"] == RUN and run["run_attempt"] == ATTEMPT
            and run["head_sha"] == SOURCE and run["head_branch"] == TAG
            and run["event"] == "push" and run["path"] == ".github/workflows/release.yml"
            and run["status"] == "completed" and run["conclusion"] == "failure"
            and run["repository"]["id"] == REPO_ID, "original release run identity changed")
    jobs = {job["id"]: job for job in api.pages(
        f"actions/runs/{RUN}/attempts/{ATTEMPT}/jobs", "jobs")}
    result = []
    for job_id, prefix in GATE_JOBS.items():
        job = jobs.get(job_id, {})
        require(job.get("run_id") == RUN and job.get("head_sha") == SOURCE
                and job.get("name", "").startswith(prefix)
                and job.get("status") == "completed" and job.get("conclusion") == "success",
                f"original successful gate is missing: {job_id}")
        result.append({"id": job_id, "name": job["name"], "conclusion": "success"})
    failed = jobs.get(FAILED_PUBLISH_JOB, {})
    require(failed.get("name") == "publish" and failed.get("conclusion") == "failure",
            "original publish failure is missing")
    steps = {step["name"]: step["conclusion"] for step in failed["steps"]}
    for step in ("Prepare source and test Arch package",
                 "Verify every platform artifact belongs to the accepted source"):
        require(steps.get(step) == "success", "original publication prerequisite was not successful: " + step)
    require(steps.get("Publish or verify GitHub Release") == "failure",
            "original failure was not the attachment upload step")
    return {"passed": True, "source_sha": SOURCE, "run_id": RUN, "run_attempt": ATTEMPT,
            "jobs": result, "arch_package_gate": "success", "artifact_ledger_gate": "success",
            "original_publish_job": FAILED_PUBLISH_JOB, "original_publish_result": "failure"}


def read_original_zip(stem, data):
    artifact_id, byte_count, sha = ARTIFACTS[stem]
    require(len(data) == byte_count and digest(data) == sha, "original ZIP identity mismatch: " + stem)
    files, names, total = {}, set(), 0
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        for item in archive.infolist():
            path = zip_name(item.filename)
            require(item.filename not in names, "duplicate original ZIP entry")
            names.add(item.filename)
            mode = stat.S_IFMT(item.external_attr >> 16)
            require(mode in (0, stat.S_IFREG, stat.S_IFDIR), "nonregular original ZIP entry")
            if item.is_dir():
                continue
            require(mode != stat.S_IFDIR, "ZIP file has directory attributes")
            total += item.file_size
            require(item.file_size <= 64 * 1024 * 1024 and total <= MAX_ZIP_TOTAL, "ZIP contents exceed size bounds")
            payload = archive.read(item)
            if len(path.parts) == 1:
                files[flat_name(item.filename)] = payload
    build_name = stem + ".build.json"
    build = json.loads(files[build_name])
    platform = stem.split("-")[1]
    require(build["source_sha"] == SOURCE and build["source_ref"] == "refs/tags/" + TAG
            and build["version"] == VERSION and build["run_id"] == str(RUN)
            and build["run_attempt"] == str(ATTEMPT) and build["build_only"] is False
            and build["candidate_platform"] == "all" and build["passed"] is True
            and build["status"] == "accepted", "build provenance mismatch: " + stem)
    require(set(build["outcomes"]) == OUTCOMES[platform]
            and set(build["outcomes"].values()) == {"success"}, "build outcome mismatch: " + stem)
    if platform == "windows":
        require(build["paint_diagnostics_enabled"] is False
                and build["nc_rendering_diagnostics_enabled"] is False
                and build["resize_control_diagnostic"] == {"requested": False, "outcome": "skipped"},
                "diagnostic Windows artifacts cannot be published")
    ledger = build["files"]
    require(set(files) == set(ledger) | {build_name, build_name + ".sha256"},
            "original ZIP top-level files do not match the build ledger: " + stem)
    for name, entry in ledger.items():
        flat_name(name)
        require(type(entry["bytes"]) is int and entry["bytes"] >= 0
                and len(files[name]) == entry["bytes"] and digest(files[name]) == entry["sha256"],
                "per-file artifact identity mismatch: " + name)
    native = json.loads(files[stem + ".native.json"])
    require(native["passed"] is True and native["source_sha"] == SOURCE, "native gate failed: " + stem)
    if platform == "macos":
        spawn = json.loads(files[stem + ".spawn.json"])
        require(spawn["passed"] is True and spawn["source_sha"] == SOURCE, "macOS spawn gate failed")
    verify_checksums(files)
    return files, {"id": artifact_id, "name": stem, "zip_bytes": byte_count, "zip_sha256": sha,
                   "total_zip_entries": len(names), "top_level_files": len(files),
                   "empty_top_level_files": sorted(name for name, payload in files.items() if not payload),
                   "build_report_sha256": digest(files[build_name]), "passed": True}


def verify_checksums(files):
    for filename, payload in files.items():
        if not (filename.endswith(".sha256") or filename == "SHA256SUMS"):
            continue
        lines = payload.decode("ascii").splitlines()
        require(bool(lines), "empty checksum file: " + filename)
        for line in lines:
            match = re.fullmatch(r"([0-9a-f]{64}) [ *](.+)", line)
            require(match is not None, "malformed checksum file: " + filename)
            sha, name = match.groups()
            flat_name(name)
            require(name in files and digest(files[name]) == sha, "checksum failed: " + name)


def bundle_bytes(originals, manifest):
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_STORED) as archive:
        entries = {"manifest.json": encoded(manifest)}
        entries.update({"artifacts/" + stem + ".zip": data for stem, data in originals.items()})
        for name, payload in sorted(entries.items()):
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.create_system = 3
            info.external_attr = (stat.S_IFREG | 0o644) << 16
            info.compress_type = zipfile.ZIP_STORED
            archive.writestr(info, payload)
    return output.getvalue()


def inspect_bundle(data, expected_manifest):
    require(expected_manifest["repository"] == REPO and expected_manifest["release_tag"] == TAG
            and expected_manifest["source_sha"] == SOURCE and expected_manifest["original_run_id"] == RUN
            and expected_manifest["original_run_attempt"] == ATTEMPT, "evidence manifest identity mismatch")
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        expected = {"manifest.json"} | {"artifacts/" + stem + ".zip" for stem in ARTIFACTS}
        require(len(archive.infolist()) == len(expected) and set(archive.namelist()) == expected,
                "recovery evidence ZIP has unexpected entries")
        require(archive.read("manifest.json") == encoded(expected_manifest), "evidence manifest changed")
        empty, records = {}, []
        for stem in sorted(ARTIFACTS):
            files, record = read_original_zip(stem, archive.read("artifacts/" + stem + ".zip"))
            records.append(record)
            empty.update({name: digest(payload) for name, payload in files.items() if not payload})
        require(records == expected_manifest["artifacts"], "evidence artifact records changed")
        require(empty == expected_manifest["empty_top_level_files"], "empty evidence was not preserved")
        return {"passed": True, "bytes": len(data), "sha256": digest(data),
                "original_archives_verified": len(ARTIFACTS), "empty_files_verified": len(empty)}


def source_identity(source):
    def git(*arguments):
        return subprocess.check_output(["git", *arguments], cwd=source, text=True).strip()
    require(git("rev-parse", "HEAD") == SOURCE, "source checkout is not the final release source")
    require(not git("status", "--porcelain", "--untracked-files=all"), "release source checkout is dirty")


def audit_registry(out):
    result = {"crate": "ztty", "version": VERSION, "expected_sha256": CRATE_SHA256,
              "source_sha_expected": SOURCE, "read_only": True, "passed": False}
    directory = out / "registry"
    directory.mkdir(exist_ok=True)
    try:
        index = download_public("https://raw.githubusercontent.com/rust-lang/crates.io-index/master/zt/ty/ztty")
        versions = [json.loads(line) for line in index.splitlines() if line]
        versions = [entry for entry in versions if entry.get("vers") == VERSION]
        require(len(versions) == 1 and versions[0]["cksum"] == CRATE_SHA256
                and versions[0]["yanked"] is False, "official registry index identity mismatch")
        payload = download_public(f"https://static.crates.io/crates/ztty/ztty-{VERSION}.crate")
        require(digest(payload) == CRATE_SHA256, "official crate archive checksum mismatch")
        (directory / f"ztty-{VERSION}.crate").write_bytes(payload)
        with tarfile.open(fileobj=io.BytesIO(payload), mode="r:gz") as archive:
            members = archive.getmembers()
            names = [member.name for member in members]
            require(len(names) == len(set(names)), "crate contains duplicate archive names")
            require(sum(member.size for member in members) <= MAX_ZIP_TOTAL, "crate expands beyond size bound")
            for member in members:
                path = zip_name(member.name)
                require(path.parts[0] == "ztty-" + VERSION and (member.isfile() or member.isdir()),
                        "crate contains an unexpected path or file type")
                require("ztty-ui" not in path.parts, "experimental UI appeared in the release crate")
            def read(name):
                member = archive.getmember("ztty-" + VERSION + "/" + name)
                require(member.isfile() and member.size < 1024 * 1024, "crate metadata is not bounded")
                return archive.extractfile(member).read()
            vcs = json.loads(read(".cargo_vcs_info.json"))
            require(vcs["git"]["sha1"] == SOURCE and vcs["git"].get("dirty", False) is False
                    and vcs["path_in_vcs"] == "", "published crate VCS source is not the final clean release")
            import tomllib
            manifest = tomllib.loads(read("Cargo.toml").decode("utf-8"))
            require(manifest["package"]["name"] == "ztty" and manifest["package"]["version"] == VERSION,
                    "published crate name or version mismatch")
            dependencies = manifest["dependencies"]
            require(dependencies["zterm"]["version"] == "0.3.0"
                    and dependencies["zpty"]["version"] == "0.2.1"
                    and "ztty-ui" not in dependencies, "published crate dependency mismatch")
            for name in ("zterm", "zpty"):
                require(not any(key in dependencies[name] for key in ("path", "git", "registry", "registry-index")),
                        "published dependency is not from the default registry")
        result.update({"passed": True, "status": "verified", "sha256": digest(payload),
                       "archive_bytes": len(payload), "vcs": vcs,
                       "dependencies": {"zterm": "0.3.0", "zpty": "0.2.1"}})
    except Unavailable as error:
        # A registry outage does not invalidate already accepted GitHub binaries.
        result.update({"status": "unavailable", "error": str(error)})
    except Exception as error:
        result.update({"status": "identity_mismatch", "error": str(error)})
        write_json(directory / "audit.json", result)
        raise
    write_json(directory / "audit.json", result)
    return result


def audit(api, source, out):
    source_identity(source)
    gates = audit_gates(api)
    original_directory = out / "original-artifacts"
    assets_directory = out / "assets"
    original_directory.mkdir(exist_ok=True)
    assets_directory.mkdir(exist_ok=True)
    originals, assets, records, empty = {}, {}, [], {}
    for stem, (artifact_id, byte_count, sha) in sorted(ARTIFACTS.items()):
        metadata = api.get(f"actions/artifacts/{artifact_id}")
        require(metadata["id"] == artifact_id and metadata["name"] == stem
                and metadata["size_in_bytes"] == byte_count and metadata["digest"] == "sha256:" + sha
                and metadata["expired"] is False
                and metadata["workflow_run"]["id"] == RUN
                and metadata["workflow_run"]["head_sha"] == SOURCE,
                "original artifact metadata changed: " + stem)
        data = api.get(f"actions/artifacts/{artifact_id}/zip", binary=True)
        files, record = read_original_zip(stem, data)
        records.append(record)
        originals[stem] = data
        (original_directory / (stem + ".zip")).write_bytes(data)
        for name, payload in files.items():
            if payload:
                require(name not in assets or assets[name] == payload, "conflicting artifact filenames")
                assets[name] = payload
            else:
                empty[name] = digest(payload)
    manifest = {"schema": 1, "repository": REPO, "release_tag": TAG, "source_sha": SOURCE,
                "original_run_id": RUN, "original_run_attempt": ATTEMPT,
                "artifacts": records, "empty_top_level_files": empty,
                "preservation": "The six original Actions ZIPs are retained byte-for-byte, including empty logs and nested evidence."}
    evidence = bundle_bytes(originals, manifest)
    bundle_audit = inspect_bundle(evidence, manifest)
    assets[BUNDLE] = evidence
    assets[BUNDLE + ".sha256"] = (digest(evidence) + "  " + BUNDLE + "\n").encode("ascii")
    environment = {key: value for key, value in os.environ.items() if key not in ("GH_TOKEN", "GITHUB_TOKEN")}
    subprocess.run(["bash", "tools/release-source.sh", str(out)], cwd=source,
                   env=environment, check=True, timeout=120, stdout=subprocess.PIPE)
    for name in ("ztty-" + VERSION + ".tar.gz", "SHA256SUMS"):
        assets[name] = (out / name).read_bytes()
    assets["install.sh"] = (source / "install.sh").read_bytes()
    verify_checksums({**assets, **{name: b"" for name in empty}})
    for name, payload in assets.items():
        flat_name(name)
        require(bool(payload), "an empty file was selected for standalone upload")
        (assets_directory / name).write_bytes(payload)
    registry = audit_registry(out)
    source_identity(source)
    plan = {"schema": 1, "repository": REPO, "source_sha": SOURCE, "tag": TAG,
            "helper_sha256": digest(Path(__file__).read_bytes()), "bundle_manifest": manifest,
            "assets": {name: {"bytes": len(payload), "sha256": digest(payload)}
                       for name, payload in sorted(assets.items())}}
    write_json(out / "plan.json", plan)
    result = {"schema": 1, "passed": True, "read_only": True, "source_sha": SOURCE, "tag": TAG,
              "helper_sha256": plan["helper_sha256"], "original_gates": gates,
              "artifact_audit": {"passed": True, "count": len(records), "records": records},
              "evidence_bundle": bundle_audit, "registry": registry,
              "plan_sha256": digest((out / "plan.json").read_bytes()),
              "standalone_asset_count": len(assets), "omitted_empty_standalone_files": sorted(empty)}
    write_json(out / "audit.json", result)
    return result


def prepared(api, source, out):
    source_identity(source)
    audit_result = json.loads((out / "audit.json").read_bytes())
    plan_data = (out / "plan.json").read_bytes()
    plan = json.loads(plan_data)
    require(audit_result["passed"] is True and audit_result["source_sha"] == SOURCE
            and audit_result["tag"] == TAG and audit_result["plan_sha256"] == digest(plan_data),
            "audit does not authorize this publication plan")
    require(plan["repository"] == REPO and plan["source_sha"] == SOURCE and plan["tag"] == TAG
            and plan["helper_sha256"] == digest(Path(__file__).read_bytes()),
            "publication helper or plan identity changed")
    assets = {}
    for name, expected in plan["assets"].items():
        flat_name(name)
        payload = (out / "assets" / name).read_bytes()
        require(payload and len(payload) == expected["bytes"] and digest(payload) == expected["sha256"],
                "prepared asset changed: " + name)
        assets[name] = payload
    require(BUNDLE in assets and "SHA256SUMS" in assets and "install.sh" in assets
            and "ztty-" + VERSION + ".tar.gz" in assets, "required release payload is missing")
    inspect_bundle(assets[BUNDLE], plan["bundle_manifest"])
    for stem in ARTIFACTS:
        require(stem + ".build.json" in assets and stem + ".native.json" in assets,
                "a platform's release provenance is missing")
    empty = plan["bundle_manifest"]["empty_top_level_files"]
    verify_checksums({**assets, **{name: b"" for name in empty}})
    audit_result["original_gates"] = audit_gates(api)
    return audit_result, plan, assets


def release_candidates(api):
    found = {}
    try:
        known = api.get(f"releases/{KNOWN_DRAFT_ID}")
        require(known["tag_name"] == TAG, "the original partial release was retargeted")
        found[known["id"]] = known
    except ApiError as error:
        if error.status != 404:
            raise
    for release in api.pages("releases"):
        if release["tag_name"] == TAG:
            found[release["id"]] = release
    require(len(found) <= 1, "multiple releases refer to the fixed tag")
    return next(iter(found.values()), None)


def release_assets(api, release_id):
    result = {}
    for asset in api.pages(f"releases/{release_id}/assets"):
        require(asset["name"] not in result, "duplicate release asset names")
        result[asset["name"]] = asset
    return result


def verified_asset(api, asset, expected):
    require(asset["state"] == "uploaded", "existing asset is unfinished: " + asset["name"])
    require(asset["size"] == len(expected), "existing asset size differs: " + asset["name"])
    payload = api.get(f"releases/assets/{asset['id']}", binary=True)
    require(payload == expected, "existing release asset bytes differ: " + asset["name"])
    return {"id": asset["id"], "name": asset["name"], "bytes": len(payload), "sha256": digest(payload)}


def ensure_asset(api, release_id, name, payload):
    require(payload, "refusing a standalone zero-byte asset")
    existing = release_assets(api, release_id).get(name)
    if existing is not None:
        return verified_asset(api, existing, payload)
    path = f"/repos/{REPO}/releases/{release_id}/assets?" + urllib.parse.urlencode({"name": name})
    try:
        asset = api.request("POST", path, payload, upload=True)
    except ApiError as error:
        if error.status not in (409, 422):
            raise
        # Another publisher may have uploaded the same bytes. Never retry POST
        # or overwrite; verify the existing asset instead.
        asset = release_assets(api, release_id).get(name)
        require(asset is not None, "asset creation conflicted without a visible matching asset")
    require(asset["name"] == name, "upload returned the wrong asset identity")
    return verified_asset(api, asset, payload)


def recovery_notes():
    return (
        "<!-- ztty-v031-publication-recovery -->\n"
        "## Publication recovery\n\n"
        f"Published from fixed tag {TAG}, source {SOURCE}. The original "
        f"[run {RUN}](https://github.com/{REPO}/actions/runs/{RUN}) passed all six native "
        "architecture gates, Verify, and source/Arch package verification. Its final "
        "attachment upload failed when GitHub rejected a zero-byte stderr log.\n\n"
        f"{BUNDLE} retains all six original Actions ZIPs byte-for-byte, including "
        "empty logs and nested evidence. Nonempty release assets are checked against "
        "their original build ledgers and downloaded again before publication. "
        "The original failed upload remains recorded in Actions.\n"
    )


def latest_policy(api):
    try:
        latest = api.get("releases/latest")
    except ApiError as error:
        if error.status == 404:
            return "true"
        raise
    match = re.fullmatch(r"v([0-9]+)\.([0-9]+)\.([0-9]+)", latest["tag_name"])
    if match and tuple(map(int, match.groups())) <= (0, 3, 1):
        return "true"
    return "false"


def publish(api, source, out):
    audit_result, plan, assets = prepared(api, source, out)
    empty = plan["bundle_manifest"]["empty_top_level_files"]
    release = release_candidates(api)
    if release is None:
        audit_tag(api)
        try:
            release = api.request("POST", f"/repos/{REPO}/releases", {
                "tag_name": TAG, "target_commitish": SOURCE, "name": "ztty " + VERSION,
                "body": recovery_notes(), "draft": True, "prerelease": False,
            })
        except ApiError as error:
            if error.status not in (409, 422):
                raise
            release = release_candidates(api)
            require(release is not None, "release creation conflicted without a visible matching release")
    require(release["tag_name"] == TAG and release["prerelease"] is False,
            "existing release does not have the expected stable identity")
    release_id = release["id"]
    state = {"schema": 1, "passed": False, "mode": "publish", "source_sha": SOURCE, "tag": TAG,
             "original_gates": audit_result["original_gates"],
             "artifact_audit": audit_result["artifact_audit"], "registry": audit_result["registry"],
             "evidence_bundle": audit_result["evidence_bundle"],
             "release": {"id": release_id, "url": release["html_url"], "draft_before": release["draft"]},
             "removed_empty_starters": [], "asset_readbacks": []}
    write_json(out / "publication-progress.json", state)
    existing = release_assets(api, release_id)
    # Check every existing payload before making the first attachment change.
    # A mismatched or unfinished nonempty asset requires investigation.
    for name, payload in assets.items():
        if name in existing:
            verified_asset(api, existing[name], payload)
    for name in empty:
        if name not in existing:
            continue
        item = existing[name]
        require(item["size"] == 0, "an originally empty log now has different content")
        if item["state"] == "uploaded":
            verified_asset(api, item, b"")
        else:
            require(item["state"] == "starter", "unexpected empty asset state")
    # Preserve and read back original ZIPs before removing an empty failed upload
    # placeholder. Completed assets, including valid empty assets, are retained.
    ensure_asset(api, release_id, BUNDLE, assets[BUNDLE])
    for name in sorted(empty):
        item = release_assets(api, release_id).get(name)
        if item is not None and item["state"] == "starter":
            require(item["size"] == 0 and digest(b"") == empty[name], "empty placeholder identity changed")
            api.request("DELETE", f"/repos/{REPO}/releases/assets/{item['id']}")
            state["removed_empty_starters"].append({"id": item["id"], "name": name})
    for name, payload in sorted(assets.items()):
        ensure_asset(api, release_id, name, payload)
    current = release_assets(api, release_id)
    for name, payload in sorted(assets.items()):
        require(name in current, "release readback is missing an asset: " + name)
        state["asset_readbacks"].append(verified_asset(api, current[name], payload))
    read_bundle = api.get(f"releases/assets/{current[BUNDLE]['id']}", binary=True)
    state["evidence_bundle"] = inspect_bundle(read_bundle, plan["bundle_manifest"])
    state["release_readback"] = {"passed": True, "nonempty_assets": len(assets),
                                 "empty_files_preserved": len(empty)}
    audit_tag(api)
    source_identity(source)
    release = api.get(f"releases/{release_id}")
    require(release["tag_name"] == TAG and release["prerelease"] is False, "release identity changed")
    if release["draft"]:
        body = release.get("body") or ""
        if "<!-- ztty-v031-publication-recovery -->" not in body:
            body = (body.rstrip() + "\n\n" + recovery_notes()).lstrip()
        release = api.request("PATCH", f"/repos/{REPO}/releases/{release_id}",
                              {"draft": False, "body": body, "make_latest": latest_policy(api)})
    require(release["draft"] is False and release["tag_name"] == TAG, "release was not published")
    audit_tag(api)
    published = api.get(f"releases/{release_id}")
    require(published["draft"] is False and published["tag_name"] == TAG, "published release identity mismatch")
    current = release_assets(api, release_id)
    for name, payload in assets.items():
        require(name in current, "published release lost an asset: " + name)
        verified_asset(api, current[name], payload)
    state["release"].update({"draft": False, "url": published["html_url"],
                            "published_at": published["published_at"]})
    state["passed"] = True
    state["recovery_workflow"] = {
        "run_id": os.environ["GITHUB_RUN_ID"], "run_attempt": os.environ["GITHUB_RUN_ATTEMPT"],
        "helper_commit": os.environ["GITHUB_SHA"], "helper_sha256": digest(Path(__file__).read_bytes()),
    }
    audit_name = f"ztty-{VERSION}-recovery-{os.environ['GITHUB_RUN_ID']}-{os.environ['GITHUB_RUN_ATTEMPT']}.json"
    flat_name(audit_name)
    audit_bytes = encoded(state)
    (out / "publication-audit.json").write_bytes(audit_bytes)
    audit_asset = ensure_asset(api, release_id, audit_name, audit_bytes)
    state["audit_asset"] = audit_asset
    write_json(out / "publication-result.json", state)
    return state


def compact_report(value):
    return {"source_sha": SOURCE, "tag": TAG, "original_run_id": RUN, "original_run_attempt": ATTEMPT,
            "passed": value.get("passed", False), "mode": value.get("mode", "audit"),
            "original_gates_passed": value.get("original_gates", {}).get("passed"),
            "artifact_audit_passed": value.get("artifact_audit", {}).get("passed"),
            "artifact_count": value.get("artifact_audit", {}).get("count"),
            "evidence_bundle": value.get("evidence_bundle"), "registry": value.get("registry"),
            "release": value.get("release"), "release_readback": value.get("release_readback"),
            "audit_asset": value.get("audit_asset"), "error": value.get("error")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("audit", "publish"))
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    source, out = arguments.source.resolve(), arguments.output.resolve()
    require(source != out and source not in out.parents, "evidence output must be outside the source checkout")
    out.mkdir(parents=True, exist_ok=True)
    result = {"passed": False, "mode": arguments.mode}
    try:
        api = GitHub()
        result = audit(api, source, out) if arguments.mode == "audit" else publish(api, source, out)
        result["mode"] = arguments.mode
    except Exception as error:
        progress = out / "publication-progress.json"
        if arguments.mode == "publish" and progress.exists():
            result = json.loads(progress.read_bytes())
        result.update({"passed": False, "error": type(error).__name__ + ": " + str(error)})
        raise
    finally:
        write_json(out / "execution-result.json", result)
        print("ZTTY_RELEASE_RECOVERY_AUDIT=" + json.dumps(compact_report(result), sort_keys=True, separators=(",", ":")))


if __name__ == "__main__":
    main()
