#!/usr/bin/env python3
"""Attribute a real idle ztty broker and launcher without counting the PTY child.

Run from the repo: python3 docs/benchmarks/roadmap_idle_smaps.py --binary PATH --output PATH
Creates a private Xvfb display and unique broker socket. No existing window or
process is sampled or stopped. smaps contains mapping accounting, not memory data.
"""
from __future__ import annotations

import argparse
import base64
import contextlib
import ctypes
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import select
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
from benchmark_terminals import FRAME, MARKER, ZTTY_CONFIG, host_pressure, owned_processes
from measure_runtime import memory, process_stat, same_process
from measure_multiwindow import cleanup
from Xlib import display

FIELDS = ("Size", "Rss", "Pss", "Shared_Clean", "Shared_Dirty", "Private_Clean", "Private_Dirty", "Swap", "SwapPss")
HEADER = re.compile(r"^[0-9a-f]+-[0-9a-f]+\s+\S+\s+[0-9a-f]+\s+\S+\s+\d+\s*(.*)$")


def category(path, binary):
    lower = path.lower()
    if path == str(binary):
        return "ztty_executable"
    if lower.endswith((".ttf", ".ttc", ".otf", ".otc")):
        return "font_files"
    if path == "/etc/ld.so.cache":
        return "dynamic_loader_cache"
    if "fontconfig/" in path and ".cache-" in Path(path).name:
        return "fontconfig_cache"
    if re.search(r"\.so(?:\.\d[\w.]*)?$", Path(path).name) and path.startswith("/"):
        return "shared_libraries"
    if path == "[heap]":
        return "heap"
    if path.startswith("[stack"):
        return "stack"
    if not path or path.startswith("[anon"):
        return "anonymous"
    return "other_files" if path.startswith("/") else "kernel_special"


def smaps(pid, started, binary):
    if not same_process(pid, started):
        raise ProcessLookupError(pid)
    mappings = []
    current = None
    for line in Path(f"/proc/{pid}/smaps").read_text().splitlines():
        found = HEADER.match(line)
        if found:
            current = {"path": found[1], **{key + "_bytes": 0 for key in FIELDS}}
            mappings.append(current)
        elif current is not None and ":" in line:
            key, value = line.split(":", 1)
            if key in FIELDS:
                current[key + "_bytes"] = int(value.split()[0]) * 1024
    categories, files = {}, {}
    for record in mappings:
        name = category(record["path"], binary)
        for target, key in ((categories, name), (files, record["path"] or "[anonymous]")):
            aggregate = target.setdefault(key, {field + "_bytes": 0 for field in FIELDS})
            for field in FIELDS:
                aggregate[field + "_bytes"] += record[field + "_bytes"]
    result = {
        "pid": pid, "start_ticks": started, "executable": str(Path(f"/proc/{pid}/exe").resolve()),
        "rollup": memory(pid), "categories": categories,
        "files": [{"path": path, **values} for path, values in sorted(files.items(), key=lambda item: -item[1]["Rss_bytes"])],
        "mapping_count": len(mappings),
    }
    if not same_process(pid, started):
        raise ProcessLookupError(pid)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    if args.output.exists():
        parser.error("choose a new evidence path; existing evidence is preserved")
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("cannot enable child subreaper")
    report = {
        "ok": False, "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "sampler_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "scope": "One idle 120x32 terminal after UTF-8/color frame and font warmup; sum only exact copied ztty executable processes sharing this private run marker. PTY child and X server excluded and identified separately.",
        "limitations": "Busy host; three near-in-time snapshots, not independent performance repetitions. Reading smaps is not an allocation profiler. RSS counts shared mapped pages in each process.",
        "host_before": host_pressure(), "samples": [],
    }
    launcher = xvfb = connection = None
    identities = {}
    with tempfile.TemporaryDirectory(prefix="ztty-idle-smaps-") as temporary:
        directory = Path(temporary)
        tag = directory.name
        name = "idle-smaps"
        title = "ztty-idle-smaps-" + tag
        (directory / "ztty.conf").write_text(ZTTY_CONFIG)
        env = os.environ.copy()
        for key in ("WAYLAND_DISPLAY", "KITTY_LISTEN_ON", "DBUS_SESSION_BUS_ADDRESS"):
            env.pop(key, None)
        env.update({MARKER: tag, "ZTTY_SOCKET": "@" + tag, "LC_ALL": "C.UTF-8", "LANG": "C.UTF-8", "XMODIFIERS": "@im=none", "XDG_CONFIG_HOME": str(directory / "config"), "XDG_CACHE_HOME": str(directory / "cache"), "XDG_RUNTIME_DIR": temporary})
        with (directory / "stderr.log").open("wb") as stderr, (directory / "xvfb.log").open("wb") as xlog:
            try:
                read_fd, write_fd = os.pipe()
                try:
                    xvfb = subprocess.Popen(["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1920x1200x24", "-dpi", "96", "-nolisten", "tcp"], env=env, pass_fds=(write_fd,), stdout=subprocess.DEVNULL, stderr=xlog, start_new_session=True)
                finally:
                    os.close(write_fd)
                try:
                    if not select.select([read_fd], [], [], 10)[0]:
                        raise TimeoutError("private X display did not become ready")
                    number = os.read(read_fd, 64).strip().decode("ascii")
                    assert number.isdecimal(), number
                finally:
                    os.close(read_fd)
                env["DISPLAY"] = ":" + number
                connection = display.Display(env["DISPLAY"])
                frame = FRAME + b"\x1b]0;" + title.encode() + b"\x07"
                child = "import os,sys,base64,time; os.write(1,base64.b64decode(sys.argv[1])); time.sleep(120)"
                command = [str(binary), "--name", name, "--config", str(directory / "ztty.conf"), "--font", "monospace:size=11", "--geometry", "120x32", "--scrollback", "2000", "-e", sys.executable, "-c", child, base64.b64encode(frame).decode()]
                launcher = subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL, stderr=stderr, start_new_session=True)
                pid_atom = connection.intern_atom("_NET_WM_PID")
                title_atom = connection.intern_atom("_NET_WM_NAME")
                window = None
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline and window is None:
                    for candidate in connection.screen().root.query_tree().children:
                        with contextlib.suppress(Exception):
                            prop = candidate.get_full_property(title_atom, 0)
                            if prop and prop.value.decode(errors="replace") == title:
                                window = candidate
                                break
                    if window is None:
                        time.sleep(0.05)
                if window is None:
                    raise TimeoutError("own terminal window did not become ready")
                owner = int(window.get_full_property(pid_atom, 0).value[0])
                report.update({"run_marker": tag, "display": env["DISPLAY"], "window_id": window.id, "gui_owner_pid": owner, "launcher_pid": launcher.pid, "xserver_pid": xvfb.pid, "warmup_seconds": 2.0, "configuration": ZTTY_CONFIG, "frame_sha256": hashlib.sha256(FRAME).hexdigest()})
                time.sleep(2)
                identities = owned_processes(tag)
                application = {pid: started for pid, started in identities.items() if Path(f"/proc/{pid}/exe").resolve() == binary}
                assert set(application) == {owner, launcher.pid}, application
                report["excluded_run_processes"] = [{"pid": pid, "start_ticks": started, "executable": str(Path(f"/proc/{pid}/exe").resolve())} for pid, started in identities.items() if pid not in application]
                for index in range(3):
                    processes = [smaps(pid, started, binary) for pid, started in application.items()]
                    for process in processes:
                        process["role"] = "broker" if process["pid"] == owner else "launcher"
                    report["samples"].append({"index": index + 1, "processes": processes, "host": host_pressure()})
                    time.sleep(0.25)
                closed = subprocess.run([str(binary), "action", name, "close"], env=env, capture_output=True, text=True, timeout=5)
                report["close_exit"] = closed.returncode
                assert closed.returncode == 0, closed.stderr
                report["launcher_exit"] = launcher.wait(timeout=6)
                report["ok"] = report["launcher_exit"] == 0
            except Exception as error:
                report["error"] = repr(error)
            finally:
                identities.update(owned_processes(tag))
                report["remaining_owned_pids"] = cleanup(identities)
                if connection:
                    with contextlib.suppress(Exception):
                        connection.close()
                for process in (launcher, xvfb):
                    if process:
                        with contextlib.suppress(Exception):
                            process.wait(timeout=2)
                report["ok"] = report["ok"] and not report["remaining_owned_pids"]
                report["stderr"] = (directory / "stderr.log").read_text(errors="replace")
                report["host_after"] = host_pressure()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "output": str(args.output), "error": report.get("error"), "samples": len(report["samples"])}))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
