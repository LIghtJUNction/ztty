#!/usr/bin/env python3
"""Check release bookkeeping when a mouse shortcut leaves an old tab mapped.

A real Ctrl+button8 press selects a synchronized-update-held tab. The release
still belongs to the old native window's implicit grab. After that release has
crossed an actual XTEST-to-PTY input fence, the target commits and receives an
unmodified SGR mouse pair. Only an exact one-time missing release is accepted
with --expect missing-release; timeouts, crashes and unrelated input failures
always fail this fixture.
"""
import argparse
import contextlib
import ctypes
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import tempfile
import time
import traceback

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from isolated_xvfb import start_xvfb
from gui_renderer import record_renderer
from measure_runtime import process_stat, same_process
from measure_multiwindow import cleanup
from repaint_gui import add_owned_children
from tab_switch_pixels import child, frame, digest, send, receive, wait, COLORS, COLS, ROWS

SGR = re.compile(rb"\x1b\[<([0-9]+);([0-9]+);([0-9]+)([Mm])")


def events(data):
    matches = list(SGR.finditer(data))
    if b"".join(match.group(0) for match in matches) != data:
        raise AssertionError(("non-SGR input in mouse probe", data.hex()))
    return [{"button_code": int(match[1]), "column": int(match[2]),
             "row": int(match[3]), "kind": "press" if match[4] == b"M" else "release"}
            for match in matches]


def main():
    if len(sys.argv) == 5 and sys.argv[1] == "--child":
        child(sys.argv[2], sys.argv[3], sys.argv[4])
        return 0
    from PIL import Image
    from Xlib import X, XK, display
    from Xlib.ext import xtest

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--renderer", choices=("software", "gpu"), default="software")
    parser.add_argument("--expect", choices=("complete", "missing-release"), default="complete")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    binary, output = args.binary.resolve(strict=True), args.output.resolve()
    screenshots = (args.screenshots or output.with_suffix("")).resolve()
    if output.exists() or screenshots.exists():
        parser.error("use new output and screenshot paths")
    output.parent.mkdir(parents=True, exist_ok=True)
    screenshots.mkdir(parents=True)
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("PR_SET_CHILD_SUBREAPER failed")
    report = {"ok": False, "binary": str(binary), "binary_sha256": digest(binary),
              "renderer": args.renderer, "expect": args.expect,
              "test_script_sha256": digest(__file__), "checks": [], "parse_markers": [],
              "binding": "mousebind = ctrl+button8 = next-tab",
              "scope": "Owned Xvfb, XTEST, same live PTYs, real visible placeholder and exact SGR input",
              "helper_sha256": {name: digest(ROOT / name) for name in
                  ("tests/tab_switch_pixels.py", "tests/repaint_gui.py", "tools/isolated_xvfb.py",
                   "tools/gui_renderer.py", "tools/measure_runtime.py", "tools/measure_multiwindow.py")}}
    server = connection = listener = None
    broker = None
    identities, views, launchers, snapshots = {}, {}, [], {}
    with tempfile.TemporaryDirectory(prefix="ztty-tab-mouse-") as temporary:
        directory = Path(temporary)
        logpath = directory / "stderr.log"
        with logpath.open("wb") as log:
            try:
                server = start_xvfb(screen="1600x1000x24", dpi=96, stderr=log)
                report["display_ownership"] = server.evidence
                connection = display.Display(server.display)
                env = dict(os.environ, DISPLAY=server.display, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                           LIBGL_ALWAYS_SOFTWARE="1", XDG_RUNTIME_DIR=temporary,
                           XDG_CONFIG_HOME=str(directory / "config-home"), XDG_CACHE_HOME=str(directory / "cache"),
                           XDG_STATE_HOME=str(directory / "state"), XDG_DATA_HOME=str(directory / "data"),
                           ZTTY_SOCKET="@ztty-tab-mouse-" + directory.name)
                for name in ("WAYLAND_DISPLAY", "LD_PRELOAD", "ZTTY_SESSION", "ZTTY_X11_DIAGNOSTIC_CONTROL",
                             "ZTTY_EGL_PIXEL_TRACE", "ZTTY_X11_PRESENT_TRACE"):
                    env.pop(name, None)
                config = directory / "appearance.conf"
                config.write_text("font = monospace:size=11\nbackground = #030507\nforeground = #ffffff\n"
                                  "background-opacity = 100\nbackground-blur = false\ncursor-blink = false\n"
                                  "window-padding = 0\nrenderer = " + args.renderer + "\n"
                                  "mousebind = ctrl+button8 = next-tab\n")
                control = directory / "control.sock"
                listener = socket.socket(socket.AF_UNIX)
                listener.bind(str(control))
                listener.listen(2)
                listener.settimeout(12)

                def rpc(request):
                    with socket.socket(socket.AF_UNIX) as channel:
                        channel.settimeout(3)
                        channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                        send(channel, request)
                        reply = receive(channel.makefile("rb"))
                    assert reply.get("ok") is True, reply
                    return reply["result"]

                def info(view):
                    return rpc({"op": "info", "session": view["name"]})

                def command(view, op, **fields):
                    send(view["channel"], {"op": op, **fields})
                    reply = receive(view["stream"])
                    assert reply.get("ok") is True, reply
                    return reply

                def input_bytes(view):
                    return bytes.fromhex(command(view, "status")["input_hex"])

                def picture(window):
                    geometry = window.get_geometry()
                    raw = window.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xFFFFFFFF)
                    return Image.frombytes("RGB", (geometry.width, geometry.height), raw.data, "raw", "BGRX")

                def color_frame(window, key):
                    image = picture(window)
                    return image if image.tobytes() == bytes(COLORS[key]) * (image.width * image.height) else None

                def start(name, key, extra=()):
                    nonlocal broker
                    launcher = subprocess.Popen([str(binary), "--name", name, "--config", str(config),
                        "--geometry", f"{COLS}x{ROWS}", *extra, "-e", sys.executable,
                        str(Path(__file__).resolve()), "--child", str(control), name, key],
                        env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
                    launchers.append(launcher)
                    identities[launcher.pid] = process_stat(launcher.pid)["start_ticks"]
                    channel, _ = listener.accept()
                    channel.settimeout(3)
                    stream = channel.makefile("rb")
                    hello = receive(stream)
                    assert hello["name"] == name
                    session = wait(lambda: next((s for s in rpc({"op": "list"})["sessions"]
                                                 if s["name"] == name), None), "new owned session")
                    if broker is None:
                        broker = session["broker_pid"]
                        identities[broker] = process_stat(broker)["start_ticks"]
                        assert digest(f"/proc/{broker}/exe") == report["binary_sha256"]
                    assert session["pid"] == hello["pid"] and session["broker_pid"] == broker == hello["parent_pid"]
                    identities[hello["pid"]] = process_stat(hello["pid"])["start_ticks"]
                    view = {"name": name, "channel": channel, "stream": stream, "hello": hello,
                            "window": connection.create_resource_object("window", session["window_id"]), "marker": 0}
                    views[name] = view
                    wait(lambda: info(view)["title"] == name + "-ready", "initial bytes parsed")
                    wait(lambda: color_frame(view["window"], key), "complete initial native frame")
                    report.setdefault("renderers", []).append(record_renderer(connection, view["window"], args.renderer))
                    return view

                def emit(view, text, stage):
                    view["marker"] += 1
                    marker = view["name"] + "-" + str(view["marker"])
                    started = time.monotonic_ns()
                    command(view, "write", text=text + "\x1b]2;" + marker + "\x07")
                    wait(lambda: info(view)["title"] == marker, stage + " parser marker")
                    parsed = time.monotonic_ns()
                    report["parse_markers"].append({"stage": stage, "session": view["name"],
                        "title": marker, "before_write_ns": started, "parsed_ns": parsed})
                    return parsed

                def key(symbol):
                    code = connection.keysym_to_keycode(XK.string_to_keysym(symbol))
                    assert code
                    xtest.fake_input(connection, X.KeyPress, code)
                    xtest.fake_input(connection, X.KeyRelease, code)

                a = start("mouse-target-a", "A")
                b = start("mouse-old-b", "B", ("--tab", a["name"]))
                wait(lambda: info(b)["visible"] and not info(a)["visible"], "initial active tab B")
                parent_id = info(b)["workspace"]
                assert parent_id is not None and info(a)["workspace"] == parent_id
                parent = connection.create_resource_object("window", parent_id)
                wait(lambda: color_frame(parent, "B"), "old page covers workspace")
                assert input_bytes(a) == input_bytes(b) == b""
                geometry = a["window"].get_geometry()
                metadata = info(a)
                assert geometry.width % metadata["cols"] == geometry.height % metadata["rows"] == 0
                cw, ch = geometry.width // metadata["cols"], geometry.height // metadata["rows"]
                col, row = 7, 5
                px, py = (col - 1) * cw + cw // 2, (row - 1) * ch + ch // 2
                b["window"].set_input_focus(X.RevertToParent, X.CurrentTime)
                b["window"].warp_pointer(px, py)
                connection.sync()
                ctrl = connection.keysym_to_keycode(XK.string_to_keysym("Control_L"))
                assert ctrl
                sync_begin_ns = time.monotonic_ns()
                emit(a, "\x1b[?2026h" + frame("C"), "begin synchronized target")
                press_ns = time.monotonic_ns()
                xtest.fake_input(connection, X.KeyPress, ctrl)
                xtest.fake_input(connection, X.ButtonPress, 8)
                connection.sync()
                wait(lambda: info(a)["visible"] and not info(b)["visible"], "custom mouse shortcut selects held target")
                assert b["window"].get_attributes().map_state == X.IsViewable, "old native view retired before its release"
                snapshots["held-before-release"] = color_frame(parent, "B")
                assert snapshots["held-before-release"] is not None, "held target exposed pixels before commit"
                release_ns = time.monotonic_ns()
                xtest.fake_input(connection, X.ButtonRelease, 8)
                xtest.fake_input(connection, X.KeyRelease, ctrl)
                connection.sync()
                assert b["window"].get_attributes().map_state == X.IsViewable
                # An actual input fence, delivered after the old window's
                # implicit-grab release on the same X event queue. The child
                # records raw input without echo, so this does not commit sync.
                a["window"].set_input_focus(X.RevertToParent, X.CurrentTime)
                key("x")
                connection.sync()
                wait(lambda: input_bytes(a) == b"x", "post-release XTEST input fence")
                fenced_ns = time.monotonic_ns()
                assert input_bytes(b) == b"", "custom shortcut leaked to old PTY"
                assert b["window"].get_attributes().map_state == X.IsViewable
                snapshots["held-after-input-fence"] = color_frame(parent, "B")
                assert snapshots["held-after-input-fence"] is not None, "input fence ended synchronization"
                assert (time.monotonic_ns() - sync_begin_ns) / 1_000_000 < 100, "held observation missed pre-timeout interval"
                end_parsed_ns = emit(a, "\x1b[?2026l", "explicit synchronized end")
                assert (end_parsed_ns - sync_begin_ns) / 1_000_000 < 120, "explicit end was parsed too near/after automatic timeout"
                snapshots["committed-target"] = wait(lambda: color_frame(parent, "C"), "target frame commits after explicit end")
                wait(lambda: b["window"].get_attributes().map_state == X.IsUnmapped, "old native placeholder retires")
                report["shortcut"] = {"sync_begin_before_write_ns": sync_begin_ns,
                    "button_press_ns": press_ns, "button_release_ns": release_ns,
                    "input_fence_received_ns": fenced_ns, "explicit_end_parsed_ns": end_parsed_ns,
                    "begin_to_end_parsed_ms": (end_parsed_ns - sync_begin_ns) / 1_000_000,
                    "old_native_mapped_before_and_after_release": True,
                    "old_native_mapped_after_input_fence": True,
                    "target_input_fence_hex": input_bytes(a).hex(), "old_input_hex": input_bytes(b).hex()}
                report["checks"].append("Ctrl+button8 release crossed a real XTEST-to-PTY input fence while the old native placeholder remained mapped")

                emit(a, "\x1b[?1000h\x1b[?1006h", "enable SGR button reporting")
                a["window"].set_input_focus(X.RevertToParent, X.CurrentTime)
                a["window"].warp_pointer(px, py)
                connection.sync()
                modifiers = a["window"].query_pointer().mask & (X.ShiftMask | X.ControlMask | X.Mod1Mask | X.Mod4Mask)
                assert modifiers == 0, ("mouse probe still has modifiers", modifiers)
                expected_press = f"\x1b[<128;{col};{row}M".encode()
                expected_release = f"\x1b[<128;{col};{row}m".encode()

                def mouse_probe(fence):
                    before = input_bytes(a)
                    started = time.monotonic_ns()
                    xtest.fake_input(connection, X.ButtonPress, 8)
                    xtest.fake_input(connection, X.ButtonRelease, 8)
                    key(fence)
                    connection.sync()
                    # Receiving the following key proves that event processing
                    # advanced past the release. An absent release cannot be
                    # mistaken for a delayed event or a timed-out dead process.
                    def fenced_input():
                        current = input_bytes(a)
                        assert current.startswith(before), "PTY input history changed"
                        tail = current[len(before):]
                        return tail if tail.endswith(fence.encode()) else None
                    tail = wait(fenced_input, "normal mouse pair input fence")
                    payload = tail[:-1]
                    return {"before_offset": len(before), "start_ns": started,
                            "fence_received_ns": time.monotonic_ns(), "fence": fence,
                            "raw_hex": tail.hex(), "mouse_payload_hex": payload.hex(),
                            "events": events(payload)}, payload

                first, payload = mouse_probe("z")
                report["first_mouse_probe"] = first
                if payload == expected_press + expected_release:
                    observed = "complete"
                elif payload == expected_press:
                    observed = "missing-release"
                else:
                    raise AssertionError(("unexpected mouse result, not the stale shortcut regression", first))
                control_probe, control_payload = mouse_probe("q")
                report["second_mouse_control"] = control_probe
                assert control_payload == expected_press + expected_release, "subsequent ordinary mouse routing is broken"
                report["observed"] = observed
                report["release_delivered"] = observed == "complete"
                report["regression_detected"] = observed == "missing-release"
                report["expected_mouse_events"] = events(expected_press + expected_release)
                assert same_process(broker, identities[broker])
                for view in (a, b):
                    status = command(view, "status")
                    for field in ("pid", "parent_pid", "nonce", "tty_rdev"):
                        assert status[field] == view["hello"][field], (view["name"], field)
                    assert same_process(status["pid"], identities[status["pid"]])
                    assert info(view)["running"] is True
                assert all(process.poll() is None for process in launchers)
                assert digest(f"/proc/{broker}/exe") == report["binary_sha256"]
                assert input_bytes(b) == b""
                report["checks"].append("first button8 probe completed its input fence; a second probe delivered an exact complete SGR pair")
                report["checks"].append("same live broker and PTYs remained usable; timeout and exit were not treated as reproduction")
                report["ok"] = observed == args.expect
                if not report["ok"]:
                    report["error"] = f"expected {args.expect}, observed {observed}"
            except Exception as error:
                report["error"], report["traceback"] = repr(error), traceback.format_exc()
            finally:
                if connection is not None and "ctrl" in locals():
                    with contextlib.suppress(Exception):
                        xtest.fake_input(connection, X.ButtonRelease, 8)
                        xtest.fake_input(connection, X.KeyRelease, ctrl)
                        connection.sync()
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
    report["screenshots"] = {}
    for name, image in snapshots.items():
        if image is not None:
            path = screenshots / (name + ".png")
            image.save(path)
            report["screenshots"][name] = {"path": str(path), "sha256": digest(path)}
    output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "observed": report.get("observed"),
        "regression_detected": report.get("regression_detected"), "expect": args.expect,
        "error": report.get("error"), "output": str(output)}))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
