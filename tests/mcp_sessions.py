#!/usr/bin/env python3
"""Verify persistent PTYs, cursors and shared namespaces through real MCP calls."""
import argparse
import base64
import hashlib
import json
import os
import pathlib
import select
import signal
import subprocess
import tempfile
import time

from mcp_transport import HttpServer, INIT, READY, line, packet, send

class StdioClient:
    def __init__(self, binary, cwd=None, read_only=False):
        command = [binary, "mcp"] + (["--read-only"] if read_only else [])
        self.proc = subprocess.Popen(command, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=cwd, env=os.environ)
        send(self.proc, INIT)
        assert "result" in line(self.proc)
        send(self.proc, READY)
        self.identifier = 1

    def tool(self, name, arguments=None, expect_error=False):
        self.identifier += 1
        send(self.proc, packet("tools/call", self.identifier,
                              {"name": name, "arguments": arguments or {}}))
        response = line(self.proc)
        assert response["id"] == self.identifier and "result" in response, response
        assert response["result"]["isError"] is expect_error, response
        return response["result"]["structuredContent"]

    def close(self):
        if self.proc.poll() is None:
            self.proc.stdin.close()
            self.proc.wait(timeout=5)
        assert self.proc.returncode == 0, self.proc.stderr.read().decode()
        self.proc.stdout.close()
        self.proc.stderr.close()

def http_tool(server, sid, name, arguments=None, expect_error=False):
    status, _, response = server.request(
        packet("tools/call", 9, {"name": name, "arguments": arguments or {}}), sid)
    assert status == 200 and "result" in response, (status, response)
    assert response["result"]["isError"] is expect_error, response
    return response["result"]["structuredContent"]

def collect(read, uri, needle=None, cursor=0, byte_limit=7, exited=False):
    output = ""
    deadline = time.monotonic() + 6
    last = None
    while time.monotonic() < deadline:
        value = read("session_read", {"session": uri, "cursor": cursor, "max_bytes": byte_limit})
        assert value["next_cursor"] >= cursor
        assert not value["truncated"], value
        raw = base64.b64decode(value["output_base64"], validate=True)
        assert raw.decode("utf-8", "replace") == value["output"], value
        assert value["next_cursor"] == cursor + len(raw), value
        cursor = value["next_cursor"]
        output += value["output"]
        last = value
        if ((needle is None or needle in output)
                and (not exited or (value["output_eof"] and value["exit_code"] is not None))
                and not value["has_more"]):
            return output, cursor, value
        if not value["has_more"]:
            time.sleep(0.015)
    raise AssertionError({"wanted": needle, "output": output, "last": last})


def queued_input_reacquire(client, directory, sessions):
    """A full real PTY input queue must survive an idempotent control request."""
    ready = pathlib.Path(directory) / "queue-ready"
    start = pathlib.Path(directory) / "queue-start"
    result = pathlib.Path(directory) / "queue-result.json"
    child = r'''import hashlib,json,os,pathlib,select,sys,time,tty
tty.setraw(0)
pathlib.Path(sys.argv[1]).touch()
while not pathlib.Path(sys.argv[2]).exists(): time.sleep(.01)
expected=int(pathlib.Path(sys.argv[2]).read_text())
received=0
digest=hashlib.sha256()
deadline=time.monotonic()+5
while received<expected and time.monotonic()<deadline:
    if not select.select([0],[],[],.05)[0]: continue
    data=os.read(0,min(65536,expected-received))
    if not data: break
    received+=len(data)
    digest.update(data)
temporary=pathlib.Path(sys.argv[3]+".tmp")
temporary.write_text(json.dumps({"received":received,"sha256":digest.hexdigest()}))
temporary.replace(sys.argv[3])
time.sleep(30)
'''
    created = client.tool("session_create", {
        "name": "queued-input", "history": 0,
        "command": ["python3", "-c", child, str(ready), str(start), str(result)],
    })
    uri = created["uri"]
    sessions.append(uri)
    deadline = time.monotonic() + 5
    while not ready.exists() and time.monotonic() < deadline:
        time.sleep(.01)
    assert ready.exists(), "queued-input child did not enter raw mode"
    data = "0123456789abcdef" * 4096
    accepted = 0
    digest = hashlib.sha256()
    # Allow the broker's 8 MiB queue plus the kernel PTY buffer, with a fixed
    # 10 MiB sending budget. A real queue-full response is still required before
    # reacquiring control while the child is deliberately not reading.
    send_budget = 10 * 1024 * 1024
    for attempt in range(1, send_budget // len(data) + 1):
        client.identifier += 1
        send(client.proc, packet("tools/call", client.identifier, {
            "name": "session_write", "arguments": {"session": uri, "data": data},
        }))
        response = line(client.proc)
        assert response["id"] == client.identifier and "result" in response, response
        if response["result"]["isError"]:
            assert "session input queue is full" in json.dumps(response), response
            break
        assert response["result"]["structuredContent"]["written"] == len(data)
        accepted += len(data)
        digest.update(data.encode())
    else:
        raise AssertionError({"error": "could not establish bounded PTY input backpressure",
                              "send_budget_bytes": send_budget, "accepted_bytes": accepted})
    assert accepted > 0
    for takeover in [False, True]:
        acquired = client.tool("session_control", {
            "session": uri, "action": "acquire", "takeover": takeover,
        })
        assert acquired["input_owner"] == created["input_owner"], acquired
        assert acquired["pid"] == created["pid"], acquired
    start.write_text(str(accepted))
    deadline = time.monotonic() + 6
    while not result.exists() and time.monotonic() < deadline:
        time.sleep(.01)
    assert result.exists(), "queued-input child did not finish reading"
    observed = json.loads(result.read_text())
    assert observed == {"received": accepted, "sha256": digest.hexdigest()}, observed
    client.tool("session_close", {"session": uri})
    sessions.remove(uri)
    return {"status": "passed", "accepted_bytes": accepted,
            "received_bytes": observed["received"], "sha256": observed["sha256"],
            "send_budget_bytes": send_budget, "write_attempts": attempt,
            "queue_full_before_reacquire": True}


def invalid_prefix_split_utf8(client, directory, sessions):
    """Malformed earlier bytes must not consume a later unfinished codepoint."""
    ready = pathlib.Path(directory) / "invalid-prefix-ready"
    resume = pathlib.Path(directory) / "invalid-prefix-resume"
    child = r'''import os,pathlib,sys,time
os.write(1,b"\xff\xf0\x9f")
pathlib.Path(sys.argv[1]).touch()
while not pathlib.Path(sys.argv[2]).exists(): time.sleep(.01)
os.write(1,b"\x9a\x80END")
'''
    created = client.tool("session_create", {
        "name": "invalid-prefix-utf8", "history": 0,
        "command": ["python3", "-c", child, str(ready), str(resume)],
    })
    uri = created["uri"]
    sessions.append(uri)
    deadline = time.monotonic() + 5
    partial = None
    while time.monotonic() < deadline:
        partial = client.tool("session_read", {"session": uri, "max_bytes": 65536})
        if ready.exists() and (partial["has_more"] or partial["next_cursor"]):
            break
        time.sleep(.01)
    assert partial is not None and partial["running"], partial
    assert partial["output"] == "\ufffd" and partial["next_cursor"] == 1, partial
    assert base64.b64decode(partial["output_base64"], validate=True) == b"\xff"
    assert partial["has_more"] and not partial["truncated"], partial
    pending = client.tool("session_read", {"session": uri, "cursor": 1, "max_bytes": 1})
    assert pending["output"] == "" and pending["next_cursor"] == 1 and pending["has_more"], pending
    resume.touch()
    suffix, cursor, finished = collect(client.tool, uri, cursor=1, byte_limit=1, exited=True)
    assert suffix == "🚀END" and cursor == 8 and finished["exit_code"] == 0, (suffix, cursor, finished)
    client.tool("session_close", {"session": uri})
    sessions.remove(uri)
    return {"status": "passed", "initial_cursor": 1, "final_cursor": cursor,
            "output": partial["output"] + suffix}


def invalid_utf8_page_boundaries(operator, http, sid, directory, sessions):
    """A page boundary must not turn known-invalid bytes into an endless wait."""
    release = pathlib.Path(directory) / "invalid-page-release"
    payload = b"\xe2X\xf0\x9fX" + "中文🚀".encode() + b"\xe2\xc2\xa2END"
    final_suffix = b"\xf0\x9f"
    child = r'''import os,pathlib,sys,time
payload=bytes.fromhex(sys.argv[2])
assert os.write(1,payload)==len(payload)
while not pathlib.Path(sys.argv[1]).exists(): time.sleep(.01)
os.write(1,bytes.fromhex(sys.argv[3]))
'''
    created = operator.tool("session_create", {
        "name": "invalid-page-boundaries", "history": 0,
        "command": ["python3", "-c", child, str(release), payload.hex(), final_suffix.hex()],
    })
    uri = created["uri"]
    sessions.append(uri)
    identity = session_identity(created)

    def snapshot(expected, eof):
        # Confirm the broker has all bytes before asserting immediate progress.
        # The child remains alive behind the release gate throughout open reads.
        deadline = time.monotonic() + 6
        value = None
        while time.monotonic() < deadline:
            value = operator.tool("session_read", {
                "session": uri, "max_bytes": 65536, "expected_session": identity,
            })
            if (value["next_cursor"] == len(expected) and value["output_eof"] is eof
                    and (not eof or value["exit_code"] is not None)):
                break
            time.sleep(.01)
        assert value is not None and value["next_cursor"] == len(expected), value
        assert value["output"] == expected.decode("utf-8", "replace"), value
        assert base64.b64decode(value["output_base64"], validate=True) == expected
        assert not value["truncated"] and not value["has_more"], value
        assert value["output_eof"] is eof and value["running"] is not eof, value
        if eof:
            assert value["exit_code"] == 0, value

    def pages(read, transport, start, maximum, source, eof):
        cursor = start
        cursors = [cursor]
        output = ""
        while cursor < len(source):
            value = read("session_read", {
                "session": uri, "cursor": cursor, "max_bytes": maximum,
                "expected_session": identity,
            })
            next_cursor = value["next_cursor"]
            assert cursor < next_cursor <= min(len(source), cursor + maximum + 3), value
            assert value["output"] == source[cursor:next_cursor].decode("utf-8", "replace"), value
            assert base64.b64decode(value["output_base64"], validate=True) == source[cursor:next_cursor]
            assert value["output_eof"] is eof and value["running"] is not eof, value
            assert not value["truncated"] and value["has_more"] is (next_cursor < len(source)), value
            output += value["output"]
            cursor = next_cursor
            cursors.append(cursor)
        assert output == source[start:].decode("utf-8", "replace"), (transport, output)
        return {"transport": transport, "max_bytes": maximum, "cursors": cursors,
                "output": output, "output_eof": eof}

    def read_http(name, arguments):
        return http_tool(http, sid, name, arguments)

    try:
        snapshot(payload, False)
        # Separate clients resume at different byte offsets; one reader reaching
        # the end must not advance the other's cursor. Both malformed prefixes
        # lie at a page boundary while complete Chinese/emoji bytes stay intact.
        open_reads = [
            pages(operator.tool, "stdio", 0, 1, payload, False),
            pages(read_http, "http", 2, 2, payload, False),
        ]
        release.touch()
        complete = payload + final_suffix
        snapshot(complete, True)
        closed_reads = [
            pages(operator.tool, "stdio", 0, 2, complete, True),
            pages(read_http, "http", 2, 1, complete, True),
        ]
        operator.tool("session_close", {"session": uri})
        sessions.remove(uri)
        return {"status": "passed", "payload_hex": payload.hex(),
                "eof_suffix_hex": final_suffix.hex(), "open_reads": open_reads,
                "closed_reads": closed_reads}
    finally:
        # The enclosing suite closes tracked sessions on every failure path.
        release.touch(exist_ok=True)


def session_identity(value):
    return {key: value[key] for key in ("uri", "id", "created_at", "broker_pid")}


def guarded_output_resume(binary, operator, http, sid, sessions):
    """Old cursors must never silently resume a replacement named PTY."""
    first = None
    second = None
    try:
        old = operator.tool("session_create", {
            "name": "resume-reuse", "history": 0,
            "command": ["/bin/sh", "-c", "printf OLD"],
        })
        uri = old["uri"]
        sessions.append(uri)
        old_output, cursor, state = collect(operator.tool, uri, needle="OLD", exited=True)
        assert old_output == "OLD" and cursor == 3 and state["output_eof"], state
        expected = session_identity(state)
        first = StdioClient(binary, read_only=True)
        second = StdioClient(binary, read_only=True)
        first_page = first.tool("session_read", {
            "session": uri, "cursor": 0, "max_bytes": 1, "expected_session": expected,
        })
        second_page = second.tool("session_read", {
            "session": uri, "cursor": 0, "max_bytes": 2, "expected_session": expected,
        })
        assert first_page["output"] == "O" and first_page["next_cursor"] == 1, first_page
        assert second_page["output"] == "OL" and second_page["next_cursor"] == 2, second_page
        first.close()
        first = StdioClient(binary, read_only=True)
        resumed = first.tool("session_read", {
            "session": uri, "cursor": first_page["next_cursor"], "expected_session": expected,
        })
        other = second.tool("session_read", {
            "session": uri, "cursor": second_page["next_cursor"], "expected_session": expected,
        })
        across_http = http_tool(http, sid, "session_read", {
            "session": uri, "cursor": first_page["next_cursor"], "expected_session": expected,
        })
        assert resumed["output"] == "LD" and other["output"] == "D", (resumed, other)
        assert across_http["output"] == "LD" and across_http["output_eof"], across_http

        mismatches = []
        for key in expected:
            wrong = dict(expected)
            wrong[key] = "ztty://mcp/stdio/different" if key == "uri" else expected[key] + 1
            rejected = first.tool("session_read", {
                "session": uri, "cursor": 0, "expected_session": wrong,
            }, expect_error=True)
            assert rejected == {"error": "session identity changed; start a new read"}, rejected
            mismatches.append(key)

        malformed = []
        for key in expected:
            missing = dict(expected)
            del missing[key]
            malformed.append((f"missing_{key}", missing))
            invalid = dict(expected)
            invalid[key] = None
            malformed.append((f"type_{key}", invalid))
        malformed.extend([
            ("extra_field", {**expected, "cursor": 0}),
            ("not_object", []),
        ])
        for label, guard in malformed:
            first.identifier += 1
            send(first.proc, packet("tools/call", first.identifier, {
                "name": "session_read",
                "arguments": {"session": uri, "cursor": 0, "expected_session": guard},
            }))
            rejected = line(first.proc)
            assert rejected["error"]["code"] == -32602 and "result" not in rejected, (label, rejected)
        first.identifier += 1
        duplicate = json.dumps(expected)[:-1] + ', "id": ' + str(expected["id"]) + "}"
        wire = json.dumps(packet("tools/call", first.identifier, {
            "name": "session_read",
            "arguments": {"session": uri, "expected_session": expected},
        }))
        assert wire.count(json.dumps(expected)) == 1
        first.proc.stdin.write(wire.replace(json.dumps(expected), duplicate).encode() + b"\n")
        first.proc.stdin.flush()
        rejected = line(first.proc)
        assert rejected["error"]["code"] in (-32700, -32602) and "result" not in rejected, rejected

        # Renaming keeps the original URI and PTY identity. CLI actions retain
        # their normal input-ownership checks rather than injecting an actor.
        for args in [
            ["control", uri, "acquire", "--takeover"],
            ["rename", uri, "resume-renamed"],
        ]:
            action = subprocess.run([binary, *args], capture_output=True, text=True, timeout=5)
            assert action.returncode == 0, (args, action.stdout, action.stderr)
        renamed = first.tool("session_read", {
            "session": "resume-renamed", "cursor": 1, "expected_session": expected,
        })
        assert renamed["name"] == "resume-renamed" and renamed["output"] == "LD", renamed
        assert session_identity(renamed) == expected
        operator.tool("session_control", {"session": uri, "action": "acquire", "takeover": True})
        operator.tool("session_close", {"session": uri})
        sessions.remove(uri)
        replacement = operator.tool("session_create", {
            "name": "resume-reuse", "history": 0,
            "command": ["/bin/sh", "-c", "printf REPLACEMENT"],
        })
        sessions.append(replacement["uri"])
        complete, _, replacement_state = collect(
            operator.tool, replacement["uri"], needle="REPLACEMENT", exited=True)
        assert replacement["uri"] == uri and replacement["id"] != old["id"], replacement
        stale = first.tool("session_read", {
            "session": uri, "cursor": cursor, "expected_session": expected,
        }, expect_error=True)
        assert stale == {"error": "session identity changed; start a new read"}, stale
        stale_http = http_tool(http, sid, "session_read", {
            "session": uri, "cursor": cursor, "expected_session": expected,
        }, expect_error=True)
        assert stale_http == stale, stale_http
        fresh = first.tool("session_read", {
            "session": uri, "cursor": 0, "expected_session": session_identity(replacement_state),
        })
        legacy = second.tool("session_read", {"session": uri, "cursor": cursor})
        assert complete == fresh["output"] == "REPLACEMENT" and fresh["output_eof"], fresh
        assert legacy["output"] == "LACEMENT" and not legacy["truncated"], legacy
        operator.tool("session_close", {"session": uri})
        sessions.remove(uri)
        return {
            "status": "passed",
            "old_identity": expected,
            "replacement_identity": session_identity(replacement_state),
            "resumed_stdio": resumed["output"],
            "independent_observer": other["output"],
            "resumed_http": across_http["output"],
            "mismatched_identity_fields_rejected": mismatches,
            "malformed_guards_rejected": len(malformed) + 1,
            "rename_preserved_identity": True,
            "stale_guard_error": stale["error"],
            "legacy_unguarded_output": legacy["output"],
        }
    finally:
        for client in (first, second):
            if client is not None:
                client.close()


def guarded_session_mutations(tool, other, label, sessions):
    """A reused name must not redirect guarded input, resize, control or close."""
    child = r'''import os,sys,tty
tty.setraw(0)
os.write(1,b"READY\n")
for data in sys.stdin.buffer:
    if data == b"size\n":
        size = os.get_terminal_size(0)
        os.write(1,f"{size.columns}x{size.lines}\n".encode())
    else:
        os.write(1,data)
'''
    name = f"mutation-{label}"

    def create():
        value = tool("session_create", {
            "name": name, "command": ["python3", "-u", "-c", child],
            "cols": 83, "rows": 21, "history": 0,
        })
        sessions.append(value["uri"])
        output, cursor, _ = collect(tool, value["uri"], needle="READY\n")
        assert output == "READY\n", output
        return value, cursor

    old, cursor = create()
    uri = old["uri"]
    # Existing clients can still use all four operations without a guard.
    tool("session_write", {"session": uri, "data": "LEGACY\n"})
    output, cursor, _ = collect(tool, uri, needle="LEGACY\n", cursor=cursor)
    assert output == "LEGACY\n", output
    resized = tool("session_resize", {"session": uri, "cols": 87, "rows": 25})
    assert (resized["cols"], resized["rows"]) == (87, 25), resized
    tool("session_write", {"session": uri, "data": "size\n"})
    output, cursor, _ = collect(tool, uri, needle="87x25\n", cursor=cursor)
    assert output == "87x25\n", output
    assert tool("session_control", {"session": uri, "action": "release"})["input_owner"] is None
    acquired = tool("session_control", {"session": uri, "action": "acquire"})
    assert acquired["input_owner"] == old["input_owner"], acquired
    assert tool("session_close", {"session": uri})["closed"]
    sessions.remove(uri)

    fresh, cursor = create()
    expected = session_identity(fresh)
    stale = session_identity(old)
    assert fresh["uri"] == uri and fresh["id"] != old["id"], (old, fresh)
    assert fresh["input_owner"] == old["input_owner"], (old, fresh)

    def unchanged():
        value = tool("session_read", {
            "session": uri, "cursor": cursor, "expected_session": expected,
        })
        assert session_identity(value) == expected and value["running"], value
        assert value["pid"] == fresh["pid"], value
        assert (value["cols"], value["rows"]) == (83, 21), value
        assert value["input_owner"] == fresh["input_owner"], value
        assert value["output"] == "" and value["next_cursor"] == cursor, value

    stale_error = {"error": "session identity changed; refresh session metadata"}
    mutations = [
        ("session_write", {"data": "STALE\n"}),
        ("session_resize", {"cols": 7, "rows": 3}),
        ("session_control", {"action": "release"}),
        ("session_control", {"action": "acquire", "takeover": True}),
        ("session_close", {}),
    ]
    for method, arguments in mutations:
        rejected = tool(method, {
            "session": uri, "expected_session": stale, **arguments,
        }, expect_error=True)
        assert rejected == stale_error, (method, rejected)
        unchanged()

    # A stale explicit takeover from another trusted actor must not steal B.
    rejected = other("session_control", {
        "session": uri, "action": "acquire", "takeover": True,
        "expected_session": stale,
    }, expect_error=True)
    assert rejected == stale_error, rejected
    unchanged()

    # Each identity component matters, also when selecting by a short name.
    for key in expected:
        wrong = dict(expected)
        wrong[key] = "ztty://mcp/stdio/different" if key == "uri" else expected[key] + 1
        rejected = tool("session_write", {
            "session": name, "data": "WRONG\n", "expected_session": wrong,
        }, expect_error=True)
        assert rejected == stale_error, (key, rejected)
    unchanged()

    # Public identity fields do not grant input ownership to another client.
    for method, arguments in mutations[:2] + [("session_close", {})]:
        rejected = other(method, {
            "session": uri, "expected_session": expected, **arguments,
        }, expect_error=True)
        assert "input control is required" in rejected["error"], (method, rejected)
    unchanged()

    tool("session_write", {
        "session": name, "data": "GUARDED\n", "expected_session": expected,
    })
    output, cursor, _ = collect(tool, uri, needle="GUARDED\n", cursor=cursor)
    assert output == "GUARDED\n", output
    resized = tool("session_resize", {
        "session": uri, "cols": 91, "rows": 27, "expected_session": expected,
    })
    assert (resized["cols"], resized["rows"]) == (91, 27), resized
    tool("session_write", {
        "session": uri, "data": "size\n", "expected_session": expected,
    })
    output, _, _ = collect(tool, uri, needle="91x27\n", cursor=cursor)
    assert output == "91x27\n", output
    released = tool("session_control", {
        "session": uri, "action": "release", "expected_session": expected,
    })
    assert released["input_owner"] is None, released
    acquired = tool("session_control", {
        "session": uri, "action": "acquire", "expected_session": expected,
    })
    assert acquired["input_owner"] == fresh["input_owner"], acquired
    closed = tool("session_close", {"session": uri, "expected_session": expected})
    assert closed["closed"] and not closed["running"], closed
    sessions.remove(uri)
    assert all(s["uri"] != uri for s in tool("session_list")["sessions"])
    return {
        "status": "passed", "transport": label, "old_identity": stale,
        "replacement_identity": expected, "stale_mutations_rejected": len(mutations),
        "stale_cross_actor_takeover_rejected": True,
        "mismatched_identity_fields_rejected": list(expected),
        "unguarded_compatibility": True, "guarded_mutations": True,
        "identity_does_not_grant_control": True,
    }


def descendant_output_eof(operator, directory, sessions):
    """A reaped PTY leader is not EOF while its descendant still owns the slave."""
    ready = pathlib.Path(directory) / "descendant-writer"
    release = pathlib.Path(directory) / "descendant-release"
    child = r'''import os,pathlib,signal,sys,time
signal.signal(signal.SIGHUP,signal.SIG_IGN)
read_fd,write_fd=os.pipe()
if os.fork():
    os.close(write_fd)
    assert os.read(read_fd,1)==b"R"
    os.close(read_fd)
    os.write(1,b"PREFIX")
    os._exit(7)
os.close(read_fd)
ready=pathlib.Path(sys.argv[1])
temporary=ready.with_name(ready.name+".tmp")
temporary.write_text(str(os.getpid()))
temporary.replace(ready)
os.write(write_fd,b"R")
os.close(write_fd)
deadline=time.monotonic()+6
while not pathlib.Path(sys.argv[2]).exists():
    if time.monotonic()>deadline: os._exit(9)
    time.sleep(.01)
os.write(1,b"SUFFIX")
os._exit(0)
'''
    created = operator.tool("session_create", {
        "name": "descendant-eof", "history": 0,
        "command": ["python3", "-c", child, str(ready), str(release)],
    })
    uri = created["uri"]
    sessions.append(uri)
    pidfd = None
    try:
        deadline = time.monotonic() + 5
        pending = None
        while time.monotonic() < deadline:
            if ready.exists() and pidfd is None:
                pidfd = os.pidfd_open(int(ready.read_text()))
            pending = operator.tool("session_read", {
                "session": uri, "expected_session": session_identity(created),
            })
            if pending["output"] == "PREFIX" and pending["exit_code"] == 7:
                # The writer can become ready during session_read itself.
                # Bind it before leaving the loop so every assertion failure
                # still waits for this owned process before temporary cleanup.
                if pidfd is None:
                    pidfd = os.pidfd_open(int(ready.read_text()))
                break
            time.sleep(.01)
        assert pending is not None and pending["output"] == "PREFIX", pending
        assert not pending["running"] and pending["exit_code"] == 7, pending
        assert not pending["has_more"] and not pending["output_eof"], pending
        release.touch()
        def read_guarded(name, arguments):
            return operator.tool(name, {**arguments, "expected_session": session_identity(created)})
        suffix, cursor, finished = collect(
            read_guarded, uri, cursor=pending["next_cursor"], byte_limit=2, exited=True)
        assert suffix == "SUFFIX" and cursor == 12, (suffix, cursor, finished)
        assert finished["output_eof"] and not finished["has_more"] and finished["exit_code"] == 7, finished
        operator.tool("session_close", {"session": uri})
        sessions.remove(uri)
        return {
            "status": "passed",
            "after_direct_child_exit": {
                key: pending[key] for key in ("output", "running", "exit_code", "has_more", "output_eof")
            },
            "later_descendant_output": suffix,
            "final_cursor": cursor,
            "final_output_eof": finished["output_eof"],
        }
    finally:
        # Cover failures between the child writing its ready file and the first
        # successful read. Bind before releasing it, while it still owns the PTY.
        if pidfd is None:
            published = time.monotonic() + 1
            while not ready.exists() and time.monotonic() < published:
                time.sleep(.01)
            if ready.exists():
                try:
                    pidfd = os.pidfd_open(int(ready.read_text()))
                except ProcessLookupError:
                    pass
        release.touch(exist_ok=True)
        if pidfd is not None:
            if not select.select([pidfd], [], [], 2)[0]:
                signal.pidfd_send_signal(pidfd, signal.SIGKILL)
                select.select([pidfd], [], [], 2)
            os.close(pidfd)


def main(binary):
    os.environ["ZTTY_SOCKET"] = f"@ztty-mcp-sessions-{os.getpid()}"
    os.environ["ZTTY_MCP_SMOKE"] = f"真实-{os.getpid()}-🛰"
    os.environ.pop("ZTTY_MCP_TOKEN", None)
    stdio = None
    http = None
    replacement = None
    sessions = []
    report = {}
    with tempfile.TemporaryDirectory(prefix="ztty-mcp-cwd-") as directory:
        try:
            stdio = StdioClient(binary, cwd=directory)
            created = stdio.tool("session_create", {"name": "shared", "command": ["/bin/sh"]})
            uri = created["uri"]
            sessions.append(uri)
            assert uri == "ztty://mcp/stdio/shared" and created["running"]
            broker_pid = created["broker_pid"]
            assert created["attached"] is False
            stdio.tool("session_create", {"name": "shared", "command": ["/bin/sh"]}, expect_error=True)
            command = "printf '\\n%s|%s|%s\\n' \"$ZTTY_MCP_SMOKE\" \"$PWD\" \"$((6*7))\"\n"
            stdio.tool("session_write", {"session": uri, "data": command})
            expected = f'{os.environ["ZTTY_MCP_SMOKE"]}|{directory}|42'
            output, cursor, read = collect(stdio.tool, uri, needle=expected)
            assert "\ufffd" not in output, output
            resized = stdio.tool("session_resize", {"session": uri, "cols": 93, "rows": 23})
            assert (resized["cols"], resized["rows"]) == (93, 23)
            stdio.tool("session_write", {"session": uri, "data": "stty size\n"})
            _, cursor, _ = collect(stdio.tool, uri, needle="23 93", cursor=cursor)
            report["stdio_pty"] = "passed"
            report["unicode_cursor_pages"] = "passed"
            report["cwd_environment_resize"] = "passed"
            report["same_owner_preserves_queued_pty_input"] = queued_input_reacquire(
                stdio, directory, sessions)

            second_directory = str(pathlib.Path(directory) / "second creator")
            pathlib.Path(second_directory).mkdir()
            os.environ["ZTTY_MCP_SMOKE"] = f"HTTP-{os.getpid()}"
            argument = "spaces \"quotes\" \\backslash 🧪"
            split = stdio.tool("session_create", {
                "name": "split-utf8", "history": 0,
                "command": ["python3", "-c", "import os,time; os.write(1,b'START'+bytes([240,159])); time.sleep(.08); os.write(1,bytes([154,128])+b'END')"],
            })
            sessions.append(split["uri"])
            split_output, _, _ = collect(stdio.tool, split["uri"], needle="START🚀END", byte_limit=1, exited=True)
            assert chr(0xfffd) not in split_output
            stdio.tool("session_close", {"session": split["uri"]})
            sessions.remove(split["uri"])
            report["split_pty_utf8"] = "passed"
            report["invalid_prefix_split_utf8"] = invalid_prefix_split_utf8(
                stdio, directory, sessions)

            http = HttpServer(binary)
            sid = http.initialize()
            assert http.request(READY, sid)[0] == 202
            other = http_tool(http, sid, "session_create", {
                "name": "shared",
                "command": ["/bin/sh", "-c", "printf 'HTTP_CHILD_DONE\\n%s|%s|%s\\n' \"$ZTTY_MCP_SMOKE\" \"$PWD\" \"$1\"; exit 7", "sh", argument],
                "cwd": second_directory,
            })
            other_uri = other["uri"]
            sessions.append(other_uri)
            assert other_uri == f"ztty://mcp/http/127.0.0.1:{http.port}/shared"
            assert other["broker_pid"] == broker_pid
            stdio.tool("session_read", {"session": "shared"}, expect_error=True)
            listed = stdio.tool("session_list")
            assert {v["uri"] for v in listed["sessions"]} == set(sessions)
            text, _, exited = collect(stdio.tool, other_uri, needle="HTTP_CHILD_DONE", exited=True)
            assert exited["exit_code"] == 7
            assert f'{os.environ["ZTTY_MCP_SMOKE"]}|{second_directory}|{argument}' in text, text
            report["creator_cwd_argv_env_isolation"] = "passed"
            report["http_stdio_shared_broker"] = "passed"
            report["namespaces_and_ambiguity"] = "passed"
            report["exit_code"] = 7

            # Separate MCP transport sessions get distinct trusted identities.
            http_tool(http,sid,"session_write",{"session":uri,"data":"forbidden"},expect_error=True)
            http_tool(http,sid,"session_control",{"session":uri,"action":"acquire"},expect_error=True)
            http_tool(http,sid,"session_control",{"session":uri,"action":"acquire","takeover":True})
            stdio.tool("session_write",{"session":uri,"data":"forbidden"},expect_error=True)
            stdio.tool("session_resize",{"session":uri,"cols":80,"rows":24},expect_error=True)
            stdio.tool("session_close",{"session":uri},expect_error=True)
            http_tool(http,sid,"session_write",{"session":uri,"data":"printf 'TAKEOVER_OK\\n'\n"})
            collect(stdio.tool,uri,needle="TAKEOVER_OK",cursor=cursor)
            report["exclusive_input_owner_and_explicit_takeover"]="passed"

            marked=stdio.tool("session_create",{"name":"command-record","command":["python3","-c", "import os; os.write(1,b'\\x1b]133;A\\x07prompt> \\x1b]133;B\\x07example\\x1b]133;C\\x07\\r\\ncommand result\\r\\n\\x1b]133;D;7\\x07')"]})
            sessions.append(marked["uri"])
            collect(stdio.tool,marked["uri"],needle="command result",exited=True)
            records=http_tool(http,sid,"session_commands",{"session":marked["uri"]})["commands"]
            assert len(records)==1 and records[0]["exit_status"]==7,records
            record=http_tool(http,sid,"session_command_read",{"session":marked["uri"],"command_id":records[0]["id"],"max_bytes":5})
            assert record["complete"] and record["truncated"] and len(record["text"].encode())<=5,record
            record=http_tool(http,sid,"session_command_read",{"session":marked["uri"],"command_id":records[0]["id"]})
            assert "command result" in record["text"] and not record["truncated"],record
            stdio.tool("session_close",{"session":marked["uri"]})
            sessions.remove(marked["uri"])
            report["bounded_command_reads_across_observers"]="passed"
            report["guarded_output_resume"] = guarded_output_resume(
                binary, stdio, http, sid, sessions)
            http_calls = lambda name, arguments=None, expect_error=False: http_tool(
                http, sid, name, arguments, expect_error)
            report["guarded_session_mutations"] = [
                guarded_session_mutations(stdio.tool, http_calls, "stdio", sessions),
                guarded_session_mutations(http_calls, stdio.tool, "http", sessions),
            ]
            report["invalid_utf8_page_boundaries"] = invalid_utf8_page_boundaries(
                stdio, http, sid, directory, sessions)
            report["descendant_output_eof"] = descendant_output_eof(
                stdio, directory, sessions)

            stdio.close()
            stdio = None
            still_running = http_tool(http, sid, "session_read", {"session": uri})
            assert still_running["running"] and still_running["broker_pid"] == broker_pid
            assert http.request(method="DELETE", sid=sid)[0] == 200
            http.close()
            http = None
            listing = subprocess.run([binary, "attach"], capture_output=True, text=True, timeout=5)
            assert listing.returncode == 0 and all(item in listing.stdout for item in sessions), listing
            replacement = StdioClient(binary)
            for item in sessions:
                replacement.tool("session_read", {"session": item})
            report["transport_exit_persistence"] = "passed"
            report["attach_default_list"] = "passed"
            for item in sessions:
                replacement.tool("session_control",{"session":item,"action":"acquire","takeover":True})
                closed = replacement.tool("session_close", {"session": item})
                assert closed["closed"] and not closed["running"] and not closed["attached"]
            sessions.clear()
            assert replacement.tool("session_list")["sessions"] == []
            report["close_releases_sessions"] = "passed"
            return report
        finally:
            if sessions:
                if replacement is None:
                    replacement = StdioClient(binary)
                for item in sessions:
                    try:
                        replacement.tool("session_control",{"session":item,"action":"acquire","takeover":True})
                        replacement.tool("session_close", {"session": item})
                    except Exception:
                        pass
            for client in [stdio, replacement]:
                if client is not None:
                    client.close()
            if http is not None:
                http.close()

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    args = parser.parse_args()
    print(json.dumps(main(os.path.abspath(args.binary)), indent=2, ensure_ascii=False))
