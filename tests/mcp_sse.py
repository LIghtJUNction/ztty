#!/usr/bin/env python3
"""Verify opt-in MCP SSE responses against real PTYs and an owned broker.

HTTP chunk decoding is delegated to Python's standard library; SSE framing,
early headers, comments, exact request IDs and terminal stream completion are
checked independently. No existing terminal or broker is touched.
"""
import argparse
import hashlib
import http.client
import json
import os
import pathlib
import socket
import subprocess
import tempfile
import time

from mcp_transport import HEADERS, HttpServer, INIT, READY, packet
from mcp_wait import (
    HTTP_WAIT_LIMIT, Harness, cancelled, numeric_read,
    tool_packet, tool_result, wire_id,
)

RESPONSIVE_SECONDS = 2.0
MAX_SSE_BLOCK = 2 * 1024 * 1024


class SseRequest:
    """One POST and a strict, bounded SSE parser over decoded HTTP chunks."""
    def __init__(self, h, sid, value, authorization=None, half_close=False):
        self.connection = http.client.HTTPConnection(
            h.http.host, h.http.port, timeout=RESPONSIVE_SECONDS)
        headers = {**HEADERS, "Authorization": "Bearer " + h.http.token}
        if sid is not None:
            headers.update({"Mcp-Session-Id": sid,
                            "Mcp-Protocol-Version": "2025-11-25"})
        if authorization is not None:
            headers["Authorization"] = authorization
        body = value.encode() if isinstance(value, str) else json.dumps(
            value, ensure_ascii=False).encode()
        self.response = None
        self.comments = []
        self.started = time.monotonic()
        self.connection.request("POST", "/mcp", body, headers)
        self.socket = self.connection.sock
        if half_close:
            self.socket.shutdown(socket.SHUT_WR)
        h.pending_http.append(self)
        self.response = self.connection.getresponse()
        self.headers_ms = (time.monotonic() - self.started) * 1000
        assert self.response.status == 200, {
            "status": self.response.status, "body": self.response.read()[:200]}
        assert self.response.getheader("Content-Type", "").split(";")[0].strip() == "text/event-stream"
        assert self.response.getheader("Transfer-Encoding", "").lower() == "chunked"
        assert self.response.getheader("Content-Length") is None
        assert self.response.getheader("Connection", "").lower() == "close"

    def block(self, timeout=RESPONSIVE_SECONDS):
        self.socket.settimeout(timeout)
        lines = []
        size = 0
        while True:
            line = self.response.readline(MAX_SSE_BLOCK + 1)
            size += len(line)
            assert size <= MAX_SSE_BLOCK, "unbounded SSE block"
            if not line:
                assert not lines, ("partial SSE block", lines)
                return None
            assert line.endswith(b"\n") and b"\r" not in line, ("unsafe SSE line", line[:100])
            if line == b"\n":
                if lines:
                    return lines
                continue
            lines.append(line[:-1])

    def initial_comment(self):
        block = self.block()
        assert block == [b": ztty"], ("missing immediate SSE priming comment", block)
        elapsed = time.monotonic() - self.started
        assert elapsed < RESPONSIVE_SECONDS, ("SSE headers/comment were delayed", elapsed)
        self.comments.append(block[0].decode())
        return round(elapsed * 1000, 3)

    def finish(self, timeout=RESPONSIVE_SECONDS):
        while True:
            block = self.block(timeout)
            assert block is not None, "SSE stream ended without its JSON-RPC response"
            if all(line.startswith(b":") for line in block):
                self.comments.extend(line.decode() for line in block)
                continue
            assert len(block) == 2 and block[0] == b"event: message", block
            assert block[1].startswith(b"data: "), block
            payload = block[1][6:]
            value = json.loads(payload)
            # A complete HTTP zero chunk is mandatory for a successful stream:
            # HTTPResponse raises IncompleteRead when a sender just closes TCP.
            assert self.block(timeout) is None, "more than one final SSE event"
            return value, payload

    def assert_cancelled(self):
        self.socket.settimeout(RESPONSIVE_SECONDS)
        started = time.monotonic()
        try:
            remaining = self.response.read()
        except http.client.IncompleteRead as error:
            remaining = error.partial
        assert b"event:" not in remaining and b"data:" not in remaining, (
            "cancelled request delivered a result", remaining[:200])
        assert time.monotonic() - started < RESPONSIVE_SECONDS

    def close(self):
        if self.response is not None:
            self.response.close()
        self.connection.close()


def initialize(h, authorization=None, identifier="sse-initialize"):
    request = SseRequest(h, None, {**INIT, "id": identifier}, authorization)
    value, wire = request.finish()
    assert value["id"] == identifier
    assert value["result"]["protocolVersion"] == "2025-11-25"
    sid = request.response.getheader("Mcp-Session-Id")
    assert sid and len(sid) == 48 and all(c in "0123456789abcdef" for c in sid)
    request.close()
    headers = {"Authorization": authorization} if authorization else None
    assert h.http.request(READY, sid, headers=headers)[0] == 202
    return sid, wire


def start_sse(h):
    if h.http is None:
        h.http = HttpServer(
            h.binary, token="owned-sse-operator", observer_token="owned-sse-observer",
            extra_args=("--http-response", "sse"))
    return initialize(h)[0]


def response(h, sid, value, **kwargs):
    request = SseRequest(h, sid, value, **kwargs)
    try:
        return request.finish()
    finally:
        request.close()


def read_wait(h, sid, uri, identifier, cursor=0, wait_ms=30000, raw=False):
    arguments = {"session": uri, "cursor": cursor, "wait_ms": wait_ms}
    value = numeric_read(identifier, arguments) if raw else tool_packet(
        identifier, "session_read", arguments)
    request = SseRequest(h, sid, value)
    request.initial_comment()
    return request


def sse_lifecycle(h):
    h.sid = start_sse(h)
    identifier = "init🙂\r\nid: injected\ndata: injected"
    extra, wire = initialize(h, identifier=identifier)
    assert wire_id(wire) == identifier and b"\n" not in wire and b"\r" not in wire
    value, _ = response(h, h.sid, packet("ping", "ping"))
    assert value == {"jsonrpc": "2.0", "id": "ping", "result": {}}
    value, _ = response(h, h.sid, packet("missing/method", "request-error"))
    assert value["error"]["code"] == -32601
    value, _ = response(h, h.sid, tool_packet("bad-arguments", "session_read", {}))
    assert value["error"]["code"] == -32602
    for accepted in ["application/json", "text/event-stream",
                     "application/json, text/event-stream;q=0"]:
        assert h.http.request(packet("ping", "bad-accept"), h.sid,
                              headers={"Accept": accepted})[0] == 406
    status, headers, value = h.http.request({"jsonrpc": "wrong"}, h.sid)
    assert status == 400 and headers["Content-Type"] == "application/json"
    assert value["error"]["code"] == -32600
    assert h.http.request(packet("ping"), h.sid,
                          headers={"Authorization": "Bearer wrong"})[0] == 401
    assert h.http.request(packet("ping"), "unknown-session")[0] == 404
    assert h.http.request(method="GET", sid=h.sid)[0] == 405
    assert h.http.request(method="DELETE", sid=extra)[0] == 200
    assert h.http.request(packet("ping"), extra)[0] == 404
    return {"sse_initialization": True, "single_event_and_zero_chunk": True,
            "escaped_identifier": identifier, "request_errors_are_sse": True,
            "http_errors_remain_json": True, "notifications_empty_202": True,
            "get_not_advertised": True}


GATED_CHILD = r"""import os,pathlib,sys,time,tty
tty.setraw(0)
pathlib.Path(sys.argv[1]).touch()
while not pathlib.Path(sys.argv[2]).exists():
    time.sleep(.005)
os.write(1,sys.argv[3].encode())
os.read(0,1)
"""


def prompt_payload_and_observer(h):
    sid = start_sse(h)
    gate = h.directory / "release-sse-payload"
    payload = 'SSE🙂\r\nevent: forged\ndata: {"fake":true}\n: comment\t\\done\n'
    created, _ = h.create(GATED_CHILD, (gate, payload))
    uri = created["uri"]
    request = read_wait(h, sid, uri, "payload")
    assert not gate.exists(), "child released before early stream proof"
    observer, _ = initialize(h, "Bearer owned-sse-observer", "observer-init")
    observed = SseRequest(h, observer, tool_packet("observe", "session_read", {
        "session": uri, "cursor": 0, "wait_ms": 30000}),
        authorization="Bearer owned-sse-observer")
    observed.initial_comment()
    denied, _ = response(h, observer, tool_packet("deny-write", "session_write", {
        "session": uri, "data": "must-not-write"}),
        authorization="Bearer owned-sse-observer")
    assert denied["error"]["code"] == -32001
    assert h.http.request(packet("ping"), sid, headers={
        "Authorization": "Bearer owned-sse-observer"})[0] == 403
    gate.touch()
    first, wire = request.finish()
    second, _ = observed.finish()
    for value in [first, second]:
        result = tool_result(value)
        assert result["output"] == payload, result
        assert result["next_cursor"] == len(payload.encode()) and not result["timed_out"]
    assert b"\n" not in wire and b"\r" not in wire, "PTY output escaped SSE data line"
    elapsed = request.headers_ms
    request.close()
    observed.close()
    h.close_session(uri)
    return {"headers_before_child_release_ms": round(elapsed, 3),
            "payload_bytes": len(payload.encode()), "observer_read": True,
            "observer_write_denied": True, "role_session_isolation": True,
            "output_has_no_sse_line_injection": True}


def process_cpu(pid):
    # Field 3 is enclosed in parentheses and may contain spaces.
    fields = pathlib.Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")


def heartbeat_and_idle(h):
    sid = start_sse(h)
    created, _ = h.create()
    uri = created["uri"]
    request = read_wait(h, sid, uri, "heartbeat", wait_ms=10000)
    started = time.monotonic()
    cpu_before = process_cpu(h.http.proc.pid)
    block = request.block(timeout=7.5)
    elapsed = time.monotonic() - started
    cpu = process_cpu(h.http.proc.pid) - cpu_before
    assert block and all(line.startswith(b":") for line in block), block
    assert 4.0 <= elapsed < 7.5, ("heartbeat cadence", elapsed, block)
    assert cpu < max(.5, elapsed * .2), ("idle SSE transport busy-spun", cpu, elapsed)
    request.comments.extend(line.decode() for line in block)
    h.operator.tool("session_write", {"session": uri, "data": "HEARTBEAT"})
    value, _ = request.finish()
    assert tool_result(value)["output"] == "HEARTBEAT"
    assert len(request.comments) == 2, request.comments
    request.close()
    h.close_session(uri)
    return {"heartbeat_ms": round(elapsed * 1000, 3),
            "transport_cpu_seconds": round(cpu, 3),
            "comments": request.comments, "final_output_after_heartbeat": True}


def timeout_and_half_close(h):
    sid = start_sse(h)
    created, _ = h.create()
    uri = created["uri"]
    request = read_wait(h, sid, uri, "short-timeout", wait_ms=150)
    value, _ = request.finish()
    elapsed = time.monotonic() - request.started
    result = tool_result(value)
    assert result["timed_out"] and result["output"] == "" and result["next_cursor"] == 0
    assert .08 <= elapsed < RESPONSIVE_SECONDS, elapsed
    request.close()
    for name, arguments in [
        ("session_list", {}),
        ("session_read", {"session": uri, "cursor": 0, "wait_ms": 0}),
    ]:
        ordinary = SseRequest(h, sid, tool_packet("half-" + name, name, arguments),
                              half_close=True)
        tool_result(ordinary.finish()[0])
        ordinary.close()
    timed = read_wait(h, sid, uri, "half-closed-wait")
    timed.socket.shutdown(socket.SHUT_WR)
    timed.assert_cancelled()
    timed.close()
    h.close_session(uri)
    return {"timeout_ms": round(elapsed * 1000, 3), "ordinary_half_close": True,
            "waiting_half_close_releases_request": True}


def cancellation_and_exact_ids(h):
    sid = start_sse(h)
    other, _ = initialize(h, identifier="other-initialize")
    created, _ = h.create()
    uri = created["uri"]
    first = read_wait(h, sid, uri, "1e0", raw=True)
    duplicate = SseRequest(h, sid, numeric_read("1.0", {
        "session": uri, "cursor": 0, "wait_ms": 30000}))
    denied, wire = duplicate.finish()
    assert denied["error"]["code"] == -32600 and wire_id(wire) == ("number", "1.0")
    duplicate.close()
    assert h.http.request(cancelled(1), other)[0] == 202
    h.operator.tool("session_write", {"session": uri, "data": "ONE"})
    value, wire = first.finish()
    assert tool_result(value)["output"] == "ONE" and wire_id(wire) == ("number", "1e0")
    first.close()
    cursor = len("ONE")
    exact = read_wait(h, sid, uri, "7.0", cursor=cursor, raw=True)
    # HttpServer.request JSON-encodes strings, so send the cancellation object
    # with an exact integer peer rather than losing its numeric type.
    assert h.http.request(cancelled(7), sid)[0] == 202
    exact.assert_cancelled()
    exact.close()
    lower = read_wait(h, sid, uri, "9007199254740992", cursor=cursor, raw=True)
    upper = read_wait(h, sid, uri, "9007199254740993", cursor=cursor, raw=True)
    assert h.http.request(cancelled(9007199254740992), sid)[0] == 202
    lower.assert_cancelled()
    lower.close()
    h.operator.tool("session_write", {"session": uri, "data": "TWO"})
    value, wire = upper.finish()
    assert tool_result(value)["output"] == "TWO"
    assert wire_id(wire) == ("number", "9007199254740993")
    upper.close()
    assert h.http.request(method="DELETE", sid=other)[0] == 200
    h.close_session(uri)
    return {"equivalent_numeric_duplicate_rejected": True,
            "equivalent_numeric_cancelled": True, "original_token_echoed": True,
            "adjacent_large_ids_remain_distinct": True,
            "cross_session_cancel_ignored": True}


def capacity_disconnect_and_delete(h):
    sid = start_sse(h)
    created, _ = h.create()
    uri = created["uri"]
    pending = [read_wait(h, sid, uri, f"capacity-{index}")
               for index in range(HTTP_WAIT_LIMIT)]
    overflow, _ = response(h, sid, tool_packet("overflow", "session_read", {
        "session": uri, "cursor": 0, "wait_ms": 30000}))
    assert overflow["error"]["code"] == -32000, overflow
    begin = time.monotonic()
    ping, _ = response(h, sid, packet("ping", "capacity-ping"))
    listing, _ = response(h, sid, tool_packet("capacity-list", "session_list"))
    elapsed = time.monotonic() - begin
    assert ping["result"] == {} and elapsed < RESPONSIVE_SECONDS
    tool_result(listing)
    for request in pending:
        request.close()
    reused = [read_wait(h, sid, uri, f"reused-{index}")
              for index in range(HTTP_WAIT_LIMIT)]
    h.operator.tool("session_write", {"session": uri, "data": "REUSED"})
    for request in reused:
        assert tool_result(request.finish()[0])["output"] == "REUSED"
        request.close()
    deleted = read_wait(h, sid, uri, "delete-pending", cursor=len("REUSED"))
    assert h.http.request(method="DELETE", sid=sid)[0] == 200
    deleted.assert_cancelled()
    deleted.close()
    assert h.operator.tool("session_read", {"session": uri})["running"]
    h.close_session(uri)
    return {"wait_limit": HTTP_WAIT_LIMIT, "all_slots_reused_after_disconnect": True,
            "management_at_limit_ms": round(elapsed * 1000, 3),
            "delete_cancels_stream_and_preserves_terminal": True}


def json_default_and_cli(h):
    for arguments in [(), ("--http-response", "json")]:
        server = HttpServer(h.binary, extra_args=arguments)
        try:
            sid = server.initialize()
            assert server.request(READY, sid)[0] == 202
            status, headers, value = server.request(packet("ping", "json"), sid)
            assert status == 200 and headers["Content-Type"] == "application/json"
            assert "Transfer-Encoding" not in headers and "Content-Length" in headers
            assert value["result"] == {}
        finally:
            server.close()
    invalid = [
        ["mcp", "--http-response", "sse"],
        ["mcp", "--transport", "stdio", "--http-response", "json"],
        ["mcp", "--transport", "http", "--http-response", "invalid"],
        ["mcp", "--transport", "http", "--http-response"],
    ]
    for arguments in invalid:
        result = subprocess.run([h.binary, *arguments], input=b"", capture_output=True, timeout=3)
        assert result.returncode != 0, ("invalid option accepted", arguments)
    return {"default_json": True, "explicit_json": True,
            "invalid_cli_cases": len(invalid)}


CHECKS = (
    sse_lifecycle, prompt_payload_and_observer, heartbeat_and_idle,
    timeout_and_half_close, cancellation_and_exact_ids,
    capacity_disconnect_and_delete, json_default_and_cli,
)


def main(binary, output=None, checks=None):
    selected = CHECKS if checks is None else tuple(
        check for check in CHECKS if check.__name__ in checks)
    report = {"binary_sha256": hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest(),
              "selected_checks": [check.__name__ for check in selected],
              "checks": {}, "cleanup": "pending"}
    keys = ["ZTTY_SOCKET", "XDG_RUNTIME_DIR", "ZTTY_MCP_TOKEN", "ZTTY_MCP_OBSERVER_TOKEN"]
    original = {key: os.environ.get(key) for key in keys}
    try:
        with tempfile.TemporaryDirectory(prefix="ztty-mcp-sse-") as directory:
            os.environ["ZTTY_SOCKET"] = f"@ztty-sse-{os.getpid()}-{time.monotonic_ns()}"
            os.environ["XDG_RUNTIME_DIR"] = directory
            os.environ.pop("ZTTY_MCP_TOKEN", None)
            os.environ.pop("ZTTY_MCP_OBSERVER_TOKEN", None)
            harness = None
            try:
                harness = Harness(binary, directory)
                for check in selected:
                    started = time.monotonic()
                    try:
                        details = check(harness)
                    except Exception as error:
                        report["checks"][check.__name__] = {
                            "status": "failed", "error_type": type(error).__name__,
                            "error": str(error),
                            "elapsed_ms": round((time.monotonic() - started) * 1000, 3)}
                        raise
                    report["checks"][check.__name__] = {
                        "status": "passed",
                        "elapsed_ms": round((time.monotonic() - started) * 1000, 3), **details}
            finally:
                if harness is not None:
                    try:
                        harness.shutdown()
                    except Exception as error:
                        report["cleanup"] = "failed"
                        report["cleanup_error"] = str(error)
                        raise
                    report["cleanup"] = "passed"
                else:
                    report["cleanup"] = "not-started"
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
    parser.add_argument("--check", dest="checks", action="append",
                        choices=[check.__name__ for check in CHECKS])
    args = parser.parse_args()
    main(os.path.abspath(args.binary), args.output, args.checks)
