#!/usr/bin/env python3
"""Default backend acceptance on owned Xvfb and headless Sway surfaces.

Requires a Wayland-enabled binary, Sway/swaymsg, Xvfb and python-xlib. All
desktop and broker endpoints are private. Every command invocation appends a
durable record, then completes a real PTY DSR round trip. This checks policy,
surface creation, attachment identity and command execution, not GPU speed.
Run auto_backend_wire.py separately for new-CLI/legacy-broker compatibility.
"""
import argparse
import contextlib
import ctypes
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import select
import shutil
import socket
import subprocess
import sys
import tempfile
import time


def process_record():
    fields = Path("/proc/self/stat").read_text().rsplit(")", 1)[1].split()
    return {"pid": os.getpid(), "start_ticks": int(fields[19])}


def child(ledger, ready, title):
    import tty
    record = process_record()
    fd = os.open(ledger, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
    try:
        data = json.dumps(record).encode() + b"\n"
        assert os.write(fd, data) == len(data)
        os.fsync(fd)
    finally:
        os.close(fd)
    tty.setraw(0)
    os.write(1, ("\x1b]2;" + title + "\x07\x1b[2J\x1b[Hauto backend fixture\x1b[6n").encode())
    deadline, incoming = time.monotonic() + 8, bytearray()
    while time.monotonic() < deadline:
        if select.select([0], [], [], max(0, deadline - time.monotonic()))[0]:
            data = os.read(0, 4096)
            if not data:
                raise RuntimeError("PTY closed before DSR response")
            incoming.extend(data)
            match = re.search(rb"\x1b\[([0-9]+);([0-9]+)R", incoming)
            if match:
                size = os.get_terminal_size(0)
                record.update(dsr=[int(match[1]), int(match[2])], cols=size.columns, rows=size.lines,
                              probe_generation=0)
                temporary = Path(ready).with_suffix(".tmp")
                temporary.write_text(json.dumps(record) + "\n")
                temporary.replace(ready)
                incoming.clear()
                generation = 0
                while True:
                    message = os.read(0, 4096)
                    if not message:
                        return 0
                    if b"\x1d" in message:
                        generation += 1
                        os.write(1, b"\x1b[6n")
                    incoming.extend(message.replace(b"\x1d", b""))
                    reply = re.search(rb"\x1b\[([0-9]+);([0-9]+)R", incoming)
                    if reply:
                        record.update(dsr=[int(reply[1]), int(reply[2])], probe_generation=generation)
                        temporary.write_text(json.dumps(record) + "\n")
                        temporary.replace(ready)
                        incoming.clear()
                    if len(incoming) > 65536:
                        raise RuntimeError("unexpected excessive PTY probe input")
            if len(incoming) > 65536:
                raise RuntimeError("unexpected excessive PTY input")
    raise TimeoutError("PTY did not answer DSR")


def read_json(path):
    try:
        return json.loads(path.read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        return None


def wait(predicate, seconds=10, process=None, log=None):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        if process is not None and process.poll() is not None:
            detail = log.read_text(errors="replace")[-4096:] if log is not None else ""
            raise RuntimeError(f"launcher exited {process.returncode}: {detail}")
        time.sleep(0.02)
    raise TimeoutError("observable test condition did not become true")


def rpc(address, request, timeout=2):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
        channel.settimeout(timeout)
        channel.connect("\0" + address[1:])
        channel.sendall(json.dumps(request).encode() + b"\n")
        data = bytearray()
        while b"\n" not in data:
            chunk = channel.recv(65536)
            if not chunk:
                raise RuntimeError("broker disconnected")
            data.extend(chunk)
            if len(data) > 4 * 1024 * 1024:
                raise RuntimeError("oversized broker reply")
    envelope = json.loads(data.split(b"\n", 1)[0])
    if not envelope.get("ok"):
        raise RuntimeError(envelope.get("error", "broker request failed"))
    return envelope["result"]


def nodes(tree):
    yield tree
    for node in tree.get("nodes", []) + tree.get("floating_nodes", []):
        yield from nodes(node)


def identity(info):
    return {**{key: info[key] for key in
               ("id", "created_at", "pid", "broker_pid", "backend", "window_id", "attachment")},
            "last_view_backend": info.get("last_view_backend")}


class Case:
    def __init__(self, harness, name):
        self.h = harness
        self.name = name
        self.root = harness.output / name
        self.root.mkdir()
        self.tag = "ztty-auto-" + secrets.token_hex(12)
        self.env = harness.env.copy()
        self.env.update(ZTTY_SOCKET="@" + self.tag, ZTTY_MEMORY_TAG=self.tag,
                        XDG_CONFIG_HOME=str(self.root / "config"),
                        XDG_CACHE_HOME=str(self.root / "cache"),
                        XDG_STATE_HOME=str(self.root / "state"))
        config = self.root / "config/ztty/ztty.conf"
        config.parent.mkdir(parents=True)
        config.write_text("font=monospace:pixelsize=16\nbackground-opacity=100\n"
                          "window-padding=8\ncursor-blink=false\n")
        self.processes, self.streams, self.owned, self.expected_counts = [], [], {}, {}
        self.allowed_images = {harness.binary}
        self.details = {"name": name, "passed": False}

    def call(self, op, session=None, **extra):
        request = {"op": op, "actor": "local", **extra}
        if session is not None:
            request["session"] = session
        return rpc(self.env["ZTTY_SOCKET"], request)

    def info(self, name):
        try:
            return self.call("info", name)
        except (OSError, RuntimeError):
            return None

    def remember(self, info):
        broker = info["broker_pid"]
        assert Path(f"/proc/{broker}/exe").resolve() in self.allowed_images, "foreign broker executable"
        marker = ("ZTTY_MEMORY_TAG=" + self.tag).encode()
        assert marker in Path(f"/proc/{broker}/environ").read_bytes().split(b"\0"), "foreign broker tag"
        for pid in (broker, info["pid"]):
            self.owned[pid] = self.h.process_stat(pid)["start_ticks"]

    def spawn(self, arguments, label, env=None):
        log_path = self.root / (label + ".log")
        stream = log_path.open("wb")
        self.streams.append(stream)
        process = subprocess.Popen([str(self.h.binary), *arguments], env=env or self.env,
                                   stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
        self.processes.append(process)
        with contextlib.suppress(OSError):
            self.owned[process.pid] = self.h.process_stat(process.pid)["start_ticks"]
        return process, log_path

    def launch(self, name, backend=None, env=None, target=None, expect=None):
        arguments = [] if backend is None else ["--backend", backend]
        if target is not None:
            arguments += [target[0], target[1]]
        ledger, ready = self.root / (name + ".runs.jsonl"), self.root / (name + ".ready.json")
        arguments += ["--name", name, "--geometry", "80x24", "-e", sys.executable,
                      str(Path(__file__).resolve()), "--child", str(ledger), str(ready), name]
        process, log = self.spawn(arguments, name, env)
        self.expected_counts[ledger] = 0 if expect == "error" else 1
        if expect == "error":
            code = process.wait(timeout=10)
            output = log.read_text(errors="replace")
            assert code != 0, "backend/preflight failure unexpectedly succeeded"
            assert output.strip(), "failure did not provide a diagnostic"
            listing = self.call("list")
            assert all(s["name"] != name for s in listing["sessions"]), "failed launch retained a session"
            self.details.setdefault("rejections", []).append(
                {"session": name, "returncode": code, "diagnostic": output[-4096:]})
            return None
        state = wait(lambda: read_json(ready), process=process, log=log)
        info = wait(lambda: self.info(name), process=process, log=log)
        assert state["pid"] == info["pid"] and state["dsr"] == [1, 21], "wrong PTY or DSR response"
        assert info["backend"] == expect and info["attached"], f"wrong backend: {info}"
        self.remember(info)
        wait(lambda: self.h.surface(info, name), process=process, log=log)
        self.details.setdefault("sessions", []).append({"name": name, **identity(info), "dsr": state["dsr"]})
        return info

    def headless(self, name):
        # Start an empty owned broker through its public CLI. This session has
        # never owned any view, unlike a session created on Wayland then detached.
        process, log = self.spawn(["list"], "start-empty-broker")
        assert process.wait(timeout=8) == 0, log.read_text(errors="replace")
        ledger, ready = self.root / (name + ".runs.jsonl"), self.root / (name + ".ready.json")
        self.expected_counts[ledger] = 1
        info = self.call("create", name=name, headless=True, cols=80, rows=24,
                         command=[sys.executable, str(Path(__file__).resolve()),
                                  "--child", str(ledger), str(ready), name])
        self.remember(info)
        state = wait(lambda: read_json(ready))
        assert state["pid"] == info["pid"] and state["dsr"] == [1, 21]
        assert not info["attached"] and info["backend"] is None and info.get("last_view_backend") is None
        self.details.setdefault("sessions", []).append({"name": name, **identity(info), "dsr": state["dsr"]})
        return info

    def attach(self, name, env=None, observe=False, backend=None):
        arguments = ["attach"]
        if observe:
            arguments += ["--observe"]
        if backend is not None:
            arguments += ["--backend", backend]
        arguments.append(name)
        process, log = self.spawn(arguments, name + "-attach", env)
        def attached():
            info = self.info(name)
            return info if info and info["attached"] and (not observe or info["read_only"]) else None
        info = wait(attached, process=process, log=log)
        self.remember(info)
        wait(lambda: self.h.surface(info, name), process=process, log=log)
        return info

    def close(self):
        # Even a rejected create must not leave an unreported child. Records
        # are written by our own fixture before any PTY interaction.
        for ledger in self.expected_counts:
            if ledger.exists():
                for line in ledger.read_text().splitlines():
                    with contextlib.suppress(json.JSONDecodeError, KeyError):
                        record = json.loads(line)
                        self.owned[record["pid"]] = record["start_ticks"]
        with contextlib.suppress(OSError, RuntimeError):
            for session in self.call("list")["sessions"]:
                with contextlib.suppress(OSError, RuntimeError):
                    self.call("close", session["uri"])
        found = {}
        for executable in self.allowed_images:
            found.update(self.h.owned_processes(executable, self.tag))
        for pid in found:
            self.owned.update(self.h.descendants(pid))
        self.owned.update(found)
        for process in self.processes:
            if process.pid in self.owned and self.h.same_process(process.pid, self.owned[process.pid]):
                self.owned.update(self.h.descendants(process.pid))
        remaining = self.h.cleanup(self.owned)
        for process in self.processes:
            with contextlib.suppress(subprocess.TimeoutExpired):
                process.wait(timeout=1)
        for stream in self.streams:
            stream.close()
        records = []
        for ledger, count in self.expected_counts.items():
            lines = ledger.read_text().splitlines() if ledger.exists() else []
            records.append({"ledger": ledger.name, "expected": count, "actual": len(lines),
                            "processes": [json.loads(line) for line in lines]})
        self.details["command_executions"] = records
        self.details["remaining_owned_pids"] = remaining
        self.details["passed"] = (self.details["passed"] and not remaining
                                  and all(r["expected"] == r["actual"] for r in records))


class Harness:
    def __init__(self, args, runtime):
        self.binary, self.output = args.binary.resolve(), args.output.resolve()
        self.args, self.runtime = args, runtime
        sys.path.insert(0, str(args.repo_root.resolve() / "tools"))
        from isolated_xvfb import start_xvfb
        from measure_runtime import process_stat, descendants, same_process
        from measure_multiwindow import cleanup, owned_processes
        self.process_stat, self.descendants = process_stat, descendants
        self.same_process, self.cleanup, self.owned_processes = same_process, cleanup, owned_processes
        self.xserver = self.xconnection = self.compositor = None
        self.streams, self.compositor_identity = [], None
        self.env = os.environ.copy()
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "SWAYSOCK", "XAUTHORITY", "ZTTY_SOCKET", "LD_PRELOAD"):
            self.env.pop(key, None)
        self.env.update(XDG_RUNTIME_DIR=str(runtime), WLR_BACKENDS="headless",
                        WLR_HEADLESS_OUTPUTS="1", WLR_RENDERER="pixman",
                        WLR_LIBINPUT_NO_DEVICES="1", LIBGL_ALWAYS_SOFTWARE="1", LP_NUM_THREADS="2",
                        XDG_CONFIG_HOME=str(runtime / "config"), XKB_DEFAULT_LAYOUT="us",
                        XMODIFIERS="@im=none", LC_ALL="C.UTF-8")
        self.start_xvfb = start_xvfb

    def start(self):
        from Xlib import display
        config = self.runtime / "sway.conf"
        config.write_text("xwayland disable\noutput HEADLESS-1 mode 1280x720\n"
                          "output * bg #111111 solid_color\nseat seat0 fallback true\n"
                          "default_border none\nfocus_follows_mouse no\n")
        log = (self.output / "sway.log").open("wb")
        self.streams.append(log)
        self.compositor = subprocess.Popen([str(self.args.sway), "--debug", "--unsupported-gpu", "-c", str(config)],
                                          env=self.env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        self.compositor_identity = self.process_stat(self.compositor.pid)["start_ticks"]
        ipc_path = wait(lambda: next(self.runtime.glob("sway-ipc.*.sock"), None), process=self.compositor)
        wayland_path = wait(lambda: next((p for p in self.runtime.glob("wayland-*") if p.is_socket()), None),
                            process=self.compositor)
        self.env.update(SWAYSOCK=str(ipc_path), WAYLAND_DISPLAY=wayland_path.name)
        self.ipc("get_tree")
        log = (self.output / "xvfb.log").open("wb")
        self.streams.append(log)
        self.xserver = self.start_xvfb(screen="1280x720x24", stderr=log, env=self.env)
        self.xconnection = display.Display(self.xserver.display)
        self.env["DISPLAY"] = self.xserver.display

    def ipc(self, kind):
        result = subprocess.run([str(self.args.swaymsg), "-r", "-t", kind], env=self.env,
                                capture_output=True, text=True, timeout=4)
        if result.returncode:
            raise RuntimeError(result.stderr or result.stdout)
        return json.loads(result.stdout)

    def surface(self, info, name):
        if info["backend"] == "wayland":
            return any(node.get("app_id") == "org.project-z.ztty" and node.get("pid") == info["broker_pid"]
                       and node.get("name") in (name, name + " · observe") for node in nodes(self.ipc("get_tree")))
        from Xlib import error
        try:
            window = self.xconnection.create_resource_object("window", info["window_id"])
            prop = window.get_full_property(self.xconnection.intern_atom("_NET_WM_PID"), 0)
            return window.get_attributes().map_state == 2 and prop is not None and int(prop.value[0]) == info["broker_pid"]
        except error.XError:
            return False

    def close(self):
        remaining = []
        if self.xconnection is not None:
            self.xconnection.close()
        if self.xserver is not None:
            self.xserver.close()
        if self.compositor is not None and self.compositor_identity is not None:
            owned = {self.compositor.pid: self.compositor_identity}
            if self.same_process(self.compositor.pid, self.compositor_identity):
                owned.update(self.descendants(self.compositor.pid))
            remaining = self.cleanup(owned)
            with contextlib.suppress(subprocess.TimeoutExpired):
                self.compositor.wait(timeout=1)
        for stream in self.streams:
            stream.close()
        return remaining


def without_display(env):
    env = env.copy()
    env.pop("DISPLAY", None)
    return env


def bad_x11(env):
    return {**env, "DISPLAY": "ztty-intentionally-invalid-display"}


def bad_wayland(env):
    return {**env, "WAYLAND_DISPLAY": "missing-ztty-auto-" + secrets.token_hex(12)}


def create_case(case, backend=None, display_mode="both", expected="x11"):
    env = case.env.copy()
    if display_mode == "no_x11":
        env = without_display(env)
    elif display_mode == "bad_x11":
        env = bad_x11(env)
    elif display_mode == "bad_wayland":
        env = bad_wayland(env)
    elif display_mode == "neither":
        env = bad_wayland(bad_x11(env))
    case.launch("terminal", backend=backend, env=env, expect=expected)


def preserve_attachment(case, backend):
    initial = case.launch("terminal", backend=backend, expect=backend)
    # An observer transition proves the CLI completed attach rather than merely
    # remaining alive while the unchanged pre-existing window was polled.
    after = case.attach("terminal", env=bad_wayland(bad_x11(case.env)), observe=True)
    assert identity(initial) == identity(after), "Auto moved/replaced an already attached view"
    case.details["attachment_identity_unchanged"] = identity(after)


def native_detach_x11(case):
    initial = case.launch("terminal", backend="wayland", expect="wayland")
    case.call("detach", "terminal")
    after = case.attach("terminal", backend="x11")
    assert after["backend"] == "x11" and after["pid"] == initial["pid"], "explicit attach did not reuse PTY on X11"
    assert after.get("last_view_backend") == "x11", "explicit attachment did not update backend provenance"
    case.details["reattached"] = identity(after)


def native_detach_restore(case, both_displays):
    initial = case.launch("terminal", env=without_display(case.env), expect="wayland")
    assert initial.get("last_view_backend") == "wayland"
    case.call("detach", "terminal")
    detached = case.info("terminal")
    assert not detached["attached"] and detached.get("last_view_backend") == "wayland"
    env = case.env if both_displays else without_display(case.env)
    after = case.attach("terminal", env=env)
    assert after["backend"] == "wayland" and after["pid"] == initial["pid"], "Auto did not restore remembered native view"
    assert after.get("last_view_backend") == "wayland"
    assert after["broker_pid"] == initial["broker_pid"] and after["attachment"] != initial["attachment"]
    case.details["reattached"] = identity(after)


def native_provenance_through_reload(case):
    initial = case.launch("terminal", env=without_display(case.env), expect="wayland")
    broker = initial["broker_pid"]
    broker_ticks = case.h.process_stat(broker)["start_ticks"]
    child_ticks = case.h.process_stat(initial["pid"])["start_ticks"]
    case.call("detach", "terminal")
    detached = case.info("terminal")
    assert detached.get("last_view_backend") == "wayland" and not detached["attached"]
    # As in reload_runtime.py, a distinct ELF with an ignored tail makes the
    # replacement observable without building or installing another runtime.
    target = case.h.runtime / "auto-backend-reload-image"
    target.write_bytes(case.h.binary.read_bytes() + b"\nztty-auto-backend-provenance-reload\n")
    target.chmod(0o700)
    target = target.resolve()
    target_digest = hashlib.sha256(target.read_bytes()).hexdigest()
    case.allowed_images.add(target)
    process, log = case.spawn(["reload", "--binary", str(target)], "reload-provenance")
    assert process.wait(timeout=30) == 0, log.read_text(errors="replace")[-4096:]
    reply = json.loads(log.read_text())
    assert reply.get("reloaded") is True and reply.get("broker_pid") == broker
    assert Path(f"/proc/{broker}/exe").resolve() == target
    assert hashlib.sha256(Path(f"/proc/{broker}/exe").read_bytes()).hexdigest() == target_digest
    assert case.h.process_stat(broker)["start_ticks"] == broker_ticks, "reload replaced broker process"
    after_reload = case.info("terminal")
    assert identity(after_reload) == identity(detached), "reload lost detached session identity or backend provenance"
    assert case.h.process_stat(initial["pid"])["start_ticks"] == child_ticks, "reload replaced the command"
    attached = case.attach("terminal", env=without_display(case.env))
    assert attached["backend"] == "wayland" and attached.get("last_view_backend") == "wayland"
    assert attached["pid"] == initial["pid"] and attached["broker_pid"] == broker
    assert case.h.process_stat(initial["pid"])["start_ticks"] == child_ticks
    case.call("write", "terminal", data="\x1d")
    probe = wait(lambda: (value := read_json(case.root / "terminal.ready.json"))
                 and value.get("probe_generation") == 1 and value)
    assert probe["pid"] == initial["pid"] and probe["start_ticks"] == child_ticks
    assert probe["dsr"] == [1, 21], "restored PTY did not answer the new command's DSR"
    case.details["reload"] = {"target_sha256": target_digest, "broker_start_ticks": broker_ticks,
                              "child_start_ticks": child_ticks, "reply": reply,
                              "detached_after_reload": identity(after_reload), "reattached": identity(attached),
                              "post_reload_pty_probe": probe}


def existing_x11_guard(case):
    initial = case.launch("original", expect="x11")
    case.launch("rejected", env=bad_x11(case.env), expect="error")
    after = case.info("original")
    assert identity(after) == identity(initial) and after["running"], "failed Auto fallback changed existing X11 session"
    assert len(case.call("list")["sessions"]) == 1
    case.details["original_unchanged"] = identity(after)


def existing_native_allows_fallback(case):
    initial = case.launch("original", backend="wayland", env=without_display(case.env), expect="wayland")
    after = case.launch("additional", env=bad_x11(case.env), expect="wayland")
    assert initial["broker_pid"] == after["broker_pid"] and initial["pid"] != after["pid"]


def headless_guard(case):
    initial = case.headless("original")
    arguments = ["attach", "original"]
    process, log = case.spawn(arguments, "guarded-attach", bad_x11(case.env))
    assert process.wait(timeout=10) != 0, "Auto native attach bypassed never-attached headless-pool reload guard"
    after = case.info("original")
    assert not after["attached"] and after["pid"] == initial["pid"] and after["running"]
    assert identity(after) == identity(initial)
    case.details["rejection"] = log.read_text(errors="replace")[-4096:]


def remembered_x11_no_migration(case):
    initial = case.launch("original", expect="x11")
    native = case.launch("native-anchor", backend="wayland", expect="wayland")
    case.call("detach", "original")
    detached = case.info("original")
    assert detached.get("last_view_backend") == "x11"
    process, log = case.spawn(["attach", "original"], "remembered-x11-attach", bad_x11(case.env))
    assert process.wait(timeout=10) != 0, "Auto migrated a remembered X11 view to native Wayland"
    after = case.info("original")
    assert identity(after) == identity(detached) and after["running"]
    assert after["pid"] == initial["pid"] and not after["attached"]
    assert identity(case.info("native-anchor")) == identity(native)
    case.details["rejection"] = log.read_text(errors="replace")[-4096:]


def workspace_success(case):
    initial = case.launch("original", expect="x11")
    for index, mode in enumerate(("--tab", "--split-right", "--split-down")):
        info = case.launch("member" + str(index), target=(mode, "original"), expect="x11")
        assert info["broker_pid"] == initial["broker_pid"]
        assert info["workspace"] is not None
    infos = case.call("list")["sessions"]
    assert len({s["workspace"] for s in infos}) == 1 and len({s["pid"] for s in infos}) == 4
    case.details["workspace_members"] = len(infos)


def workspace_rejection(case, kind):
    if kind == "missing":
        case.launch("anchor", expect="x11")
        target = "does-not-exist"
    else:
        backend = "wayland" if kind == "native" else "x11"
        case.launch("anchor", backend=backend, expect=backend)
        target = "anchor"
        if kind == "detached":
            case.call("detach", "anchor")
    original = case.info("anchor")
    for index, mode in enumerate(("--tab", "--split-right", "--split-down")):
        case.launch("rejected" + str(index),
                    backend="wayland" if kind == "explicit-wayland" else None,
                    env=bad_x11(case.env) if kind == "bad-x11" else None,
                    target=(mode, target), expect="error")
    after = case.info("anchor")
    assert identity(after) == identity(original) and after["running"], "workspace failure changed anchor"
    assert len(case.call("list")["sessions"]) == 1


def scenarios():
    return [
        ("default_both_prefers_x11", lambda c: create_case(c)),
        ("explicit_auto_both_prefers_x11", lambda c: create_case(c, backend="auto")),
        ("auto_without_display_falls_back_once", lambda c: create_case(c, display_mode="no_x11", expected="wayland")),
        ("auto_bad_display_falls_back_once", lambda c: create_case(c, display_mode="bad_x11", expected="wayland")),
        ("explicit_x11_does_not_fallback", lambda c: create_case(c, backend="x11", display_mode="bad_x11", expected="error")),
        ("explicit_wayland_does_not_fallback", lambda c: create_case(c, backend="wayland", display_mode="bad_wayland", expected="error")),
        ("auto_neither_backend_executes_nothing", lambda c: create_case(c, display_mode="neither", expected="error")),
        ("auto_attach_keeps_x11", lambda c: preserve_attachment(c, "x11")),
        ("auto_attach_keeps_native", lambda c: preserve_attachment(c, "wayland")),
        ("detached_native_explicit_x11_reuses_pty", native_detach_x11),
        ("detached_native_auto_restores_without_x11", lambda c: native_detach_restore(c, False)),
        ("detached_native_auto_restores_with_both_displays", lambda c: native_detach_restore(c, True)),
        ("detached_native_provenance_survives_reload", native_provenance_through_reload),
        ("detached_x11_auto_does_not_migrate_to_native", remembered_x11_no_migration),
        ("existing_x11_blocks_auto_native", existing_x11_guard),
        ("existing_native_allows_auto_native", existing_native_allows_fallback),
        ("never_attached_headless_pool_blocks_auto_native", headless_guard),
        ("workspace_auto_keeps_tab_and_splits", workspace_success),
        *[("workspace_" + kind.replace("-", "_") + "_preflight", lambda c, kind=kind: workspace_rejection(c, kind))
          for kind in ("missing", "native", "detached", "explicit-wayland", "bad-x11")],
    ]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--sway", type=Path, default=Path(shutil.which("sway") or "/nonexistent/sway"))
    parser.add_argument("--swaymsg", type=Path)
    args = parser.parse_args()
    args.sway = args.sway.resolve()
    args.swaymsg = args.swaymsg.resolve() if args.swaymsg else args.sway.with_name("swaymsg")
    if not all(p.is_file() for p in (args.binary, args.sway, args.swaymsg)):
        parser.error("--binary, --sway and --swaymsg must name executable files")
    if not (args.repo_root / "tools/isolated_xvfb.py").is_file():
        parser.error("--repo-root must contain the existing tools/isolated_xvfb.py helper")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    report = {"schema": 1, "scope": "private Xvfb + headless Sway, software Mesa; real views and PTYs",
              "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
              "test_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "checks": [], "passed": False,
              "limits": ["No physical desktop, output-scale or GPU performance claim",
                         "Successful fallback injection uses initial X11 connection failure",
                         "Zero command records show no command execution; they do not alone prove zero fork syscalls",
                         "The existing headless/native guard is tested without exercising executable reload itself"]}
    harness = None
    try:
        assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0, "cannot own/reap broker descendants"
        with tempfile.TemporaryDirectory(prefix="ztty-auto-gui-") as temporary:
            runtime = Path(temporary)
            os.chmod(runtime, 0o700)
            harness = Harness(args, runtime)
            try:
                harness.start()
                report["display_ownership"] = harness.xserver.evidence
                for name, function in scenarios():
                    case = Case(harness, name)
                    try:
                        function(case)
                        case.details["passed"] = True
                    except Exception as error:
                        case.details["error"] = f"{type(error).__name__}: {error}"
                    finally:
                        try:
                            case.close()
                        except Exception as error:
                            case.details["passed"] = False
                            case.details["cleanup_error"] = f"{type(error).__name__}: {error}"
                        report["checks"].append(case.details)
            finally:
                report["remaining_owned_helpers"] = harness.close()
        report["passed"] = (len(report["checks"]) == len(scenarios())
                            and all(c["passed"] for c in report["checks"])
                            and not report["remaining_owned_helpers"])
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
    try:
        report["binary_sha256_after"] = hashlib.sha256(args.binary.read_bytes()).hexdigest()
        report["binary_unchanged"] = report["binary_sha256_after"] == report["binary_sha256"]
    except OSError as error:
        report["binary_unchanged"] = False
        report["binary_after_error"] = f"{type(error).__name__}: {error}"
    report["passed"] = report["passed"] and report["binary_unchanged"]
    report["elapsed_seconds"] = time.monotonic() - started
    (output / "validation.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "checks": len(report["checks"]),
                      "report": str(output / "validation.json")}))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    if len(sys.argv) == 5 and sys.argv[1] == "--child":
        raise SystemExit(child(*sys.argv[2:]))
    raise SystemExit(main())
