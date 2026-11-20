#!/usr/bin/env python3
"""Sequential shared-Wayland acceptance for fixed source, two ELFs, and a prebuilt IME peer."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import signal
import subprocess
import time

from identities import (binary_identity, harness_identities, plugin_identities,
                        source_identity, tool_identities)
from owned_process import enable_subreaper, run_owned
from plan import DEFAULT_SWAY, build_plan

HERE = Path(__file__).resolve().parent


def write_json(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    temporary.replace(path)


def read_receipt(step):
    path = Path(step["report"])
    try:
        value = json.loads(path.read_text())
    except (OSError, ValueError) as error:
        return {"ok": False, "error": f"{type(error).__name__}: {error}"}
    flag = value.get("ok", value.get("passed"))
    checks = value.get("checks")
    count = len(checks) if isinstance(checks, list) else None
    expected = step["expected_checks"]
    names = [check.get("name") for check in checks if isinstance(check, dict)] if isinstance(checks, list) else []
    missing = sorted(set(step["required_check_names"]) - set(names))
    strict_ok = (not step["strict_checks"] or (isinstance(checks, list)
                 and all(isinstance(check, dict) and check.get("passed") is True for check in checks)))
    return {"ok": flag is True and (expected is None or count == expected) and not missing and strict_ok,
            "reported_success": flag, "checks": count, "expected_checks": expected,
            "missing_required_checks": missing, "strict_check_results": strict_ok,
            "error": value.get("error"), "path": str(path)}


def compact_log(raw, destination):
    result = subprocess.run(["rtk", "log", str(raw)], capture_output=True, text=True)
    destination.write_text(result.stdout + result.stderr)
    lines = result.stdout.splitlines()
    if lines:
        print("\n".join(line if len(line) <= 300 else line[:297] + "..."
                        for line in lines[-12:]), flush=True)
    return {"command": ["rtk", "log", str(raw)], "exit_code": result.returncode,
            "path": str(destination)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--sha256")
    parser.add_argument("--source-sha")
    parser.add_argument("--no-wayland-binary", type=Path)
    parser.add_argument("--no-wayland-sha256")
    parser.add_argument("--ime-peer", type=Path)
    parser.add_argument("--ime-peer-sha256")
    parser.add_argument("--repo", type=Path, default=HERE.parents[1])
    parser.add_argument("--fixtures", type=Path)
    parser.add_argument("--sway", type=Path, default=DEFAULT_SWAY)
    parser.add_argument("--swaymsg", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--plan", action="store_true", help="Print commands; execute no test or binary")
    args = parser.parse_args()
    repo = args.repo.resolve()
    binary = args.binary.resolve() if args.binary else Path("/ABSOLUTE/FROZEN/ztty")
    fixtures = (args.fixtures or repo / "artifacts/final-plugins-20261120").resolve()
    sway = args.sway.resolve()
    swaymsg = (args.swaymsg or sway.with_name("swaymsg")).resolve()
    ime_peer = args.ime_peer.resolve() if args.ime_peer else Path("/ABSOLUTE/FROZEN/ime-peer")
    no_wayland = args.no_wayland_binary.resolve() if args.no_wayland_binary else Path("/ABSOLUTE/FROZEN/ztty-no-wayland")
    output = (args.output or HERE / "runs" /
              datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")).resolve()
    steps = build_plan(repo, binary, output, fixtures, sway, swaymsg, ime_peer, no_wayland)
    if args.plan:
        print(json.dumps({"prepared_only": True, "stages": len(steps), "steps": steps}, indent=2))
        return 0
    required = ("binary", "sha256", "source_sha", "no_wayland_binary",
                "no_wayland_sha256", "ime_peer", "ime_peer_sha256")
    missing = ["--" + name.replace("_", "-") for name in required if not getattr(args, name)]
    if missing:
        parser.error("Execution requires: " + ", ".join(missing))
    if args.no_wayland_sha256 == args.sha256:
        parser.error("--no-wayland-sha256 must identify a different, feature-disabled ELF")

    def supporting_identities():
        return {"no_wayland_binary": binary_identity(no_wayland, args.no_wayland_sha256),
                "ime_peer": binary_identity(ime_peer, args.ime_peer_sha256)}

    if output.exists():
        parser.error("--output must not exist; previous acceptance evidence is never overwritten")
    output.mkdir(parents=True)
    report = {"schema": 1, "passed": False, "status": "preflight",
              "started_utc": datetime.now(timezone.utc).isoformat(), "steps": [],
              "expected_source_sha": args.source_sha, "expected_binary_sha256": args.sha256,
              "planned_stages": len(steps),
              "expected_no_wayland_sha256": args.no_wayland_sha256,
              "expected_ime_peer_sha256": args.ime_peer_sha256}
    stopped = {"signal": None}
    for signum in (signal.SIGTERM, signal.SIGINT):
        signal.signal(signum, lambda number, _frame: stopped.update(signal=number))
    report_path = output / "acceptance.json"
    try:
        report["binary_before"] = binary_identity(binary, args.sha256)
        report["supporting_before"] = supporting_identities()
        report["source_before"] = source_identity(repo, args.source_sha)
        report["plugins_before"] = plugin_identities(fixtures)
        report["tools"] = tool_identities(sway, swaymsg)
        report["harness_before"] = harness_identities(repo, HERE)
        enable_subreaper()
        write_json(output / "plan.json", {"stages": len(steps), "steps": steps})
        report["status"] = "running"
        write_json(report_path, report)
        for index, step in enumerate(steps, 1):
            if stopped["signal"] is not None:
                break
            binary_identity(binary, args.sha256)
            supporting_identities()
            directory = Path(step["directory"])
            directory.mkdir()
            raw = directory / "raw.log"
            entry = {**step, "index": index, "status": "running", "raw_log": str(raw),
                     "started_utc": datetime.now(timezone.utc).isoformat()}
            report["steps"].append(entry)
            write_json(report_path, report)
            with (output / "commands.jsonl").open("a") as stream:
                stream.write(json.dumps({"event": "start", **entry}) + "\n")
            print(f"START {index}/{len(steps)} {step['name']}", flush=True)
            last = [0.0]

            def heartbeat(elapsed):
                if elapsed - last[0] >= 30:
                    print(f"RUNNING {step['name']} {int(elapsed)}s", flush=True)
                    last[0] = elapsed

            execution = run_owned(
                step["command"], repo, raw, step["timeout_seconds"],
                lambda: stopped["signal"] is not None, heartbeat,
            )
            entry["execution"] = execution
            write_json(directory / "execution.json", execution)
            entry["receipt"] = read_receipt(step)
            entry["compact_log"] = compact_log(raw, directory / "rtk.log")
            entry["passed"] = (
                execution["exit_code"] == 0 and entry["receipt"]["ok"]
                and not execution.get("timed_out") and not execution.get("cancelled")
                and not execution["cleanup"]["remaining"] and execution["runtime_removed"]
            )
            entry["status"] = "passed" if entry["passed"] else "failed"
            entry["binary_after"] = binary_identity(binary, args.sha256)
            entry["supporting_after"] = supporting_identities()
            write_json(report_path, report)
            with (output / "commands.jsonl").open("a") as stream:
                stream.write(json.dumps({"event": "finish", "name": step["name"],
                                        "passed": entry["passed"],
                                        "exit_code": execution["exit_code"],
                                        "report": step["report"], "raw_log": str(raw)}) + "\n")
            print(json.dumps({"stage": step["name"], "passed": entry["passed"],
                              "checks": entry["receipt"].get("checks"),
                              "seconds": round(execution["seconds"], 2)}), flush=True)
            if execution["cleanup"]["remaining"]:
                raise RuntimeError("Owned process cleanup failed; later stages will not start")
        report["status"] = "cancelled" if stopped["signal"] is not None else "completed"
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        report["status"] = "failed"
        if report["steps"] and report["steps"][-1]["status"] == "running":
            report["steps"][-1].update(status="failed", error=report["error"], passed=False)
    finally:
        try:
            report["binary_after"] = binary_identity(binary, args.sha256)
            report["supporting_after"] = supporting_identities()
            if report.get("supporting_before") != report["supporting_after"]:
                raise ValueError("Supporting ELF identities changed during acceptance")
            report["source_after"] = source_identity(repo, args.source_sha)
            report["plugins_after"] = plugin_identities(fixtures)
            report["harness_after"] = harness_identities(repo, HERE)
            if report.get("plugins_before") != report["plugins_after"]:
                raise ValueError("Plugin identities changed during acceptance")
            if report.get("harness_before") != report["harness_after"]:
                raise ValueError("Test harness files changed during acceptance")
            report["final_identity_check"] = True
        except Exception as error:
            report["final_identity_check"] = False
            report["final_identity_error"] = f"{type(error).__name__}: {error}"
        report["stopped_signal"] = stopped["signal"]
        report["finished_utc"] = datetime.now(timezone.utc).isoformat()
        report["passed"] = (
            report["status"] == "completed" and report["final_identity_check"]
            and len(report["steps"]) == len(steps)
            and all(step.get("passed") is True for step in report["steps"])
        )
        write_json(report_path, report)
    print(json.dumps({"passed": report["passed"], "status": report["status"],
                      "stages": len(report["steps"]), "report": str(report_path)}), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
