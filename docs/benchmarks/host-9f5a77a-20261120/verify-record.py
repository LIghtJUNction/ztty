#!/usr/bin/env python3
"""Read-only arithmetic/provenance verification; never launches benchmarks."""
from __future__ import annotations

import csv
import gzip
import hashlib
import io
import json
import math
from pathlib import Path
import statistics

HERE = Path(__file__).resolve().parent
SOURCE = "3cace1d18446d1cc63dc84d635955e5a2b213a0b"
BINARY_SOURCE = "9f5a77a1299093d0ba609bfafa2cec3b0ba931dd"
BINARY = "c7935e9e76476a9cd7d0186f23f5806e3e2e2fc556a494b0e3741a2c893b3807"
OLD_SOURCE = "155fea51aa138b0503ea1d80be4252aacb2c61a7"
OLD_BINARY = "9f58c1a679ccca62cffd09d9991fbbb9c35fff4254f7d64dfc5eedaa6a2bb704"
MIB = 1024 ** 2
PAIRED = "attempt2/throughput/paired.json.gz"
TERMINALS = "attempt2/terminals/terminals.json.gz"
OUTPUT_NAMES = [kind + "-" + mode for kind in ("ascii-log", "inplace-ascii", "unicode-log") for mode in ("headless", "window")]
CHECKS = []


def require(ok, message):
    if not ok:
        raise AssertionError(message)
    CHECKS.append(message)


def same_number(a, b, label):
    require(math.isclose(a, b, rel_tol=1e-10, abs_tol=1e-12), label)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def read(name):
    data = (HERE / name).read_bytes()
    return json.loads(gzip.decompress(data) if name.endswith(".gz") else data)


def percentile(values, p):
    values = sorted(values)
    return values[min(len(values) - 1, int((len(values) - 1) * p / 100))]


def make_summary():
    paired, terminals = read(PAIRED), read(TERMINALS)
    outputs = [read("attempt2/output/" + name + ".json") for name in OUTPUT_NAMES]
    def runs(report):
        return [{"order": i, "app": row["app"], "variant": row.get("variant"),
                 "windows": row["windows"], "repetition": row["repetition"],
                 "output_mib_per_second": row["output_mib_per_second"],
                 "application_pss_bytes": row["application"]["Pss_bytes"],
                 "latency_p50_us": row["latency_p50_us"], "latency_p95_us": row["latency_p95_us"]}
                for i, row in enumerate(report["runs"], 1)]
    return {"schema": 1, "collector_source": SOURCE, "binary_source": BINARY_SOURCE,
            "paired": {"summary": paired["summary"], "runs": runs(paired)},
            "terminals": {"summary": terminals["summary"], "runs": runs(terminals)},
            "output": [{"name": name, "kind": item["runs"][0]["kind"], "mode": item["runs"][0]["mode"],
                        "payload_bytes": item["runs"][0]["payload_bytes"],
                        "samples": item["runs"][0]["output"], "median_mib_s": item["runs"][0]["median_mib_s"],
                        "idle_dsr_p50_ns": item["runs"][0]["idle_dsr_p50_ns"],
                        "idle_dsr_p95_ns": item["runs"][0]["idle_dsr_p95_ns"]}
                       for name, item in zip(OUTPUT_NAMES, outputs)],
            "counts": {"paired_runs": 12, "terminal_runs": 18, "output_groups": 6,
                       "paired_bursts": 198, "terminal_bursts": 297, "output_bursts": 18,
                       "paired_latency_values": 19800, "terminal_latency_values": 29700,
                       "output_idle_latency_values": 1200}}


def burst_csv():
    stream = io.StringIO(newline="")
    writer = csv.writer(stream, lineterminator="\n")
    writer.writerow(["phase", "run_order", "app", "variant", "kind", "mode", "windows", "repetition", "window_index", "sample_index", "bytes", "seconds", "elapsed_ns", "mib_per_second"])
    for phase, name in (("paired", PAIRED), ("terminals", TERMINALS)):
        for i, run in enumerate(read(name)["runs"], 1):
            for window in run["performance"]:
                for n, sample in enumerate(window["output"], 1):
                    writer.writerow([phase, i, run["app"], run.get("variant", ""), run["workload"], run["mode"], run["windows"], run["repetition"], window["index"], n, sample["bytes"], sample["seconds"], "", sample["mib_per_second"]])
    for i, name in enumerate(OUTPUT_NAMES, 1):
        run = read("attempt2/output/" + name + ".json")["runs"][0]
        for n, sample in enumerate(run["output"], 1):
            writer.writerow(["output", i, "ztty", "", run["kind"], run["mode"], "", "", "", n, sample["bytes"], "", sample["elapsed_ns"], sample["mib_s"]])
    return stream.getvalue()


def sample_document(summary):
    lines = ["# 全部 run 与输出样本", "", "表内数值直接取公开原始 JSON，不作舍入。run 顺序与采集顺序一致。",
             "513 个逐窗口输出 burst 在 [CSV](burst-samples.csv)；全部 50,700 个 DSR 原值保存在各原始 JSON。", ""]
    for group in ("paired", "terminals"):
        lines += ["## " + group, "", "| 顺序 | 应用/版本 | 窗口 | 轮 | MiB/s | PSS B | DSR p50 µs | DSR p95 µs |",
                  "| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |"]
        for row in summary[group]["runs"]:
            values = [row["order"], row["variant"] or row["app"], row["windows"], row["repetition"], row["output_mib_per_second"], row["application_pss_bytes"], row["latency_p50_us"], row["latency_p95_us"]]
            lines.append("| " + " | ".join(map(str, values)) + " |")
        lines.append("")
    lines += ["## output", "", "| 组 | 样本 | 字节 | elapsed_ns | MiB/s |", "| --- | ---: | ---: | ---: | ---: |"]
    for group in summary["output"]:
        for i, sample in enumerate(group["samples"], 1):
            lines.append("| " + " | ".join(map(str, [group["name"], i, sample["bytes"], sample["elapsed_ns"], sample["mib_s"]])) + " |")
    return "\n".join(lines) + "\n"


def tables(summary):
    paired = ["| 窗口 | 旧版 MiB/s，中位数 [范围] | 新版 MiB/s，中位数 [范围] | 中位数之比 | 配对比值中位数 |", "| ---: | ---: | ---: | ---: | ---: |"]
    memory = ["| 窗口 | 旧 → 新应用 PSS MiB | 旧 → 新 DSR p50 µs | 旧 → 新 DSR p95 µs |", "| ---: | ---: | ---: | ---: |"]
    for row in summary["paired"]["summary"]:
        values = [row["windows"]]
        for side in ("before", "after"):
            v = row[side]
            values.append(f'{v["median_output_mib_per_second"]:.3f} [{v["output_range_mib_per_second"][0]:.3f}, {v["output_range_mib_per_second"][1]:.3f}]')
        values += [f'{row["ratio_of_median_throughputs"]:.6f}', f'{row["median_paired_throughput_ratio"]:.6f}']
        paired.append("| " + " | ".join(map(str, values)) + " |")
        b, a = row["before"], row["after"]
        memory.append(f'| {row["windows"]} | {b["median_application_pss_bytes"]/MIB:.2f} → {a["median_application_pss_bytes"]/MIB:.2f} | {b["median_latency_p50_us"]:.4f} → {a["median_latency_p50_us"]:.4f} | {b["median_latency_p95_us"]:.3f} → {a["median_latency_p95_us"]:.3f} |')
    output = ["| 负载/模式 | 三次 MiB/s（原顺序） | 中位数 MiB/s | idle DSR p50 / p95 µs |", "| --- | --- | ---: | ---: |"]
    for row in summary["output"]:
        output.append(f'| {row["name"]} | ' + ", ".join(f'{s["mib_s"]:.3f}' for s in row["samples"]) + f' | {row["median_mib_s"]:.3f} | {row["idle_dsr_p50_ns"]/1000:.3f} / {row["idle_dsr_p95_ns"]/1000:.3f} |')
    terminals = ["| 应用 | 窗口 | 应用 PSS MiB | 应用 + 完整 X server PSS MiB | MiB/s，中位数 [范围] | DSR p50 / p95 µs |", "| --- | ---: | ---: | ---: | ---: | ---: |"]
    for row in summary["terminals"]["summary"]:
        terminals.append(f'| {row["app"]} | {row["windows"]} | {row["median_pss_bytes"]/MIB:.2f} | {row["median_app_plus_xserver_pss_bytes"]/MIB:.2f} | {row["median_output_mib_per_second"]:.3f} [{row["output_range_mib_per_second"][0]:.3f}, {row["output_range_mib_per_second"][1]:.3f}] | {row["median_latency_p50_us"]:.4f} / {row["median_latency_p95_us"]:.3f} |')
    return {"PAIRED_TABLE": "\n".join(paired), "MEMORY_TABLE": "\n".join(memory), "OUTPUT_TABLE": "\n".join(output), "TERMINALS_TABLE": "\n".join(terminals)}


def verify_runs(report, phase):
    bursts, latency_count, screen_count = 0, 0, 0
    for i, run in enumerate(report["runs"], 1):
        label = f"{phase}/{i}"
        require(run["ok"] is True and run["remaining_owned_pids"] == [] and run["host_contention"] == [], label + " completion/cleanup/guard")
        require(len(run["performance"]) == run["windows"], label + " all windows")
        require(len(run["memory_samples"]) == 3, label + " three memory snapshots")
        same_number(run["application"]["Pss_bytes"], statistics.median(x["application"]["Pss_bytes"] for x in run["memory_samples"]), label + " PSS aggregation")
        rates, latency = [], []
        for window in run["performance"]:
            integrity = window["output_integrity"]
            require(integrity["untimed"] is True and integrity["final_cursor_checked"] is True, label + " untimed CPR")
            require(integrity["payload_sha256"] == report["settings"]["output_sha256"], label + " identical payload")
            screen = integrity.get("screen")
            if run["app"] == "ztty":
                require(screen["all_text_cells_checked"] is True and (screen["rows"], screen["columns"]) == (32, 120), label + " full ztty screen")
                screen_count += 1
            else:
                require(screen is None, label + " external app oracle boundary")
            require(len(window["output"]) == 3 and len(window["latency_us"]) == 300, label + " all samples")
            for sample in window["output"]:
                require(sample["bytes"] == 1050937 and sample["seconds"] > 0, label + " burst size")
                same_number(sample["mib_per_second"], sample["bytes"] / MIB / sample["seconds"], label + " burst arithmetic")
            rate = statistics.median(sample["mib_per_second"] for sample in window["output"])
            same_number(window["median_output_mib_per_second"], rate, label + " window median")
            rates.append(rate)
            latency.extend(window["latency_us"])
            bursts += 3
        same_number(run["output_mib_per_second"], statistics.median(rates), label + " run median")
        same_number(run["latency_p50_us"], statistics.median(latency), label + " DSR p50")
        same_number(run["latency_p95_us"], percentile(latency, 95), label + " DSR p95")
        latency_count += len(latency)
        require(all(sample["application"]["Swap_bytes"] == 0 for sample in run["memory_samples"]), label + " sampled application swap")
        if phase == "paired":
            expected = OLD_BINARY if run["variant"] == "before" else BINARY
            require(run["binary_sha256_before"] == run["binary_sha256_after"] == expected, label + " unchanged ELF")
            if run["variant"] == "before":
                require(run["renderer_config_emitted"] is False and all(x["actual"] is None for x in run["renderers"]), label + " unknown old renderer preserved")
            else:
                require(all(x["actual"] == "software" and "llvmpipe" in x["driver"] for x in run["renderers"]), label + " observed Auto fallback")
        elif run["app"] == "ztty":
            require(all(x["requested"] == x["actual"] == "gpu" and "llvmpipe" in x["driver"] for x in run["renderers"]), label + " explicit GPU on llvmpipe")
    return bursts, latency_count, screen_count


def main():
    index = read("redaction.json")
    rows = {x["public_file"]: x for x in index["files"]}
    require(len(rows) == len(index["files"]) == 86, "86 original attachments")
    full = list(rows.values()) + index["derived_files"]
    expected = {x["public_file"] for x in full} | {"redaction.json"}
    require({str(p.relative_to(HERE)) for p in HERE.rglob("*") if p.is_file()} == expected and len(expected) == 93, "complete 93-file bundle")
    for row in full:
        data = (HERE / row["public_file"]).read_bytes()
        require(sha(data) == row["public_sha256"] and len(data) == row["public_bytes"], "file hash: " + row["public_file"])
        if row.get("storage") == "gzip":
            payload = gzip.decompress(data)
            require(sha(payload) == row["public_payload_sha256"] and len(payload) == row["public_payload_bytes"], "lossless payload: " + row["public_file"])
    original = lambda path: rows[path]["original_sha256"]
    by_source = {x["source_relative_path"]: x for x in rows.values()}
    for check in index["canonical_runs_validation"]:
        row = by_source[check["source_relative_path"]]
        raw = read(row["public_file"])
        options = {"sort_keys": True}
        if check["canonical_style"] == "compact":
            options["separators"] = (",", ":")
        original_record = raw.get("raw_runs_sha256", raw.get("derived", {}).get("source_runs_sha256"))
        require(check["original_matches_record"] is True and check["original_file_sha256"] == row["original_sha256"] and original_record == check["original_canonical_runs_sha256"], "original canonical link")
        require(sha(json.dumps(raw["runs"], **options).encode()) == check["public_canonical_runs_sha256"], "public canonical runs")
    eq = read("production-equivalence.json")
    require(eq == index["production_equivalence"] and eq["changed_paths"] == [] and eq["binary_source"] == BINARY_SOURCE and eq["collector_source"] == SOURCE, "fixed production inputs equivalent")

    cohort, launch = read("attempt2/cohort-receipt.json"), read("attempt2/launch.json")
    require(cohort["ok"] is True and cohort["not_started"] == [] and cohort["launch_sha256"] == original("attempt2/launch.json"), "successful cohort launch")
    require(launch["collector_source"] == SOURCE and launch["binary_source"] == BINARY_SOURCE and launch["wrapper_sha256"] == original("collector/run-final-performance.py"), "collector versus binary source")
    for phase, count in (("throughput", 1), ("output", 6), ("terminals", 1)):
        receipt = read("attempt2/" + phase + "/receipt.json")
        summary = next(x for x in cohort["phases"] if x["phase"] == phase)
        require(summary["exit_code"] == 0 and summary["receipt_sha256"] == original("attempt2/" + phase + "/receipt.json"), "phase closure: " + phase)
        require(receipt["ok"] is True and receipt["status"] == "complete" and len(receipt["runs"]) == count, "phase runs: " + phase)
        require(receipt["collector_source"] == SOURCE and receipt["binary_source"] == BINARY_SOURCE and receipt["binary_sha256"] == BINARY, "phase inputs: " + phase)
        for name, digest in receipt["helper_sha256"].items():
            require(original("collector/tools/" + name) == digest, "fixed helper: " + name)
        for timing in ("execution_before", "execution_after"):
            execution = receipt[timing]
            require(execution["nice"] == -4 and execution["affinity"] == list(range(24)), "actual execution policy")
            require(all(x["cpu.max"] == "max 100000" for x in execution["ancestor_cpu_limits"] if "cpu.max" in x), "recorded parent CPU limits")
        for run in receipt["runs"]:
            name = "attempt2/" + phase + "/" + run["name"] + ".json"
            if name not in rows:
                name += ".gz"
            require(run["exit_code"] == 0 and run["collector_ok"] is True and run["before_reasons"] == run["after_reasons"] == [], "phase host guard and result")
            require(run["result_sha256"] == original(name), "phase result original hash")

    paired, terminals = read(PAIRED), read(TERMINALS)
    require(paired["ok"] is True and len(paired["runs"]) == 12 and terminals["ok"] is True and len(terminals["runs"]) == 18, "complete 12/18 matrix")
    require(paired["binaries"]["before"]["source_revision"] == OLD_SOURCE and paired["binaries"]["before"]["sha256"] == OLD_BINARY and paired["binaries"]["after"]["source_revision"] == BINARY_SOURCE and paired["binaries"]["after"]["sha256"] == BINARY, "paired immutable binaries")
    require(paired["source_sha256"] == paired["source_sha256_after"] and paired["git_before"] == paired["git_after"], "paired source unchanged")
    require([(x["repetition"], x["windows"], x["variant"]) for x in paired["runs"]] == [(x["repetition"], x["windows"], x["variant"]) for x in paired["run_order"]], "paired original ordering")
    require(verify_runs(paired, "paired") == (198, 19800, 66), "paired sample/oracle totals")
    require(verify_runs(terminals, "terminals") == (297, 29700, 33), "terminal sample/oracle totals")
    for row in paired["summary"]:
        rates = {}
        for side in ("before", "after"):
            selected = [r for r in paired["runs"] if r["windows"] == row["windows"] and r["variant"] == side]
            require(len(selected) == 3, "three paired repetitions")
            rates[side] = [r["output_mib_per_second"] for r in selected]
            require(rates[side] == row[side]["output_mib_per_second"], "paired rates retain order")
            for key, field in (("median_output_mib_per_second", "output_mib_per_second"), ("median_latency_p50_us", "latency_p50_us"), ("median_latency_p95_us", "latency_p95_us")):
                same_number(row[side][key], statistics.median(r[field] for r in selected), "paired summary " + key)
            same_number(row[side]["median_application_pss_bytes"], statistics.median(r["application"]["Pss_bytes"] for r in selected), "paired PSS summary")
        ratios = [a / b for a, b in zip(rates["after"], rates["before"])]
        require(ratios == row["paired_throughput_ratios"], "all paired ratios")
        same_number(row["ratio_of_median_throughputs"], statistics.median(rates["after"]) / statistics.median(rates["before"]), "ratio of medians")
        same_number(row["median_paired_throughput_ratio"], statistics.median(ratios), "median paired ratio")
    for row in terminals["summary"]:
        selected = [r for r in terminals["runs"] if r["app"] == row["app"] and r["windows"] == row["windows"]]
        require(len(selected) == row["successful_runs"] == 3, "three terminal repetitions")
        for key, values in (("median_pss_bytes", [r["application"]["Pss_bytes"] for r in selected]), ("median_output_mib_per_second", [r["output_mib_per_second"] for r in selected]), ("median_latency_p50_us", [r["latency_p50_us"] for r in selected]), ("median_latency_p95_us", [r["latency_p95_us"] for r in selected]), ("median_app_plus_xserver_pss_bytes", [r["application"]["Pss_bytes"] + r["xserver"]["after"]["Pss_bytes"] for r in selected])):
            same_number(row[key], statistics.median(values), "terminal summary " + key)
        require(row["max_sampled_application_swap_bytes"] == row["max_sampled_app_plus_xserver_swap_bytes"] == 0, "terminal recorded swap boundary")

    for name in OUTPUT_NAMES:
        report = read("attempt2/output/" + name + ".json")
        run = report["runs"][0]
        require(report["ok"] is True and report["binary_sha256"] == BINARY and report["binary_source_commit"] == BINARY_SOURCE and report["collector_source_commit"] == SOURCE, "output source")
        require(run["ok"] is True and run["owned_remaining"] == [] and run["all_visible_text_checked"] is True, "output checks and cleanup")
        require(run["affinity"] == {"broker": [23], "writer": [22], "xserver": [21]}, "separate output affinity")
        require(len(run["output"]) == 3 and len(run["idle_dsr_latency_ns"]) == 200, "output samples")
        for sample in run["output"]:
            same_number(sample["mib_s"], sample["bytes"] / MIB / (sample["elapsed_ns"] / 1e9), "output burst arithmetic")
        same_number(run["median_mib_s"], statistics.median(s["mib_s"] for s in run["output"]), "output median")
        latency = sorted(run["idle_dsr_latency_ns"])
        require(run["idle_dsr_p50_ns"] == latency[100] and run["idle_dsr_p95_ns"] == latency[190], "output percentile definition")
        if run["mode"] == "window":
            require(run["renderer_before"] == run["renderer_after"] and run["renderer_before"]["actual"] == "software", "actual software before/after")

    first = read("attempt1/throughput/paired.json")
    failed = read("attempt1/cohort-receipt.json")
    diagnosis = read("collector/legacy-baseline-compatibility.json")
    require(first["ok"] is False and len(first["runs"]) == 1 and first["runs"][0]["ok"] is False and "performance" not in first["runs"][0], "failed launch is not timing sample")
    require("unknown appearance option (renderer = auto)" in first["runs"][0]["terminal_stderr"], "old config rejection retained")
    require(failed["ok"] is False and len(failed["phases"]) == 1 and failed["phases"][0]["exit_code"] == 1, "initial stopped cohort")
    require(diagnosis["completed_timing_samples"] == 0 and diagnosis["after_binary_started"] is False, "initial timing boundary")
    for name, digest in diagnosis["original_files"].items():
        require(original("attempt1/throughput/" + name) == digest, "initial original hash")
    for name, digest in first["source_sha256"].items():
        require(original(index["attempt1_helpers"][name]) == digest, "old collector source retained")
    diagnostic = read("diagnostic/diagnostic.json")
    require(diagnostic["ok"] is True and diagnostic["before"] == diagnostic["after"], "diagnostic stable input")
    require(diagnostic["binary_source"] == BINARY_SOURCE and diagnostic["before"]["binary_sha256"] == BINARY and diagnostic["before"]["source"] == "6885cafdf05903ba5a4a9d3e56121e3a3684bc2e", "separate diagnostic identity")
    require(diagnostic["script_sha256"] == original("diagnostic/diagnose.py") and len(diagnostic["runs"]) == 2, "separate diagnostic script")
    for record, pss in zip(diagnostic["runs"], (112327680, 12172288)):
        require(record["ok"] is True and record["remaining_owned_pids"] == [] and record["monitor_errors"] == [], "diagnostic cleanup")
        require(record["application"]["Pss_bytes"] == pss and record["application"]["Swap_bytes"] == 0, "diagnostic PSS values")
        require(original("diagnostic/" + record["raw"]) == record["raw_sha256"], "diagnostic raw identity")
        require(len(record["mapping_snapshots"]) == 4, "both owned processes at two diagnostic times")
        maps = ""
        for snapshot in record["mapping_snapshots"]:
            for kind in ("maps", "rollup"):
                require(original("diagnostic/" + snapshot[kind]) == snapshot[kind + "_sha256"], "diagnostic snapshot identity")
            maps += (HERE / "diagnostic" / snapshot["maps"]).read_text().lower()
        require(all((name in maps) == (record["mode"] == "auto") for name in ("llvm", "gallium", "nvidia")), "observed diagnostic library mappings")
    require(diagnostic["runs"][0]["application"]["Pss_bytes"] - diagnostic["runs"][1]["application"]["Pss_bytes"] == 100155392, "diagnostic PSS difference")
    summary = make_summary()
    require(read("summary.json") == summary, "all summary values match raw")
    require((HERE / "SAMPLES.md").read_text() == sample_document(summary), "all run tables match raw")
    require((HERE / "burst-samples.csv").read_text() == burst_csv(), "all 513 burst rows match raw")
    require(len(list(csv.DictReader(io.StringIO(burst_csv())))) == 513, "complete burst CSV")
    text = (HERE / "README.md").read_text()
    require(all(value in text for value in tables(summary).values()), "report tables match raw")
    print(json.dumps({"ok": True, "files": 93, "assertions": len(CHECKS), "collector_source": SOURCE,
                      "binary_source": BINARY_SOURCE, "paired_runs": 12, "terminal_runs": 18,
                      "output_groups": 6, "output_bursts": 513, "paired_terminal_latency_values": 49500,
                      "output_idle_latency_values": 1200, "original_failure_preserved": True,
                      "product_commands_executed": 0}, indent=2))


if __name__ == "__main__":
    main()
