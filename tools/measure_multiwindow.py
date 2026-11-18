#!/usr/bin/env python3
"""Account for multiple real ztty windows on an isolated X server.

PSS/private bytes include every ztty process in the owned run, including a
detached host and waiting launchers. PTY children and X-server allocations are
recorded separately; RSS is descriptive and is not added to PSS.
"""
import argparse
import contextlib
import ctypes
import datetime
import json
import os
from pathlib import Path
import shlex
import signal
import statistics
import subprocess
import tempfile
import time

from measure_runtime import FRAME, descendants, machine, memory, pixmap_bytes, process_stat, same_process
from isolated_xvfb import start_xvfb

ROOT = Path(__file__).resolve().parents[1]
SETTINGS = "background-opacity = 100\nbackground-blur = false\ncursor-blink = false\n"
FIELDS = ("Rss_bytes", "Pss_bytes", "Private_Clean_bytes", "Private_Dirty_bytes", "Swap_bytes")


def owned_processes(binary, tag):
    """Identify only this run; never select the user's other terminal windows."""
    marker = ("ZTTY_MEMORY_TAG=" + tag).encode()
    found = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            if (entry / "exe").resolve() != binary:
                continue
            if marker not in (entry / "environ").read_bytes().split(b"\0"):
                continue
            pid = int(entry.name)
            found[pid] = process_stat(pid)["start_ticks"]
        except (OSError, ValueError):
            continue
    return found


def terminal_windows(display):
    result = {}
    atom = display.intern_atom("_NET_WM_PID")
    for window in display.screen().root.query_tree().children:
        try:
            if window.get_wm_class() != ("ztty", "Ztty"):
                continue
            prop = window.get_full_property(atom, 0)
            if prop is not None:
                result[window.id] = {"pid": int(prop.value[0]), "window": window}
        except Exception:
            continue
    return result


def totals(samples):
    pids = [sample["pid"] for sample in samples]
    if len(pids) != len(set(pids)):
        raise ValueError("a process was included more than once")
    if any(key not in sample for sample in samples for key in FIELDS):
        raise ValueError("required process memory counter is unavailable")
    return {key: sum(sample[key] for sample in samples) for key in FIELDS}


def cleanup(identities):
    for sig in (signal.SIGTERM, signal.SIGKILL):
        for pid, started in identities.items():
            if same_process(pid, started):
                with contextlib.suppress(ProcessLookupError):
                    os.kill(pid, sig)
        deadline = time.monotonic() + 1
        while time.monotonic() < deadline:
            for pid in identities:
                with contextlib.suppress(ChildProcessError):
                    os.waitpid(pid, os.WNOHANG)
            if not any(same_process(pid, started) for pid, started in identities.items()):
                return []
            time.sleep(0.02)
    return [pid for pid, started in identities.items() if same_process(pid, started)]


def measure(binary, count, repetition, expect_shared):
    from Xlib import display
    result = {"windows": count, "repetition": repetition, "ok": False}
    processes, identities = [], {}
    xvfb = xserver = connection = None
    with tempfile.TemporaryDirectory(prefix="ztty-multiwindow-") as temporary:
        directory = Path(temporary)
        (directory / "appearance.conf").write_text(SETTINGS)
        with (directory / "xvfb.log").open("wb") as xlog, (directory / "terminal.log").open("wb") as tlog:
            try:
                xserver = start_xvfb(screen="1920x1200x24", dpi=96, stderr=xlog, timeout=10)
                xvfb = xserver.process
                display_name = xserver.display
                result["display_ownership"] = xserver.evidence
                connection = display.Display(display_name)
                before_server = memory(xvfb.pid)
                environment = os.environ.copy()
                environment.pop("WAYLAND_DISPLAY", None)
                environment.update(
                    DISPLAY=display_name, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                    LIBGL_ALWAYS_SOFTWARE="1", ZTTY_MEMORY_TAG=temporary,
                    ZTTY_SOCKET="@ztty-multiwindow-" + directory.name,
                    XDG_RUNTIME_DIR=str(directory),
                )
                start = time.monotonic()
                for index in range(count):
                    title = "\x1b]2;ztty memory " + str(index + 1) + "\x07"
                    command = [
                        str(binary), "--config", str(directory / "appearance.conf"),
                        "--font", "monospace:size=11", "--geometry", "120x32",
                        "--scrollback", "2000", "-e", "/bin/sh", "-c",
                        "printf %s " + shlex.quote(title + FRAME) + "; exec sleep 300",
                    ]
                    processes.append(subprocess.Popen(
                        command, env=environment, stdout=subprocess.DEVNULL, stderr=tlog,
                        start_new_session=True,
                    ))
                    deadline = time.monotonic() + 15
                    while time.monotonic() < deadline:
                        windows = terminal_windows(connection)
                        if len(windows) == index + 1:
                            break
                        time.sleep(0.02)
                    else:
                        raise TimeoutError(f"window {index + 1} did not appear")
                time.sleep(0.5)
                windows = terminal_windows(connection)
                if len(windows) != count:
                    raise RuntimeError("window count changed before sampling")
                owners = sorted({item["pid"] for item in windows.values()})
                identities = owned_processes(binary, temporary)
                if not identities or not set(owners).issubset(identities):
                    raise RuntimeError("window owner missing from owned process accounting")
                samples = [{"pid": pid, **memory(pid)} for pid in sorted(identities)]
                application = totals(samples)
                application["Private_bytes"] = application["Private_Clean_bytes"] + application["Private_Dirty_bytes"]
                child_ids = {}
                for pid in identities:
                    child_ids.update(descendants(pid))
                child_ids = {pid: started for pid, started in child_ids.items() if pid not in identities}
                children = [{"pid": pid, **memory(pid)} for pid in sorted(child_ids)]
                by_owner = {item["pid"]: window for window, item in windows.items()}
                graphics = [pixmap_bytes(display_name, window) | {"pid": pid}
                            for pid, window in sorted(by_owner.items())]
                after_server = memory(xvfb.pid)
                if expect_shared and len(owners) != 1:
                    raise AssertionError(f"{count} windows have {len(owners)} GUI owners; expected one")
                result.update(
                    ok=True, elapsed_seconds=round(time.monotonic() - start, 3),
                    gui_process_count=len(owners), gui_owner_pids=owners,
                    ztty_process_count=len(identities), application=application,
                    processes=samples, pty_children=children, pty_children_total=totals(children),
                    xserver={"before": before_server, "after": after_server,
                             "delta": {k: after_server[k] - before_server[k] for k in FIELDS}},
                    pixmap_clients=graphics,
                    pixmap_total_bytes=sum(g.get("bytes", 0) for g in graphics)
                        if all(g["available"] for g in graphics) else None,
                )
                identities.update(child_ids)
            except Exception as error:
                result["error"] = repr(error)
            finally:
                # A host may detach from all launcher parents. The unique run tag
                # and process start times keep teardown scoped to this measurement.
                owned = owned_processes(binary, temporary)
                for pid in owned:
                    identities.update(descendants(pid))
                identities.update(owned)
                for process in processes:
                    with contextlib.suppress(OSError, ValueError):
                        identities[process.pid] = process_stat(process.pid)["start_ticks"]
                        identities.update(descendants(process.pid))
                remaining = cleanup(identities)
                for process in processes:
                    with contextlib.suppress(subprocess.TimeoutExpired):
                        process.wait(timeout=1)
                if connection is not None:
                    connection.close()
                if xserver is not None:
                    xserver.close()
                result["remaining_owned_pids"] = remaining
                if remaining:
                    result.update(ok=False, cleanup_error="owned processes remained after teardown")
        result["terminal_stderr"] = (directory / "terminal.log").read_text(errors="replace")[-4096:]
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--compare-binary", type=Path,
                        help="interleave an independent baseline with this binary")
    parser.add_argument("--compare-label", default="baseline")
    parser.add_argument("--counts", default="1,5,10,20")
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--expect-shared", action="store_true")
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/multiwindow-memory.json")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    counts = [int(value) for value in args.counts.split(",")]
    if not counts or any(value < 1 or value > 64 for value in counts):
        parser.error("counts must contain numbers from 1 through 64")
    if not 1 <= args.repetitions <= 20:
        parser.error("repetitions must be from 1 through 20")
    # Reap detached owned hosts after cleanup instead of leaving orphan zombies.
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("cannot enable child subreaper")
    configurations = [(args.label, binary, args.expect_shared)]
    if args.compare_binary is not None:
        configurations.insert(0, (args.compare_label, args.compare_binary.resolve(strict=True), False))
    if len({label for label, _, _ in configurations}) != len(configurations):
        parser.error("comparison labels must differ")
    rows = []
    for repetition in range(1, args.repetitions + 1):
        ordered = configurations if repetition % 2 else list(reversed(configurations))
        for count in counts:
            for label, executable, expect_shared in ordered:
                row = measure(executable, count, repetition, expect_shared)
                row["label"] = label
                rows.append(row)
                print(json.dumps({"label": label, "windows": count, "repetition": repetition,
                                  "ok": row["ok"], "application": row.get("application"),
                                  "error": row.get("error")}), flush=True)
    summary = []
    for label, _, _ in configurations:
        for count in counts:
            values = [row for row in rows if row["label"] == label and row["windows"] == count and row["ok"]]
            if values:
                summary.append({
                    "label": label, "windows": count, "successful_runs": len(values),
                    "median_pss_bytes": statistics.median(row["application"]["Pss_bytes"] for row in values),
                    "median_private_bytes": statistics.median(row["application"]["Private_bytes"] for row in values),
                    "median_private_dirty_bytes": statistics.median(row["application"]["Private_Dirty_bytes"] for row in values),
                    "median_rss_bytes": statistics.median(row["application"]["Rss_bytes"] for row in values),
                    "gui_process_counts": sorted({row["gui_process_count"] for row in values}),
                    "ztty_process_counts": sorted({row["ztty_process_count"] for row in values}),
                })
    result = {
        "schema": 1, "label": args.label, "binary": str(binary),
        "configurations": [{"label": label, "binary": str(executable), "expect_shared": shared}
                           for label, executable, shared in configurations],
        "timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "machine": machine(), "settings": {"font": "monospace:size=11", "geometry": "120x32",
                                          "scrollback": 2000, "appearance": SETTINGS},
        "scope": "Idle windows after static UTF-8/color output, mostly empty history, isolated software Xvfb.",
        "accounting": "Sum of PSS/private memory of unique ztty processes. PTY children and X-server allocations separate.",
        "ok": all(row["ok"] for row in rows), "summary": summary, "runs": rows,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"ok": result["ok"], "output": str(args.output), "summary": summary}), flush=True)
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
