import argparse
import json
import os
import platform
import statistics
import subprocess
import time
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--candidate-suffix", default="")
parser.add_argument("--output-suffix", default="")
args = parser.parse_args()
output = Path("artifacts/core-scroll")
cpu = max(os.sched_getaffinity(0))
record = {
    "scope": "VT parser and retained screen only; excludes PTY and rendering",
    "base_commit": "5ce8a1dea01c38d10cbfc7a7ab9c4d3b0cdb5107",
    "candidate_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
    "cpu_affinity": cpu,
    "cpu_model": next(line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")),
    "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
    "kernel": platform.release(),
    "started_unix": time.time(),
    "runs": [],
}
governor = Path(f"/sys/devices/system/cpu/cpu{cpu}/cpufreq/scaling_governor")
record["cpu_governor"] = governor.read_text().strip() if governor.exists() else None
for pair in range(5):
    for variant in (["baseline", "candidate"] if pair % 2 == 0 else ["candidate", "baseline"]):
        for kind in ["scroll", "core"]:
            command = ["taskset", "-c", str(cpu), str(output / f"{kind}-{variant}{args.candidate_suffix if variant == 'candidate' else ''}")]
            started = time.time()
            data = json.loads(subprocess.check_output(command, text=True))
            record["runs"].append({"pair": pair, "variant": variant, "kind": kind, "started_unix": started, "elapsed_seconds": time.time() - started, "command": command, "result": data})
            (output / f"raw-ab{args.output_suffix}.json").write_text(json.dumps(record, indent=2) + "\n")
    print(f"pair {pair + 1}/5 complete", flush=True)

summary = {"scope": record["scope"], "cpu_affinity": cpu, "scroll": [], "core": {}}
scroll_runs = [run for run in record["runs"] if run["kind"] == "scroll"]
for reference in scroll_runs[0]["result"]["cases"]:
    key = {field: reference[field] for field in ["cols", "rows", "count", "command"]}
    item = dict(key)
    for variant in ["baseline", "candidate"]:
        cases = [case for run in scroll_runs if run["variant"] == variant for case in run["result"]["cases"] if all(case[field] == value for field, value in key.items())]
        medians = [statistics.median(case["ns_per_scroll"]) for case in cases]
        item[variant] = {"median_ns": statistics.median(medians), "run_medians_ns": medians, "allocations": sum(case["allocations"] for case in cases)}
    item["speedup"] = item["baseline"]["median_ns"] / item["candidate"]["median_ns"]
    summary["scroll"].append(item)

core_runs = [run for run in record["runs"] if run["kind"] == "core"]
for variant in ["baseline", "candidate"]:
    values = [run["result"] for run in core_runs if run["variant"] == variant]
    summary["core"][variant] = {field: statistics.median(value[field] for value in values) for field in ["ascii_mib_s", "ascii_full_history_mib_s", "sgr_mib_s", "unicode_mib_s", "initial_core_heap_bytes", "steady_state_allocations"]}
    for field in ["burst_1024_ns", "edit_ns"]:
        summary["core"][variant][field] = {subfield: statistics.median(value[field][subfield] for value in values) for subfield in ["p50", "p95", "p99", "allocations"]}
(output / f"summary-ab{args.output_suffix}.json").write_text(json.dumps(summary, indent=2) + "\n")
print(json.dumps(summary, indent=2), flush=True)
