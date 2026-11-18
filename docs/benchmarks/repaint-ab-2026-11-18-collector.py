#!/usr/bin/env python3
"""Pinned old/new ztty output comparison with fresh final pixels on guarded displays."""
import contextlib, datetime, hashlib, json, os, shlex, socket
import subprocess, sys, tempfile, termios, time, traceback, tty
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
import benchmark_terminals as bench
from isolated_xvfb import start_xvfb
from measure_runtime import process_stat, same_process
from measure_multiwindow import cleanup
SOURCE = "60c672aa6ee6dc79ba44182f1dd6079feced9a5c"
SHA = "f73d7b66b8b081244e4b4ad9a302271e4d3778d456bff8e615cc03ae8ebcabf5"
BINARY = ROOT / "artifacts/arm-candidate-ztty"
OUT = ROOT / "artifacts/repaint-ab-2026-11-18"
BURSTS = 64

def utc(): return datetime.datetime.now(datetime.timezone.utc).isoformat()
def digest(path): return hashlib.sha256(path.read_bytes()).hexdigest()

def child(path):
    previous = termios.tcgetattr(0)
    channel = socket.socket(socket.AF_UNIX)
    try:
        tty.setraw(0)
        channel.connect(path)
        settings = bench.receive_json(channel)
        bench.write_all(("\x1b]2;" + settings["title"] + "\x07").encode() + bench.FRAME)
        bench.exchange(bench.BARRIER, bench.BARRIER_REPLY)
        payload = bench.output_payload()
        bench.write_all(payload)
        bench.exchange(bench.BARRIER, bench.BARRIER_REPLY)
        size = os.get_terminal_size(0)
        bench.send_json(channel, {
            "ready": True, "pid": os.getpid(), "cols": size.columns, "rows": size.lines,
            "payload_bytes": len(payload), "payload_sha256": hashlib.sha256(payload).hexdigest(),
        })
        if bench.receive_json(channel) != {"op": "diagnostic", "bursts": BURSTS}:
            raise RuntimeError("unexpected diagnostic command")
        samples = []
        started = time.perf_counter_ns()
        for _ in range(BURSTS):
            burst_started = time.perf_counter_ns()
            bench.write_all(payload)
            bench.exchange(bench.BARRIER, bench.BARRIER_REPLY)
            samples.append(time.perf_counter_ns() - burst_started)
        elapsed = time.perf_counter_ns() - started
        # The timed payload is unchanged. This untimed, fresh visual marker must
        # become visible without another PTY request or X11 input/Expose event.
        final_marker = b"\x1b[1;1H\x1b[48;2;45;125;205m\x1b[2KAB FINAL VISIBLE\x1b[0m"
        bench.write_all(final_marker)
        bench.send_json(channel, {
            "bursts": BURSTS, "bytes": BURSTS * len(payload), "elapsed_ns": elapsed,
            "burst_elapsed_ns": samples,
            "untimed_final_marker_sha256": hashlib.sha256(final_marker).hexdigest(),
            "untimed_final_marker_bytes": len(final_marker),
            "scope": "PTY write and DSR completion, not frame presentation",
        })
        if bench.receive_json(channel) != {"op": "drain"}:
            raise RuntimeError("unexpected drain command")
        bench.exchange(bench.BARRIER, bench.BARRIER_REPLY)
        bench.send_json(channel, {"drained": True})
        if bench.receive_json(channel) != {"op": "quit"}:
            raise RuntimeError("unexpected quit command")
    finally:
        channel.close()
        termios.tcsetattr(0, termios.TCSANOW, previous)

def extend_owned_children(identities):
    """Read only children files of already owned process identities, no /proc-wide scan."""
    pending = list(identities)
    seen = set()
    while pending:
        pid = pending.pop()
        if pid in seen or not same_process(pid, identities[pid]):
            continue
        seen.add(pid)
        try:
            children = Path(f"/proc/{pid}/task/{pid}/children").read_text().split()
        except OSError:
            continue
        for item in children:
            child_pid = int(item)
            if child_pid not in identities:
                with contextlib.suppress(OSError, ValueError):
                    identities[child_pid] = process_stat(child_pid)["start_ticks"]
                    pending.append(child_pid)

def measure(name, binary, binary_sha):
    BINARY, SHA = binary, binary_sha
    from Xlib import X, display
    from PIL import Image
    out = OUT / name
    out.mkdir()
    result = {"name": name, "instrumented": False, "started_utc": utc(),
              "host_at_start": bench.host_pressure(), "ok": False}
    pressure = bench.comparison_pressure_reasons(result["host_at_start"])
    if pressure:
        result.update(error="comparison deferred: " + "; ".join(pressure),
                      host_pressure_gate=pressure, remaining_known_owned_pids=[])
        (out / "verification.json").write_text(json.dumps(result, indent=2) + "\n")
        return result
    identities = {}
    channel = listener = xserver = xconnection = launcher = None
    with tempfile.TemporaryDirectory(prefix="ztty-repaint-ab-") as temporary:
        directory = Path(temporary)
        tag = directory.name
        title = "ztty-x11-diagnostic-" + tag
        result["ownership_marker"] = {bench.MARKER: tag}
        (directory / "ztty.conf").write_text(bench.ZTTY_CONFIG + "window-padding = 16\n")
        wrapper = "#!/bin/sh\nexec " + shlex.join([
            sys.executable, str(Path(__file__).resolve()), "--child",
            str(directory / "control.sock")]) + "\n"
        (directory / "probe").write_text(wrapper)
        (directory / "probe").chmod(0o700)
        environment = os.environ.copy()
        for key in ("WAYLAND_DISPLAY", "KITTY_LISTEN_ON", "KITTY_PID", "KITTY_WINDOW_ID",
                    "TERM_PROGRAM", "TERM_PROGRAM_VERSION", "KITTY_CONFIG_DIRECTORY",
                    "GHOSTTY_BIN_DIR", "GHOSTTY_RESOURCES_DIR", "DBUS_SESSION_BUS_ADDRESS",
                    "LD_PRELOAD", "ZTTY_X11_DIAGNOSTIC_CONTROL"):
            environment.pop(key, None)
        environment.update({
            bench.MARKER: tag, "LC_ALL": "C.UTF-8", "LANG": "C.UTF-8", "XMODIFIERS": "@im=none",
            "GDK_BACKEND": "x11", "LIBGL_ALWAYS_SOFTWARE": "1",
            "ZTTY_SOCKET": "@ztty-x11-diag-" + tag, "XDG_RUNTIME_DIR": temporary,
            "XDG_CONFIG_HOME": str(directory / "config"), "XDG_CACHE_HOME": str(directory / "cache"),
            "XDG_DATA_HOME": str(directory / "data"),
        })
        with (out / "xvfb.log").open("wb") as xlog, (out / "terminal.log").open("wb") as tlog:
            try:
                if digest(BINARY) != SHA:
                    raise RuntimeError("immutable binary changed")
                xserver = start_xvfb(screen="1600x1000x24", dpi=96, stderr=xlog,
                                    env=environment, timeout=10)
                environment["DISPLAY"] = xserver.display
                result["display_ownership"] = xserver.evidence
                xconnection = display.Display(xserver.display)
                result["instrumentation_environment"] = {
                    key: environment.get(key) for key in ("LD_PRELOAD", "ZTTY_X11_DIAGNOSTIC_CONTROL")}
                listener = socket.socket(socket.AF_UNIX)
                listener.bind(str(directory / "control.sock"))
                listener.listen(1)
                listener.settimeout(15)
                command = bench.terminal_command("ztty", BINARY, directory, tag)
                result["terminal_command"] = command
                result["terminal_config"] = bench.ZTTY_CONFIG + "window-padding = 16\n"
                launcher = subprocess.Popen(command, env=environment, stdout=subprocess.DEVNULL,
                                            stderr=tlog, start_new_session=True)
                identities[launcher.pid] = process_stat(launcher.pid)["start_ticks"]
                channel, _ = listener.accept()
                channel.settimeout(30)
                bench.send_json(channel, {"title": title})
                ready = bench.receive_json(channel)
                result["child_ready"] = ready
                if not ready.get("ready") or (ready["cols"], ready["rows"]) != (120, 32):
                    raise RuntimeError("child readiness or geometry failed")
                child_pid = ready["pid"]
                marker = (bench.MARKER + "=" + tag).encode()
                if marker not in Path(f"/proc/{child_pid}/environ").read_bytes().split(b"\0"):
                    raise RuntimeError("probe child ownership marker missing")
                identities[child_pid] = process_stat(child_pid)["start_ticks"]
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    windows = bench.application_windows(xconnection, title)
                    if title in windows:
                        break
                    time.sleep(0.02)
                else:
                    raise TimeoutError("owned terminal window did not appear")
                window = windows[title]
                broker = window["pid"]
                if marker not in Path(f"/proc/{broker}/environ").read_bytes().split(b"\0"):
                    raise RuntimeError("broker ownership marker missing")
                if Path(f"/proc/{broker}/exe").resolve() != BINARY or digest(Path(f"/proc/{broker}/exe")) != SHA:
                    raise RuntimeError("running broker binary identity differs")
                identities[broker] = process_stat(broker)["start_ticks"]
                child_stat = Path(f"/proc/{child_pid}/stat").read_text()
                child_ppid = int(child_stat[child_stat.rfind(")") + 2:].split()[1])
                if child_ppid != broker:
                    raise RuntimeError("probe is not a child of the verified broker")
                result["broker"] = {**process_stat(broker), "exe": str(BINARY), "exe_sha256": SHA}
                result["owned_processes"] = identities.copy()
                (out / "broker-maps.txt").write_text(Path(f"/proc/{broker}/maps").read_text())
                window["window"].set_input_focus(X.RevertToParent, X.CurrentTime)
                xconnection.sync()
                time.sleep(0.05)
                result["broker_before"] = process_stat(broker)
                result["phase_started_utc"] = utc()
                active_started = time.perf_counter_ns()
                bench.send_json(channel, {"op": "diagnostic", "bursts": BURSTS})
                result["workload"] = bench.receive_json(channel)
                result["phase_elapsed_ns"] = time.perf_counter_ns() - active_started
                result["phase_ended_utc"] = utc()
                result["broker_after"] = process_stat(broker)
                xconnection.sync()
                geometry = window["window"].get_geometry()
                marker_started = time.monotonic()
                while True:
                    pixel_data = window["window"].get_image(
                        geometry.width - 20, 20, 1, 1, X.ZPixmap, 0xffffffff).data
                    pixel = Image.frombytes("RGB", (1, 1), pixel_data, "raw", "BGRX").getpixel((0, 0))
                    if pixel == (45, 125, 205):
                        break
                    if time.monotonic() - marker_started >= 0.2:
                        raise RuntimeError("fresh final marker did not become visible without further PTY input")
                    time.sleep(0.001)
                result["fresh_final_marker_visible_ms"] = (time.monotonic() - marker_started) * 1000
                result["final_marker_verified_before_drain"] = True
                captured = window["window"].get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xffffffff)
                if geometry.depth != 24 or len(captured.data) != geometry.width * geometry.height * 4:
                    raise RuntimeError("unexpected screenshot pixel format")
                image = Image.frombytes("RGB", (geometry.width, geometry.height), captured.data, "raw", "BGRX")
                image.save(out / "final-frame.png")
                result["final_frame"] = {
                    "width": geometry.width, "height": geometry.height,
                    "rgb_sha256": hashlib.sha256(image.tobytes()).hexdigest(),
                    "png_sha256": digest(out / "final-frame.png"),
                }
                bench.send_json(channel, {"op": "drain"})
                if bench.receive_json(channel) != {"drained": True}:
                    raise RuntimeError("post-capture drain failed")
                bench.send_json(channel, {"op": "quit"})
                result["binary_sha256_after"] = digest(BINARY)
                result["ok"] = result["binary_sha256_after"] == SHA
            except Exception as error:
                result["error"] = repr(error)
                result["traceback"] = traceback.format_exc()
            finally:
                if channel is not None: channel.close()
                if listener is not None: listener.close()
                extend_owned_children(identities)
                remaining = cleanup(identities)
                if launcher is not None:
                    with contextlib.suppress(subprocess.TimeoutExpired): launcher.wait(timeout=1)
                if xconnection is not None: xconnection.close()
                if xserver is not None:
                    number = xserver.number
                    xserver.close()
                    result["display_cleanup"] = {
                        "xvfb_exit_code": xserver.process.returncode,
                        "pid_lock_exists": Path(f"/tmp/.X{number}-lock").exists(),
                        "socket_exists": Path(f"/tmp/.X11-unix/X{number}").exists(),
                        "lease_exists": Path(f"/tmp/ztty-xvfb-{number}.lease").exists()}
                result["remaining_known_owned_pids"] = remaining
                if remaining: result["ok"] = False
    result["ended_utc"] = utc()
    result["host_at_end"] = bench.host_pressure()
    result["pressure_flags_after"] = bench.comparison_pressure_reasons(result["host_at_end"])
    result["artifact_sha256"] = {p.name: digest(p) for p in out.iterdir() if p.is_file()}
    (out / "verification.json").write_text(json.dumps(result, indent=2) + "\n")
    return result

def main():
    import argparse
    import statistics
    global OUT
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=OUT)
    parser.add_argument("--before", type=Path, default=ROOT.parent / "production-worktree/artifacts/arm-candidate-ztty")
    parser.add_argument("--after", type=Path, default=ROOT / "artifacts/repaint-final-ztty")
    args = parser.parse_args()
    OUT = args.output.resolve()
    if OUT.exists():
        raise RuntimeError("refuse to repeat or overwrite an existing AB collection")
    if subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip() != SOURCE:
        raise RuntimeError("after source commit changed")
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True).strip():
        raise RuntimeError("after source tree is not clean")
    variants = {
        "before": {
            "binary": str(args.before.resolve(strict=True)),
            "sha256": "f73d7b66b8b081244e4b4ad9a302271e4d3778d456bff8e615cc03ae8ebcabf5",
            "source_commit": "2dc6769b795e407d8fbecc2a305f1045868863a5"},
        "after": {
            "binary": str(args.after.resolve(strict=True)),
            "sha256": "786fec62515d50129be2216ce587b6fbda297368298922acad741f4a597f1c7a",
            "source_commit": SOURCE},
    }
    for info in variants.values():
        if digest(Path(info["binary"])) != info["sha256"]:
            raise RuntimeError("pinned comparison binary hash differs")
        info["bytes"] = Path(info["binary"]).stat().st_size
    OUT.mkdir()
    report = {
        "purpose": "three before/after pairs for the bounded repaint change only; not a Kitty/Ghostty comparison",
        "started_utc": utc(), "command": [sys.executable, *sys.argv],
        "variants": variants, "repetitions": 3, "bursts_per_run": BURSTS,
        "run_order": ["before", "after", "after", "before", "before", "after"],
        "driver_sha256": digest(Path(__file__)),
        "helpers_sha256": {name: digest(ROOT / name) for name in (
            "tools/benchmark_terminals.py", "tools/isolated_xvfb.py",
            "tools/measure_runtime.py", "tools/measure_multiwindow.py")},
        "source_files_sha256_at_commit": {},
        "measurement": {
            "window_count": 1, "sharing": "shared broker plus waiting launcher",
            "grid": [120, 32], "screen": "1600x1000x24", "dpi": 96,
            "font": "monospace:size=11", "scrollback": 2000, "padding": 16,
            "timed": "64 unchanged original CUP+SGR payloads, each followed by DSR completion",
            "fresh_pixels": "untimed unique blue final marker on row 1; XGetImage verifies visibility before further PTY requests; final whole-window pixels compared",
            "no_instrumentation": True,
            "pressure_gate": "unchanged tools/benchmark_terminals.py comparison_pressure_reasons at each run start; end flags also invalidate comparison"},
        "limitations": [
            "DSR completion is a logical PTY fence, not display presentation.",
            "Marker observations and final screenshots use private Xvfb, not a physical GPU/display.",
            "Only one visible idle-history session and this in-place ASCII/color workload are measured; history view force, Unicode/images, 5/10/20 sessions and native Wayland are outside this AB.",
            "Three short pairs do not establish a universal speedup or a hardware input-to-display latency bound.",
        ], "runs": [], "ok": False,
    }
    for variant, info in variants.items():
        hashes = {}
        for name in ("src/main.rs", "src/session.rs", "src/repaint.rs", "crates/zterm/src/terminal.rs"):
            r = subprocess.run(["git", "show", info["source_commit"] + ":" + name],
                               cwd=ROOT, capture_output=True)
            hashes[name] = hashlib.sha256(r.stdout).hexdigest() if r.returncode == 0 else None
        report["source_files_sha256_at_commit"][variant] = hashes
    for index, variant in enumerate(report["run_order"]):
        info = variants[variant]
        run = measure(f"{index + 1:02d}-{variant}", Path(info["binary"]), info["sha256"])
        run["variant"] = variant
        report["runs"].append(run)
        (OUT / "verification.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({"run": index + 1, "variant": variant, "ok": run["ok"],
                          "error": run.get("error"), "workload_ns": run.get("workload", {}).get("elapsed_ns"),
                          "final_marker_ms": run.get("fresh_final_marker_visible_ms"),
                          "pressure_flags_after": run.get("pressure_flags_after"),
                          "remaining": run.get("remaining_known_owned_pids")}), flush=True)
        if not run["ok"] or run.get("pressure_flags_after"):
            break
    if len(report["runs"]) == 6 and all(r["ok"] and not r.get("pressure_flags_after") for r in report["runs"]):
        report["summary"] = {}
        for variant in variants:
            selected = [r for r in report["runs"] if r["variant"] == variant]
            rates = [r["workload"]["bytes"] / (1024 ** 2) / (r["workload"]["elapsed_ns"] / 1e9) for r in selected]
            report["summary"][variant] = {
                "runs": len(selected), "mib_per_second": rates,
                "median_mib_per_second": statistics.median(rates),
                "max_observed_final_marker_ms": max(r["fresh_final_marker_visible_ms"] for r in selected)}
        report["summary"]["throughput_ratio"] = (
            report["summary"]["after"]["median_mib_per_second"] /
            report["summary"]["before"]["median_mib_per_second"])
        report["final_frame_pixels_equal"] = len({r["final_frame"]["rgb_sha256"] for r in report["runs"]}) == 1
        report["all_fresh_markers_verified_before_drain"] = all(r["final_marker_verified_before_drain"] for r in report["runs"])
        report["ok"] = report["final_frame_pixels_equal"] and report["all_fresh_markers_verified_before_drain"]
    report["ended_utc"] = utc()
    report["source_commit_after"] = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    report["source_status_after"] = subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True)
    report["binary_sha256_after"] = {name: digest(Path(info["binary"])) for name, info in variants.items()}
    if report["source_commit_after"] != SOURCE or report["source_status_after"].strip():
        report["ok"] = False
    if any(report["binary_sha256_after"][name] != info["sha256"] for name, info in variants.items()):
        report["ok"] = False
    report["raw_runs_sha256"] = hashlib.sha256(json.dumps(report["runs"], sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    (OUT / "verification.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "summary": report.get("summary"),
                      "final_frame_pixels_equal": report.get("final_frame_pixels_equal")}), flush=True)
    return 0 if report["ok"] else 1

if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--child": child(sys.argv[2])
    else: raise SystemExit(main())
