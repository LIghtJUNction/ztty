#!/usr/bin/env python3
"""Measure owned ztty/Kitty windows under isolated software-rendered Xvfb.

Uses the real PTY probe, /proc accounting, and optional XRes pixmap accounting.
This is not a physical-key-to-screen, GPU, or native desktop latency benchmark.
"""
import argparse
import contextlib
import ctypes
import ctypes.util
import datetime
import json
import os
from pathlib import Path
import platform
import runpy
import select
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]
PROBE = ROOT / "tests" / "pty_probe.py"
KITTY_PROFILES = {
    "default": {},
    "input-zero": {"input_delay": "0"},
    "low-latency": {"input_delay": "0", "repaint_delay": "2", "sync_to_monitor": "no"},
}
KITTY_BASE_SETTINGS = {
    "linux_display_server": "x11", "font_family": "monospace", "font_size": "11",
    "initial_window_width": "120c", "initial_window_height": "32c",
    "remember_window_size": "no", "scrollback_lines": "2000",
}


def configuration_label(app, profile):
    return f"kitty:{profile}" if app == "kitty" else app


FRAME = (
    "\x1b[2J\x1b[H\x1b[?25l"
    "\x1b[38;2;138;180;248mztty terminal runtime measurement\x1b[0m\r\n\r\n"
    "  One window · 120 columns × 32 rows · 2000 history lines\r\n"
    "  UTF-8: 中文终端  日本語  café  naïve  e\u0301\r\n"
    "  \x1b[31mred\x1b[0m  \x1b[32mgreen\x1b[0m  \x1b[34mblue\x1b[0m  "
    "\x1b[38;2;200;140;90m24-bit RGB\x1b[0m\r\n\r\n"
    "  Idle: no typing, no scrolling, hidden cursor, no animation.\r\n"
    "  Round trip: child → PTY → terminal → PTY → child.\r\n"
    "  Physical display scanout is outside this measurement.\r\n"
)


def send_json(connection, value):
    connection.sendall(json.dumps(value, ensure_ascii=False).encode("utf-8") + b"\n")


def receive_json(connection):
    data = bytearray()
    while b"\n" not in data:
        chunk = connection.recv(65536)
        if not chunk:
            raise RuntimeError("probe control connection closed")
        data.extend(chunk)
        if len(data) > 2 * 1024 * 1024:
            raise RuntimeError("probe control response is too large")
    return json.loads(data.split(b"\n", 1)[0])


def child(socket_path, query_only=False):
    """The measured application's child is idle until the runner requests work."""
    import termios
    import tty
    previous = termios.tcgetattr(0)
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        listener.bind(socket_path)
        listener.listen(1)
        tty.setraw(0)
        os.write(1, FRAME.encode("utf-8"))
        connection, _ = listener.accept()
        with connection:
            size = os.get_terminal_size(0)
            send_json(connection, {"ready": True, "pid": os.getpid(),
                                   "cols": size.columns, "rows": size.lines,
                                   "environment": {name: os.environ.get(name) for name in
                                                   ["TERM", "COLORTERM", "TERM_PROGRAM"]}})
            while True:
                command = receive_json(connection)
                if command["action"] == "quit":
                    send_json(connection, {"done": True})
                    return 0
                if command["action"] != "benchmark":
                    raise ValueError("unknown probe action")
                result_path = Path(command["result_path"])
                original_argv = sys.argv
                status = 0
                try:
                    sys.argv = [str(PROBE), str(result_path), "--benchmark-only"]
                    if query_only:
                        sys.argv.append("--query-only")
                    runpy.run_path(str(PROBE), run_name="__main__")
                except SystemExit as error:
                    status = error.code or 0
                except Exception as error:
                    status = 1
                    result_path.write_text(json.dumps({"ok": False, "error": repr(error)}))
                finally:
                    sys.argv = original_argv
                result = json.loads(result_path.read_text())
                send_json(connection, {"done": True, "status": status, "result": result})
    finally:
        with contextlib.suppress(OSError):
            termios.tcsetattr(0, termios.TCSANOW, previous)
        listener.close()


def process_stat(pid):
    line = Path(f"/proc/{pid}/stat").read_text()
    fields = line[line.rfind(")") + 2:].split()
    return {"pid": pid, "state": fields[0], "user_ticks": int(fields[11]),
            "system_ticks": int(fields[12]), "threads": int(fields[17]),
            "start_ticks": int(fields[19])}


def memory(pid):
    wanted = {"Rss", "Pss", "Pss_Anon", "Pss_File", "Pss_Shmem", "Private_Clean",
              "Private_Dirty", "Shared_Clean", "Shared_Dirty", "Swap"}
    result = {}
    for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
        if ":" in line:
            key, value = line.split(":", 1)
            if key in wanted:
                result[key + "_bytes"] = int(value.split()[0]) * 1024
    if "Rss_bytes" not in result or "Pss_bytes" not in result:
        raise RuntimeError(f"RSS/PSS unavailable for process {pid}")
    return result


def font_files(pid):
    paths = set()
    for line in Path(f"/proc/{pid}/maps").read_text().splitlines():
        parts = line.split(maxsplit=5)
        if len(parts) == 6 and Path(parts[5]).suffix.lower() in {".ttf", ".otf", ".ttc", ".pcf", ".pfa", ".pfb"}:
            paths.add(parts[5])
    with contextlib.suppress(OSError):
        for fd in Path(f"/proc/{pid}/fd").iterdir():
            with contextlib.suppress(OSError):
                target = os.readlink(fd)
                if Path(target).suffix.lower() in {".ttf", ".otf", ".ttc", ".pcf", ".pfa", ".pfb"}:
                    paths.add(target)
    return sorted(paths)


def capture(command, timeout=10):
    try:
        completed = subprocess.run(command, capture_output=True, text=True, timeout=timeout)
        return {"command": command, "exit_code": completed.returncode,
                "stdout": completed.stdout.strip()[:4096], "stderr": completed.stderr.strip()[:4096]}
    except Exception as error:
        return {"command": command, "error": repr(error)}


def pixmap_bytes(display_name, window_id):
    """XRes accounts server-side PIXMAP resources for the window's X client."""
    result = {"available": False,
              "scope": "server PIXMAP resources attributed to the window's X client; not total GPU/driver/font memory",
              "source": "https://xorg.freedesktop.org/releases/X11R6.9.0/doc/html/XResQueryClientPixmapBytes.3.html"}
    display = None
    old_handler = None
    handler_installed = False
    xlib = None
    try:
        xres_path = ctypes.util.find_library("XRes")
        xlib_path = ctypes.util.find_library("X11")
        if not xres_path or not xlib_path:
            raise RuntimeError("libXRes or libX11 is unavailable")
        xlib = ctypes.CDLL(xlib_path)
        xres = ctypes.CDLL(xres_path)
        xlib.XOpenDisplay.argtypes = [ctypes.c_char_p]
        xlib.XOpenDisplay.restype = ctypes.c_void_p
        xlib.XCloseDisplay.argtypes = [ctypes.c_void_p]
        xlib.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
        xlib.XSetErrorHandler.argtypes = [ctypes.c_void_p]
        xlib.XSetErrorHandler.restype = ctypes.c_void_p
        error_seen = []
        handler_type = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)

        @handler_type
        def error_handler(_display, _event):
            error_seen.append(True)
            return 0

        old_handler = xlib.XSetErrorHandler(ctypes.cast(error_handler, ctypes.c_void_p))
        handler_installed = True
        display = xlib.XOpenDisplay(display_name.encode("ascii"))
        if not display:
            raise RuntimeError("XOpenDisplay failed")
        xres.XResQueryExtension.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int)]
        xres.XResQueryExtension.restype = ctypes.c_int
        event, error = ctypes.c_int(), ctypes.c_int()
        if not xres.XResQueryExtension(display, ctypes.byref(event), ctypes.byref(error)):
            raise RuntimeError("X-Resource extension is unavailable")
        xres.XResQueryVersion.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int)]
        xres.XResQueryVersion.restype = ctypes.c_int
        major, minor = ctypes.c_int(), ctypes.c_int()
        xres.XResQueryVersion(display, ctypes.byref(major), ctypes.byref(minor))
        xres.XResQueryClientPixmapBytes.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong)]
        xres.XResQueryClientPixmapBytes.restype = ctypes.c_int
        total = ctypes.c_ulong()
        status = xres.XResQueryClientPixmapBytes(display, window_id, ctypes.byref(total))
        xlib.XSync(display, 0)
        if not status or error_seen:
            raise RuntimeError("XRes pixmap request failed")
        result.update(available=True, bytes=int(total.value), extension_version=[major.value, minor.value])
    except Exception as error:
        result["error"] = repr(error)
    finally:
        if display and xlib:
            xlib.XCloseDisplay(display)
        if xlib and handler_installed:
            xlib.XSetErrorHandler(old_handler)
    return result


def descendants(pid):
    found = {}
    todo = [pid]
    while todo and len(found) < 256:
        current = todo.pop()
        with contextlib.suppress(OSError, ValueError):
            for task in Path(f"/proc/{current}/task").iterdir():
                for child_pid in (task / "children").read_text().split():
                    child_pid = int(child_pid)
                    if child_pid not in found:
                        found[child_pid] = process_stat(child_pid)["start_ticks"]
                        todo.append(child_pid)
    return found


def same_process(pid, start):
    try:
        return process_stat(pid)["start_ticks"] == start
    except (OSError, ValueError):
        return False


def stop(process, extra=None):
    if process is None:
        return []
    owned = descendants(process.pid)
    owned.update(extra or {})
    for pid, started in owned.items():
        if same_process(pid, started):
            with contextlib.suppress(ProcessLookupError):
                os.kill(pid, signal.SIGTERM)
    try:
        process.wait(timeout=0.5)
    except subprocess.TimeoutExpired:
        with contextlib.suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=0.5)
        except subprocess.TimeoutExpired:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=2)
    for pid, started in owned.items():
        if same_process(pid, started):
            with contextlib.suppress(ProcessLookupError):
                os.kill(pid, signal.SIGKILL)
    deadline = time.monotonic() + 1
    while time.monotonic() < deadline:
        for pid in list(owned):
            with contextlib.suppress(ChildProcessError, ProcessLookupError):
                os.waitpid(pid, os.WNOHANG)
            if not same_process(pid, owned[pid]):
                del owned[pid]
        if not owned:
            break
        time.sleep(0.01)
    return sorted(owned)


def wait_for(predicate, process, description, timeout=15):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if process.poll() is not None:
            raise RuntimeError(f"application exited with {process.returncode} while waiting for {description}")
        result = predicate()
        if result:
            return result
        time.sleep(0.01)
    raise TimeoutError("timed out waiting for " + description)


def measure(app, binary, repetition, args, profile="default"):
    from Xlib import X, display
    result = {"app": app, "profile": profile, "configuration": configuration_label(app, profile),
              "repetition": repetition, "ok": False}
    terminal = xvfb = connection = d = None
    child_identity = {}
    with tempfile.TemporaryDirectory(prefix=f"ztty-runtime-{app}-") as temporary:
        directory = Path(temporary)
        xlog_path, tlog_path = directory / "xvfb.stderr", directory / "terminal.stderr"
        xlog, tlog = xlog_path.open("wb"), tlog_path.open("wb")
        try:
            read_fd, write_fd = os.pipe()
            try:
                xvfb_command = ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1920x1200x24", "-dpi", "96", "-nolisten", "tcp"]
                xvfb = subprocess.Popen(xvfb_command, pass_fds=(write_fd,), stdout=subprocess.DEVNULL,
                                        stderr=xlog, start_new_session=True)
            finally:
                os.close(write_fd)
            try:
                if not select.select([read_fd], [], [], 10)[0]:
                    raise TimeoutError("Xvfb did not allocate a display")
                number = os.read(read_fd, 64).strip().decode("ascii")
                if not number.isdecimal():
                    raise RuntimeError("Xvfb returned an invalid display number")
            finally:
                os.close(read_fd)
            display_name = ":" + number
            d = display.Display(display_name)
            root = d.screen().root
            result["xserver"] = {"pid": xvfb.pid, "screen_pixels": [1920, 1200], "depth": 24,
                                 "dpi_requested": 96, "vendor": str(d.display.info.vendor),
                                 "release": int(d.display.info.release_number), "environment": "isolated Xvfb"}
            environment = os.environ.copy()
            environment.pop("WAYLAND_DISPLAY", None)
            environment.pop("KITTY_LISTEN_ON", None)
            environment.update(DISPLAY=display_name, LC_ALL="C.UTF-8", XMODIFIERS="@im=none", LIBGL_ALWAYS_SOFTWARE="1")
            socket_path = directory / "control.sock"
            child_command = [sys.executable, str(Path(__file__).resolve()), "--child", str(socket_path)]
            if args.query_only:
                child_command.append("--query-only")
            if app == "ztty":
                command = [str(binary), "--font", "monospace:size=11", "--geometry", "120x32", "--scrollback", "2000", "-e"] + child_command
                result["explicit_settings"] = {"font": "monospace:size=11", "geometry": "120x32", "scrollback": "2000"}
            else:
                result["explicit_settings"] = KITTY_BASE_SETTINGS | KITTY_PROFILES[profile]
                command = [str(binary), "-c", "NONE"]
                for name, value in result["explicit_settings"].items():
                    command.extend(["-o", f"{name}={value}"])
                command.extend(child_command)
            result["command"] = command
            result["requested"] = {"columns": 120, "rows": 32, "font_family": "monospace", "font_size_points": 11, "scrollback_lines": 2000}
            result["fontconfig_match"] = capture(["fc-match", "-f", "%{family}\n%{style}\n%{file}\n", "monospace:size=11"])
            before_server = memory(xvfb.pid)
            terminal = subprocess.Popen(command, env=environment, stdout=subprocess.DEVNULL, stderr=tlog, start_new_session=True)
            wait_for(socket_path.exists, terminal, "idle PTY probe")
            connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            connection.settimeout(args.timeout)
            connection.connect(str(socket_path))
            ready = receive_json(connection)
            if not ready.get("ready"):
                raise RuntimeError("probe did not become ready")
            child_identity[ready["pid"]] = process_stat(ready["pid"])["start_ticks"]
            result["probe"] = ready
            if (ready["cols"], ready["rows"]) != (120, 32):
                raise AssertionError(f"PTY geometry differs from requested 120x32: {ready}")
            pid_atom = d.intern_atom("_NET_WM_PID")

            def find_window():
                for window in root.query_tree().children:
                    try:
                        prop = window.get_full_property(pid_atom, X.AnyPropertyType)
                        classes = window.get_wm_class() or ()
                        if (prop is not None and terminal.pid in prop.value) or any(c.lower() == app for c in classes):
                            return window
                    except Exception:
                        continue
                return None

            window = wait_for(find_window, terminal, "application window")
            window.set_input_focus(X.RevertToParent, X.CurrentTime)
            d.sync()
            time.sleep(args.settle_ms / 1000)
            geometry = window.get_geometry()
            prop = window.get_full_property(pid_atom, X.AnyPropertyType)
            app_pid = int(prop.value[0]) if prop is not None and len(prop.value) else terminal.pid
            if app_pid != terminal.pid and app_pid not in descendants(terminal.pid):
                raise RuntimeError("window PID does not belong to the launched process")
            result["application_pid"] = app_pid
            result["window"] = {"pixels": [geometry.width, geometry.height], "depth": geometry.depth,
                                "class": list(window.get_wm_class() or ()), "columns": ready["cols"], "rows": ready["rows"]}
            result["observed_font_files"] = font_files(app_pid)
            result["font_note"] = "Fontconfig match is a requested-font resolution; observed files are mapped/open font files in this application, including fallbacks."
            result["process_memory"] = memory(app_pid)
            result["process_memory_scope"] = "application PID only, excludes the Python PTY child and X server; RSS is not total terminal memory"
            first = process_stat(app_pid)
            started = time.monotonic()
            time.sleep(args.idle_ms / 1000)
            second = process_stat(app_pid)
            elapsed = time.monotonic() - started
            hz = os.sysconf("SC_CLK_TCK")
            ticks = second["user_ticks"] + second["system_ticks"] - first["user_ticks"] - first["system_ticks"]
            result["idle_cpu"] = {"sample_seconds": round(elapsed, 6), "ticks": ticks, "clock_ticks_per_second": hz,
                                  "cpu_seconds": round(ticks / hz, 6), "percent_of_one_core": round(ticks / hz / elapsed * 100, 4),
                                  "threads": second["threads"], "resolution_note": "Zero ticks means below /proc clock-tick resolution during this sample, not a universal zero-CPU guarantee."}
            after_server = memory(xvfb.pid)
            result["xserver_memory"] = {"before_application": before_server, "with_idle_application": after_server,
                                        "rss_delta_bytes": after_server["Rss_bytes"] - before_server["Rss_bytes"],
                                        "pss_delta_bytes": after_server["Pss_bytes"] - before_server["Pss_bytes"],
                                        "scope": "fresh Xvfb process delta; includes server caches and allocations beyond pixmaps"}
            result["xres_pixmap"] = pixmap_bytes(display_name, window.id)
            result["pixel_surface_estimate"] = {"bytes": geometry.width * geometry.height * 4,
                                                "formula": "window_width * window_height * 4",
                                                "scope": "estimate for one 32-bit pixel surface, not measured allocation or total graphics memory"}
            result_path = directory / "roundtrip.json"
            send_json(connection, {"action": "benchmark", "result_path": str(result_path)})
            benchmark = receive_json(connection)
            result["pty_probe"] = benchmark.get("result", {})
            if not benchmark.get("done") or benchmark.get("status") != 0 or not result["pty_probe"].get("ok"):
                raise AssertionError("PTY round-trip probe failed")
            if result["pty_probe"].get("pty_roundtrip_us", {}).get("samples") != 1000:
                raise AssertionError("PTY probe did not produce the required 1000 measured samples")
            result["ok"] = True
        except Exception as error:
            result["error"] = repr(error)
        finally:
            if connection:
                with contextlib.suppress(Exception):
                    connection.settimeout(1)
                    send_json(connection, {"action": "quit"})
                    receive_json(connection)
                connection.close()
            leftovers = []
            try:
                leftovers.extend(stop(terminal, child_identity))
            except Exception as error:
                result["cleanup_error"] = repr(error)
            if d:
                with contextlib.suppress(Exception):
                    d.close()
            try:
                leftovers.extend(stop(xvfb))
            except Exception as error:
                result["xvfb_cleanup_error"] = repr(error)
            result["unreaped_or_live_owned_pids"] = sorted(set(leftovers))
            if leftovers or "cleanup_error" in result or "xvfb_cleanup_error" in result:
                result["ok"] = False
            xlog.close()
            tlog.close()
            result["terminal_stderr"] = tlog_path.read_text(errors="replace")[-12000:]
            result["xvfb_stderr"] = xlog_path.read_text(errors="replace")[-4000:]
    return result


def machine():
    cpu = None
    for line in Path("/proc/cpuinfo").read_text().splitlines():
        if line.startswith("model name") or line.startswith("Hardware"):
            cpu = line.split(":", 1)[1].strip()
            break
    governor = Path("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
    return {"cpu_model": cpu, "logical_cpus": os.cpu_count(), "affinity": sorted(os.sched_getaffinity(0)),
            "os": platform.system(), "kernel": platform.release(), "architecture": platform.machine(),
            "python": platform.python_version(), "rust": capture(["rustc", "-vV"]),
            "cpu_governor": governor.read_text().strip() if governor.exists() else None,
            "load_average_at_start": list(os.getloadavg())}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--child", help=argparse.SUPPRESS)
    parser.add_argument("--ztty", type=Path, default=ROOT / "target/release/ztty")
    parser.add_argument("--kitty", default=shutil.which("kitty"))
    parser.add_argument("--apps", nargs="+", choices=["ztty", "kitty"], default=["ztty", "kitty"],
                        help="applications to measure (default: ztty kitty)")
    parser.add_argument("--kitty-profiles", nargs="+", choices=list(KITTY_PROFILES), default=list(KITTY_PROFILES),
                        help="Kitty timing profiles (default: default input-zero low-latency)")
    parser.add_argument("--query-only", action="store_true",
                        help="time DSR queries without moving or writing a character in each timed request")
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/runtime-metrics.json")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--settle-ms", type=int, default=500)
    parser.add_argument("--idle-ms", type=int, default=500)
    parser.add_argument("--timeout", type=float, default=60)
    args = parser.parse_args()
    if args.child:
        return child(args.child, args.query_only)
    if not 1 <= args.repeats <= 10 or args.idle_ms < 250 or args.settle_ms < 0 or args.timeout <= 0:
        parser.error("require 1..10 repeats, idle >=250ms, settle >=0ms, and timeout >0")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    applications = list(dict.fromkeys(args.apps))
    profiles = list(dict.fromkeys(args.kitty_profiles))
    configurations = [(app, profile) for app in applications
                      for profile in (profiles if app == "kitty" else ["default"])]
    report = {"schema": 2, "ok": False, "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "method": {"display": "fresh Xvfb per application per run, 1920x1200x24, requested 96 DPI",
                         "rendering": "LIBGL_ALWAYS_SOFTWARE=1; software X11 in Xvfb, no native compositor",
                         "font": "monospace, 11 points", "geometry": "120x32 cells", "history_limit": 2000,
                         "settle_ms": args.settle_ms, "idle_sample_ms": args.idle_ms, "repeats": args.repeats,
                         "run_order": "one run per configuration each repetition; starting configuration rotates by one each repetition",
                         "kitty_default": "-c NONE with shared font/geometry/X11 settings; timing options use installed Kitty defaults",
                         "workload": "query-only" if args.query_only else "home-write-query",
                         "roundtrip": "100 warmups + 1000 requests, child/PTY/terminal/PTY/child; includes Python probe overhead; DSR is not a frame-completion barrier",
                         "scope_limits": ["not physical key-to-screen latency", "not hardware GPU/native desktop performance",
                                          "RSS/PSS, X server delta, XRes bytes, and pixel estimate are separate accounts; do not blindly sum shared mappings",
                                          "Ghostty is not measured by this runner"]},
              "configurations": {configuration_label(app, profile): {
                  "app": app, "profile": profile,
                  "timing_overrides": KITTY_PROFILES[profile] if app == "kitty" else {},
              } for app, profile in configurations},
              "runs": [], "applications": {}, "errors": []}
    try:
        # Adopt only descendants of this runner if a failed terminal exits before
        # its PTY child. Cleanup still waits for specific known PIDs, never -1.
        libc = ctypes.CDLL(None)
        report["subreaper_enabled"] = libc.prctl(36, 1, 0, 0, 0) == 0
        report["machine"] = machine()
        if not shutil.which("Xvfb"):
            raise RuntimeError("Xvfb is required")
        binaries = {}
        for name in applications:
            path = args.ztty if name == "ztty" else args.kitty
            if not path or not Path(path).is_file():
                report["errors"].append(f"{name} executable unavailable: {path}")
                continue
            binary = Path(path).resolve()
            binaries[name] = binary
            report["applications"][name] = {"binary": str(binary), "binary_bytes": binary.stat().st_size,
                                              "binary_size_scope": "entry executable only; excludes application resources and shared runtime libraries",
                                              "version": capture([str(binary), "--version"])}
        for repetition in range(1, args.repeats + 1):
            offset = (repetition - 1) % len(configurations)
            order = configurations[offset:] + configurations[:offset]
            for name, profile in order:
                if name not in binaries:
                    continue
                label = configuration_label(name, profile)
                load_at_start = list(os.getloadavg())
                try:
                    measured = measure(name, binaries[name], repetition, args, profile)
                except Exception as error:
                    measured = {"app": name, "profile": profile, "configuration": label,
                                "repetition": repetition, "ok": False, "error": repr(error)}
                measured["load_average_at_start"] = load_at_start
                measured["run_index"] = len(report["runs"]) + 1
                report["runs"].append(measured)
                args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
                print(json.dumps({"app": name, "profile": profile, "configuration": label,
                                  "run": repetition, "ok": measured["ok"],
                                  "rss_bytes": measured.get("process_memory", {}).get("Rss_bytes"),
                                  "pty_us": measured.get("pty_probe", {}).get("pty_roundtrip_us"),
                                  "error": measured.get("error")}, ensure_ascii=False), flush=True)
    except Exception as error:
        report["errors"].append(repr(error))
    summary = {}
    for label, configuration in report["configurations"].items():
        attempted = [run for run in report["runs"] if run["configuration"] == label]
        runs = [run for run in attempted if run["ok"]]
        summary[label] = {**configuration, "requested_runs": args.repeats,
                          "successful_runs": len(runs), "failed_runs": len(attempted) - len(runs),
                          "missing_runs": args.repeats - len(attempted)}
        if runs:
            summary[label].update({"median_process_rss_bytes": statistics.median(run["process_memory"]["Rss_bytes"] for run in runs),
                            "median_process_pss_bytes": statistics.median(run["process_memory"]["Pss_bytes"] for run in runs),
                            "median_idle_percent_one_core": statistics.median(run["idle_cpu"]["percent_of_one_core"] for run in runs),
                            "median_of_run_pty_p50_us": statistics.median(run["pty_probe"]["pty_roundtrip_us"]["p50"] for run in runs),
                            "median_of_run_pty_p95_us": statistics.median(run["pty_probe"]["pty_roundtrip_us"]["p95"] for run in runs)})
    report["summary"] = summary
    report["ok"] = not report["errors"] and len(report["runs"]) == len(configurations) * args.repeats and all(run["ok"] for run in report["runs"])
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"Saved {args.output}", flush=True)
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
