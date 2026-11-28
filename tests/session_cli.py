#!/usr/bin/env python3
"""Windowless session CLI acceptance using owned PTYs and a private Linux socket."""
import argparse
import base64
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import sys
import tempfile
import time

from mcp_sessions import StdioClient


def eventually(check, timeout=5):
    deadline = time.monotonic() + timeout
    while not check():
        assert time.monotonic() < deadline, "timed out waiting for fixture"
        time.sleep(.01)


def exercise(binary, directory):
    def cli(*args, code=0, data=None):
        result = subprocess.run([binary, *args], input=data, capture_output=True, timeout=8)
        assert result.returncode == code, (args, result.returncode, result.stderr, result.stdout[:500])
        return json.loads(result.stdout) if result.stdout else result.stderr.decode()

    def read_bytes(page):
        return base64.b64decode(page["output_base64"], validate=True)

    client = StdioClient(binary)
    sessions = set()

    def create(name, argv):
        info = client.tool("session_create", {"name": name, "command": argv})
        sessions.add(info["uri"])
        return info["uri"]

    def close(uri):
        client.tool("session_control", {"session": uri, "action": "acquire"})
        client.tool("session_close", {"session": uri})
        sessions.remove(uri)

    # The child reports exact input through a separate owned file. PTY echo
    # and command text appearing on the screen cannot satisfy these assertions.
    child = """import os,sys,tty
tty.setraw(0)
with open(sys.argv[1], 'wb', buffering=0) as received:
    os.write(1, b'ready')
    while True:
        data=os.read(0, 4096)
        received.write(data)
        if data.endswith(b'!'): break
"""
    a_file, b_file = directory / "a", directory / "b"
    try:
        a = create("first", [sys.executable, "-c", child, str(a_file)])
        b = create("second", [sys.executable, "-c", child, str(b_file)])
        for uri in (a, b):
            assert read_bytes(cli("wait", uri)) == b"ready"
        owner = cli("info", a)["input_owner"]
        assert "owned" in cli("send", a, "--", "denied", code=1)
        assert cli("read", a)["input_owner"] == owner
        assert cli("exec", a, "--", "false", code=69)["status"] == "unsupported"
        assert cli("wait", a, "--cursor", "5", "--timeout-ms", "25", code=124)["status"] == "timeout"
        assert cli("wait", a, "--until", "eof", "--timeout-ms", "25", code=124)["status"] == "timeout"
        released = client.tool("session_control", {"session": a, "action": "release"})
        pinned = json.dumps(released)
        assert cli("send", a, "--expect", pinned, "--", "abc")["written"] == 3
        assert cli("send", a, "--expect", pinned, "--", "stale", code=1).startswith("ztty: input control epoch")
        assert cli("send", a, "--enter", "--", "xy")["written"] == 3
        # Two writers holding the same snapshot may submit only once in that epoch.
        snapshot = json.dumps(cli("info", a))
        writers = [subprocess.Popen([binary, "send", a, "--expect", snapshot, "--", "W"],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE) for _ in range(2)]
        for writer in writers:
            writer.communicate(timeout=5)
        assert sorted(writer.returncode for writer in writers) == [0, 1]
        large = bytes(range(32)) * 2048
        assert cli("send", a, "--stdin", data=large)["written"] == 65536
        assert cli("send", a, "--stdin", data=b"\0\xff!")["drained"]
        assert cli("wait", a, "--until", "eof")["status"] == "eof"
        assert cli("wait", a, "--cursor", "5", code=3)["status"] == "eof"
        assert a_file.read_bytes() == b"abcxy\rW" + large + b"\0\xff!"
        assert b_file.read_bytes() == b""
        assert cli("info", a)["input_owner"] is None

        # Parked readers must never follow a short name into a new incarnation.
        waiter = subprocess.Popen([binary, "wait", b, "--cursor", "5", "--expect",
                                   json.dumps(cli("info", b))], stdout=subprocess.PIPE)
        close(b)
        b = create("second", ["/bin/cat"])
        assert waiter.wait(timeout=6) == 4
        assert json.load(waiter.stdout)["status"] == "session_changed_or_gone"
        waiter.stdout.close()
        close(a)
        a = create("first", ["/bin/cat"])
        client.tool("session_control", {"session": a, "action": "release"})
        assert cli("send", a, "--expect", pinned, "--", "wrong", code=4)["status"] == "session_changed_or_gone"
        assert read_bytes(cli("read", a)) == b""

        # The ordinary local lifetime retires the session on natural exit.
        # Create that same lifetime on a headless fixture PTY.
        with socket.socket(socket.AF_UNIX) as conn:
            conn.connect("\0" + os.environ["ZTTY_SOCKET"][1:])
            request = {"op": "create", "name": "local-exit", "origin": "local", "headless": True,
                       "command": ["/bin/sh", "-c", "sleep 1; printf final"]}
            conn.sendall(json.dumps(request).encode() + b"\n")
            with conn.makefile("rb") as wire:
                created = json.loads(wire.readline())["result"]
        local_end = cli("wait", created["uri"], "--until", "eof", "--expect", json.dumps(created))
        assert local_end["status"] == "eof" and read_bytes(local_end) == b"final"

        job = cli("exec", "--new", "nonzero", "--", "/bin/sh", "-c", "printf verified; exit 7", code=7)
        assert job["status"] == "completed" and job["exit_code"] == 7
        assert read_bytes(job) == b"verified" and not job["retained"]
        assert cli("read", job["uri"], code=4)["status"] == "session_changed_or_gone"

        # Keep the original job alive after timeout. A colliding creator cannot
        # execute an alternative argv in that session.
        gate = directory / "gate"
        sleeper = "import pathlib,sys,time\np=pathlib.Path(sys.argv[1])\nwhile not p.exists(): time.sleep(.01)\nprint('finished')"
        job = cli("exec", "--new", "slow", "--timeout-ms", "20", "--", sys.executable,
                  "-c", sleeper, str(gate), code=124)
        assert job["status"] == "timeout"
        assert "already exists" in cli("exec", "--new", "slow", "--", "/bin/false", code=1)
        gate.touch()
        end = cli("wait", job["uri"], "--expect", json.dumps(job), "--until", "eof")
        assert read_bytes(end) == b"finished\r\n"
        cli("close", job["uri"])

        job = cli("exec", "--new", "big", "--max-bytes", "11", "--", sys.executable,
                  "-c", "import os; os.write(1,b'A'*300000+b'END')")
        assert job["retained"] and job["truncated"] and job["first_cursor"] > 0
        assert job["status"] == "completed" and read_bytes(job) == b"A"*11
        cursor = job["next_cursor"]
        page = cli("read", job["uri"], "--cursor", str(cursor), "--max-bytes", "15")
        assert read_bytes(page) == b"A"*15 and page["next_cursor"] == cursor+15 and not page["truncated"]
        assert read_bytes(cli("read", job["uri"], "--cursor", "300000")) == b"END"
        cli("close", job["uri"])

        # Disconnect a sender while the raw PTY cannot drain. The broker must
        # release its temporary lease and discard only the unaccepted suffix.
        ready = directory / "blocked"
        blocked = create("blocked", [sys.executable, "-c",
            "import pathlib,sys,time,tty; tty.setraw(0); pathlib.Path(sys.argv[1]).touch(); time.sleep(20)", str(ready)])
        eventually(ready.exists)
        client.tool("session_control", {"session": blocked, "action": "release"})
        with tempfile.TemporaryFile() as data:
            data.write(b"q"*65536)
            data.seek(0)
            sender = subprocess.Popen([binary, "send", blocked, "--stdin"], stdin=data, stdout=subprocess.PIPE)
            try:
                eventually(lambda: cli("info", blocked)["input_owner"] == "cli:send")
            finally:
                sender.kill()
                sender.wait(timeout=5)
                sender.stdout.close()
        eventually(lambda: cli("info", blocked)["input_owner"] is None)
        close(blocked)
        close(a)
        close(b)
        assert not cli("list")["sessions"]
    finally:
        for uri in sessions:
            client.tool("session_control", {"session": uri, "action": "acquire", "takeover": True})
            client.tool("session_close", {"session": uri})
        client.close()

    # Probing with connect() would itself keep the broker from idling out.
    address = os.environ["ZTTY_SOCKET"]
    eventually(lambda: address not in Path("/proc/net/unix").read_text().split(), 7)


def lost_ack(binary):
    """After the peer consumes a complete write, dropping its reply cannot replay input."""
    label = os.environ["ZTTY_SOCKET"] + "-ack"
    info = {"uri": "ztty://local/ack", "id": 1, "created_at": 1, "broker_pid": os.getpid(),
            "control_epoch": 1, "input_owner": None}
    with socket.socket(socket.AF_UNIX) as listener:
        listener.bind("\0" + label[1:])
        listener.listen(4)
        listener.settimeout(5)
        with subprocess.Popen([binary, "send", "ack", "--", "one"], env=dict(os.environ, ZTTY_SOCKET=label),
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE) as proc:
            for operation in ("info", "cli-send"):
                conn, _ = listener.accept()
                with conn, conn.makefile("rb") as wire:
                    request = json.loads(wire.readline())
                    assert request["op"] == operation
                    if operation == "info":
                        conn.sendall(json.dumps({"ok": True, "result": info}).encode() + b"\n")
                    else:
                        assert bytes.fromhex(request["_data_hex"]) == b"one"
                        assert request["expected_session"]["id"] == info["id"]
            out, err = proc.communicate(timeout=5)
            assert proc.returncode == 1 and not out and b"disconnected" in err
            assert not select.select([listener], [], [], 0)[0]


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    binary = parser.parse_args().binary.resolve()
    with tempfile.TemporaryDirectory(prefix="ztty-session-cli-") as tmp:
        # Child PTYs inherit only this synthetic environment.
        env = {"PATH": os.defpath, "HOME": tmp, "XDG_RUNTIME_DIR": tmp,
               "ZTTY_SOCKET": f"@ztty-cli-test-{os.getpid()}-{time.monotonic_ns()}"}
        if "LD_LIBRARY_PATH" in os.environ:
            env["LD_LIBRARY_PATH"] = os.environ["LD_LIBRARY_PATH"]
        os.environ.clear()
        os.environ.update(env)
        exercise(binary, Path(tmp))
        lost_ack(binary)
    print("Headless session CLI PTY acceptance passed")
