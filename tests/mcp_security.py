#!/usr/bin/env python3
"""Check MCP observer authorization and hostile HTTP frames against a real binary."""
import argparse
import json
import os
import signal
import socket
import subprocess
import tempfile

from mcp_transport import HttpServer, INIT, READY, LIST, line, packet, send
from mcp_sessions import StdioClient


def raw_tool(proc, name, arguments=None):
    send(proc, packet("tools/call", 70, {"name": name, "arguments": arguments or {}}))
    return line(proc)


def observer_stdio(binary, uri):
    proc = subprocess.Popen([binary, "mcp", "--read-only"], stdin=subprocess.PIPE,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=os.environ)
    try:
        send(proc, INIT)
        assert "result" in line(proc)
        send(proc, READY)
        send(proc, LIST)
        names = {t["name"] for t in line(proc)["result"]["tools"]}
        assert names == {"session_list", "session_read", "session_commands", "session_command_read"}
        read = raw_tool(proc, "session_read", {"session": uri})
        assert read["result"]["isError"] is False
        for name, args in [
            ("session_create", {"name": "observer-must-not-create"}),
            ("session_write", {"session": uri, "data": "printf PWNED\n"}),
            ("session_resize", {"session": uri, "cols": 90, "rows": 30}),
            ("session_close", {"session": uri}),
            ("session_control", {"session": uri, "action": "acquire", "takeover": True}),
        ]:
            response = raw_tool(proc, name, args)
            assert response["error"]["code"] == -32001, response
        forged = raw_tool(proc, "session_write", {"session": uri, "data": "x", "_actor": "local"})
        assert forged["error"]["code"] == -32602
        proc.stdin.close()
        proc.wait(timeout=5)
        assert proc.returncode == 0
    finally:
        if proc.poll() is None:
            proc.terminate()
            proc.wait(timeout=5)
        proc.stdout.close()
        proc.stderr.close()


def rpc(server, sid, name, args, authorization=None):
    headers = {"Authorization": authorization} if authorization else None
    return server.request(packet("tools/call", 8, {"name": name, "arguments": args}),
                          sid=sid, headers=headers)


def observer_http(binary, uri):
    server = HttpServer(binary, token="test-operator", observer_token="test-observer")
    try:
        operator_sid = server.initialize()
        assert server.request(READY, operator_sid)[0] == 202
        observer_header = {"Authorization": "Bearer test-observer"}
        status, headers, result = server.request(INIT, headers=observer_header)
        assert status == 200 and "result" in result
        observer_sid = headers["Mcp-Session-Id"]
        assert server.request(READY, observer_sid, headers=observer_header)[0] == 202
        status, _, body = server.request(LIST, observer_sid, headers=observer_header)
        assert status == 200
        assert {t["name"] for t in body["result"]["tools"]} == {"session_list", "session_read", "session_commands", "session_command_read"}
        assert rpc(server, observer_sid, "session_read", {"session": uri},
                   "Bearer test-observer")[2]["result"]["isError"] is False
        for name, args in [
            ("session_create", {}),
            ("session_write", {"session": uri, "data": "printf PWNED\n"}),
            ("session_resize", {"session": uri, "cols": 90, "rows": 30}),
            ("session_close", {"session": uri}),
            ("session_control", {"session": uri, "action": "acquire", "takeover": True}),
        ]:
            status, _, body = rpc(server, observer_sid, name, args, "Bearer test-observer")
            assert status == 200 and body["error"]["code"] == -32001, body
        assert server.request(LIST, operator_sid, headers=observer_header)[0] == 403
        assert server.request(LIST, observer_sid)[0] == 403
        assert server.request(INIT, headers={"Authorization": ""})[0] == 401
        assert server.request(method="DELETE", sid=observer_sid, headers=observer_header)[0] == 200
        assert server.request(method="DELETE", sid=operator_sid)[0] == 200
    finally:
        server.close()
    restricted = HttpServer(binary, token="test-operator", extra_args=["--read-only"])
    try:
        sid = restricted.initialize()
        restricted.request(READY, sid)
        status, _, body = rpc(restricted, sid, "session_create", {})
        assert status == 200 and body["error"]["code"] == -32001
    finally:
        restricted.close()


def wire_status(server, request):
    with socket.create_connection((server.host, server.port), timeout=5) as connection:
        connection.sendall(request)
        data = connection.recv(4096)
        return int(data.split(b" ", 2)[1])


def hostile_http(binary):
    server = HttpServer(binary)
    try:
        for value in [
            None, [],
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "result": {}},
            {"jsonrpc": "2.0", "id": 1.5, "method": "initialize"},
            {"jsonrpc": "2.0", "id": 1, "error": {"code": "bad", "message": "bad"}},
        ]:
            if value is None:
                wire = f"POST /mcp HTTP/1.1\r\nHost: {server.host}:{server.port}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: 4\r\n\r\nnull".encode()
                assert wire_status(server, wire) == 400
            else:
                status, _, body = server.request(value)
                assert status == 400 and body["error"]["code"] == -32600, (status, body)
        assert server.request(INIT, headers={"Origin": f"http://localhost:{server.port}"})[0] == 403
        assert server.request(INIT, headers={"Accept": "application/json;q=0, text/event-stream"})[0] == 406
        prefix = f"POST /mcp HTTP/1.1\r\nHost: {server.host}:{server.port}\r\n".encode()
        for suffix, expected in [
            (b"Content-Length: 0\r\nContent-Length: 0\r\n\r\n", 400),
            (b"Content-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", 400),
            (b"Content-Length: 1048577\r\n\r\n", 413),
            (b"X: " + b"x" * 16384 + b"\r\n\r\n", 431),
            (b"X: \x7f\r\nContent-Length: 0\r\n\r\n", 400),
            (b"Transfer-Encoding: chunked\r\n\r\n0\r\nAuthorization: Bearer forged\r\n\r\n", 400),
            (b"Transfer-Encoding: chunked\r\n\r\n" + b"x" * 1025, 400),
        ]:
            assert wire_status(server, prefix + suffix) == expected
    finally:
        server.close()
    # Wildcard binds must not disable the Host allowlist, even with a valid token.
    wildcard = HttpServer(binary, host="0.0.0.0", token="test-operator",
                          extra_args=["--allow-host", "trusted.example:8732"])
    try:
        assert wildcard.request(INIT, headers={"Host": "attacker.invalid:8732"})[0] == 403
        status, _, result = wildcard.request(INIT, headers={"Host": "trusted.example:8732",
                                                            "Origin": "http://trusted.example:8732"})
        assert status == 200 and "result" in result
    finally:
        wildcard.close()


def main(binary):
    broker_pid = None
    with tempfile.TemporaryDirectory(prefix="ztty-protocol-runtime-") as runtime:
        os.environ["XDG_RUNTIME_DIR"] = runtime
        os.environ["ZTTY_SOCKET"] = f"@ztty-security-{os.getpid()}"
        os.environ.pop("ZTTY_MCP_TOKEN", None)
        os.environ.pop("ZTTY_MCP_OBSERVER_TOKEN", None)
        operator = StdioClient(binary)
        uri = None
        try:
            created = operator.tool("session_create", {"name": "operator-owned", "command": ["/bin/sh"]})
            uri = created["uri"]
            broker_pid = created["broker_pid"]
            observer_stdio(binary, uri)
            observer_http(binary, uri)
            result = operator.tool("session_read", {"session": uri})
            assert result["running"] and result["cols"] == created["cols"] and result["rows"] == created["rows"]
            assert "PWNED" not in result["output"]
            assert len(operator.tool("session_list")["sessions"]) == 1
            hostile_http(binary)
            return {"stdio_observer": "passed", "http_observer": "passed",
                    "role_session_binding": "passed", "host_origin_allowlist": "passed",
                    "strict_jsonrpc": "passed", "hostile_framing_and_limits": "passed"}
        finally:
            if uri is not None:
                operator.tool("session_close", {"session": uri})
            operator.close()
            if broker_pid:
                # The daemon was created under our unique socket/runtime.
                try:
                    os.kill(broker_pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    args = parser.parse_args()
    print(json.dumps(main(os.path.abspath(args.binary)), indent=2))
