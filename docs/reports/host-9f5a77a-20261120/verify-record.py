#!/usr/bin/env python3
"""Verify this evidence bundle offline. Never executes Cargo or product code."""
from __future__ import annotations

import gzip
import hashlib
import json
from pathlib import Path
import re
import tomllib

HERE = Path(__file__).resolve().parent
SOURCE = "9f5a77a1299093d0ba609bfafa2cec3b0ba931dd"
BEFORE = "f2db31d7ea35bf1a0554e76d69d2e44faafd0cab"
CORE = "a2074bac4c030cfd7c0b2514c852a89dcf54648f"
WAYLAND = "c7935e9e76476a9cd7d0186f23f5806e3e2e2fc556a494b0e3741a2c893b3807"
NO_DEFAULT = "8ac2b30e72c33b68688234a180bd25e48f0fa64f8b54fac492fd597568bdeb46"
PEER = "521fe34ee30e65aab7fb940939a623d94b8e7a86bb38743bb5caaac87ddb7f2d"
LOCK = "b86e3438a14ff8710a28b09bafa33ab1861f8cf116b722dc94be83133d4543b5"
PACKAGES = {
    "zgpui": ("0.1.0", "1d796d1fc63808dc099ed7b10c92e75835ae2814", "9943673fb607eade54661ff65243c4441a537f75188e361160a97344570bff90"),
    "zgraphics": ("0.2.0", "fe6357f98cf80fb6cc74739e45e8866ee17706ad", "cc775cfedeae7a0563bb11e51e25b32e9d4a72757fb2935221ab7bc6f2c88e54"),
    "zinput": ("0.1.3", "37617313dd7aaabcca9dd567344a76c4cbe03907", "d854f1abbf3a1b2fe2ce8ac2e9ac9ccf05452e6561cf8e8fc70f9f252c0914bd"),
    "zpty": ("0.2.2", "66919bc6dcb7757e0437fe955bfa809fb1282b68", "1d46215c9aaad115aa3637217dcd3d7752d4f511f05ca29729c83ae58c0dd955"),
    "zterm": ("0.5.1", CORE, "fc48bcaf9892a77d17ab235316448c3e235b0dccd00e5963b542586585c26c91"),
    "ztty-plugin-api": ("0.1.0", "36b367ffda4cea9b3b3dfbf38dc6375567912323", "e00adc5df1eefa237ea1ccd8d837ceecabef1976141b78918901d95cb8e9d4a6"),
}
STAGES = [
    ("xim-all", 37), ("input-software", 14), ("clipboard-transfer", 25),
    ("copy-paste", 13), ("osc52-gpu", 13), ("graphics-software", 8),
    ("graphics-gpu", 8), ("text-software", 8), ("text-gpu", 8),
    ("retained-frame-software", 2), ("repaint-software", 11), ("repaint-gpu", 11),
    ("native-ui-software", 24), ("native-ui-gpu", 24), ("workspaces", 17),
    ("shared-gui", 9), ("plugin-cli-current", 22), ("plugin-cli-legacy", 22),
    ("plugin-gui-current", 10), ("wayland-shared-gui", 65), ("wayland-cli", 14),
]
EGL_NAMES = ["gpu::tests::" + name for name in (
    "egl_composites_clipping_alpha_and_replaced_images_with_exact_pixels",
    "font_reload_keeps_the_surface_and_image_cache_and_updates_ui_metrics",
    "image_texture_coordinates_preserve_every_column_at_four_k_width",
    "native_fonts_shape_joining_scripts_and_complete_emoji_clusters",
    "shaped_terminal_cells_render_whole_clusters_and_reuse_the_bounded_atlas",
    "shared_display_survives_interleaved_windows_resize_and_context_destruction",
)]
CHECKS = []


def require(ok, label):
    if not ok:
        raise AssertionError(label)
    CHECKS.append(label)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def read_json(name):
    data = (HERE / name).read_bytes()
    return json.loads(gzip.decompress(data) if name.endswith(".gz") else data)


def result_counts(text):
    return [tuple(map(int, row)) for row in re.findall(
        r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;", text)]


def memory_events(text):
    return {key: int(value) for key, value in (line.split() for line in text.splitlines())}


def main():
    index = read_json("redaction.json")
    require(index["source"] == SOURCE, "index source")
    rows = {row["public_file"]: row for row in index["files"]}
    require(len(rows) == len(index["files"]) == 76, "76 distinct source attachments")
    all_rows = list(rows.values()) + index["derived_files"]
    expected_paths = {row["public_file"] for row in all_rows} | {"redaction.json"}
    actual_paths = {str(path.relative_to(HERE)) for path in HERE.rglob("*") if path.is_file()}
    require(actual_paths == expected_paths and len(actual_paths) == 80, "complete 80-file bundle")
    for row in all_rows:
        name = row["public_file"]
        require(not Path(name).is_absolute() and ".." not in Path(name).parts, "relative path: " + name)
        data = (HERE / name).read_bytes()
        require(sha(data) == row["public_sha256"] and len(data) == row["public_bytes"], "public hash: " + name)
        if row.get("storage") == "gzip":
            payload = gzip.decompress(data)
            require(sha(payload) == row["public_payload_sha256"] and len(payload) == row["public_payload_bytes"], "gzip payload: " + name)
    original = lambda name: rows[name]["original_sha256"]
    require(sha((HERE / "source/Cargo.lock").read_bytes()) == LOCK, "fixed host lock")

    gate = read_json("local/gate.json")
    require(gate["source"] == gate["source_after"] == SOURCE and gate["dirty_after"] == "", "local clean identity")
    require(gate["passed"] is True and gate["exit_code"] == 0 and gate["command"] == ["make", "check"], "full local gate")
    require(gate["features"] == ["wayland"] and gate["bare_metal_required"] is True, "local gate scope")
    require(gate["log_sha256"] == original("local/gate.log") and gate["cargo_lock_sha256"] == LOCK, "local source log links")
    counts = result_counts((HERE / "local/gate.log").read_text())
    require(tuple(map(sum, zip(*counts))) == (472, 0, 6), "472 passed plus six ignored")
    require(gate["wayland_elf"]["sha256"] == WAYLAND and gate["wayland_elf"]["bytes"] == 1237696, "Wayland ELF identity")
    require(gate["historical_outputs_preserved"] is True, "historical outputs preserved")
    require(gate["memory_peak_bytes"] == 805306368 and gate["swap_peak_bytes"] == 0, "local resource values")
    ge = memory_events(gate["memory_events"])
    require(ge["max"] == 3326 and ge["oom"] == ge["oom_kill"] == 0, "local cap and OOM records")

    egl = read_json("egl/original.json")
    verified = read_json("egl/verified.json")
    require(egl["source"] == SOURCE and egl["zterm"] == CORE, "EGL source")
    require(egl["passed"] is False and egl["status"] == "failed" and "AssertionError" in egl["error"], "original parser failure retained")
    require(len(egl["steps"]) == 2 and all(step["exit_code"] == 0 for step in egl["steps"]), "both actual Cargo commands passed")
    require(egl["script_sha256"] == original("egl/run.py") and egl["root_gate_receipt_sha256"] == original("local/gate.json"), "EGL original links")
    require(verified["passed"] is True and verified["source"] == verified["source_after"] == SOURCE, "supplement source")
    require(verified["original_receipt_sha256"] == original("egl/original.json") and verified["egl_log_sha256"] == original("egl/tests.log"), "supplement links to unchanged originals")
    require("no recompilation or test rerun" in verified["scope"], "supplement scope retained")
    require(verified["default_binary"]["sha256"] == NO_DEFAULT and verified["default_binary"]["bytes"] == 1144288, "no-default-features ELF")
    require(verified["wayland_binary"]["sha256"] == WAYLAND and verified["lock_sha256"] == LOCK, "EGL companion identity")
    log = (HERE / "egl/tests.log").read_text()
    require(result_counts(log) == [(6, 0, 0)] and verified["egl_tests"] == EGL_NAMES, "six EGL results")
    blocks = re.findall(r"(?m)^test (gpu::tests::\S+) \.\.\.(.*?)(?=^test gpu::tests::|^test result:|\Z)", log, re.S)
    require([name for name, _ in blocks] == EGL_NAMES and all(re.search(r"(?m)\bok\s*$", block.rstrip()) for _, block in blocks), "six complete multiline test blocks")
    require("OpenGL renderer: NVIDIA GeForce RTX 4060 Laptop GPU/PCIe/SSE2" in log, "observed EGL renderer")
    require(egl["memory_peak_bytes"] == 805306368 and egl["swap_peak_bytes"] == 0 and memory_events(egl["memory_events"])["max"] == 108, "original EGL cap record")

    gui = read_json("gui/acceptance.json")
    close = read_json("gui/final-closure.json")
    require(gui["passed"] is True and gui["status"] == "completed" and gui["final_identity_check"] is True, "GUI completed with final identity")
    require(gui["expected_source_sha"] == SOURCE and gui["expected_binary_sha256"] == WAYLAND and gui["expected_no_wayland_sha256"] == NO_DEFAULT and gui["expected_ime_peer_sha256"] == PEER, "GUI expected inputs")
    for stem in ("binary", "supporting", "source", "plugins", "harness"):
        require(gui[stem + "_before"] == gui[stem + "_after"], "unchanged GUI " + stem)
    require(gui["source_before"]["clean"] is True and gui["source_before"]["sha"] == SOURCE and len(gui["harness_before"]) == 82, "GUI source and 82-file harness")
    require(gui["binary_before"]["sha256"] == WAYLAND and gui["supporting_before"]["no_wayland_binary"]["sha256"] == NO_DEFAULT and gui["supporting_before"]["ime_peer"]["sha256"] == PEER, "actual GUI inputs")
    require(gui["plugins_before"]["old_plugin_zplugin"] == "0.3.0" and gui["plugins_before"]["new_plugin_zplugin"] == "0.3.1", "two plugin generations")
    require(len(gui["steps"]) == gui["planned_stages"] == 21, "21 actual stages")
    for number, ((name, count), step) in enumerate(zip(STAGES, gui["steps"]), 1):
        require(step["name"] == name and step["passed"] is True and step["execution"]["exit_code"] == 0, "stage passed: " + name)
        receipt = step["receipt"]
        require(receipt["ok"] is True and receipt["checks"] == count and receipt["missing_required_checks"] == [] and receipt["strict_check_results"] is True, "strict receipt: " + name)
        raw = read_json(f"gui/stages/{number:02d}-{name}.json")
        require(len(raw["checks"]) == count and raw.get("ok", raw.get("passed")) is True, "raw check count/status: " + name)
        for check in raw["checks"]:
            if isinstance(check, dict):
                require(all(check[key] is True for key in ("ok", "passed") if key in check and isinstance(check[key], bool)), "raw check flags: " + name)
        require(step["execution"]["cleanup"]["remaining"] == [] and step["execution"]["runtime_removed"] is True, "owned cleanup: " + name)
    require(sum(step["receipt"]["checks"] for step in gui["steps"]) == 365, "365 actual GUI checks")
    signals = gui["steps"][-1]["execution"]["cleanup"]["signals"]
    require(len(signals) == 4 and all(row["signal"] == "SIGTERM" for row in signals), "four owned CLI cleanup SIGTERMs retained")
    require(close["source"] == SOURCE and close["stages"] == 21 and close["checks_total"] == 365, "GUI closure counts")
    for flag in ("passed", "source_clean", "all_stage_exit_codes_zero", "all_stage_owned_remaining_empty", "all_stage_runtime_removed", "final_identity_check"):
        require(close[flag] is True, "GUI closure " + flag)
    for field, path in (("acceptance_sha256", "gui/acceptance.json"), ("unit_journal_sha256", "gui/unit-journal.jsonl"), ("launch_sha256", "gui/launch.json"), ("default_egl_verified_sha256", "egl/verified.json")):
        require(close[field] == original(path), "GUI closure original link: " + field)

    registry = read_json("registry/result.json")
    rc = read_json("registry/closure.json")
    helper_error = read_json("registry/closure-helper-error.json")
    require(registry["source"] == SOURCE and registry["ok"] is True and len(registry["individual_checks"]) == 1, "changed registry graph 1+1")
    require(registry["before"] == registry["after"] and registry["before"]["status"] == "" and registry["before"]["lock_sha256"] == LOCK, "registry source and lock unchanged")
    checks = registry["individual_checks"] + [registry["combined_check"]]
    require(checks[0]["selected"] == ["zterm"] and set(checks[1]["selected"]) == set(PACKAGES), "registry selected crates")
    for name, check in zip(("zterm", "combined"), checks):
        require(check["ok"] is True and check["consumer_library_no_std"] is True, "registry consumer passed: " + name)
        require([row["stage"] for row in check["commands"]] == ["metadata", "no_std_library_check", "consumer_run"] and all(row["exit_code"] == 0 and row["ok"] is True for row in check["commands"]), "three actual Cargo operations: " + name)
        for package in check["packages"]:
            version, vcs, checksum = PACKAGES[package["name"]]
            require((package["version"], package["vcs"]["git"]["sha1"], package["cargo_lock_checksum"]) == (version, vcs, checksum), "registry package identity: " + package["name"])
            require(package["source"] == "registry+https://github.com/rust-lang/crates.io-index" and package["library_source_has_no_std"] is True, "registry source: " + package["name"])
        manifest = tomllib.loads((HERE / f"registry/consumers/{name}/Cargo.toml").read_text())
        require("patch" not in manifest and "replace" not in manifest, "no consumer source overrides: " + name)
        for dep, specification in manifest["dependencies"].items():
            require(dep in PACKAGES, "known direct consumer dependency")
            version = specification if isinstance(specification, str) else specification["version"]
            require(version == "=" + PACKAGES[dep][0] and (isinstance(specification, str) or not ({"path", "git"} & specification.keys())), "exact registry dependency: " + dep)
    require(len(registry["raw_commands"]) == 6 and all(row["exit_code"] == 0 for row in registry["raw_commands"]), "six Cargo commands without rerun")
    require(len(registry["consumer_files"]) == 8, "eight exact consumer source files")
    for path, digest in registry["consumer_files"].items():
        require(original("registry/" + path) == digest, "consumer original hash: " + path)
    require(registry["historical_full_six_individual_plus_combined"]["repeated_in_this_run"] is False, "old complete graph kept separate")
    require(helper_error["test_results_unchanged"] is True and rc["closure_helper_error"] == helper_error, "read-only closure helper failure retained")
    require(rc["source"] == SOURCE and rc["ok"] is True and rc["owned_remaining_pids"] == [] and rc["owned_cgroup_present_after_exit"] is False and rc["stop_exit"] == 0, "registry cleanup complete")
    require(rc["result_sha256"] == original("registry/result.json") and rc["source_clean_after"] is True and rc["historical_full_graph_preserved"] is True, "registry closure provenance")
    resources = registry["resources_after"]["files"]
    require(resources["memory.peak"] == "358707200" and resources["memory.swap.peak"] == "0" and memory_events(resources["memory.events"])["oom"] == 0, "registry resources")

    audit = read_json("size/audit.json.gz")
    invocation = read_json("size/invocation.json")
    require(audit["complete"] is True and audit["requested"]["before"] == BEFORE and audit["requested"]["after"] == SOURCE, "fixed size comparison")
    expected = ((79, 40436, 1432454, 3, 1345, 44946), (89, 30886, 1094059, 39, 14306, 491136))
    for side, values in zip(("before", "after"), expected):
        files = audit[side]["files"]
        for owner, total_key, offset in (("root_repository", "root_repository", 0), ("gitlink_library", "gitlink_libraries", 3)):
            selected = [row for row in files if row["owner"] == owner]
            actual = (len(selected), sum(row["lines"] for row in selected), sum(row["bytes"] for row in selected))
            require(actual == values[offset:offset + 3], "size records: " + side + "/" + owner)
            require(audit[side]["totals"][total_key] == dict(zip(("files", "lines", "bytes"), actual)), "size subtotal: " + side + "/" + owner)
        require(audit[side]["totals"]["combined"] == {"files": len(files), "lines": sum(row["lines"] for row in files), "bytes": sum(row["bytes"] for row in files)}, "expanded size total: " + side)
    require(len(audit["same_blob_owner_changes"]) == 19, "19 unchanged-blob ownership transfers")
    require(audit["script"]["sha256"] == invocation["script_sha256"] == original("size/audit.py"), "same size script")
    require(invocation["exit_code"] == 0 and invocation["previous_artifacts_preserved"] is True and invocation["root_head_after"] == SOURCE and invocation["root_status_after"] == "", "size audit closure")
    summary = read_json("summary.json")
    require(summary["source"] == SOURCE and summary["zterm_source"] == CORE and summary["rust"] == {"passed": 472, "failed": 0, "initially_ignored": 6, "egl_executed_passed": 6}, "summary identity and Rust totals")
    require(summary["gui"] == {"stages_passed": 21, "checks_passed": 365} and summary["registry"] == {"standalone": 1, "combined": 1, "cargo_commands": 6}, "summary actual scope")
    require(summary["source_size"] == {"before": audit["before"]["totals"], "after": audit["after"]["totals"]}, "summary size values")
    print(json.dumps({"ok": True, "source": SOURCE, "files": 80, "assertions": len(CHECKS), "rust_passed": 472, "egl_passed": 6, "gui_stages": 21, "gui_checks": 365, "registry_graphs": "1+1", "cargo_commands": 6, "product_commands_executed": 0}, indent=2))


if __name__ == "__main__":
    main()
