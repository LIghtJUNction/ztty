#!/usr/bin/env python3
"""Measure real terminal windows, then publish reproducible JSON and SVG.

Run from the repository: python3 tools/benchmark_terminals.py
No global terminal installation or default-terminal changes are performed.
Requires Linux /proc, Xvfb, python-xlib, dbus-daemon, Kitty, and Ghostty.
"""
import argparse
import base64
import contextlib
import ctypes
import datetime
import hashlib
import html
import json
import math
import os
from pathlib import Path
import re
import select
import shlex
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import termios
import time
import traceback
import tty

from measure_runtime import (
    capture, descendants, font_files, machine, memory, pixmap_bytes,
    process_stat, same_process,
)
from measure_multiwindow import cleanup
from isolated_xvfb import start_xvfb

ROOT = Path(__file__).resolve().parents[1]
REPORT = ROOT / "docs/benchmarks/terminals-10-windows.json"
SVG = ROOT / "docs/benchmarks/terminals-10-windows.svg"
WINDOWS, COLS, ROWS = 10, 120, 32
REPETITIONS = 3
LATENCY_WARMUP, LATENCY_SAMPLES = 50, 300
OUTPUT_WARMUP, OUTPUT_SAMPLES = 1, 3
OUTPUT_TARGET = 1024 * 1024
MEMORY_SAMPLES = 3
HISTORY_ROWS = 100_000
MARKER = "ZTTY_TERMINAL_BENCHMARK"
FIELDS = (
    "Rss_bytes", "Pss_bytes", "Pss_Anon_bytes", "Pss_File_bytes",
    "Pss_Shmem_bytes", "Private_Clean_bytes", "Private_Dirty_bytes", "Swap_bytes",
)
APPS = ("ztty", "ghostty", "kitty")
MODES = ("shared", "independent")
WORKLOADS = ("idle", "history", "images", "hidden")
NAMES = {"ztty": "ztty", "ghostty": "Ghostty", "kitty": "Kitty"}
ZTTY_CONFIG = "background-opacity = 100\nbackground-blur = false\ncursor-blink = false\n"
KITTY_CONFIG = """linux_display_server x11
font_family monospace
font_size 11
initial_window_width 120c
initial_window_height 32c
remember_window_size no
scrollback_lines 2000
background_opacity 1
cursor_blink_interval 0
enable_audio_bell no
shell_integration disabled
confirm_os_window_close 0
"""
SESSION_BUS_CONFIG = '<busconfig>\n  <type>session</type>\n  <listen>unix:path={address}</listen>\n  <auth>EXTERNAL</auth>\n  <policy context="default">\n    <allow own="*"/>\n    <allow send_destination="*"/>\n    <allow receive_sender="*"/>\n  </policy>\n  <!-- No service directories: this private bus only coordinates instances. -->\n</busconfig>\n'
GHOSTTY_CONFIG = """font-family = monospace
font-size = 11
window-width = 120
window-height = 32
window-padding-x = 0
window-padding-y = 0
scrollback-limit = 10000000
background-opacity = 1
cursor-style-blink = false
window-decoration = none
gtk-titlebar = false
gtk-single-instance = true
shell-integration = none
linux-cgroup = never
quit-after-last-window-closed = true
"""
FRAME = (
    "\x1b[2J\x1b[H\x1b[?25l"
    "\x1b[1;36mTerminal benchmark: fixed window workload\x1b[0m\r\n"
    "120 columns x 32 rows | monospace 11 pt | fixed warm-up frame\r\n"
    "UTF-8: Chinese \u4e2d\u6587 | Japanese \u65e5\u672c\u8a9e | cafe\u0301 | caf\u00e9\r\n"
    "\x1b[31mred\x1b[0m \x1b[32mgreen\x1b[0m \x1b[34mblue\x1b[0m "
    "\x1b[38;2;230;150;60mtruecolor\x1b[0m\r\n"
    "No scrolling or animation in the memory sample.\r\n"
    "PTY completion is measured with cursor-position replies (DSR).\r\n"
).encode()
BARRIER = b"\x1b[H\x1b[6n"
BARRIER_REPLY = b"\x1b[1;1R"
LATENCY_COMMAND = b"\x1b[Hq\x1b[6n"
LATENCY_REPLY = b"\x1b[1;2R"
OUTPUT_CHECK = b"\x1b[6n"
OUTPUT_CHECK_REPLY = f"\x1b[{ROWS};{COLS}R".encode("ascii")


def send_json(connection, value):
    connection.sendall(json.dumps(value, ensure_ascii=False).encode() + b"\n")


def receive_json(connection):
    data = bytearray()
    while not data.endswith(b"\n"):
        block = connection.recv(65536)
        if not block:
            raise EOFError("probe control socket closed")
        data.extend(block)
        if len(data) > 4 * 1024 * 1024:
            raise ValueError("probe response exceeds 4 MiB")
    return json.loads(data)


def write_all(data):
    view = memoryview(data)
    while view:
        count = os.write(1, view[:65536])
        if count <= 0:
            raise OSError("PTY write made no progress")
        view = view[count:]


def exchange(command, expected, timeout=15):
    """Same real-PTY DSR barrier used by tests/pty_probe.py."""
    write_all(command)
    data = bytearray()
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not select.select([0], [], [], max(0, deadline - time.monotonic()))[0]:
            break
        block = os.read(0, 4096)
        if not block:
            break
        data.extend(block)
        if expected in data:
            return
        if len(data) > 65536:
            raise AssertionError("unexpected excess terminal input")
    raise AssertionError("missing DSR response: " + repr(bytes(data[-256:])))


def output_payload():
    """In-place ASCII/color screen updates; no scrollback or partial escape."""
    blocks = []
    total, frame = 0, 0
    while total < OUTPUT_TARGET:
        rows = []
        for row in range(1, ROWS + 1):
            letter = chr(65 + (frame + row) % 26)
            text = (f"frame {frame:04d} row {row:02d} " + letter * COLS)[:COLS - 1]
            rows.append(f"\x1b[{row};1H\x1b[{31 + frame % 7}m{text}")
        block = ("".join(rows) + "\x1b[0m").encode("ascii")
        blocks.append(block)
        total += len(block)
        frame += 1
    return b"".join(blocks)


def graphics_payload():
    """One deterministic 256x256 RGBA image, with protocol-sized chunks."""
    pixels = bytes(value for y in range(256) for x in range(256)
                   for value in (x, y, (x + y) % 256, 255))
    encoded = base64.b64encode(pixels)
    chunks = [encoded[offset:offset + 4096] for offset in range(0, len(encoded), 4096)]
    commands = []
    for index, chunk in enumerate(chunks):
        header = "a=T,f=32,s=256,v=256,i=42,q=0," if index == 0 else ""
        header += "m=" + str(int(index + 1 < len(chunks)))
        commands.append(b"\x1b_G" + header.encode() + b";" + chunk + b"\x1b\\")
    return b"".join(commands)


def prepare_workload(workload):
    if workload == "history":
        # Same bytes and rows for each app. Their configured capacity units differ.
        row = "0123456789" * 11
        for start in range(0, HISTORY_ROWS, 1000):
            write_all("".join(f"{index:06d} {row}\r\n" for index in range(start, start + 1000)).encode())
        exchange(BARRIER, BARRIER_REPLY)
    elif workload == "images":
        exchange(graphics_payload(), b"\x1b_Gi=42;OK\x1b\\")
        exchange(BARRIER, BARRIER_REPLY)
    elif workload not in ("idle", "hidden"):
        raise ValueError("unknown workload")


def output_screen(payload):
    """Expected last frame from the fixed workload, outside timed sections."""
    frame = payload.rsplit(b"\x1b[0m", 2)[-2]
    rows = re.findall(rb"\x1b\[(\d+);1H\x1b\[3[1-7]m([ -~]+)", frame)
    if [int(row) for row, _ in rows] != list(range(1, ROWS + 1)):
        raise ValueError("output payload has no complete final screen")
    if any(len(text) != COLS - 1 for _, text in rows):
        raise ValueError("output payload has an unexpected row width")
    return "".join(text.decode("ascii") + "\n" for _, text in rows)


def broker_value(address, request):
    """Read only the private ztty broker created by this run."""
    if not address.startswith("@") or not 2 <= len(address) <= 107:
        raise ValueError("benchmark broker must use its private abstract socket")
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(5)
        connection.connect("\0" + address[1:])
        send_json(connection, request)
        response = receive_json(connection)
    if response.get("ok") is not True:
        raise AssertionError("benchmark broker read failed: " + repr(response))
    return response["result"]


def verify_ztty_screen(address, title, expected):
    sessions = broker_value(address, {"op": "list"})["sessions"]
    matches = [session for session in sessions if session["title"] == title]
    if len(matches) != 1:
        raise AssertionError("cannot identify the benchmark session by its unique title")
    value = broker_value(address, {"op": "read", "session": matches[0]["uri"], "max_bytes": 1})
    if (value["cols"], value["rows"]) != (COLS, ROWS) or value["screen_truncated"]:
        raise AssertionError("benchmark screen geometry differs or screen was clipped")
    if value["screen"] != expected:
        raise AssertionError("output final screen differs from the complete workload")
    return {"all_text_cells_checked": True, "rows": ROWS, "columns": COLS,
            "screen_utf8_sha256": hashlib.sha256(expected.encode()).hexdigest()}


def percentile(values, percent):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int((len(ordered) - 1) * percent / 100))]


def probe(path):
    previous = termios.tcgetattr(0)
    connection = socket.socket(socket.AF_UNIX)
    try:
        tty.setraw(0)
        connection.connect(path)
        send_json(connection, {"pid": os.getpid()})
        settings = receive_json(connection)
        title = settings["title"]
        write_all(("\x1b]2;" + title + "\x07").encode() + FRAME)
        exchange(BARRIER, BARRIER_REPLY)
        prepare_workload(settings.get("workload", "idle"))
        size = os.get_terminal_size(0)
        send_json(connection, {
            "ready": True, "pid": os.getpid(), "cols": size.columns, "rows": size.lines,
            "environment": {key: os.environ.get(key) for key in
                            ("TERM", "COLORTERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION")},
        })
        while True:
            request = receive_json(connection)
            if request["op"] == "quit":
                return 0
            if request["op"] == "sample":
                exchange(BARRIER, BARRIER_REPLY)
                size = os.get_terminal_size(0)
                send_json(connection, {"cols": size.columns, "rows": size.lines})
            elif request["op"] == "restore":
                write_all(FRAME)
                exchange(BARRIER, BARRIER_REPLY)
                send_json(connection, {"restored": True})
            elif request["op"] == "performance":
                latency = []
                for index in range(LATENCY_WARMUP + LATENCY_SAMPLES):
                    started = time.perf_counter_ns()
                    exchange(LATENCY_COMMAND, LATENCY_REPLY)
                    elapsed = (time.perf_counter_ns() - started) / 1000
                    if index >= LATENCY_WARMUP:
                        latency.append(elapsed)
                payload = output_payload()
                output = []
                for index in range(OUTPUT_WARMUP + OUTPUT_SAMPLES):
                    started = time.perf_counter_ns()
                    write_all(payload)
                    exchange(BARRIER, BARRIER_REPLY)
                    elapsed = (time.perf_counter_ns() - started) / 1e9
                    if index >= OUTPUT_WARMUP:
                        output.append({
                            "seconds": elapsed, "bytes": len(payload),
                            "mib_per_second": len(payload) / (1024 * 1024) / elapsed,
                        })
                # Keep the original timed payload and HOME/DSR barrier exactly
                # unchanged. This separate, untimed replay checks the cursor
                # before any HOME reset can hide dropped print operations.
                write_all(payload)
                exchange(OUTPUT_CHECK, OUTPUT_CHECK_REPLY)
                send_json(connection, {
                    "output_integrity": {
                        "untimed": True, "final_cursor_checked": True,
                        "command_hex": OUTPUT_CHECK.hex(),
                        "expected_reply_hex": OUTPUT_CHECK_REPLY.hex(),
                        "payload_bytes": len(payload),
                        "payload_sha256": hashlib.sha256(payload).hexdigest(),
                    },
                    "latency_us": latency,
                    "latency_p50_us": statistics.median(latency),
                    "latency_p95_us": percentile(latency, 95),
                    "latency_p99_us": percentile(latency, 99),
                    "output": output,
                    "median_output_mib_per_second": statistics.median(
                        value["mib_per_second"] for value in output),
                })
            else:
                raise ValueError("unknown probe operation")
    finally:
        with contextlib.suppress(OSError):
            connection.close()
        termios.tcsetattr(0, termios.TCSANOW, previous)


def owned_processes(tag):
    """The run marker follows detached hosts, launchers and activated helpers."""
    marker = (MARKER + "=" + tag).encode()
    found = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            if marker not in (entry / "environ").read_bytes().split(b"\0"):
                continue
            pid = int(entry.name)
            stat = process_stat(pid)
            if stat["state"] != "Z":
                found[pid] = stat["start_ticks"]
        except (OSError, ValueError):
            continue
    # Also include children that sanitize their inherited environment.
    for pid in list(found):
        found.update(descendants(pid))
    return found


def totals(samples):
    """Missing kernel counters remain unavailable; duplicate PIDs are an error."""
    pids = [sample["pid"] for sample in samples]
    if len(pids) != len(set(pids)):
        raise ValueError("a process was included more than once")
    required = ("Rss_bytes", "Pss_bytes", "Private_Clean_bytes", "Private_Dirty_bytes")
    for sample in samples:
        if any(key not in sample for key in required):
            raise ValueError("required process memory counter is unavailable")
    result = {
        key: sum(sample[key] for sample in samples)
        if all(key in sample and sample[key] is not None for sample in samples) else None
        for key in FIELDS
    }
    result["Private_bytes"] = result["Private_Clean_bytes"] + result["Private_Dirty_bytes"]
    return result


def sample_process(pid, started):
    if not same_process(pid, started):
        raise ProcessLookupError(pid)
    result = {"pid": pid, "start_ticks": started, **memory(pid)}
    result["exe"] = str(Path(f"/proc/{pid}/exe").resolve())
    result["argv"] = [
        arg.decode(errors="replace")
        for arg in Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0") if arg
    ]
    if not same_process(pid, started):
        raise ProcessLookupError(pid)
    return result


def idle_cpu(identities, seconds=0.5):
    """Percentage of one CPU core; absent/reused process identities invalidate it."""
    try:
        before = {pid: process_stat(pid) for pid in identities}
    except (OSError, ValueError):
        return {"available": False, "reason": "process exited before CPU sample"}
    if any(row["start_ticks"] != identities[pid] for pid, row in before.items()):
        return {"available": False, "reason": "process identity changed before CPU sample"}
    started = time.monotonic()
    time.sleep(seconds)
    after = {}
    for pid, identity in identities.items():
        try:
            row = process_stat(pid)
        except (OSError, ValueError):
            return {"available": False, "reason": "process exited during CPU sample"}
        if row["start_ticks"] != identity:
            return {"available": False, "reason": "process identity changed during CPU sample"}
        after[pid] = row
    elapsed = time.monotonic() - started
    ticks = sum(after[pid]["user_ticks"] + after[pid]["system_ticks"]
                - before[pid]["user_ticks"] - before[pid]["system_ticks"] for pid in identities)
    hz = os.sysconf("SC_CLK_TCK")
    return {"available": True, "elapsed_seconds": elapsed, "clock_ticks_per_second": hz,
            "cpu_ticks": ticks, "percent_of_one_core": ticks / hz / elapsed * 100,
            "threads": sum(row["threads"] for row in after.values()),
            "processes": [{"pid": pid, "before": before[pid], "after": after[pid]}
                          for pid in sorted(identities)]}


def host_pressure():
    counters = {}
    for line in Path("/proc/meminfo").read_text().splitlines():
        name, value = line.split(":", 1)
        if name in ("MemTotal", "MemAvailable", "SwapTotal", "SwapFree"):
            counters[name + "_bytes"] = int(value.split()[0]) * 1024
    counters["swap_used_bytes"] = counters["SwapTotal_bytes"] - counters["SwapFree_bytes"]
    counters["load_average"] = list(os.getloadavg())
    counters["logical_cpus"] = len(os.sched_getaffinity(0))
    counters["psi"] = {}
    for resource in ("cpu", "memory", "io"):
        path = Path("/proc/pressure") / resource
        try:
            counters["psi"][resource] = path.read_text().strip()
        except OSError:
            counters["psi"][resource] = None
    return counters


def comparison_pressure_reasons(snapshot):
    reasons = []
    if snapshot["MemAvailable_bytes"] < max(2 * 1024 ** 3, snapshot["MemTotal_bytes"] // 10):
        reasons.append("available memory below 2 GiB or 10% of RAM")
    if snapshot["load_average"][0] > snapshot["logical_cpus"] / 2:
        reasons.append("one-minute load exceeds half the available logical CPUs")
    return reasons


def application_windows(connection, prefix, include_hidden=False):
    from Xlib import X
    result = {}
    pid_atom = connection.intern_atom("_NET_WM_PID")
    name_atom = connection.intern_atom("_NET_WM_NAME")
    for window in connection.screen().root.query_tree().children:
        try:
            title_property = window.get_full_property(name_atom, 0)
            title = (title_property.value.decode(errors="replace")
                     if title_property is not None and isinstance(title_property.value, bytes)
                     else window.get_wm_name() or "")
            if not title.startswith(prefix):
                continue
            visible = window.get_attributes().map_state == X.IsViewable
            if not visible and not include_hidden:
                continue
            owner = window.get_full_property(pid_atom, 0)
            if owner is None:
                raise RuntimeError("terminal window has no _NET_WM_PID")
            geometry = window.get_geometry()
            result[title] = {
                "id": window.id, "pid": int(owner.value[0]), "window": window,
                "title": title, "width": geometry.width, "height": geometry.height,
                "depth": geometry.depth, "wm_class": window.get_wm_class(),
                "visible": visible,
            }
        except (OSError, AttributeError):
            continue
    return result


def terminal_command(app, binary, directory, tag, mode="shared"):
    if mode not in MODES:
        raise ValueError("unknown process-sharing mode")
    wrapper = str(directory / "probe")
    if app == "ztty":
        return [str(binary), "--config", str(directory / "ztty.conf"),
                "--font", "monospace:size=11", "--geometry", "120x32",
                "--scrollback", "2000", "-e", wrapper]
    if app == "kitty":
        options = []
        for line in KITTY_CONFIG.splitlines():
            key, value = line.split(maxsplit=1)
            options.extend(["-o", key + "=" + value])
        sharing = ["--single-instance", "--instance-group", tag] if mode == "shared" else []
        return [str(binary), "--config", "NONE", *options, *sharing, wrapper]
    return [
        str(binary), "--config-default-files=false",
        "--config-file=" + str(directory / "ghostty.conf"),
        "--class=org.ztty.benchmark." + tag.replace("-", "_"),
        "--gtk-single-instance=" + ("true" if mode == "shared" else "false"),
        "--command=" + wrapper,
    ]


def measure(app, binary, repetition, settle, count=None, mode="shared", workload="idle",
            purpose="comparison", ztty_renderer="auto"):
    from Xlib import X, display
    count = WINDOWS if count is None else count
    result = {"app": app, "windows": count, "mode": mode, "workload": workload,
              "repetition": repetition, "ok": False,
              "load_average_at_start": list(os.getloadavg())}
    result["host_at_start"] = host_pressure()
    result["host_contention"] = comparison_pressure_reasons(result["host_at_start"])
    if purpose == "comparison" and result["host_contention"]:
        result["error"] = "comparison deferred: " + "; ".join(result["host_contention"])
        return result
    launchers, connections, identities = [], [], {}
    xvfb = xserver = bus = xconnection = listener = None
    with tempfile.TemporaryDirectory(prefix="ztty-compare-") as temporary:
        directory = Path(temporary)
        tag = directory.name
        prefix = "terminal-benchmark-" + tag + "-"
        ztty_config = ZTTY_CONFIG + f"renderer = {ztty_renderer}\n"
        (directory / "ztty.conf").write_text(ztty_config)
        ghostty_config = GHOSTTY_CONFIG.replace(
            "gtk-single-instance = true",
            "gtk-single-instance = " + ("true" if mode == "shared" else "false"))
        (directory / "ghostty.conf").write_text(ghostty_config)
        bus_config = SESSION_BUS_CONFIG.format(address=html.escape(str(directory / "bus")))
        (directory / "dbus.conf").write_text(bus_config)
        result["dbus_config"] = bus_config
        control_path = str(directory / "control.sock")
        wrapper = "#!/bin/sh\nexec " + shlex.join([
            sys.executable, str(Path(__file__).resolve()), "--child", control_path,
        ]) + "\n"
        (directory / "probe").write_text(wrapper)
        (directory / "probe").chmod(0o700)
        environment = os.environ.copy()
        for key in ("WAYLAND_DISPLAY", "KITTY_LISTEN_ON", "KITTY_PID", "KITTY_WINDOW_ID",
                    "TERM_PROGRAM", "TERM_PROGRAM_VERSION", "KITTY_CONFIG_DIRECTORY", "GHOSTTY_BIN_DIR",
                    "GHOSTTY_RESOURCES_DIR", "DBUS_SESSION_BUS_ADDRESS"):
            environment.pop(key, None)
        environment.update({
            MARKER: tag, "LC_ALL": "C.UTF-8", "LANG": "C.UTF-8",
            "XMODIFIERS": "@im=none", "GDK_BACKEND": "x11",
            "LIBGL_ALWAYS_SOFTWARE": "1", "GSK_RENDERER": "gl",
            "GDK_SCALE": "1", "GDK_DPI_SCALE": "1",
            "GSETTINGS_BACKEND": "memory", "GTK_A11Y": "none", "NO_AT_BRIDGE": "1",
            "ZTTY_SOCKET": "@ztty-compare-" + tag,
            "XDG_RUNTIME_DIR": temporary,
            "XDG_CONFIG_HOME": str(directory / "config"),
            "XDG_CACHE_HOME": str(directory / "cache"),
            "XDG_DATA_HOME": str(directory / "data"),
        })
        resources = binary.parent.parent / "share/ghostty"
        if app == "ghostty" and resources.is_dir():
            environment["GHOSTTY_RESOURCES_DIR"] = str(resources)
        with (directory / "xvfb.log").open("wb") as xlog, \
                (directory / "terminal.log").open("wb") as tlog, \
                (directory / "dbus.log").open("wb") as blog:
            try:
                xserver = start_xvfb(
                    screen=f"7680x{max(2160, ((count + 4) // 5) * 1080)}x24",
                    dpi=96, stderr=xlog, env=environment, timeout=10,
                )
                xvfb = xserver.process
                environment["DISPLAY"] = display_name = xserver.display
                result["display_ownership"] = xserver.evidence
                xconnection = display.Display(display_name)
                server_before = memory(xvfb.pid)
                bus = subprocess.Popen(
                    ["dbus-daemon", "--config-file=" + str(directory / "dbus.conf"),
                     "--nofork", "--print-address=1"],
                    stdout=subprocess.PIPE, stderr=blog, env=environment,
                    start_new_session=True,
                )
                if not select.select([bus.stdout], [], [], 10)[0]:
                    raise TimeoutError("private D-Bus session did not start")
                address = bus.stdout.readline().decode().strip()
                if not address.startswith("unix:"):
                    raise RuntimeError("unexpected D-Bus address")
                environment["DBUS_SESSION_BUS_ADDRESS"] = address
                infrastructure = {xvfb.pid, bus.pid}
                listener = socket.socket(socket.AF_UNIX)
                listener.bind(control_path)
                listener.listen(count)
                listener.settimeout(30)
                command = terminal_command(app, binary, directory, tag, mode)
                result["command"] = command
                result["commands"] = []
                result["config"] = {
                    "ztty": ztty_config, "kitty": KITTY_CONFIG, "ghostty": ghostty_config,
                }[app]
                result["environment_overrides"] = {
                    key: environment[key] for key in (
                        "LC_ALL", "XMODIFIERS", "GDK_BACKEND", "LIBGL_ALWAYS_SOFTWARE",
                        "GSK_RENDERER", "GDK_SCALE", "GDK_DPI_SCALE", "GSETTINGS_BACKEND",
                        "GTK_A11Y", "NO_AT_BRIDGE",
                    )
                }
                started = time.monotonic()
                probes = []
                launch_times = []
                for index in range(count):
                    if app == "ztty" and mode == "independent":
                        environment["ZTTY_SOCKET"] = "@ztty-compare-" + tag + "-" + str(index)
                    launch_tag = tag if mode == "shared" else tag + "_" + str(index)
                    launch_command = terminal_command(app, binary, directory, launch_tag, mode)
                    result["commands"].append(launch_command)
                    launched = time.monotonic()
                    launchers.append(subprocess.Popen(
                        launch_command, env=environment, stdout=subprocess.DEVNULL,
                        stderr=tlog, start_new_session=True,
                    ))
                    channel, _ = listener.accept()
                    channel.settimeout(90)
                    connections.append(channel)
                    hello = receive_json(channel)
                    title = prefix + str(index + 1)
                    send_json(channel, {"title": title, "workload": workload})
                    ready = receive_json(channel)
                    launch_times.append(time.monotonic() - launched)
                    if not ready.get("ready") or hello["pid"] != ready["pid"]:
                        raise RuntimeError("probe failed to initialize")
                    deadline = time.monotonic() + 15
                    while time.monotonic() < deadline:
                        windows = application_windows(xconnection, prefix)
                        if title in windows:
                            break
                        time.sleep(0.02)
                    else:
                        raise TimeoutError("mapped terminal window did not appear")
                    window = windows[title]["window"]
                    if windows[title]["width"] > 1536 or windows[title]["height"] > 1080:
                        raise RuntimeError("terminal window exceeds its non-overlapping tile")
                    window.configure(x=(index % 5) * 1536, y=(index // 5) * 1080)
                    xconnection.sync()
                    send_json(channel, {"op": "sample"})
                    geometry = receive_json(channel)
                    if geometry != {"cols": COLS, "rows": ROWS}:
                        raise AssertionError(f"{app} terminal grid differs: {geometry}")
                    probes.append({"index": index + 1, "title": title, **ready,
                                   "broker_socket": environment["ZTTY_SOCKET"] if app == "ztty" else None})
                result["windows_ready_seconds"] = time.monotonic() - started
                result["launch_to_dsr_ready_seconds"] = launch_times
                if count == 10:
                    result["ten_windows_ready_seconds"] = result["windows_ready_seconds"]
                # DSR establishes terminal state; this settling interval is only
                # for font/resource warm-up before memory sampling, not a timer barrier.
                time.sleep(settle)
                windows = application_windows(xconnection, prefix)
                if len(windows) != count:
                    raise RuntimeError(f"expected {count} mapped windows, found {len(windows)}")
                if app == "ztty":
                    from gui_renderer import record_renderer
                    result["renderers"] = [
                        record_renderer(xconnection, row["window"], ztty_renderer)
                        for row in windows.values()
                    ]
                    if any(row["actual"] not in ("gpu", "software") for row in result["renderers"]):
                        raise RuntimeError("ztty omitted its actual renderer identity")
                if workload == "hidden":
                    for title, row in windows.items():
                        if title != prefix + "1":
                            row["window"].unmap()
                    xconnection.sync()
                    time.sleep(settle)
                    windows = application_windows(xconnection, prefix, include_hidden=True)
                    visible = sum(row["visible"] for row in windows.values())
                    if len(windows) != count or visible != 1:
                        raise AssertionError("hidden workload must retain all windows with exactly one visible")
                owners = sorted({row["pid"] for row in windows.values()})
                expected_owners = 1 if mode == "shared" else count
                if len(owners) != expected_owners:
                    raise AssertionError(
                        f"{app} {mode} mode: {len(owners)} GUI owner processes; expected {expected_owners}")
                probe_ids = {}
                for row in probes:
                    probe_ids[row["pid"]] = process_stat(row["pid"])["start_ticks"]
                    probe_ids.update(descendants(row["pid"]))
                memory_snapshots = []
                for sample_index in range(MEMORY_SAMPLES):
                    owned = owned_processes(tag)
                    app_ids = {pid: start for pid, start in owned.items()
                               if pid not in infrastructure and pid not in probe_ids}
                    if not set(owners).issubset(app_ids):
                        raise RuntimeError("GUI owner missing from all-process accounting")
                    application, vanished = [], []
                    for pid, process_start in sorted(app_ids.items()):
                        try:
                            application.append(sample_process(pid, process_start))
                        except (FileNotFoundError, ProcessLookupError):
                            vanished.append(pid)
                    if not set(owners).issubset({row["pid"] for row in application}):
                        raise RuntimeError("GUI owner disappeared while sampling")
                    children = [sample_process(pid, start) for pid, start in sorted(probe_ids.items())]
                    memory_snapshots.append({
                        "index": sample_index + 1, "application": totals(application),
                        "application_processes": application, "vanished_processes": vanished,
                        "pty_children": children, "pty_children_total": totals(children),
                    })
                    if sample_index + 1 < MEMORY_SAMPLES:
                        time.sleep(0.1)
                application = {
                    key: statistics.median(row["application"][key] for row in memory_snapshots)
                    if all(row["application"][key] is not None for row in memory_snapshots) else None
                    for key in memory_snapshots[0]["application"]
                }
                cpu = idle_cpu(app_ids)
                current_app_ids = {pid: identity for pid, identity in owned_processes(tag).items()
                                   if pid not in infrastructure and pid not in probe_ids}
                if current_app_ids != app_ids:
                    cpu = {"available": False, "reason": "application process set changed during CPU sample"}
                server_after = memory(xvfb.pid)
                # XRes returns allocations for a whole X client, not one window.
                # Query each unique owner once; never multiply a shared client's total.
                representative = {row["pid"]: row["id"] for row in windows.values()}
                graphics = [
                    pixmap_bytes(display_name, window_id) | {"pid": pid}
                    for pid, window_id in sorted(representative.items())
                ]
                result.update(
                    gui_process_count=len(owners), gui_owner_pids=owners,
                    windows_observed=[
                        {key: value for key, value in row.items() if key != "window"}
                        for _, row in sorted(windows.items())
                    ],
                    probes=probes, application=application, memory_samples=memory_snapshots,
                    application_process_counts=[
                        len(row["application_processes"]) for row in memory_snapshots
                    ],
                    idle_cpu=cpu,
                    fonts={str(pid): font_files(pid) for pid in owners},
                    pty_children_total=memory_snapshots[-1]["pty_children_total"],
                    xserver={"before": server_before, "after": server_after,
                             "delta": {key: server_after[key] - server_before[key]
                                       if key in server_after and key in server_before else None
                                       for key in FIELDS}},
                    private_dbus=sample_process(bus.pid, process_stat(bus.pid)["start_ticks"]),
                    gpu_memory={"available": False, "reason": "Xvfb with Mesa software GL; hardware GPU/VRAM not measured"},
                    first_presented_frame={"available": False, "reason": "no presentation fence; startup uses DSR"},
                    input_to_display_latency={"available": False,
                                              "reason": "DSR times PTY processing, not display presentation"},
                    pixmap_clients=graphics,
                    pixmap_total_bytes=sum(row["bytes"] for row in graphics)
                        if all(row["available"] for row in graphics) else None,
                )
                performance = []
                expected_screen = output_screen(output_payload())
                for index, channel in enumerate(connections):
                    window = windows[prefix + str(index + 1)]["window"]
                    if windows[prefix + str(index + 1)]["visible"]:
                        window.configure(stack_mode=X.Above)
                        window.set_input_focus(X.RevertToParent, X.CurrentTime)
                        xconnection.sync()
                    send_json(channel, {"op": "performance"})
                    values = receive_json(channel)
                    if values.get("output_integrity", {}).get("final_cursor_checked") is not True:
                        raise AssertionError("output completion check was not performed")
                    if app == "ztty":
                        values["output_integrity"]["screen"] = verify_ztty_screen(
                            probes[index]["broker_socket"], prefix + str(index + 1), expected_screen)
                    send_json(channel, {"op": "restore"})
                    if receive_json(channel) != {"restored": True}:
                        raise AssertionError("probe failed to restore the idle frame")
                    performance.append({"index": index + 1, **values})
                latency = [value for row in performance for value in row["latency_us"]]
                result["performance"] = performance
                result["latency_p50_us"] = statistics.median(latency)
                result["latency_p95_us"] = percentile(latency, 95)
                result["latency_p99_us"] = percentile(latency, 99)
                result["output_mib_per_second"] = statistics.median(
                    row["median_output_mib_per_second"] for row in performance)
                if app == "ztty":
                    result["renderers_after"] = [
                        record_renderer(xconnection, row["window"], ztty_renderer)
                        for row in windows.values()
                    ]
                    if result["renderers_after"] != result["renderers"]:
                        raise RuntimeError("ztty renderer identity changed during measurement")
                if len(application_windows(xconnection, prefix, include_hidden=workload == "hidden")) != count:
                    raise RuntimeError("window count changed during performance measurements")
                result["ok"] = True
                for channel in connections:
                    send_json(channel, {"op": "quit"})
            except Exception as error:
                result["ok"] = False
                result["error"] = repr(error)
                result["traceback"] = traceback.format_exc()
            finally:
                for channel in connections:
                    with contextlib.suppress(OSError):
                        channel.close()
                if listener is not None:
                    listener.close()
                owned = owned_processes(tag)
                identities.update(owned)
                for process in launchers:
                    with contextlib.suppress(OSError, ValueError):
                        identities[process.pid] = process_stat(process.pid)["start_ticks"]
                        identities.update(descendants(process.pid))
                # Keep server and bus alive until all app children are stopped.
                infra_ids = {process.pid for process in (bus, xvfb) if process is not None}
                app_cleanup = {pid: start for pid, start in identities.items() if pid not in infra_ids}
                remaining = cleanup(app_cleanup)
                for process in launchers:
                    with contextlib.suppress(subprocess.TimeoutExpired):
                        process.wait(timeout=1)
                if xconnection is not None:
                    xconnection.close()
                if bus is not None:
                    bus.terminate()
                    try:
                        bus.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        bus.kill()
                        bus.wait(timeout=2)
                if xserver is not None:
                    xserver.close()
                remaining.extend(pid for pid in owned_processes(tag) if pid not in remaining)
                result["remaining_owned_pids"] = remaining
                if remaining:
                    result.update(ok=False, cleanup_error="owned processes remained after teardown")
        result["terminal_stderr"] = (directory / "terminal.log").read_text(errors="replace")[-12000:]
        result["dbus_stderr"] = (directory / "dbus.log").read_text(errors="replace")[-4000:]
        result["load_average_at_end"] = list(os.getloadavg())
        result["host_at_end"] = host_pressure()
    return result


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def resolve_binary(argument, fallback):
    if argument:
        path = Path(argument).expanduser()
        if not path.exists():
            found = shutil.which(str(argument))
            if found:
                path = Path(found)
        return path.resolve(strict=True)
    found = shutil.which(fallback)
    if found:
        return Path(found).resolve()
    raise FileNotFoundError(f"{fallback} is unavailable; supply --{fallback} /absolute/path")


def ghostty_binary(argument):
    if argument:
        return resolve_binary(argument, "ghostty")
    installed = shutil.which("ghostty")
    if installed:
        return Path(installed).resolve()
    # This pinned, extracted Arch package is optional and never installed globally.
    roots = [ROOT, ROOT.parent.parent]
    for root in roots:
        candidate = root / "artifacts/benchmark-cache/ghostty-1.4.0-1/usr/bin/ghostty"
        if candidate.is_file():
            return candidate.resolve()
    return resolve_binary(None, "ghostty")


def binary_metadata(path):
    metadata = {
        "path": str(path), "sha256": sha256(path),
        "version": capture([str(path), "--version"]),
    }
    provenance = path.parent.parent.parent / "provenance.json"
    if provenance.is_file():
        metadata["distribution_provenance"] = json.loads(provenance.read_text())
    return metadata


def group_key(run):
    return (run["windows"], run.get("mode", "shared"), run.get("workload", "idle"), run["app"])


def summarize(runs, repetitions=REPETITIONS):
    def available(values, signed=False):
        values = list(values)
        for value in values:
            if value is not None and (type(value) not in (int, float) or not math.isfinite(value)
                                      or not signed and value < 0):
                raise ValueError("optional memory counters must be finite nonnegative numbers")
        return values if values and all(value is not None for value in values) else None

    def extent(values):
        return [min(values), max(values)] if values is not None else None

    def combine(application, server, field):
        values = available((application.get(field), server.get(field)))
        return sum(values) if values is not None else None

    rows = []
    for key in sorted({group_key(run) for run in runs}):
        count, mode, workload, app = key
        selected = [run for run in runs if group_key(run) == key and run["ok"]]
        if len(selected) != repetitions or {run["repetition"] for run in selected} != set(range(1, repetitions + 1)):
            raise ValueError(f"{key} requires {repetitions} distinct completed repetitions")
        def median_value(field):
            values = available(run["application"].get(field) for run in selected)
            return statistics.median(values) if values is not None else None
        cpu_values = [run.get("idle_cpu", {}).get("percent_of_one_core") for run in selected]
        combined_pss, app_swaps, combined_swaps, server_swaps, server_deltas = [], [], [], [], []
        for run in selected:
            server = run.get("xserver", {}).get("after", {})
            combined_pss.append(combine(run["application"], server, "Pss_bytes"))
            server_swaps.append(server.get("Swap_bytes"))
            server_deltas.append(run.get("xserver", {}).get("delta", {}).get("Pss_bytes"))
            samples = run.get("memory_samples")
            if not samples:
                app_swaps.append(None)
                combined_swaps.append(None)
            for sample in samples or []:
                app_swaps.append(sample["application"].get("Swap_bytes"))
                combined_swaps.append(combine(sample["application"], server, "Swap_bytes"))
        combined_pss = available(combined_pss)
        app_swaps = available(app_swaps)
        combined_swaps = available(combined_swaps)
        rows.append({
            "app": app, "windows": count, "mode": mode, "workload": workload,
            "successful_runs": len(selected),
            "median_rss_bytes": median_value("Rss_bytes"),
            "median_pss_bytes": median_value("Pss_bytes"),
            "median_private_bytes": median_value("Private_bytes"),
            "median_idle_cpu_percent": statistics.median(cpu_values)
                if all(value is not None for value in cpu_values) else None,
            "median_swap_bytes": median_value("Swap_bytes"),
            "median_app_plus_xserver_pss_bytes": statistics.median(combined_pss)
                if combined_pss is not None else None,
            "app_plus_xserver_pss_range_bytes": extent(combined_pss),
            "sampled_application_swap_range_bytes": extent(app_swaps),
            "max_sampled_application_swap_bytes": max(app_swaps) if app_swaps is not None else None,
            "sampled_app_plus_xserver_swap_range_bytes": extent(combined_swaps),
            "max_sampled_app_plus_xserver_swap_bytes": max(combined_swaps)
                if combined_swaps is not None else None,
            "xserver_after_swap_range_bytes": extent(available(server_swaps)),
            "xserver_delta_pss_range_bytes": extent(available(server_deltas, signed=True)),
            "median_latency_p50_us": statistics.median(run["latency_p50_us"] for run in selected),
            "median_latency_p95_us": statistics.median(run["latency_p95_us"] for run in selected),
            "median_output_mib_per_second": statistics.median(run["output_mib_per_second"] for run in selected),
            "pss_range_bytes": [min(run["application"]["Pss_bytes"] for run in selected),
                                max(run["application"]["Pss_bytes"] for run in selected)],
            "latency_p50_range_us": [min(run["latency_p50_us"] for run in selected),
                                     max(run["latency_p50_us"] for run in selected)],
            "output_range_mib_per_second": [min(run["output_mib_per_second"] for run in selected),
                                            max(run["output_mib_per_second"] for run in selected)],
            "gui_process_counts": sorted({run["gui_process_count"] for run in selected}),
            "application_process_counts": sorted({
                count for run in selected for count in run["application_process_counts"]
            }),
        })
    return rows


def validate_report(report):
    if not report.get("ok") or report.get("schema") not in (1, 2):
        raise ValueError("refusing to render incomplete or unsupported comparison data")
    settings = report["settings"]
    counts = settings.get("counts", [settings.get("windows", WINDOWS)])
    modes = settings.get("modes", ["shared"])
    workloads = settings.get("workloads", ["idle"])
    apps = settings.get("apps", list(APPS))
    repetitions = settings["repetitions"]
    if type(repetitions) is not int or not 1 <= repetitions <= 20:
        raise ValueError("invalid repetition count")
    if not counts or any(type(count) is not int or not 1 <= count <= 20 for count in counts):
        raise ValueError("invalid window counts")
    for values, allowed in ((modes, MODES), (workloads, WORKLOADS), (apps, APPS)):
        if not values or len(values) != len(set(values)) or not set(values).issubset(allowed):
            raise ValueError("invalid matrix selection")
    if len(counts) != len(set(counts)):
        raise ValueError("duplicate window counts")
    expected = {(count, mode, workload, app) for count in counts for mode in modes
                for workload in workloads for app in apps}
    runs = report.get("runs", [])
    if (any(not run.get("ok") or run.get("error") or run.get("cleanup_error")
            or run.get("remaining_owned_pids") for run in runs)
            or {group_key(run) for run in runs} != expected):
        raise ValueError("comparison is missing requested matrix cells or contains failed runs")
    for run in runs:
        measured = [run["application"][key] for key in ("Rss_bytes", "Pss_bytes", "Private_bytes")]
        measured.extend(run[key] for key in ("latency_p50_us", "latency_p95_us", "output_mib_per_second"))
        if any(type(value) not in (int, float) or not math.isfinite(value) or value <= 0 for value in measured):
            raise ValueError("required measurements must be finite positive numbers")
    summarize(runs, repetitions)
    return repetitions


def overview_available(report):
    if report.get("purpose", "comparison") != "comparison":
        return False
    return {run["app"] for run in report.get("runs", [])
            if group_key(run)[:3] == (10, "shared", "idle")} == set(APPS)


def refresh_summary(report):
    # Collection identity and every raw observation remain untouched.
    repetitions = validate_report(report)
    report["summary"] = summarize(report["runs"], repetitions)
    report["derived"] = {
        "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "renderer_script_sha256": sha256(__file__),
        "source_runs_sha256": hashlib.sha256(
            json.dumps(report["runs"], sort_keys=True).encode()).hexdigest(),
        "combined_pss_formula": f"median across {repetitions} runs within each count/mode/workload/application cell of (run.application.Pss_bytes + run.xserver.after.Pss_bytes); includes full dedicated Xvfb baseline",
        "swap_formula": "range/max over every recorded application memory snapshot; for the combined counter add xserver.after.Swap_bytes from the same run before taking range/max",
        "scope": "resident PSS excludes swap; Swap counters remain separate and are not added to PSS as a unique physical-memory total",
        "missing_counters": "Derived values remain null if any required counter or sampled snapshot is unavailable; missing values are never zero-filled.",
    }

def svg_document(report, source_path=REPORT):
    source_path = Path(source_path)
    if source_path.is_absolute():
        try:
            source_path = source_path.relative_to(ROOT)
        except ValueError:
            pass
    repetitions = validate_report(report)
    if not overview_available(report):
        raise ValueError("README overview requires comparison data")
    focused = [run for run in report["runs"] if group_key(run)[:3] == (10, "shared", "idle")]
    if {run["app"] for run in focused} != set(APPS):
        raise ValueError("ten-window overview requires all three apps in shared idle mode")
    rows = summarize(focused, repetitions)
    by_app = {row["app"]: row for row in rows}
    colors = {"ztty": "#1a73e8", "ghostty": "#00897b", "kitty": "#8754c8"}
    parts = [
        '<svg xmlns="http://www.w3.org/2000/svg" width="1120" height="790" viewBox="0 0 1120 790" role="img" aria-labelledby="title desc">',
        '<title id="title">Ten terminal windows: memory and real PTY performance</title>',
        f'<desc id="desc">{repetitions} fresh-process runs per terminal, all using shared instances. '
        'Application PSS excludes the display server; a second panel adds full dedicated Xvfb PSS. Probe workloads and private D-Bus are separate. '
        'Latency and throughput use DSR logical completion, not a rendered-frame fence.</desc>',
        '<style>'
        'text{font-family:Arial,Helvetica,sans-serif;fill:#e8eaed}'
        '.muted{fill:#bdc1c6}.h{font-size:20px;font-weight:600}.small{font-size:13px}'
        '.value{font-size:16px;font-weight:600}.label{font-size:15px}'
        '</style>',
        '<rect width="1120" height="790" rx="18" fill="#17191d"/>',
        '<text x="36" y="47" font-size="28" font-weight="600">10 windows. Shared instances. Same workload.</text>',
        f'<text x="36" y="76" class="muted" font-size="15">Linux X11 / isolated Xvfb · software rendering · medians of {repetitions} fresh-process runs</text>',
    ]
    panels = [
        (36, 102, "Application PSS", "MiB resident · lower is better", "median_pss_bytes", 1024 * 1024),
        (580, 102, "App + full X server PSS", "MiB resident · lower is better", "median_app_plus_xserver_pss_bytes", 1024 * 1024),
        (36, 355, "PTY round-trip latency (p50)", "µs · lower is better", "median_latency_p50_us", 1),
        (580, 355, "Output processing throughput", "MiB/s · higher is better", "median_output_mib_per_second", 1),
    ]
    for x, y, title, subtitle, key, divisor in panels:
        parts.extend([
            f'<rect x="{x}" y="{y}" width="504" height="229" rx="12" fill="#202329" stroke="#41464d"/>',
            f'<text x="{x + 20}" y="{y + 32}" class="h">{html.escape(title)}</text>',
            f'<text x="{x + 20}" y="{y + 55}" class="small muted">{html.escape(subtitle)}</text>',
        ])
        if any(type(by_app[app][key]) not in (int, float) or not math.isfinite(by_app[app][key])
               or by_app[app][key] <= 0 for app in APPS):
            raise ValueError(f"{title} is unavailable or invalid in the measured data")
        values = {app: by_app[app][key] / divisor for app in APPS}
        maximum = max(values.values())
        if maximum <= 0 or not all(value > 0 for value in values.values()):
            raise ValueError("measured metric must be positive")
        for index, app in enumerate(APPS):
            yy = y + 93 + index * 49
            width = 295 * values[app] / maximum
            label = f"{values[app]:.2f}" if values[app] < 100 else f"{values[app]:.1f}"
            parts.extend([
                f'<text x="{x + 20}" y="{yy + 5}" class="label">{NAMES[app]}</text>',
                f'<rect x="{x + 93}" y="{yy - 12}" width="295" height="22" rx="4" fill="#30343b"/>',
                f'<rect x="{x + 93}" y="{yy - 12}" width="{width:.3f}" height="22" rx="4" fill="{colors[app]}"/>',
                f'<text x="{x + 484}" y="{yy + 5}" text-anchor="end" class="value">{label}</text>',
            ])
    date = report["timestamp"][:10] + " UTC"
    versions = []
    for app in APPS:
        first = report["binaries"][app]["version"].get("stdout", "").splitlines()
        versions.append(first[0] if first else NAMES[app])
    swap_line = "Max sampled Swap (app + X server counters): " + " · ".join(
        NAMES[app] + " " + (f"{by_app[app]['max_sampled_app_plus_xserver_swap_bytes'] / (1024 * 1024):.2f} MiB"
                          if by_app[app]['max_sampled_app_plus_xserver_swap_bytes'] is not None else "unavailable")
        for app in APPS)
    footer = [
        "App PSS counts all terminal processes. The second panel includes the full dedicated 7680×2160 Xvfb baseline.",
        "PTY probes and private D-Bus are separate. PSS covers resident pages only; this is not total GPU/VRAM accounting.",
        f"One active window, nine idle; write → DSR reply. Logical completion, not frame rate. Memory: fixed frame + {report['settings']['memory_settle_seconds']:g}s settle.",
        swap_line,
        " · ".join(versions) + " · " + date + " · 120×32 cells · monospace 11 pt",
        f"Raw samples, versions, hashes and scope: {source_path}",
    ]
    loads = [run[key][0] for run in focused
             for key in ("load_average_at_start", "load_average_at_end") if run.get(key)]
    if loads:
        cores = report.get("machine", {}).get("logical_cpus")
        line = f"Recorded host load1: {min(loads):.1f}–{max(loads):.1f}"
        if cores:
            line += f"; {cores} logical CPUs."
            if max(loads) > cores / 2:
                line += " Timing samples were collected on a busy host."
        footer.append(line)
    for index, line in enumerate(footer):
        parts.append(f'<text x="36" y="{625 + index * 25}" class="small muted">{html.escape(line)}</text>')
    parts.append("</svg>")
    return "\n".join(parts) + "\n"


def atomic_write(path, content):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(content)
    os.replace(temporary, path)


def scaling_svg_document(report):
    repetitions = validate_report(report)
    rows = summarize(report["runs"], repetitions)
    present = {(row["workload"], row["mode"]) for row in rows}
    panels = [(workload, mode) for workload in WORKLOADS for mode in MODES
              if (workload, mode) in present]
    height = 120 + ((len(panels) + 1) // 2) * 310 + 90
    colors = {"ztty": "#72a7ff", "ghostty": "#6dcbb1", "kitty": "#c09aff"}
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="1120" height="{height}" '
        f'viewBox="0 0 1120 {height}" role="img" aria-labelledby="title desc">',
        '<title id="title">Terminal memory scaling by window count and process mode</title>',
        '<desc id="desc">Measured application PSS in MiB. Each panel has its own vertical scale. '
        'PTY child workloads and Xvfb memory are recorded separately.</desc>',
        '<style>text{font-family:Arial,Helvetica,sans-serif;fill:#e8eaed}.muted{fill:#aeb5c2}</style>',
        f'<rect width="1120" height="{height}" fill="#111318"/>',
        '<text x="36" y="43" font-size="27">Terminal memory scaling'
        + (' — validation run' if report.get("purpose") == "smoke" else '') + '</text>',
        f'<text x="36" y="73" font-size="14" class="muted">Application PSS · MiB · '
        f'{repetitions} fresh-process {"run" if repetitions == 1 else "runs"} · isolated X11 software rendering</text>',
    ]
    for index, (workload, mode) in enumerate(panels):
        x, y = 36 + index % 2 * 548, 110 + index // 2 * 310
        values = [row for row in rows if (row["workload"], row["mode"]) == (workload, mode)]
        counts = sorted({row["windows"] for row in values})
        maximum = max(row["median_pss_bytes"] for row in values) / (1024 * 1024) * 1.12
        if maximum <= 0:
            raise ValueError("PSS must be positive")
        parts.append(f'<text x="{x}" y="{y}" font-size="18">{html.escape(workload)} / {mode}</text>')
        left, top, width, plot_height = x + 48, y + 30, 440, 170
        def point(row):
            xx = left + width * (row["windows"] - min(counts)) / max(1, max(counts) - min(counts))
            yy = top + plot_height * (1 - row["median_pss_bytes"] / (1024 * 1024) / maximum)
            return xx, yy
        for step in range(4):
            value = maximum * step / 3
            yy = top + plot_height * (1 - step / 3)
            parts.extend([
                f'<line x1="{left}" x2="{left + width}" y1="{yy:.2f}" y2="{yy:.2f}" stroke="#343944"/>',
                f'<text x="{left - 8}" y="{yy + 4:.2f}" text-anchor="end" font-size="12" class="muted">{value:.1f}</text>',
            ])
        for count in counts:
            xx = left + width * (count - min(counts)) / max(1, max(counts) - min(counts))
            parts.append(f'<text x="{xx:.2f}" y="{top + plot_height + 22}" text-anchor="middle" '
                         f'font-size="12" class="muted">{count}</text>')
        for app_index, app in enumerate(APPS):
            selected = sorted((row for row in values if row["app"] == app), key=lambda row: row["windows"])
            if not selected:
                continue
            points = " ".join(f"{xx:.2f},{yy:.2f}" for xx, yy in map(point, selected))
            parts.append(f'<polyline points="{points}" fill="none" stroke="{colors[app]}" stroke-width="2"/>')
            for row in selected:
                xx, yy = point(row)
                label = f'{NAMES[app]}, {row["windows"]} windows: {row["median_pss_bytes"] / (1024 * 1024):.2f} MiB'
                parts.append(f'<circle cx="{xx:.2f}" cy="{yy:.2f}" r="3.5" fill="{colors[app]}">'
                             f'<title>{html.escape(label)}</title></circle>')
            parts.append(f'<text x="{x + app_index * 169}" y="{y + 257}" font-size="13" '
                         f'style="fill:{colors[app]}">{NAMES[app]} '
                         f'{selected[-1]["median_pss_bytes"] / (1024 * 1024):.2f} MiB</text>')
    parts.append(f'<text x="36" y="{height - 61}" font-size="13" class="muted">'
                 'Panel scales are independent. Legends show the largest window count. '
                 'Native GPU memory and display latency remain unmeasured.</text>')
    swaps = [row["max_sampled_application_swap_bytes"] for row in rows]
    maximum_swap = max(swaps) if swaps and all(value is not None for value in swaps) else None
    swap_note = (f'Maximum sampled application Swap: {maximum_swap / (1024 * 1024):.2f} MiB.'
                 if maximum_swap is not None else 'Application Swap counters are unavailable for part of this dataset.')
    parts.append(f'<text x="36" y="{height - 36}" font-size="13" class="muted">'
                 + html.escape(swap_note + ' Resident PSS excludes swap; paging can lower resident memory values.') + '</text>')
    parts.append('</svg>')
    return "\n".join(parts) + "\n"


def update_readmes(repetitions=REPETITIONS):
    entries = [
        ("README.md",
         "![Ten-window terminal memory and PTY performance comparison](docs/benchmarks/terminals-10-windows.svg)\n\n"
         f"Ten windows per terminal, shared instances, {repetitions} real runs. "
         "[Method and rerun command](docs/benchmarks/README.md) · "
         "[Raw measurements](docs/benchmarks/terminals-10-windows.json)\n\n"
         "Rerun from current source: `make benchmark-terminals`.\n"),
        ("README.zh-CN.md",
         "![十窗口终端内存与 PTY 性能比较](docs/benchmarks/terminals-10-windows.svg)\n\n"
         f"每个终端均开启 10 个窗口、使用共享实例，实测 {repetitions} 轮。"
         "[测量方法与复跑命令](docs/benchmarks/README.md) · "
         "[原始数据](docs/benchmarks/terminals-10-windows.json)\n\n"
         "从当前源码重新构建并实测：`make benchmark-terminals`。\n"),
    ]
    start, end = "<!-- terminal-benchmark:start -->", "<!-- terminal-benchmark:end -->"
    for filename, content in entries:
        path = ROOT / filename
        text = path.read_text()
        block = start + "\n" + content + end
        if start in text:
            text = re.sub(re.escape(start) + ".*?" + re.escape(end), lambda _: block,
                          text, count=1, flags=re.DOTALL)
        else:
            title, rest = text.split("\n", 1)
            if not title.startswith("# "):
                raise ValueError(f"{filename} must begin with the project title")
            text = title + "\n\n" + block + "\n" + rest
        atomic_write(path, text)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ztty", type=Path, default=ROOT / "target/release/ztty")
    parser.add_argument("--ztty-renderer", choices=("auto", "gpu", "software"), default="auto",
                        help="record the selected path; gpu requires the real EGL/GLES renderer")
    parser.add_argument("--ghostty", type=Path)
    parser.add_argument("--kitty", type=Path)
    parser.add_argument("--json", type=Path, default=REPORT)
    parser.add_argument("--svg", type=Path, default=SVG)
    parser.add_argument("--matrix", action="store_true", help="measure 1,5,10,20 windows in shared and independent modes")
    parser.add_argument("--counts", help="comma-separated window counts, 1 through 20")
    parser.add_argument("--modes", help="comma-separated shared,independent process modes")
    parser.add_argument("--workloads", default="idle", help="comma-separated idle,history,images,hidden")
    parser.add_argument("--apps", default=",".join(APPS), help="comma-separated ztty,ghostty,kitty")
    parser.add_argument("--repetitions", type=int, default=REPETITIONS)
    parser.add_argument("--purpose", choices=("comparison", "smoke"), default="comparison",
                        help="smoke validates the harness under load and cannot update README comparisons")
    parser.add_argument("--render-only", action="store_true",
                        help="recompute summary and SVG from completed raw JSON without running terminals")
    parser.add_argument("--settle-seconds", type=float, default=1.0)
    parser.add_argument("--no-readme", action="store_true", help="leave README image references unchanged")
    args = parser.parse_args()
    if args.purpose == "smoke" and not args.no_readme:
        parser.error("smoke runs require --no-readme")
    if not args.no_readme and (args.json.resolve() != REPORT.resolve() or args.svg.resolve() != SVG.resolve()):
        parser.error("custom output paths require --no-readme to preserve README evidence links")
    try:
        counts = [int(value) for value in (args.counts or ("1,5,10,20" if args.matrix else "10")).split(",")]
    except ValueError:
        parser.error("counts must be comma-separated integers")
    if not counts or len(counts) != len(set(counts)) or any(value < 1 or value > 20 for value in counts):
        parser.error("counts must be distinct integers from 1 through 20")
    modes = (args.modes or ("shared,independent" if args.matrix else "shared")).split(",")
    workloads = args.workloads.split(",")
    apps = args.apps.split(",")
    for name, values, allowed in (("modes", modes, MODES), ("workloads", workloads, WORKLOADS), ("apps", apps, APPS)):
        if not values or len(values) != len(set(values)) or any(value not in allowed for value in values):
            parser.error(name + " must select distinct values from " + ",".join(allowed))
    if not 1 <= args.repetitions <= 20:
        parser.error("repetitions must be from 1 through 20")
    scaling_path = args.svg.with_name(args.svg.stem + "-scaling.svg")
    if args.render_only:
        report = json.loads(args.json.read_text())
        refresh_summary(report)
        overview = overview_available(report)
        if not args.no_readme and not overview:
            parser.error("README overview requires all three apps at 10 shared idle windows; use --no-readme")
        scaling = scaling_svg_document(report)
        svg = svg_document(report, args.json) if overview else scaling
        atomic_write(args.json, json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        atomic_write(args.svg, svg)
        atomic_write(scaling_path, scaling)
        if not args.no_readme:
            update_readmes(report["settings"]["repetitions"])
        return 0
    if not 0.1 <= args.settle_seconds <= 10:
        parser.error("settle-seconds must be from 0.1 through 10")
    if not args.no_readme and not (
            10 in counts and "shared" in modes and "idle" in workloads and set(apps) == set(APPS)):
        parser.error("README overview requires all three apps at 10 shared idle windows; use --no-readme")
    for program in ("Xvfb", "dbus-daemon"):
        if not shutil.which(program):
            parser.error(program + " is required; existing results were not changed")
    try:
        import Xlib  # noqa: F401
        resolvers = {
            "ztty": lambda: args.ztty.resolve(strict=True),
            "ghostty": lambda: ghostty_binary(args.ghostty),
            "kitty": lambda: resolve_binary(args.kitty, "kitty"),
        }
        binaries = {app: resolvers[app]() for app in apps}
    except (ImportError, FileNotFoundError) as error:
        parser.error(str(error) + "; existing results were not changed")
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("cannot enable child subreaper")
    metadata = {app: binary_metadata(path) for app, path in binaries.items()}
    payload = output_payload()
    report = {
        "schema": 2, "ok": False, "purpose": args.purpose,
        "timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "machine": machine(), "command": [sys.executable, str(Path(__file__).resolve()), *sys.argv[1:]],
        "script_sha256": sha256(__file__),
        "source_sha256": {name: sha256(ROOT / "tools" / name) for name in
                          ("benchmark_terminals.py", "measure_runtime.py", "measure_multiwindow.py", "isolated_xvfb.py", "gui_renderer.py")},
        "git": {
            "head": capture(["git", "-C", str(ROOT), "rev-parse", "HEAD"]),
            "status": capture(["git", "-C", str(ROOT), "status", "--short"]),
        },
        "binaries": metadata,
        "settings": {
            "windows": counts[0] if len(counts) == 1 else None,
            "counts": counts, "modes": modes, "workloads": workloads, "apps": apps,
            "repetitions": args.repetitions, "geometry": [COLS, ROWS],
            "ztty_renderer": args.ztty_renderer,
            "font": "monospace", "font_size_pt": 11, "xvfb_screen": "7680 pixels wide; height grows to tile selected window count", "dpi": 96,
            "sharing": {"ztty": "same broker socket in shared mode; separate socket per window in independent mode",
                        "kitty": "--single-instance + unique --instance-group only in shared mode",
                        "ghostty": "gtk-single-instance=true/false + unique class + private D-Bus"},
            "history": {"ztty_lines": 2000, "kitty_lines": 2000, "ghostty_bytes": 10000000,
                        "contents": "history workload writes 100000 numbered rows; other workloads keep empty history"},
            "workload_details": {
                "idle": "fixed UTF-8/color frame; no scrolling",
                "history": "100000 numbered rows (11.9 MB input); configured history capacities have different units",
                "images": "one 256x256 raw RGBA Kitty image per window; successful OK reply required",
                "hidden": "all OS windows except one unmapped after initialization; not a tab benchmark",
            },
            "graphics_payload_sha256": hashlib.sha256(graphics_payload()).hexdigest(),
            "font_warmup_frame_utf8": FRAME.decode(),
            "font_warmup_frame_sha256": hashlib.sha256(FRAME).hexdigest(),
            "memory_settle_seconds": args.settle_seconds,
            "memory_samples_per_run": MEMORY_SAMPLES,
            "latency_command_hex": LATENCY_COMMAND.hex(), "latency_reply_hex": LATENCY_REPLY.hex(),
            "latency_warmup_per_window": LATENCY_WARMUP, "latency_samples_per_window": LATENCY_SAMPLES,
            "output_bytes_per_sample": len(payload), "output_sha256": hashlib.sha256(payload).hexdigest(),
            "output_warmup_per_window": OUTPUT_WARMUP, "output_samples_per_window": OUTPUT_SAMPLES,
            "untimed_output_check_command_hex": OUTPUT_CHECK.hex(),
            "untimed_output_check_reply_hex": OUTPUT_CHECK_REPLY.hex(),
        },
        "method": {
            "application_memory": "sum smaps_rollup PSS and Private_Clean+Private_Dirty for every marked terminal process and helper, excluding known probe subtrees and isolated infrastructure",
            "memory_aggregation": "median of three process snapshots per run; median of requested fresh-process repetitions per matrix cell",
            "latency_aggregation": "per-run p50/p95 across window_count x 300 measured DSR round trips; median of requested repetitions",
            "output_aggregation": "median of three 1 MiB output bursts per window, then median of selected windows and repetitions",
            "performance_scope": "one PTY producer at a time; visible windows are focused, hidden windows remain unmapped; write-to-DSR logical processing completion",
            "output_integrity": "after timed samples, replay the same payload outside timing and require direct CPR at row 32 column 120; ztty also checks all final screen text through its private broker; this is not a physical frame fence",
            "cold_warm": "fresh terminal processes and isolated config per run; memory sampled after fixed frame and font warm-up; OS page/font caches are not flushed",
            "graphics_scope": "Xvfb software rendering only; X-server PSS delta and XRes client pixmaps recorded separately; does not quantify native GPU/VRAM or desktop compositor behavior",
            "window_visibility": "windows tiled on a private X display; hidden workload retains all windows but maps only one",
            "desktop_services": "private EXTERNAL-authenticated D-Bus without service activation directories; GTK accessibility disabled; portal/accessibility desktop services are outside this isolated configuration",
            "ordering": "app order rotates each repetition; one app/count/mode/workload cell runs at a time",
            "limitations": "single machine; history capacities have different units; startup waits for DSR, not first presentation; native GPU VRAM and input-to-display latency unavailable",
        },
        "sources": [
            "https://ghostty.org/docs/config/reference",
            "https://docs.gtk.org/gtk4/running.html",
            "https://dbus.freedesktop.org/doc/dbus-daemon.1.html",
            "https://ghostty.org/docs/install/binary",
            "https://sw.kovidgoyal.net/kitty/invocation/",
            "https://sw.kovidgoyal.net/kitty/graphics-protocol/",
            "https://man7.org/linux/man-pages/man5/proc_pid_smaps.5.html",
            "https://xorg.freedesktop.org/releases/X11R6.9.0/doc/html/XResQueryClientPixmapBytes.3.html",
        ],
        "runs": [],
    }
    for repetition in range(args.repetitions):
        offset = repetition % len(apps)
        order = apps[offset:] + apps[:offset]
        for count in counts:
            for mode in modes:
                for workload in workloads:
                    for app in order:
                        if sha256(binaries[app]) != metadata[app]["sha256"]:
                            raise RuntimeError(f"{app} binary changed during benchmark")
                        run = measure(app, binaries[app], repetition + 1, args.settle_seconds,
                                      count=count, mode=mode, workload=workload, purpose=args.purpose,
                                      ztty_renderer=args.ztty_renderer)
                        report["runs"].append(run)
                        print(json.dumps({
                            "app": app, "windows": count, "mode": mode, "workload": workload,
                            "repetition": repetition + 1, "ok": run["ok"],
                            "pss_mib": run["application"]["Pss_bytes"] / (1024 * 1024)
                                if run.get("application") else None,
                            "latency_p50_us": run.get("latency_p50_us"),
                            "output_mib_per_second": run.get("output_mib_per_second"),
                            "error": run.get("error"),
                        }), flush=True)
                        if not run["ok"]:
                            failure = ROOT / "artifacts" / ("terminal-benchmark-failed-" + str(time.time_ns()) + ".json")
                            atomic_write(failure, json.dumps(report, ensure_ascii=False, indent=2) + "\n")
                            print("Benchmark failed; prior JSON/SVG preserved. Diagnostic: " + str(failure),
                                  file=sys.stderr)
                            return 1
    report["completed_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    report["ok"] = True
    refresh_summary(report)
    scaling = scaling_svg_document(report)
    svg = svg_document(report, args.json) if overview_available(report) else scaling
    # Validate and build everything before replacing either tracked artifact.
    serialized = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    atomic_write(args.json, serialized)
    atomic_write(args.svg, svg)
    atomic_write(scaling_path, scaling)
    if not args.no_readme:
        update_readmes(args.repetitions)
    print(json.dumps({"ok": True, "json": str(args.json), "svg": str(args.svg),
                      "scaling_svg": str(scaling_path),
                      "summary": report["summary"]}), flush=True)
    return 0


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--child":
        raise SystemExit(probe(sys.argv[2]))
    raise SystemExit(main())
