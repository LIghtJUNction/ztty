# Changelog

## Unreleased

### Added

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

- Closing an observing view preserves another operator's live PTY; observing an existing workspace does not steal focus.
- Ownership handoff cancels pending input and asynchronous clipboard transfers.
- Command navigation skips regions invalidated by a screen-width change.
- Config watches follow atomic file saves and replacement of the containing directory, and release unused watches.
- Runtime theme updates distinguish default, indexed and explicit RGB colors without enlarging the 28-byte cell.
- Large JSON object duplicate-key validation avoids quadratic work.

### Verification

- Six layout tests and an isolated real X11/PTY acceptance script cover geometry, actual keyboard routing, hidden output, detach/reattach and ownership.
- Native macOS/Windows workspace UI and multi-view GUI observation remain pending.
- Native macOS/Windows search, command palette and shared MCP runtime are not yet implemented.
