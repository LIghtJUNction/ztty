#!/usr/bin/env python3
"""Verify persistent PTYs, cursors and shared namespaces through real MCP calls."""
import argparse
import json
import os
import pathlib
import subprocess
import tempfile
import time

from mcp_transport import HttpServer, INIT, READY, line, packet, send

class StdioClient:
    def __init__(self, binary, cwd=None):
        self.proc = subprocess.Popen([binary, "mcp"], stdin=subprocess.PIPE,
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
        cursor = value["next_cursor"]
        output += value["output"]
        last = value
        if ((needle is None or needle in output)
                and (not exited or (not value["running"] and value["exit_code"] is not None))
                and not value["has_more"]):
            return output, cursor, value
        if not value["has_more"]:
            time.sleep(0.015)
    raise AssertionError({"wanted": needle, "output": output, "last": last})

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
