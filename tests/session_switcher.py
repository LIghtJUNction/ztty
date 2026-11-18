#!/usr/bin/env python3
"""Exercise session switching and search copying with real X11 keyboard input.

Requires Linux, Xvfb, python-xlib and Pillow. The display, broker, PTYs and
configuration belong to this test; screenshots contain actual native pixels.
"""

import argparse
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

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from isolated_xvfb import start_xvfb


CHILD = r'''import os,sys,tty
tty.setraw(0)
out=open(sys.argv[1],"wb",buffering=0)
os.write(1,("\x1b]2;"+sys.argv[2]+"\x07\r\n  "+sys.argv[2]+
            "\r\n\r\n  Session switcher · 中文\r\n\r\n  READY\r\n").encode())
while True:
    data=os.read(0,4096)
    if not data: break
    out.write(data)
    os.write(1,data)
'''

CONFIG = """font = monospace:size=11
background = #000000
foreground = #e1e7f0
background-opacity = 100
background-blur = false
cursor-blink = false
window-padding = 20
selection-background = #355577
selection-foreground = #ffffff
"""


def main():
    from Xlib import X, XK, display
    from Xlib.ext import xtest
    from PIL import Image, ImageChops

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("artifacts/session-switcher.json"))
    parser.add_argument("--screenshots", type=Path, default=Path("artifacts/session-switcher"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    repository = Path(__file__).resolve().parent.parent
    report = {"ok": False, "checks": [], "screenshots": {},
              "platform": "Linux X11 / private Xvfb", "binary": str(binary),
              "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "source_sha256": {}}
    for name in ("src/actions.rs", "src/session.rs", "src/search.rs", "src/selection.rs",
                 "src/x11.rs", "crates/zterm/src/terminal.rs", "tests/session_switcher.py"):
        path = repository / name
        if path.is_file():
            report["source_sha256"][name] = hashlib.sha256(path.read_bytes()).hexdigest()
    owned = []
    children = {}
    broker_pid = broker_start = xvfb = connection = None
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("PR_SET_CHILD_SUBREAPER failed")

    def start_ticks(pid):
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]

    def wait(predicate, label):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                value = predicate()
                if value:
                    return value
            except (FileNotFoundError, ConnectionRefusedError):
                pass
            time.sleep(0.02)
        raise TimeoutError(label)

    with tempfile.TemporaryDirectory(prefix="ztty-switcher-") as temporary:
        root = Path(temporary)
        log = (root / "stderr.log").open("wb")
        try:
            xvfb = start_xvfb(screen="1600x1000x24", stderr=log, timeout=10)
            connection = display.Display(xvfb.display)
            report["display_ownership"] = xvfb.evidence
            env = dict(os.environ, DISPLAY=xvfb.display, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                       XDG_RUNTIME_DIR=temporary, XDG_CONFIG_HOME=str(root / "config-home"),
                       ZTTY_SOCKET="@ztty-switcher-" + root.name)
            env.pop("WAYLAND_DISPLAY", None)
            env.pop("LD_PRELOAD", None)
            config = root / "config-home" / "ztty" / "config"
            config.parent.mkdir(parents=True)
            config.write_text(CONFIG)
            inputs = {}

            def rpc(request, success=True):
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                    channel.settimeout(5)
                    channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                    channel.sendall(json.dumps(request).encode() + b"\n")
                    data = bytearray()
                    while b"\n" not in data:
                        part = channel.recv(8192)
                        if not part:
                            raise RuntimeError("broker disconnected")
                        data.extend(part)
                        if len(data) > 1048576:
                            raise RuntimeError("oversized broker response")
                response = json.loads(data.split(b"\n", 1)[0])
                assert response["ok"] is success, response
                return response["result"] if success else response

            def info(selector):
                return rpc({"op": "info", "session": selector})

            def screen(selector):
                return rpc({"op": "read", "session": selector})["screen"]

            def register(view, path):
                nonlocal broker_pid, broker_start
                if broker_pid is None:
                    broker_pid, broker_start = view["broker_pid"], start_ticks(view["broker_pid"])
                    assert Path(f"/proc/{broker_pid}/exe").resolve() == binary
                assert view["broker_pid"] == broker_pid
                inputs[view["uri"]] = path
                children[view["pid"]] = start_ticks(view["pid"])
                wait(lambda: "READY" in screen(view["uri"]), view["uri"] + " PTY output")
                return info(view["uri"])

            def start(name, title, extra=()):
                path = root / (name + ".input")
                process = subprocess.Popen(
                    [str(binary), "--name", name, "--config", str(config), "--geometry", "100x26",
                     *extra, "-e", "/usr/bin/python3", "-c", CHILD, str(path), title],
                    env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True,
                )
                owned.append(process)
                wait(lambda: process.poll() is None and any(s["name"] == name
                     for s in rpc({"op": "list"})["sessions"]), name + " startup")
                return register(info(name), path)

            def input_bytes(selector):
                path = inputs[selector]
                return path.read_bytes() if path.exists() else b""

            def window(selector):
                return connection.create_resource_object("window", info(selector)["window_id"])

            def focused(selector):
                connection.sync()
                focus = connection.get_input_focus().focus
                return getattr(focus, "id", focus) == info(selector)["window_id"]

            def key(selector, *keys):
                window(selector).set_input_focus(X.RevertToParent, X.CurrentTime)
                connection.sync()
                codes = [connection.keysym_to_keycode(XK.string_to_keysym(name)) for name in keys]
                assert all(codes), keys
                for code in codes:
                    xtest.fake_input(connection, X.KeyPress, code)
                for code in reversed(codes):
                    xtest.fake_input(connection, X.KeyRelease, code)
                connection.sync()

            def text(selector, value):
                names = {" ": "space", "-": "minus", "/": "slash", ".": "period"}
                for character in value:
                    key(selector, names.get(character, character))

            def image(selector):
                view = window(selector)
                connection.sync()
                geometry = view.get_geometry()
                pixels = view.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xFFFFFFFF)
                return Image.frombytes("RGB", (geometry.width, geometry.height), pixels.data, "raw", "BGRX")

            def changed(selector, before, label):
                def rendered():
                    picture = image(selector)
                    return picture if ImageChops.difference(before, picture).getbbox() else None
                return wait(rendered, label)

            def capture(selector, name, picture=None):
                args.screenshots.mkdir(parents=True, exist_ok=True)
                path = args.screenshots / (name + ".png")
                if picture is None:
                    picture = image(selector)
                picture.save(path)
                report["screenshots"][name] = {"path": str(path),
                    "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}

            def switcher(selector, query=""):
                before = image(selector)
                key(selector, "Control_L", "Shift_L", "s")
                opened = changed(selector, before, "session switcher opens")
                if query:
                    text(selector, query)
                    changed(selector, opened, "session filter renders")

            def choose(source, target):
                key(source, "Return")
                wait(lambda: info(target)["visible"] and focused(target), target + " receives X focus")

            alpha = start("logs", "local build logs")
            beta = start("editor", "review unicode 中文", ["--tab", alpha["uri"]])
            gamma = start("console", "remote metrics")
            a, b, c = (view["uri"] for view in (alpha, beta, gamma))
            wait(lambda: info(b)["visible"] and not info(a)["visible"], "initial hidden tab")
            assert info(a)["workspace"] == info(b)["workspace"] is not None
            remote_path = root / "remote.input"
            remote_env = {name: value for name, value in env.items() if name != "DISPLAY"}
            remote = register(rpc({"op": "create", "name": "logs", "origin": "mcp/stdio",
                "headless": True, "cols": 100, "rows": 26, "_actor": "switcher-agent",
                "_env": [f"{name}={value}".encode().hex() for name, value in remote_env.items()],
                "command": ["/usr/bin/python3", "-c", CHILD, str(remote_path), "remote build logs"]}), remote_path)
            r = remote["uri"]
            assert remote["input_owner"] == "switcher-agent" and not remote["attached"]
            initial = {uri: info(uri) for uri in (a, b, c, r)}
            report["initial_sessions"] = initial

            switcher(c)
            capture(c, "sessions")
            text(c, "logs build local")
            choose(c, a)
            assert info(a)["visible"] and not info(b)["visible"]
            assert info(a)["workspace"] == initial[a]["workspace"]
            report["checks"].append("name, title and URI words focus a hidden tab in its existing workspace from another window")

            switcher(a, "local")
            key(a, "Down")
            key(a, "Down")
            key(a, "Up")
            choose(a, b)
            assert info(b)["visible"] and not info(a)["visible"]
            switcher(b, "metrics console")
            choose(b, c)
            assert info(c)["workspace"] is None
            for uri in (a, b, c):
                current = info(uri)
                for field in ("id", "pid", "window_id", "workspace", "input_owner", "read_only"):
                    assert current[field] == initial[uri][field], (uri, field, current)
            report["checks"].append("Up/Down selection and cross-window activation preserve every existing PTY, view, workspace and input owner")

            before = image(c)
            key(c, "Control_L", "Shift_L", "p")
            opened = changed(c, before, "command palette opens")
            text(c, "session switch")
            filtered = changed(c, opened, "multiword command filter")
            capture(c, "command-palette")
            key(c, "Return")
            changed(c, filtered, "sessions action opens the switcher")
            text(c, "no-session-matches")
            key(c, "Return")
            # With no match Enter must leave the overlay open. Subsequent query
            # editing must select a view instead of leaking bytes into the PTY.
            key(c, "Control_L", "u")
            text(c, "logs local")
            choose(c, a)
            assert all(input_bytes(uri) == b"" for uri in inputs)
            report["checks"].append("multiword command palette opens sessions; Enter on an empty result stays editable and no palette key reaches a PTY")

            switcher(a, "stdio logs")
            capture(a, "attach-read-only")
            choose(a, r)
            attached = info(r)
            assert attached["attached"] and attached["read_only"] and attached["running"]
            assert attached["pid"] == remote["pid"] and attached["input_owner"] == "switcher-agent"
            assert (attached["cols"], attached["rows"]) == (remote["cols"], remote["rows"])
            assert info(a)["window_id"] == initial[a]["window_id"]
            key(r, "x")
            switcher(r, "editor review")
            choose(r, b)
            rpc({"op": "write", "session": r, "_actor": "switcher-agent", "data": "owner-barrier"})
            wait(lambda: input_bytes(r) == b"owner-barrier", "agent owner input barrier")
            switcher(b, "stdio logs")
            choose(b, r)
            assert info(r)["read_only"] and info(r)["window_id"] == attached["window_id"]
            switcher(r, "editor")
            choose(r, b)
            report["checks"].append("same-name URI filtering explicitly attaches an agent-owned headless PTY on the current display without taking control or resizing it; re-focusing keeps it read-only")

            # Snapshot selection uses session identity and the displayed intent:
            # a Focus entry must not silently turn into Attach after detachment.
            plain_source = image(b)
            switcher(b, "console")
            rpc({"op": "detach", "session": c})
            detached = info(c)
            source_before = info(b)
            before_inputs = {uri: input_bytes(uri) for uri in inputs}
            capture(b, "stale-focus-before-enter")
            report["stale_focus"] = {
                "phase": "target detached while original Focus entry remains open",
                "source": b, "target": c, "source_barrier_hex": "71",
                "target_before": detached, "source_before": source_before,
                "screenshots": ["stale-focus-before-enter"],
            }
            key(b, "Return")
            # The detached broker redirects stderr to /dev/null. Use an actual
            # keyboard/PTY barrier instead: this later ordinary key reaches the
            # source only after Enter has been processed and closed the overlay.
            # An unhandled Enter or still-open query cannot satisfy exact bytes.
            key(b, "q")
            report["stale_focus"]["phase"] = "Enter and following source PTY barrier key sent"

            def rejected_frame():
                received = input_bytes(b)
                report["stale_focus"]["source_input_hex"] = received.hex()
                if received != before_inputs[b] + b"q":
                    return None
                picture = image(b)
                line_height = (picture.height - 40) // source_before["rows"]
                notice_top = picture.height - 20 - line_height
                cleared = (0, picture.height * 2 // 3, picture.width, notice_top)
                restored = not ImageChops.difference(plain_source.crop(cleared), picture.crop(cleared)).getbbox()
                report["stale_focus"]["palette_region_restored"] = restored
                if not restored:
                    return None
                notice = picture.crop((0, notice_top, picture.width, picture.height))
                # The terminal below READY is blank; overlay text has this
                # distinct foreground. Observe the transient notice while it
                # is present, rather than waiting for its expiry or a sleep.
                pixels = sum(count for count, color in notice.getcolors(notice.width * notice.height)
                             if color == (205, 214, 228))
                report["stale_focus"]["notice_pixels"] = pixels
                return (picture, pixels) if pixels > 10 else None

            rejected, notice_pixels = wait(rejected_frame, "stale Focus rejection renders after Enter is processed")
            capture(b, "stale-focus-rejected", rejected)
            after_rejection = info(c)
            assert not after_rejection["attached"] and after_rejection["running"], after_rejection
            for field in ("id", "pid", "window_id", "workspace", "input_owner", "read_only", "cols", "rows"):
                assert after_rejection[field] == detached[field], (field, detached, after_rejection)
            assert after_rejection["pid"] == gamma["pid"] and focused(b)
            source_after = info(b)
            for field in ("id", "pid", "window_id", "workspace", "input_owner", "read_only", "cols", "rows"):
                assert source_after[field] == source_before[field], (field, source_before, source_after)
            for uri, before in before_inputs.items():
                assert input_bytes(uri) == before + (b"q" if uri == b else b""), uri
            report["stale_focus"].update({
                "phase": "Enter processed, source PTY barrier received, transient rejection notice visible",
                "notice_pixels": notice_pixels, "palette_region_restored": True,
                "target_after": after_rejection, "source_after": source_after,
                "screenshots": ["stale-focus-before-enter", "stale-focus-rejected"],
            })
            switcher(b, "stdio logs")
            choose(b, r)
            switcher(r, "console")
            capture(r, "observer-attach")
            choose(r, c)
            observed = info(c)
            assert observed["read_only"] and observed["input_owner"] == "local"
            assert observed["pid"] == gamma["pid"] and observed["window_id"] != gamma["window_id"]
            key(c, "x")
            switcher(c, "editor")
            choose(c, b)
            rpc({"op": "write", "session": c, "data": "local-barrier"})
            wait(lambda: input_bytes(c) == b"local-barrier", "local owner input barrier")
            switcher(b, "console")
            choose(b, c)
            key(c, "Control_L", "Shift_L", "w")
            wait(lambda: not info(c)["attached"], "attached observer view closes")
            assert info(c)["running"] and info(c)["pid"] == gamma["pid"]
            assert info(c)["input_owner"] == "local"
            report["checks"].append("stale Focus cannot attach implicitly; attaching from an observer remains read-only even for a local-owned PTY, and closing that view preserves its process")

            # Copy through the actual X selection protocol, resetting ownership
            # before each request so a previous successful copy cannot pass.
            clipboard_window = connection.screen().root.create_window(
                0, 0, 1, 1, 0, X.CopyFromParent, X.InputOutput, X.CopyFromParent,
                event_mask=X.PropertyChangeMask,
            )
            clipboard = connection.intern_atom("CLIPBOARD")
            utf8 = connection.intern_atom("UTF8_STRING")
            transfer = connection.intern_atom("ZTTY_SWITCHER_CLIPBOARD")

            def copy(expected):
                clipboard_window.set_selection_owner(clipboard, X.CurrentTime)
                connection.sync()
                key(b, "Control_L", "Shift_L", "c")
                wait(lambda: connection.get_selection_owner(clipboard).id != clipboard_window.id,
                     "fresh search clipboard ownership")
                clipboard_window.delete_property(transfer)
                clipboard_window.convert_selection(clipboard, utf8, transfer, X.CurrentTime)
                connection.flush()
                value = wait(lambda: clipboard_window.get_full_property(transfer, utf8), "UTF-8 clipboard transfer")
                actual = bytes(value.value).decode("utf-8")
                assert actual == expected, {"expected": expected, "actual": actual}

            query = "crosswrapneedle"
            cols = info(b)["cols"]
            rpc({"op": "write", "session": b,
                 "data": f"\x1b[2J\x1b[1;{cols - 3}H{query}\x1b[5;1Hsearch-ready"})
            wait(lambda: "search-ready" in screen(b), "soft-wrapped query output")
            before_keys = input_bytes(b)
            key(b, "Control_L", "Shift_L", "f")
            key(b, "Control_L", "u")
            text(b, query)
            copy(query)
            capture(b, "soft-wrap-search")
            rpc({"op": "write", "session": b,
                 "data": "\x1b[2J\x1b[Hreplaced old cells\x1b[4;2H" + query + "\x1b[5;1Hstale-ready"})
            wait(lambda: "stale-ready" in screen(b), "screen edit invalidates old search hit")
            after_output = input_bytes(b)
            assert after_output.startswith(before_keys)
            copy(query)
            key(b, "Control_L", "u")
            text(b, "stale-ready")
            copy("stale-ready")
            key(b, "Escape")
            assert input_bytes(b) == after_output
            report["checks"].append("real search keys copy across soft wrapping; copying after screen replacement refreshes the stale hit, and editing the query never sends PTY input")

            report["final_sessions"] = {uri: info(uri) for uri in inputs}
            for uri, view in report["final_sessions"].items():
                assert view["pid"] == initial[uri]["pid"]
                assert view["input_owner"] == initial[uri]["input_owner"]
                rpc({"op": "close", "session": uri, "_actor": view["input_owner"] or "local"})
            wait(lambda: not rpc({"op": "list"})["sessions"], "owned sessions close")
            for process in owned:
                process.wait(timeout=5)
            report["ok"] = True
        except Exception as error:
            import traceback
            report["error"] = str(error)
            report["traceback"] = traceback.format_exc()
            try:
                report["failure_sessions"] = rpc({"op": "list"})["sessions"]
                for view in report["failure_sessions"]:
                    if view["attached"] and view["visible"]:
                        capture(view["uri"], "failure-" + str(view["id"]))
            except Exception as detail:
                report["failure_diagnostic_error"] = str(detail)
            raise
        finally:
            if connection is not None:
                connection.close()
            eligible = dict(children)
            if broker_pid is not None:
                eligible[broker_pid] = broker_start
            for pid, identity in eligible.items():
                try:
                    if start_ticks(pid) == identity:
                        os.kill(pid, signal.SIGTERM)
                except (FileNotFoundError, ProcessLookupError):
                    pass
            for process in owned:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=3)
            if xvfb is not None:
                xvfb.close()
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                try:
                    reaped, _ = os.waitpid(-1, os.WNOHANG)
                except ChildProcessError:
                    break
                if reaped == 0:
                    time.sleep(0.02)
            log.close()
            report["stderr"] = (root / "stderr.log").read_text(errors="replace")[-12000:]
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
