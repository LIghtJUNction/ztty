#!/usr/bin/env python3
"""Verify the archived release records offline; no download or product command."""
from __future__ import annotations
import gzip
import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOURCE = "c4f8f48c9598284a589e609f25242b49ceabfea8"
RUN = 44668398577
RELEASE = 452238617
TAG_OBJECT = "4288d646b3273285483cf1fdad0ccc8e0c42b9ae"
CHECKS = []


def require(value, label):
    if not value:
        raise AssertionError(label)
    CHECKS.append(label)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def read(name):
    path = HERE / name
    if not path.exists():
        path = HERE / (name + ".gz")
    data = path.read_bytes()
    return json.loads(gzip.decompress(data) if path.suffix == ".gz" else data)


def assets_table():
    assets = read("audit/public-asset-index.json")["assets"]
    lines = ["# 47 个已发布资产", "", "本表从完成实际下载校验的索引生成；字节与 SHA-256 不作改写。", "",
             "| 资产 | 字节数 | SHA-256 |", "| --- | ---: | --- |"]
    for name, item in sorted(assets.items()):
        lines.append(f'| [{name}]({item["url"]}) | {item["bytes"]} | `{item["sha256"]}` |')
    lines += ["", "SHA256SUMS 只覆盖源码 tarball；其他文件按对应 sidecar 与 GitHub asset digest 分别核验。", ""]
    return "\n".join(lines)


def make_summary():
    audit = read("audit/verification.json")
    return {"schema": 1, "source": SOURCE, "tag": "v0.5.0", "run_id": RUN,
            "release_id": RELEASE, "published_at": audit["release_published_at"],
            "assets": audit["public_assets"], "asset_bytes": audit["public_asset_bytes"],
            "native_platforms": read("audit/native-platforms.json"),
            "source_archive": audit["source_archive"], "evidence_zip": audit["evidence_zip"],
            "aur": audit["aur"], "installer": {
                "source": SOURCE, "release_tag": "v0.5.0", "success": True,
                "scope": "private-prefix Linux x86_64 installer, version/help; user installation unchanged"}}


def main():
    index = read("redaction.json")
    rows = {row["public_file"]: row for row in index["files"]}
    require(index["source"] == SOURCE and len(rows) == len(index["files"]) == 64, "64 distinct original attachments")
    all_rows = list(rows.values()) + index["derived_files"]
    expected = {row["public_file"] for row in all_rows} | {"redaction.json"}
    require({str(p.relative_to(HERE)) for p in HERE.rglob("*") if p.is_file()} == expected, "complete scoped bundle")
    for row in all_rows:
        name = row["public_file"]
        require(not Path(name).is_absolute() and ".." not in Path(name).parts, "relative file: " + name)
        data = (HERE / name).read_bytes()
        require(sha(data) == row["public_sha256"] and len(data) == row["public_bytes"], "public file: " + name)
        if row.get("storage") == "gzip":
            payload = gzip.decompress(data)
            require(sha(payload) == row["public_payload_sha256"] and len(payload) == row["public_payload_bytes"], "lossless JSON: " + name)

    def original(name):
        return rows[name if name in rows else name + ".gz"]["original_sha256"]

    require(original("audit/verification.json") == "6bf85fefec7a104278eeb7f0b580db4fb4b19d9a8c7bee278f46c51b6ef40f7b", "fixed final audit receipt")
    audit = read("audit/verification.json")
    require(audit["ok"] is True and audit["source_sha"] == audit["tag_commit"] == SOURCE, "formal source identity")
    require(audit["run_id"] == RUN and audit["attempt"] == 1 and audit["event"] == "workflow_dispatch" and audit["conclusion"] == "success", "completed actual release run")
    require(audit["annotated_tag"] == TAG_OBJECT and audit["tag_signature"] == "unsigned", "annotated tag scope")
    require(len(audit["job_conclusions"]) == 9 and all(x["conclusion"] == "success" for x in audit["job_conclusions"]), "all nine jobs")
    require(audit["verification_assertions"] == 13751, "audit assertion count retained, not product-test count")
    for name, item in audit["outputs"].items():
        require(original("audit/" + name) == item["sha256"], "audit output link: " + name)
    require(original("native/macos-cross-review.json") == audit["macos_review"]["sha256"], "independent Mac review link")

    release = read("release/original-api.json")
    require(release["id"] == RELEASE and release["tag_name"] == "v0.5.0" and release["draft"] is False and release["prerelease"] is False, "actual public release")
    require(release["published_at"] == audit["release_published_at"] == "2026-11-20T03:41:44Z", "publication timestamp")
    assets = read("audit/public-asset-index.json")
    official = {row["name"]: row for row in release["assets"]}
    require(len(official) == len(assets["assets"]) == audit["public_assets"] == 47 and set(official) == set(assets["assets"]), "complete 47-asset set")
    for name, item in assets["assets"].items():
        api = official[name]
        require(item["http_status"] == 200 and item["bytes"] == api["size"] and item["sha256"] == api["digest"].removeprefix("sha256:"), "actual download/API identity: " + name)
        require(item["id"] == api["id"] and item["url"] == api["browser_download_url"], "asset URL/id: " + name)
    require(sum(x["bytes"] for x in assets["assets"].values()) == audit["public_asset_bytes"] == 28446629, "all asset bytes")
    require(assets["checksum_files"]["SHA256SUMS"] == ["ztty-0.5.0.tar.gz"], "source-only SHA256SUMS boundary")
    for sidecar, targets in assets["checksum_files"].items():
        require(sidecar in official and targets and all(t in official for t in targets), "checksum indexed target: " + sidecar)

    source = read("audit/source-archive-verification.json")
    require(source == audit["source_archive"] and source["files"] == 1700 and len(source["gitlinks"]) == 5, "recursive source archive count")
    require(source["all_blobs_match"] is True and source["exact_tracked_file_set"] is True, "source exact tracked blobs")
    require(source["archive"]["sha256"] == assets["assets"]["ztty-0.5.0.tar.gz"]["sha256"], "source asset linkage")
    evidence = read("audit/native-evidence-verification.json")
    require(evidence == audit["evidence_zip"] and evidence["files"] == 949 and evidence["zip_entries"] == 950 and len(evidence["empty_files"]) == 10, "top-level evidence scope")
    require(evidence["all_original_bytes_equal"] is True and evidence["sha256"] == assets["assets"]["ztty-0.5.0-native-evidence.zip"]["sha256"], "evidence archive identity")
    workflows = read("audit/workflow-artifact-index.json")
    platforms = read("audit/native-platforms.json")
    require(len(workflows) == len(platforms) == audit["workflow_artifacts"] == 6 and set(workflows) == set(platforms), "six distinct platform artifacts")

    for stem, scope in platforms.items():
        base = "native/" + stem + "/" + stem
        build, native = read(base + ".build.json"), read(base + ".native.json")
        require(build["source_sha"] == native["source_sha"] == SOURCE and int(build["run_id"]) == RUN and int(build["run_attempt"]) == 1, "platform source/run: " + stem)
        require(build["version"] == "0.5.0" and build["build_only"] is False and build["candidate_platform"] == "all", "formal gate inputs: " + stem)
        require(build["passed"] is True and all(v == "success" for v in build["outcomes"].values()) and native["passed"] is True, "platform outcomes: " + stem)
        require(scope["remaining_owned"] == [] and native["binary_sha256"] == scope["tested_binary_sha256"], "tested binary/cleanup: " + stem)
        for kind in ("build", "native"):
            name = stem + "." + kind + ".json"
            require(original("native/" + stem + "/" + name) == scope[kind]["sha256"] == assets["assets"][name]["sha256"], "native/public report identity: " + name)

        if "linux" in stem:
            require(native["source_before"] == native["source_after"] and native["binary_sha256"] == native["binary_sha256_after"], "Linux fixed runtime inputs")
            require([(c["name"], c["checks_completed"]) for c in native["cases"]] == [(c["name"], c["checks"]) for c in scope["base_cases"]], "Linux base counts from originals")
            for case in native["cases"]:
                require(case["passed"] is True and case["exit_code"] == 0 and case["cleanup"]["remaining_owned_pids"] == [], "Linux base passed/clean: " + case["name"])
            count = sum(c["checks_completed"] for c in native["cases"])
            require(count == 101, "Linux 101 base checks")
            for record in scope["extra_reports"]:
                path = "native/" + stem + "/extra/" + record["report"]
                raw = read(path)
                require(raw[record["flag"]] is True and len(raw["checks"]) == record["checks"], "Linux supplementary actual checks: " + record["report"])
                require(original(path) == record["identity"]["sha256"], "Linux supplementary original hash")
                count += len(raw["checks"])
            require(count == scope["gui_checks"] == 278, "278 Linux GUI checks")
            require((scope["egl_tests"], scope["driver_contract_tests"], scope["working_directory_tests"], scope["input_vectors"]) == (6, 17, 4, 198), "separate Linux gates retained")
            require(scope["nested_evidence_files"] == 146 and scope["nested_png_files"] == 75, "Linux nested artifact scope")
        elif "windows" in stem:
            require(len(native["checks"]) == 31 and all(c["passed"] is True for c in native["checks"]), "31 Windows contracts")
            cold = native["cold_start_runs"]
            require(cold["required"] == cold["completed"] == len(cold["runs"]) == 8 and cold["passed"] is True, "eight cold starts")
            require(all(r["passed"] is True and r["checks_completed"] == 31 and r["source_sha"] == SOURCE and r["binary_sha256"] == native["binary_sha256"] for r in cold["runs"]), "same contracts passed in every cold start")
            workspace = native["workspace"]
            require(workspace["passed"] is True and len(workspace["checks"]) == 22 and all(c["passed"] is True for c in workspace["checks"]), "22 workspace checks")
            require(any(c["name"] == "maximize_restore_keeps_pty_identity" and c["passed"] is True for c in workspace["checks"]), "maximize preserves PTY identity")
            require(native["contract_tests"]["passed"] is True and any("Ran 41 tests" in line for line in scope["contract_test_summary"]), "41 fixture contract tests")
            manifest = read(base + ".manifest.json")
            require(manifest == scope["package_manifest"] and manifest["files"]["ztty.exe"]["sha256"] == native["binary_sha256"], "Windows packaged tested binary")
        else:
            require(len(native["checks"]) == 24 and all(native["checks"].values()), "24 macOS checks")
            require(len(native["stages"]) == 13 and all(s["passed"] is True and s["exit_code"] == 0 for s in native["stages"]), "13 macOS stages")
            require(native["remaining_owned_processes"] == [] and native["ime_keyboard"] == "not_performed", "macOS cleanup and IME boundary")
            manifest = read(base + ".manifest.json")
            require(manifest["signature"] == "ad-hoc-verified" and manifest["notarized"] is False, "macOS signature scope")
            review = next(r for r in read("native/macos-cross-review.json")["results"] if r["architecture"] == scope["architecture"])
            require(review["pre_sign_native_binary_sha256"] == native["binary_sha256"] == review["bundle_build"]["source_binary_sha256"], "macOS pre-sign to bundle BUILD")
            require(review["signed_binary"] == manifest["files"]["ztty.app/Contents/MacOS/ztty"], "macOS signed manifest identity")
        if "linux" not in stem:
            name = stem + ".manifest.json"
            require(original("native/" + stem + "/" + name) == assets["assets"][name]["sha256"], "public package manifest")

    require(audit["aur"]["pushed"] is True and audit["aur"]["commit_short"] == "70b24f3" and "HEAD -> master" in audit["aur"]["log_lines"][1], "actual AUR push")
    installer = read("installer/verification.json")
    require(original("installer/verification.json") == "92bb4bcdbff7c40c3fdaad217ed0c678ac7e019e67ce44a56eb5f0920f64d0c7", "fixed isolated installer receipt")
    require(installer["success"] is True and installer["source_sha"] == SOURCE and installer["release_id"] == RELEASE and installer["release_tag"] == "v0.5.0", "actual installer source")
    require(all(installer["checks"].values()) and len(installer["checks"]) == 6 and all(c["exit_code"] == 0 for c in installer["commands"]), "six installer smoke assertions")
    require(installer["guard_before"] == installer["guard_after"] and len(installer["guard_before"]) == 6 and installer["user_installation_and_default_files_unchanged"] is True, "six user paths unchanged")
    require(installer["private_prefix_removed"] is True and installer["installed_binary"]["sha256"] == platforms["ztty-linux-x86_64"]["tested_binary_sha256"], "private install matches formal native ELF")
    require(installer["installer_sha256"] == original("installer/install.sh") == assets["assets"]["install.sh"]["sha256"], "published installer script identity")
    for command in installer["commands"]:
        for stream in ("stdout", "stderr"):
            require(original("installer/" + command["label"] + "." + stream + ".log") == command[stream + "_sha256"], "installer raw log")
    require(original("installer/installed-desktop.txt") == installer["desktop"]["sha256"], "actual private desktop file")

    old = (HERE / "release/original-body.md").read_bytes()
    new = (HERE / "release/published-body.md").read_bytes()
    notes = read("release/release-notes-edit.json")
    failure = read("release/release-notes-first-readback-failure.json")
    require(old == release["body"].encode() and len(old) == 458 and new.count(old) == 1, "original generated notes retained exactly once")
    require(sha(old) == notes["original_body_sha256"] and sha(new) == notes["published_body_sha256"] == "a2ef81f3444c822cb75789df8b72209e08fcb71d1e53d10cce99e9f58b3cf252", "published notes exact hashes")
    require(notes["success"] is True and notes["assets_unchanged"] is True and notes["mutation_repeated"] is False, "one notes update and verified readback")
    require(failure["wrapper_exit_code"] == 1 and failure["edit_step_exit_code"] == 0 and failure["mutation_repeated"] is False, "network readback failure preserved without mislabelling release")
    require(read("summary.json") == make_summary(), "summary derived from originals")
    require((HERE / "ASSETS.md").read_text() == assets_table(), "all asset rows match raw")
    print(json.dumps({"ok": True, "source": SOURCE, "run_id": RUN, "files": len(expected),
                      "offline_assertions": len(CHECKS), "original_audit_assertions": 13751,
                      "platforms": 6, "assets": 47, "original_evidence_files": 949,
                      "product_commands_executed": 0, "downloads_performed": 0}, indent=2))


if __name__ == "__main__":
    main()
