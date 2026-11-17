# Changelog

## Unreleased

### Added

- Three-run guarded 1/10-window benchmark evidence and README charts, including measured RSS/PSS/Swap, source hashes, the unmet 10 MiB RSS target and the PTY throughput gap.
- Linux standalone `--observe` command windows and the `ztty-observer` external-terminal adapter; `close NAME` follows existing input ownership authorization.
- Linux x86_64/ARM64 release gates now require four real X11/PTY suites with source, executable and display ownership evidence; Windows ARM64 must execute natively.
- Versioned Linux workspace snapshots restore live tabs, split ratios, focus and geometry without replacing PTYs; bounded schema/identity checks reject invalid layouts before UI mutation.

- Linux workspace tabs and horizontal/vertical split panes in the shared broker, with pane focus, swap, resizing, maximize/restore and moves between native windows.
- Stable session IDs, creation timestamps, filtered listing and rename that preserves existing session URIs.
- Explicit local detach, observe attach and input ownership with deliberate takeover.
- Hidden-pane release of rebuildable Xrender image uploads and retained X pixmaps.
- Linux UTF-8 history search with visible match highlighting, bounded query storage and forward/backward navigation.
- Temporary command palette, named key/mouse actions, live theme/font/binding reload and font-size shortcuts.
- Bounded OSC 133 command records, command navigation/copy, CLI output and observer-safe MCP command reads.
- Opt-in Bash, Zsh, Fish and Nushell integration scripts and the `shell-integration` command.
- Read-only MCP roles, separate observer/operator tokens, exclusive trusted actor identities and stricter HTTP/JSON framing.
- Reproducible independent/shared 1/5/10/20 benchmark tooling, workload checks, retained raw measurements and SVG generation.

### Fixed

- Custom benchmark charts now link their actual raw-data path; renderer-only updates preserve collection identity and historical evidence.
- Closing a newly launched observer retires its own viewer child, allowing the next ssmcp task to reopen visibly; attaching to an existing session still preserves that session on close.
- Config reload reconnects without losing a directory created before its ancestor watch is installed; deterministic real inotify regressions prove the old failure and the repaired behavior.
- Native Linux evidence is archived and read back before staging cleanup, preserving the flat Release checksum contract; incomplete gates cannot produce accepted artifacts.
- Benchmark reports now pin the isolated Xvfb helper hash and preserve collection-time identity when regenerating charts.
- Linux input preserves horizontal wheels and extra mouse buttons 8–11; configured bindings support all eleven X11 buttons.
- Mouse releases follow the destination of their original press across Shift changes; focus and ownership transitions clear held input without cancelling a valid focus-loss clipboard transfer.
- Workspace restore retains live parent/child windows and retries PTY dimensions after resize errors; stale queued focus/geometry events no longer overwrite the restored layout.
- Saved layouts reject a replacement session that reuses a closed session's canonical URI.

- Closing an observing view preserves another operator's live PTY; observing an existing workspace does not steal focus.
- Ownership handoff cancels pending input and asynchronous clipboard transfers.
- Command navigation skips regions invalidated by a screen-width change.
- Config watches follow atomic file saves and replacement of the containing directory, and release unused watches.
- Runtime theme updates distinguish default, indexed and explicit RGB colors without enlarging the 28-byte cell.
- Large JSON object duplicate-key validation avoids quadratic work.

### Verification

- Final local gates pass 170 Rust unit/integration/documentation tests, strict Clippy, MCP transport/session/security tests and bare-metal checks; the zune-png bare-metal SIMD future-compatibility warning is retained in the evidence.
- Real ssmcp relay integration passes three native commands and ten checks, including window reuse and reopening; daily Linux GUI coverage now has fourteen checks.
- Windows native gates now exercise six Sixel pixel/recovery groups and save owned-window PNG evidence; execution on x64 and ARM64 is still required.
- Twenty-seven real X11/PTY vectors cover legacy keyboard bytes, four wheel directions, extra buttons, press ownership, focus transitions, X10 mode and custom bindings. Button 11 uses explicitly recorded synthetic X11 events because the virtual pointer exposes ten buttons.
- Six layout tests and an isolated real X11/PTY acceptance script cover geometry, actual keyboard routing, hidden output, detach/reattach and ownership.
- Native macOS/Windows workspace UI and multi-view GUI observation remain pending.
- Native macOS/Windows search, command palette and shared MCP runtime are not yet implemented.
