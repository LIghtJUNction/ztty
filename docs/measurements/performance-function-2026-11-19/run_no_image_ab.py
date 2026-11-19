import hashlib, json, platform, statistics, subprocess, time
from pathlib import Path
p = Path("artifacts/preview-fix")
data = {
    "scope": "text-only parser and screen; excludes PTY and rendering",
    "baseline_commit": "11a74a51f0f6c52471c006582d46439cd713ce26",
    "candidate_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
    "cpu_affinity": 23, "kernel": platform.release(), "runs": [],
    "benchmark_binary_sha256": {label: hashlib.sha256((p / f"core-{label}").read_bytes()).hexdigest() for label in ["baseline", "candidate"]},
}
for pair in range(3):
    for label in (["baseline", "candidate"] if pair % 2 == 0 else ["candidate", "baseline"]):
        argv = ["taskset", "-c", "23", str(p / f"core-{label}")]
        started = time.time()
        result = json.loads(subprocess.check_output(argv, text=True))
        data["runs"].append({"pair": pair, "label": label, "command": argv, "started_unix": started, "result": result})
        (p / "no-image-ab.json").write_text(json.dumps(data, indent=2) + "\n")
    print(f"pair {pair + 1}/3 complete", flush=True)
fields = ["ascii_mib_s", "ascii_full_history_mib_s", "sgr_mib_s", "unicode_mib_s", "initial_core_heap_bytes", "steady_state_allocations"]
summary = {label: {field: statistics.median(run["result"][field] for run in data["runs"] if run["label"] == label) for field in fields} for label in ["baseline", "candidate"]}
(p / "no-image-summary.json").write_text(json.dumps(summary, indent=2) + "\n")
print(json.dumps(summary), flush=True)
