#!/usr/bin/env python3
"""Observe two fixed ztty ELFs with the existing output-pipeline collector."""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--baseline-source", required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--candidate-source", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    versions = {
        "baseline": (args.baseline.resolve(strict=True), args.baseline_source),
        "candidate": (args.candidate.resolve(strict=True), args.candidate_source),
    }
    collector = root / "tools/benchmark_output_pipeline.py"
    report = {
        "ok": False,
        "scope": "Short integration regression check; not a new optimization claim",
        "pairs": 2,
        "order": ["baseline", "candidate", "candidate", "baseline"],
        "mib": 8,
        "samples": 3,
        "started_unix": time.time(),
        "runner_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "collector_sha256": hashlib.sha256(collector.read_bytes()).hexdigest(),
        "binaries": {
            version: {"path": str(binary), "source_commit": source,
                      "sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
            for version, (binary, source) in versions.items()
        },
        "runs": [],
    }

    def save():
        (out / "raw.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")

    save()
    for pair in range(2):
        for version in (["baseline", "candidate"] if pair == 0 else ["candidate", "baseline"]):
            binary, source = versions[version]
            assert hashlib.sha256(binary.read_bytes()).hexdigest() == report["binaries"][version]["sha256"]
            path = out / f"{pair}-{version}.json"
            command = [sys.executable, str(collector), "--binary", str(binary),
                       "--binary-source", source, "--output", str(path),
                       "--mib", "8", "--samples", "3"]
            with path.with_suffix(".log").open("w") as log:
                result = subprocess.run(command, cwd=root, stdout=log, stderr=subprocess.STDOUT)
            if result.returncode:
                report["failure"] = {"pair": pair, "version": version,
                                     "exit_code": result.returncode, "path": str(path)}
                save()
                return result.returncode
            data = json.loads(path.read_text())
            assert data["ok"] and data["binary_sha256"] == report["binaries"][version]["sha256"]
            assert data["collector_sha256"] == report["collector_sha256"]
            assert all(run["all_visible_text_checked"] and not run["owned_remaining"] for run in data["runs"])
            report["runs"].append({"pair": pair, "version": version, "data": data})
            save()
            print(pair, version, "complete", flush=True)
    rows = []
    for kind in report["runs"][0]["data"]["settings"]["kinds"]:
        for mode in report["runs"][0]["data"]["settings"]["modes"]:
            row = {"kind": kind, "mode": mode}
            for version in versions:
                runs = [next(run for run in group["data"]["runs"]
                             if run["kind"] == kind and run["mode"] == mode)
                        for group in report["runs"] if group["version"] == version]
                medians = [run["median_mib_s"] for run in runs]
                row[version] = {
                    "mib_s": statistics.median(medians),
                    "range_mib_s": [min(medians), max(medians)],
                    "run_medians_mib_s": medians,
                    "idle_dsr_p50_ns": statistics.median(run["idle_dsr_p50_ns"] for run in runs),
                    "idle_dsr_p95_ns": statistics.median(run["idle_dsr_p95_ns"] for run in runs),
                }
            row["throughput_delta_percent"] = (row["candidate"]["mib_s"] / row["baseline"]["mib_s"] - 1) * 100
            rows.append(row)
    report["ok"] = True
    report["finished_unix"] = time.time()
    save()
    summary = {
        "ok": True,
        "scope": report["scope"],
        "method": "Median of two per-run medians (three samples per run); range is those two medians, not a confidence interval. AB then BA. Logical PTY cursor reply, not physical display latency.",
        "binaries": report["binaries"],
        "rows": rows,
    }
    (out / "summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(summary, ensure_ascii=False), flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
