# Portable session state: input ownership

The Linux broker now uses `SessionControl` for the same ownership decisions that
native frontends will need. The module imports only `alloc::string::String`; it
has no transport, window, PTY or operating-system calls. This extraction does not
implement the shared macOS or Windows runtime.

## State and effects

An actor is a bounded ASCII identity supplied by the trusted request boundary.
It is not an operating-system login or a replacement for HTTP authentication.

| Request | State result | Existing broker effect |
| --- | --- | --- |
| Same owner acquires again | No ownership change | Preserve pending input; a local view leaves explicit observation mode |
| Another actor acquires without takeover | Reject | Preserve state and pending input |
| Another actor explicitly takes over | New owner | Clear queued input and cancel pending paste and held input state |
| Current owner releases | No owner | Clear queued input and cancel pending input state |
| A different actor releases | Reject | Preserve state |

The external `input_owner` JSON field and existing errors remain unchanged.
The frontend applies queue cancellation and read-only view updates after the
state transition. Keeping those effects outside the state module avoids giving
a portable type implicit access to windows or transport connections.

## Verification

Nine state tests cover identity boundaries, ownership queries, same-owner
idempotency, explicit takeover, release and reacquisition. They include the
production module directly and contain no platform-specific calls.

A real MCP test first fills the live PTY input path until the broker returns
`session input queue is full`. It reacquires the same session with both takeover
values and then allows the child to read. The child receives all **1,048,576
bytes**, with SHA-256
`aca1cd027e979588d14b877b7b0cb8585ad9fec599eb45801992ee5382b3760f`.
This verifies that a state-idempotent request cannot silently discard input
already accepted by the broker.

The daily X11 test now includes a fifteenth check: an existing local owner
attaches as an observer, reacquires control and sends real XTEST keyboard input
to the same PTY PID. All previous fourteen checks remain.

The integrated source, based on `05782fc23b23306d61932ad39777407802886548`, passed:

```sh
CARGO_BUILD_JOBS=1 ZTTY_BARE_METAL=1 sh tools/check.sh
python3 tests/daily_workflow.py --binary target/release/ztty \
  --output artifacts/session-control-integrated/daily15.json \
  --screenshots artifacts/session-control-integrated/screenshots
```

The full gate passed **183 Rust tests**, Clippy with warnings denied, release
build, Rust `no_std` checks and the real MCP transport, session and permission
tests. The optional bare-metal library check passed with the already recorded
`zune-png 0.5.2` future-compatibility warning.

[Integrated source and binary hashes](measurements/session-control-2026-11-18.json),
[full gate log](measurements/session-control-2026-11-18.log) and
[15-check GUI evidence](measurements/session-control-daily-2026-11-18.json)
retain the actual execution scope. Native macOS/Windows ownership, shared PTYs
and tabs still require their runtime integration and native acceptance.

## Bounded output and independent byte cursors

`OutputTail` now owns the retained raw output used by the Linux broker.
It imports only `core + alloc`. Screen snapshots stay in `Session`, where
the current terminal cells are available. This is a shared state component;
native macOS/Windows session APIs are still not implemented.

Ordinary sessions retain up to 64 KiB, persistent sessions up to 256 KiB.
A read does not modify shared state. Each observer can keep a byte cursor,
disconnect and resume independently. A cursor older than the retained window
gets `truncated=true`; increasing retention cannot recover discarded bytes.
The read limit defaults to 16,384, accepts 1–65,536 and may extend by at most
three bytes to finish a UTF-8 character. Cursors count original bytes, not
characters in the lossy text representation.

### Confirmed split UTF-8 repair

The old production binary was tested with a real PTY child that first wrote
`FF F0 9F`, then waited for an explicit file gate before writing more bytes.
The initial illegal `FF` hid the incomplete four-byte character at the end
from a check that inspected only the first UTF-8 error.

| Stage | Old behavior | Corrected behavior |
| --- | --- | --- |
| Only `FF F0 9F` has arrived | Two replacement characters; cursor 3 | One replacement character; cursor 1 |
| `9A 80 END` arrives later | The character prefix has already been consumed | Reconstructs `�🚀END`; cursor 8 |

The correction scans past already determined invalid sequences and retains
the still incomplete suffix. The scan and allocation remain bounded by the
requested page size. At EOF, incomplete bytes are consumed using the existing
lossy conversion. While a suffix is waiting for more bytes, `has_more` can be
true even when the returned text is empty; clients should wait before retrying
a cursor that made no progress.

Seventeen new regressions cover independent replay and eviction, bytewise
four-byte characters with a one-byte page limit, multiple malformed
sequences, EOF, invalid read limits, retention changes and storage bounds.
Empty output does not preallocate a retained buffer.

The integrated full gate passed **200 Rust tests**, Clippy, release, no_std,
the optional bare-metal component check, all real MCP tests and daily GUI
**15/15**. The existing zune-png bare-metal future warning remains. The same
gated MCP regression failed on the old binary and passed after integration.

- [Integrated hashes and commands](measurements/output-tail-2026-11-18.json)
- [Full integrated gate log](measurements/output-tail-2026-11-18.log)
- [Original real-MCP failure](measurements/output-tail-before-fix-2026-11-18.log)
- [Original reproduction identity](measurements/output-tail-origin-2026-11-18.json)
- [Integrated daily GUI evidence](measurements/output-tail-daily-2026-11-18.json)


## Shared PTY pump and bounded input

The next extracted state is `SessionCore`: one real `zpty::Pty`, its
`zterm::Terminal`, bounded pending input, ownership, retained output and EOF.
The Linux broker now consumes this implementation. A read feeds exactly one
block and returns history lengths before and after that block; the frontend
keeps its own work budget and scroll anchor. Child exit and output EOF remain
separate so bytes already buffered in the PTY can drain after reaping.

Input writes are accepted in full or rejected unchanged. Partial OS writes
retain the unwritten suffix. Revoking ownership clears only bytes not yet
accepted by `Pty::write`; it cannot retract bytes already queued in the
Windows writer thread. Reply generation uses the same bounded queue and
clears rejected replies, preserving the existing overflow policy.

A successful resize now orders the operations as PTY size, cell metrics when
needed, then terminal grid. A native PTY error leaves the terminal state
unchanged, allowing the frontend to retry. EOF-only screen resize remains
an explicit frontend policy.

[Source hashes and commands](measurements/session-core-2026-11-18.json),
[full gate](measurements/session-core-2026-11-18.log),
[daily GUI](measurements/session-core-daily-2026-11-18.json), and
[workspace GUI](measurements/session-core-workspaces-2026-11-18.json)
record the integrated Linux candidate. Eight single-threaded real-PTY checks
cover bounded input/ownership, reply overflow, UTF-8/EOF, history reset,
kernel size readback, draining after reap, fatal read/write errors and failed
resize. The dedicated test executable forks before any test worker threads;
it restores owned descriptors and child/fd counts. The regular Rust tests,
strict Clippy, release/no_std audit and MCP gates pass. Bare-metal portable
library checking also passes with the previously recorded zune-png warning.

This step does not yet provide shared runtime, attach or MCP on macOS and
Windows. Their frontend integration and actual native acceptance follow.

## Native frontend integration

The application package now also exposes a real `no_std` library target.
`input_queue`, `output_tail`, `session_control` and `session_core` are
public modules; the three frontends consume the same implementation.
This introduces no dependency, platform trait or dispatcher. The integration
tests import that library directly instead of compiling source copies.
The library needs alloc and zpty's OS ABI; it is not a bare-metal PTY library.

Windows and macOS now hold `SessionCore` in place of separate terminal,
PTY and pending-write fields. Their existing 8 MiB outer input limits remain;
Linux keeps 1 MiB. Windows retains its sixteen-read event budget and existing
ConPTY exit policy. macOS retains its 1 MiB event budget and CFFileDescriptor
read/write callbacks. Window surfaces, fonts, selection, cursor timing and
event-loop ownership remain platform concerns.

Both native resize paths commit PTY size before physical cell metrics and the
terminal grid. macOS backing scale and Windows candidate DPI fonts are
committed only after that operation succeeds. On failure they retain the old grid, emit a diagnostic and
request a repaint; subsequent resize events can retry. The Windows frontend
still exits on its existing child-reap condition: changing this to wait
unconditionally for pipe EOF would require separately proving ConPTY close
and output-drain ordering.

The [integration record](measurements/native-core-port-2026-11-18.json)
contains source and binary hashes. Linux passes 200 regular Rust tests,
eight real-PTY core checks, strict Clippy, release/no_std and MCP checks,
plus the fifteen-check daily GUI suite. Windows GNU and both Darwin targets
pass `cargo check`. The fifth native candidate at `7a1d06b` now accepts the
Windows integration on native x64 and ARM64 with all 30 checks and full
cleanup. The macOS helper still needs a successful native compilation and
pixel run; [candidate evidence](native-candidates-2026-11-18.md) records that
remaining boundary.

The shared core is groundwork for native shared runtime, tabs/splits,
attach and MCP. Those features are not implemented by this extraction.


## Shared workspace layout and action modules

The no_std library now also exports `actions`, `json`, `layout` and
`workspace_state`. Linux and the layout tests consume these same library
modules. The four algorithms are unchanged: 32 tabs, 64 panes, workspace
schema 2, bounded parsing and stable pane identities retain their existing
behavior. Native frontends can now use this layout implementation without
compiling private source copies or adding another dependency.

Source `ba7e82e5678e8a5e7cc5735fac76b242bebc492d` passes 202 unit and integration
tests, six doctests and the eight single-threaded real-PTY checks. Strict
Clippy, release/no_std, live MCP transport/session/security and the optional
bare-metal component gate pass; the known zune-png warning remains. The
actual X11 workspace suite passes all ten checks, including retained PTYs,
hidden output and failed-resize recovery. This export does not by itself
add native tabs, shared windows or attach.

[Integrated record](measurements/portable-layout-2026-11-18.json),
[full gate log](measurements/portable-layout-2026-11-18.log) and
[workspace GUI evidence](measurements/portable-layout-workspaces-2026-11-18.json)
record source, binary and report hashes.
