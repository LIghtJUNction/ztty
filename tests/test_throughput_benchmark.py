#!/usr/bin/env python3
"""Untimed output-integrity and paired-publication regressions (fixture timings)."""
import argparse
import copy
import hashlib
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import benchmark_terminals as bench
import benchmark_throughput as paired

PAYLOAD_SHA256 = "76376cc29e434d25d2fd58b984b231d6ad3efae2c7a9b0d8c87e762d12427f29"
PAYLOAD_BYTES = 1_050_937


def fixture_report():
    report = {
        "settings": {"counts": [1, 10], "rounds": 3, "output_sha256": PAYLOAD_SHA256,
                     "output_bytes_per_sample": PAYLOAD_BYTES},
        "binaries": {variant: {"sha256": variant + "-fixture"} for variant in paired.VARIANTS},
        "runs": [],
    }
    for repetition, count, variant in paired.run_order((1, 10), 3):
        rate = (10 + repetition) * (2 if variant == "after" else 1)
        seconds = PAYLOAD_BYTES / (1024 * 1024) / rate
        report["runs"].append({
            "windows": count, "repetition": repetition, "variant": variant, "ok": True,
            "remaining_owned_pids": [], "binary_sha256_before": variant + "-fixture",
            "binary_sha256_after": variant + "-fixture",
            "output_mib_per_second": rate, "latency_p50_us": 10, "latency_p95_us": 20,
            "application": {"Pss_bytes": count * 1024},
            "gui_owner_pids": [42], "gui_process_count": 1,
            "windows_observed": [{"id": index, "title": f"fixture-{index}", "pid": 42}
                                 for index in range(1, count + 1)],
            "performance": [{
                "index": index,
                "median_output_mib_per_second": rate,
                "output_integrity": {
                    "final_cursor_checked": True, "payload_sha256": PAYLOAD_SHA256,
                    "screen": {"all_text_cells_checked": True},
                },
                "output": [{"bytes": PAYLOAD_BYTES, "seconds": seconds,
                            "mib_per_second": rate} for _ in range(bench.OUTPUT_SAMPLES)],
            } for index in range(1, count + 1)],
        })
    return report


class OutputIntegrityTests(unittest.TestCase):
    def test_original_timed_workload_and_completion_barrier_are_unchanged(self):
        payload = bench.output_payload()
        self.assertEqual(len(payload), PAYLOAD_BYTES)
        self.assertEqual(hashlib.sha256(payload).hexdigest(), PAYLOAD_SHA256)
        self.assertEqual(bench.BARRIER, b"\x1b[H\x1b[6n")
        self.assertEqual(bench.BARRIER_REPLY, b"\x1b[1;1R")
        self.assertEqual(bench.OUTPUT_CHECK, b"\x1b[6n")
        self.assertEqual(bench.OUTPUT_CHECK_REPLY, b"\x1b[32;120R")

    def test_expected_last_frame_contains_every_row_in_order(self):
        rows = bench.output_screen(bench.output_payload()).splitlines()
        self.assertEqual(len(rows), 32)
        self.assertEqual(rows[0], "frame 0250 row 01 " + "R" * 101)
        self.assertEqual(rows[-1], "frame 0250 row 32 " + "W" * 101)
        self.assertTrue(all(len(row) == 119 for row in rows))
        damaged = bench.output_payload().replace(b"\x1b[32;1H", b"\x1b[31;1H")
        with self.assertRaises(ValueError):
            bench.output_screen(damaged)

    def test_final_screen_cannot_pass_with_missing_printed_cells(self):
        expected = bench.output_screen(bench.output_payload())
        listing = {"sessions": [{"title": "test-window", "uri": "ztty://local/test"}]}
        good = {"cols": 120, "rows": 32, "screen_truncated": False, "screen": expected}
        with patch.object(bench, "broker_value", side_effect=[listing, good]):
            check = bench.verify_ztty_screen("@private-test-only", "test-window", expected)
        self.assertTrue(check["all_text_cells_checked"])
        self.assertEqual(check["screen_utf8_sha256"], hashlib.sha256(expected.encode()).hexdigest())
        for value in (
            dict(good, screen=expected.replace("RRR", "R R", 1)),
            dict(good, screen_truncated=True),
            dict(good, cols=119),
        ):
            with self.subTest(value=value["cols"]), patch.object(
                    bench, "broker_value", side_effect=[listing, value]):
                with self.assertRaises(AssertionError):
                    bench.verify_ztty_screen("@private-test-only", "test-window", expected)

    def test_session_selection_requires_exact_unique_title(self):
        expected = bench.output_screen(bench.output_payload())
        for sessions in ([], [{"title": "other", "uri": "unused"}],
                         [{"title": "wanted", "uri": "one"}, {"title": "wanted", "uri": "two"}]):
            with patch.object(bench, "broker_value", return_value={"sessions": sessions}):
                with self.assertRaises(AssertionError):
                    bench.verify_ztty_screen("@private-test-only", "wanted", expected)


class PairedCollectorTests(unittest.TestCase):
    def test_ab_ba_order_retains_every_pair(self):
        plan = list(paired.run_order((1, 10), 3))
        self.assertEqual(len(plan), 12)
        for repetition in (1, 2, 3):
            selected = [variant for r, count, variant in plan if r == repetition and count == 10]
            self.assertEqual(selected, ["after", "before"] if repetition == 2 else ["before", "after"])
        self.assertEqual(paired.parse_counts("1,10"), (1, 10))
        for value in ("", "2", "1,1", "1,20", "one"):
            with self.subTest(value=value), self.assertRaises(argparse.ArgumentTypeError):
                paired.parse_counts(value)

    def test_summary_keeps_raw_samples_and_reports_paired_ratios(self):
        report = fixture_report()
        original = copy.deepcopy(report)
        rows = paired.summarize(report)
        self.assertEqual(report, original)
        self.assertEqual([row["windows"] for row in rows], [1, 10])
        for row in rows:
            self.assertEqual(row["before"]["output_mib_per_second"], [11, 12, 13])
            self.assertEqual(row["after"]["output_mib_per_second"], [22, 24, 26])
            self.assertEqual(row["paired_throughput_ratios"], [2, 2, 2])
            self.assertEqual(row["ratio_of_median_throughputs"], 2)

    def test_missing_or_duplicate_pair_is_not_a_completed_comparison(self):
        for duplicate in (False, True):
            report = fixture_report()
            if duplicate:
                report["runs"][1] = copy.deepcopy(report["runs"][0])
            else:
                report["runs"].pop()
            with self.subTest(duplicate=duplicate), self.assertRaises(ValueError):
                paired.summarize(report)

    def test_incomplete_or_changed_executables_cannot_be_published(self):
        for change in ({"ok": False}, {"remaining_owned_pids": [999]},
                       {"binary_sha256_after": "changed"}, {"binary_sha256_before": "wrong"}):
            report = fixture_report()
            report["runs"][0].update(change)
            with self.subTest(change=change), self.assertRaises(ValueError):
                paired.summarize(report)

    def test_failed_cursor_or_full_screen_check_cannot_be_published(self):
        for change in ({"final_cursor_checked": False},
                       {"screen": {"all_text_cells_checked": False}},
                       {"payload_sha256": "another-workload"}):
            report = fixture_report()
            report["runs"][0]["performance"][0]["output_integrity"].update(change)
            with self.subTest(change=change), self.assertRaises(ValueError):
                paired.summarize(report)

    def test_summary_rejects_rates_inconsistent_with_raw_time_and_bytes(self):
        for level in ("sample", "window", "run"):
            report = fixture_report()
            run = report["runs"][0]
            if level == "sample":
                run["performance"][0]["output"][0]["mib_per_second"] = 1e9
            elif level == "window":
                run["performance"][0]["median_output_mib_per_second"] = 1e9
            else:
                run["output_mib_per_second"] = 1e9
            with self.subTest(level=level), self.assertRaises(ValueError):
                paired.summarize(report)

    def test_cleanup_and_mapped_window_evidence_must_be_explicit_and_consistent(self):
        for field, replacement in (
            ("remaining_owned_pids", None),
            ("gui_owner_pids", []),
            ("gui_owner_pids", [99]),
            ("gui_process_count", 2),
            ("windows_observed", []),
        ):
            report = fixture_report()
            report["runs"][0][field] = replacement
            with self.subTest(field=field, replacement=replacement), self.assertRaises(ValueError):
                paired.summarize(report)
        report = fixture_report()
        del report["runs"][0]["remaining_owned_pids"]
        with self.assertRaises(ValueError):
            paired.summarize(report)

    def test_duplicate_window_or_performance_identity_cannot_replace_a_missing_window(self):
        for target in ("performance", "windows_observed"):
            report = fixture_report()
            run = next(run for run in report["runs"] if run["windows"] == 10)
            run[target][1] = copy.deepcopy(run[target][0])
            with self.subTest(target=target), self.assertRaises(ValueError):
                paired.summarize(report)

    def test_missing_samples_or_invalid_measurements_cannot_be_published(self):
        for field, value in (("output_mib_per_second", 0), ("latency_p50_us", float("nan")),
                             ("latency_p95_us", float("inf"))):
            report = fixture_report()
            report["runs"][0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                paired.summarize(report)
        for remove_window in (False, True):
            report = fixture_report()
            if remove_window:
                report["runs"][0]["performance"].pop()
            else:
                report["runs"][0]["performance"][0]["output"].pop()
            with self.subTest(remove_window=remove_window), self.assertRaises(ValueError):
                paired.summarize(report)


if __name__ == "__main__":
    unittest.main()
