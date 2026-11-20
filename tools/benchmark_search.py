#!/usr/bin/env python3
"""Compare search.rs from a Git ref and the current tree with identical zterm.

This measures synchronous full-history find(), not input latency or GPU frames.
Uses only Python/Rust standard libraries and the repository's existing zterm.
Example:
    python3 tools/benchmark_search.py --baseline-ref HEAD --lines 100000 \
        --samples 7 --output docs/benchmarks/search-before-after.json
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


def command(args, cwd=None):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-ref", default="HEAD")
    parser.add_argument("--lines", type=int, default=100000, help="Logical lines injected before measurement; the history budget may retain fewer rows")
    parser.add_argument("--samples", type=int, default=7)
    parser.add_argument("--iterations", type=int, default=1)
    parser.add_argument("--cpu", type=int, help="Pin the benchmark process to one available CPU")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if min(args.lines, args.samples, args.iterations) <= 0:
        parser.error("lines, samples and iterations must be positive")
    root = Path(__file__).resolve().parents[1]
    baseline_commit = command(["git", "rev-parse", "--verify", args.baseline_ref + "^{commit}"], root)
    candidate_commit = command(["git", "rev-parse", "HEAD"], root)
    before = command(["git", "show", baseline_commit + ":src/search.rs"], root) + "\n"
    after = (root / "src/search.rs").read_text()
    harness = (root / "tools/search_benchmark.rs").read_text()
    metadata = {
        "schema": 1,
        "scope": "synchronous full-history literal search; excludes terminal feed/build time",
        "baseline_commit": baseline_commit,
        "candidate_commit": candidate_commit,
        "candidate_worktree_status": command(["git", "status", "--porcelain"], root),
        "baseline_search_sha256": hashlib.sha256(before.encode()).hexdigest(),
        "candidate_search_sha256": hashlib.sha256(after.encode()).hexdigest(),
        "harness_sha256": hashlib.sha256(harness.encode()).hexdigest(),
        "rustc": command(["rustc", "--version"]),
        "machine": platform.machine(),
        "system": platform.system(),
        "kernel": platform.release(),
        "samples": args.samples,
        "iterations_per_sample": args.iterations,
        "logical_lines_per_scenario": args.lines,
        "load_average_before": os.getloadavg() if hasattr(os, "getloadavg") else None,
    }
    try:
        metadata["zterm_commit"] = command(["git", "-C", str(root / "crates/zterm"), "rev-parse", "HEAD"])
        metadata["zterm_worktree_status"] = command(["git", "-C", str(root / "crates/zterm"), "status", "--porcelain"])
    except subprocess.CalledProcessError:
        metadata["zterm_commit"] = None
    if args.cpu is not None:
        if not hasattr(os, "sched_setaffinity"):
            parser.error("--cpu is supported only where sched_setaffinity is available")
        os.sched_setaffinity(0, {args.cpu})
    metadata["cpu_affinity"] = sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None
    started = time.time()
    with tempfile.TemporaryDirectory(prefix="ztty-search-bench-") as directory:
        project = Path(directory)
        source = project / "src"
        source.mkdir()
        (source / "before.rs").write_text(before)
        (source / "after.rs").write_text(after)
        (source / "main.rs").write_text(harness)
        manifest = (
            '[package]\nname = "ztty-search-benchmark"\nversion = "0.0.0"\nedition = "2024"\n'
            '[workspace]\n[dependencies]\nzterm = { path = '
            + json.dumps(str(root / "crates/zterm"))
            + ', default-features = false }\n'
            '[profile.release]\nlto = "fat"\ncodegen-units = 1\npanic = "abort"\n'
        )
        (project / "Cargo.toml").write_text(manifest)
        target = root / "target/search-benchmark"
        subprocess.run(
            ["cargo", "build", "--release", "--manifest-path", str(project / "Cargo.toml"),
             "--target-dir", str(target)],
            check=True,
        )
        binary = target / "release/ztty-search-benchmark"
        if os.name == "nt":
            binary = binary.with_suffix(".exe")
        result = subprocess.run(
            [str(binary), str(args.lines), str(args.samples), str(args.iterations)],
            check=True, stdout=subprocess.PIPE, text=True,
        )
        measurements = json.loads(result.stdout)
    metadata["elapsed_seconds_including_build"] = time.time() - started
    metadata["load_average_after"] = os.getloadavg() if hasattr(os, "getloadavg") else None
    for measurement in measurements:
        baseline = measurement["baseline_ns"]
        candidate = measurement["candidate_ns"]
        measurement["baseline_median_ms"] = statistics.median(baseline) / args.iterations / 1e6
        measurement["candidate_median_ms"] = statistics.median(candidate) / args.iterations / 1e6
        measurement["paired_speedup_median"] = statistics.median(a / b for a, b in zip(baseline, candidate))
        measurement["paired_latency_change_percent_median"] = statistics.median(
            100 * (b / a - 1) for a, b in zip(baseline, candidate)
        )
    report = {"metadata": metadata, "measurements": measurements}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    print(json.dumps(report, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
