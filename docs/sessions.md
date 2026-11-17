# Sessions and MCP

ztty keeps terminal sessions in one process for the current user. Ordinary
launches, attached windows and MCP tools use that same process. Each session
owns its PTY, screen and history; windows on the same display reuse the display
connection and font caches. See [architecture](architecture.md) for ownership
and [performance](performance.md#multiple-window-memory) for measured memory.

## Open, list and attach

Run a named terminal from an existing shell:

```sh
ztty --name work -e /bin/bash &
ztty --name logs -e /bin/sh &
ztty attach
ztty attach work
```

Each ordinary launch opens its window and waits until that window closes or
its command finishes. The `&` in the example leaves the launching shell
available. The waiting launcher is small and is included in the multi-window
memory measurements.

`ztty attach` without an argument prints the session list. With a name or
complete address, it opens the session's window, or raises the existing window,
and waits for that window to close. There is one visible window per session.
Attaching a completed MCP session lets you inspect its retained screen.

Names use 1–64 ASCII letters, digits, dots, underscores or hyphens; `.`
and `..` are reserved. Omitting a name assigns `session-N`.
A complete address is unique within the local broker. The same short name may
exist in different namespaces; an ambiguous short name produces an error and
requires its complete address.

| Created through | Example address |
| --- | --- |
| Ordinary terminal | `ztty://local/work` |
| MCP stdio | `ztty://mcp/stdio/work` |
| MCP HTTP | `ztty://mcp/http/127.0.0.1:8732/work` |
| MCP HTTP on IPv6 | `ztty://mcp/http/[::1]:8732/work` |

```sh
ztty attach ztty://mcp/stdio/work
ztty attach ztty://mcp/http/127.0.0.1:8732/work
```

These addresses identify sessions in the local broker. The HTTP portion records
which MCP listener created the session; attaching the address does not connect
to a remote machine.

## Session lifetime

| Event | Ordinary local session | MCP session |
| --- | --- | --- |
| Window closes | Hang up the child and remove the session | Detach the window; keep the PTY and history |
| Child exits | Close the window and remove the session | Retain output, screen and exit status for inspection |
| MCP transport exits or HTTP transport session is deleted | Unaffected | Keep the terminal session |
| `session_close` tool | Close the session and release its resources | Close the session and release its resources |

Detaching removes that session's window resources. The broker drops unreferenced
font sets after their last window closes, but keeps opened X11 connections until
it exits. It exits after approximately
three seconds with no remaining sessions, pending clients or children to reap.
Completed MCP sessions count as retained sessions until explicitly closed.
The broker owns the PTYs, so stopping that process closes its sessions.

Each creator supplies its own command arguments, working directory and
environment. A later launch uses its caller's environment even when the broker
was started by another terminal or an MCP server.

## MCP over stdio

```sh
ztty mcp
# Equivalent:
ztty mcp --transport stdio
```

The server reads one JSON-RPC message per input line and writes protocol
responses to stdout. Diagnostic output uses stderr. A client configuration
using the common `mcpServers` format can launch it with:

```json
{
  "mcpServers": {
    "ztty": {
      "command": "ztty",
      "args": ["mcp"]
    }
  }
}
```

MCP starts without opening a desktop window. Use `session_create` to
start a PTY, then `ztty attach` to view it.

## MCP over Streamable HTTP

```sh
ztty mcp --transport http --host 127.0.0.1 --port 8732
```

Connect an MCP client to `http://127.0.0.1:8732/mcp`. The defaults are
loopback and port 8732. IPv4, IPv6 and hostnames are supported; port `0`
selects an available port and reports the actual URL on stderr.
`--http` is shorthand for `--transport http`;
`streamable-http` is also accepted as the transport name.

A listener outside loopback requires a bearer token. Prefer the environment
variable so the token is absent from the process argument list:

```sh
ZTTY_MCP_TOKEN='replace-with-a-long-random-token' \
  ztty mcp --transport http --host 0.0.0.0 --port 8732
```

Clients send `Authorization: Bearer <token>`. `--token TOKEN`
is also supported. The listener implements plain HTTP; use a TLS terminator or
secure tunnel across an untrusted network.

The implementation uses the JSON response form of
[Streamable HTTP](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).
POST requests return JSON, accepted notifications return HTTP 202, and GET
returns HTTP 405 because there is no unsolicited SSE stream. Initialization
returns an `Mcp-Session-Id`; subsequent requests include that ID and
the negotiated `Mcp-Protocol-Version`. DELETE removes that HTTP
transport session while leaving its terminal sessions available. Idle HTTP
transport sessions expire after 30 minutes; retained terminals remain available.

The server validates Origin, local Host headers, content types and protocol
versions. It supports `2025-11-25`, `2025-06-18` and
`2025-03-26`, with negotiation following the
[MCP lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle).

## Tools

| Tool | Arguments | Result |
| --- | --- | --- |
| `session_create` | Optional `name`, `command`, `cwd`, `cols`, `rows`, `history` | URI, child/broker PIDs, dimensions and state |
| `session_list` | None | All local and MCP sessions |
| `session_read` | `session`; optional `cursor`, `max_bytes` | Output page, screen snapshot, next cursor and state |
| `session_write` | `session`, `data` | Queued input and session state |
| `session_resize` | `session`, `cols`, `rows` | Updated PTY and screen dimensions |
| `session_close` | `session` | Closed session metadata |

`command` is an executable-and-arguments array. Use an explicit shell
when shell expansion, pipelines or redirection are needed. With no command,
the server uses its inherited `SHELL`, falling back to `/bin/sh`.
The default grid is 120×32 with 2,000 history rows; dimensions are limited to
2–512 columns and 2–256 rows.

For example, after initialization:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "session_create",
    "arguments": {
      "name": "checks",
      "command": ["/bin/sh", "-lc", "cargo test"],
      "cwd": "/path/to/project"
    }
  }
}
```

Follow the returned URI with `session_read`, or open it with
`ztty attach ztty://mcp/stdio/checks`. Send a newline in
`session_write.data` to submit a command to an interactive shell;
`\u0003` sends Ctrl+C. A write accepts at most 65,536 UTF-8 bytes.

### Output cursors and memory bounds

`session_read.cursor` is a raw byte offset; start at zero and pass the
returned `next_cursor` to the next call. `cursor` in the response
is an alias for that next offset. `max_bytes` accepts 1–65,536.
A text page may extend by up to three bytes to finish a UTF-8 character.
An incomplete trailing character is held until more PTY output arrives.

MCP sessions retain a rolling 256 KiB of raw output, separate from screen
history. When a cursor precedes the retained range, `truncated` is true
and `first_cursor` identifies the retained range's start.
`has_more` reports additional available output. The screen snapshot
and `screen_truncated` flag provide a separate view of the current
terminal contents. Actual invalid byte sequences use replacement characters
in the text representation.

Screen history grows on demand and has its own 8 MiB cell-storage cap. The
broker allows 128 terminal sessions; close completed sessions to release
their retained state. Each HTTP server allows 64 transport sessions and 64
concurrent connections, with a 1 MiB message limit, a 4 MiB aggregate incoming
buffer limit and a ten-second request deadline.

## Verify

```sh
python3 tests/mcp_transport.py --binary ./target/release/ztty
python3 tests/mcp_sessions.py --binary ./target/release/ztty
python3 tests/shared_gui_smoke.py --binary ./target/release/ztty
```

The MCP tests cover transport lifecycle, IPv6, concurrent requests, real PTYs,
UTF-8 cursor boundaries, creator environment isolation, namespaced addressing,
exit status and persistence. The GUI test covers shared-window behavior on
a private Xvfb display. The current backend is Linux X11/XWayland.
