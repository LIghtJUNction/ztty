#!/usr/bin/env python3
"""Recompute this saved experiment; do not run benchmarks or access the network."""
import csv
import hashlib
import json
import math
import statistics
from pathlib import Path

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
MAIN_REPORT = BENCH.parent / "resume-2026-11-20.md"
METRICS = ("ascii_mib_s", "ascii_full_history_mib_s", "sgr_mib_s", "unicode_mib_s",
           "cell_bytes", "initial_core_heap_bytes", "steady_state_allocations")


def sha(data):
    return hashlib.sha256(data).hexdigest()


def read(name):
    return json.loads((HERE / name).read_text())


def close(a, b):
    if a is None or b is None:
        assert a is b
    else:
        assert math.isclose(a, b, rel_tol=1e-14, abs_tol=1e-14), (a, b)


def main():
    index = read("redaction.json")
    for entry in index["files"]:
        data = (HERE / entry["public_file"]).read_bytes()
        assert sha(data) == entry["public_sha256"], entry["public_file"]
        assert len(data) == entry["public_bytes"]
    for entry in index["derived_files"]:
        assert sha((HERE / entry["file"]).read_bytes()) == entry["sha256"]
    for entry in index["prior_evidence"]:
        assert sha((BENCH / entry["file"]).read_bytes()) == entry["sha256"]
    prefix = index["prior_report"]
    assert sha(MAIN_REPORT.read_bytes()[:prefix["bytes"]]) == prefix["sha256"]
    three = read("core-three-way-28d5d26/comparison.json")
    focused = read("core-focused-a207-28d5/comparison.json")
    for name, obj, count, order_key, after_key in (
        ("core-three-way-28d5d26", three, 9, "order", "host_contention_after"),
        ("core-focused-a207-28d5", focused, 12, "orders", "host_reasons_after"),
    ):
        assert obj["ok"] and obj["status"] == "complete"
        assert obj["root_source"] == "da8b697d6ca3b3948810ee632e3f5d90cbd60913"
        assert obj["cpu"] == 23 and len(obj["runs"]) == count
        expected_order = [(i + 1, side) for i, order in enumerate(obj[order_key]) for side in order]
        assert [(r["round"], r["side"]) for r in obj["runs"]] == expected_order
        for run in obj["runs"]:
            assert run["exit_code"] == 0 and not run[after_key]
            assert run["command"][:3] == ["taskset", "-c", "23"]
            path = f"{name}/{run['round']}-{run['side']}"
            assert read(path + ".stdout.json") == run["result"]
            assert (HERE / (path + ".stderr.log")).read_bytes() == b""
            assert run["result"]["geometry"] == [120, 32]
        for metric in METRICS:
            saved = obj["summary"][metric]
            samples = {side: [r["result"][metric] for r in obj["runs"] if r["side"] == side]
                       for side in obj["sources"]}
            medians = {side: statistics.median(values) for side, values in samples.items()}
            assert saved["samples"] == samples and saved["medians"] == medians
            if count == 9:
                candidate = medians["candidate_0_5_1"]
                for side, key in (("published_0_5", "candidate_over_published"),
                                  ("legacy_0_4", "candidate_over_legacy")):
                    close(saved[key], candidate / medians[side] if medians[side] else None)
            else:
                close(saved["batch_over_borrowed"],
                      medians["print_batch"] / medians["borrowed"] if medians["borrowed"] else None)
                for i, ratio in enumerate(saved["per_round_batch_over_borrowed"]):
                    a, b = samples["borrowed"][i], samples["print_batch"][i]
                    close(ratio, b / a if a else None)
    assert three["source_and_binary_identities_preserved"]
    assert three["original_six_samples_unchanged"] and three["previous_nine_samples_and_candidate_unchanged"]
    assert focused["identities_and_prior_records_preserved"]
    assert three["sources"]["candidate_0_5_1"] == focused["sources"]["print_batch"]
    assert three["binary_sha256"]["candidate_0_5_1"] == focused["binary_sha256"]["print_batch"]
    for obj, keys in ((three, ("execution_context_before", "execution_context_after")),
                      (focused, ("execution_before", "execution_after"))):
        for key in keys:
            context = obj[key]
            assert context["nice"] == -4
            assert all(x["cpu.max"].split()[0] == "max" for x in context["ancestor_cpu_limits"] if "cpu.max" in x)
    builds = {side: read(path) for side, path in (
        ("borrowed", "build-a207.json"), ("print_batch", "build-28d5.json"))}
    assert builds["borrowed"]["profile"] == builds["print_batch"]["profile"]
    assert builds["borrowed"]["rustc"] == builds["print_batch"]["rustc"]
    for side, build in builds.items():
        assert build["passed"] and build["source"] == focused["sources"][side]
        assert build["binary_sha256"] == focused["binary_sha256"][side]
        assert build["example_sha256"] == sha((HERE / "source/benchmark.rs").read_bytes())
    for folder, commands in (("core-three-way-28d5d26", 9), ("core-focused-a207-28d5", 12)):
        receipt = read(folder + "/launch-receipt.json")
        assert receipt["recorded_after_completion"] and receipt["job_observed_exit"] == 0
        assert receipt["performance_commands_executed"] == commands
    receipt = read("core-focused-a207-28d5/launch-receipt.json")
    assert receipt["attempts_started"] == 1
    assert receipt["prior_deferred_or_failed_attempts_for_this_focused_collection"] == 0
    files = {e["public_file"]: e for e in index["files"]}
    for filename, expected in (
        ("run-three-way-print-batch.py", three["collector_sha256"]),
        ("run-focused-print-batch.py", focused["collector_sha256"]),
        ("build-a207.json", focused["build_record_sha256"]["borrowed"]),
        ("build-28d5.json", focused["build_record_sha256"]["print_batch"]),
    ):
        assert files[filename]["original_sha256"] == expected
    rows = list(csv.DictReader((HERE / "measurements.csv").open(newline="")))
    source_rows = [(name, i + 1, run) for name, obj in (
        ("three_way", three), ("focused", focused)) for i, run in enumerate(obj["runs"])]
    assert len(rows) == len(source_rows) == 21
    for row, (cohort, seq, run) in zip(rows, source_rows):
        assert (row["cohort"], int(row["sequence"]), int(row["round"]), row["side"]) == (
            cohort, seq, run["round"], run["side"])
        for metric in METRICS:
            close(float(row[metric]), run["result"][metric])
        for prefix in ("edit_ns", "burst_1024_ns"):
            for key in ("p50", "p95", "p99", "allocations"):
                assert int(row[prefix + "_" + key]) == run["result"][prefix][key]
        close(float(row["elapsed_seconds"]), run["elapsed_seconds"])
        for key in ("child_user_seconds", "child_system_seconds"):
            if key in run:
                close(float(row[key]), run[key])
            else:
                assert row[key] == ""
    decision = read("decision.json")
    assert decision["selected_source"] == focused["sources"]["borrowed"]
    assert decision["rejected_source"] == focused["sources"]["print_batch"]
    unicode_pairs = focused["summary"]["unicode_mib_s"]["per_round_batch_over_borrowed"]
    assert sum(ratio > 1 for ratio in unicode_pairs) == 2
    assert all(focused["summary"][key]["batch_over_borrowed"] < 1 for key in METRICS[:4])
    print(json.dumps({"verified": True, "cohorts": [9, 12], "sample_count": 21,
                      "prior_files_preserved": len(index["prior_evidence"]),
                      "selected_source": decision["selected_source"],
                      "benchmarks_builds_network_executed": False}, sort_keys=True))


if __name__ == "__main__":
    main()
