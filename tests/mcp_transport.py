#!/usr/bin/env python3
"""Exercise ztty MCP framing, lifecycle and both live transports."""
import argparse
import concurrent.futures
import http.client
import json
import os
import re
import select
import socket
import subprocess
import time

INIT = {"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                   "clientInfo": {"name": "ztty-transport-test", "version": "1"}}}
READY = {"jsonrpc": "2.0", "method": "notifications/initialized"}
LIST = {"jsonrpc": "2.0", "id": 2, "method": "tools/list"}
HEADERS = {"Content-Type": "application/json",
           "Accept": "application/json, text/event-stream"}

def packet(method, identifier=1, params=None):
    value = {"jsonrpc": "2.0", "id": identifier, "method": method}
    if params is not None:
        value["params"] = params
    return value

def line(proc):
    assert select.select([proc.stdout], [], [], 5)[0], "stdio response timed out"
    data = proc.stdout.readline()
    assert data, f"MCP process exited {proc.poll()}"
    return json.loads(data)

def send(proc, value):
    proc.stdin.write(json.dumps(value, ensure_ascii=False).encode() + b"\n")
    proc.stdin.flush()

def stdio(binary):
    env = dict(os.environ)
    env.pop("ZTTY_MCP_TOKEN", None)
    proc = subprocess.Popen([binary, "mcp"], stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    try:
        send(proc, LIST)
        assert line(proc)["error"]["code"] == -32000
        send(proc, packet("ping", 9007199254740993123456789))
        assert line(proc)["id"] == 9007199254740993123456789
        initial = json.loads(json.dumps(INIT))
        initial["params"]["protocolVersion"] = "2025-06-18"
        send(proc, initial)
        assert line(proc)["result"]["protocolVersion"] == "2025-06-18"
        send(proc, READY)
        send(proc, LIST)
        tools = line(proc)["result"]["tools"]
        assert {tool["name"] for tool in tools} == {
            "session_create", "session_list", "session_read",
            "session_write", "session_resize", "session_close", "session_control"}
        schemas = {tool["name"]: tool["inputSchema"] for tool in tools}
        assert schemas["session_read"]["properties"]["max_bytes"]["maximum"] == 65536
        assert schemas["session_write"]["properties"]["data"]["maxLength"] == 65536
        send(proc, packet("tools/call", 21, {"name": "session_read", "arguments": {"session": "unused", "max_bytes": 65537}}))
        assert line(proc)["error"]["code"] == -32602
        send(proc, packet("tools/call", 22, {"name": "session_write", "arguments": {"session": "unused", "data": "界" * 21846}}))
        assert line(proc)["error"]["code"] == -32602
        send(proc, packet("does/not/exist", "🚀"))
        unknown = line(proc)
        assert unknown["id"] == "🚀" and unknown["error"]["code"] == -32601
        send(proc, packet("tools/call", 4, {"name": "session_create", "arguments": {"op": "close"}}))
        assert line(proc)["error"]["code"] == -32602
        proc.stdin.write(b'{"broken"\n')
        proc.stdin.flush()
        assert line(proc)["error"]["code"] == -32700
        send(proc, packet("tools/call", 5, {"name": "session_list", "arguments": {}}))
        assert line(proc)["result"]["isError"] is False
        send(proc, packet("ping", 6))
        assert line(proc) == {"jsonrpc": "2.0", "id": 6, "result": {}}
        proc.stdin.close()
        proc.wait(timeout=5)
        assert proc.returncode == 0 and proc.stdout.read() == b""
        return {"stdio": "passed", "tool_count": len(tools)}
    finally:
        if proc.poll() is None:
            proc.terminate()
            proc.wait(timeout=5)
        proc.stdout.close()
        proc.stderr.close()

class HttpServer:
    def __init__(self, binary, host="127.0.0.1", token=None, observer_token=None, extra_args=()):
        env = dict(os.environ)
        env.pop("ZTTY_MCP_TOKEN", None)
        env.pop("ZTTY_MCP_OBSERVER_TOKEN", None)
        if observer_token:
            env["ZTTY_MCP_OBSERVER_TOKEN"] = observer_token
        if token:
            env["ZTTY_MCP_TOKEN"] = token
        self.proc = subprocess.Popen([binary, "mcp", "--transport", "http",
                                      "--host", host, "--port", "0", *extra_args],
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
        assert select.select([self.proc.stderr], [], [], 5)[0], "HTTP startup timed out"
        message = self.proc.stderr.readline().decode()
        match = re.search(r"http://(?:\[[^\]]+\]|[^:]+):(\d+)/mcp", message)
        if not match:
            self.close()
            raise AssertionError(f"HTTP startup failed: {message}")
        self.host = host
        self.port = int(match.group(1))
        self.token = token

    def request(self, value=None, sid=None, method="POST", headers=None, chunked=False):
        connection = http.client.HTTPConnection(self.host, self.port, timeout=5)
        try:
            merged = dict(HEADERS)
            if sid:
                merged["Mcp-Session-Id"] = sid
                merged["Mcp-Protocol-Version"] = "2025-11-25"
            if self.token:
                merged["Authorization"] = "Bearer " + self.token
            if headers:
                merged.update(headers)
            body = json.dumps(value).encode() if value is not None else b""
            if chunked:
                body = [body[:7], body[7:31], body[31:]]
            connection.request(method, "/mcp", body=body, headers=merged, encode_chunked=chunked)
            response = connection.getresponse()
            payload = response.read()
            return response.status, dict(response.getheaders()), json.loads(payload) if payload else None
        finally:
            connection.close()

    def initialize(self, chunked=False):
        status, headers, body = self.request(INIT, chunked=chunked)
        assert status == 200 and body["result"]["protocolVersion"] == "2025-11-25"
        sid = headers["Mcp-Session-Id"]
        assert len(sid) == 48 and all(c in "0123456789abcdef" for c in sid)
        return sid

    def close(self):
        if self.proc.poll() is None:
            self.proc.terminate()
            self.proc.wait(timeout=5)
        self.proc.stdout.close()
        self.proc.stderr.close()

def http_transport(binary):
    server = HttpServer(binary)
    slow = None
    try:
        assert server.request(method="GET")[0] == 405
        assert server.request(INIT, headers={"Origin": "https://attacker.invalid"})[0] == 403
        assert server.request(INIT, headers={"Host": "attacker.invalid"})[0] == 403
        assert server.request(INIT, headers={"Content-Type": "text/plain"})[0] == 415
        assert server.request(INIT, headers={"Accept": "application/json"})[0] == 406
        assert server.request(LIST)[0] == 400
        assert server.request(INIT, headers={"Mcp-Protocol-Version": "bad"})[0] == 400
        sid = server.initialize()
        other = server.initialize(chunked=True)
        assert sid != other
        assert server.request(READY, sid)[0] == 202
        assert server.request(LIST, sid)[2]["result"]["tools"]
        assert server.request(LIST, other)[2]["error"]["code"] == -32000
        assert server.request(READY, other)[0] == 202
        slow = socket.create_connection((server.host, server.port), timeout=5)
        slow.sendall(b"POST /mcp HTTP/1.1\r\nHost: ")
        begin = time.monotonic()
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            responses = list(pool.map(lambda _: server.request(LIST, sid)[0], range(16)))
        assert responses == [200] * 16 and time.monotonic() - begin < 5
        assert server.request(LIST, sid, headers={"Mcp-Protocol-Version": "2025-06-18"})[0] == 400
        assert server.request(method="DELETE", sid=sid)[0] == 200
        assert server.request(LIST, sid)[0] == 404
        assert server.request(LIST, other)[0] == 200
        assert server.request(method="DELETE", sid=other)[0] == 200
        return {"http": "passed", "concurrent_requests": 16, "chunked": "passed"}
    finally:
        if slow:
            slow.close()
        server.close()

def authentication(binary):
    env = dict(os.environ)
    env.pop("ZTTY_MCP_TOKEN", None)
    denied = subprocess.run([binary, "mcp", "--http", "--host", "0.0.0.0", "--port", "0"],
                            env=env, capture_output=True, timeout=5)
    assert denied.returncode != 0 and b"require" in denied.stderr
    server = HttpServer(binary, token="ztty-test-only-bearer-token")
    try:
        assert server.request(INIT, headers={"Authorization": ""})[0] == 401
        assert server.request(INIT, headers={"Authorization": "Bearer wrong"})[0] == 401
        sid = server.initialize()
        assert server.request(method="DELETE", sid=sid)[0] == 200
    finally:
        server.close()
    return {"authentication": "passed"}

def ipv6(binary):
    if not socket.has_ipv6:
        return {"ipv6": "unavailable"}
    probe = socket.socket(socket.AF_INET6)
    try:
        probe.bind(("::1", 0))
    except OSError:
        return {"ipv6": "unavailable"}
    finally:
        probe.close()
    server = HttpServer(binary, host="::1")
    try:
        sid = server.initialize()
        assert server.request(READY, sid)[0] == 202
        assert server.request(LIST, sid)[0] == 200
        assert server.request(method="DELETE", sid=sid)[0] == 200
        return {"ipv6": "passed"}
    finally:
        server.close()

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    args = parser.parse_args()
    binary = os.path.abspath(args.binary)
    os.environ.setdefault("ZTTY_SOCKET", f"@ztty-mcp-transport-{os.getpid()}")
    report = {}
    for test in [stdio, http_transport, authentication, ipv6]:
        report.update(test(binary))
    print(json.dumps(report, indent=2))
