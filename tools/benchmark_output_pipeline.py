#!/usr/bin/env python3
"""Measure common output through a private ztty broker, with and without X11.

Every timed write ends with a cursor-position response. This measures logical
PTY processing, not physical presentation. Mapped windows and retained text are
checked separately. Only this collector's broker, PTY and Xvfb are controlled.
"""
import argparse
import contextlib
import ctypes
import hashlib
import json
import math
import os
from pathlib import Path
import select
import signal
import socket
import statistics
import struct
import subprocess
import sys
import tempfile
import time
import tty

from isolated_xvfb import start_xvfb
from benchmark_terminals import output_payload, output_screen

COLS, ROWS = 120, 32
KINDS = ("ascii-log", "color-log", "unicode-log", "progress", "inplace-ascii")
APPEARANCE = {"font": "monospace:size=11", "background-opacity": "100",
              "background-blur": "false", "cursor-blink": "false"}
ROOT = Path(__file__).resolve().parents[1]


def digest(data):
    return hashlib.sha256(data).hexdigest()


def send(channel, value):
    channel.sendall(json.dumps(value).encode() + b"\n")


def receive(stream):
    line = stream.readline(1024 * 1024)
    if not line or not line.endswith(b"\n"):
        raise RuntimeError("missing bounded control reply")
    return json.loads(line)


def write_all(data):
    pending = memoryview(data)
    while pending:
        count = os.write(1, pending[:65536])
        if count <= 0:
            raise OSError("PTY write made no progress")
        pending = pending[count:]


def exchange(command, expected):
    write_all(command)
    received = bytearray()
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if not select.select([0], [], [], max(0, deadline - time.monotonic()))[0]:
            break
        data = os.read(0, 4096)
        if not data:
            break
        received.extend(data)
        if bytes(received) == expected:
            return
        if len(received) >= len(expected):
            break
    raise AssertionError(f"DSR response differs: {bytes(received)!r} != {expected!r}")


def payload(kind, target):
    if kind == "inplace-ascii":
        block = output_payload()
        return block * math.ceil(target / len(block)), output_screen(block), (ROWS, COLS)
    texts, parts = [], []
    size = index = 0
    while size < target:
        if kind == "unicode-log":
            text = f"{index:06d} 终端响应 · Ελληνικά · 日本語 · e\u0301 · 안녕하세요"
            data = (text + "\r\n").encode()
        elif kind == "progress":
            text = f"progress {index:06d} [" + "#" * 60 + "] processing build artifacts"
            data = ("\r" + text + "\x1b[K").encode()
        else:
            text = (f"{index:06d} INFO pipeline=/work/build step=compile status=ready "
                    "detail=0123456789abcdefghijklmnopqrstuvwxyz")
            if kind == "color-log":
                data = (f"\x1b[3{index % 7 + 1}m{text[:11]}\x1b[0m{text[11:]}\r\n").encode()
            else:
                data = (text + "\r\n").encode()
        texts.append(text)
        parts.append(data)
        size += len(data)
        index += 1
    if kind == "progress":
        screen = texts[-1] + "\n" * ROWS
        cursor = (1, len(texts[-1]) + 1)
    else:
        screen = "".join(line + "\n" for line in texts[-(ROWS - 1):]) + "\n"
        cursor = (ROWS, 1)
    return b"".join(parts), screen, cursor


def child(path, cpu):
    os.sched_setaffinity(0, {cpu})
    tty.setraw(0)
    channel = socket.socket(socket.AF_UNIX)
    channel.connect(path)
    stream = channel.makefile("rb")
    send(channel, {"pid": os.getpid(), "affinity": sorted(os.sched_getaffinity(0))})
    data, expected = b"", b""
    while True:
        request = receive(stream)
        op = request["op"]
        if op == "prepare":
            data = Path(request["payload"]).read_bytes()
            row, col = request["cursor"]
            expected = f"\x1b[{row};{col}R".encode()
            write_all(b"\x1b[0m\x1b[?25l\x1b[2J\x1b[H")
            exchange(b"\x1b[5n", b"\x1b[0n")
            # Warm the same stream, including history recycling and fonts.
            write_all(data)
            exchange(b"\x1b[6n", expected)
            send(channel, {"ready": True, "bytes": len(data), "sha256": digest(data)})
        elif op == "measure":
            rates = []
            for _ in range(request["samples"]):
                started = time.perf_counter_ns()
                write_all(data)
                exchange(b"\x1b[6n", expected)
                elapsed = time.perf_counter_ns() - started
                rates.append({"elapsed_ns": elapsed, "bytes": len(data),
                              "mib_s": len(data) * 1e9 / elapsed / 1048576})
            latency = []
            for n in range(220):
                started = time.perf_counter_ns()
                exchange(b"\x1b[5n", b"\x1b[0n")
                if n >= 20:
                    latency.append(time.perf_counter_ns() - started)
            send(channel, {"output": rates, "idle_dsr_latency_ns": latency})
        elif op == "profile":
            deadline = time.monotonic() + request["seconds"]
            size = 0
            while time.monotonic() < deadline:
                write_all(data)
                exchange(b"\x1b[6n", expected)
                size += len(data)
            send(channel, {"profile_bytes": size})
        elif op == "quit":
            send(channel, {"bye": True})
            return
        else:
            raise ValueError(op)


def identity(pid):
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        return int(fields[19])
    except FileNotFoundError:
        return None


def cpu_ns(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return (int(fields[11]) + int(fields[12])) * 1_000_000_000 // os.sysconf("SC_CLK_TCK")


def measure(args, kind, mode):
    from Xlib import X, display
    result = {"kind": kind, "mode": mode, "ok": False, "owned_remaining": []}
    owned = {}
    server = xconnection = channel = stream = listener = None
    profiler = None
    with tempfile.TemporaryDirectory(prefix="ztty-output-") as temporary:
        directory = Path(temporary)
        log = (directory / "terminal.log").open("wb")
        try:
            data, expected_screen, cursor = payload(kind, args.mib * 1048576)
            result.update(payload_bytes=len(data), payload_sha256=digest(data),
                          expected_cursor=list(cursor), expected_screen_sha256=digest(expected_screen.encode()))
            payload_file = directory / "payload.bin"
            payload_file.write_bytes(data)
            env = dict(os.environ, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                       XDG_CONFIG_HOME=str(directory / "config"), XDG_RUNTIME_DIR=temporary,
                       ZTTY_SOCKET="@" + directory.name)
            env.pop("WAYLAND_DISPLAY", None)
            server = start_xvfb(screen="1600x1000x24", stderr=log, env=env)
            env["DISPLAY"] = server.display
            os.sched_setaffinity(server.process.pid, {args.xserver_cpu})
            result["display_ownership"] = server.evidence
            xconnection = display.Display(server.display)
            listener = socket.socket(socket.AF_UNIX)
            listener.bind(str(directory / "control"))
            listener.listen(1)
            listener.settimeout(20)
            subprocess.run(["taskset", "-c", str(args.broker_cpu), str(args.binary), "list"],
                           env=env, stdout=log, stderr=log, check=True, timeout=15)

            def rpc(value):
                with socket.socket(socket.AF_UNIX) as peer:
                    peer.settimeout(10)
                    peer.connect("\0" + env["ZTTY_SOCKET"][1:])
                    pid, uid, _ = struct.unpack("3i", peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
                    assert uid == os.getuid()
                    if not owned:
                        assert digest(Path(f"/proc/{pid}/exe").read_bytes()) == args.binary_sha256
                        owned[pid] = identity(pid)
                        result["broker_pid"] = pid
                    assert pid == result["broker_pid"]
                    send(peer, value)
                    reply = receive(peer.makefile("rb"))
                    assert reply.get("ok") is True, reply
                    return reply["result"]

            created = rpc({"op": "create", "origin": "local", "name": "output-probe",
                           "headless": mode == "headless", "cols": COLS, "rows": ROWS,
                           "history": args.history, "_appearance": list(APPEARANCE.items()),
                           "command": [sys.executable, str(Path(__file__).resolve()), "--child",
                                       str(directory / "control"), str(args.writer_cpu)]})
            channel, _ = listener.accept()
            channel.settimeout(60)
            stream = channel.makefile("rb")
            hello = receive(stream)
            owned[hello["pid"]] = identity(hello["pid"])
            result["writer_pid"] = hello["pid"]
            info = rpc({"op": "info", "session": created["uri"]})
            assert info["pid"] == hello["pid"] and info["cols"] == COLS and info["rows"] == ROWS
            assert info["attached"] is (mode == "window")
            result["persistent"] = info.get("persistent")
            result["affinity"] = {"broker": sorted(os.sched_getaffinity(result["broker_pid"])),
                                  "writer": hello["affinity"],
                                  "xserver": sorted(os.sched_getaffinity(server.process.pid))}
            if mode == "window":
                view = xconnection.create_resource_object("window", info["window_id"])
                xconnection.sync()
                assert view.get_attributes().map_state == X.IsViewable
                result["mapped_window"] = info["window_id"]
                size = view.get_geometry()
                result["window_pixels"] = [size.width, size.height]
            send(channel, {"op": "prepare", "payload": str(payload_file), "cursor": cursor})
            prepared = receive(stream)
            assert prepared["ready"] and prepared["sha256"] == result["payload_sha256"]
            # Font work and the repaint deadline must settle before sampling.
            time.sleep(.05)
            before = cpu_ns(result["broker_pid"])
            x_before = cpu_ns(server.process.pid)
            send(channel, {"op": "measure", "samples": args.samples})
            measured = receive(stream)
            result.update(measured)
            result["broker_cpu_ns"] = cpu_ns(result["broker_pid"]) - before
            result["xserver_cpu_ns"] = cpu_ns(server.process.pid) - x_before
            result["median_mib_s"] = statistics.median(s["mib_s"] for s in result["output"])
            ordered = sorted(result["idle_dsr_latency_ns"])
            result["idle_dsr_p50_ns"], result["idle_dsr_p95_ns"] = ordered[len(ordered)//2], ordered[int(len(ordered)*.95)]
            retained = rpc({"op": "read", "session": created["uri"], "max_bytes": 1})
            assert not retained["screen_truncated"]
            assert retained["screen"] == expected_screen, (kind, retained["screen"], expected_screen)
            result["all_visible_text_checked"] = True
            if args.perf_seconds:
                stem = args.output.parent / f"profile-{mode}-{kind}"
                with stem.with_suffix(".perf.log").open("wb") as perf_log:
                    profiler = subprocess.Popen(["perf", "record", "-q", "-o", str(stem.with_suffix(".data")),
                        "-F", "499", "-e", "cycles:u", "-g", "--call-graph", "dwarf,8192",
                        "-p", str(result["broker_pid"]), "--", "sleep", str(args.perf_seconds + .2)],
                        stdout=perf_log, stderr=perf_log)
                    time.sleep(.1)
                    send(channel, {"op": "profile", "seconds": args.perf_seconds})
                    result.update(receive(stream))
                    profiler.wait(timeout=15)
                    assert profiler.returncode == 0
                with stem.with_suffix(".txt").open("wb") as report:
                    subprocess.run(["perf", "report", "--stdio", "--no-children", "--percent-limit", "0.5",
                                    "--sort", "dso,symbol", "-i", str(stem.with_suffix(".data"))],
                                   stdout=report, stderr=subprocess.STDOUT, check=True, timeout=15)
                result["perf_report"] = str(stem.with_suffix(".txt"))
            send(channel, {"op": "quit"})
            assert receive(stream)["bye"]
            result["ok"] = True
        except Exception as exc:
            result["error"] = repr(exc)
        finally:
            for resource in (stream, channel, listener):
                if resource is not None:
                    resource.close()
            if profiler is not None and profiler.poll() is None:
                profiler.terminate()
                profiler.wait(timeout=5)
            for pid, token in owned.items():
                if token is not None and identity(pid) == token:
                    with contextlib.suppress(ProcessLookupError):
                        os.kill(pid, signal.SIGTERM)
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                try:
                    if os.waitpid(-1, os.WNOHANG)[0] == 0:
                        time.sleep(.01)
                except ChildProcessError:
                    break
                if not any(identity(pid) == token for pid, token in owned.items()):
                    break
            for pid, token in owned.items():
                if identity(pid) == token:
                    result["owned_remaining"].append(pid)
            if xconnection is not None:
                xconnection.close()
            if server is not None:
                server.close()
            log.close()
            result["stderr"] = (directory / "terminal.log").read_text(errors="replace")[-4000:]
            if result["owned_remaining"]:
                result["ok"] = False
    return result


def main():
    if len(sys.argv) == 4 and sys.argv[1] == "--child":
        child(sys.argv[2], int(sys.argv[3]))
        return 0
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--binary-source", default=None, help="source commit of the supplied ELF")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--kinds", default=",".join(KINDS))
    parser.add_argument("--modes", default="headless,window")
    parser.add_argument("--mib", type=int, default=8)
    parser.add_argument("--samples", type=int, default=3)
    parser.add_argument("--history", type=int, default=2000)
    parser.add_argument("--perf-seconds", type=float, default=0)
    parser.add_argument("--broker-cpu", type=int, default=23)
    parser.add_argument("--writer-cpu", type=int, default=22)
    parser.add_argument("--xserver-cpu", type=int, default=21)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.binary_sha256 = digest(args.binary.read_bytes())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    assert 1 <= args.mib <= 128 and 1 <= args.samples <= 9
    assert 0 <= args.perf_seconds <= 5 and 0 <= args.history <= 100000
    kinds, modes = args.kinds.split(","), args.modes.split(",")
    assert all(kind in KINDS for kind in kinds) and all(mode in ("headless", "window") for mode in modes)
    assert {args.broker_cpu, args.writer_cpu, args.xserver_cpu} <= os.sched_getaffinity(0)
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("PR_SET_CHILD_SUBREAPER failed")
    governor = Path(f"/sys/devices/system/cpu/cpu{args.broker_cpu}/cpufreq/scaling_governor")
    report = {"ok": False, "binary": str(args.binary), "binary_sha256": args.binary_sha256,
              "binary_source_commit": args.binary_source,
              "collector_sha256": digest(Path(__file__).read_bytes()),
              "collector_source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "scope": "PTY write through logical cursor reply; software X11 rendering is concurrent, not a display fence",
              "geometry": [COLS, ROWS], "history": args.history, "appearance": APPEARANCE,
              "settings": {"mib": args.mib, "samples": args.samples, "kinds": kinds, "modes": modes,
                           "origin": "local", "perf_seconds": args.perf_seconds},
              "host": {"kernel": os.uname().release, "architecture": os.uname().machine,
                       "broker_cpu_governor": governor.read_text().strip() if governor.exists() else None,
                       "load_before": list(os.getloadavg()), "cpu_accounting_tick_ns": 1_000_000_000 // os.sysconf("SC_CLK_TCK")},
              "runs": []}
    for kind in kinds:
        for mode in modes:
            result = measure(args, kind, mode)
            report["runs"].append(result)
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
            print(json.dumps({k: result.get(k) for k in ("kind", "mode", "ok", "median_mib_s", "idle_dsr_p50_ns", "error")}), flush=True)
            if not result["ok"]:
                return 1
    report["ok"] = True
    report["host"]["load_after"] = list(os.getloadavg())
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
