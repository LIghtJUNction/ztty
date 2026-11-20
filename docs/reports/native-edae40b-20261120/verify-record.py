#!/usr/bin/env python3
"""Verify the saved native acceptance record without running product code."""
import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOURCE = "edae40b03be0207fd062e13bc4304c42bf0d9fc0"
RUN = 44656228400
LINUX_COUNTS = {
    "workspaces/report.json": 17, "daily_workflow/report.json": 19,
    "session_switcher/report.json": 6, "sixel_gui/report.json": 10,
    "input_gui/report.json": 14, "repaint_gui/report.json": 11,
    "native_ui_gui/report.json": 24, "gpu-text.json": 8,
    "gpu-ui.json": 24, "gpu-repaint.json": 11, "retained-frame.json": 2,
    "graphics-gpu/graphics-extensions.json": 8,
    "graphics-software/graphics-extensions.json": 8,
    "osc52-gpu/osc52.json": 13, "osc52-software/osc52.json": 13,
    "clipboard.json": 25, "wayland/validation.json": 65,
}


def read(name):
    return json.loads((HERE / name).read_text())


def passed(report):
    flag = report.get("ok")
    if not isinstance(flag, bool):
        flag = report.get("passed")
    assert flag is True
    checks = report.get("checks", [])
    if isinstance(checks, dict):
        assert all(value is True for value in checks.values())
    else:
        for item in checks:
            if isinstance(item, dict):
                for key in ("ok", "passed"):
                    if key in item:
                        assert item[key] is True


def main():
    index = read("evidence-index.json")
    for entry in index["public_files"]:
        data = (HERE / entry["file"]).read_bytes()
        assert len(data) == entry["bytes"], entry["file"]
        assert hashlib.sha256(data).hexdigest() == entry["sha256"], entry["file"]
    run = read("run.json")
    assert run["id"] == RUN and run["head_sha"] == SOURCE
    assert run["run_attempt"] == 1 and run["conclusion"] == "success"
    jobs = read("jobs.json")
    assert len(jobs) == 9
    assert sum(job["conclusion"] == "success" for job in jobs) == 7
    assert {job["name"] for job in jobs if job["conclusion"] == "skipped"} == {"verify", "publish"}
    dispatch = read("dispatch-summary.json")
    assert dispatch["source"] == SOURCE and dispatch["run_id"] == RUN
    assert dispatch["build_only"] is True and dispatch["candidate_platform"] == "all"
    assert not dispatch["windows_paint_diagnostics"] and not dispatch["windows_nc_rendering_diagnostics"]
    artifacts = read("artifacts.json")
    assert len(artifacts) == 6
    for artifact in artifacts:
        assert artifact["workflow_run"]["head_sha"] == SOURCE
        assert artifact["workflow_run"]["id"] == RUN
        archive = next(x for x in index["archived_artifacts"] if x["artifact_id"] == artifact["id"])
        assert archive["sha256"] == artifact["digest"].split(":", 1)[1]
        assert archive["bytes"] == artifact["size_in_bytes"]
    log_receipt = read("log-checksums.json")
    assert log_receipt["all_plugin_exact"] and len(log_receipt["logs"]) == 7
    assert all(row["plugin_exact_match"] for row in log_receipt["logs"])
    acceptance = read("acceptance.json")
    assert acceptance["source"] == SOURCE and acceptance["run_id"] == RUN
    assert acceptance["application_published"] is False
    assert acceptance["zterm_version"] == "0.5.0"
    signing = read("audit/identity-and-signing.json")
    for arch in ("x86_64", "aarch64"):
        for platform in ("linux", "windows", "macos"):
            stem = f"ztty-{platform}-{arch}"
            native = read(f"reports/{stem}/{stem}.native.json")
            assert native["source_sha"] == SOURCE and native["passed"] is True
        stem = f"ztty-linux-{arch}"
        native = read(f"reports/{stem}/{stem}.native.json")
        for snapshot in ("source_before", "source_after"):
            assert native[snapshot] == {"commit": SOURCE, "tracked_changes": []}
        assert len(native["cases"]) == 7 and all(c["passed"] for c in native["cases"])
        assert all(c["cleanup"]["remaining_owned_pids"] == [] for c in native["cases"])
        for path, count in LINUX_COUNTS.items():
            record = read(f"reports/{stem}/native/{path}")
            passed(record)
            assert len(record["checks"]) == count, (arch, path)
            if record.get("binary_sha256"):
                assert record["binary_sha256"] == native["binary_sha256"], (arch, path)
        input_record = read(f"reports/{stem}/native/input_gui/report.json")
        assert len(input_record["vectors"]) == 198 and all(v["ok"] for v in input_record["vectors"])
        assert input_record["cleanup"]["owned_processes_remaining"] == []
        assert input_record["keyboard_layout"]["keypad_type"] == "ZTTY_TEST_PC_KEYPAD"
        assert input_record["keyboard_layout"]["fixture_sha256"] == "e9f664e17a5e4886f078f0bbf35ce80f4c3878ee17256c964cc13a593d0dc113"
        daily = read(f"reports/{stem}/native/daily_workflow/report.json")["command_reflow"]
        assert daily["before"] == daily["after"]
        assert [row["bytes"] for row in daily["retained_outputs"]] == [20, 2872]
        wayland = read(f"reports/{stem}/native/wayland/validation.json")
        assert wayland["source"] == SOURCE
        assert sum(row["name"].startswith("ime_") for row in wayland["checks"]) == 7
        names = {row["name"] for row in wayland["checks"]}
        assert {"x11_first_routes_both_backends_in_one_broker",
                "x11_first_keeps_separate_ptys", "x11_first_mixed_sessions_closed",
                "wayland_first_mixed_sessions_closed",
                "mixed_backend_tests_preserve_original_session"} <= names
        assert all(wayland[key] == [] for key in (
            "owned_brokers_remaining", "owned_children_remaining", "owned_helpers_remaining"))
        gate_text = (HERE / f"reports/{stem}/{stem}.gate-tests.log").read_text()
        assert "Ran 17 tests" in gate_text and "test_external_sigterm_unwinds_cleanup_before_driver_exits" in gate_text
        assert "4 passed; 0 failed" in (HERE / f"reports/{stem}/{stem}.working-directory.log").read_text()
        assert "6 passed; 0 failed" in (HERE / f"reports/{stem}/native/gpu-tests.log").read_text()
        stem = f"ztty-windows-{arch}"
        assert "Ran 41 tests" in (HERE / f"reports/{stem}/{stem}.native.contract.log").read_text()
        native = read(f"reports/{stem}/{stem}.native.json")
        passed(native)
        assert len(native["checks"]) == 31
        assert not native["paint_diagnostics_enabled"] and not native["nc_rendering_diagnostics_enabled"]
        cold = native["cold_start_runs"]
        assert cold["passed"] and cold["required"] == cold["completed"] == 8
        assert cold["source_sha"] == SOURCE and cold["binary_sha256"] == native["binary_sha256"]
        workspace = read(f"reports/{stem}/{stem}.native.workspace.json")
        passed(workspace)
        assert len(workspace["checks"]) == 22
        assert workspace["binary_sha256"] == native["binary_sha256"]
        assert workspace["cleanup"]["all_probe_processes_exited"]
        maximize = next(x for x in workspace["checks"] if x["name"] == "maximize_restore_keeps_pty_identity")
        assert all(x["owned"] and x["visible"] and x["rgb"] == x["expected"] for x in maximize["pixels"])
        desktop = read(f"reports/{stem}/{stem}.native.desktop.json")
        assert desktop["passed"] and desktop["cleanup"]["job_pids_after"] == []
        assert all(desktop["cleanup"][key] for key in (
            "input_restored", "thread_restored", "owned_desktop_closed", "handles_closed"))
        stem = f"ztty-macos-{arch}"
        native = read(f"reports/{stem}/{stem}.native.json")
        passed(native)
        assert len(native["checks"]) == 24 and native["remaining_owned_processes"] == []
        assert native["ime_keyboard"] == "not_performed"
        assert native["pixel_readback"] == "WindowServer window via ScreenCaptureKit"
        assert "Ran 10 tests" in (HERE / f"reports/{stem}/{stem}.native.contract.log").read_text()
        spawn = read(f"reports/{stem}/{stem}.spawn.json")
        assert spawn["passed"] and spawn["test"]["passed"] and len(spawn["test"]["checks"]) == 8
        evidence = next(x for x in signing["macos"] if x["architecture"] == arch)
        assert evidence["signature"] == "ad-hoc-verified" and evidence["notarized"] is False
        assert len(evidence["packages"]) == 2
        for package in evidence["packages"]:
            assert package["tested_source_binary_sha256"] == native["binary_sha256"]
            assert package["files_verified"] == 5
        assert evidence["packages"][0]["packaged_binary_sha256"] == evidence["packages"][1]["packaged_binary_sha256"]
    assert sum(LINUX_COUNTS.values()) == 278
    print(json.dumps({"verified": True, "source": SOURCE, "run_id": RUN,
                      "native_platforms": 6, "public_file_hashes": len(index["public_files"]),
                      "product_tests_builds_dispatches_executed": False}, sort_keys=True))


if __name__ == "__main__":
    main()
