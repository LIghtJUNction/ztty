# Sessions and MCP

On Linux, ztty keeps terminal sessions in one process for the current user. Ordinary
launches, attached windows and MCP tools use that same process. Each session
owns its PTY, screen and history; windows on the same display reuse the display
connection and font caches. See [architecture](architecture.md) for ownership
and [performance](performance.md#multiple-window-memory) for measured memory.

The shared broker, attach command and MCP transports described here currently
require Linux. Native macOS and Windows terminal backends are implemented
separately; see the [platform table](../README.md#platforms-and-verification).

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

The first row applies to a local operator view. Closing a read-only view, or a
view whose input is owned by another actor, detaches it and keeps the PTY alive.
This also applies to the window manager's close action. An observing attach to
an existing workspace leaves its active tab and X input focus unchanged.

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
responses to stdout. Tool requests run concurrently; match each response by its
JSON-RPC ID because completion order can differ from submission order.
Diagnostic output uses stderr. A client configuration
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

The server validates Origin, Host headers on every bind address, content types and protocol
versions. It supports `2025-11-25`, `2025-06-18` and
`2025-03-26`, with negotiation following the
[MCP lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle).

## Tools

| Tool | Arguments | Result |
| --- | --- | --- |
| `session_create` | Optional `name`, `command`, `cwd`, `cols`, `rows`, `history` | URI, child/broker PIDs, dimensions and state |
| `session_list` | None | All local and MCP sessions |
| `session_read` | `session`; optional `cursor`, `max_bytes`, `expected_session`, `wait_ms` | Output page, screen snapshot, next cursor, `output_eof` and state; positive waits also return `timed_out` |
| `session_commands` | `session` | At most 128 OSC 133 records, exit status and availability |
| `session_command_read` | `session`, `command_id`; optional `part`, `max_bytes` | Rendered command input/output, `complete` and `truncated` |
| `session_write` | `session`, `data` | Queued input and session state |
| `session_resize` | `session`, `cols`, `rows` | Updated PTY and screen dimensions |
| `session_close` | `session` | Closed session metadata |
| `session_control` | `session`, `action`; optional `takeover` | Acquire/release input ownership; takeover is explicit |

### Observation and input ownership

Run `ztty mcp --read-only` to expose only the four session/command reading tools.
For an HTTP listener that accepts both roles, set `ZTTY_MCP_TOKEN` for operators
and a different `ZTTY_MCP_OBSERVER_TOKEN` for observers. When either is configured,
every request must authenticate; a transport session cannot switch roles by
changing its bearer token. Observers cannot create, resize, write, close or
acquire control, even if they manually submit a hidden tool name.

Each MCP connection receives an independent trusted actor identity. Creation
grants input ownership to that actor. A replacement connection can read retained
output immediately; it must call `session_control` with `action: "acquire"` and
`takeover: true` to replace a previous owner. A plain acquire refuses an existing
owner. The broker clears unsent input and cancels pending GUI paste transfers
when ownership changes. Bytes already delivered to the child cannot be recalled.
The actor identifier in metadata is not the HTTP transport-session credential.

Local GUI input resumes after Ctrl+Shift+G, or `ztty control NAME acquire --takeover`.
Input authority and visibility are separate: read-only viewing does not grant
typing, paste, mouse reports, focus reports or PTY resize. The child still receives
normal terminal protocol replies such as DSR while observed or detached.

`--allow-host name:port` adds an explicit HTTP authority, for example behind a
trusted reverse proxy. An Origin header must agree with the request Host. The
transport implements the 2025-11-25 JSON response lifecycle; it does not claim
conformance with newer MCP revisions or provide an unsolicited SSE subscription.

Command reads use the terminal's retained physical cells. They are bounded text
extraction, not a second raw stdout archive. A resized or overwritten region can
be unavailable, and completed commands can leave the history buffer. Check both
`complete` and `truncated`; see [daily workflow](daily-workflow.md).

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
Already available bytes beyond the requested page are checked before deciding
to wait: a malformed prefix followed by ASCII or another character advances
the cursor, even with a one-byte page. Only a genuinely incomplete suffix waits
for more output; EOF releases that suffix using replacement characters.

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
concurrent connections, with a 1 MiB message limit and a 4 MiB budget for
in-flight payloads and retained broker buffers. Allocator capacity, parsed JSON,
session state and operating-system buffers are separate memory costs. HTTP request
receiving and response sending each have their own ten-second deadline; a pending
output wait has a separate deadline. At most 32 HTTP connections may wait for
output, preserving slots for management requests. Each stdio process accepts
32 concurrent tool requests, including at most 24 positive output waits, and
bounds its aggregate queued data to 4 MiB. A stdio writer that makes no progress
for five seconds terminates the transport and releases its pending requests.

### Guarded resume and output termination

A canonical URI can be reused after its old session is closed. A byte offset
alone cannot distinguish that replacement from the previous output stream.
When resuming or reconnecting, send `expected_session` with exactly the four
identity fields from the original create/list/read response:

```json
{
  "session": "ztty://mcp/stdio/checks",
  "cursor": 4096,
  "expected_session": {
    "uri": "ztty://mcp/stdio/checks",
    "id": 7,
    "created_at": 1794988800000,
    "broker_pid": 12345
  }
}
```

These numbers are illustrative; copy the actual returned values. The guard is
checked before output or screen data is read. A different URI, session ID,
creation time or broker process returns a tool error. Renaming the display name
preserves the original URI and identity, so a guarded read can continue.
Missing, unknown, duplicate or incorrectly typed identity fields are rejected.
The identifiers prevent accidental stream confusion; transport authentication
and input ownership still determine authority.

Existing calls without `expected_session` remain supported and have no instance
guard. A reconnecting client should retain both its original identity and its
own `next_cursor`; independent observers do not consume each other's position.
After an identity mismatch, explicitly start a new read at zero only if the new
session is the intended one. Do not silently replace the saved identity.

`output_eof` means this endpoint's PTY reader has terminated, either at EOF or
after an unrecoverable read error, so no more bytes will be appended. A direct
child can already have exited while a descendant still holds the PTY and writes
later. Therefore `running: false` and an `exit_code` do not prove that output has
finished. Use `output_eof && !has_more` to identify a drained retained stream,
and still check `truncated` for expired bytes. This condition does not guarantee
a successful child exit or lossless transport.

### Waiting for output

Set `wait_ms` to a value from 1 to 30,000 to park an empty read until output
becomes available. Omitting it, or passing zero, keeps immediate-read behavior.
The same byte cursor, page size, observer permissions and optional identity
guard apply in either mode:

```json
{
  "session": "ztty://mcp/stdio/checks",
  "cursor": 4096,
  "max_bytes": 16384,
  "wait_ms": 30000,
  "expected_session": {
    "uri": "ztty://mcp/stdio/checks",
    "id": 7,
    "created_at": 1794988800000,
    "broker_pid": 12345
  }
}
```

A positive wait completes when it can deliver bytes, report an expired cursor,
or report `output_eof`. These responses have `timed_out: false`. At the deadline,
an otherwise empty live stream returns `timed_out: true` with its current screen
and unchanged cursor. An incomplete UTF-8 suffix stays pending until it becomes
decodable, the reader terminates, or the deadline expires. Direct-child exit
alone does not end a wait while a descendant still owns the PTY.

Every parked read binds to the PTY instance selected when the request was
accepted. Closing that PTY produces a tool error, even if a new terminal reuses
its name before the reply is sent. A rename preserves the instance and URI.
Keep sending `expected_session` when issuing later requests or reconnecting so
the identity check also covers the interval between requests.

The broker, stdio transport and HTTP listener drive these reads through their
existing poll loops. Other clients can list, write, resize and close sessions
while a read waits. A parked broker read retains only its session ID, cursor,
page limit and deadline; its screen response is built on completion. The broker
caps combined window and output waiters at 128 while retaining management
connection capacity. HTTP continues to use JSON request/response mode; SSE and
unsolicited output subscriptions are separate remaining work.

### Cancelling a pending request

Send the MCP cancellation notification using the original request ID:

```json
{
  "jsonrpc": "2.0",
  "method": "notifications/cancelled",
  "params": {"requestId": 42}
}
```

String IDs match their decoded characters exactly. Numeric IDs match their
exact integer value, so `42`, `42.0` and `4.2e1` identify the same request,
and `-0` matches `0`. Strings and numbers stay separate: `"42"` is not `42`.
Matching never rounds large integers through floating point; replies preserve
the original request's ID token. Both transports reject a second request whose
ID matches a currently pending request. Clients must still issue a new ID for
every request within an MCP session, as required by the
[base protocol](https://modelcontextprotocol.io/specification/2025-11-25/basic/index).

Cancellation drops that request's broker connection and produces no result for
the cancelled request. Unknown, malformed and already completed cancellations
are ignored. On HTTP, cancellation is restricted to the same authenticated MCP
transport session; another session using the same request ID has no effect.
Disconnecting its connection or deleting the HTTP session also releases pending
reads. Keep the input direction open while a positive wait is pending: input
EOF cancels it, including HTTP `shutdown(SHUT_WR)`. Accepted ordinary requests
still complete after input EOF while the response direction remains available.
These operations leave terminal processes alive. Bytes already sent
by a write operation remain subject to the existing input-ownership rules and
cannot be recalled. Clients should ignore responses that raced with a
[cancellation](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/cancellation).

## Verify

```sh
python3 tests/mcp_transport.py --binary ./target/release/ztty
python3 tests/mcp_sessions.py --binary ./target/release/ztty
python3 tests/mcp_security.py --binary ./target/release/ztty
python3 tests/mcp_wait.py --binary ./target/release/ztty --output artifacts/mcp-wait/report.json
python3 tests/daily_workflow.py --binary ./target/release/ztty
python3 tests/shared_gui_smoke.py --binary ./target/release/ztty
```

The MCP tests cover transport lifecycle, IPv6, concurrent requests, real PTYs,
UTF-8 cursor boundaries, guarded reconnection across transports, same-name session
replacement, independent observers, delayed descendant output, creator environment
isolation, namespaced addressing, exit status and persistence. The GUI test covers shared-window behavior on
a private Xvfb display. These shared-session tests exercise Linux X11/XWayland;
native macOS and Windows frontends have their own acceptance records.

## Split output and byte cursors

Output reads preserve an incomplete UTF-8 suffix until its remaining bytes or EOF
arrive, including when earlier bytes are malformed. A returned cursor counts
raw bytes. A nonadvancing cursor with has_more=true can mean an incomplete
character, so wait before retrying it. Independent observers do not consume one
another's cursor. See [the bounded output contract and real PTY regression](session-core.md#bounded-output-and-independent-byte-cursors).
