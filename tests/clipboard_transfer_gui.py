#!/usr/bin/env python3
"""Check real X11 clipboard transfers against bytes received by a private PTY.

The synthetic clipboard owner implements direct and ICCCM INCR replies. Tests
exercise encoding, complete-transfer rejection, recovery and the 4 MiB bound.
Only this test's private Xvfb, broker and child processes are cleaned up.
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
os.write(1,b"\x1b[?2004hCLIPBOARD READY\r\n")
while True:
    data=os.read(0,65536)
    if not data: break
    out.write(data)
'''


def identity(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
    except (FileNotFoundError, ProcessLookupError):
        return None


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    from Xlib import X, XK, Xatom, display, protocol
    from Xlib.ext import xtest

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    report = {"ok": False, "checks": [], "cleanup_errors": [],
              "binary": str(binary), "binary_sha256": digest(binary.read_bytes()),
              "test_checkout_files_sha256": {}}
    for name in ("src/x11.rs", "src/main.rs", "src/session.rs", "tests/clipboard_transfer_gui.py"):
        report["test_checkout_files_sha256"][name] = digest((Path(__file__).resolve().parents[1] / name).read_bytes())
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("PR_SET_CHILD_SUBREAPER failed")
    launcher = connection = xvfb = None
    owned = {}
    with tempfile.TemporaryDirectory(prefix="ztty-clipboard-transfer-") as temporary:
        root = Path(temporary)
        log = (root / "stderr.log").open("wb")
        try:
            xvfb = start_xvfb(screen="1000x700x24", stderr=log, timeout=10)
            report["display_ownership"] = xvfb.evidence
            connection = display.Display(xvfb.display)
            env = dict(os.environ, DISPLAY=xvfb.display, XMODIFIERS="@im=none",
                       XDG_RUNTIME_DIR=temporary, XDG_CONFIG_HOME=str(root / "config"),
                       ZTTY_SOCKET="@" + root.name, LC_ALL="C.UTF-8")
            env.pop("WAYLAND_DISPLAY", None)
            path = root / "input.bin"

            def rpc(request):
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                    channel.settimeout(3)
                    channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                    channel.sendall(json.dumps(request).encode() + b"\n")
                    data = bytearray()
                    while b"\n" not in data:
                        piece = channel.recv(8192)
                        if not piece or len(data) > 1048576:
                            raise RuntimeError("invalid private broker reply")
                        data.extend(piece)
                result = json.loads(data.split(b"\n", 1)[0])
                assert result["ok"], result
                return result["result"]

            def wait(predicate, label, seconds=8):
                until = time.monotonic() + seconds
                while time.monotonic() < until:
                    try:
                        result = predicate()
                        if result:
                            return result
                    except (FileNotFoundError, ConnectionRefusedError):
                        pass
                    time.sleep(0.01)
                raise TimeoutError(label)

            launcher = subprocess.Popen(
                [str(binary), "--name", "clipboard", "--geometry", "80x20",
                 "-e", "/usr/bin/python3", "-c", CHILD, str(path)], env=env,
                stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
            wait(lambda: rpc({"op": "list"})["sessions"], "private clipboard session")
            info = rpc({"op": "info", "session": "clipboard"})
            for pid in (info["broker_pid"], info["pid"]):
                owned[pid] = identity(pid)
                assert owned[pid] is not None
            wait(lambda: "CLIPBOARD READY" in rpc({"op": "read", "session": "clipboard"})["screen"],
                 "raw PTY and bracketed paste mode")
            window = connection.create_resource_object("window", info["window_id"])
            owner = connection.screen().root.create_window(
                0, 0, 1, 1, 0, X.CopyFromParent, X.InputOutput, X.CopyFromParent,
                event_mask=X.PropertyChangeMask)
            clipboard = connection.intern_atom("CLIPBOARD")
            utf8 = connection.intern_atom("UTF8_STRING")
            incr = connection.intern_atom("INCR")
            owner.set_selection_owner(clipboard, X.CurrentTime)
            connection.sync()
            state = {}

            def send_property(requestor, prop, kind, form, payload):
                # A direct reply is fully stored before SelectionNotify. Chunk
                # appends keep each X request below the server request limit.
                if form == 8:
                    requestor.change_property(prop, kind, form, payload[:32768], X.PropModeReplace)
                    for offset in range(32768, len(payload), 32768):
                        requestor.change_property(prop, kind, form, payload[offset:offset+32768],
                                                  X.PropModeAppend)
                else:
                    requestor.change_property(prop, kind, form, payload, X.PropModeReplace)

            def pump():
                while connection.pending_events():
                    event = connection.next_event()
                    if event.type == X.SelectionRequest:
                        requestor = event.requestor
                        prop = event.property or event.target
                        reject = state.get("latin") and event.target == utf8
                        if state.get("mode") == "missing":
                            reject = True
                        reply_prop = 0 if reject else prop
                        if not reject:
                            kind = state.get("kind", Xatom.STRING if state.get("latin") else utf8)
                            state["requestor"], state["property"] = requestor, prop
                            if state.get("mode") == "incr":
                                requestor.change_attributes(event_mask=X.PropertyChangeMask)
                                send_property(requestor, prop, incr, state.get("header_format", 32),
                                              state.get("header", [state.get("hint", 0)]))
                                state["pending"] = iter(state["chunks"] + [(kind, 8, b"")])
                            else:
                                send_property(requestor, prop, kind, state.get("format", 8),
                                              state.get("payload", b""))
                                state["done"] = True
                        requestor.send_event(protocol.event.SelectionNotify(
                            time=event.time, requestor=requestor, selection=event.selection,
                            target=event.target, property=reply_prop), event_mask=0)
                        state["requests"] = state.get("requests", 0) + 1
                        state["last_event"] = time.monotonic()
                        connection.flush()
                    elif (event.type == X.PropertyNotify and event.state == X.PropertyDelete
                          and "pending" in state and event.window.id == state["requestor"].id
                          and event.atom == state["property"]):
                        chunk = next(state["pending"], None)
                        if chunk is None:
                            state.pop("pending")
                            state["done"] = True
                        else:
                            send_property(state["requestor"], state["property"], *chunk)
                            state["chunks_sent"] = state.get("chunks_sent", 0) + 1
                        state["last_event"] = time.monotonic()
                        connection.flush()

            def key(*names):
                window.set_input_focus(X.RevertToParent, X.CurrentTime)
                connection.sync()
                keys = [connection.keysym_to_keycode(XK.string_to_keysym(name)) for name in names]
                for code in keys:
                    xtest.fake_input(connection, X.KeyPress, code)
                for code in reversed(keys):
                    xtest.fake_input(connection, X.KeyRelease, code)
                connection.flush()

            def run_case(name, spec, expected):
                pump()
                state.clear()
                state.update(spec)
                offset = path.stat().st_size
                started = time.monotonic()
                state["last_event"] = started
                key("Control_L", "Shift_L", "v")
                while time.monotonic() - started < 12:
                    pump()
                    age = time.monotonic() - state["last_event"]
                    data = path.read_bytes()[offset:]
                    settled = state.get("requests", 0) and age > 0.25
                    if settled and (len(data) >= len(expected) or age > 1.0):
                        break
                    time.sleep(0.003)
                # A real key event after the last clipboard property is an
                # event-loop/PTY barrier. It prevents a delayed empty reply or
                # partial paste from being attributed to the next test case.
                connection.sync()
                key("Control_L", "g")
                barrier_deadline = time.monotonic() + 8
                while time.monotonic() < barrier_deadline:
                    pump()
                    if path.read_bytes()[offset:].endswith(b"\x07"):
                        break
                    time.sleep(0.003)
                received = path.read_bytes()[offset:]
                assert received.endswith(b"\x07"), "PTY input barrier did not arrive"
                actual = received[:-1]
                passed = actual == expected and bool(state.get("requests"))
                result = {"name": name, "ok": passed, "expected_bytes": len(expected),
                          "actual_bytes": len(actual), "expected_sha256": digest(expected),
                          "actual_sha256": digest(actual), "requests": state.get("requests", 0),
                          "chunks_sent": state.get("chunks_sent", 0)}
                if not passed:
                    result["actual_prefix_hex"] = actual[:64].hex()
                report["checks"].append(result)
                print(json.dumps({"name": name, "ok": passed}), flush=True)

            def bracket(data):
                return b"\x1b[200~" + data + b"\x1b[201~" if data else b""

            simple = "first\t中文\r\nsecond\n".encode()
            run_case("direct UTF-8 multiline and tabs", {"payload": simple}, bracket(simple))
            split = "café 中🙂文\n".encode()
            chunks = [(utf8, 8, split[n:n+2]) for n in range(0, len(split), 2)]
            run_case("INCR UTF-8 split inside codepoints", {"mode": "incr", "chunks": chunks}, bracket(split))
            latin = b"caf\xe9 \xa3\xff\r\n"
            decoded = latin.decode("latin1").encode()
            run_case("direct STRING fallback", {"latin": True, "payload": latin}, bracket(decoded))
            chunks = [(Xatom.STRING, 8, latin[n:n+2]) for n in range(0, len(latin), 2)]
            run_case("INCR STRING fallback", {"latin": True, "mode": "incr", "chunks": chunks}, bracket(decoded))
            large = ("line\t中文🙂\r\n".encode() * 80000)
            chunks = [(utf8, 8, large[n:n+32768]) for n in range(0, len(large), 32768)]
            run_case("large INCR complete byte-for-byte", {"mode": "incr", "chunks": chunks,
                                                          "hint": len(large)}, bracket(large))
            run_case("empty clipboard sends no paste brackets", {"payload": b""}, b"")
            run_case("control characters cannot close paste brackets",
                     {"payload": b"A\x1b[201~B\x00\x03\tC\n"}, bracket(b"A[201~B\tC\n"))
            invalid = [
                ("missing text", {"mode": "missing"}),
                ("non-text direct type", {"kind": Xatom.ATOM, "payload": b"not text"}),
                ("wrong direct format", {"format": 32, "payload": [42]}),
                ("malformed INCR header", {"mode": "incr", "header_format": 8, "header": b"bad",
                                           "chunks": [(utf8, 8, b"bad-header-data")]}),
                ("INCR type change rejects prefix", {"mode": "incr", "chunks": [
                    (utf8, 8, b"partial-"), (Xatom.STRING, 8, b"wrong-type")]}),
                ("malformed INCR chunk rejects prefix", {"mode": "incr", "chunks": [
                    (utf8, 8, b"partial-"), (utf8, 32, [42])]}),
                ("oversized direct reply", {"payload": b"x" * (4 * 1024 * 1024 + 1)}),
                ("oversized INCR declaration", {"mode": "incr", "hint": 4 * 1024 * 1024 + 1,
                                               "chunks": [(utf8, 8, b"too-large-declaration")]}),
                ("cumulative INCR limit", {"mode": "incr", "chunks":
                    [(utf8, 8, b"x" * 32768)] * 128 + [(utf8, 8, b"x")]}),
            ]
            for name, spec in invalid:
                run_case(name, spec, b"")
                run_case("recovery after " + name, {"payload": b"recovered"}, bracket(b"recovered"))
            report["ok"] = all(item["ok"] for item in report["checks"])
        except Exception as error:
            report["error"] = str(error)
        finally:
            if connection is not None:
                connection.close()
            for pid, token in owned.items():
                try:
                    if identity(pid) == token:
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
                        time.sleep(0.01)
                except ChildProcessError:
                    break
            for pid, token in owned.items():
                if identity(pid) == token:
                    report["cleanup_errors"].append({"pid": pid, "reason": "test process remains"})
            if report["cleanup_errors"]:
                report["ok"] = False
            log.close()
            report["stderr"] = (root / "stderr.log").read_text(errors="replace")[-4000:]
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "checks": len(report["checks"]),
                      "error": report.get("error"), "output": str(args.output)}))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
