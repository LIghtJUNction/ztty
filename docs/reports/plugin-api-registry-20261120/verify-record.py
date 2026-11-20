#!/usr/bin/env python3
"""Verify saved publication/consumer evidence without network, Cargo or uploads."""
import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOURCE = "36b367ffda4cea9b3b3dfbf38dc6375567912323"
ARCHIVE = "e00adc5df1eefa237ea1ccd8d837ceecabef1976141b78918901d95cb8e9d4a6"
RESULT = "d4803771883b01f6c2cbcffede8db1937dabd9e3c8c3ed90b88df7161de6b4e7"
LOCK = "c4290a5f00aa4437ad31007dfcc0f35e582b4c4f62511ade176ff553fc34efa3"
VERSIONS = {"zinput": "0.1.3", "zgraphics": "0.2.0", "zpty": "0.2.2",
            "zgpui": "0.1.0", "ztty-plugin-api": "0.1.0", "zterm": "0.5.0"}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def read(name):
    return json.loads((HERE / name).read_text())


def main():
    manifest = read("redaction.json")
    entries = {entry["public_file"]: entry for entry in manifest["files"]}
    for name, entry in entries.items():
        assert digest((HERE / name).read_bytes()) == entry["public_sha256"], name
    preflight, published, verified = [read(name) for name in (
        "api-preflight.json", "api-publish.json", "api-verify.json")]
    for record in (preflight, published, verified):
        assert record["ok"] and record["source"] == SOURCE
        assert record["version"] == "0.1.0"
        assert record["inputs_before"] == record["inputs_after"]
        assert record["inputs_before"]["Cargo.lock"] == LOCK
        archive = record["archive"]
        assert archive["sha256"] == ARCHIVE and archive["bytes"] == 3482
        assert len(archive["members"]) == 7
        assert archive["vcs"] == {
            "git": {"sha1": SOURCE}, "path_in_vcs": "crates/ztty-plugin-api"}
        assert archive["zplugin"] == {
            "version": "=0.3.1", "features": ["macros"], "default-features": False}
        resources = record["resources"]
        assert int(resources["memory.swap.peak"]) == 0
        events = dict(line.split() for line in resources["memory.events"].splitlines())
        assert all(int(events[key]) == 0 for key in ("oom", "oom_kill", "oom_group_kill"))
    assert preflight["exit_code"] == published["exit_code"] == 0
    assert preflight["command"] == published["command"] + ["--dry-run"]
    assert published["uploaded"] is True
    for record in (preflight, published):
        assert len(record["registry_before"]) == 3
        assert all(endpoint["status"] == 404 for endpoint in record["registry_before"])
    assert len(verified["registry_before"]) == 3
    assert all(endpoint["status"] == 200 for endpoint in verified["registry_before"])
    assert verified["registry_before"][2]["sha256"] == verified["registry_checksum"] == ARCHIVE
    for record, log in ((preflight, "api-preflight.log"), (published, "api-publish.log")):
        assert record["log_sha256"] == entries[log]["original_sha256"]
    assert preflight["inputs_before"] == published["inputs_before"] == verified["inputs_before"]
    index = read("registry-index-version.json")
    assert (index["name"], index["vers"], index["cksum"]) == ("ztty-plugin-api", "0.1.0", ARCHIVE)
    assert not index["yanked"] and index["pubtime"] == "2026-11-20T01:22:36Z"
    assert len(index["deps"]) == 1
    assert index["deps"][0]["name"] == "zplugin" and index["deps"][0]["req"] == "=0.3.1"
    api = read("registry-api.json")["version"]
    assert (api["crate"], api["num"], api["checksum"], api["crate_size"]) == (
        "ztty-plugin-api", "0.1.0", ARCHIVE, 3482)
    assert not api["yanked"]
    result = read("consumers-result.json")
    closure = read("consumers-closure.json")
    assert closure["source"] == SOURCE and closure["ok"] and closure["exit_code"] == 0
    assert closure["lock_before"] == LOCK
    assert closure["report_sha256"] == entries["consumers-result.json"]["original_sha256"] == RESULT
    assert closure["log_sha256"] == entries["consumers-run.log"]["original_sha256"]
    assert closure["tool_sha256"] == "b1bae659a7f125309f8060e9cb3d8894a14665651770cbe3f5af18223925f51c"
    assert result["ok"] and result["versions"] == VERSIONS
    assert set(result["requested_packages"]) == set(VERSIONS)
    assert len(result["individual_checks"]) == closure["individual_checks"] == 6
    assert {tuple(check["selected"]) for check in result["individual_checks"]} == {
        (name,) for name in VERSIONS}
    assert closure["combined_check"] is True and result["combined_check"]["ok"]
    assert set(result["combined_check"]["selected"]) == set(VERSIONS)
    assert result["packages"] == closure["packages"]
    packages = {package["name"]: package for package in result["packages"]}
    assert set(packages) == set(VERSIONS)
    assert packages["ztty-plugin-api"]["vcs"]["git"]["sha1"] == SOURCE
    assert packages["ztty-plugin-api"]["cargo_lock_checksum"] == ARCHIVE
    command_count = 0
    for check in [*result["individual_checks"], result["combined_check"]]:
        assert check["ok"] and check["consumer_library_no_std"]
        assert check["consumer_dependency_kind"] == "exact crates.io versions; no path or git dependencies"
        assert [command["stage"] for command in check["commands"]] == [
            "metadata", "no_std_library_check", "consumer_run"]
        for command in check["commands"]:
            assert command["ok"] and command["exit_code"] == 0
            assert command["timeout_seconds"] == result["cargo_timeout_seconds"] == 90
            command_count += 1
        assert check["commands"][-1]["stdout"].strip() == "registry consumer passed"
        for package in check["packages"]:
            expected = packages[package["name"]]
            for key in ("version", "source", "vcs", "cargo_lock_checksum", "library_source_has_no_std"):
                assert package[key] == expected[key], (package["name"], key)
            assert package["version"] == VERSIONS[package["name"]]
            assert package["source"] == "registry+https://github.com/rust-lang/crates.io-index"
            assert package["library_source_has_no_std"]
    assert command_count == 21
    log = [json.loads(line) for line in (HERE / "consumers-run.log").read_text().splitlines()]
    assert len(log) == 7 and all(line["ok"] for line in log)
    assert [line["package"] for line in log[:-1]] == result["requested_packages"]
    assert log[-1]["requested_packages"] == result["requested_packages"]
    resources = closure["resources"]
    assert int(resources["memory.peak"]) == 322183168
    assert int(resources["memory.swap.peak"]) == 0
    events = dict(line.split() for line in resources["memory.events"].splitlines())
    assert all(int(events[key]) == 0 for key in ("oom", "oom_kill", "oom_group_kill"))
    launch = read("consumers-launch.json")["command"]
    assert all(flag in launch for flag in (
        "--property=MemoryMax=768M", "--property=MemorySwapMax=0",
        "--property=CPUQuota=100%", "--property=Nice=10"))
    history = manifest["releasing_history"]
    releasing = (HERE.parent.parent / "releasing.md").read_bytes()
    start = releasing.index(history["marker"].encode())
    assert digest(releasing[start:start + history["bytes"]]) == history["sha256"]
    print(json.dumps({"verified": True, "public_file_hashes": len(entries),
                      "api_published": True, "registry_consumers": 6,
                      "combined_consumer": True, "cargo_commands_recorded": command_count,
                      "historical_release_bytes_preserved": history["bytes"],
                      "network_builds_uploads_executed": False}, sort_keys=True))


if __name__ == "__main__":
    main()
