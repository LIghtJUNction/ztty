#!/usr/bin/env python3
"""Paired old/new ztty runs using the unchanged ten-window benchmark workload.

Examples:
  python3 tools/benchmark_throughput.py --before /old/ztty --after /new/ztty \
    --rounds 1 --counts 1 --json artifacts/throughput-exploration.json
  python3 tools/benchmark_throughput.py --before /old/ztty --after /new/ztty \
    --json docs/benchmarks/throughput-ab.json

No Kitty/Ghostty numbers or README images are replaced by this collector.
"""
import argparse
import ctypes
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import statistics
import sys
import traceback

import benchmark_terminals as bench

ROOT = Path(__file__).resolve().parents[1]
VARIANTS = ("before", "after")


def utc():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def host_state():
    counters = {}
    for line in Path("/proc/meminfo").read_text().splitlines():
        key, value = line.split(":", 1)
        if key in ("MemTotal", "MemAvailable", "SwapTotal", "SwapFree"):
            counters[key + "_bytes"] = int(value.split()[0]) * 1024
    counters["swap_used_bytes"] = counters["SwapTotal_bytes"] - counters["SwapFree_bytes"]
    counters["load_average"] = list(os.getloadavg())
    counters["logical_cpus"] = len(os.sched_getaffinity(0))
    counters["swap_io_pages"] = {
        key: int(value)
        for key, value in (line.split() for line in Path("/proc/vmstat").read_text().splitlines())
        if key in ("pswpin", "pswpout")
    }
    counters["pressure"] = {}
    for resource in ("cpu", "memory", "io"):
        path = Path("/proc/pressure") / resource
        try:
            counters["pressure"][resource] = path.read_text().strip()
        except OSError:
            counters["pressure"][resource] = None
    return counters


def parse_counts(value):
    try:
        result = tuple(int(count) for count in value.split(","))
    except ValueError as error:
        raise argparse.ArgumentTypeError("counts must be 1, 10, or 1,10") from error
    if not result or len(set(result)) != len(result) or any(count not in (1, 10) for count in result):
        raise argparse.ArgumentTypeError("counts must select unique values from 1 and 10")
    return result


def run_order(counts, rounds):
    for repetition in range(1, rounds + 1):
        # AB / BA / AB balances ordering without hiding any completed sample.
        variants = VARIANTS if repetition % 2 else VARIANTS[::-1]
        ordered_counts = counts if repetition % 2 else counts[::-1]
        for count in ordered_counts:
            for variant in variants:
                yield repetition, count, variant


def positive(value):
    if not isinstance(value, (int, float)) or isinstance(value, bool) or not math.isfinite(value) or value <= 0:
        raise ValueError("measurement must be finite and positive")
    return value


def summarize(report):
    counts = report["settings"]["counts"]
    rounds = report["settings"]["rounds"]
    expected = {(count, repetition, variant) for repetition, count, variant in run_order(counts, rounds)}
    observed = set()
    computed_rates = {}
    for run in report["runs"]:
        identity = (run["windows"], run["repetition"], run["variant"])
        if identity in observed or identity not in expected:
            raise ValueError("duplicate or unexpected before/after case")
        observed.add(identity)
        if run["ok"] is not True or run.get("remaining_owned_pids") != []:
            raise ValueError("failed measurement or missing successful process cleanup")
        windows = run.get("windows_observed", [])
        owners = run.get("gui_owner_pids", [])
        if (len(windows) != run["windows"]
                or len({window["id"] for window in windows}) != run["windows"]
                or len({window["title"] for window in windows}) != run["windows"]
                or len(owners) != 1
                or run.get("gui_process_count") != 1
                or {window["pid"] for window in windows} != set(owners)):
            raise ValueError("missing or inconsistent mapped-window/GUI-owner evidence")
        if run["binary_sha256_before"] != run["binary_sha256_after"]:
            raise ValueError("binary changed during measurement")
        if run["binary_sha256_before"] != report["binaries"][run["variant"]]["sha256"]:
            raise ValueError("binary differs from the recorded executable")
        if (len(run["performance"]) != run["windows"]
                or {window["index"] for window in run["performance"]}
                != set(range(1, run["windows"] + 1))):
            raise ValueError("missing or duplicate window performance samples")
        window_rates = []
        for window in run["performance"]:
            integrity = window.get("output_integrity", {})
            if integrity.get("final_cursor_checked") is not True:
                raise ValueError("missing direct cursor completion check")
            if integrity.get("screen", {}).get("all_text_cells_checked") is not True:
                raise ValueError("missing complete final screen text check")
            if integrity.get("payload_sha256") != report["settings"]["output_sha256"]:
                raise ValueError("integrity check used a different output workload")
            if len(window["output"]) != bench.OUTPUT_SAMPLES:
                raise ValueError("missing raw throughput samples")
            sample_rates = []
            for sample in window["output"]:
                if sample["bytes"] != report["settings"]["output_bytes_per_sample"]:
                    raise ValueError("timed output byte count differs")
                rate = sample["bytes"] / (1024 * 1024) / positive(sample["seconds"])
                if not math.isclose(positive(sample["mib_per_second"]), rate, rel_tol=1e-9):
                    raise ValueError("sample throughput differs from its raw bytes and elapsed time")
                sample_rates.append(rate)
            rate = statistics.median(sample_rates)
            if not math.isclose(positive(window["median_output_mib_per_second"]), rate, rel_tol=1e-9):
                raise ValueError("window throughput differs from its raw sample median")
            window_rates.append(rate)
        rate = statistics.median(window_rates)
        if not math.isclose(positive(run["output_mib_per_second"]), rate, rel_tol=1e-9):
            raise ValueError("run throughput differs from its window medians")
        computed_rates[identity] = rate
        for key in ("latency_p50_us", "latency_p95_us"):
            positive(run[key])
        positive(run["application"]["Pss_bytes"])
    if observed != expected:
        raise ValueError("before/after run matrix is incomplete")
    summary = []
    for count in counts:
        row = {"windows": count}
        for variant in VARIANTS:
            selected = sorted(
                (run for run in report["runs"] if run["windows"] == count and run["variant"] == variant),
                key=lambda run: run["repetition"],
            )
            rates = [computed_rates[(count, run["repetition"], variant)] for run in selected]
            row[variant] = {
                "runs": len(selected), "output_mib_per_second": rates,
                "median_output_mib_per_second": statistics.median(rates),
                "output_range_mib_per_second": [min(rates), max(rates)],
                "median_latency_p50_us": statistics.median(run["latency_p50_us"] for run in selected),
                "median_latency_p95_us": statistics.median(run["latency_p95_us"] for run in selected),
                "median_application_pss_bytes": statistics.median(run["application"]["Pss_bytes"] for run in selected),
            }
        row["ratio_of_median_throughputs"] = (
            row["after"]["median_output_mib_per_second"] / row["before"]["median_output_mib_per_second"])
        row["paired_throughput_ratios"] = [
            after / before for before, after in zip(
                row["before"]["output_mib_per_second"], row["after"]["output_mib_per_second"])
        ]
        row["median_paired_throughput_ratio"] = statistics.median(row["paired_throughput_ratios"])
        summary.append(row)
    return summary


def source_state():
    return {
        "head": bench.capture(["git", "-C", str(ROOT), "rev-parse", "HEAD"]),
        "status": bench.capture(["git", "-C", str(ROOT), "status", "--short"]),
    }


def verify_sources(expected):
    actual = {name: bench.sha256(ROOT / "tools" / name) for name in expected}
    if actual != expected:
        raise RuntimeError("benchmark source changed during collection")
    return actual


def save(path, report):
    report["raw_runs_sha256"] = hashlib.sha256(
        json.dumps(report["runs"], sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    bench.atomic_write(path, json.dumps(report, ensure_ascii=False, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--after", type=Path, required=True)
    parser.add_argument("--before-source", help="known source revision for the old executable")
    parser.add_argument("--after-source", help="known source revision for the new executable")
    parser.add_argument("--counts", type=parse_counts, default=(1, 10))
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--settle-seconds", type=float, default=1.0)
    parser.add_argument("--json", type=Path, required=True)
    args = parser.parse_args()
    if not 1 <= args.rounds <= 9:
        parser.error("rounds must be from 1 through 9")
    if not 0.1 <= args.settle_seconds <= 10:
        parser.error("settle-seconds must be from 0.1 through 10")
    if args.json.exists():
        parser.error("refusing to overwrite a previous A/B collection; choose a new path")
    for program in ("Xvfb", "dbus-daemon"):
        if not shutil.which(program):
            parser.error(program + " is required")
    try:
        import Xlib  # noqa: F401
        binaries = {"before": args.before.resolve(strict=True), "after": args.after.resolve(strict=True)}
    except (ImportError, FileNotFoundError) as error:
        parser.error(str(error))
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("cannot enable child subreaper")
    metadata = {variant: bench.binary_metadata(binary) for variant, binary in binaries.items()}
    metadata["before"]["source_revision"] = args.before_source
    metadata["after"]["source_revision"] = args.after_source
    payload = bench.output_payload()
    helpers = ("benchmark_throughput.py", "benchmark_terminals.py",
               "measure_runtime.py", "measure_multiwindow.py", "isolated_xvfb.py")
    report = {
        "schema": 1, "ok": False, "timestamp": utc(),
        "purpose": "paired comparison" if args.rounds >= 3 else "exploration",
        "machine": bench.machine(), "command": [sys.executable, *sys.argv],
        "git_before": source_state(), "binaries": metadata,
        "source_sha256": {name: bench.sha256(ROOT / "tools" / name)
                          for name in helpers if (ROOT / "tools" / name).is_file()},
        "settings": {
            "counts": list(args.counts), "rounds": args.rounds, "geometry": [bench.COLS, bench.ROWS],
            "sharing": "shared ztty broker plus every waiting launcher",
            "memory_settle_seconds": args.settle_seconds,
            "output_bytes_per_sample": len(payload),
            "output_sha256": hashlib.sha256(payload).hexdigest(),
            "output_warmup_per_window": bench.OUTPUT_WARMUP,
            "output_samples_per_window": bench.OUTPUT_SAMPLES,
            "timed_completion_command_hex": bench.BARRIER.hex(),
            "timed_completion_reply_hex": bench.BARRIER_REPLY.hex(),
            "untimed_output_check_command_hex": bench.OUTPUT_CHECK.hex(),
            "untimed_output_check_reply_hex": bench.OUTPUT_CHECK_REPLY.hex(),
        },
        "method": {
            "ordering": "fresh-process AB / BA alternating each round; window count order also alternates",
            "timing": "unchanged benchmark_terminals.py payload and HOME/DSR barrier; one active window at a time",
            "aggregation": "median of three bursts per window, median of windows per run, then median of runs",
            "integrity": "separate untimed replay requires direct CPR at 32;120 and all 32 final screen text rows; no rendering work is intentionally omitted",
            "memory": "pre-performance resident PSS including shared host and all waiting launchers; full memory snapshots and X server retained",
            "host": "available memory, swap use, swap I/O counters, pressure stalls, affinity and load recorded; unrelated work is left running",
            "scope": "Xvfb software rendering and logical PTY processing; not physical presentation or keyboard-to-photon latency",
        },
        "run_order": [{"repetition": r, "windows": count, "variant": variant}
                      for r, count, variant in run_order(args.counts, args.rounds)],
        "runs": [],
    }
    original_windows = bench.WINDOWS
    save(args.json, report)
    try:
        for repetition, count, variant in run_order(args.counts, args.rounds):
            binary = binaries[variant]
            digest = bench.sha256(binary)
            if digest != metadata[variant]["sha256"]:
                raise RuntimeError("executable changed before measurement: " + variant)
            verify_sources(report["source_sha256"])
            before = host_state()
            started = utc()
            bench.WINDOWS = count
            run = bench.measure("ztty", binary, repetition, args.settle_seconds)
            run.update(variant=variant, started_at=started, completed_at=utc(),
                       host_at_start=before, host_at_end=host_state(),
                       binary_sha256_before=digest, binary_sha256_after=bench.sha256(binary))
            report["runs"].append(run)
            verify_sources(report["source_sha256"])
            save(args.json, report)
            print(json.dumps({"variant": variant, "windows": count, "repetition": repetition,
                              "ok": run["ok"], "output_mib_per_second": run.get("output_mib_per_second"),
                              "latency_p50_us": run.get("latency_p50_us"), "error": run.get("error"),
                              "remaining_owned_pids": run.get("remaining_owned_pids")}), flush=True)
            if not run["ok"]:
                raise RuntimeError("measurement failed; diagnostic and partial samples preserved")
        report["summary"] = summarize(report)
        report["ok"] = True
    except Exception as error:
        report["error"] = repr(error)
        report["traceback"] = traceback.format_exc()
    finally:
        bench.WINDOWS = original_windows
        report["completed_at"] = utc()
        report["git_after"] = source_state()
        report["source_sha256_after"] = {
            name: bench.sha256(ROOT / "tools" / name) for name in report["source_sha256"]
        }
        if report["source_sha256_after"] != report["source_sha256"]:
            report["ok"] = False
            report["error"] = "benchmark source changed during collection"
        save(args.json, report)
    print(json.dumps({"ok": report["ok"], "purpose": report["purpose"],
                      "json": str(args.json), "summary": report.get("summary"),
                      "error": report.get("error")}), flush=True)
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
