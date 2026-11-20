#!/usr/bin/env python3
"""Check the archived nine-run record without building or running benchmarks."""
import csv
import hashlib
import io
import json
import math
from pathlib import Path
import statistics

HERE = Path(__file__).resolve().parent
METRICS = (
    "ascii_mib_s", "ascii_full_history_mib_s", "sgr_mib_s", "unicode_mib_s",
    "cell_bytes", "initial_core_heap_bytes", "steady_state_allocations",
)
SIDES = ("legacy_0_4", "published_0_5", "candidate_0_5_1")
FIELDS = (
    "run", "round", "side", "exit_code", "elapsed_seconds",
    "geometry_columns", "geometry_rows",
    *METRICS,
    "edit_p50_ns", "edit_p95_ns", "edit_p99_ns", "edit_allocations",
    "burst_1024_p50_ns", "burst_1024_p95_ns", "burst_1024_p99_ns",
    "burst_1024_allocations",
)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def measurements_csv(record):
    out = io.StringIO(newline="")
    writer = csv.DictWriter(out, fieldnames=FIELDS, lineterminator="\n")
    writer.writeheader()
    for index, run in enumerate(record["runs"], 1):
        result = run["result"]
        row = {"run": index, **{key: run[key] for key in (
            "round", "side", "exit_code", "elapsed_seconds")}}
        row.update(geometry_columns=result["geometry"][0], geometry_rows=result["geometry"][1])
        row.update({key: result[key] for key in METRICS})
        for prefix, key in (("edit", "edit_ns"), ("burst_1024", "burst_1024_ns")):
            row.update({f"{prefix}_{part}_ns": result[key][part]
                        for part in ("p50", "p95", "p99")})
            row[f"{prefix}_allocations"] = result[key]["allocations"]
        writer.writerow(row)
    return out.getvalue()


def main():
    manifest = json.loads((HERE / "redaction.json").read_text())
    for entry in manifest["files"]:
        assert digest((HERE / entry["public_file"]).read_bytes()) == entry["public_sha256"], entry["public_file"]
    for entry in manifest["prior_evidence"]:
        path = HERE.parent / entry["file"]
        assert digest(path.read_bytes()) == entry["sha256"], entry["file"]
    report = HERE.parent.parent / "resume-2026-11-20.md"
    prior = manifest["prior_report"]
    assert digest(report.read_bytes()[:prior["bytes"]]) == prior["sha256"]
    data = json.loads((HERE / "comparison.json").read_text())
    receipt = json.loads((HERE / "receipt.json").read_text())
    assert receipt["sha256"] == manifest["files"][0]["original_sha256"]
    assert data["ok"] and data["status"] == "complete" and receipt["ok"]
    assert len(data["runs"]) == receipt["runs"] == 9
    assert [(r["round"], r["side"]) for r in data["runs"]] == [
        (round_, side) for round_, order in enumerate(data["order"], 1) for side in order]
    assert data["order"] == [list(SIDES), list(SIDES[1:] + SIDES[:1]),
                             list(SIDES[2:] + SIDES[:2])]
    assert digest((HERE / "run-three-way.py").read_bytes()) == data["collector_sha256"]
    build = json.loads((HERE / "candidate-build.json").read_text())
    assert build["source"] == data["sources"]["candidate_0_5_1"]
    assert build["binary_sha256"] == data["binary_sha256"]["candidate_0_5_1"]
    assert build["example_sha256"] == data["example_sha256"]
    for run in data["runs"]:
        stem = f'{run["round"]}-{run["side"]}'
        assert json.loads((HERE / f"{stem}.stdout.json").read_text()) == run["result"], stem
        assert (HERE / f"{stem}.stderr.log").read_text() == run["stderr"], stem
        assert run["exit_code"] == 0 and not run["host_contention_after"], stem
        assert run["result"]["geometry"] == [120, 32], stem
        for host in (run["host_before"], run["host_after"]):
            assert host["MemAvailable_bytes"] >= max(2 * 1024**3, host["MemTotal_bytes"] // 10)
            assert host["load_average"][0] <= host["logical_cpus"] / 2
    assert receipt["all_exit_zero"] and receipt["all_host_guards_passed"]
    assert math.isclose(sum(r["elapsed_seconds"] for r in data["runs"]),
                        receipt["total_binary_seconds"], rel_tol=1e-12)
    for metric in METRICS:
        summary = data["summary"][metric]
        samples = {side: [r["result"][metric] for r in data["runs"] if r["side"] == side]
                   for side in SIDES}
        medians = {side: statistics.median(values) for side, values in samples.items()}
        assert samples == summary["samples"], metric
        assert medians == summary["medians"], metric
        for name, side in (("candidate_over_published", SIDES[1]),
                           ("candidate_over_legacy", SIDES[0])):
            expected = medians[SIDES[2]] / medians[side] if medians[side] else None
            assert expected == summary[name], (metric, name)
    assert (HERE / "measurements.csv").read_text() == measurements_csv(data)
    assert data["source_and_binary_identities_preserved"] and data["original_six_samples_unchanged"]
    assert receipt["identities_preserved"] and receipt["old_samples_unchanged"]
    assert data["original_six_samples_sha256"] == manifest["original_six_samples_sha256"]
    print(json.dumps({"verified": True, "runs": len(data["runs"]), "summary_metrics": len(METRICS),
                      "public_file_hashes": len(manifest["files"]), "prior_evidence_files": len(manifest["prior_evidence"]),
                      "prior_report_prefix_unchanged": True, "benchmarks_executed": False}, sort_keys=True))


if __name__ == "__main__":
    main()
