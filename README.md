# ztty

A lean terminal for Linux, written in Rust without the Rust standard library.

ztty runs real shells and terminal applications through a Linux PTY. Its screen
engine and input protocols are independent libraries; the desktop frontend uses
the system X11 and Xft libraries. The visual design is deliberately quiet: a dark
background, readable text, generous padding, and no application toolbar.

The long-term goal is to exceed Kitty and Ghostty in responsiveness and memory
efficiency. Current measurements, their scope, and remaining work are documented
in [the performance report](docs/performance.md). This first release is a working
Linux terminal with an explicit compatibility boundary.

[中文介绍](README.zh-CN.md)

![Actual ztty protocol preview](docs/assets/ztty-preview.png)

## Build and run

Requirements: Linux, Rust 1.100.0, a C linker, X11/Xft development libraries, and a
monospace font. The X11 frontend can also run through XWayland on Wayland desktops.
Automated GUI acceptance currently uses Xvfb/X11 on x86_64; a real XWayland
session and IBus/Fcitx input-method sessions have not yet been accepted.

```sh
# Arch Linux
sudo pacman -S --needed base-devel libx11 libxft fontconfig

# Debian / Ubuntu
sudo apt install build-essential libx11-dev libxft-dev libfontconfig1-dev

cargo build --locked --release
./target/release/ztty
./target/release/ztty --font 'monospace:size=12' --geometry 100x28
./target/release/ztty --scrollback 4000 -e /bin/bash
```

`make install` installs the executable and desktop entry under `~/.local`.
Set `PREFIX` to choose another location. The default shell comes from `SHELL`,
with `/bin/sh` as the fallback. Child sessions receive `TERM=xterm-256color`,
`COLORTERM=truecolor`, and `TERM_PROGRAM=ztty`.

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
# For applications that need only the parser:
cargo add zvte@0.1.3 --no-default-features
```

[`zinput 0.1.0`](https://crates.io/crates/zinput/0.1.0) and
[`zpty 0.1.0`](https://crates.io/crates/zpty/0.1.0) are published.
`zterm 0.1.0` is available in this workspace; its registry publication is pending.
See [the release record](docs/releasing.md) for the verified publication state.

All production Rust crates use `#![no_std]`. The libraries use `alloc` where
variable storage is necessary, with the allocator supplied by their host.
Normal dependencies have default features disabled. The GUI links system C ABI
libraries (`libc`, `libutil`, `X11`, `Xft`, and the compiler unwinder); those are
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
renderer uses the selected font face. Blink attributes do not schedule timers.

Capability replies advertise the implemented subset. Image support and complete
Kitty/Ghostty feature parity are not claimed.

## Verify

```sh
sh tools/check.sh
cargo test --workspace --doc
cargo run -p zpty --example capture
cargo run --release -p zterm --example benchmark
python3 tests/gui_smoke.py --binary ./target/release/ztty
python3 tools/verify_registry.py --packages zinput zpty

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
