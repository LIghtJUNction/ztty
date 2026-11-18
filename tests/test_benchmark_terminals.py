#!/usr/bin/env python3
"""Accounting and publication regressions; all numbers below are test fixtures."""
import base64
import copy
import hashlib
import json
from pathlib import Path
import re
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import benchmark_terminals as bench
import measure_multiwindow as multi


def memory(pid=7):
    return {"pid": pid, **{field: 1024 for field in bench.FIELDS}}


def report(counts=(10,), modes=("shared",), workloads=("idle",), repetitions=3):
    runs = []
    for count in counts:
        for mode in modes:
            for workload in workloads:
                for app in bench.APPS:
                    for repetition in range(1, repetitions + 1):
                        runs.append({
                            "ok": True, "windows": count, "mode": mode, "workload": workload,
                            "app": app, "repetition": repetition,
                            "application": {"Rss_bytes": count * 4096, "Pss_bytes": count * 2048,
                                            "Private_bytes": count * 1024, "Swap_bytes": count * 64},
                            "xserver": {"after": {"Pss_bytes": 8192, "Swap_bytes": 128},
                                        "delta": {"Pss_bytes": -1024}},
                            "memory_samples": [{"application": {"Swap_bytes": count * 32}},
                                               {"application": {"Swap_bytes": count * 64}},
                                               {"application": {"Swap_bytes": count * 96}}],
                            "latency_p50_us": 5, "latency_p95_us": 9,
                            "output_mib_per_second": 15, "gui_process_count": 1,
                            "application_process_counts": [1, 1, 1],
                        })
    return {"schema": 2, "ok": True, "timestamp": "2026-11-18T00:00:00Z", "runs": runs,
            "settings": {"counts": list(counts), "modes": list(modes),
                         "workloads": list(workloads), "repetitions": repetitions,
                         "memory_settle_seconds": 1.0},
            "binaries": {app: {"version": {"stdout": app + " fixture"}} for app in bench.APPS}}


class AccountingTests(unittest.TestCase):
    def test_paired_window_default_and_explicit_matrix_count_are_distinct(self):
        # The paired collector changes WINDOWS between cases; matrix callers
        # supply count explicitly. A captured default would open ten windows
        # for the paired one-window case.
        with patch.dict(sys.modules, {"Xlib": SimpleNamespace(X=None, display=None)}), \
                patch.object(bench, "WINDOWS", 1), \
                patch.object(bench, "host_pressure", return_value={}), \
                patch.object(bench, "comparison_pressure_reasons", return_value=["busy fixture"]):
            self.assertEqual(bench.measure("ztty", Path("/fixture"), 1, 0)["windows"], 1)
            self.assertEqual(bench.measure("ztty", Path("/fixture"), 1, 0, count=2)["windows"], 2)

    def test_shared_pages_are_counted_once_per_process(self):
        samples = [memory(7), memory(8)]
        self.assertEqual(bench.totals(samples)["Pss_bytes"], 2048)
        self.assertEqual(bench.totals(samples)["Private_bytes"], 4096)
        for module in (bench, multi):
            with self.subTest(module=module.__name__), self.assertRaises(ValueError):
                module.totals([memory(7), memory(7)])

    def test_missing_pss_is_not_zero(self):
        sample = memory()
        del sample["Pss_bytes"]
        for module in (bench, multi):
            with self.subTest(module=module.__name__), self.assertRaises(ValueError):
                module.totals([sample])

    def test_missing_optional_kernel_counter_remains_unavailable(self):
        sample = memory()
        del sample["Pss_Shmem_bytes"]
        self.assertIsNone(bench.totals([memory(8), sample])["Pss_Shmem_bytes"])

    def test_pid_reuse_invalidates_cpu_measurement(self):
        with patch.object(bench, "process_stat", return_value={"start_ticks": 2}):
            self.assertFalse(bench.idle_cpu({7: 1})["available"])

    def test_exited_process_is_unavailable_not_idle(self):
        with patch.object(bench, "process_stat", side_effect=ProcessLookupError):
            self.assertFalse(bench.idle_cpu({7: 1})["available"])


class PublicationTests(unittest.TestCase):
    def test_server_pss_and_swap_are_paired_before_aggregation(self):
        data = report()
        selected = [run for run in data["runs"] if run["app"] == "ztty"]
        for run, app_pss, server_pss, server_swap, app_swaps in zip(
                selected, (100, 300, 1000), (900, 100, 50), (8, 0, 100),
                ((1, 40), (90, 2), (5, 6))):
            run["application"]["Pss_bytes"] = app_pss
            run["xserver"]["after"] = {"Pss_bytes": server_pss, "Swap_bytes": server_swap}
            run["memory_samples"] = [{"application": {"Swap_bytes": value}} for value in app_swaps]
        row = next(row for row in bench.summarize(data["runs"]) if row["app"] == "ztty")
        self.assertEqual(row["median_app_plus_xserver_pss_bytes"], 1000)
        self.assertEqual(row["app_plus_xserver_pss_range_bytes"], [400, 1050])
        self.assertEqual(row["sampled_application_swap_range_bytes"], [1, 90])
        self.assertEqual(row["sampled_app_plus_xserver_swap_range_bytes"], [2, 106])
        self.assertEqual(row["max_sampled_app_plus_xserver_swap_bytes"], 106)
        self.assertEqual(row["xserver_delta_pss_range_bytes"], [-1024, -1024])

    def test_missing_server_pss_is_unavailable_and_cannot_draw_overview(self):
        data = report()
        del data["runs"][0]["xserver"]["after"]["Pss_bytes"]
        row = next(row for row in bench.summarize(data["runs"]) if row["app"] == "ztty")
        self.assertIsNone(row["median_app_plus_xserver_pss_bytes"])
        self.assertIsNone(row["app_plus_xserver_pss_range_bytes"])
        with self.assertRaisesRegex(ValueError, "unavailable"):
            bench.svg_document(data)
        ET.fromstring(bench.scaling_svg_document(data))

    def test_missing_swap_snapshot_does_not_become_zero_or_partial_maximum(self):
        for remove_snapshots in (False, True):
            data = report()
            if remove_snapshots:
                del data["runs"][0]["memory_samples"]
            else:
                del data["runs"][0]["memory_samples"][0]["application"]["Swap_bytes"]
            row = next(row for row in bench.summarize(data["runs"]) if row["app"] == "ztty")
            self.assertIsNone(row["max_sampled_application_swap_bytes"])
            self.assertIsNone(row["max_sampled_app_plus_xserver_swap_bytes"])
            self.assertIsNone(row["sampled_application_swap_range_bytes"])
            self.assertIn("unavailable", bench.scaling_svg_document(data))

    def test_invalid_optional_counters_are_rejected(self):
        for value in (-1, float("nan"), float("inf"), "unknown"):
            data = report()
            data["runs"][0]["memory_samples"][0]["application"]["Swap_bytes"] = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                bench.summarize(data["runs"])

    def test_refresh_summary_preserves_collection_identity_and_all_raw_matrix_cells(self):
        data = report(counts=(1, 10), modes=bench.MODES, repetitions=1)
        data.update({"script_sha256": "original-collector", "git": {"head": "original-source"},
                     "source_sha256": {"isolated_xvfb.py": "original-display-guard"}})
        before = copy.deepcopy(data)
        bench.refresh_summary(data)
        self.assertEqual(data["runs"], before["runs"])
        for key in ("timestamp", "script_sha256", "source_sha256", "git", "settings", "binaries"):
            self.assertEqual(data[key], before[key])
        self.assertEqual(len(data["summary"]), 12)
        self.assertEqual(data["derived"]["source_runs_sha256"],
                         hashlib.sha256(json.dumps(data["runs"], sort_keys=True).encode()).hexdigest())
        self.assertEqual(data["derived"]["renderer_script_sha256"], bench.sha256(bench.__file__))
        self.assertIn("across 1 runs", data["derived"]["combined_pss_formula"])
        self.assertIn("Resident PSS excludes swap", bench.scaling_svg_document(data))

    def test_each_count_mode_workload_has_its_own_aggregate(self):
        data = report(counts=(1, 5, 10, 20), modes=bench.MODES, workloads=bench.WORKLOADS)
        bench.validate_report(data)
        rows = bench.summarize(data["runs"])
        self.assertEqual(len(rows), 4 * 2 * 4 * 3)
        self.assertTrue(all(row["median_pss_bytes"] == row["windows"] * 2048 for row in rows))

    def test_duplicate_repetition_does_not_count_as_a_fresh_run(self):
        data = report()
        data["runs"][1]["repetition"] = 1
        with self.assertRaises(ValueError):
            bench.validate_report(data)

    def test_missing_matrix_cell_is_rejected(self):
        data = report(counts=(1, 10))
        data["runs"] = [run for run in data["runs"] if run["windows"] == 10]
        with self.assertRaises(ValueError):
            bench.svg_document(data)

    def test_failed_run_is_rejected_even_if_ok_flag_was_set(self):
        data = report()
        data["runs"][0]["ok"] = False
        with self.assertRaises(ValueError):
            bench.scaling_svg_document(data)

    def test_recorded_failure_or_teardown_error_prevents_publication(self):
        for failure in ({"error": "measurement failed"}, {"cleanup_error": "teardown failed"},
                        {"remaining_owned_pids": [999]}):
            data = report()
            data["runs"][0].update(failure)
            with self.subTest(failure=failure), self.assertRaises(ValueError):
                bench.validate_report(data)

    def test_nonfinite_measurements_are_not_published(self):
        for value in (float("nan"), float("inf"), None, 0):
            data = report()
            data["runs"][0]["application"]["Pss_bytes"] = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                bench.scaling_svg_document(data)

    def test_overview_uses_only_ten_shared_idle_windows(self):
        data = report(counts=(1, 10), modes=bench.MODES, workloads=("idle", "history"))
        self.assertTrue(bench.overview_available(data))
        svg = bench.svg_document(data, Path("reports/<run&>.json"))
        tree = ET.fromstring(svg)
        self.assertIn("10 windows. Shared instances.", svg)
        self.assertIn("reports/&lt;run&amp;&gt;.json", svg)
        self.assertIn("reports/<run&>.json", "".join(tree.itertext()))
        self.assertNotIn("docs/benchmarks/terminals-10-windows.json", svg)
        self.assertIn("docs/benchmarks/terminals-10-windows.json", bench.svg_document(data))
        ET.fromstring(bench.scaling_svg_document(data))
        # A reviewed dated graph must not break the next normal publication.
        start, end = "<!-- terminal-benchmark:start -->", "<!-- terminal-benchmark:end -->"
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            for name in ("README.md", "README.zh-CN.md"):
                (temporary / name).write_text(
                    f"# ztty\n\n{start}\n![Dated](guarded-dated.svg)\n{end}\n\nKeep this project text.\n")
            with patch.object(bench, "ROOT", temporary):
                bench.update_readmes(3)
            for name in ("README.md", "README.zh-CN.md"):
                text = (temporary / name).read_text()
                self.assertEqual(text.count(start), 1)
                self.assertEqual(text.count(end), 1)
                self.assertIn("docs/benchmarks/terminals-10-windows.svg", text)
                self.assertIn("docs/benchmarks/terminals-10-windows.json", text)
                self.assertNotIn("guarded-dated.svg", text)
                self.assertTrue(text.endswith("\n\nKeep this project text.\n"))

    def test_partial_smoke_dataset_never_publishes_ten_window_claim(self):
        data = report(counts=(1,), repetitions=1)
        self.assertFalse(bench.overview_available(data))
        with self.assertRaises(ValueError):
            bench.svg_document(data)
        ET.fromstring(bench.scaling_svg_document(data))

    def test_loaded_host_validation_is_not_a_readme_comparison(self):
        data = report()
        data["purpose"] = "smoke"
        self.assertFalse(bench.overview_available(data))
        with self.assertRaises(ValueError):
            bench.svg_document(data)
        self.assertIn("validation run", bench.scaling_svg_document(data))

    def test_legacy_baseline_remains_renderable(self):
        data = report()
        data["schema"] = 1
        data["settings"] = {"windows": 10, "repetitions": 3, "memory_settle_seconds": 1.0}
        for run in data["runs"]:
            del run["mode"]
            del run["workload"]
        ET.fromstring(bench.svg_document(data))

    def test_legacy_busy_host_context_stays_visible_in_overview(self):
        data = report()
        data["machine"] = {"logical_cpus": 16}
        for run in data["runs"]:
            run["load_average_at_start"] = [32, 30, 28]
        svg = bench.svg_document(data)
        self.assertIn("32.0–32.0", svg)
        self.assertIn("busy host", svg)


class LaunchAndWorkloadTests(unittest.TestCase):
    def test_independent_modes_do_not_reuse_gui_process(self):
        for app in ("kitty", "ghostty"):
            shared = bench.terminal_command(app, Path("/terminal"), Path("/fixture"), "run", "shared")
            independent = bench.terminal_command(app, Path("/terminal"), Path("/fixture"), "run", "independent")
            if app == "kitty":
                self.assertIn("--single-instance", shared)
                self.assertNotIn("--single-instance", independent)
            else:
                self.assertIn("--gtk-single-instance=true", shared)
                self.assertIn("--gtk-single-instance=false", independent)

    def test_graphics_transfer_has_valid_bounded_chunks(self):
        frames = re.findall(rb"\x1b_G([^;]+);([^\x1b]*)\x1b\\", bench.graphics_payload())
        self.assertGreater(len(frames), 1)
        self.assertTrue(all(len(payload) <= 4096 for _, payload in frames))
        self.assertTrue(all(header.endswith(b"m=1") for header, _ in frames[:-1]))
        self.assertTrue(frames[-1][0].endswith(b"m=0"))
        pixels = base64.b64decode(b"".join(payload for _, payload in frames), validate=True)
        self.assertEqual(len(pixels), 256 * 256 * 4)
        self.assertEqual(pixels[:4], b"\x00\x00\x00\xff")

    def test_graphics_workload_requires_successful_protocol_response(self):
        with patch.object(bench, "exchange", side_effect=AssertionError("missing OK")):
            with self.assertRaises(AssertionError):
                bench.prepare_workload("images")


if __name__ == "__main__":
    unittest.main()
