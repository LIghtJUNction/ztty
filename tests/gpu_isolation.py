#!/usr/bin/env python3
"""Stall owned EGL presentation calls; verify management and unchanged live PTYs.

This is a controlled userspace stall on Xvfb/Mesa, not a physical GPU test.
"""
import argparse
import contextlib
import ctypes
import hashlib
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import sys
import tempfile
import time
import tty

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from isolated_xvfb import start_xvfb
from measure_runtime import process_stat, same_process
from measure_multiwindow import cleanup
from repaint_gui import add_owned_children, receive, send, wait as timed_wait

def wait(predicate, label):
    return timed_wait(predicate, label)[0]


def child(path, log):
    tty.setraw(0)
    task = subprocess.Popen(["sleep", "120"])
    with Path(log).open("a") as output:
        output.write("launch\n")
    channel = socket.socket(socket.AF_UNIX)
    channel.connect(path)
    send(channel, {"pid": os.getpid(), "task": task.pid})
    incoming = bytearray()
    heartbeat = 0
    try:
        while True:
            heartbeat += 1
            ready, _, _ = select.select([0, channel], [], [], 0.025)
            if 0 in ready:
                incoming.extend(os.read(0, 4096))
            if channel in ready:
                request = receive(channel)
                if request["op"] == "write":
                    os.write(1, b"\x1b[?25l\x1b[48;2;23;153;97m\x1b[2J\x1b[Hstill alive")
                send(channel, {"ok": True, "input_hex": incoming.hex(), "heartbeat": heartbeat})
    finally:
        task.terminate()
        task.wait()


class NegativeControlPassed(Exception):
    pass


def main():
    from Xlib import X, display
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--expect-blocked", action="store_true",
                        help="negative control for an unisolated baseline; never a release pass")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    report = {"ok": False, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "scope": "Owned Xvfb/Mesa and test-only eglSwapBuffers stall; physical GPU/Hyprland unverified",
              "negative_control": args.expect_blocked,
              "checks": [], "list_ms": []}
    assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0
    identities, launchers, channels = {}, [], []
    server = connection = listener = trigger = None
    with tempfile.TemporaryDirectory(prefix="ztty-gpu-isolation-") as temporary:
        directory = Path(temporary)
        with (directory / "stderr.log").open("wb") as log:
            try:
                server = start_xvfb(screen="1800x1000x24", dpi=96, stderr=log)
                connection = display.Display(server.display)
                env = dict(os.environ, DISPLAY=server.display, XMODIFIERS="@im=none",
                           XDG_CONFIG_HOME=str(directory / "config"),
                           XDG_CACHE_HOME=str(directory / "cache"),
                           XDG_STATE_HOME=str(directory / "state"),
                           ZTTY_SOCKET="@" + directory.name)
                env.pop("WAYLAND_DISPLAY", None)
                shim = directory / "render-stall.so"
                subprocess.run(["cc", "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-shared", "-fPIC",
                                str(ROOT / "tests/egl_render_stall.c"), "-ldl", "-o", str(shim)], check=True)
                trigger = directory / "armed"
                env["LD_PRELOAD"] = str(shim)
                env["ZTTY_TEST_RENDER_STALL"] = str(trigger)
                config = directory / "gpu.conf"
                config.write_text("renderer = gpu\nfont = monospace:size=11\ncursor-blink = false\nbackground-opacity = 100\nbackground-blur = false\n")
                listener = socket.socket(socket.AF_UNIX)
                control = directory / "control"
                listener.bind(str(control))
                listener.listen(8)
                listener.settimeout(10)

                def rpc(request):
                    with socket.socket(socket.AF_UNIX) as channel:
                        channel.settimeout(4)
                        channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                        send(channel, request)
                        reply = receive(channel)
                        assert reply["ok"], reply
                        return reply["result"]

                def status(channel):
                    send(channel, {"op": "status"})
                    return receive(channel)

                def text(window, name):
                    prop = window.get_full_property(connection.intern_atom(name), 0)
                    return bytes(prop.value).decode() if prop is not None else ""

                views = []
                for index in range(4):
                    name = f"isolation-{index}"
                    command_log = directory / f"launch-{index}"
                    launcher = subprocess.Popen([str(binary), "--name", name, "--config", str(config),
                        "--geometry", "60x16", "-e", sys.executable, str(Path(__file__).resolve()),
                        "--child", str(control), str(command_log)], env=env, stdout=log, stderr=log)
                    launchers.append(launcher)
                    identities[launcher.pid] = process_stat(launcher.pid)["start_ticks"]
                    channel, _ = listener.accept()
                    channel.settimeout(5)
                    channels.append(channel)
                    hello = receive(channel)
                    info = rpc({"op": "info", "session": name})
                    for pid in (hello["pid"], hello["task"], info["broker_pid"]):
                        identities[pid] = process_stat(pid)["start_ticks"]
                    window = connection.create_resource_object("window", info["window_id"])
                    window.configure(x=(index % 2) * 850, y=(index // 2) * 450)
                    connection.sync()
                    worker = info["broker_pid"] if args.expect_blocked else int(
                        wait(lambda: text(window, "_ZTTY_GPU_WORKER"), "GPU worker property"))
                    identities[worker] = process_stat(worker)["start_ticks"]
                    assert text(window, "_ZTTY_RENDERER") == "gpu"
                    rpc({"op": "write", "session": name, "data": "once\n"})
                    wait(lambda: status(channel)["input_hex"] == b"once\n".hex(), "initial input")
                    views.append(dict(name=name, channel=channel, worker=worker, window=window,
                                      info=info, hello=hello, command_log=command_log))
                assert len({v["info"]["broker_pid"] for v in views}) == 1
                if not args.expect_blocked:
                    assert len({v["worker"] for v in views}) == 4
                report["checks"].append("four real GPU views share one broker")
                before = [status(v["channel"])["heartbeat"] for v in views]
                # Pause only eglSwapBuffers, through the same native call in both builds.
                trigger.touch()
                for view in views:
                    send(view["channel"], {"op": "write"})
                    assert receive(view["channel"])["ok"]
                wait(lambda: list(directory.glob("armed.entered-*")), "real EGL stall entered")
                started = time.monotonic()
                if args.expect_blocked:
                    try:
                        subprocess.run([str(binary), "list"], env=env, capture_output=True, timeout=0.75)
                    except subprocess.TimeoutExpired:
                        report["negative_control_list_timed_out"] = True
                    else:
                        raise AssertionError("baseline unexpectedly serviced list during stalled EGL")
                    assert all(status(v["channel"])["heartbeat"] > before[i] for i, v in enumerate(views))
                    trigger.unlink()
                    listed = subprocess.run([str(binary), "list"], env=env, capture_output=True, text=True, timeout=3)
                    assert listed.returncode == 0 and all(v["name"] in listed.stdout for v in views)
                    assert all(v["command_log"].read_text() == "launch\n" for v in views)
                    report["checks"].append("baseline CLI list times out during EGL stall; tasks survive and trigger removal restores list")
                    report["ok"] = True
                    raise NegativeControlPassed()
                listed = subprocess.run([str(binary), "list"], env=env, capture_output=True,
                                        text=True, timeout=3)
                report["list_ms"].append((time.monotonic() - started) * 1000)
                assert listed.returncode == 0, listed.stderr
                assert all(view["name"] in listed.stdout for view in views), listed.stdout
                assert report["list_ms"][-1] < 3000
                assert len(list(directory.glob("armed.entered-*"))) == 4
                report["checks"].append("real CLI list returns within 3 seconds while all four EGL presenters stall")
                for index, view in enumerate(views):
                    wait(lambda: text(view["window"], "_ZTTY_RENDERER") == "software", "software fallback")
                    info = rpc({"op": "info", "session": view["name"]})
                    assert info["id"] == view["info"]["id"] and info["pid"] == view["hello"]["pid"]
                    assert same_process(info["pid"], identities[info["pid"]])
                    assert same_process(view["hello"]["task"], identities[view["hello"]["task"]])
                    assert status(view["channel"])["heartbeat"] > before[index]
                    assert view["command_log"].read_text() == "launch\n"
                    assert status(view["channel"])["input_hex"] == b"once\n".hex()
                    def painted():
                        data = view["window"].get_image(25, 80, 1, 1, X.ZPixmap, 0xffffffff).data
                        return tuple(data[:3]) == (97, 153, 23)
                    wait(painted, "software frame pixel")
                report["checks"].append("software pixels recover; same PTYs/tasks progress; no relaunch or input replay")
                # Detach then attach the same terminal using an explicit software config.
                software = directory / "config" / "ztty" / "ztty.conf"
                software.parent.mkdir(parents=True)
                software.write_text("renderer = software\n")
                view = views[0]
                subprocess.run([str(binary), "detach", view["name"]], env=env, check=True, timeout=3, stdout=log)
                attach = subprocess.Popen([str(binary), "attach", view["name"]], env=env, stdout=log, stderr=log)
                launchers.append(attach)
                identities[attach.pid] = process_stat(attach.pid)["start_ticks"]
                info = wait(lambda: (row if (row := rpc({"op": "info", "session": view["name"]}))["attached"] else None), "reattach")
                assert info["pid"] == view["hello"]["pid"] and info["id"] == view["info"]["id"]
                replacement = connection.create_resource_object("window", info["window_id"])
                wait(lambda: text(replacement, "_ZTTY_RENDERER") == "software", "replacement software window")
                rpc({"op": "write", "session": view["name"], "data": "after\n"})
                wait(lambda: status(view["channel"])["input_hex"] == b"once\nafter\n".hex(), "input after reattach")
                assert view["command_log"].read_text() == "launch\n"
                report["checks"].append("software replacement attaches same live PTY and accepts exactly one new input")
                report["ok"] = True
            except NegativeControlPassed:
                pass
            except BaseException as error:
                report["error"] = repr(error)
                raise
            finally:
                if trigger:
                    trigger.unlink(missing_ok=True)
                add_owned_children(identities, [os.getpid()])
                # The test connection must close before its X server. Terminal
                # cleanup includes adopted helpers, but Xvfb has its own owner.
                if server:
                    identities.pop(server.process.pid, None)
                for channel in channels:
                    channel.close()
                if listener:
                    listener.close()
                report["remaining_owned_pids"] = cleanup(identities)
                for launcher in launchers:
                    with contextlib.suppress(subprocess.TimeoutExpired):
                        launcher.wait(timeout=2)
                if connection:
                    connection.close()
                if server:
                    server.close()
                report["stderr"] = (directory / "stderr.log").read_text(errors="replace")
                args.output.parent.mkdir(parents=True, exist_ok=True)
                args.output.write_text(json.dumps(report, indent=2) + "\n")
    assert not report["remaining_owned_pids"], report
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--child":
        child(sys.argv[2], sys.argv[3])
    else:
        main()
