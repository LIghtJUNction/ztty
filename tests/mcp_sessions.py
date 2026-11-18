#!/usr/bin/env python3
"""Verify persistent PTYs, cursors and shared namespaces through real MCP calls."""
import argparse
import hashlib
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
    # Stop at an actual broker queue-full response, proving that bytes still
    # reside in its user-space queue while the child is deliberately not reading.
    for _ in range(32):
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
        raise AssertionError("could not establish bounded PTY input backpressure")
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
