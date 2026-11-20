#!/usr/bin/env python3
"""Check image-only clipboard rejection and text compatibility on owned Xvfb.

The owner advertises image/png but deliberately answers unadvertised text
conversions with PNG bytes. --expect legacy-leak requires the exact old PTY
bytes for each regression, not a timeout or a crash. Ctrl+V is checked as a
raw application key; this fixture does not claim to test a Codex attachment.
"""

import argparse
import ctypes
import hashlib
import io
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
import traceback
import unicodedata
import zlib

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from isolated_xvfb import start_xvfb


CHILD = r'''import os,sys,tty
tty.setraw(0)
out=open(sys.argv[1],"wb",buffering=0)
os.write(1,b"\x1b[?25l\x1b[?2004h\x1b[2J\x1b[HCLIPBOARD IMAGE GUARD READY\r\n")
while True:
    data=os.read(0,65536)
    if not data: break
    out.write(data)
'''

CONFIG = """font = monospace:size=11
background = #030507
foreground = #ffffff
background-opacity = 100
background-blur = false
cursor-blink = false
window-padding = 16
renderer = software
"""


def identity(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
    except (FileNotFoundError, ProcessLookupError):
        return None


def digest(data):
    return hashlib.sha256(data).hexdigest()


def synthetic_png():
    def chunk(kind, data):
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data)))
    pixels = b"\0" + bytes((23, 67, 109)) * 2
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 2, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(pixels * 2)) + chunk(b"IEND", b""))


def old_paste(data, latin=False):
    # Old host used from_utf8_lossy; zinput removes control characters except
    # tab/newline/carriage return before wrapping bracketed paste.
    text = data.decode("latin1" if latin else "utf8", errors="replace")
    text = "".join(c for c in text if unicodedata.category(c) != "Cc" or c in "\t\r\n")
    return bracket(text.encode())


def bracket(data):
    return b"\x1b[200~" + data + b"\x1b[201~" if data else b""


def main():
    from PIL import Image
    from Xlib import X, XK, Xatom, display, protocol
    from Xlib.ext import xtest

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--expect", choices=("reject", "legacy-leak"), default="reject")
    args = parser.parse_args()
    binary, output = args.binary.resolve(strict=True), args.output.resolve()
    pictures = output.with_suffix("")
    if output.exists() or pictures.exists():
        parser.error("use a new output path and artifact directory")
    output.parent.mkdir(parents=True, exist_ok=True)
    pictures.mkdir()
    png = synthetic_png()
    with Image.open(io.BytesIO(png)) as test_image:
        test_image.load()
        assert test_image.size == (2, 2) and test_image.getpixel((0, 0)) == (23, 67, 109)
    (pictures / "synthetic-clipboard.png").write_bytes(png)
    report = {"ok": False, "checks": [], "cleanup_errors": [],
              "binary": str(binary), "binary_sha256": digest(binary.read_bytes()),
              "test_script_sha256": digest(Path(__file__).read_bytes()),
              "xvfb_helper_sha256": digest((ROOT / "tools/isolated_xvfb.py").read_bytes()),
              "expect": args.expect, "png_sha256": digest(png), "png_bytes": len(png),
              "scope": "Owned private Xvfb, synthetic selection owner, XTEST, native pixels and raw PTY; no real desktop clipboard or Codex process"}
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("PR_SET_CHILD_SUBREAPER failed")
    launcher = connection = xvfb = None
    owned = {}
    with tempfile.TemporaryDirectory(prefix="ztty-clipboard-image-") as temporary:
        directory = Path(temporary)
        logpath = directory / "stderr.log"
        with logpath.open("wb") as log:
            try:
                xvfb = start_xvfb(screen="1280x800x24", stderr=log, timeout=10)
                report["display_ownership"] = xvfb.evidence
                connection = display.Display(xvfb.display)
                env = dict(os.environ, DISPLAY=xvfb.display, XMODIFIERS="@im=none", LC_ALL="C.UTF-8",
                           LIBGL_ALWAYS_SOFTWARE="1", XDG_RUNTIME_DIR=temporary,
                           XDG_CONFIG_HOME=str(directory / "config-home"), XDG_CACHE_HOME=str(directory / "cache"),
                           XDG_STATE_HOME=str(directory / "state"), XDG_DATA_HOME=str(directory / "data"),
                           ZTTY_SOCKET="@ztty-clipboard-image-" + directory.name)
                for key in ("WAYLAND_DISPLAY", "LD_PRELOAD", "ZTTY_SESSION", "ZTTY_X11_DIAGNOSTIC_CONTROL",
                            "ZTTY_EGL_PIXEL_TRACE", "ZTTY_X11_PRESENT_TRACE"):
                    env.pop(key, None)
                config = directory / "appearance.conf"
                config.write_text(CONFIG)
                path = directory / "input.bin"

                def rpc(request):
                    with socket.socket(socket.AF_UNIX) as channel:
                        channel.settimeout(3)
                        channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                        peer, uid, _ = struct.unpack("3i", channel.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
                        assert uid == os.getuid()
                        channel.sendall(json.dumps(request).encode() + b"\n")
                        data = bytearray()
                        while b"\n" not in data:
                            part = channel.recv(8192)
                            assert part and len(data) < 1048576, "invalid private broker reply"
                            data.extend(part)
                    reply = json.loads(data.split(b"\n", 1)[0])
                    assert reply.get("ok") is True, reply
                    return reply["result"], peer

                def wait(predicate, label, timeout=8):
                    deadline = time.monotonic() + timeout
                    while time.monotonic() < deadline:
                        pump()
                        try:
                            value = predicate()
                            if value:
                                return value
                        except (FileNotFoundError, ConnectionRefusedError):
                            pass
                        time.sleep(.003)
                    raise TimeoutError(label)

                # No selection request is possible until the launcher starts.
                pump = lambda: None
                launcher = subprocess.Popen([str(binary), "--name", "image-guard", "--config", str(config),
                    "--geometry", "100x18", "-e", sys.executable, "-c", CHILD, str(path)],
                    env=env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
                owned[launcher.pid] = identity(launcher.pid)
                wait(lambda: rpc({"op": "list"})[0]["sessions"], "private session starts", 15)
                info, broker = rpc({"op": "info", "session": "image-guard"})
                assert broker == info["broker_pid"]
                assert digest(Path(f"/proc/{broker}/exe").read_bytes()) == report["binary_sha256"]
                for pid in (broker, info["pid"]):
                    owned[pid] = identity(pid)
                    assert owned[pid] is not None
                wait(lambda: "CLIPBOARD IMAGE GUARD READY" in rpc({"op": "read", "session": "image-guard"})[0]["screen"],
                     "raw PTY and bracketed paste mode")
                window = connection.create_resource_object("window", info["window_id"])
                window.set_input_focus(X.RevertToParent, X.CurrentTime)
                connection.sync()
                owner = connection.screen().root.create_window(0, 0, 1, 1, 0, X.CopyFromParent,
                    X.InputOutput, X.CopyFromParent, event_mask=X.PropertyChangeMask)
                clipboard = connection.intern_atom("CLIPBOARD")
                targets = connection.intern_atom("TARGETS")
                utf8 = connection.intern_atom("UTF8_STRING")
                image_png = connection.intern_atom("image/png")
                incr = connection.intern_atom("INCR")
                owner.set_selection_owner(clipboard, X.CurrentTime)
                connection.sync()
                assert connection.get_selection_owner(clipboard).id == owner.id
                state = {}

                def pump():
                    while connection.pending_events():
                        event = connection.next_event()
                        if event.type == X.SelectionRequest:
                            assert event.selection == clipboard
                            target, prop = event.target, event.property or event.target
                            state.setdefault("requests", []).append(connection.get_atom_name(target))
                            requestor = event.requestor
                            if target == targets:
                                requestor.change_property(prop, Xatom.ATOM, 32, [targets, *state["advertised"]])
                                if state.get("image_only"):
                                    state["done"] = True
                            elif target not in state["accepted"]:
                                prop = X.NONE
                            else:
                                kind = state.get("kind", target)
                                if state.get("incr"):
                                    requestor.change_attributes(event_mask=X.PropertyChangeMask)
                                    requestor.change_property(prop, incr, 32, [len(state["payload"])])
                                    data = state["payload"]
                                    state["pending"] = iter([data[n:n+2] for n in range(0, len(data), 2)] + [b""])
                                    state["requestor"], state["property"], state["kind"] = requestor, prop, kind
                                else:
                                    requestor.change_property(prop, kind, 8, state["payload"])
                                    state["done"] = True
                            requestor.send_event(protocol.event.SelectionNotify(
                                time=event.time, requestor=requestor, selection=event.selection,
                                target=target, property=prop), event_mask=0)
                            connection.flush()
                        elif (event.type == X.PropertyNotify and event.state == X.PropertyDelete
                              and "pending" in state and event.window.id == state["requestor"].id
                              and event.atom == state["property"]):
                            chunk = next(state["pending"], None)
                            if chunk is None:
                                state.pop("pending")
                                state["done"] = True
                            else:
                                state["requestor"].change_property(state["property"], state["kind"], 8, chunk)
                                state["chunks_sent"] = state.get("chunks_sent", 0) + 1
                            connection.flush()

                def key(*names):
                    codes = [connection.keysym_to_keycode(XK.string_to_keysym(name)) for name in names]
                    assert all(codes), names
                    for code in codes:
                        xtest.fake_input(connection, X.KeyPress, code)
                    for code in reversed(codes):
                        xtest.fake_input(connection, X.KeyRelease, code)
                    connection.sync()

                def picture():
                    geometry = window.get_geometry()
                    pixels = window.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xffffffff)
                    return Image.frombytes("RGB", (geometry.width, geometry.height), pixels.data, "raw", "BGRX")

                def footer():
                    frame = picture()
                    # The hidden cursor and ready text are above this region.
                    return frame.crop((0, frame.height - 48, frame.width, frame.height)).tobytes()

                clean_footer = wait(lambda: (data if data == bytes((3, 5, 7)) * (len(data) // 3) else None)
                                    if (data := footer()) else None,
                                    "complete native background before feedback probe")

                def input_fence(offset):
                    # SelectionNotify/property data and XTEST are issued on
                    # this same X connection in order. Seeing Ctrl+G at the raw
                    # PTY proves the UI passed those events, not just XSync.
                    connection.sync()
                    key("Control_L", "g")
                    wait(lambda: path.read_bytes()[offset:].endswith(b"\x07"), "XTEST-to-PTY input fence")
                    current, peer = rpc({"op": "info", "session": "image-guard"})
                    assert peer == broker == current["broker_pid"]
                    assert current["pid"] == info["pid"] and current["running"]
                    connection.sync()
                    pump()
                    received = path.read_bytes()[offset:]
                    assert received.endswith(b"\x07")
                    return received[:-1]

                state.update(advertised=[image_png], accepted=[utf8, Xatom.STRING], payload=png, image_only=True)
                offset = path.stat().st_size
                key("Control_L", "v")
                wait(lambda: path.read_bytes()[offset:] == b"\x16", "default Ctrl+V raw input")
                actual = input_fence(offset)
                assert actual == b"\x16" and not state.get("requests"), (actual, state)
                report["checks"].append({"name": "Ctrl+V remains raw application input", "ok": True,
                    "actual_hex": actual.hex(), "selection_requests": [],
                    "scope": "Proves key forwarding only; an application must implement image attachment itself"})

                def case(name, spec, expected, guard=False, show_refusal=False):
                    pump()
                    assert "pending" not in state, "previous INCR owner is still active"
                    state.clear()
                    state.update(spec)
                    offset = path.stat().st_size
                    key("Control_L", "Shift_L", "v")
                    wait(lambda: state.get("done"), name + " selection transfer completes")
                    actual = input_fence(offset)
                    row = {"name": name, "ok": False, "guard": guard,
                           "advertised": [connection.get_atom_name(a) for a in state["advertised"]],
                           "requests": list(state.get("requests", [])),
                           "expected_bytes": len(expected), "actual_bytes": len(actual),
                           "expected_sha256": digest(expected), "actual_sha256": digest(actual),
                           "chunks_sent": state.get("chunks_sent", 0)}
                    report["checks"].append(row)
                    if actual != expected:
                        row["actual_prefix_hex"] = actual[:128].hex()
                    assert actual == expected, row
                    assert state.get("requests"), "paste did not request the synthetic clipboard"
                    if args.expect == "reject" and spec.get("image_only"):
                        assert state["requests"] == ["TARGETS"], "image-only clipboard triggered a text conversion"
                    if guard and args.expect == "legacy-leak":
                        assert actual, "legacy mode requires the actual old data leak"
                    if show_refusal and args.expect == "reject":
                        wait(lambda: footer() != clean_footer, "visible image-only refusal feedback")
                        screenshot = pictures / "image-only-refused.png"
                        picture().save(screenshot)
                        row["visible_feedback"] = {"footer_changed": True, "screenshot": str(screenshot),
                            "sha256": digest(screenshot.read_bytes()),
                            "expected_message": "Clipboard has no supported text; use the app's image-paste shortcut"}
                    row["ok"] = True
                    print(json.dumps({"name": name, "ok": True}), flush=True)

                image_spec = {"advertised": [image_png], "accepted": [utf8, Xatom.STRING],
                              "payload": png, "image_only": True}
                case("image-only owner lies about UTF8 conversion", image_spec,
                     old_paste(png) if args.expect == "legacy-leak" else b"", guard=True, show_refusal=True)
                case("image-only owner lies about STRING fallback", {**image_spec, "accepted": [Xatom.STRING]},
                     old_paste(png, latin=True) if args.expect == "legacy-leak" else b"", guard=True)
                invalid = b"valid prefix \xff invalid suffix"
                invalid_spec = {"advertised": [utf8], "accepted": [utf8], "payload": invalid}
                for incremental in (False, True):
                    case("invalid UTF8 is rejected " + ("after complete INCR" if incremental else "directly"),
                         {**invalid_spec, "incr": incremental},
                         old_paste(invalid) if args.expect == "legacy-leak" else b"", guard=True)
                wrong_type = b"wrong actual STRING type"
                case("advertised UTF8 rejects a STRING property", {"advertised": [utf8], "accepted": [utf8],
                     "payload": wrong_type, "kind": Xatom.STRING},
                     bracket(wrong_type) if args.expect == "legacy-leak" else b"", guard=True)
                utf_text = "first\t中文 café 🙂\r\nsecond\n".encode()
                latin_text = b"caf\xe9 \xa3\xff\r\n"
                for name, atom, data, decoded in (("UTF8", utf8, utf_text, utf_text),
                                                  ("STRING", Xatom.STRING, latin_text, latin_text.decode("latin1").encode())):
                    for incremental in (False, True):
                        case(name + (" INCR split across codepoints" if incremental else " direct"),
                             {"advertised": [atom], "accepted": [atom], "payload": data, "incr": incremental},
                             bracket(decoded))
                report["ok"] = len(report["checks"]) == 10 and all(row["ok"] for row in report["checks"])
            except Exception as error:
                report["error"] = str(error)
                report["traceback"] = traceback.format_exc()
            finally:
                if connection is not None:
                    connection.close()
                for pid, token in owned.items():
                    try:
                        if token is not None and identity(pid) == token:
                            os.kill(pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                if launcher is not None:
                    try:
                        launcher.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        launcher.terminate()
                        launcher.wait(timeout=3)
                if xvfb is not None:
                    xvfb.close()
                until = time.monotonic() + 3
                while time.monotonic() < until:
                    try:
                        if os.waitpid(-1, os.WNOHANG)[0] == 0:
                            time.sleep(.01)
                    except ChildProcessError:
                        break
                for pid, token in owned.items():
                    if token is not None and identity(pid) == token:
                        report["cleanup_errors"].append({"pid": pid, "reason": "test process remains"})
                if report["cleanup_errors"]:
                    report["ok"] = False
        report["stderr"] = logpath.read_text(errors="replace")[-4000:]
        output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "checks": len(report["checks"]),
                      "error": report.get("error"), "output": str(output)}))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
