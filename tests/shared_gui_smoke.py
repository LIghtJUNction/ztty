#!/usr/bin/env python3
"""Verify event routing and shared resource lifetimes on an owned X server."""
import argparse
import contextlib
import ctypes
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from measure_multiwindow import cleanup, owned_processes, terminal_windows
from measure_runtime import descendants, process_stat
from mcp_transport import INIT, READY, line, packet, send as mcp_send

CHILD = """import os,sys,tty
tty.setraw(0)
out=open(sys.argv[1],"wb",buffering=0)
os.write(1,("\\x1b]2;"+sys.argv[2]+"\\x07"+sys.argv[2]+"  中文 é\\r\\n").encode())
while True:
 data=os.read(0,256)
 if not data: break
 out.write(data)
 os.write(1,data)
"""


def main():
    from Xlib import X, XK, display, protocol
    from Xlib.ext import xtest
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--capacity-only", action="store_true",
                        help="check reserved broker control slots without rerunning GUI lifecycle")
    parser.add_argument("--output", type=Path, default=Path("artifacts/shared-gui-smoke.json"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    result = {"ok": False, "binary": str(binary), "checks": [], "remaining_owned_pids": []}
    ctypes.CDLL(None).prctl(36, 1, 0, 0, 0)
    xvfb = d = None
    processes, identities = [], {}
    with tempfile.TemporaryDirectory(prefix="ztty-shared-gui-") as temporary:
        directory = Path(temporary)
        tag = directory.name
        with (directory / "terminal.log").open("wb") as tlog:
            try:
                rfd, wfd = os.pipe()
                try:
                    xvfb = subprocess.Popen(
                        ["Xvfb", "-displayfd", str(wfd), "-screen", "0", "1920x1200x24", "-nolisten", "tcp"],
                        pass_fds=(wfd,), stdout=subprocess.DEVNULL, stderr=tlog, start_new_session=True)
                finally:
                    os.close(wfd)
                try:
                    if not select.select([rfd], [], [], 10)[0]:
                        raise TimeoutError("Xvfb did not start")
                    number = os.read(rfd, 64).strip().decode()
                finally:
                    os.close(rfd)
                if not number.isdecimal():
                    raise RuntimeError("invalid display number")
                d = display.Display(":" + number)
                env = os.environ.copy()
                env.pop("WAYLAND_DISPLAY", None)
                env.update(DISPLAY=":" + number, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                           ZTTY_MEMORY_TAG=temporary, ZTTY_SOCKET="@ztty-shared-gui-" + tag,
                           XDG_RUNTIME_DIR=temporary)

                def rpc(request, environment=env):
                    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                        channel.settimeout(3)
                        channel.connect("\0" + environment["ZTTY_SOCKET"][1:])
                        channel.sendall(json.dumps(request).encode() + b"\n")
                        data = bytearray()
                        while b"\n" not in data:
                            block = channel.recv(8192)
                            if not block:
                                raise RuntimeError("broker disconnected during " + str(request.get("op")))
                            data.extend(block)
                            if len(data) > 1048576:
                                raise RuntimeError("oversized broker reply")
                    envelope = json.loads(data.split(b"\n", 1)[0])
                    if not envelope["ok"]:
                        raise AssertionError(envelope)
                    return envelope["result"]

                def metadata(environment=env):
                    return {"_env": [(key + "=" + value).encode().hex()
                                     for key, value in environment.items()],
                            "_cwd": str(directory).encode().hex()}

                def wait(predicate, description):
                    deadline = time.monotonic() + 10
                    while time.monotonic() < deadline:
                        value = predicate()
                        if value:
                            return value
                        time.sleep(0.02)
                    raise TimeoutError(description)

                def named(title):
                    for value in terminal_windows(d).values():
                        if value["window"].get_wm_name() == title:
                            return value
                    return None

                def start_window(index, font_size):
                    title = f"shared-window-{index}"
                    command = [str(binary), "--name", title, "--font", f"monospace:size={font_size}",
                               "--geometry", "80x24", "-e", "/usr/bin/python3", "-c", CHILD,
                               str(directory / f"input-{index}"), title]
                    processes.append(subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL,
                                                      stderr=tlog, start_new_session=True))
                    return title

                def open_window(index, font_size):
                    title = start_window(index, font_size)
                    return wait(lambda: named(title), "window " + str(index))

                def send(window, text):
                    window.set_input_focus(X.RevertToParent, X.CurrentTime)
                    d.sync()
                    for letter in text:
                        code = d.keysym_to_keycode(XK.string_to_keysym(letter))
                        if not code:
                            raise RuntimeError("no keycode for " + letter)
                        xtest.fake_input(d, X.KeyPress, code)
                        xtest.fake_input(d, X.KeyRelease, code)
                    d.sync()

                def received(index, expected):
                    path = directory / f"input-{index}"
                    wait(lambda: path.exists() and path.read_bytes() == expected,
                         f"PTY {index} expected {expected!r}")

                def close(window, expected_count):
                    message = protocol.event.ClientMessage(
                        window=window, client_type=d.intern_atom("WM_PROTOCOLS"),
                        data=(32, [d.intern_atom("WM_DELETE_WINDOW"), X.CurrentTime, 0, 0, 0]))
                    window.send_event(message, event_mask=0)
                    d.flush()
                    wait(lambda: len(terminal_windows(d)) == expected_count, "window close")

                if args.capacity_only:
                    bootstrap = subprocess.run([str(binary), "attach"], env=env,
                                               stdout=subprocess.DEVNULL, stderr=tlog, timeout=5)
                    if bootstrap.returncode:
                        raise AssertionError("broker bootstrap failed")
                    created = rpc({"op": "create", "origin": "local", "name": "capacity",
                                   "headless": False, "command": ["/bin/sleep", "300"], **metadata()})
                    wait(lambda: len(terminal_windows(d)) == 1, "capacity test window")
                    payload = json.dumps({"op": "wait", "session": created["uri"],
                                          "attachment": created["attachment"]}).encode() + b"\n"
                    observers = []
                    try:
                        for _ in range(128):
                            channel = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                            channel.settimeout(3)
                            channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                            channel.sendall(payload)
                            observers.append(channel)
                        listed = rpc({"op": "list"})
                        if not any(item["uri"] == created["uri"] for item in listed["sessions"]):
                            raise AssertionError("management list lost live session")
                        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as excess:
                            excess.settimeout(3)
                            excess.connect("\0" + env["ZTTY_SOCKET"][1:])
                            excess.sendall(payload)
                            data = bytearray()
                            while b"\n" not in data:
                                block = excess.recv(8192)
                                if not block:
                                    raise AssertionError("excess waiter closed without error")
                                data.extend(block)
                            rejected = json.loads(data.split(b"\n", 1)[0])
                        if rejected.get("ok") is not False or "wait" not in rejected.get("error", "").lower():
                            raise AssertionError("129th waiter did not get an explicit capacity error")
                        current = rpc({"op": "read", "session": created["uri"]})
                        if not current["running"]:
                            raise AssertionError("control read failed while waiter slots full")
                        closed = rpc({"op": "close", "session": created["uri"]})
                        if not closed["closed"]:
                            raise AssertionError("control close failed while waiter slots full")
                        result["checks"].append({"name": "waiter_capacity_preserves_control_requests",
                                                 "ok": True, "waiters": 128, "windows": 1,
                                                 "excess_waiter_error": rejected["error"]})
                    finally:
                        for channel in observers:
                            channel.close()
                else:
                    titles = [start_window(1, 11), start_window(2, 11), start_window(3, 13)]
                    first, second, third = [
                        wait(lambda title=title: named(title), "concurrent window " + title) for title in titles
                    ]
                    owners = {entry["pid"] for entry in (first, second, third)}
                    if len(owners) != 1:
                        raise AssertionError("multiple GUI owners: " + repr(owners))
                    owner = owners.pop()
                    for item, letter, index in ((first, "a", 1), (second, "b", 2), (third, "c", 3)):
                        send(item["window"], letter)
                        received(index, letter.encode())
                    result["checks"].append({"name": "concurrent_start_and_independent_pty_input", "ok": True, "windows": 3,
                                             "gui_owner_pid": owner, "different_font_sizes": [11, 13]})

                    attached = subprocess.Popen([str(binary), "attach", "shared-window-2"],
                                                env=env, stdout=subprocess.DEVNULL, stderr=tlog,
                                                start_new_session=True)
                    processes.append(attached)
                    wait(lambda: d.get_input_focus().focus.id == second["window"].id, "attach focus")
                    if len(terminal_windows(d)) != 3:
                        raise AssertionError("attach created an extra window")
                    result["checks"].append({"name": "attach_reuses_window", "ok": True})

                    close(first["window"], 2)
                    send(second["window"], "z")
                    received(2, b"bz")
                    received(3, b"c")
                    result["checks"].append({"name": "close_one_preserves_shared_font_peer", "ok": True})

                    close(second["window"], 1)
                    wait(lambda: attached.poll() is not None, "attach launcher completion")
                    fourth = open_window(4, 11)
                    if fourth["pid"] != owner:
                        raise AssertionError("host changed while a window remained")
                    send(fourth["window"], "d")
                    received(4, b"d")
                    send(third["window"], "y")
                    received(3, b"cy")
                    result["checks"].append({"name": "reload_font_group_with_other_windows_alive", "ok": True})

                    fifth = open_window(5, 11)
                    send(fifth["window"], "q")
                    received(5, b"q")
                    # Pause only this owned host so the X server can queue an
                    # Expose followed by DestroyNotify before it dispatches either.
                    # The stale Expose must not terminate every terminal window.
                    os.kill(owner, signal.SIGSTOP)
                    try:
                        fifth["window"].clear_area(exposures=True)
                        fifth["window"].destroy()
                        d.sync()
                    finally:
                        os.kill(owner, signal.SIGCONT)
                    wait(lambda: len(terminal_windows(d)) == 2, "external window destruction")
                    send(third["window"], "z")
                    received(3, b"cyz")
                    result["checks"].append({"name": "external_destroy_preserves_host_and_survivor", "ok": True})

                    close(third["window"], 1)
                    close(fourth["window"], 0)
                    result["checks"].append({"name": "close_all_windows", "ok": True})

                    create = {"op": "create", "origin": "local", "name": "wait-reuse",
                              "headless": False, "command": ["/bin/sleep", "300"],
                              "cols": 80, "rows": 24, **metadata()}
                    old = rpc(create)
                    if old.get("attachment") is None:
                        raise AssertionError("create did not return attachment generation")
                    rpc({"op": "close", "session": old["uri"]})
                    replacement = rpc(create)
                    if replacement["uri"] != old["uri"] or replacement["attachment"] == old["attachment"]:
                        raise AssertionError("replacement did not receive a new generation")
                    delayed = rpc({"op": "wait", "session": old["uri"], "attachment": old["attachment"]})
                    if "exit_code" not in delayed:
                        raise AssertionError("delayed old wait did not resolve")
                    current = rpc({"op": "read", "session": replacement["uri"]})
                    if not current["running"] or current["attachment"] != replacement["attachment"]:
                        raise AssertionError("old wait affected replacement")
                    rpc({"op": "close", "session": replacement["uri"]})
                    result["checks"].append({"name": "same_name_replacement_keeps_old_wait_generation", "ok": True})

                    headless_env = env.copy()
                    headless_env.pop("DISPLAY", None)
                    headless_env["ZTTY_SOCKET"] += "-headless"
                    mcp = subprocess.Popen([str(binary), "mcp"], stdin=subprocess.PIPE,
                                           stdout=subprocess.PIPE, stderr=tlog, env=headless_env,
                                           start_new_session=True)
                    processes.append(mcp)
                    mcp_send(mcp, INIT)
                    if "result" not in line(mcp):
                        raise AssertionError("headless MCP initialize failed")
                    mcp_send(mcp, READY)
                    mcp_id = 1

                    def mcp_tool(name, arguments):
                        nonlocal mcp_id
                        mcp_id += 1
                        mcp_send(mcp, packet("tools/call", mcp_id, {"name": name, "arguments": arguments}))
                        response = line(mcp)
                        if response.get("id") != mcp_id or response["result"]["isError"]:
                            raise AssertionError(response)
                        return response["result"]["structuredContent"]

                    persistent = mcp_tool("session_create", {"name": "headless-first",
                                                            "command": ["/bin/sleep", "300"]})
                    if persistent["attached"] or not persistent["running"]:
                        raise AssertionError("MCP session was not headless")
                    gui_env = headless_env | {"DISPLAY": env["DISPLAY"]}
                    attached = subprocess.Popen([str(binary), "attach", persistent["uri"]],
                                                env=gui_env, stdout=subprocess.DEVNULL, stderr=tlog,
                                                start_new_session=True)
                    processes.append(attached)
                    wait(lambda: len(terminal_windows(d)) == 1, "attach to broker created without DISPLAY")
                    live = rpc({"op": "read", "session": persistent["uri"]}, gui_env)
                    if not live["attached"] or live["broker_pid"] != persistent["broker_pid"]:
                        raise AssertionError("headless attach replaced the broker")
                    result["checks"].append({"name": "headless_mcp_broker_then_display_attach", "ok": True,
                                             "xauth_scope": "isolated Xvfb without authentication"})

                    old_token = live["attachment"]
                    window = next(iter(terminal_windows(d).values()))["window"]
                    close(window, 0)
                    wait(lambda: attached.poll() is not None, "first attachment launcher completion")
                    result["active_stage"] = "persistent reattach"
                    reopened = rpc({"op": "attach", "session": persistent["uri"], **metadata(gui_env)}, gui_env)
                    if reopened["attachment"] == old_token or reopened["pid"] != persistent["pid"]:
                        raise AssertionError("reattach did not preserve PTY and renew attachment")
                    result["active_stage"] = "wait for detached attachment"
                    delayed = rpc({"op": "wait", "session": persistent["uri"], "attachment": old_token}, gui_env)
                    if "exit_code" not in delayed:
                        raise AssertionError("detached attachment wait did not resolve")
                    current = rpc({"op": "read", "session": persistent["uri"]}, gui_env)
                    if not current["running"] or not current["attached"]:
                        raise AssertionError("stale attachment wait affected live view")
                    result["checks"].append({"name": "persistent_reattach_renews_wait_generation", "ok": True})
                    mcp_tool("session_close", {"session": persistent["uri"]})
                    mcp.stdin.close()
                    mcp.wait(timeout=5)
                    mcp.stdout.close()
                    if mcp.returncode != 0:
                        raise AssertionError("MCP transport failed during shutdown")
                result.pop("active_stage", None)
                result["ok"] = True
            except Exception as error:
                result["error"] = repr(error)
            finally:
                owned = owned_processes(binary, temporary)
                for pid in owned:
                    identities.update(descendants(pid))
                identities.update(owned)
                for process in processes:
                    with contextlib.suppress(OSError, ValueError):
                        identities[process.pid] = process_stat(process.pid)["start_ticks"]
                        identities.update(descendants(process.pid))
                result["remaining_owned_pids"] = cleanup(identities)
                if result["remaining_owned_pids"]:
                    result["ok"] = False
                for process in processes:
                    with contextlib.suppress(subprocess.TimeoutExpired):
                        process.wait(timeout=1)
                if d is not None:
                    d.close()
                if xvfb is not None:
                    xvfb.terminate()
                    xvfb.wait(timeout=2)
        result["terminal_stderr"] = (directory / "terminal.log").read_text(errors="replace")[-4096:]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
