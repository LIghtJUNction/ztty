# ztty

A lean terminal for Linux, written in Rust without the Rust standard library.

ztty runs real shells and terminal applications through a Linux PTY. Its screen
engine and input protocols are independent libraries; the desktop frontend uses
the system X11, Xft, and Xrender libraries. The visual design is deliberately quiet: a dark
background, readable text, generous padding, and no application toolbar.

Multiple windows automatically share one session process, display connection
and font cache. Each window keeps its own PTY, screen and history. In the
recorded eight-window comparison, total ztty PSS fell from 47.021 to 15.366 MiB,
including the waiting launchers. The single-window case rose by 0.539 MiB.
See [measurement conditions and graphics-memory accounting](docs/performance.md#multiple-window-memory).

[中文介绍](README.zh-CN.md)

![Actual ztty protocol preview](docs/assets/ztty-preview.png)

## Install

The examples below describe the current source tree. Release downloads follow
the latest published tag; build from source for unreleased changes.

On Arch Linux, install the AUR package with `yay -S ztty`. Other Linux
distributions can install a prebuilt x86_64 or AArch64 binary with:

```sh
curl -fsSLO https://github.com/LIghtJUNction/ztty/releases/latest/download/install.sh
sh install.sh
```

The script verifies the archive and installs to `~/.local/bin`. The binary
requires glibc, X11, Xft, and Xrender. Native macOS and Windows builds are planned.

## Build and run

Requirements: Linux, Rust 1.100.0, a C linker, X11/Xft/Xrender development libraries, and a
monospace font. The X11 frontend can also run through XWayland on Wayland desktops.
Automated GUI acceptance uses Xvfb/X11 on x86_64.

```sh
# Arch Linux
sudo pacman -S --needed base-devel libx11 libxft libxrender fontconfig

# Debian / Ubuntu
sudo apt install build-essential libx11-dev libxft-dev libxrender-dev libfontconfig1-dev

cargo build --locked --release
./target/release/ztty
./target/release/ztty --font 'monospace:size=12' --geometry 100x28
./target/release/ztty --scrollback 4000 -e /bin/bash
```

`make install` installs the executable and desktop entry under `~/.local`.
Set `PREFIX` to choose another location. The default shell comes from `SHELL`,
with `/bin/sh` as the fallback. Child sessions receive `TERM=xterm-256color`,
`COLORTERM=truecolor`, and `TERM_PROGRAM=ztty`.

## Shared sessions and MCP

Run several terminals normally; resource sharing is automatic:

```sh
ztty --name work -e /bin/bash &
ztty --name logs -e /bin/sh &
ztty attach
ztty attach work
```

`ztty attach` lists sessions when no name is given. A name opens or raises its
window and waits for it to close. Local sessions have addresses such as
`ztty://local/work`; an ambiguous short name requires the complete address.

```sh
# MCP over stdio, without opening a window
ztty mcp

# Streamable HTTP at http://127.0.0.1:8732/mcp
ztty mcp --transport http --host 127.0.0.1 --port 8732

# View a session created by an MCP client
ztty attach ztty://mcp/stdio/checks
ztty attach ztty://mcp/http/127.0.0.1:8732/checks
```

MCP exposes six tools to create, list, read, write, resize and close PTYs.
MCP-created sessions keep their output when the transport exits; closing their
window detaches it. Ordinary local windows keep the usual close-to-exit
behavior. HTTP listeners outside loopback require a bearer token.

See [sessions and MCP](docs/sessions.md) for naming, lifecycle, HTTP tokens,
client setup and bounded UTF-8 output cursors.

## Appearance

Edit `$XDG_CONFIG_HOME/ztty/ztty.conf` (default `~/.config/ztty/ztty.conf`),
or choose a file with `ztty --config PATH`. Settings take effect on the next
launch. Omitted keys use the built-in defaults and `--font` overrides the file.
Use UTF-8, one `key = value` per line, and whole-line comments starting with `#`.

```ini
font = monospace:size=11
window-padding = 16
background = #161a23
foreground = #e1e7f0
background-opacity = 92
background-blur = true
cursor-shape = underline
cursor-color = #b1ccff
cursor-blink = true
cursor-blink-interval = 500
selection-foreground = #f0f4fa
selection-background = #394969
# Optional ANSI palette overrides, one line per index from 0 through 15.
palette = 1=#e06c75
palette = 4=#61afef
```

Padding is in pixels (0–128), opacity is a percentage (0–100), and the blink
interval is the duration of each phase in milliseconds (100–5000). Cursor
shapes are `block`, `underline`, and `bar`. Applications can change the active
shape and blink mode through DECSCUSR and DEC private mode 12.

Opacity affects the default background. Explicit ANSI backgrounds, selected
text, and glyph colors keep their own opacity. Blur requires a compositor.
`background-blur` controls the KWin-compatible X11 blur request. Compositors
with their own rules, including Hyprland, apply their configured blur policy
to transparent windows. Set opacity to 100 for a fully opaque window on any
desktop.

## Controls

| Action | Control |
| --- | --- |
| Select text | Drag with the left mouse button |
| Select while a TUI captures the mouse | Shift + drag |
| Copy selection | Ctrl + Shift + C |
| Paste clipboard | Ctrl + Shift + V |
| Paste primary selection | Middle mouse button |
| Browse history | Mouse wheel or Shift + PageUp / PageDown |
| Open an OSC 8 link | Ctrl + click |

When an application captures mouse events, hold Shift to use local selection,
wheel scrolling, or primary-selection paste.

Links are opened only after an explicit click, using argv rather than a shell.
Supported link schemes are HTTP, HTTPS, and mailto. Clipboard input is bounded;
ESC and non-text controls are removed before bracketed-paste wrapping.

## Reusable Project Z components

Each package has its own manifest, license, documentation, and public API. They
can be used without the graphical application.

| Package | Responsibility | Rust dependencies |
| --- | --- | --- |
| [zinput](crates/zinput) | Legacy / Kitty keyboard, mouse, focus, and paste encoding | None |
| [zpty](crates/zpty) | Linux nonblocking PTY and explicit child lifecycle | None |
| [zterm](crates/zterm) | Screen, scrollback, VT state, queries, and damage tracking | zinput, zvte, unicode-width |
| [zvte](https://github.com/LIghtJUNction/zvte) | Allocation-free streaming ANSI/VT parser | None |

```sh
cargo add zinput@0.1.0 --no-default-features
cargo add zpty@0.1.0 --no-default-features
cargo add zterm@0.1.0 --no-default-features
# For applications that need only the parser:
cargo add zvte@0.1.3 --no-default-features
```

[`zinput 0.1.0`](https://crates.io/crates/zinput/0.1.0),
[`zpty 0.1.0`](https://crates.io/crates/zpty/0.1.0), and
[`zterm 0.1.0`](https://crates.io/crates/zterm/0.1.0) are published.
All three passed individual and combined consumers using exact registry versions
outside this workspace, including a real PTY session.
See [the release record](docs/releasing.md) for the verified publication state.

All production Rust crates use `#![no_std]`. The libraries use `alloc` where
variable storage is necessary, with the allocator supplied by their host.
Normal dependencies have default features disabled. The GUI links system C ABI
libraries (`libc`, `libutil`, `X11`, `Xft`, `Xrender`, and the compiler unwinder); those are
listed separately from the Rust dependency graph. Development tests and
benchmark runners use `std`.

## Terminal behavior

- Streaming UTF-8, CJK double-width cells, and two inline combining scalars per
  cell; input works across arbitrary byte boundaries.
- Main and alternate screens, cursor save/restore, scrolling regions, tabs,
  insert/delete/erase, DEC line drawing, 16/256 colors, and RGB colors.
- Kitty keyboard progressive enhancement flags 1, 2, 4, 8, and 16, including
  independent main/alternate flag stacks and event reports.
- SGR and legacy mouse reports, focus events, bracketed paste, and synchronized
  output with a 150 ms recovery timeout.
- OSC 0/2 titles, OSC 7 working-directory metadata, OSC 8 hyperlinks, palette
  queries, DSR, DA, DECRQM, and a bounded subset of XTGETTCAP.
- XIM input, font fallback, selection, clipboard/primary selection, and INCR
  clipboard reception.

The screen model stores 28 bytes per cell on the tested x86_64 target. Full-screen
scrolling rotates row indices; retained history grows lazily, is limited to
8 MiB of cells, and reuses old rows once full. The event loop sleeps in `poll`
when idle and redraws damaged rows.

## Current boundaries

This release uses X11/XWayland. Native Wayland, a dedicated GPU renderer, Sixel,
Kitty graphics, OSC 52 clipboard access, complex text shaping / full emoji ZWJ
layout, and history reflow are future work. Resizing preserves and crops existing
rows; shrinking vertically keeps the cursor visible and retains displaced main
screen rows in history. Italic attributes are retained by the core; the initial
renderer uses the selected font face. Text blink attributes are not animated.
Cursor blinking is independent and runs only while the visible cursor is focused.

Capability replies advertise the implemented subset. Image support and complete
Kitty/Ghostty feature parity are not claimed.

## Verify

```sh
sh tools/check.sh
cargo test --workspace --doc
cargo run -p zpty --example capture
cargo run --release -p zterm --example benchmark
python3 tests/gui_smoke.py --binary ./target/release/ztty
python3 tests/shared_gui_smoke.py --binary ./target/release/ztty
python3 tests/mcp_transport.py --binary ./target/release/ztty
python3 tests/mcp_sessions.py --binary ./target/release/ztty
# Optional: real Fcitx 5 XIM with the Pinyin addon and a private D-Bus session.
python3 tests/gui_smoke.py --fcitx --binary ./target/release/ztty
python3 tools/verify_registry.py

# Optional portability gate for the platform-independent libraries:
rustup target add x86_64-unknown-none
ZTTY_BARE_METAL=1 sh tools/check.sh
```

The GUI test starts its own Xvfb session and uses real X11 input, clipboard,
resize, PTY, and frame presentation paths. It requires Xvfb, python-xlib, and
Pillow. It leaves the current desktop session untouched and produces a JSON
result plus a screenshot under `artifacts/`.

See [architecture](docs/architecture.md), [protocol references](docs/protocols.md),
and [release instructions](docs/releasing.md).

MIT © LIghtJUNction
