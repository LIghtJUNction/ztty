#!/usr/bin/env python3
"""Use real XTEST, clipboard and private PTYs to validate registered text actions."""
import argparse
import base64
import ctypes
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from isolated_xvfb import start_xvfb
from copy_paste_gui import CHILD, CONFIG, process_token


def main():
    from Xlib import X, XK, Xatom, display, protocol
    from Xlib.ext import xtest
    from PIL import Image

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--example", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, default=Path("artifacts/zplugin/gui"))
    args = parser.parse_args()
    binary, example, fixture = [p.resolve(strict=True) for p in (args.binary, args.example, args.fixture)]
    args.artifacts.mkdir(parents=True, exist_ok=True)
    digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
    report = {"ok": False, "checks": [], "cleanup_errors": [], "screenshots": {},
              "binary_sha256": digest(binary), "example_sha256": digest(example), "fixture_sha256": digest(fixture),
              "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "source_sha256": {f: digest(ROOT / f) for f in (
                  "src/main.rs", "src/session.rs", "src/appearance.rs", "src/plugin_host.rs",
                  "src/text_plugins.rs", "tests/plugin_gui.py", "tests/copy_paste_gui.py")},
              "scope": "Owned Xvfb, explicit private broker, synthetic text, native clipboard and raw PTYs"}
    assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0
    xvfb = connection = None
    owned, launchers = {}, []
    clipboard_value = b"preserved clipboard"

    def record(name, **details):
        report["checks"].append(dict(name=name, ok=True, **details))

    with tempfile.TemporaryDirectory(prefix="ztty-plugin-gui-") as temporary:
        root = Path(temporary)
        log = (root / "stderr.log").open("wb")
        try:
            xvfb = start_xvfb(screen="1600x1000x24", stderr=log)
            report["display_ownership"] = xvfb.evidence
            connection = display.Display(xvfb.display)
            env = dict(os.environ, DISPLAY=xvfb.display, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                XDG_RUNTIME_DIR=temporary, XDG_CONFIG_HOME=str(root / "config-home"),
                ZTTY_SOCKET="@ztty-plugin-" + root.name, ZTTY_PLUGIN_TEST_MARKER=str(root / "marker"))
            env.pop("WAYLAND_DISPLAY", None)
            config = root / "config"
            registrations = f"text-plugin = fault={fixture}\ntext-plugin = dedent=/nonexistent/override-me.so\n"
            config.write_text(CONFIG + registrations)
            clipboard = connection.intern_atom("CLIPBOARD")
            utf8 = connection.intern_atom("UTF8_STRING")
            transfer = connection.intern_atom("ZTTY_PLUGIN_TEST")
            targets = connection.intern_atom("TARGETS")
            owner = connection.screen().root.create_window(0, 0, 1, 1, 0, X.CopyFromParent,
                X.InputOutput, X.CopyFromParent, event_mask=X.PropertyChangeMask)

            def pump():
                while connection.pending_events():
                    event = connection.next_event()
                    if event.type != X.SelectionRequest:
                        continue
                    prop = event.property or event.target
                    if event.target == targets:
                        event.requestor.change_property(prop, Xatom.ATOM, 32, [utf8])
                    elif event.target == utf8:
                        event.requestor.change_property(prop, utf8, 8, clipboard_value)
                    else:
                        prop = X.NONE
                    event.requestor.send_event(protocol.event.SelectionNotify(time=event.time,
                        requestor=event.requestor, selection=event.selection, target=event.target, property=prop), propagate=False)
                    connection.flush()

            def wait(predicate, label, timeout=5):
                deadline = time.monotonic() + timeout
                while time.monotonic() < deadline:
                    pump()
                    try:
                        value = predicate()
                        if value:
                            return value
                    except (FileNotFoundError, ConnectionRefusedError):
                        pass
                    time.sleep(.01)
                raise AssertionError(label)

            def rpc(request):
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                    channel.settimeout(2)
                    channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                    pid, uid, _ = struct.unpack("3i", channel.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
                    assert uid == os.getuid()
                    owned.setdefault(pid, process_token(pid))
                    channel.sendall(json.dumps(request).encode() + b"\n")
                    data = bytearray()
                    while b"\n" not in data:
                        part = channel.recv(8192)
                        assert part
                        data.extend(part)
                        assert len(data) < 1048576
                reply = json.loads(data.split(b"\n", 1)[0])
                assert reply["ok"], reply
                return reply["result"]

            def info(name="plugin-test"):
                return rpc({"op": "info", "session": name})

            def launch(name):
                input_path, request_path = root / (name + ".input"), root / (name + ".request")
                launcher = subprocess.Popen([str(binary), "--name", name, "--config", str(config),
                    "--text-plugin", f"Dedent={example}", "--geometry", "60x16", "-e", "/usr/bin/python3", "-c", CHILD,
                    str(input_path), str(request_path)], env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
                launchers.append(launcher)
                owned[launcher.pid] = process_token(launcher.pid)
                wait(lambda: any(s["name"] == name for s in rpc({"op": "list"})["sessions"]), "test session starts", 15)
                initial = info(name)
                owned[initial["pid"]] = process_token(initial["pid"])
                view = connection.create_resource_object("window", initial["window_id"])
                wait(lambda: input_path.exists(), "raw PTY receiver ready")
                wait(lambda: view.get_attributes().map_state == X.IsViewable, "test window mapped")
                view.set_input_focus(X.RevertToParent, X.CurrentTime)
                connection.sync()
                return view, input_path, request_path

            view, input_path, request_path = launch("plugin-test")
            geometry = view.get_geometry()
            cw, ch = (geometry.width - 32) // 60, (geometry.height - 32) // 16
            sequence = 0

            def scene(text):
                nonlocal sequence
                sequence += 1
                replacement = root / "request.new"
                data = "\x1b[?25l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[2J\x1b[H" + text
                replacement.write_text(json.dumps({"sequence": sequence, "data": base64.b64encode(data.encode()).decode()}))
                replacement.replace(request_path)
                wait(lambda: view.get_wm_name() == f"copy-{sequence}", "test output painted")

            def capture(name):
                connection.sync()
                pixels = view.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xffffffff)
                image = Image.frombytes("RGB", (geometry.width, geometry.height), pixels.data, "raw", "BGRX")
                path = args.artifacts / (name + ".png")
                image.save(path)
                report["screenshots"][name] = {"path": str(path), "sha256": digest(path)}

            def key(*names):
                codes = [connection.keysym_to_keycode(XK.string_to_keysym(name)) for name in names]
                assert all(codes), names
                for code in codes:
                    xtest.fake_input(connection, X.KeyPress, code)
                for code in reversed(codes):
                    xtest.fake_input(connection, X.KeyRelease, code)
                connection.sync()

            def run_plugin(name, capture_menu=False):
                key("Control_L", "Shift_L", "p")
                for char in name:
                    key("minus" if char == "-" else char)
                if capture_menu:
                    time.sleep(.1)
                    capture("registered-actions")
                key("Return")

            def drag(start, end):
                time.sleep(.42)
                view.warp_pointer(16 + start[0] * cw + cw // 2, 16 + start[1] * ch + ch // 2)
                connection.sync()
                xtest.fake_input(connection, X.ButtonPress, 1)
                view.warp_pointer(16 + end[0] * cw + cw // 2, 16 + end[1] * ch + ch // 2)
                connection.sync()
                xtest.fake_input(connection, X.ButtonRelease, 1)
                connection.sync()

            def set_clipboard(text="preserved clipboard"):
                nonlocal clipboard_value
                clipboard_value = text.encode()
                owner.set_selection_owner(clipboard, X.CurrentTime)
                connection.sync()

            def selection(atom=clipboard):
                owner.delete_property(transfer)
                owner.convert_selection(atom, utf8, transfer, X.CurrentTime)
                connection.flush()
                value = wait(lambda: owner.get_full_property(transfer, utf8), "selection conversion")
                return bytes(value.value).decode()

            def received(path=input_path):
                return path.read_bytes() if path.exists() else b""

            def started_worker(previous):
                path = root / "marker"
                def probe():
                    lines = path.read_text().splitlines() if path.exists() else []
                    workers = [int(line.split()[1]) for line in lines if line.startswith("create ")]
                    return workers[-1] if len(workers) > previous else None
                return wait(probe, "native extension started")

            def worker_count():
                path = root / "marker"
                return sum(line.startswith("create ") for line in path.read_text().splitlines()) if path.exists() else 0

            scene("    visible first\r\n      visible second")
            set_clipboard()
            run_plugin("dedent", capture_menu=True)
            wait(lambda: selection().rstrip("\n") == "visible first\n  visible second", "visible text transformed")
            capture("copied-visible-text")
            record("config registration reaches broker; CLI case-normalized override loads the real example; no selection uses visible text")

            scene("    alpha\r\n      世界é")
            drag((0, 0), (10, 1))
            assert selection(Xatom.PRIMARY) == "    alpha\n      世界é"
            run_plugin("dedent")
            wait(lambda: selection() == "alpha\n  世界é", "selected Unicode transformed")
            capture("copied-selection")
            assert received() == b""
            record("explicit selected text preserves wide and combining characters and hard newline; result only enters clipboard")

            wrapped = "    " + "ABCDEF" * 12
            scene(wrapped)
            drag((0, 0), (15, 1))
            assert selection(Xatom.PRIMARY) == wrapped
            run_plugin("dedent")
            wait(lambda: selection() == wrapped[4:], "soft-wrapped text transformed without newline")
            record("soft-wrapped selection is a single logical input line")

            for value in ("empty", "invalid-utf8", "panic", "crash"):
                scene(value)
                drag((0, 0), (len(value) - 1, 0))
                set_clipboard()
                before = worker_count()
                run_plugin("fault")
                pid = started_worker(before)
                wait(lambda: not Path(f"/proc/{pid}").exists(), "failed worker reaped")
                time.sleep(.05)
                assert selection() == "preserved clipboard"
                assert info()["visible"] and received() == b""
                record(value + " leaves clipboard and PTY unchanged and reaps worker")
            capture("extension-error")

            scene("timeout")
            drag((0, 0), (6, 0))
            set_clipboard()
            before = worker_count()
            run_plugin("fault")
            pid = started_worker(before)
            other, other_input, _ = launch("other-view")
            started = time.monotonic()
            key("z")
            wait(lambda: received(other_input) == b"z", "other window remains interactive", .75)
            latency = (time.monotonic() - started) * 1000
            view.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            key("Escape")
            wait(lambda: not Path(f"/proc/{pid}").exists(), "Escape cancels worker")
            assert selection() == "preserved clipboard" and received() == b""
            record("hung plugin does not block another window; Escape cancels without reaching source PTY", other_window_input_ms=latency)

            before = worker_count()
            run_plugin("fault")
            pid = started_worker(before)
            config.write_text(CONFIG)
            wait(lambda: not Path(f"/proc/{pid}").exists(), "registration removal cancels worker")
            assert selection() == "preserved clipboard"
            record("live configuration removal cancels the owning call and preserves clipboard")

            config.write_text(CONFIG + registrations)
            # Explicit reload is a response barrier for the following action.
            rpc({"op": "action", "session": "plugin-test", "action": "reload"})
            scene("timeout")
            drag((0, 0), (6, 0))
            before = worker_count()
            run_plugin("fault")
            pid = started_worker(before)
            rpc({"op": "action", "session": "plugin-test", "action": "close"})
            wait(lambda: not Path(f"/proc/{pid}").exists(), "closed view's worker reaped")
            assert info("other-view")["visible"]
            other.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            key("x")
            wait(lambda: received(other_input) == b"zx", "other window survives owner close")
            record("closing the owning view reaps only its worker; another live PTY remains responsive")
            report["ok"] = True
        except Exception as error:
            import traceback
            report.update(error=str(error), traceback=traceback.format_exc())
            report["received_hex"] = {p.name: p.read_bytes().hex() for p in root.glob("*.input")}
            try:
                capture("failure")
            except Exception:
                pass
        finally:
            if connection is not None:
                connection.close()
            for pid, token in reversed(list(owned.items())):
                try:
                    if process_token(pid) == token:
                        os.kill(pid, signal.SIGTERM)
                except (FileNotFoundError, ProcessLookupError):
                    pass
                except Exception as error:
                    report["cleanup_errors"].append(str(error))
            for launcher in launchers:
                try:
                    launcher.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    launcher.kill()
                    launcher.wait(timeout=3)
            if xvfb is not None:
                xvfb.close()
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                try:
                    pid, _ = os.waitpid(-1, os.WNOHANG)
                    if pid == 0:
                        time.sleep(.01)
                    else:
                        continue
                except ChildProcessError:
                    break
            log.close()
            report["stderr"] = (root / "stderr.log").read_text(errors="replace")[-8000:]
    report["ok"] &= not report["cleanup_errors"]
    path = args.artifacts / "plugin-gui.json"
    path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "checks": len(report["checks"]), "output": str(path)}))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
