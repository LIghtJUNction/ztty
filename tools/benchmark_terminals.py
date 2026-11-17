#!/usr/bin/env python3
"""Measure ten real terminal windows, then atomically publish raw JSON and SVG.

Run from the repository: python3 tools/benchmark_terminals.py
No global terminal installation or default-terminal changes are performed.
Requires Linux /proc, Xvfb, python-xlib, dbus-daemon, Kitty, and Ghostty.
"""
import argparse
import contextlib
import ctypes
import datetime
import hashlib
import html
import json
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

ROOT = Path(__file__).resolve().parents[1]
REPORT = ROOT / "docs/benchmarks/terminals-10-windows.json"
SVG = ROOT / "docs/benchmarks/terminals-10-windows.svg"
WINDOWS, COLS, ROWS = 10, 120, 32
REPETITIONS = 3
LATENCY_WARMUP, LATENCY_SAMPLES = 50, 300
OUTPUT_WARMUP, OUTPUT_SAMPLES = 1, 3
OUTPUT_TARGET = 1024 * 1024
MEMORY_SAMPLES = 3
MARKER = "ZTTY_TERMINAL_BENCHMARK"
FIELDS = (
    "Rss_bytes", "Pss_bytes", "Pss_Anon_bytes", "Pss_File_bytes",
    "Pss_Shmem_bytes", "Private_Clean_bytes", "Private_Dirty_bytes", "Swap_bytes",
)
APPS = ("ztty", "ghostty", "kitty")
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
    "\x1b[1;36mTerminal benchmark: ten shared-instance windows\x1b[0m\r\n"
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
                write_all(FRAME)
                exchange(BARRIER, BARRIER_REPLY)
                send_json(connection, {
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
    result = {key: sum(sample.get(key, 0) for sample in samples) for key in FIELDS}
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
    return result


def application_windows(connection, prefix):
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
            if window.get_attributes().map_state != X.IsViewable:
                continue
            owner = window.get_full_property(pid_atom, 0)
            if owner is None:
                raise RuntimeError("terminal window has no _NET_WM_PID")
            geometry = window.get_geometry()
            result[title] = {
                "id": window.id, "pid": int(owner.value[0]), "window": window,
                "title": title, "width": geometry.width, "height": geometry.height,
                "depth": geometry.depth, "wm_class": window.get_wm_class(),
            }
        except (OSError, AttributeError):
            continue
    return result


def terminal_command(app, binary, directory, tag):
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
        return [str(binary), "--config", "NONE", *options,
                "--single-instance", "--instance-group", tag, wrapper]
    return [
        str(binary), "--config-default-files=false",
        "--config-file=" + str(directory / "ghostty.conf"),
        "--class=org.ztty.benchmark." + tag.replace("-", "_"),
        "--gtk-single-instance=true", "--command=" + wrapper,
    ]


def measure(app, binary, repetition, settle):
    from Xlib import X, display
    result = {"app": app, "windows": WINDOWS, "repetition": repetition, "ok": False,
              "load_average_at_start": list(os.getloadavg())}
    launchers, connections, identities = [], [], {}
    xvfb = bus = xconnection = listener = None
    with tempfile.TemporaryDirectory(prefix="ztty-compare-") as temporary:
        directory = Path(temporary)
        tag = directory.name
        prefix = "terminal-benchmark-" + tag + "-"
        (directory / "ztty.conf").write_text(ZTTY_CONFIG)
        (directory / "ghostty.conf").write_text(GHOSTTY_CONFIG)
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
                read_fd, write_fd = os.pipe()
                try:
                    xvfb = subprocess.Popen(
                        ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "7680x2160x24",
                         "-dpi", "96", "-nolisten", "tcp"],
                        pass_fds=(write_fd,), stdout=subprocess.DEVNULL, stderr=xlog,
                        env=environment, start_new_session=True,
                    )
                finally:
                    os.close(write_fd)
                try:
                    if not select.select([read_fd], [], [], 10)[0]:
                        raise TimeoutError("Xvfb did not allocate a display")
                    number = os.read(read_fd, 64).strip().decode("ascii")
                    if not number.isdecimal():
                        raise RuntimeError("invalid Xvfb display number")
                finally:
                    os.close(read_fd)
                environment["DISPLAY"] = display_name = ":" + number
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
                listener.listen(WINDOWS)
                listener.settimeout(30)
                command = terminal_command(app, binary, directory, tag)
                result["command"] = command
                result["config"] = {
                    "ztty": ZTTY_CONFIG, "kitty": KITTY_CONFIG, "ghostty": GHOSTTY_CONFIG,
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
                for index in range(WINDOWS):
                    launchers.append(subprocess.Popen(
                        command, env=environment, stdout=subprocess.DEVNULL,
                        stderr=tlog, start_new_session=True,
                    ))
                    channel, _ = listener.accept()
                    channel.settimeout(90)
                    connections.append(channel)
                    hello = receive_json(channel)
                    title = prefix + str(index + 1)
                    send_json(channel, {"title": title})
                    ready = receive_json(channel)
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
                    probes.append({"index": index + 1, "title": title, **ready})
                result["ten_windows_ready_seconds"] = time.monotonic() - started
                # DSR establishes terminal state; this settling interval is only
                # for font/resource warm-up before memory sampling, not a timer barrier.
                time.sleep(settle)
                windows = application_windows(xconnection, prefix)
                if len(windows) != WINDOWS:
                    raise RuntimeError(f"expected {WINDOWS} mapped windows, found {len(windows)}")
                owners = sorted({row["pid"] for row in windows.values()})
                if len(owners) != 1:
                    raise AssertionError(f"{app} sharing failed: {len(owners)} GUI owner processes")
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
                    for key in memory_snapshots[0]["application"]
                }
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
                    fonts={str(pid): font_files(pid) for pid in owners},
                    pty_children_total=memory_snapshots[-1]["pty_children_total"],
                    xserver={"before": server_before, "after": server_after,
                             "delta": {key: server_after[key] - server_before[key] for key in FIELDS}},
                    private_dbus=sample_process(bus.pid, process_stat(bus.pid)["start_ticks"]),
                    pixmap_clients=graphics,
                    pixmap_total_bytes=sum(row["bytes"] for row in graphics)
                        if all(row["available"] for row in graphics) else None,
                )
                performance = []
                for index, channel in enumerate(connections):
                    window = windows[prefix + str(index + 1)]["window"]
                    window.configure(stack_mode=X.Above)
                    window.set_input_focus(X.RevertToParent, X.CurrentTime)
                    xconnection.sync()
                    send_json(channel, {"op": "performance"})
                    values = receive_json(channel)
                    performance.append({"index": index + 1, **values})
                latency = [value for row in performance for value in row["latency_us"]]
                result["performance"] = performance
                result["latency_p50_us"] = statistics.median(latency)
                result["latency_p95_us"] = percentile(latency, 95)
                result["latency_p99_us"] = percentile(latency, 99)
                result["output_mib_per_second"] = statistics.median(
                    row["median_output_mib_per_second"] for row in performance)
                if len(application_windows(xconnection, prefix)) != WINDOWS:
                    raise RuntimeError("window count changed during performance measurements")
                result["ok"] = True
                for channel in connections:
                    send_json(channel, {"op": "quit"})
            except Exception as error:
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
                for process in (bus, xvfb):
                    if process is not None:
                        process.terminate()
                        try:
                            process.wait(timeout=2)
                        except subprocess.TimeoutExpired:
                            process.kill()
                            process.wait(timeout=2)
                remaining.extend(pid for pid in owned_processes(tag) if pid not in remaining)
                result["remaining_owned_pids"] = remaining
                if remaining:
                    result.update(ok=False, cleanup_error="owned processes remained after teardown")
        result["terminal_stderr"] = (directory / "terminal.log").read_text(errors="replace")[-12000:]
        result["dbus_stderr"] = (directory / "dbus.log").read_text(errors="replace")[-4000:]
        result["load_average_at_end"] = list(os.getloadavg())
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


def summarize(runs):
    rows = []
    for app in APPS:
        selected = [run for run in runs if run["app"] == app and run["ok"]]
        if len(selected) != REPETITIONS:
            raise ValueError(f"{app} has {len(selected)} completed runs; requires {REPETITIONS}")
        combined_pss = [run["application"]["Pss_bytes"] + run["xserver"]["after"]["Pss_bytes"] for run in selected]
        app_swaps = [sample["application"]["Swap_bytes"] for run in selected for sample in run["memory_samples"]]
        combined_swaps = [sample["application"]["Swap_bytes"] + run["xserver"]["after"]["Swap_bytes"]
                          for run in selected for sample in run["memory_samples"]]
        rows.append({
            "app": app, "windows": WINDOWS, "successful_runs": len(selected),
            "median_pss_bytes": statistics.median(run["application"]["Pss_bytes"] for run in selected),
            "median_app_plus_xserver_pss_bytes": statistics.median(combined_pss),
            "app_plus_xserver_pss_range_bytes": [min(combined_pss), max(combined_pss)],
            "sampled_application_swap_range_bytes": [min(app_swaps), max(app_swaps)],
            "max_sampled_application_swap_bytes": max(app_swaps),
            "sampled_app_plus_xserver_swap_range_bytes": [min(combined_swaps), max(combined_swaps)],
            "max_sampled_app_plus_xserver_swap_bytes": max(combined_swaps),
            "xserver_after_swap_range_bytes": [
                min(run["xserver"]["after"]["Swap_bytes"] for run in selected),
                max(run["xserver"]["after"]["Swap_bytes"] for run in selected),
            ],
            "xserver_delta_pss_range_bytes": [
                min(run["xserver"]["delta"]["Pss_bytes"] for run in selected),
                max(run["xserver"]["delta"]["Pss_bytes"] for run in selected),
            ],
            "median_private_bytes": statistics.median(run["application"]["Private_bytes"] for run in selected),
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


def refresh_summary(report):
    # Collection identity and every raw observation remain untouched.
    report["summary"] = summarize(report["runs"])
    report["derived"] = {
        "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "renderer_script_sha256": sha256(__file__),
        "source_runs_sha256": hashlib.sha256(
            json.dumps(report["runs"], sort_keys=True).encode()).hexdigest(),
        "combined_pss_formula": "median across three runs of (run.application.Pss_bytes + run.xserver.after.Pss_bytes); includes full dedicated Xvfb baseline",
        "swap_formula": "range/max over every recorded application memory snapshot; for the combined counter add xserver.after.Swap_bytes from the same run before taking range/max",
        "scope": "resident PSS excludes swap; Swap counters remain separate and are not added to PSS as a unique physical-memory total",
    }

def svg_document(report):
    if not report.get("ok") or len(report.get("runs", [])) != len(APPS) * REPETITIONS:
        raise ValueError("refusing to render incomplete comparison data")
    rows = summarize(report["runs"])
    by_app = {row["app"]: row for row in rows}
    colors = {"ztty": "#1a73e8", "ghostty": "#00897b", "kitty": "#8754c8"}
    parts = [
        '<svg xmlns="http://www.w3.org/2000/svg" width="1120" height="790" viewBox="0 0 1120 790" role="img" aria-labelledby="title desc">',
        '<title id="title">Ten terminal windows: memory and real PTY performance</title>',
        '<desc id="desc">Three fresh-process runs per terminal, all using shared instances. '
        'Application PSS excludes the display server; a second panel adds full dedicated Xvfb PSS. Probe workloads and private D-Bus are separate. '
        'Latency and throughput use DSR logical completion, not a rendered-frame fence.</desc>',
        '<style>'
        ':root{--bg:#fff;--panel:#f8fafd;--ink:#202124;--muted:#5f6368;--line:#dfe3e8;--track:#e9edf3}'
        '@media(prefers-color-scheme:dark){:root{--bg:#17191d;--panel:#202329;--ink:#e8eaed;--muted:#bdc1c6;--line:#41464d;--track:#30343b}}'
        'text{font-family:Arial,Helvetica,sans-serif;fill:var(--ink)}'
        '.muted{fill:var(--muted)}.h{font-size:20px;font-weight:600}.small{font-size:13px}'
        '.value{font-size:16px;font-weight:600}.label{font-size:15px}'
        '</style>',
        '<rect width="1120" height="790" rx="18" fill="var(--bg)"/>',
        '<text x="36" y="47" font-size="28" font-weight="600">10 windows. Shared instances. Same workload.</text>',
        '<text x="36" y="76" class="muted" font-size="15">Linux X11 / isolated Xvfb · software rendering · medians of 3 fresh-process runs</text>',
    ]
    panels = [
        (36, 102, "Application PSS", "MiB resident · lower is better", "median_pss_bytes", 1024 * 1024),
        (580, 102, "App + full X server PSS", "MiB resident · lower is better", "median_app_plus_xserver_pss_bytes", 1024 * 1024),
        (36, 355, "PTY round-trip latency (p50)", "µs · lower is better", "median_latency_p50_us", 1),
        (580, 355, "Output processing throughput", "MiB/s · higher is better", "median_output_mib_per_second", 1),
    ]
    for x, y, title, subtitle, key, divisor in panels:
        parts.extend([
            f'<rect x="{x}" y="{y}" width="504" height="229" rx="12" fill="var(--panel)" stroke="var(--line)"/>',
            f'<text x="{x + 20}" y="{y + 32}" class="h">{html.escape(title)}</text>',
            f'<text x="{x + 20}" y="{y + 55}" class="small muted">{html.escape(subtitle)}</text>',
        ])
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
                f'<rect x="{x + 93}" y="{yy - 12}" width="295" height="22" rx="4" fill="var(--track)"/>',
                f'<rect x="{x + 93}" y="{yy - 12}" width="{width:.3f}" height="22" rx="4" fill="{colors[app]}"/>',
                f'<text x="{x + 484}" y="{yy + 5}" text-anchor="end" class="value">{label}</text>',
            ])
    date = report["timestamp"][:10] + " UTC"
    versions = []
    for app in APPS:
        first = report["binaries"][app]["version"].get("stdout", "").splitlines()
        versions.append(first[0] if first else NAMES[app])
    swap_line = "Max sampled Swap (app + X server counters): " + " · ".join(
        NAMES[app] + " " + f"{by_app[app]['max_sampled_app_plus_xserver_swap_bytes'] / (1024 * 1024):.2f} MiB"
        for app in APPS)
    footer = [
        "App PSS counts all terminal processes. The second panel includes the full dedicated 7680×2160 Xvfb baseline.",
        "PTY probes and private D-Bus are separate. PSS covers resident pages only; this is not total GPU/VRAM accounting.",
        f"One active window, nine idle; write → DSR reply. Logical completion, not frame rate. Memory: fixed frame + {report['settings']['memory_settle_seconds']:g}s settle.",
        swap_line,
        " · ".join(versions) + " · " + date + " · 120×32 cells · monospace 11 pt",
        "Raw samples, versions, hashes and scope: docs/benchmarks/terminals-10-windows.json",
    ]
    for index, line in enumerate(footer):
        parts.append(f'<text x="36" y="{625 + index * 25}" class="small muted">{html.escape(line)}</text>')
    parts.append("</svg>")
    return "\n".join(parts) + "\n"


def atomic_write(path, content):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(content)
    os.replace(temporary, path)


def update_readmes():
    entries = [
        ("README.md",
         "![Ten-window terminal memory and PTY performance comparison](docs/benchmarks/terminals-10-windows.svg)\n\n"
         "Ten windows per terminal, shared instances, three real runs. "
         "[Method and rerun command](docs/benchmarks/README.md) · "
         "[Raw measurements](docs/benchmarks/terminals-10-windows.json)\n\n"
         "Rerun from current source: `make benchmark-terminals`.\n"),
        ("README.zh-CN.md",
         "![十窗口终端内存与 PTY 性能比较](docs/benchmarks/terminals-10-windows.svg)\n\n"
         "每个终端均开启 10 个窗口、使用共享实例，实测 3 轮。"
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
    parser.add_argument("--ghostty", type=Path)
    parser.add_argument("--kitty", type=Path)
    parser.add_argument("--json", type=Path, default=REPORT)
    parser.add_argument("--svg", type=Path, default=SVG)
    parser.add_argument("--render-only", action="store_true",
                        help="recompute summary and SVG from completed raw JSON without running terminals")
    parser.add_argument("--settle-seconds", type=float, default=1.0)
    parser.add_argument("--no-readme", action="store_true", help="leave README image references unchanged")
    args = parser.parse_args()
    if args.render_only:
        report = json.loads(args.json.read_text())
        refresh_summary(report)
        svg = svg_document(report)
        atomic_write(args.json, json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        atomic_write(args.svg, svg)
        if not args.no_readme:
            update_readmes()
        return 0
    if not 0.1 <= args.settle_seconds <= 10:
        parser.error("settle-seconds must be from 0.1 through 10")
    for program in ("Xvfb", "dbus-daemon"):
        if not shutil.which(program):
            parser.error(program + " is required; existing results were not changed")
    try:
        import Xlib  # noqa: F401
        binaries = {
            "ztty": args.ztty.resolve(strict=True),
            "ghostty": ghostty_binary(args.ghostty),
            "kitty": resolve_binary(args.kitty, "kitty"),
        }
    except (ImportError, FileNotFoundError) as error:
        parser.error(str(error) + "; existing results were not changed")
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("cannot enable child subreaper")
    metadata = {app: binary_metadata(path) for app, path in binaries.items()}
    payload = output_payload()
    report = {
        "schema": 1, "ok": False,
        "timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "machine": machine(), "command": [sys.executable, str(Path(__file__).resolve()), *sys.argv[1:]],
        "script_sha256": sha256(__file__),
        "git": {
            "head": capture(["git", "-C", str(ROOT), "rev-parse", "HEAD"]),
            "status": capture(["git", "-C", str(ROOT), "status", "--short"]),
        },
        "binaries": metadata,
        "settings": {
            "windows": WINDOWS, "repetitions": REPETITIONS, "geometry": [COLS, ROWS],
            "font": "monospace", "font_size_pt": 11, "xvfb_screen": "7680x2160x24", "dpi": 96,
            "sharing": {"ztty": "default broker", "kitty": "--single-instance + unique --instance-group",
                        "ghostty": "gtk-single-instance=true + unique class + private D-Bus"},
            "history": {"ztty_lines": 2000, "kitty_lines": 2000, "ghostty_bytes": 10000000,
                        "contents": "empty; static frame and output updates never scroll"},
            "font_warmup_frame_utf8": FRAME.decode(),
            "font_warmup_frame_sha256": hashlib.sha256(FRAME).hexdigest(),
            "memory_settle_seconds": args.settle_seconds,
            "memory_samples_per_run": MEMORY_SAMPLES,
            "latency_command_hex": LATENCY_COMMAND.hex(), "latency_reply_hex": LATENCY_REPLY.hex(),
            "latency_warmup_per_window": LATENCY_WARMUP, "latency_samples_per_window": LATENCY_SAMPLES,
            "output_bytes_per_sample": len(payload), "output_sha256": hashlib.sha256(payload).hexdigest(),
            "output_warmup_per_window": OUTPUT_WARMUP, "output_samples_per_window": OUTPUT_SAMPLES,
        },
        "method": {
            "application_memory": "sum smaps_rollup PSS and Private_Clean+Private_Dirty for every marked terminal process and helper, excluding known probe subtrees and isolated infrastructure",
            "memory_aggregation": "median of three process snapshots per run; median of three fresh-process runs per app",
            "latency_aggregation": "per-run p50/p95 across 10 x 300 measured DSR round trips; median of three run statistics",
            "output_aggregation": "median of three 1 MiB output bursts per window, then median of ten windows, then median of three runs",
            "performance_scope": "one focused window at a time, other nine idle; raw PTY write-to-DSR response; logical processing completion, not a physical rendering/scanout fence",
            "cold_warm": "fresh terminal processes and isolated config per run; memory sampled after fixed frame and font warm-up; OS page/font caches are not flushed",
            "graphics_scope": "Xvfb software rendering only; X-server PSS delta and XRes client pixmaps recorded separately; does not quantify native GPU/VRAM or desktop compositor behavior",
            "window_visibility": "all ten windows mapped and tiled without overlap on one private 7680x2160 X display",
            "desktop_services": "private EXTERNAL-authenticated D-Bus without service activation directories; GTK accessibility disabled; portal/accessibility desktop services are outside this isolated configuration",
            "ordering": "three app orders rotate by one app each repetition; only one app's ten windows run at a time",
            "limitations": "single machine, one grid/font/output pattern; history capacities have different units and histories remain empty; no real keyboard-to-photon claim",
        },
        "sources": [
            "https://ghostty.org/docs/config/reference",
            "https://docs.gtk.org/gtk4/running.html",
            "https://dbus.freedesktop.org/doc/dbus-daemon.1.html",
            "https://ghostty.org/docs/install/binary",
            "https://sw.kovidgoyal.net/kitty/invocation/",
            "https://man7.org/linux/man-pages/man5/proc_pid_smaps.5.html",
            "https://xorg.freedesktop.org/releases/X11R6.9.0/doc/html/XResQueryClientPixmapBytes.3.html",
        ],
        "runs": [],
    }
    for repetition in range(REPETITIONS):
        order = APPS[repetition:] + APPS[:repetition]
        for app in order:
            if sha256(binaries[app]) != metadata[app]["sha256"]:
                raise RuntimeError(f"{app} binary changed during benchmark")
            run = measure(app, binaries[app], repetition + 1, args.settle_seconds)
            report["runs"].append(run)
            print(json.dumps({
                "app": app, "repetition": repetition + 1, "ok": run["ok"],
                "pss_mib": run.get("application", {}).get("Pss_bytes", 0) / (1024 * 1024)
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
    refresh_summary(report)
    report["completed_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    report["ok"] = True
    svg = svg_document(report)
    # Validate and build everything before replacing either tracked artifact.
    serialized = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    atomic_write(args.json, serialized)
    atomic_write(args.svg, svg)
    if not args.no_readme:
        update_readmes()
    print(json.dumps({"ok": True, "json": str(args.json), "svg": str(args.svg),
                      "summary": report["summary"]}), flush=True)
    return 0


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--child":
        raise SystemExit(probe(sys.argv[2]))
    raise SystemExit(main())
