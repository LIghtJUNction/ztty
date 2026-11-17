# Changelog

## Unreleased

### Added

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

- Twenty-seven real X11/PTY vectors cover legacy keyboard bytes, four wheel directions, extra buttons, press ownership, focus transitions, X10 mode and custom bindings. Button 11 uses explicitly recorded synthetic X11 events because the virtual pointer exposes ten buttons.
- Six layout tests and an isolated real X11/PTY acceptance script cover geometry, actual keyboard routing, hidden output, detach/reattach and ownership.
- Native macOS/Windows workspace UI and multi-view GUI observation remain pending.
- Native macOS/Windows search, command palette and shared MCP runtime are not yet implemented.
