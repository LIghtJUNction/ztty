#!/usr/bin/env python3
"""Check lossless session_read bytes through actual stdio and HTTP MCP transports."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

from mcp_sessions import (StdioClient, http_tool, invalid_prefix_split_utf8,
                          invalid_utf8_page_boundaries, session_identity)
from mcp_transport import HttpServer, READY

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("artifacts/native-raw-output/acceptance.json"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    report = {"ok": False, "checks": [], "cleanup_errors": [],
              "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "source_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in (
                  "src/session.rs", "src/output_tail.rs", "tests/mcp_sessions.py", "tests/mcp_raw_output.py")}}
    stdio = http = None
    sessions = []
    with tempfile.TemporaryDirectory(prefix="ztty-native-raw-") as temporary:
        root = Path(temporary)
        os.environ["ZTTY_SOCKET"] = "@ztty-native-raw-" + root.name
        os.environ.pop("ZTTY_MCP_TOKEN", None)
        os.environ.pop("ZTTY_MCP_OBSERVER_TOKEN", None)
        try:
            stdio = StdioClient(str(binary), cwd=temporary)
            http = HttpServer(str(binary))
            sid = http.initialize()
            assert http.request(READY, sid)[0] == 202
            details = invalid_prefix_split_utf8(stdio, temporary, sessions)
            report["checks"].append({"name": "invalid prefix then split UTF-8 preserves exact bytes and waits for a complete character", "ok": True, "details": details})
            details = invalid_utf8_page_boundaries(stdio, http, sid, temporary, sessions)
            report["checks"].append({"name": "stdio and HTTP keep malformed bytes and complete UTF-8 across live and EOF pages", "ok": True, "details": details})

            def pages(read, transport, created, payload, maximum):
                cursor, result, cursors = 0, bytearray(), [0]
                while cursor < len(payload):
                    value = read("session_read", {"session": created["uri"], "cursor": cursor,
                        "max_bytes": maximum, "expected_session": session_identity(created)})
                    data = base64.b64decode(value["output_base64"], validate=True)
                    next_cursor = value["next_cursor"]
                    assert cursor < next_cursor <= cursor + maximum + 3, value
                    assert data == payload[cursor:next_cursor]
                    assert value["output"] == data.decode("utf-8", "replace")
                    assert not value["truncated"]
                    result.extend(data)
                    cursor = next_cursor
                    cursors.append(cursor)
                assert bytes(result) == payload
                end = read("session_read", {"session": created["uri"], "cursor": cursor, "wait_ms": 15})
                assert end["output_base64"] == end["output"] == "" and end["next_cursor"] == cursor
                assert end["timed_out"] is True and end["running"]
                return {"transport": transport, "max_bytes": maximum, "pages": len(cursors) - 1,
                        "final_cursor": cursor, "raw_sha256": hashlib.sha256(result).hexdigest()}

            child = r'''import os,pathlib,sys,time,tty
tty.setraw(0)
data=pathlib.Path(sys.argv[1]).read_bytes()
while data:
    count=os.write(1,data)
    data=data[count:]
pathlib.Path(sys.argv[2]).touch()
while True: time.sleep(.1)
'''
            for name, payload, limits in (
                ("every-byte-value", bytes(range(256)) + "中文🚀".encode() + b"\0tail", [1, 2, 7]),
                ("maximum-page-utf8-extension", b"x" * 65535 + "🚀END".encode(), [65536]),
            ):
                ready = root / (name + ".ready")
                payload_path = root / (name + ".bin")
                payload_path.write_bytes(payload)
                created = stdio.tool("session_create", {"name": name, "history": 0,
                    "command": ["python3", "-c", child, str(payload_path), str(ready)]})
                sessions.append(created["uri"])
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    value = stdio.tool("session_read", {"session": created["uri"], "max_bytes": 65536})
                    if ready.exists() and value["next_cursor"] >= min(len(payload), 65536):
                        break
                    time.sleep(.01)
                assert ready.exists(), name
                results = []
                for maximum in limits:
                    results.append(pages(stdio.tool, "stdio", created, payload, maximum))
                    results.append(pages(lambda tool, args: http_tool(http, sid, tool, args),
                                         "http", created, payload, maximum))
                stdio.tool("session_close", {"session": created["uri"]})
                sessions.remove(created["uri"])
                report["checks"].append({"name": name, "ok": True, "payload_bytes": len(payload), "reads": results})
            report["ok"] = True
        except Exception as error:
            import traceback
            report.update(error=str(error), traceback=traceback.format_exc())
        finally:
            if stdio is not None:
                for uri in sessions:
                    try:
                        stdio.tool("session_close", {"session": uri})
                    except Exception as error:
                        report["cleanup_errors"].append(str(error))
                try:
                    stdio.close()
                except Exception as error:
                    report["cleanup_errors"].append(str(error))
            if http is not None:
                try:
                    http.close()
                except Exception as error:
                    report["cleanup_errors"].append(str(error))
    report["ok"] &= not report["cleanup_errors"]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "checks": len(report["checks"]), "output": str(args.output)}))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
