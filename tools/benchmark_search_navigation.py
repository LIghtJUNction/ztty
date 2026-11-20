#!/usr/bin/env python3
"""Compare uncached search with bounded navigation using identical dependencies.

Run this inside the repository's shared Cargo owner-lock batch. The runner does
not acquire that lock again and does not override Cargo's target directory.
Feed, correctness checks and cache preparation for warm navigation are outside
the timed interval. This measures logical search, not visible input latency.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import tempfile
import time
import tomllib


def command(argv, root):
    return subprocess.check_output(argv, cwd=root, text=True).strip()


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def cargo_metadata(root, manifest=None):
    argv = ["cargo", "metadata", "--locked", "--offline", "--format-version", "1"]
    if manifest is not None:
        argv += ["--manifest-path", str(manifest)]
    return json.loads(command(argv, root))


def dependency_graph(metadata):
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    roots = [package["id"] for package in packages.values() if package["name"] == "zterm"]
    if len(roots) != 1:
        raise RuntimeError(f"expected one zterm package, found {len(roots)}")
    pending = roots[:]
    selected = set()
    while pending:
        package_id = pending.pop()
        if package_id in selected:
            continue
        selected.add(package_id)
        pending.extend(dependency["pkg"] for dependency in nodes[package_id]["deps"])
    return sorted(
        ({"name": packages[key]["name"], "version": packages[key]["version"],
          "source": packages[key].get("source"), "features": sorted(nodes[key]["features"])}
         for key in selected),
        key=lambda package: (package["name"], package["version"], package["source"] or ""),
    )


def toml_value(value):
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, str):
        return json.dumps(value)
    if isinstance(value, list) and all(isinstance(item, str) for item in value):
        return "[" + ", ".join(json.dumps(item) for item in value) + "]"
    raise ValueError(f"unsupported zterm dependency value: {value!r}")


def benchmark_manifest(dependency):
    if not isinstance(dependency, dict) or "git" not in dependency:
        raise RuntimeError("the production zterm dependency must use its Git repository")
    if any(key in dependency for key in ("path", "rev", "branch", "tag")):
        raise RuntimeError("expected the canonical zterm Git default-branch dependency")
    if not dependency["git"].startswith("https://github.com/") or not dependency["git"].endswith(".git"):
        raise RuntimeError("zterm Git URL is not canonical")
    permitted = {"git", "version", "default-features", "features", "package"}
    if set(dependency) - permitted:
        raise RuntimeError("unsupported production zterm dependency keys")
    rendered = ", ".join(f"{key} = {toml_value(value)}" for key, value in dependency.items())
    return (
        '[package]\nname = "ztty-search-navigation-benchmark"\n'
        'version = "0.0.0"\nedition = "2024"\n'
        '[workspace]\n[dependencies]\nzterm = { ' + rendered + ' }\n'
        '[profile.release]\nlto = "fat"\ncodegen-units = 1\npanic = "abort"\n'
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-ref", default="HEAD")
    parser.add_argument("--lines", type=int, default=1200)
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--steps", type=int, default=160)
    parser.add_argument("--first-iterations", type=int, default=5)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--lockfile", type=Path, help="Replay a captured benchmark Cargo.lock")
    args = parser.parse_args()
    if min(args.lines, args.samples, args.steps, args.first_iterations) <= 0:
        parser.error("lines, samples, steps and first-iterations must be positive")
    available_cpus = sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None
    if args.cpu is not None and (available_cpus is None or args.cpu not in available_cpus):
        parser.error("--cpu must be one of the current process's available CPUs")

    root = Path(__file__).resolve().parents[1]
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    source_manifest = tomllib.loads((root / "Cargo.toml").read_text())
    production_lock = (root / "Cargo.lock").read_bytes()
    production = cargo_metadata(root)
    expected_graph = dependency_graph(production)
    baseline_commit = command(["git", "rev-parse", "--verify", args.baseline_ref + "^{commit}"], root)
    before = subprocess.check_output(["git", "show", baseline_commit + ":src/search.rs"], cwd=root)
    after = (root / "src/search.rs").read_bytes()
    harness_path = root / "tools/search_navigation_benchmark.rs"
    harness = harness_path.read_bytes()
    started = time.time()
    metadata = {
        "schema": 1,
        "scope": "logical search and result navigation; excludes feed, rendering and input-to-pixel latency",
        "baseline_commit": baseline_commit,
        "candidate_base_commit": command(["git", "rev-parse", "HEAD"], root),
        "candidate_tracked_status": command(["git", "status", "--porcelain", "--untracked-files=no"], root),
        "baseline_search_sha256": sha256(before),
        "candidate_search_sha256": sha256(after),
        "harness_sha256": sha256(harness),
        "runner_sha256": sha256(Path(__file__).read_bytes()),
        "production_cargo_lock_sha256": sha256(production_lock),
        "rustc": command(["rustc", "--version"], root),
        "system": platform.system(), "machine": platform.machine(), "kernel": platform.release(),
        "requested_lines": args.lines, "samples": args.samples,
        "navigation_steps": args.steps, "first_search_iterations": args.first_iterations,
        "cpu_affinity": [args.cpu] if args.cpu is not None else available_cpus,
        "load_average_before": os.getloadavg() if hasattr(os, "getloadavg") else None,
        "timing_method": {
            "sample_order": "AB then BA, alternating by sample",
            "first_search": "backward from None, fresh boxed cache per iteration; construction and destruction included",
            "cold_navigation": "backward initial query plus configured navigation steps; cache creation included",
            "warm_navigation": "initial query and two preparation moves excluded; window refills during measured steps included",
            "warm_preparation": "two backward moves for backward navigation, two forward moves otherwise",
            "allocation_measurement": "separate untimed phase; allocator counting disabled during timings",
        },
    }
    with tempfile.TemporaryDirectory(prefix="ztty-search-navigation-") as directory:
        project = Path(directory)
        source = project / "src"
        source.mkdir()
        (source / "before.rs").write_bytes(before)
        (source / "after.rs").write_bytes(after)
        (source / "main.rs").write_bytes(harness)
        manifest = project / "Cargo.toml"
        manifest.write_text(benchmark_manifest(source_manifest["dependencies"]["zterm"]))
        (project / "Cargo.lock").write_bytes(args.lockfile.read_bytes() if args.lockfile else production_lock)
        argv = ["cargo", "build", "--offline", "--release", "--manifest-path", str(manifest),
                "--message-format=json-render-diagnostics"]
        if args.lockfile is not None:
            argv.append("--locked")
        build = subprocess.run(argv, cwd=root, check=True, text=True, stdout=subprocess.PIPE)
        artifacts = {
            message["executable"]
            for message in map(json.loads, build.stdout.splitlines())
            if message.get("reason") == "compiler-artifact"
            and message.get("executable")
            and message.get("target", {}).get("name") == "ztty-search-navigation-benchmark"
            and "bin" in message["target"]["kind"]
        }
        if len(artifacts) != 1:
            raise RuntimeError("Cargo did not report exactly one benchmark executable")
        binary = Path(artifacts.pop())
        resolved = cargo_metadata(root, manifest)
        actual_graph = dependency_graph(resolved)
        if expected_graph != actual_graph:
            raise RuntimeError("benchmark dependency graph differs from production:\n" +
                               json.dumps({"production": expected_graph, "benchmark": actual_graph}, indent=2))
        if production["target_directory"] != resolved["target_directory"]:
            raise RuntimeError("benchmark did not use the production Cargo target directory")
        lock_bytes = (project / "Cargo.lock").read_bytes()
        lock_path = output.with_suffix(".Cargo.lock")
        lock_path.write_bytes(lock_bytes)
        metadata.update({
            "dependency_graph_matches_production": True,
            "dependency_graph_comparison": "name, version, source and enabled features for the complete zterm closure",
            "dependency_graph": actual_graph,
            "target_directory": resolved["target_directory"],
            "cargo_lockfile": lock_path.name,
            "cargo_lockfile_sha256": sha256(lock_bytes),
            "cargo_lockfile_reused": args.lockfile is not None,
            "binary_sha256": sha256(binary.read_bytes()),
        })
        preexec = None
        if args.cpu is not None:
            def preexec():
                os.sched_setaffinity(0, {args.cpu})
        execution = subprocess.run(
            [str(binary), str(args.lines), str(args.samples), str(args.steps), str(args.first_iterations)],
            cwd=root, check=True, text=True, stdout=subprocess.PIPE, preexec_fn=preexec,
        )
        measurements = json.loads(execution.stdout)

    metadata["elapsed_seconds_including_build"] = time.time() - started
    metadata["load_average_after"] = os.getloadavg() if hasattr(os, "getloadavg") else None
    for scenario in measurements:
        if not scenario["verified"]:
            raise RuntimeError("benchmark reported an unverified scenario")
        for measurement in scenario["timings"]:
            before_ns, after_ns = measurement["baseline_ns"], measurement["candidate_ns"]
            if len(before_ns) != args.samples or len(after_ns) != args.samples or not all(before_ns + after_ns):
                raise RuntimeError("incomplete or zero-duration measurements")
            operations = measurement["operations_per_sample"]
            measurement["baseline_median_ms"] = statistics.median(before_ns) / 1e6
            measurement["candidate_median_ms"] = statistics.median(after_ns) / 1e6
            measurement["baseline_per_operation_median_us"] = statistics.median(before_ns) / operations / 1000
            measurement["candidate_per_operation_median_us"] = statistics.median(after_ns) / operations / 1000
            measurement["paired_speedup_median"] = statistics.median(a / b for a, b in zip(before_ns, after_ns))
            measurement["paired_time_change_percent_median"] = statistics.median(
                100 * (b / a - 1) for a, b in zip(before_ns, after_ns))
    report = {"metadata": metadata, "measurements": measurements}
    output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    print(json.dumps({
        "output": str(output), "verified_scenarios": len(measurements),
        "dependency_graph_matches_production": True,
        "max_retained_cache_heap_bytes": max(scenario["allocation"]["retained_heap_bytes"] for scenario in measurements),
        "measurements": [{"scenario": scenario["scenario"], "phase": timing["phase"],
                          "direction": timing["direction"], "paired_speedup_median": timing["paired_speedup_median"]}
                         for scenario in measurements for timing in scenario["timings"]],
    }, indent=2))


if __name__ == "__main__":
    main()
