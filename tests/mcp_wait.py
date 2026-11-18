#!/usr/bin/env python3
"""Exercise bounded MCP output waits with real PTYs and concurrent transports.

Every process and terminal belongs to a fresh abstract broker socket. No existing
desktop or broker is inspected or stopped. Run with --output to retain timings
and the exact binary hash, including completed checks on a later failure.
"""
import argparse
import hashlib
import http.client
import json
import os
import pathlib
import select
import signal
import socket
import subprocess
import tempfile
import time

from mcp_transport import HEADERS, HttpServer, INIT, READY, packet


STDIO_WAIT_LIMIT = 24
HTTP_WAIT_LIMIT = 32
RESPONSIVE_SECONDS = 2.0
RESPONSES_CANCEL_TIMEOUT = 2.0


def tool_packet(identifier, name, arguments=None):
    return packet("tools/call", identifier,
                  {"name": name, "arguments": arguments or {}})


def cancelled(identifier):
    return {"jsonrpc": "2.0", "method": "notifications/cancelled",
            "params": {"requestId": identifier, "reason": "owned regression"}}


def tool_result(response, failed=False):
    assert "result" in response, response
    assert response["result"]["isError"] is failed, response
    return response["result"]["structuredContent"]


def is_error(response):
    return "error" in response or response.get("result", {}).get("isError") is True


def wait_file(path, timeout=5):
    deadline = time.monotonic() + timeout
    while not path.exists():
        assert time.monotonic() < deadline, f"child handshake missing: {path.name}"
        time.sleep(.01)


class Stdio:
    """Read OS bytes directly: select cannot see Python's buffered next line."""
    def __init__(self, binary, read_only=False, initialize=True):
        self.proc = subprocess.Popen(
            [binary, "mcp", *(["--read-only"] if read_only else [])],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, bufsize=0, env=os.environ)
        self.buffer = bytearray()
        self.responses = {}
        self.identifier = 100
        if initialize:
            self.send(INIT)
            assert "result" in self.receive(1)
            self.send(READY)

    def next_id(self):
        self.identifier += 1
        return self.identifier

    def send(self, value):
        data = json.dumps(value, ensure_ascii=False).encode() + b"\n"
        while data:
            written = os.write(self.proc.stdin.fileno(), data)
            assert written > 0
            data = data[written:]

    def ingest(self, timeout):
        if b"\n" not in self.buffer:
            if not select.select([self.proc.stdout], [], [], max(0, timeout))[0]:
                return False
            data = os.read(self.proc.stdout.fileno(), 65536)
            assert data, f"MCP stdio closed unexpectedly ({self.proc.poll()})"
            self.buffer.extend(data)
        while b"\n" in self.buffer:
            line, _, self.buffer = self.buffer.partition(b"\n")
            response = json.loads(line)
            assert "id" in response, response
            identifier = response["id"]
            assert identifier not in self.responses, ("duplicate response", response)
            self.responses[identifier] = response
        return True

    def receive(self, identifier, timeout=5):
        deadline = time.monotonic() + timeout
        while identifier not in self.responses:
            remaining = deadline - time.monotonic()
            assert remaining > 0, ("response timed out", identifier, self.responses)
            self.ingest(remaining)
        return self.responses.pop(identifier)

    def assert_missing(self, identifiers, duration=.15):
        identifiers = set(identifiers)
        deadline = time.monotonic() + duration
        while True:
            assert not identifiers.intersection(self.responses), self.responses
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return
            self.ingest(remaining)

    def begin(self, name, arguments=None):
        identifier = self.next_id()
        self.send(tool_packet(identifier, name, arguments))
        return identifier

    def tool(self, name, arguments=None, failed=False, timeout=5):
        return tool_result(self.receive(self.begin(name, arguments), timeout), failed)

    def ping(self, timeout=RESPONSIVE_SECONDS):
        identifier = self.next_id()
        self.send(packet("ping", identifier))
        assert self.receive(identifier, timeout) == {
            "jsonrpc": "2.0", "id": identifier, "result": {}}

    def close(self):
        if self.proc.poll() is None:
            if not self.proc.stdin.closed:
                self.proc.stdin.close()
            try:
                self.proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.proc.terminate()
                self.proc.wait(timeout=3)
        for stream in [self.proc.stdin, self.proc.stdout, self.proc.stderr]:
            stream.close()


class HttpRead:
    """An outstanding request whose response is intentionally not read yet."""
    def __init__(self, server, sid, value, authorization=None, timeout=40):
        self.connection = http.client.HTTPConnection(server.host, server.port, timeout=timeout)
        headers = {**HEADERS, "Mcp-Session-Id": sid,
                   "Mcp-Protocol-Version": "2025-11-25"}
        if server.token:
            headers["Authorization"] = "Bearer " + server.token
        if authorization is not None:
            headers["Authorization"] = authorization
        self.connection.request("POST", "/mcp", json.dumps(value).encode(), headers)

    def assert_pending(self, timeout=.15):
        assert not select.select([self.connection.sock], [], [], timeout)[0], \
            "HTTP wait replied or disconnected before its wake condition"

    def receive(self):
        response = self.connection.getresponse()
        payload = response.read()
        return response.status, json.loads(payload) if payload else None

    def assert_cancelled(self):
        sock = self.connection.sock
        assert select.select([sock], [], [], RESPONSES_CANCEL_TIMEOUT)[0], \
            "cancelled HTTP request retained its connection"
        payload = sock.recv(65536)
        assert not payload, ("cancelled request produced a response", payload)

    def close(self):
        self.connection.close()


ECHO_CHILD = r'''import os,pathlib,sys,tty
tty.setraw(0)
pathlib.Path(sys.argv[1]).touch()
while True:
    data=os.read(0,65536)
    if not data: break
    os.write(1,data)
'''


class Harness:
    def __init__(self, binary, directory):
        self.binary = binary
        self.directory = pathlib.Path(directory)
        self.operator = Stdio(binary)
        self.sessions = set()
        self.broker_pid = None
        self.broker_pidfd = None
        self.sequence = 0
        self.http = None
        self.pending_http = []

    def create(self, code=ECHO_CHILD, arguments=(), name=None):
        self.sequence += 1
        name = name or f"wait-{self.sequence}"
        ready = self.directory / f"ready-{self.sequence}"
        created = self.operator.tool("session_create", {
            "name": name, "history": 0,
            "command": ["python3", "-u", "-c", code, str(ready), *map(str, arguments)],
        })
        self.sessions.add(created["uri"])
        if self.broker_pid is None:
            self.broker_pid = created["broker_pid"]
            self.broker_pidfd = os.pidfd_open(self.broker_pid)
        assert self.broker_pid == created["broker_pid"], "owned broker restarted unexpectedly"
        wait_file(ready)
        return created, ready

    def close_session(self, uri):
        self.operator.tool("session_control", {
            "session": uri, "action": "acquire", "takeover": True})
        self.operator.tool("session_close", {"session": uri})
        self.sessions.discard(uri)

    def start_http(self):
        if self.http is None:
            self.http = HttpServer(self.binary, token="owned-wait-operator",
                                   observer_token="owned-wait-observer")
        sid = self.http.initialize()
        assert self.http.request(READY, sid)[0] == 202
        return self.http, sid

    def http_wait(self, server, sid, uri, identifier, cursor=0, wait_ms=30000,
                  authorization=None):
        request = HttpRead(server, sid, tool_packet(identifier, "session_read", {
            "session": uri, "cursor": cursor, "wait_ms": wait_ms,
        }), authorization=authorization)
        self.pending_http.append(request)
        return request

    def shutdown(self):
        for request in self.pending_http:
            request.close()
        failures = []
        for uri in list(self.sessions):
            try:
                self.close_session(uri)
            except Exception as error:
                failures.append(f"{uri}: {error}")
        self.operator.close()
        if self.http is not None:
            self.http.close()
        if self.broker_pidfd is not None:
            # Bind the lifetime when the first terminal is created; cleanup
            # cannot accidentally signal a later process reusing the PID.
            try:
                signal.pidfd_send_signal(self.broker_pidfd, signal.SIGTERM)
            except ProcessLookupError:
                pass
            assert select.select([self.broker_pidfd], [], [], 3)[0], "owned broker did not exit"
            os.close(self.broker_pidfd)
        assert not failures, {"session_cleanup_failed": failures}


def stdio_concurrency(h):
    created, _ = h.create()
    uri = created["uri"]
    identifier = h.operator.begin("session_read", {
        "session": uri, "cursor": 0, "wait_ms": 30000})
    begin = time.monotonic()
    h.operator.ping()
    listing = h.operator.tool("session_list", timeout=RESPONSIVE_SECONDS)
    assert any(item["uri"] == uri for item in listing["sessions"])
    elapsed = time.monotonic() - begin
    assert elapsed < RESPONSIVE_SECONDS
    h.operator.assert_missing([identifier])
    h.operator.tool("session_write", {"session": uri, "data": "stdio-ready"},
                    timeout=RESPONSIVE_SECONDS)
    value = tool_result(h.operator.receive(identifier))
    assert value["output"] == "stdio-ready" and value["next_cursor"] == 11, value
    assert value["timed_out"] is False and not value["truncated"], value
    h.close_session(uri)
    return {"ping_and_list_ms": round(elapsed * 1000, 3), "output": value["output"]}


def timeouts_and_bounds(h):
    created, _ = h.create()
    uri = created["uri"]
    begin = time.monotonic()
    value = h.operator.tool("session_read", {"session": uri, "wait_ms": 140})
    elapsed = time.monotonic() - begin
    assert .100 <= elapsed < 2, elapsed
    assert value["timed_out"] is True and value["output"] == "", value
    assert value["next_cursor"] == 0 and not value["output_eof"], value
    begin = time.monotonic()
    immediate = h.operator.tool("session_read", {"session": uri, "wait_ms": 0})
    zero_elapsed = time.monotonic() - begin
    assert zero_elapsed < 1 and immediate["output"] == "", (zero_elapsed, immediate)
    for duration in [-1, 30001, True, 1.5, "10"]:
        identifier = h.operator.begin("session_read", {"session": uri, "wait_ms": duration})
        response = h.operator.receive(identifier)
        assert response.get("error", {}).get("code") == -32602, response
    future = h.operator.tool("session_read", {
        "session": uri, "cursor": 1, "wait_ms": 30000}, failed=True,
        timeout=RESPONSIVE_SECONDS)
    assert "cursor" in future["error"], future
    h.close_session(uri)
    return {"timeout_ms": round(elapsed * 1000, 3),
            "zero_wait_ms": round(zero_elapsed * 1000, 3), "invalid_values": 5}


def fragmented_utf8(h):
    child = r'''import os,pathlib,sys,tty
tty.setraw(0)
ready=pathlib.Path(sys.argv[1]); ready.touch()
while True:
    data=os.read(0,1)
    if data==b"A":
        os.write(1,b"\xf0\x9f")
        ready.with_suffix(".half").touch()
    elif data==b"B": os.write(1,b"\x98\x80")
    elif not data: break
'''
    created, ready = h.create(child)
    uri = created["uri"]
    identifier = h.operator.begin("session_read", {
        "session": uri, "cursor": 0, "max_bytes": 1, "wait_ms": 30000})
    h.operator.tool("session_write", {"session": uri, "data": "A"})
    wait_file(ready.with_suffix(".half"))
    h.operator.assert_missing([identifier])
    h.operator.tool("session_write", {"session": uri, "data": "B"})
    value = tool_result(h.operator.receive(identifier))
    assert value["output"] == "😀" and value["next_cursor"] == 4, value
    assert value["timed_out"] is False and not value["has_more"], value
    h.close_session(uri)
    return {"output": value["output"], "cursor": value["next_cursor"],
            "requested_max_bytes": 1}


def retained_output_is_immediate(h):
    child = r'''import os,pathlib,sys,tty
tty.setraw(0)
pathlib.Path(sys.argv[1]).touch()
os.read(0,1)
data=b"x"*(300*1024)
while data:
    data=data[os.write(1,data):]
while os.read(0,1): pass
'''
    created, _ = h.create(child)
    uri = created["uri"]
    h.operator.tool("session_write", {"session": uri, "data": "x"})
    deadline = time.monotonic() + 4
    while True:
        state = h.operator.tool("session_read", {"session": uri})
        if state["truncated"]:
            break
        assert time.monotonic() < deadline, state
        time.sleep(.01)
    begin = time.monotonic()
    value = h.operator.tool("session_read", {
        "session": uri, "cursor": 0, "max_bytes": 7, "wait_ms": 30000},
        timeout=RESPONSIVE_SECONDS)
    elapsed = time.monotonic() - begin
    assert value["truncated"] and value["output"] == "x" * 7, value
    assert value["timed_out"] is False and value["first_cursor"] > 0, value
    assert elapsed < RESPONSIVE_SECONDS, elapsed
    h.close_session(uri)
    return {"expired_cursor_ms": round(elapsed * 1000, 3),
            "first_cursor": value["first_cursor"], "bounded_bytes": len(value["output"])}


def eof_waits(h):
    child = r'''import os,pathlib,sys,tty
tty.setraw(0)
pathlib.Path(sys.argv[1]).touch()
os.read(0,1)
'''
    created, _ = h.create(child)
    uri = created["uri"]
    identifier = h.operator.begin("session_read", {
        "session": uri, "wait_ms": 30000})
    h.operator.tool("session_write", {"session": uri, "data": "x"})
    value = tool_result(h.operator.receive(identifier))
    assert value["output_eof"] is True and value["output"] == "", value
    assert value["timed_out"] is False and not value["has_more"], value
    h.close_session(uri)

    release = h.directory / "descendant-release"
    child = r'''import os,pathlib,signal,sys,time
signal.signal(signal.SIGHUP,signal.SIG_IGN)
r,w=os.pipe()
if os.fork():
    os.close(w); assert os.read(r,1)==b"R"; os.close(r); os._exit(7)
os.close(r)
ready=pathlib.Path(sys.argv[1]); ready.write_text(str(os.getpid()))
os.write(w,b"R"); os.close(w)
deadline=time.monotonic()+8
while not pathlib.Path(sys.argv[2]).exists():
    if time.monotonic()>deadline: os._exit(9)
    time.sleep(.01)
os.write(1,b"LATE")
os._exit(0)
'''
    created, ready = h.create(child, [release])
    uri = created["uri"]
    pidfd = os.pidfd_open(int(ready.read_text()))
    try:
        deadline = time.monotonic() + 3
        while True:
            state = h.operator.tool("session_read", {"session": uri})
            if state["exit_code"] == 7:
                break
            assert time.monotonic() < deadline, state
            time.sleep(.01)
        assert not state["output_eof"] and state["output"] == "", state
        identifier = h.operator.begin("session_read", {
            "session": uri, "wait_ms": 30000})
        h.operator.ping()
        h.operator.assert_missing([identifier])
        release.touch()
        value = tool_result(h.operator.receive(identifier))
        assert value["output"] == "LATE" and value["timed_out"] is False, value
        assert value["exit_code"] == 7, value
        h.close_session(uri)
    finally:
        release.touch(exist_ok=True)
        if not select.select([pidfd], [], [], 2)[0]:
            signal.pidfd_send_signal(pidfd, signal.SIGKILL)
            assert select.select([pidfd], [], [], 2)[0]
        os.close(pidfd)
    return {"silent_eof": "passed", "direct_child_exit_code": 7,
            "descendant_output": "LATE"}


def replacement_identity(h):
    created, _ = h.create(name="identity-wait")
    uri = created["uri"]
    identifier = h.operator.begin("session_read", {
        "session": uri, "wait_ms": 30000})
    # A broker request after the read provides a registration barrier.
    h.operator.tool("session_list")
    h.operator.assert_missing([identifier])
    h.close_session(uri)
    replacement, _ = h.create(name="identity-wait")
    assert replacement["uri"] == uri and replacement["id"] != created["id"]
    h.operator.tool("session_write", {"session": uri, "data": "NEW-PTY-ONLY"})
    response = h.operator.receive(identifier)
    assert is_error(response) and "NEW-PTY-ONLY" not in json.dumps(response), response
    h.close_session(uri)
    return {"old_id": created["id"], "new_id": replacement["id"],
            "old_wait_rejected": True}


def stdio_cancellation_and_limits(h):
    created, _ = h.create()
    uri = created["uri"]
    identifier = h.operator.begin("session_read", {
        "session": uri, "wait_ms": 30000})
    for notification in [cancelled("unknown-request"),
                         {"jsonrpc": "2.0", "method": "notifications/cancelled"},
                         {"jsonrpc": "2.0", "method": "notifications/cancelled",
                          "params": {"requestId": True}}]:
        h.operator.send(notification)
    h.operator.ping()
    h.operator.assert_missing([identifier])
    h.operator.send(cancelled(identifier))
    h.operator.ping()
    h.operator.tool("session_write", {"session": uri, "data": "after-cancel"})
    h.operator.assert_missing([identifier])
    current = h.operator.tool("session_read", {"session": uri})
    assert current["output"] == "after-cancel", current
    cursor = current["next_cursor"]

    waiting = [h.operator.begin("session_read", {
        "session": uri, "cursor": cursor, "wait_ms": 30000})
        for _ in range(STDIO_WAIT_LIMIT)]
    overflow = h.operator.begin("session_read", {
        "session": uri, "cursor": cursor, "wait_ms": 30000})
    assert is_error(h.operator.receive(overflow, RESPONSIVE_SECONDS))
    h.operator.ping()
    assert h.operator.tool("session_list", timeout=RESPONSIVE_SECONDS)["sessions"]
    h.operator.assert_missing(waiting)
    for request_id in waiting:
        h.operator.send(cancelled(request_id))
    h.operator.ping()
    resumed = h.operator.begin("session_read", {
        "session": uri, "cursor": cursor, "wait_ms": 30000})
    h.operator.tool("session_write", {"session": uri, "data": "capacity-restored"})
    value = tool_result(h.operator.receive(resumed))
    assert value["output"] == "capacity-restored", value
    h.operator.assert_missing([identifier, *waiting])

    observer = Stdio(h.binary, read_only=True)
    try:
        pending = observer.begin("session_read", {
            "session": uri, "cursor": value["next_cursor"], "wait_ms": 30000})
        observer.ping()
        observer.assert_missing([pending])
        begin = time.monotonic()
        observer.proc.stdin.close()
        observer.proc.wait(timeout=RESPONSIVE_SECONDS)
        assert observer.proc.returncode == 0 and observer.proc.stdout.read() == b""
        eof_elapsed = time.monotonic() - begin
    finally:
        observer.close()
    assert h.operator.tool("session_read", {"session": uri})["running"]
    h.close_session(uri)
    return {"wait_limit": STDIO_WAIT_LIMIT, "management_responsive": True,
            "quota_reused_after_cancel": True, "stdin_eof_ms": round(eof_elapsed * 1000, 3)}


def http_concurrency_and_observers(h):
    created, _ = h.create()
    uri = created["uri"]
    server, sid = h.start_http()
    observer_headers = {"Authorization": "Bearer owned-wait-observer"}
    status, headers, _ = server.request(INIT, headers=observer_headers)
    assert status == 200
    observer_sid = headers["Mcp-Session-Id"]
    assert server.request(READY, observer_sid, headers=observer_headers)[0] == 202
    first = h.http_wait(server, sid, uri, "operator-wait")
    second = h.http_wait(server, observer_sid, uri, "observer-wait",
                         authorization=observer_headers["Authorization"])
    begin = time.monotonic()
    status, _, value = server.request(packet("ping", "responsive"), sid)
    elapsed = time.monotonic() - begin
    assert status == 200 and value["result"] == {} and elapsed < RESPONSIVE_SECONDS
    first.assert_pending()
    second.assert_pending()
    # The same listener must continue to dispatch a broker mutation while its
    # other clients are parked, including a read-only observer.
    response = server.request(tool_packet("control", "session_control", {
        "session": uri, "action": "acquire", "takeover": True}), sid)
    assert response[0] == 200
    tool_result(response[2])
    response = server.request(tool_packet("write", "session_write", {
        "session": uri, "data": "both-observers"}), sid)
    assert response[0] == 200
    tool_result(response[2])
    values = []
    for pending in [first, second]:
        status, response = pending.receive()
        assert status == 200
        value = tool_result(response)
        assert value["output"] == "both-observers" and value["timed_out"] is False, value
        values.append(value["next_cursor"])
        pending.close()
    assert values == [14, 14]
    h.close_session(uri)
    assert server.request(method="DELETE", sid=observer_sid, headers=observer_headers)[0] == 200
    assert server.request(method="DELETE", sid=sid)[0] == 200
    return {"ping_ms": round(elapsed * 1000, 3), "independent_cursors": values}


def http_cancellation_and_disconnect(h):
    created, _ = h.create()
    uri = created["uri"]
    server, sid = h.start_http()
    other = server.initialize()
    assert server.request(READY, other)[0] == 202
    pending = h.http_wait(server, sid, uri, "same-id")
    pending.assert_pending()
    duplicate = h.http_wait(server, sid, uri, "same-id")
    status, response = duplicate.receive()
    assert status == 200 and is_error(response), (status, response)
    duplicate.close()
    pending.assert_pending()
    assert server.request(cancelled("same-id"), other)[0] == 202
    pending.assert_pending()
    assert server.request(cancelled("same-id"), sid)[0] == 202
    pending.assert_cancelled()
    pending.close()

    waiting = [h.http_wait(server, sid, uri, f"quota-{index}")
               for index in range(HTTP_WAIT_LIMIT)]
    overflow = h.http_wait(server, sid, uri, "overflow")
    status, response = overflow.receive()
    assert status == 503 or (status == 200 and is_error(response)), (status, response)
    overflow.close()
    begin = time.monotonic()
    status, _, response = server.request(packet("ping", "at-limit"), sid)
    elapsed = time.monotonic() - begin
    assert status == 200 and response["result"] == {} and elapsed < RESPONSIVE_SECONDS
    for request in waiting:
        request.assert_pending(timeout=0)
        request.close()
    reused = [h.http_wait(server, sid, uri, f"reused-{index}")
              for index in range(HTTP_WAIT_LIMIT)]
    assert server.request(packet("ping", "full-capacity-reused"), sid)[0] == 200
    for request in reused:
        request.assert_pending(timeout=0)
        request.close()
    # A new request that actually waits demonstrates that disconnected sockets
    # release capacity rather than merely rejecting or closing every new read.
    restored = h.http_wait(server, sid, uri, "restored", wait_ms=120)
    status, response = restored.receive()
    assert status == 200 and tool_result(response)["timed_out"] is True, response
    restored.close()
    deleted = h.http_wait(server, sid, uri, "deleted-session")
    deleted.assert_pending()
    assert server.request(method="DELETE", sid=sid)[0] == 200
    deleted.assert_cancelled()
    deleted.close()
    assert h.operator.tool("session_read", {"session": uri})["running"]
    assert server.request(method="DELETE", sid=other)[0] == 200
    h.close_session(uri)
    return {"wait_limit": HTTP_WAIT_LIMIT, "ping_at_limit_ms": round(elapsed * 1000, 3),
            "duplicate_id_rejected": True, "cross_session_cancel_ignored": True,
            "disconnect_releases_capacity": True,
            "delete_retains_terminal": True}


def http_long_wait(h):
    created, _ = h.create()
    uri = created["uri"]
    server, sid = h.start_http()
    begin = time.monotonic()
    pending = h.http_wait(server, sid, uri, "long-timeout", wait_ms=11000)
    status, response = pending.receive()
    elapsed = time.monotonic() - begin
    assert status == 200, (status, response)
    value = tool_result(response)
    assert value["timed_out"] is True and value["next_cursor"] == 0, value
    assert value["output"] == "" and 10.5 <= elapsed < 15, (elapsed, value)
    pending.close()
    assert server.request(method="DELETE", sid=sid)[0] == 200
    h.close_session(uri)
    return {"requested_ms": 11000, "elapsed_ms": round(elapsed * 1000, 3)}


def completed_input_preserves_ordinary_requests(h):
    """EOF cancels parked reads, while complete ordinary calls still finish."""
    created, _ = h.create()
    uri = created["uri"]
    client = Stdio(h.binary, initialize=False)
    try:
        messages = [INIT, READY,
                    tool_packet("eof-list", "session_list"),
                    tool_packet("eof-read", "session_read", {
                        "session": uri, "wait_ms": 0})]
        wire = b"".join(json.dumps(message).encode() + b"\n" for message in messages)
        assert len(wire) < select.PIPE_BUF
        # One atomic pipe write followed immediately by EOF exercises both
        # buffered-message handling and pending broker response completion.
        assert os.write(client.proc.stdin.fileno(), wire) == len(wire)
        client.proc.stdin.close()
        assert "result" in client.receive(1)
        listing = tool_result(client.receive("eof-list"))
        assert any(item["uri"] == uri for item in listing["sessions"]), listing
        value = tool_result(client.receive("eof-read"))
        assert value["uri"] == uri and value["output"] == "" and value["running"], value
        client.proc.wait(timeout=RESPONSIVE_SECONDS)
        assert client.proc.returncode == 0
        assert client.proc.stdout.read() == b"" and not client.buffer and not client.responses
    finally:
        client.close()

    server, sid = h.start_http()
    for identifier, name, arguments in [
        ("half-list", "session_list", {}),
        ("half-read", "session_read", {"session": uri, "wait_ms": 0}),
    ]:
        pending = HttpRead(server, sid, tool_packet(identifier, name, arguments))
        try:
            pending.connection.sock.shutdown(socket.SHUT_WR)
            assert select.select([pending.connection.sock], [], [], RESPONSIVE_SECONDS)[0], \
                "ordinary HTTP request did not finish after write-side EOF"
            status, response = pending.receive()
            assert status == 200 and response["id"] == identifier, (status, response)
            value = tool_result(response)
            if name == "session_list":
                assert any(item["uri"] == uri for item in value["sessions"]), value
            else:
                assert value["uri"] == uri and value["running"], value
        finally:
            pending.close()
    assert server.request(method="DELETE", sid=sid)[0] == 200
    h.close_session(uri)
    return {"single_stdio_write_messages": 4, "stdio_response_ids": [1, "eof-list", "eof-read"],
            "http_half_closed_ordinary_requests": 2}


def main(binary, output=None):
    report = {"binary_sha256": hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest(),
              "checks": {}, "cleanup": "pending"}
    original = {key: os.environ.get(key) for key in [
        "ZTTY_SOCKET", "XDG_RUNTIME_DIR", "ZTTY_MCP_TOKEN", "ZTTY_MCP_OBSERVER_TOKEN"]}
    try:
        with tempfile.TemporaryDirectory(prefix="ztty-mcp-wait-") as directory:
            os.environ["ZTTY_SOCKET"] = f"@ztty-wait-{os.getpid()}-{time.monotonic_ns()}"
            os.environ["XDG_RUNTIME_DIR"] = directory
            os.environ.pop("ZTTY_MCP_TOKEN", None)
            os.environ.pop("ZTTY_MCP_OBSERVER_TOKEN", None)
            harness = Harness(binary, directory)
            try:
                for test in [stdio_concurrency, timeouts_and_bounds, fragmented_utf8,
                             retained_output_is_immediate,
                             eof_waits, replacement_identity, stdio_cancellation_and_limits,
                             http_concurrency_and_observers, http_cancellation_and_disconnect,
                             http_long_wait, completed_input_preserves_ordinary_requests]:
                    begin = time.monotonic()
                    try:
                        details = test(harness)
                    except Exception as error:
                        report["checks"][test.__name__] = {
                            "status": "failed", "error": str(error),
                            "elapsed_ms": round((time.monotonic() - begin) * 1000, 3)}
                        raise
                    report["checks"][test.__name__] = {
                        "status": "passed", "elapsed_ms": round((time.monotonic() - begin) * 1000, 3),
                        **details}
            finally:
                harness.shutdown()
                report["cleanup"] = "passed"
    finally:
        for key, value in original.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        rendered = json.dumps(report, indent=2, ensure_ascii=False)
        if output:
            pathlib.Path(output).parent.mkdir(parents=True, exist_ok=True)
            pathlib.Path(output).write_text(rendered + "\n")
        print(rendered)
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--output")
    arguments = parser.parse_args()
    main(os.path.abspath(arguments.binary), arguments.output)
