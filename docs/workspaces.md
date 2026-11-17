# Tabs, splits and input ownership

Linux sessions can share a native workspace window while keeping separate PTYs, terminal screens and histories. Tabs and splits reuse the existing broker, display connection and font cache. The layout tree contains session IDs; moving a pane never starts a replacement process.

## Open and arrange

```sh
ztty --name work -e /bin/bash
ztty --name build --split-right work -e /bin/bash
ztty --name logs --split-down build -e /bin/bash
ztty --name notes --tab work -e /bin/bash
ztty action work next-tab
ztty action build maximize
ztty action build move-window
ztty move build work right
```

The launch commands wait for their view to close; use separate shells or append `&` when issuing them from one shell. `--cwd PATH` sets a new session's working directory. Keyboard-created tabs and panes inherit the caller's shell environment and the active session's latest local OSC 7 directory, falling back to its initial directory. Remote OSC 7 hostnames do not become local directories.

| Shortcut | Action |
| --- | --- |
| Ctrl+Shift+T | New tab |
| Ctrl+Shift+E / O | Split right / below |
| Ctrl+Shift+PageUp / PageDown | Previous / next tab |
| Ctrl+Shift+Left / Right | Previous / next pane |
| Ctrl+Shift+Up / Down | Swap with previous / next pane |
| Ctrl+Shift+Enter | Maximize / restore the focused pane |
| Ctrl+Shift+Plus / Minus | Grow / shrink the nearest split |
| Ctrl+Shift+N / U | New window / move current pane to its own window |
| Ctrl+Shift+D / W | Detach / close the current view |
| Ctrl+Shift+G | Explicitly take input control |

The native window title shows the active tab number. There is no permanent toolbar. Hidden tabs keep reading PTY output; their Xrender image uploads and full-size backing pixmaps are released and rebuilt when shown. A workspace is bounded to 32 tabs and 64 panes; the broker retains its separate 128-session limit.

## Session lifecycle and control

```sh
ztty list work
ztty info work
ztty rename work editing
ztty detach editing
ztty attach --observe editing
ztty control editing acquire --takeover
ztty control editing release
```

Session metadata includes a numeric ID, creation time in Unix milliseconds, input owner, PTY PID, window ID, workspace ID and visibility. Rename changes the short display name while preserving the creation URI. Detached sessions retain their PTY and output; their previous launch/attach waiters complete. The broker's IDs and live PTYs last only as long as the broker process.

Each trusted control identity owns input explicitly. Reads are available to observers; write, resize, rename and close require the current owner. Taking over another owner requires `takeover: true`; release and takeover discard unsent input from the previous owner. Already delivered PTY bytes cannot be recalled. A lost controller retains its ownership until an authorized operator explicitly takes over.

An observing view permits selection, copy and scrollback while suppressing keyboard input, paste, mouse reports, focus reports and PTY resize. Ctrl+Shift+G explicitly takes local control. Observer creation requests EWMH user-time zero so a cooperating window manager can map it without activation. Window-manager-specific focus behavior requires the native-desktop check.

## Reproduce the acceptance

```sh
cargo test --locked --test layout
cargo build --locked --release
python3 tests/workspaces.py --binary ./target/release/ztty
```

The six layout tests cover geometry, tab/pane removal, focus, swap, resize, maximize, duplicate IDs and limits. The real X11 test starts a private Xvfb display and verifies PTY separation, native geometry, keyboard routing, hidden output, moving panes, rename, detach/reattach, observer input denial, explicit takeover and cleanup. It saves an actual screenshot and a JSON result.

[Acceptance result](measurements/workspaces-x11-2026-11-18.json) · [Screenshot](assets/workspaces-x11-2026-11-18.png)

![Two real ztty split panes on X11](assets/workspaces-x11-2026-11-18.png)

This group is verified on Linux X11. macOS and Windows continue to use their existing single-window backends; native tabs/splits on those platforms, workspace persistence and multiple simultaneous GUI views of one PTY remain separate work.
