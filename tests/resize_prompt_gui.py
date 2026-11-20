#!/usr/bin/env python3
"""Keep queued old-width PTY output coherent with an X11 window resize."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import contextlib
import ctypes
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
import traceback

ROOT = Path(__file__).resolve().parents[1]


def wait(predicate, label, seconds=10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except (FileNotFoundError, ConnectionRefusedError):
            pass
        time.sleep(0.02)
    raise TimeoutError(label)


def exchange(path, request):
    with socket.socket(socket.AF_UNIX) as channel:
        channel.settimeout(10)
        channel.connect(path)
        channel.sendall(json.dumps(request).encode() + b"\n")
        data = bytearray()
        while b"\n" not in data:
            part = channel.recv(65536)
            if not part:
                raise EOFError("test channel closed")
            data.extend(part)
            if len(data) > 4 * 1024 * 1024:
                raise RuntimeError("oversized test response")
    response = json.loads(data.split(b"\n", 1)[0])
    if not response["ok"]:
        raise RuntimeError(response)
    return response


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--screenshots", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.screenshots.mkdir(parents=True, exist_ok=True)
    sys.path[:0] = [str(ROOT / "tools"), str(ROOT / "tests")]
    from isolated_xvfb import start_xvfb
    from measure_runtime import process_stat, same_process
    from measure_multiwindow import cleanup
    from repaint_gui import add_owned_children
    from input_gui import CHILD
    from Xlib import X, display
    from PIL import Image

    # Extend the existing fixture in memory. The file is a write-completion
    # barrier, independent of the DSR response that waits for the stopped broker.
    original = '        os.write(1,command.get("output","").encode()+b"\\x1b[6n")'
    assert CHILD.count(original) == 1, "raw PTY fixture changed"
    child = CHILD.replace(original, '''        payload=command.get("output","").encode()+b"\\x1b[6n"
        if os.write(1,payload)!=len(payload): raise RuntimeError("short probe PTY write")
        with open(sys.argv[3],"wb") as written: written.write(b"1")''')
    assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0
    report = {"ok": False, "binary": str(binary), "cases": [], "screenshots": {},
              "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "test_script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "scope": "Owned Linux X11 windows and PTYs; queued output plus ConfigureNotify; software renderer"}
    identities, launchers, sessions = {}, [], []
    server = connection = None
    with tempfile.TemporaryDirectory(prefix="ztty-resize-prompt-") as temporary:
        root = Path(temporary)
        with (root / "stderr.log").open("wb") as log:
            try:
                server = start_xvfb(screen="1800x1200x24", dpi=96, stderr=log)
                connection = display.Display(server.display)
                report["display_ownership"] = server.evidence
                address = "@ztty-resize-prompt-" + root.name
                env = dict(os.environ, DISPLAY=server.display, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                           XDG_RUNTIME_DIR=temporary, XDG_CONFIG_HOME=str(root / "config"),
                           XDG_CACHE_HOME=str(root / "cache"), XDG_STATE_HOME=str(root / "state"),
                           ZTTY_SOCKET=address)
                env.pop("WAYLAND_DISPLAY", None)
                env.pop("LD_PRELOAD", None)

                def rpc(request):
                    return exchange("\0" + address[1:], request)["result"]

                for padding in (0, 28):
                    name = f"resize-prompt-{padding}"
                    config = root / f"padding-{padding}.conf"
                    config.write_text("font = DejaVu Sans Mono:size=13\nbackground = #101010\n"
                                      "foreground = #eeeeee\nbackground-opacity = 100\nbackground-blur = false\n"
                                      f"cursor-blink = false\nrenderer = software\nwindow-padding = {padding}\n")
                    control_path, written = root / f"control-{padding}.sock", root / f"written-{padding}"
                    process = subprocess.Popen(
                        [str(binary), "--name", name, "--config", str(config), "--geometry", "120x32",
                         "-e", sys.executable, "-c", child, str(root / f"input-{padding}.bin"),
                         str(control_path), str(written)],
                        env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
                    launchers.append(process)
                    sessions.append(name)
                    identities[process.pid] = process_stat(process.pid)["start_ticks"]

                    def ready():
                        try:
                            return rpc({"op": "info", "session": name})
                        except RuntimeError:
                            return None

                    session = wait(ready, "owned terminal session")
                    for pid in (session["pid"], session["broker_pid"]):
                        identities[pid] = process_stat(pid)["start_ticks"]
                    wait(lambda: control_path.exists() and "READY" in rpc({"op": "read", "session": name})["screen"], "raw PTY ready")
                    view = connection.create_resource_object("window", session["window_id"])

                    def control(output=""):
                        return exchange(str(control_path), {"output": output, "clear": False})

                    def record(label, reply, expected_first_row, extra=None):
                        current = rpc({"op": "info", "session": name})
                        screen = rpc({"op": "read", "session": name})["screen"]
                        first = screen.split("\n", 1)[0].rstrip()
                        size = reply["winsize"]
                        # DSR orders model processing. This delay is only for
                        # pixel readback, never used to queue the resize race.
                        time.sleep(0.1)
                        geometry = view.get_geometry()
                        pixels = view.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xffffffff)
                        path = args.screenshots / f"padding-{padding}-{label}.png"
                        Image.frombytes("RGBA", (geometry.width, geometry.height), pixels.data, "raw", "BGRA").save(path)
                        report["screenshots"][f"{padding}:{label}"] = str(path)
                        case = {"name": label, "padding": padding, "screen": screen, "first_row": first,
                                "expected_first_row": expected_first_row, "pty_winsize": size,
                                "broker_cols": current["cols"], "broker_rows": current["rows"], "dsr": reply["dsr"],
                                "ok": first == expected_first_row and (size[1], size[0]) == (current["cols"], current["rows"])}
                        if extra:
                            case.update(extra)
                        report["cases"].append(case)
                        return case

                    dimensions = control()["winsize"]
                    cols = session["cols"]
                    assert (dimensions[1], dimensions[0]) == (cols, session["rows"]), (dimensions, session)
                    reset = "\x1b[?25l\x1b[0m\x1b[2J\x1b[H"
                    marker = "\x1b[1m\x1b[7m%\x1b[27m\x1b[0m"
                    tail = " " * (cols - 1) + "\r \r\nprobe\r\n> "
                    record("same-width", control(reset + marker + tail), "")
                    record("partial-line", control(reset + "x" + marker + tail), "x%")

                    broker = session["broker_pid"]
                    geometry = view.get_geometry()
                    content_width = geometry.width - 2 * padding
                    assert cols > 10 and content_width > 0 and content_width % cols == 0
                    cell_width = content_width // cols
                    assert 1 <= cell_width <= 256
                    worker = ThreadPoolExecutor(max_workers=1)
                    try:
                        try:
                            assert same_process(broker, identities[broker])
                            os.kill(broker, signal.SIGSTOP)
                            wait(lambda: process_stat(broker)["state"] in ("T", "t"), "owned broker stopped", seconds=3)
                            written.unlink(missing_ok=True)
                            future = worker.submit(control, reset + marker + tail)
                            wait(written.exists, "old-width PTY write completed", seconds=3)
                            view.configure(width=geometry.width - 10 * cell_width)
                            connection.sync()
                        finally:
                            if same_process(broker, identities[broker]):
                                os.kill(broker, signal.SIGCONT)
                        queued_reply = future.result(timeout=12)
                        # The pre-resize pump may answer that first DSR before
                        # the ioctl. Query once more after it to observe the
                        # settled PTY size without changing visible content.
                        case = record("resize-queued-marker", control(), "",
                                      {"old_columns": cols, "expected_columns": cols - 10,
                                       "queued_output_dsr": queued_reply["dsr"],
                                       "child_write_completed_before_configure": True})
                        case["ok"] = case["ok"] and case["broker_cols"] == cols - 10
                    finally:
                        if same_process(broker, identities[broker]):
                            os.kill(broker, signal.SIGCONT)
                        with contextlib.suppress(Exception):
                            view.configure(width=geometry.width, height=geometry.height)
                            connection.sync()
                        worker.shutdown(wait=True)
                    wait(lambda: control()["winsize"][1] == cols, "owned window dimensions restored")
                    rpc({"op": "close", "session": name})
                report["ok"] = len(report["cases"]) == 6 and all(case["ok"] for case in report["cases"])
            except Exception as error:
                report["error"], report["traceback"] = repr(error), traceback.format_exc()
            finally:
                if "rpc" in locals():
                    for name in reversed(sessions):
                        with contextlib.suppress(Exception):
                            rpc({"op": "close", "session": name})
                add_owned_children(identities, [os.getpid(), *identities])
                if server is not None:
                    identities.pop(server.process.pid, None)
                report["remaining_owned_pids"] = cleanup(identities)
                for process in launchers:
                    with contextlib.suppress(subprocess.TimeoutExpired):
                        process.wait(timeout=1)
                if connection is not None:
                    connection.close()
                if server is not None:
                    server.close()
                if report["remaining_owned_pids"]:
                    report["ok"] = False
                log.flush()
                report["stderr_tail"] = (root / "stderr.log").read_text(errors="replace")[-4000:]
    if hashlib.sha256(binary.read_bytes()).hexdigest() != report["binary_sha256"]:
        report["ok"], report["error"] = False, "Binary changed during verification"
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "cases": len(report["cases"]), "output": str(args.output),
                      "failed_cases": [f"padding-{case['padding']}:{case['name']}" for case in report["cases"] if not case["ok"]],
                      "error": report.get("error"), "remaining": report["remaining_owned_pids"]}))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
