#!/usr/bin/env python3
"""Check query-only runtime reload on owned raw PTYs and private X11 windows.

Uses the repository's reload child, Xvfb allocator and identity-checked cleanup.
Two copies of the supplied ELF with distinct ignored suffixes exercise an actual
same-PID exec without replacing an installed binary or contacting a user pool.
--expect-probe-refusal reproduces the old sticky extension guard deliberately.
"""
import argparse
import base64
import contextlib
import ctypes
import hashlib
import importlib.util
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


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--renderer", choices=("software", "gpu"), default="software")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--expect-probe-refusal", action="store_true")
    args = parser.parse_args()
    repo = args.repo.resolve(strict=True)
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    if output.exists():
        parser.error("refusing to overwrite an existing result")
    output.parent.mkdir(parents=True, exist_ok=True)
    pictures = output.with_suffix("")
    if pictures.exists():
        parser.error("refusing to reuse a screenshot directory")
    pictures.mkdir()
    child_script = repo / "tests/reload_runtime.py"
    sys.path.insert(0, str(repo / "tools"))
    spec = importlib.util.spec_from_file_location("reload_fixture", child_script)
    fixture = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fixture)
    from Xlib import X, display
    from PIL import Image
    from gui_renderer import record_renderer

    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("PR_SET_CHILD_SUBREAPER failed")
    report = {
        "ok": False, "binary_sha256": digest(binary),
        "test_script_sha256": digest(__file__),
        "helper_sha256": {str(path.relative_to(repo)): digest(path) for path in (
            child_script, repo / "tools/isolated_xvfb.py", repo / "tools/measure_runtime.py",
            repo / "tools/measure_multiwindow.py", repo / "tools/gui_renderer.py")},
        "scope": "owned PTYs, private Xvfb, same-PID exec and actual restored pixels",
        "renderer_requested": args.renderer, "performance_benchmark": False,
        "expected_probe_refusal": args.expect_probe_refusal,
        "identity_fixture_suffixes": True,
        "checks": [], "reloads": [], "screenshots": {},
    }
    identities, children, channels, streams, launchers = {}, {}, {}, {}, {}
    connection = server = listener = None
    broker = None
    rpc = None

    with tempfile.TemporaryDirectory(prefix="ztty-query-reload-") as temporary:
        directory = Path(temporary)
        log_path = directory / "terminal.log"
        with log_path.open("wb") as log:
            try:
                images = [directory / "query-before-ztty", directory / "query-after-ztty"]
                source = binary.read_bytes()
                for index, path in enumerate(images):
                    path.write_bytes(source + f"\nztty-query-reload-fixture-{index}\n".encode())
                    path.chmod(0o755)
                del source
                report["fixture_images"] = [digest(path) for path in images]
                assert report["fixture_images"][0] != report["fixture_images"][1]
                current_image = 0
                server = fixture.start_xvfb(screen="1800x1100x24", dpi=96, stderr=log)
                connection = display.Display(server.display)
                report["display_ownership"] = server.evidence
                env = dict(os.environ, DISPLAY=server.display, XMODIFIERS="@im=none",
                           LC_ALL="C.UTF-8", LIBGL_ALWAYS_SOFTWARE="1",
                           XDG_RUNTIME_DIR=temporary, XDG_CONFIG_HOME=str(directory / "config"),
                           ZTTY_SOCKET="@" + directory.name,
                           ZTTY_RELOAD_QUERY_TEST=directory.name)
                for name in ("WAYLAND_DISPLAY", "LD_PRELOAD", "ZTTY_SESSION"):
                    env.pop(name, None)
                config = directory / "appearance.conf"
                config.write_text("font = monospace:size=11\nbackground = #000000\n"
                                  "background-opacity = 100\nbackground-blur = false\n"
                                  "cursor-blink = false\nwindow-padding = 0\n"
                                  f"renderer = {args.renderer}\n")
                control = directory / "children.sock"
                listener = socket.socket(socket.AF_UNIX)
                listener.bind(str(control))
                listener.listen(4)
                listener.settimeout(15)

                def rpc(request):
                    with socket.socket(socket.AF_UNIX) as channel:
                        channel.settimeout(15)
                        channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                        fixture.send(channel, request)
                        with channel.makefile("rb") as stream:
                            reply = fixture.receive(stream)
                    assert reply.get("ok") is True, reply
                    return reply["result"]

                def info(name):
                    return rpc({"op": "info", "session": name})

                def command(name, op, **fields):
                    fixture.send(channels[name], {"op": op, **fields})
                    value = fixture.receive(streams[name])
                    assert value.get("ok") is True, value
                    return value

                def screen(name):
                    page = rpc({"op": "read", "session": name, "cursor": 0, "max_bytes": 65536})
                    assert not page["truncated"]
                    return page["screen"]

                def barrier(name, payload=b""):
                    before = bytes.fromhex(command(name, "status")["input_hex"])
                    data = payload + b"\x1b[6n"
                    command(name, "emit", base64=base64.b64encode(data).decode())

                    def completed():
                        result = command(name, "status")
                        received = bytes.fromhex(result["input_hex"])
                        assert received.startswith(before)
                        tail = received[len(before):]
                        return tail if re.search(rb"\x1b\[[0-9]+;[0-9]+R$", tail) else None

                    return fixture.wait(completed, name + " actual DSR barrier")

                def start(name):
                    nonlocal broker
                    process = subprocess.Popen(
                        [str(images[current_image]), "--name", name, "--config", str(config),
                         "--geometry", "80x20", "--scrollback", "500", "-e", sys.executable,
                         str(child_script), "--child", str(control), name],
                        env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
                    launchers[name] = process
                    identities[process.pid] = fixture.process_stat(process.pid)["start_ticks"]
                    channel, _ = listener.accept()
                    channel.settimeout(15)
                    stream = channel.makefile("rb")
                    hello = fixture.receive(stream)
                    assert hello["name"] == name
                    channels[name], streams[name], children[name] = channel, stream, hello
                    identities[hello["pid"]] = hello["start_ticks"]
                    value = fixture.wait(lambda: next((session for session in rpc({"op": "list"})["sessions"]
                                                       if session["name"] == name), None), name + " session")
                    if broker is None:
                        broker = value["broker_pid"]
                        identities[broker] = fixture.process_stat(broker)["start_ticks"]
                    assert broker == value["broker_pid"] == hello["parent_pid"]
                    assert value["pid"] == hello["pid"]
                    assert digest(f"/proc/{broker}/exe") == digest(images[current_image])
                    barrier(name)
                    view = connection.create_resource_object("window", value["window_id"])
                    view.configure(x=10 if name == anchor else 950, y=10)
                    connection.sync()

                def identity(name):
                    state = command(name, "status")
                    value = info(name)
                    pid = state["pid"]
                    assert value["pid"] == pid and value["broker_pid"] == broker == state["parent_pid"]
                    assert fixture.process_stat(pid)["start_ticks"] == state["start_ticks"]
                    assert os.stat(f"/proc/{pid}/fd/0").st_rdev == state["tty_rdev"]
                    return {key: state[key] for key in ("pid", "parent_pid", "start_ticks", "nonce",
                                                       "tty", "tty_rdev", "cols", "rows")}

                def verify_identities(expected):
                    assert fixture.same_process(broker, identities[broker])
                    sessions = rpc({"op": "list"})["sessions"]
                    assert {session["name"] for session in sessions} == set(children)
                    assert all(session["broker_pid"] == broker for session in sessions)
                    for name, previous in expected.items():
                        assert identity(name) == previous, name
                        assert launchers[name].poll() is None, name

                def picture(name):
                    view = connection.create_resource_object("window", info(name)["window_id"])
                    connection.sync()
                    assert view.get_attributes().map_state == X.IsViewable
                    geometry = view.get_geometry()
                    data = view.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xffffffff)
                    return Image.frombytes("RGB", (geometry.width, geometry.height), data.data, "raw", "BGRX")

                def settled_picture(name):
                    previous, repeats = None, 0

                    def stable():
                        nonlocal previous, repeats
                        image = picture(name)
                        key = (image.size, image.tobytes())
                        repeats = repeats + 1 if key == previous else 1
                        previous = key
                        if repeats >= 3:
                            return image
                        time.sleep(0.03)
                        return None

                    return fixture.wait(stable, name + " stable framebuffer")

                def save_picture(name, label, image=None):
                    image = image if image is not None else picture(name)
                    path = pictures / (label + ".png")
                    image.save(path)
                    report["screenshots"][label] = {"path": str(path), "sha256": digest(path),
                                                     "width": image.width, "height": image.height}

                def reload(label, succeeds=True, error_contains=None):
                    nonlocal current_image
                    expected = {name: identity(name) for name in children}
                    screens = {name: screen(name) for name in children}
                    frames = {name: settled_picture(name) for name in children}
                    old_sha = digest(f"/proc/{broker}/exe")
                    target = 1 - current_image
                    result = subprocess.run([str(images[0]), "reload", "--binary", str(images[target])],
                                            env=env, capture_output=True, text=True, timeout=30)
                    receipt = {"case": label, "expected_success": succeeds,
                               "exit_code": result.returncode, "stdout": result.stdout,
                               "stderr": result.stderr, "before_image_sha256": old_sha,
                               "target_sha256": digest(images[target]), "broker_pid": broker}
                    report["reloads"].append(receipt)
                    if succeeds:
                        assert result.returncode == 0, receipt
                        answer = json.loads(result.stdout)
                        if answer.get("ok") is True and "result" in answer:
                            answer = answer["result"]
                        assert answer["reloaded"] is True and answer["broker_pid"] == broker
                        assert answer["sessions"] == len(children)
                        current_image = target
                    else:
                        assert result.returncode == 1 and not result.stdout.strip(), receipt
                        assert result.stderr.startswith("ztty: "), receipt
                        assert error_contains in result.stderr, receipt
                    assert digest(f"/proc/{broker}/exe") == digest(images[current_image])
                    verify_identities(expected)
                    for name in children:
                        assert screen(name) == screens[name], (label, name)
                        frame = frames[name]

                        def restored_pixels():
                            image = picture(name)
                            return image.size == frame.size and image.tobytes() == frame.tobytes()

                        fixture.wait(restored_pixels, label + " " + name + " actual restored pixels")
                    marker = "INPUT-AFTER-" + label
                    old_input = bytes.fromhex(command(anchor, "status")["input_hex"])
                    rpc({"op": "write", "session": anchor, "data": marker})
                    fixture.wait(lambda: bytes.fromhex(command(anchor, "status")["input_hex"]) == old_input + marker.encode(),
                                 label + " retained PTY input")
                    barrier(anchor)
                    receipt.update({"same_broker_pid": True, "same_child_and_pty_identities": True,
                                    "same_screen": True, "same_actual_pixels": True,
                                    "input_and_output_after_attempt": True,
                                    "after_image_sha256": digest(f"/proc/{broker}/exe")})

                def close_owned(name):
                    command(name, "quit")
                    # The child exit may already have closed its window/session.
                    # A concurrent explicit close is benign only if absence is
                    # then observed on this fixture's private broker.
                    with contextlib.suppress(AssertionError):
                        rpc({"op": "close", "session": name})
                    fixture.wait(lambda: name not in {session["name"] for session in rpc({"op": "list"})["sessions"]},
                                 name + " owned session closed")
                    streams.pop(name).close()
                    channels.pop(name).close()
                    children.pop(name)
                    launchers.pop(name).wait(timeout=5)

                anchor = "reload-query-probes"
                start(anchor)
                barrier(anchor, b"\x1b[?25l\x1b[48;2;15;28;39m\x1b[2J\x1b[HQUERY-ONLY-SHELL\r\n")
                fixture.wait(lambda: any(count > 1000 and color == (15, 28, 39)
                                         for count, color in (picture(anchor).getcolors(10000) or [])),
                             "initial real framebuffer")
                report["broker_pid"] = broker
                report["broker_start_ticks"] = identities[broker]
                report["initial_identity"] = identity(anchor)
                view = connection.create_resource_object("window", info(anchor)["window_id"])
                report["renderer"] = record_renderer(connection, view, args.renderer)
                save_picture(anchor, "before-queries")
                reload("plain-control")
                response = barrier(anchor, b"\x1b_Ga=q,i=31,f=32,s=1,v=1;AAAA/w==\x1b\\")
                assert b"\x1b_Gi=31;OK\x1b\\" in response
                if args.expect_probe_refusal:
                    reload("old-query-guard", False,
                           "terminal graphics or native UI protocol state cannot be restored yet")
                    report["checks"].append("old sticky guard reproduced after a successful Kitty query; original PTY remains live")
                else:
                    reload("kitty-image-query")
                    response = barrier(anchor, b"\x1b_Ga=q,i=32,e=T,f=32,U=1\x1b\\")
                    assert b"\x1b_Gi=32,e=T" in response and b";OK\x1b\\" in response
                    reload("kitty-feature-query")
                    response = barrier(anchor, b'\x1b_ZUI;1;{"v":1,"op":"capabilities"}\x1b\\')
                    assert b'"type":"capabilities"' in response
                    assert b'"supported":true' in response and b'"last_rev":0' in response
                    reload("native-ui-capabilities")
                    report["checks"].append("Kitty image/feature probes and native UI capabilities each allow actual same-PID exec")

                    response = barrier(anchor, b"\x1b_Ga=q,i=34,f=32,s=1,v=1,m=1;\x1b\\")
                    assert b"\x1b_Gi=34;" not in response
                    reload("zero-byte-pending-query", False, "pending graphics")
                    response = barrier(anchor, b"\x1b_Ga=q,i=35,e=T\x1b\\")
                    assert b"\x1b_Gi=35,e=T;OK\x1b\\" in response
                    reload("feature-probe-with-pending-query", False, "pending graphics")
                    response = barrier(anchor, b"\x1b_Gm=0;AAAA/w==\x1b\\")
                    assert b"\x1b_Gi=34;OK\x1b\\" in response
                    reload("completed-multipart-query")
                    report["checks"].append("zero-byte pending transfer refuses safely and its original ID completes before a successful retry")

                    resource = "reload-query-graphics"
                    start(resource)
                    pixels = base64.b64encode(bytes([0, 255, 0, 255]) * 64)
                    response = barrier(resource, b"\x1b_Ga=T,i=45,f=32,s=8,v=8,C=1;" + pixels + b"\x1b\\")
                    assert b"\x1b_Gi=45;OK\x1b\\" in response
                    fixture.wait(lambda: any(count >= 64 and color == (0, 255, 0)
                                             for count, color in (picture(resource).getcolors(10000) or [])),
                                 "actual live image pixels")
                    save_picture(resource, "live-graphics-before-refusal")
                    reload("live-graphics", False, "graphics resources")
                    barrier(resource, b"\x1b_Ga=d,d=A\x1b\\")
                    reload("deleted-graphics-identity", False, "non-query graphics")
                    close_owned(resource)
                    report["checks"].append("live image resources and cleared non-query identity still refuse, without touching other owned sessions")

                    resource = "reload-query-native-ui"
                    start(resource)
                    capabilities = b'\x1b_ZUI;1;{"v":1,"op":"capabilities"}\x1b\\'
                    response = barrier(resource, b'\x1b_ZUI;1;{"v":1,"op":"replace","surface":9,"rev":1,"root":1,"nodes":[{"id":1,"type":"text","text":"OWNED NATIVE UI"}]}\x1b\\' + capabilities)
                    assert b'"last_rev":1' in response
                    reload("live-native-ui", False, "native UI resources")
                    response = barrier(resource, b'\x1b_ZUI;1;{"v":1,"op":"clear","surface":9,"rev":2,"base_rev":1}\x1b\\' + capabilities)
                    assert b'"last_rev":2' in response
                    reload("cleared-native-ui-identity", False, "native UI protocol identity")
                    assert b'"last_rev":2' in barrier(resource, capabilities)
                    close_owned(resource)
                    reload("remaining-query-only-session")
                    report["checks"].append("native scene clear retains its guarded revision identity; remaining query-only session continues to reload")
                save_picture(anchor, "after-query-checks")
                report["final_identity"] = identity(anchor)
                report["final_image_sha256"] = digest(f"/proc/{broker}/exe")
                report["ok"] = True
            except Exception as error:
                report["error"] = repr(error)
                report["traceback"] = traceback.format_exc()
            finally:
                for name, channel in channels.items():
                    with contextlib.suppress(Exception):
                        fixture.send(channel, {"op": "quit"})
                        fixture.receive(streams[name])
                    with contextlib.suppress(Exception):
                        streams[name].close()
                    channel.close()
                if rpc is not None:
                    for name in children:
                        with contextlib.suppress(Exception):
                            rpc({"op": "close", "session": name})
                for process in launchers.values():
                    with contextlib.suppress(subprocess.TimeoutExpired):
                        process.wait(timeout=2)
                if connection is not None:
                    connection.close()
                if listener is not None:
                    listener.close()
                marker = ("ZTTY_RELOAD_QUERY_TEST=" + directory.name).encode()
                for entry in Path("/proc").iterdir():
                    if entry.name.isdigit():
                        with contextlib.suppress(OSError, KeyError, ValueError):
                            if marker in (entry / "environ").read_bytes().split(b"\0"):
                                pid = int(entry.name)
                                identities.setdefault(pid, fixture.process_stat(pid)["start_ticks"])
                if server is not None:
                    identities.pop(server.process.pid, None)
                report["remaining_owned_pids"] = fixture.cleanup(identities)
                if server is not None:
                    server.close()
                    report["xserver_exit_code"] = server.process.returncode
                if report["remaining_owned_pids"]:
                    report["ok"] = False
                log.flush()
                report["terminal_log"] = log_path.read_text(errors="replace")
                output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "checks": len(report["checks"]),
                      "reloads": len(report["reloads"]), "output": str(output),
                      "error": report.get("error")}))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
