#!/usr/bin/env python3
"""Run the scoped Linux native acceptance gates and preserve their evidence."""
import base64
import contextlib
import ctypes
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]
OUT = Path(os.environ.get("ZTTY_ACCEPTANCE_OUTPUT", str(ROOT / "artifacts/native-ui-acceptance"))).resolve()
BINARY = ROOT / "target/release/ztty"
CARGO = ["cargo", "+" + os.environ.get("RUST_TOOLCHAIN", "1.100.0")]
GATES = {
    "fmt": (CARGO + ["fmt", "--all", "--", "--check"], 120),
    "clippy": (CARGO + ["clippy", "--locked", "--workspace", "--all-targets", "--features", "wayland", "--", "-D", "warnings"], 1800),
    "lib": (CARGO + ["test", "--locked", "--features", "wayland", "-p", "ztty", "--lib"], 300),
    "actions": (CARGO + ["test", "--locked", "--features", "wayland", "-p", "ztty", "--test", "actions"], 300),
    "gpu": (CARGO + ["test", "--locked", "--features", "wayland", "-p", "ztty", "--lib", "gpu::", "--", "--ignored", "--test-threads=1", "--show-output"], 300),
    "build": (CARGO + ["build", "--locked", "--release", "--features", "wayland"], 1800),
}
GPU_TESTS = {
    "gpu::tests::egl_composites_clipping_alpha_and_replaced_images_with_exact_pixels",
    "gpu::tests::font_reload_keeps_the_surface_and_image_cache_and_updates_ui_metrics",
    "gpu::tests::image_texture_coordinates_preserve_every_column_at_four_k_width",
    "gpu::tests::native_fonts_shape_joining_scripts_and_complete_emoji_clusters",
    "gpu::tests::shaped_terminal_cells_render_whole_clusters_and_reuse_the_bounded_atlas",
    "gpu::tests::shared_display_survives_interleaved_windows_resize_and_context_destruction",
}
GUI_KIND = {
    "native_ui": "native_ui", "input": "input", "repaint": "repaint",
    "native_ui_gpu": "native_ui", "repaint_gpu": "repaint",
    "text_gpu": "text", "retained_frame": "retained_frame",
}
GUI = {"native_ui": 24, "input": 14, "repaint": 11,
       "native_ui_gpu": 24, "repaint_gpu": 11, "text_gpu": 8, "retained_frame": 2}
for name, kind in GUI_KIND.items():
    renderer = "gpu" if name.endswith("_gpu") else "software"
    script = {"text": "text_gpu_gui", "retained_frame": "retained_frame_gui"}.get(kind, kind + "_gui")
    command = [sys.executable, "-B", "tests/" + script + ".py", "--binary", str(BINARY),
               "--renderer", renderer, "--output", str(OUT / (name + ".json"))]
    if kind in ("repaint", "text"):
        command += ["--screenshots", str(OUT / name)]
    elif kind in ("native_ui", "input"):
        command += ["--source-root", str(ROOT), "--screenshot", str(OUT / (name + ".png"))]
    GATES[name] = (command, 300)

GATES["wayland"] = (
    [sys.executable, "-B", "tests/wayland_gui.py", "--binary", str(BINARY),
     "--output", str(OUT / "wayland")], 300)
for option, variable in (("--sway", "ZTTY_TEST_SWAY"), ("--swaymsg", "ZTTY_TEST_SWAYMSG")):
    if os.environ.get(variable):
        GATES["wayland"][0].extend([option, os.environ[variable]])



def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(name, value):
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / name).write_text(json.dumps(value, sort_keys=True, indent=2) + "\n", encoding="utf-8")


def read_json(name):
    return json.loads((OUT / name).read_text(encoding="utf-8"))


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def snapshot():
    paths = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT).split(b"\0")
    return {os.fsdecode(path): digest(ROOT / os.fsdecode(path)) for path in paths if path}


def initialize():
    source = {"source_sha": git("rev-parse", "HEAD"), "source_tree": git("rev-parse", "HEAD^{tree}"),
              "source_ref": os.environ.get("GITHUB_REF"), "run_id": os.environ.get("GITHUB_RUN_ID"),
              "run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
              "runner_image": os.environ.get("ImageVersion"), "initialized": False}
    write_json("source.json", source)
    require(source["source_sha"] == os.environ["GITHUB_SHA"], "checkout differs from the triggering SHA")
    require(not git("status", "--porcelain", "--untracked-files=no"), "checkout has tracked changes")
    require(sys.flags.optimize == 0, "Python assertions must remain enabled")
    toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    require(toolchain == os.environ["RUST_TOOLCHAIN"] == "1.100.0", "repository toolchain mismatch")
    write_json("source-before.json", snapshot())
    source.update(initialized=True, rust_toolchain=toolchain)
    write_json("source.json", source)


def descendants():
    result, pending = {}, [os.getpid()]
    while pending:
        parent = pending.pop()
        try:
            children = Path(f"/proc/{parent}/task/{parent}/children").read_text().split()
        except FileNotFoundError:
            continue
        for child in children:
            pid = int(child)
            try:
                token = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
            except FileNotFoundError:
                continue
            if pid not in result:
                result[pid] = token
                pending.append(pid)
    return result


def cleanup_owned():
    before = descendants()
    for sig in (signal.SIGTERM, signal.SIGKILL):
        for pid, token in descendants().items():
            with contextlib.suppress(FileNotFoundError, ProcessLookupError):
                current = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
                if current == token:
                    os.kill(pid, sig)
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            for pid in descendants():
                with contextlib.suppress(ChildProcessError):
                    os.waitpid(pid, os.WNOHANG)
            if not descendants():
                return {"before": list(before), "after": []}
            time.sleep(0.02)
    return {"before": list(before), "after": list(descendants())}


def check_binary():
    expected = read_json("binary.json")["sha256"]
    actual = digest(BINARY)
    require(actual == expected == digest(OUT / "ztty"), "built or archived ELF changed")
    return actual


def validate_gui(name, gate):
    from isolated_xvfb import display_endpoints

    report = read_json(name + ".json")
    kind = GUI_KIND[name]
    renderer = "gpu" if name.endswith("_gpu") else "software"
    gate["reported_ok"] = report.get("ok")
    gate["counts"] = {"checks": len(report.get("checks", []))}
    gate["binary_sha256"] = report.get("binary_sha256")
    require(report.get("ok") is True, f"{name} report failed: {report.get('error')}")
    require(gate["counts"]["checks"] == GUI[name], f"{name} check count differs from {GUI[name]}")
    require(report.get("binary_sha256") == check_binary(), f"{name} used a different ELF")
    baseline = read_json("source-before.json")
    hashes = report.get("source_sha256")
    require(isinstance(hashes, dict) and hashes, f"{name} omitted source hashes")
    require(all(baseline.get(path) == value for path, value in hashes.items()), f"{name} source hashes differ")
    if kind in ("native_ui", "input"):
        require(report.get("source_commit") == read_json("source.json")["source_sha"], "harness commit differs")
        require(report.get("source_changes") == [], "harness observed source changes")
        remaining = report.get("cleanup", {}).get("owned_processes_remaining")
    else:
        remaining = report.get("remaining_owned_pids")
        if kind in ("repaint", "text"):
            cleanup = report.get("display_cleanup", {})
            require(all(cleanup.get(key) is False for key in ("pid_lock_exists", "socket_exists", "lease_exists")),
                    name + " display cleanup failed or was omitted")
    require(remaining == [], f"{name} left owned processes or omitted cleanup")
    records = list(report.get("renderers_before", [report.get("renderer_before")]))
    records += report.get("renderers_after", [report.get("renderer_after")]) if kind != "retained_frame" else []
    require(records and all(record and record.get("actual") == renderer for record in records),
            name + " did not use the requested renderer")
    ownership = report.get("xvfb" if kind in ("native_ui", "input") else "display_ownership", {})
    match = re.fullmatch(r":([0-9]+)", ownership.get("display", ""))
    require(match is not None, f"{name} omitted its owned display")
    number = int(match[1])
    endpoints = display_endpoints(number)
    clean = {
        "display": ownership["display"], "pid": ownership.get("pid"),
        "ownership_verified": ownership.get("pid_lock_verified") is True
        and ownership.get("filesystem_peer_pid_verified") is True and bool(ownership.get("owned_listeners")),
        "process_gone": not Path(f"/proc/{ownership.get('pid')}").exists(),
        "files_gone": not any(item["exists"] for item in endpoints["files"]),
        "listeners_gone": not endpoints["listeners"],
        "lease_gone": not Path(f"/tmp/ztty-xvfb-{number}.lease").exists(),
        "owned_processes_remaining": remaining,
    }
    gate["cleanup"] = clean
    require(100 <= number <= 30000 and all(clean[key] for key in
            ("ownership_verified", "process_gone", "files_gone", "listeners_gone", "lease_gone")),
            f"{name} did not clean its verified private display")
    if kind == "input":
        vectors, zoom = report.get("vectors", []), report.get("zoom", [])
        gate["counts"].update(vectors=len(vectors), zoom=len(zoom))
        require(len(vectors) == 198 and len(zoom) == 14, "input coverage count mismatch")
        require(all(item.get("ok") is True and item["actual_hex"] == item["expected_hex"] for item in vectors),
                "input vector failed")
        require(all(item.get("ok") is True for item in zoom), "font zoom vector failed")
        require(report.get("font_restored") is True and report.get("keyboard_mapping_restored") is True,
                "input font or keyboard mapping restoration failed")
        require(report.get("test_sha256") == baseline["tests/input_gui.py"], "input harness hash differs")
    if kind in ("repaint", "text"):
        pictures = list(report.get("screenshots", {}).values())
    elif kind == "retained_frame":
        pictures = [report.get("final_screenshot", {})]
    else:
        pictures = [report.get("screenshot", {})]
    if kind == "native_ui":
        pictures.append(report.get("demo_screenshot", {}))
    require(pictures and all("path" in picture and "sha256" in picture for picture in pictures),
            f"{name} omitted screenshots")
    for picture in pictures:
        path = Path(picture["path"]).resolve()
        require(path.is_relative_to(OUT) and digest(path) == picture["sha256"], "screenshot path/hash mismatch")


def validate_wayland(gate):
    report = read_json("wayland/validation.json")
    require(report.get("passed") is True, f"Wayland report failed: {report.get('error')}")
    checks = report.get("checks", [])
    require(len(checks) == 65 and all(row.get("passed") is True for row in checks),
            "Wayland coverage or check result changed")
    require(report.get("binary_sha256") == check_binary(), "Wayland used a different ELF")
    require(report.get("source") == read_json("source.json")["source_sha"],
            "Wayland source commit differs")
    baseline = read_json("source-before.json")
    hashes = report.get("test_source_sha256")
    require(isinstance(hashes, dict) and hashes
            and all(baseline.get(path) == value for path, value in hashes.items()),
            "Wayland harness hashes differ")
    cleanup = {key: report.get(key) for key in (
        "owned_brokers_remaining", "owned_children_remaining", "owned_helpers_remaining")}
    require(all(value == [] for value in cleanup.values()),
            "Wayland omitted cleanup or left owned processes")
    gate.update(binary_sha256=report["binary_sha256"], counts={"checks": len(checks)},
                cleanup=cleanup, compositor_version=report.get("compositor_version"),
                scope=report.get("scope"))


def run_gate(name):
    command, limit = GATES[name]
    gate = {"ok": False, "command": command, "timeout_seconds": limit, "command_exit_code": None,
            "exit_code": 1, "started_unix": time.time()}
    write_json(name + ".gate.json", gate)
    started = time.monotonic()
    try:
        require(read_json("source.json")["initialized"], "source identity was not initialized")
        require(ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0, "could not become an owned-child subreaper")
        if name in GUI or name == "wayland":
            gate["binary_sha256_before"] = check_binary()
        with (OUT / (name + ".log")).open("wb") as log:
            try:
                result = subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
                                        timeout=limit, start_new_session=True)
                gate["command_exit_code"] = result.returncode
            except subprocess.TimeoutExpired:
                gate.update(command_exit_code=124, timed_out=True)
        gate["wrapper_cleanup"] = cleanup_owned()
        require(gate["command_exit_code"] == 0, f"{name} exited {gate['command_exit_code']}")
        require(gate["wrapper_cleanup"] == {"before": [], "after": []}, "gate leaked owned children")
        if name in ("lib", "actions", "gpu"):
            log_text = (OUT / (name + ".log")).read_text(errors="replace")
            results = re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;",
                                 log_text)
            rows = re.findall(r"^test (\S+) \.\.\. (ok|ignored[^\n]*|FAILED)$", log_text, re.M)
            passed = {test for test, result in rows if result == "ok"}
            ignored = {test for test, result in rows if result.startswith("ignored")}
            require(len(rows) == len(passed) + len(ignored), name + " duplicate or failed test record")
            if name == "lib":
                # Every ordinary library test runs; only the named real-EGL
                # cases are deferred to the explicit GPU gate below.
                require(len(passed) >= 14 and ignored == GPU_TESTS, "library coverage or ignored set changed")
                expected = [(str(len(passed)), "0", str(len(GPU_TESTS)), "0", "0")]
            elif name == "gpu":
                require(passed == GPU_TESTS and not ignored, "real GPU tests did not all execute")
                filtered = len(read_json("lib.gate.json")["counts"]["passed_tests"])
                expected = [(str(len(GPU_TESTS)), "0", "0", "0", str(filtered))]
            else:
                require(len(passed) == 12 and not ignored, "action test coverage changed")
                expected = [("12", "0", "0", "0", "0")]
            gate["counts"] = {"passed_tests": sorted(passed), "ignored_tests": sorted(ignored),
                              "result_records": results}
            require(results == expected, name + " test count/result mismatch")
        if name == "build":
            with BINARY.open("rb") as stream:
                require(stream.read(4) == b"\x7fELF", "release output is not an ELF")
            shutil.copy2(BINARY, OUT / "ztty")
            write_json("binary.json", {"path": str(BINARY), "sha256": digest(BINARY),
                                      "bytes": BINARY.stat().st_size, "artifact": "ztty"})
            gate["binary_sha256"] = check_binary()
        if name in GUI:
            validate_gui(name, gate)
            gate["binary_sha256_after"] = check_binary()
        if name == "wayland":
            validate_wayland(gate)
            gate["binary_sha256_after"] = check_binary()
        gate.update(ok=True, exit_code=0)
    except Exception as error:
        gate["error"] = repr(error)
    finally:
        gate["seconds"] = round(time.monotonic() - started, 3)
        write_json(name + ".gate.json", gate)
        print("ZTTY_NATIVE_GATE_JSON=" + json.dumps({"gate": name, **gate}, separators=(",", ":")), flush=True)
        log_path = OUT / (name + ".log")
        if not gate["ok"] and log_path.exists():
            print("ZTTY_NATIVE_FAILURE_LOG_BEGIN=" + name, flush=True)
            print(log_path.read_text(errors="replace")[-14000:], flush=True)
            print("ZTTY_NATIVE_FAILURE_LOG_END=" + name, flush=True)
    return gate["exit_code"]


def finalize():
    summary = {"ok": False, "source_sha": os.environ.get("GITHUB_SHA"), "source_tree": None,
               "elf_sha256": None, "source_unchanged": False, "binary_unchanged": False, "errors": [], "gates": {}}
    for name in GATES:
        try:
            summary["gates"][name] = read_json(name + ".gate.json")
        except (OSError, ValueError) as error:
            summary["gates"][name] = {"ok": False, "exit_code": None, "error": repr(error)}
    for name in GUI:
        with contextlib.suppress(OSError, ValueError):
            report = read_json(name + ".json")
            summary["gates"][name]["report"] = {
                "ok": report.get("ok"), "checks": len(report.get("checks", [])),
                "vectors": len(report.get("vectors", [])), "zoom": len(report.get("zoom", [])),
                "binary_sha256": report.get("binary_sha256"),
                "cleanup": report.get("cleanup", {"owned_processes_remaining": report.get("remaining_owned_pids")}),
                "display_cleanup": report.get("display_cleanup"), "error": report.get("error")}
    outcomes = json.loads(os.environ.get("ACCEPTANCE_STEP_OUTCOMES", "{}"))
    summary["step_outcomes"] = {key: outcomes.get(key, {}).get("outcome") for key in
                                ("checkout", "identity", "python", "dependencies", *GATES)}
    try:
        source, baseline = read_json("source.json"), read_json("source-before.json")
        summary.update({key: source.get(key) for key in ("source_sha", "source_tree", "source_ref", "run_id", "run_attempt", "runner_image", "rust_toolchain")})
        current = snapshot()
        write_json("source-after.json", current)
        changed = sorted(path for path in baseline.keys() | current.keys() if baseline.get(path) != current.get(path))
        summary["changed_tracked_files"] = changed
        summary["tracked_file_count"] = len(baseline)
        summary["source_manifest_sha256"] = {"before": digest(OUT / "source-before.json"), "after": digest(OUT / "source-after.json")}
        summary["source_unchanged"] = source["initialized"] and not changed and not git("status", "--porcelain", "--untracked-files=no") and git("rev-parse", "HEAD") == source["source_sha"] == os.environ["GITHUB_SHA"] and git("rev-parse", "HEAD^{tree}") == source["source_tree"]
        salient = {"Cargo.toml", "Cargo.lock", "build.rs", "rust-toolchain.toml", "src/native_ui.rs",
                   "src/session.rs", "src/x11.rs", "tools/native_ui_acceptance.py", ".github/workflows/native-ui-acceptance.yml"}
        for name in GUI:
            with contextlib.suppress(OSError, ValueError):
                salient.update(read_json(name + ".json").get("source_sha256", {}))
        summary["source_hashes_sha256"] = {path: baseline[path] for path in sorted(salient) if path in baseline}
    except Exception as error:
        summary["errors"].append("source: " + repr(error))
    try:
        summary["elf_sha256"] = check_binary()
        summary["binary_unchanged"] = True
    except Exception as error:
        summary["errors"].append("ELF: " + repr(error))
    demo = None
    try:
        picture = read_json("native_ui.json")["demo_screenshot"]
        path = Path(picture["path"]).resolve()
        require(path.is_relative_to(OUT) and digest(path) == picture["sha256"], "demo screenshot hash/path mismatch")
        raw = path.read_bytes()
        require(raw.startswith(b"\x89PNG\r\n\x1a\n"), "demo screenshot is not PNG")
        demo = {"filename": path.name, "bytes": len(raw), "sha256": picture["sha256"],
                "git_blob_sha1": hashlib.sha1(b"blob " + str(len(raw)).encode() + b"\0" + raw).hexdigest()}
        summary["demo_png"] = demo.copy()
        demo["base64"] = base64.b64encode(raw).decode("ascii")
    except Exception as error:
        summary["errors"].append("demo: " + repr(error))
    summary["ok"] = not summary["errors"] and summary["source_unchanged"] and summary["binary_unchanged"] and all(
        gate.get("ok") is True and gate.get("exit_code") == 0 for gate in summary["gates"].values()) and all(
        outcome == "success" for outcome in summary["step_outcomes"].values())
    files = {str(path.relative_to(OUT)): {"bytes": path.stat().st_size, "sha256": digest(path)}
             for path in sorted(OUT.rglob("*")) if path.is_file() and path.name not in ("summary.json", "files.json", "SHA256SUMS")}
    write_json("files.json", files)
    summary["artifact_manifest_sha256"] = digest(OUT / "files.json")
    write_json("summary.json", summary)
    (OUT / "SHA256SUMS").write_text("".join(f"{digest(path)}  {path.relative_to(OUT)}\n"
        for path in sorted(OUT.rglob("*")) if path.is_file() and path.name != "SHA256SUMS"), encoding="ascii")
    print("ZTTY_NATIVE_ACCEPTANCE_SUMMARY_JSON=" + json.dumps(summary, separators=(",", ":")), flush=True)
    if demo is not None:
        print("ZTTY_NATIVE_DEMO_PNG_BASE64_JSON=" + json.dumps(demo, separators=(",", ":")), flush=True)
    return 0 if summary["ok"] else 1


if __name__ == "__main__":
    os.chdir(ROOT)
    OUT.mkdir(parents=True, exist_ok=True)
    mode = sys.argv[1]
    if mode == "init":
        initialize()
    elif mode == "run":
        raise SystemExit(run_gate(sys.argv[2]))
    elif mode == "finalize":
        raise SystemExit(finalize())
    else:
        raise SystemExit("expected init, run GATE, or finalize")
