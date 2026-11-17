# Changelog

## Unreleased

### Added

- Linux workspace tabs and horizontal/vertical split panes in the shared broker, with pane focus, swap, resizing, maximize/restore and moves between native windows.
- Stable session IDs, creation timestamps, filtered listing and rename that preserves existing session URIs.
- Explicit local detach, observe attach and input ownership with deliberate takeover.
- Hidden-pane release of rebuildable Xrender image uploads and retained X pixmaps.

### Verification

- Six layout tests and an isolated real X11/PTY acceptance script cover geometry, actual keyboard routing, hidden output, detach/reattach and ownership.
- Native macOS/Windows workspace UI and multi-view GUI observation remain pending.
