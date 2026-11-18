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
