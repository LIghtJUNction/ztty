#!/usr/bin/env python3
"""Sample real visible X11 pixels throughout tab switches and window focus/raise.

Runs only owned processes, PTYs and a private Xvfb. Every application sample
must equal one complete, known frame; black gaps and mixtures cannot pass merely
because the final screenshot is correct. A real covering window calibrates the
sampling path. These finite X11 samples do not measure a physical compositor or
prove that no transition shorter than the recorded sampling gaps ever occurred.
"""
import argparse
import contextlib
import ctypes
import hashlib
import json
import os
from pathlib import Path
import select
import secrets
import socket
import subprocess
import sys
import tempfile
import threading
import time
import traceback
import tty

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from isolated_xvfb import start_xvfb
from gui_renderer import record_renderer
from measure_runtime import process_stat, same_process
from measure_multiwindow import cleanup
from repaint_gui import add_owned_children

COLS, ROWS = 64, 18
BACKGROUND = (3, 5, 7)
COLORS = {"A": (215, 31, 49), "B": (22, 169, 203),
          "C": (47, 195, 83), "D": (211, 157, 23),
          "calibration": (241, 11, 239)}


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def send(channel, value):
    channel.sendall(json.dumps(value).encode() + b"\n")


def receive(stream):
    data = stream.readline(1024 * 1024 + 1)
    if not data or not data.endswith(b"\n") or len(data) > 1024 * 1024:
        raise RuntimeError("missing or oversized reply")
    return json.loads(data)


def wait(predicate, label, seconds=5):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        try:
            value = predicate()
            if value:
                return value
        except (FileNotFoundError, ConnectionRefusedError):
            pass
        time.sleep(0.001)
    raise TimeoutError(label)


def frame(key):
    r, g, b = COLORS[key]
    return f"\x1b[?25l\x1b[0m\x1b[48;2;{r};{g};{b}m\x1b[2J\x1b[H\x1b[0m"


def child(path, name, key):
    tty.setraw(0)
    channel = socket.socket(socket.AF_UNIX)
    channel.connect(path)
    stream = channel.makefile("rb")
    incoming = bytearray()
    nonce = secrets.token_hex(16)

    def write(text):
        pending = memoryview(text.encode())
        while pending:
            count = os.write(1, pending)
            if count <= 0:
                raise OSError("PTY write made no progress")
            pending = pending[count:]

    def state():
        return {"ok": True, "name": name, "pid": os.getpid(),
                "parent_pid": os.getppid(), "nonce": nonce,
                "tty_rdev": os.fstat(0).st_rdev,
                "input_hex": incoming.hex()}

    write(frame(key) + "\x1b]2;" + name + "-ready\x07")
    send(channel, state())
    while True:
        ready, _, _ = select.select([0, channel], [], [])
        if 0 in ready:
            data = os.read(0, 4096)
            if not data:
                return
            incoming.extend(data)
            if len(incoming) > 65536:
                raise RuntimeError("unexpected test input growth")
        if channel in ready:
            request = receive(stream)
            if request["op"] == "write":
                write(request["text"])
            elif request["op"] == "quit":
                send(channel, state())
                return
            elif request["op"] != "status":
                raise ValueError(request["op"])
            send(channel, state())


class PixelSampler:
    """Use an independent X connection so broker calls cannot pause sampling."""

    def __init__(self, display_name, drawable, box, scope):
        self.display_name, self.drawable = display_name, drawable
        self.box, self.scope = box, scope
        _, _, width, height = box
        self.references = {name: bytes(color) * (width * height)
                           for name, color in COLORS.items()}
        self.hashes = {name: hashlib.sha256(data).hexdigest()
                       for name, data in self.references.items()}
        self.condition = threading.Condition()
        self.active = self.inflight = None
        self.stopping = False
        self.error = None
        self.latest = None
        self.phases = []
        self.thread = threading.Thread(target=self._worker, daemon=True,
                                       name="ztty-visible-pixels-" + scope)
        self.thread.start()

    def _worker(self):
        from PIL import Image
        from Xlib import X, display
        connection = None
        try:
            connection = display.Display(self.display_name)
            window = connection.create_resource_object("window", self.drawable)
            x, y, width, height = self.box
            while True:
                with self.condition:
                    self.condition.wait_for(lambda: self.stopping or self.active is not None)
                    if self.stopping:
                        break
                    phase = self.active
                    self.inflight = phase
                started = time.monotonic_ns()
                raw = window.get_image(x, y, width, height, X.ZPixmap, 0xFFFFFFFF)
                ended = time.monotonic_ns()
                rgb = Image.frombytes("RGB", (width, height), raw.data, "raw", "BGRX").tobytes()
                checksum = hashlib.sha256(rgb).hexdigest()
                valid = checksum in phase["allowed_hashes"]
                sample = {"start_ns": started, "end_ns": ended,
                          "rgb_sha256": checksum, "valid": valid}
                with self.condition:
                    phase["samples"].append(sample)
                    if not valid and checksum not in phase["fault_rgb"] and len(phase["fault_rgb"]) < 8:
                        phase["fault_rgb"][checksum] = rgb
                    if len(phase["samples"]) > 200_000:
                        raise RuntimeError("unexpected excessive sampler duration")
                    self.latest = sample
                    self.inflight = None
                    self.condition.notify_all()
        except Exception as error:
            with self.condition:
                self.error = repr(error)
                self.inflight = None
                self.condition.notify_all()
        finally:
            if connection is not None:
                connection.close()

    def _healthy(self):
        if self.error:
            raise RuntimeError("pixel sampler failed: " + self.error)

    def begin(self, label, allowed, kind="application"):
        with self.condition:
            self._healthy()
            assert self.active is None and self.inflight is None
            phase = {"name": label, "scope": self.scope, "kind": kind,
                     "allowed_frames": list(allowed),
                     "allowed_hashes": {self.hashes[name] for name in allowed},
                     "samples": [], "fault_rgb": {}, "actions": []}
            self.phases.append(phase)
            self.active = phase
            self.condition.notify_all()
            ready = self.condition.wait_for(lambda: self.error or phase["samples"], timeout=3)
            self._healthy()
            if not ready:
                raise TimeoutError("sampler did not produce its first real pixel sample")
            return phase

    def seen(self, key, after_ns=0):
        with self.condition:
            self._healthy()
            return (self.latest is not None and self.latest["start_ns"] >= after_ns
                    and self.latest["rgb_sha256"] == self.hashes[key])

    def finish(self):
        with self.condition:
            self._healthy()
            phase = self.active
            assert phase is not None
            self.active = None
            drained = self.condition.wait_for(lambda: self.error or self.inflight is None, timeout=3)
            self._healthy()
            if not drained:
                raise TimeoutError("sampler did not complete its last capture")
            assert len(phase["samples"]) >= 2, "insufficient continuous pixel samples"
            return phase

    def close(self):
        with self.condition:
            self.stopping = True
            self.active = None
            self.condition.notify_all()
        self.thread.join(timeout=3)
        if self.thread.is_alive():
            raise RuntimeError("pixel sampler did not stop")
        self._healthy()


def main():
    if len(sys.argv) == 5 and sys.argv[1] == "--child":
        child(sys.argv[2], sys.argv[3], sys.argv[4])
        return 0
    from PIL import Image
    from Xlib import X, XK, display
    from Xlib.ext import xtest

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--renderer", choices=("software", "gpu"), required=True)
    parser.add_argument("--scope", choices=("all", "tabs", "focus"), default="all")
    parser.add_argument("--expect", choices=("clean", "reproduce"), default="clean")
    parser.add_argument("--switches", type=int, default=24,
                        help="switches for each RPC, XTEST and top-level focus/raise stage")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    if not 4 <= args.switches <= 128:
        parser.error("--switches must be between 4 and 128")
    binary, output = args.binary.resolve(strict=True), args.output.resolve()
    screenshots = (args.screenshots or output.with_suffix("")).resolve()
    if output.exists() or screenshots.exists() or output.with_suffix(".samples.jsonl").exists():
        parser.error("use a new output and screenshot directory")
    output.parent.mkdir(parents=True, exist_ok=True)
    screenshots.mkdir(parents=True)
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("PR_SET_CHILD_SUBREAPER failed")
    report = {"ok": False, "binary": str(binary), "binary_sha256": digest(binary),
              "test_script_sha256": digest(__file__), "renderer": args.renderer,
              "requested_scope": args.scope, "expect": args.expect, "checks": [],
              "scope": "Continuous real X11 pixels on private Xvfb; no physical compositor claim",
              "gpu_environment": "LIBGL_ALWAYS_SOFTWARE=1; GPU mode uses the local Mesa software driver",
              "colors": COLORS, "stages": [], "viewports": {}, "renderers": [],
              "process_cleanup_helper_sha256": digest(ROOT / "tests/repaint_gui.py"),
              "helper_sha256": {name: digest(ROOT / "tools" / name) for name in
                  ("isolated_xvfb.py", "gui_renderer.py", "measure_runtime.py", "measure_multiwindow.py")}}
    server = connection = listener = None
    identities, launchers, views, samplers = {}, [], {}, []
    broker = None
    with tempfile.TemporaryDirectory(prefix="ztty-switch-pixels-") as temporary:
        directory = Path(temporary)
        logpath = directory / "stderr.log"
        with logpath.open("wb") as log:
            try:
                server = start_xvfb(screen="2200x1200x24", dpi=96, stderr=log)
                connection = display.Display(server.display)
                root_window = connection.screen().root
                report["display_ownership"] = server.evidence
                env = dict(os.environ, DISPLAY=server.display, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                           LIBGL_ALWAYS_SOFTWARE="1", XDG_RUNTIME_DIR=temporary,
                           XDG_CONFIG_HOME=str(directory / "config"), XDG_CACHE_HOME=str(directory / "cache"),
                           XDG_STATE_HOME=str(directory / "state"), XDG_DATA_HOME=str(directory / "data"),
                           ZTTY_SOCKET="@ztty-switch-" + directory.name)
                for name in ("WAYLAND_DISPLAY", "LD_PRELOAD", "ZTTY_SESSION", "ZTTY_X11_DIAGNOSTIC_CONTROL",
                             "ZTTY_EGL_PIXEL_TRACE", "ZTTY_X11_PRESENT_TRACE"):
                    env.pop(name, None)
                config = directory / "appearance.conf"
                config.write_text("font = monospace:size=11\nbackground = #030507\nforeground = #ffffff\n"
                                  "background-opacity = 100\nbackground-blur = false\ncursor-blink = false\n"
                                  "window-padding = 0\nrenderer = " + args.renderer + "\n")
                control_path = directory / "control.sock"
                listener = socket.socket(socket.AF_UNIX)
                listener.bind(str(control_path))
                listener.listen(4)
                listener.settimeout(12)

                def rpc(request):
                    with socket.socket(socket.AF_UNIX) as channel:
                        channel.settimeout(3)
                        channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                        send(channel, request)
                        reply = receive(channel.makefile("rb"))
                    assert reply.get("ok") is True, reply
                    return reply["result"]

                def info(name):
                    return rpc({"op": "info", "session": name})

                def command(view, op, **fields):
                    send(view["channel"], {"op": op, **fields})
                    result = receive(view["stream"])
                    assert result.get("ok") is True, result
                    return result

                def capture(window, box=None):
                    if box is None:
                        geometry = window.get_geometry()
                        box = (0, 0, geometry.width, geometry.height)
                    x, y, width, height = box
                    raw = window.get_image(x, y, width, height, X.ZPixmap, 0xFFFFFFFF)
                    return Image.frombytes("RGB", (width, height), raw.data, "raw", "BGRX")

                def uniform(window, key, box=None):
                    image = capture(window, box)
                    return image if image.tobytes() == bytes(COLORS[key]) * (image.width * image.height) else None

                def start(name, key, extra=()):
                    nonlocal broker
                    process = subprocess.Popen([str(binary), "--name", name, "--config", str(config),
                        "--geometry", f"{COLS}x{ROWS}", *extra, "-e", sys.executable,
                        str(Path(__file__).resolve()), "--child", str(control_path), name, key],
                        env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
                    launchers.append(process)
                    identities[process.pid] = process_stat(process.pid)["start_ticks"]
                    channel, _ = listener.accept()
                    channel.settimeout(3)
                    stream = channel.makefile("rb")
                    hello = receive(stream)
                    assert hello["name"] == name
                    session = wait(lambda: next((s for s in rpc({"op": "list"})["sessions"]
                                                 if s["name"] == name), None), name + " startup")
                    assert session["pid"] == hello["pid"]
                    if broker is None:
                        broker = session["broker_pid"]
                        identities[broker] = process_stat(broker)["start_ticks"]
                        assert digest(f"/proc/{broker}/exe") == report["binary_sha256"]
                    assert session["broker_pid"] == broker == hello["parent_pid"]
                    identities[hello["pid"]] = process_stat(hello["pid"])["start_ticks"]
                    window = connection.create_resource_object("window", session["window_id"])
                    view = {"name": name, "window": window, "channel": channel,
                            "stream": stream, "hello": hello, "marker": 0, "key": key}
                    views[name] = view
                    wait(lambda: info(name)["title"] == name + "-ready", name + " initial bytes parsed")
                    wait(lambda: uniform(window, key), name + " complete initial real frame")
                    report["renderers"].append({"session": name, **record_renderer(connection, window, args.renderer)})
                    return view

                def emit(view, text):
                    view["marker"] += 1
                    marker = view["name"] + "-stage-" + str(view["marker"])
                    command(view, "write", text=text + "\x1b]2;" + marker + "\x07")
                    # Parser consumption is independent of visible frame publication.
                    wait(lambda: info(view["name"])["title"] == marker, "PTY parse marker")

                def sample_wait(sampler, key, label, after_ns=0):
                    return wait(lambda: sampler.seen(key, after_ns), label)

                def action(view, value):
                    return rpc({"op": "action", "session": view["name"], "action": value})

                def shortcut(view):
                    view["window"].set_input_focus(X.RevertToParent, X.CurrentTime)
                    connection.sync()
                    codes = [connection.keysym_to_keycode(XK.string_to_keysym(name))
                             for name in ("Control_L", "Shift_L", "Prior")]
                    assert all(codes)
                    for code in codes:
                        xtest.fake_input(connection, X.KeyPress, code)
                    for code in reversed(codes):
                        xtest.fake_input(connection, X.KeyRelease, code)
                    connection.sync()

                def calibrate(sampler, parent, x, y, key):
                    phase = sampler.begin("visible-pixel-calibration", [key], kind="calibration")
                    _, _, width, height = sampler.box
                    r, g, b = COLORS["calibration"]
                    cover = parent.create_window(x, y, width, height, 0, X.CopyFromParent,
                        X.InputOutput, X.CopyFromParent, background_pixel=(r << 16) | (g << 8) | b,
                        override_redirect=1)
                    try:
                        cover.map()
                        cover.configure(stack_mode=X.Above)
                        connection.sync()
                        sample_wait(sampler, "calibration", "real covering window reaches sampler")
                    finally:
                        cover.destroy()
                        connection.sync()
                    sample_wait(sampler, key, "original visible pixels after calibration")
                    sampler.finish()
                    assert any(s["rgb_sha256"] == sampler.hashes["calibration"] for s in phase["samples"])
                    phase["calibration_observed"] = True

                if args.scope in ("all", "tabs"):
                    a = start("switch-tab-a", "A")
                    b = start("switch-tab-b", "B", ("--tab", a["name"]))
                    wait(lambda: info(b["name"])["visible"] and not info(a["name"])["visible"], "tab grouping")
                    parent_id = info(b["name"])["workspace"]
                    assert parent_id == info(a["name"])["workspace"] and parent_id is not None
                    parent = connection.create_resource_object("window", parent_id)
                    assert a["window"].query_tree().parent.id == b["window"].query_tree().parent.id == parent_id
                    parent.configure(x=0, y=0)
                    connection.sync()
                    geometry = parent.get_geometry()
                    for view in (a, b):
                        g = view["window"].get_geometry()
                        assert (g.x, g.y, g.width, g.height) == (0, 0, geometry.width, geometry.height), g
                    wait(lambda: uniform(parent, "B"), "whole workspace parent shows B")
                    sampler = PixelSampler(server.display, parent.id,
                                           (0, 0, geometry.width, geometry.height), "tabs")
                    samplers.append(sampler)
                    report["viewports"]["tabs"] = {"drawable": parent.id, "kind": "workspace parent",
                                                     "box": list(sampler.box)}
                    calibrate(sampler, parent, 0, 0, "B")
                    active = b

                    def switch_once(target, method, phase):
                        nonlocal active
                        previous = active
                        started = time.monotonic_ns()
                        if method == "rpc":
                            action(previous, "next-tab")
                        else:
                            shortcut(previous)
                        wait(lambda: info(target["name"])["visible"] and not info(previous["name"])["visible"],
                             "logical tab selection")
                        sample_wait(sampler, target["key"], "complete target frame sampled", started)
                        observed = time.monotonic_ns()
                        # Keep observing after the first correct frame so a
                        # delayed Focus/Expose repaint cannot escape the gate.
                        time.sleep(0.020)
                        sample_wait(sampler, target["key"], "target remains complete after delayed events", observed)
                        phase["actions"].append({"method": method, "from": previous["name"],
                            "to": target["name"], "start_ns": started, "observed_target_ns": observed,
                            "settled_ns": time.monotonic_ns()})
                        active = target

                    first = sampler.begin("first-return-to-hidden-tab", ["A", "B"])
                    switch_once(a, "rpc", first)
                    sampler.finish()
                    for method in ("rpc", "xtest"):
                        phase = sampler.begin("repeated-tab-" + method, ["A", "B"])
                        for _ in range(args.switches):
                            switch_once(b if active is a else a, method, phase)
                        sampler.finish()

                    for index in range(4):
                        # The target PTY changes while its native view is hidden.
                        if active is not b:
                            phase = sampler.begin(f"hidden-output-prepare-{index}", [active["key"], "B"])
                            switch_once(b, "rpc", phase)
                            sampler.finish()
                        next_key = "C" if index % 2 == 0 else "D"
                        emit(a, frame(next_key))
                        a["key"] = next_key
                        phase = sampler.begin(f"hidden-output-return-{index}", ["B", next_key])
                        switch_once(a, "rpc" if index % 2 == 0 else "xtest", phase)
                        sampler.finish()

                    for index in range(3):
                        if active is not b:
                            phase = sampler.begin(f"sync-prepare-{index}", [active["key"], "B"])
                            switch_once(b, "rpc", phase)
                            sampler.finish()
                        emit(a, frame("A"))
                        a["key"] = "A"
                        # Start before sending begin, not after its parser
                        # acknowledgement or sampler setup. This conservative
                        # origin also covers a backend starting its 150 ms
                        # safety clock as soon as begin is parsed.
                        sync_begin_ns = time.monotonic_ns()
                        emit(a, "\x1b[?2026h" + frame("C"))
                        begin_parsed_ns = time.monotonic_ns()
                        held = sampler.begin(f"sync-held-tab-switch-{index}", ["B"])
                        started = time.monotonic_ns()
                        action(b, "next-tab")
                        wait(lambda: info(a["name"])["visible"], "sync target logically selected")
                        # Existing synchronized-output safety deadline is 150 ms.
                        # Keep this explicit hold short and fail if scheduling
                        # prevents testing before that independent deadline.
                        until = time.monotonic() + 0.030
                        while time.monotonic() < until:
                            sampler._healthy()
                            time.sleep(0.001)
                        sampler.finish()
                        held_ms = (time.monotonic_ns() - started) / 1_000_000
                        begin_to_hold_end_ms = (time.monotonic_ns() - sync_begin_ns) / 1_000_000
                        assert begin_to_hold_end_ms < 100, ("sync hold missed its pre-timeout interval", begin_to_hold_end_ms)
                        held["hold_ms"] = held_ms
                        held["sync_timing"] = {"begin_before_write_ns": sync_begin_ns,
                            "begin_parsed_ns": begin_parsed_ns, "switch_requested_ns": started,
                            "begin_to_hold_end_ms": begin_to_hold_end_ms}
                        committed = sampler.begin(f"sync-explicit-commit-{index}", ["B", "C"])
                        release_ns = time.monotonic_ns()
                        assert (release_ns - sync_begin_ns) / 1_000_000 < 120, "explicit commit reached the safety timeout"
                        emit(a, "\x1b[?2026l")
                        end_parsed_ns = time.monotonic_ns()
                        begin_to_end_parsed_ms = (end_parsed_ns - sync_begin_ns) / 1_000_000
                        # emit waits for an OSC marker after end to be parsed.
                        # Merely queueing end before the deadline is insufficient
                        # if the broker consumes it only after an automatic end.
                        assert begin_to_end_parsed_ms < 120, ("explicit end was not parsed before the safety deadline", begin_to_end_parsed_ms)
                        a["key"] = "C"
                        active = a
                        sample_wait(sampler, "C", "complete synchronized frame after explicit release", release_ns)
                        committed["actions"].append({"method": "explicit-sync-end", "start_ns": release_ns,
                            "parsed_ns": end_parsed_ns, "begin_before_write_ns": sync_begin_ns,
                            "begin_to_end_parsed_ms": begin_to_end_parsed_ms,
                            "settled_ns": time.monotonic_ns()})
                        sampler.finish()
                    report["checks"].append("sampled workspace parent through first/repeated RPC and XTEST tab switches, hidden PTY updates and synchronized commit")
                    for view in (a, b):
                        assert command(view, "status")["input_hex"] == "", "tab shortcuts reached the PTY"
                    image = capture(parent)
                    image.save(screenshots / "tabs-final.png")

                if args.scope in ("all", "focus"):
                    a = start("switch-focus-a", "A")
                    b = start("switch-focus-b", "B")
                    assert info(a["name"])["workspace"] is None and info(b["name"])["workspace"] is None
                    geometry = a["window"].get_geometry()
                    g = b["window"].get_geometry()
                    assert (g.width, g.height) == (geometry.width, geometry.height)
                    x, y = 2200 - geometry.width - 20, 40
                    assert x >= 1100, "test windows do not fit in the isolated display"
                    for view in (a, b):
                        assert view["window"].query_tree().parent.id == root_window.id
                        view["window"].configure(x=x, y=y)
                    b["window"].configure(stack_mode=X.Above)
                    b["window"].set_input_focus(X.RevertToParent, X.CurrentTime)
                    connection.sync()
                    box = (x, y, geometry.width, geometry.height)
                    wait(lambda: uniform(root_window, "B", box), "overlapping top-level B visibly covers A")
                    sampler = PixelSampler(server.display, root_window.id, box, "focus_raise")
                    samplers.append(sampler)
                    report["viewports"]["focus_raise"] = {"drawable": root_window.id, "kind": "root visible screen region",
                                                            "box": list(box), "window_ids": [a["window"].id, b["window"].id]}
                    calibrate(sampler, root_window, x, y, "B")
                    active = b
                    phase = sampler.begin("independent-top-level-focus-and-raise", ["A", "B"])
                    for _ in range(args.switches):
                        target = a if active is b else b
                        started = time.monotonic_ns()
                        target["window"].configure(stack_mode=X.Above)
                        target["window"].set_input_focus(X.RevertToParent, X.CurrentTime)
                        connection.sync()
                        assert connection.get_input_focus().focus.id == target["window"].id
                        sample_wait(sampler, target["key"], "focused/raised complete top-level frame", started)
                        observed = time.monotonic_ns()
                        time.sleep(0.020)
                        sample_wait(sampler, target["key"], "complete frame after Focus/Expose processing", observed)
                        phase["actions"].append({"method": "real-XConfigureWindow-Above-and-XSetInputFocus",
                            "from": active["name"], "to": target["name"], "start_ns": started,
                            "observed_target_ns": observed, "settled_ns": time.monotonic_ns()})
                        active = target
                    sampler.finish()
                    image = capture(root_window, box)
                    image.save(screenshots / "focus-raise-final.png")
                    report["checks"].append("separately sampled two independent overlapping top-level windows through real focus and stacking changes")

                assert same_process(broker, identities[broker])
                for name, view in views.items():
                    current = command(view, "status")
                    for field in ("pid", "parent_pid", "nonce", "tty_rdev"):
                        assert current[field] == view["hello"][field], (name, field)
                    assert same_process(current["pid"], identities[current["pid"]])
                    assert info(name)["running"] is True
                assert all(process.poll() is None for process in launchers)
                assert digest(f"/proc/{broker}/exe") == report["binary_sha256"]
                report["checks"].append("all owned commands, PTYs, broker identity and launcher wait peers remain live")
                report["ok"] = True
            except Exception as error:
                report["error"], report["traceback"] = repr(error), traceback.format_exc()
            finally:
                for sampler in samplers:
                    try:
                        sampler.close()
                    except Exception as error:
                        report.setdefault("cleanup_errors", []).append(str(error))
                        report["ok"] = False
                for view in views.values():
                    with contextlib.suppress(Exception):
                        command(view, "quit")
                    with contextlib.suppress(Exception):
                        view["stream"].close()
                    view["channel"].close()
                if "rpc" in locals():
                    for name in views:
                        with contextlib.suppress(Exception):
                            rpc({"op": "close", "session": name})
                add_owned_children(identities, [os.getpid(), *identities])
                if server is not None:
                    identities.pop(server.process.pid, None)
                report["remaining_owned_pids"] = cleanup(identities)
                for process in launchers:
                    with contextlib.suppress(subprocess.TimeoutExpired):
                        process.wait(timeout=1)
                if listener is not None:
                    listener.close()
                if connection is not None:
                    connection.close()
                if server is not None:
                    server.close()
                if report["remaining_owned_pids"]:
                    report["ok"] = False
                log.flush()
                report["stderr_tail"] = logpath.read_text(errors="replace")[-6000:]

    tracepath = output.with_suffix(".samples.jsonl")
    invalid_by_scope = {}
    with tracepath.open("w") as trace:
        for sampler in samplers:
            for phase_index, phase in enumerate(sampler.phases):
                samples = phase["samples"]
                gaps = [(b["start_ns"] - a["start_ns"]) / 1_000_000 for a, b in zip(samples, samples[1:])]
                invalid = sum(not sample["valid"] for sample in samples)
                counts = {}
                for sample in samples:
                    checksum = sample["rgb_sha256"]
                    counts[checksum] = counts.get(checksum, 0) + 1
                    trace.write(json.dumps({"scope": sampler.scope, "stage": phase["name"], **sample}) + "\n")
                details = {key: phase[key] for key in ("name", "scope", "kind", "allowed_frames", "actions")}
                details.update(samples=len(samples), invalid_samples=invalid, frame_counts=counts,
                    allowed_rgb_sha256={name: sampler.hashes[name] for name in phase["allowed_frames"]},
                    duration_ms=(samples[-1]["end_ns"] - samples[0]["start_ns"]) / 1_000_000 if samples else 0,
                    largest_sample_interval_ms=max(gaps, default=0),
                    median_sample_interval_ms=sorted(gaps)[len(gaps) // 2] if gaps else 0,
                    fault_screenshots=[])
                for index, (checksum, rgb) in enumerate(phase["fault_rgb"].items()):
                    image = Image.frombytes("RGB", sampler.box[2:], rgb)
                    path = screenshots / f"{sampler.scope}-{phase_index:02d}-unexpected-{index}.png"
                    image.save(path)
                    details["fault_screenshots"].append({"path": str(path), "sha256": digest(path),
                        "rgb_sha256": checksum, "colors": image.getcolors(16)})
                if "hold_ms" in phase:
                    details["hold_ms"] = phase["hold_ms"]
                    details["sync_timing"] = phase["sync_timing"]
                if phase["kind"] == "calibration":
                    details["calibration_observed"] = phase.get("calibration_observed", False)
                else:
                    invalid_by_scope[sampler.scope] = invalid_by_scope.get(sampler.scope, 0) + invalid
                report["stages"].append(details)
    report["sample_trace"] = {"path": str(tracepath), "sha256": digest(tracepath)}
    report["invalid_samples_by_scope"] = invalid_by_scope
    report["invalid_application_samples"] = sum(invalid_by_scope.values())
    expected = report["invalid_application_samples"] == 0 if args.expect == "clean" else report["invalid_application_samples"] > 0
    if not expected:
        report["ok"] = False
        report.setdefault("error", "pixel result did not match --expect " + args.expect)
    output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "invalid_samples_by_scope": invalid_by_scope,
                      "expect": args.expect, "error": report.get("error"), "output": str(output)}))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
