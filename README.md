# ztty

<!-- terminal-benchmark:start -->
[![Ten shared terminal windows with verified Xvfb ownership](docs/benchmarks/guarded-idle-1-10-2026-11-18.svg)](docs/benchmarks/guarded-idle-1-10-2026-11-18.md)

Three fresh-process runs per cell, at 1 and 10 shared idle windows; the figure shows 10. [Raw measurements](docs/benchmarks/guarded-idle-1-10-2026-11-18.json) · [Conditions, provenance and limits](docs/benchmarks/guarded-idle-1-10-2026-11-18.md) · rerun from current source: `make benchmark-terminals`.

Measured source: `fa2f2017`; immutable binary: `a339a7d2…1b6d4d`. All 18 scenarios verified their private Xvfb listener ownership and cleaned up their owned processes. A single ztty terminal used **22.02 MiB RSS**; the below-10-MiB target remains unmet.

Linux software-X11 results: host load1 4.46–10.58 on 24 CPUs, with 17.34–17.96 GiB host swap already in use. RSS, PSS and Swap remain separate. Real GPU memory, first presented frame and input-to-display latency were not measured; the guarded 5/20-window, independent-instance and remaining-workload matrix is still incomplete.

<!-- terminal-benchmark:end -->

A lean native terminal, written in Rust without the Rust standard library.

ztty runs shells and terminal applications through the operating system's
pseudo-terminal interface. Version 0.2 adds native macOS and Windows backends and
static Kitty Graphics Protocol support. Screen state, input encoding, PTY
ownership, and image processing are reusable libraries. The visual design is
deliberately quiet: a dark background, readable text, generous padding, and no
application toolbar.

On Linux, multiple windows automatically share one session process, display connection
and font cache. Each window keeps its own PTY, screen and history. In the
recorded pre-graphics eight-window comparison, total ztty PSS fell from 47.021 to 15.366 MiB,
including the waiting launchers. The single-window case rose by 0.539 MiB.
See [measurement conditions and graphics-memory accounting](docs/performance.md#multiple-window-memory).

The long-term goal is to exceed Kitty and Ghostty in responsiveness and memory
efficiency. Current measurements, their scope, and remaining work are documented
in [the performance report](docs/performance.md). Linux remains the first
acceptance target; the platform table records what has actually been exercised.

[中文介绍](README.zh-CN.md)

Linux now also provides history search, a temporary command palette, configurable
key/mouse bindings, live configuration reload and OSC 133 command navigation.
The shell scripts support opt-in Bash, Zsh, Fish and Nushell integration.
[Daily workflow and limits](docs/daily-workflow.md) · [Shell setup](shell/README.md)
· [MCP permissions and command reads](docs/sessions.md).

![Actual ztty window displaying a photo and transparent PNG in Hyprland/XWayland](docs/assets/kitty-graphics-hyprland.png)

## Platforms and verification

Shared sessions, attach, and MCP currently use the Linux broker. Native macOS
and Windows terminals remain available through their platform backends.

| Platform | Native implementation | Current acceptance status |
| --- | --- | --- |
| Linux | X11/XWayland, Xft text, Xrender image compositing, Linux PTY | Current x86_64 candidate: workspaces 10, daily workflow 14, Sixel 10 and 27 exact input vectors passed under guarded Xvfb; ARM64 gate remains open |
| macOS | AppKit window, CoreText/CoreGraphics rendering, Darwin PTY | 8 native session/protocol checks passed on Intel and Apple Silicon; pixel readback and IME/keyboard acceptance remain open |
| Windows | Win32 window, GDI text, own RGBA compositing, ConPTY | 25 native checks passed on x86_64, including six Sixel groups, window pixels and keyboard recovery; ARM64 remains unaccepted |

The macOS and Windows backends use their native APIs. macOS does not require
X11, and Windows does not require WSL. Cross-target compilation does not establish
runtime or visual correctness. See [protocol acceptance](docs/protocols.md#acceptance-coverage)
and [the release record](docs/releasing.md) for the verified scope and available
artifacts. The [current candidate record](docs/native-candidates-2026-11-18.md)
retains both ARM64 failures and their subsequent fixes. Earlier Hyprland images
remain historical evidence; the current development desktop has no usable
standard Xwayland endpoint.

## Install on Linux

The examples below describe the current source tree. Release downloads follow
the latest published tag; build from source for unreleased changes.

On Arch Linux, install the AUR package with `yay -S ztty`. Other Linux
distributions can install a prebuilt x86_64 or AArch64 binary with:

```sh
curl -fsSLO https://github.com/LIghtJUNction/ztty/releases/latest/download/install.sh
sh install.sh
```

The script installs the latest published release, verifies its archive, and
installs to `~/.local/bin`. The Linux binary requires glibc, X11, Xft, and Xrender.
The working tree may contain changes that have not yet reached that release.

## Build and run

The toolchain is Rust 1.100.0. On Linux, also install a C linker, X11/Xft/Xrender
development libraries, and a monospace font. The X11 frontend can run through
XWayland on Wayland desktops.

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
Set `PREFIX` to choose another location. Linux and macOS use `SHELL`, with
`/bin/sh` as the fallback; Windows uses `COMSPEC`, falling back to `cmd.exe`.
Use `-e` to select a different command and `--cwd PATH` for its initial
directory. Relative commands use the selected cwd; relative config paths keep
the launcher directory. [Startup directory behavior and native acceptance](docs/startup-directory.md)
describe the platform boundary. Child sessions receive
`TERM=xterm-256color`, `COLORTERM=truecolor`, and `TERM_PROGRAM=ztty`.

macOS builds link AppKit, CoreText, CoreGraphics, and the Darwin runtime.
Windows builds use the Win32 APIs and ConPTY. The Windows packaging path includes
a pinned Microsoft ConPTY redistribution; `zpty` can also use the Windows inbox
API. The [PTY documentation](crates/zpty/README.md) explains the operating-system
requirements and lifetime differences. Native packaging and acceptance commands
are maintained in [the release workflow](.github/workflows/release.yml).

## Inline images

Run an existing Kitty-compatible tool inside ztty to display static images:

```sh
kitten icat --scale-up photo.jpg
kitten icat transparent.png
```

The tool decodes a JPEG before transmission; ztty accepts inline PNG, RGB, or
RGBA data. Automatic detection selects inline `stream` transmission; it can also
be selected explicitly with `--transfer-mode=stream`. File and shared-memory transports
are outside this version's scope. Capability queries, compressed and chunked
uploads, image IDs, placements, source cropping, scaling, alpha blending, and
deletion are implemented. Images follow terminal scrolling, and the primary and
alternate screens keep separate image scenes.

The Linux graphics suite sends data through a real PTY and reads window pixels.
It checks colors, transparent edges, clipping, image replacement, scrolling,
resize, alternate-screen isolation, deletion, and recovery after invalid input.
Its final check runs `kitten icat` 0.50 on a photo and a transparent PNG.
See [the graphics protocol scope](docs/protocols.md#kitty-graphics) for limits
and deferred features.

## Shared sessions and MCP (Linux)

Reacquiring input control preserves already accepted input and restores an explicitly observing local view; [state rules and real PTY evidence](docs/session-core.md) describe the boundary.

On Linux, run several terminals normally; resource sharing is automatic:

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

MCP exposes nine tools to create, list, read, write, resize and close PTYs,
manage input ownership, list shell commands and read their retained output.
MCP-created sessions keep their output when the transport exits; closing their
window detaches it. Ordinary local windows keep the usual close-to-exit
behavior. HTTP listeners outside loopback require a bearer token.

See [sessions and MCP](docs/sessions.md) for naming, lifecycle, HTTP tokens,
client setup and bounded UTF-8 output cursors.

## Appearance

Linux uses `$XDG_CONFIG_HOME/ztty/ztty.conf`, defaulting to
`~/.config/ztty/ztty.conf`. macOS defaults to
`~/Library/Application Support/ztty/ztty.conf` and accepts the same explicit
`XDG_CONFIG_HOME` override. Windows uses `%APPDATA%\ztty\ztty.conf`.
Choose another file with `ztty --config PATH`. Settings take effect on the next
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

On Linux, opacity affects the default background. Explicit ANSI backgrounds,
selected text, and glyph colors keep their own opacity. Blur requires a
compositor. `background-blur` controls the KWin-compatible X11 blur request.
Compositors with their own rules, including Hyprland, apply their configured blur policy
to transparent windows. Set opacity to 100 for an opaque Linux window. macOS
uses a native visual-effect view for blur; Windows currently renders an opaque
GDI surface. Appearance parity across the three backends has not been validated.

## Linux controls

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

| Package | Source version | Responsibility | Direct Rust dependencies |
| --- | --- | --- | --- |
| [zinput](crates/zinput) | 0.1.0 | Legacy / Kitty keyboard, mouse, focus, and paste encoding | None |
| [zpty](crates/zpty) | 0.2.0 | Linux/macOS PTY, Windows ConPTY, explicit child lifecycle | None |
| [zterm](crates/zterm) | 0.2.0 | Screen, scrollback, VT state, image anchoring, queries, damage | zinput, zvte, zgraphics, unicode-width |
| [zgraphics](crates/zgraphics) | 0.1.0 | Bounded Kitty graphics, PNG/RGB/RGBA decoding, image storage and placement | zune-core, zune-png, zune-inflate |
| [zvte](https://github.com/LIghtJUNction/zvte) | 0.1.3 | Allocation-free streaming ANSI/VT parser | None |

The reusable packages are available from crates.io:

```sh
cargo add zinput@0.1.0 --no-default-features
cargo add zgraphics@0.1.0 --no-default-features
cargo add zpty@0.2.0 --no-default-features
cargo add zterm@0.2.0 --no-default-features
# For applications that need only the parser:
cargo add zvte@0.1.3 --no-default-features
```

[`zinput 0.1.0`](https://crates.io/crates/zinput/0.1.0),
[`zgraphics 0.1.0`](https://crates.io/crates/zgraphics/0.1.0),
[`zpty 0.2.0`](https://crates.io/crates/zpty/0.2.0), and
[`zterm 0.2.0`](https://crates.io/crates/zterm/0.2.0) are published.
All four packages passed individual and combined consumers using exact
registry versions outside this workspace, including a real PTY session.
See [the release record](docs/releasing.md) for the verified publication state.

Production Rust compiles without `std`. The Project Z crates use `#![no_std]`;
the PNG, DEFLATE, checksum, and Unicode-width dependencies are selected with
their `no_std` feature configuration. All direct normal dependencies disable
default features. The libraries use `alloc` where variable storage is necessary,
with the allocator supplied by their host. `zgraphics` uses shared immutable
pixel buffers and requires pointer-width atomics.

Native platform libraries remain part of the runtime: X11/Xft/Xrender and the C
runtime on Linux, Apple frameworks on macOS, and Win32/ConPTY on Windows. They
are accounted for separately from the Rust dependency graph. Tests and benchmark
runners use `std`. [The dependency audit](tools/check_no_std.py) checks the
enabled production feature graph.

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
  queries, character/cell/pixel size reports, DSR, DA, DECRQM, and a bounded
  subset of XTGETTCAP.
- Static Kitty graphics and streaming Sixel decoding with bounded image storage
  and transfer buffers. Sixel shares the existing image store and render paths;
  native acceptance boundaries are recorded in [the protocol matrix](docs/protocols.md).
- On Linux: XIM input, font fallback, selection, clipboard/primary selection,
  and INCR clipboard reception.

The screen model stores 28 bytes per cell on the tested x86_64 target. Full-screen
scrolling rotates row indices; retained history grows lazily, is limited to
8 MiB of cells, and reuses old rows once full. The Linux event loop waits in
`poll` and redraws damaged rows. Image storage is separate and allocated only
when images are received; its quotas are documented in
[the architecture](docs/architecture.md#image-lifecycle-and-memory).

## Current boundaries

The Linux backend uses X11/XWayland. Native Wayland, a dedicated GPU renderer,
Kitty file/shared-memory transport and animation, OSC 52 clipboard access,
complex text shaping / full emoji ZWJ layout, and history reflow are future work.
Kitty Unicode placeholders and relative placements are also deferred.
Resizing preserves and crops existing rows; shrinking vertically keeps the
cursor visible and retains displaced main-screen rows in history. Italic
attributes are retained by the core; the Linux renderer uses the selected font
face. Text blink attributes are not animated.
Cursor blinking is independent and runs only while the visible cursor is focused.

Capability replies advertise the implemented subset. Static images are supported;
complete Kitty/Ghostty feature parity and a performance lead remain goals.

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
python3 tests/graphics_smoke.py --binary ./target/release/ztty
# Optional: real Fcitx 5 XIM with the Pinyin addon and a private D-Bus session.
python3 tests/gui_smoke.py --fcitx --binary ./target/release/ztty
python3 tools/verify_registry.py

# Optional portability gate for the platform-independent libraries:
rustup target add x86_64-unknown-none
ZTTY_BARE_METAL=1 sh tools/check.sh
```

The Linux GUI and graphics tests start their own Xvfb sessions and use real
X11 input, clipboard, resize, PTY, and frame presentation paths. They require
Xvfb, python-xlib, and Pillow; the graphics suite also requires `kitten`.
They leave the current desktop session untouched and write JSON results and
screenshots under `artifacts/`. The native macOS and Windows gates are
[macos_native.py](tests/macos_native.py), retaining the original eight
[protocol checks](tests/macos_smoke.py), and
[windows_native.py](tests/windows_native.py). Their recorded results and remaining
acceptance gaps are listed in [protocol acceptance](docs/protocols.md#acceptance-coverage).

See [architecture](docs/architecture.md), [protocol references](docs/protocols.md),
and [release instructions](docs/releasing.md).

## Tabs and panes on Linux

Shared sessions now support tabs, splits, pane focus and moves between native windows.
Use `ztty --split-right NAME`, `ztty --tab NAME`, or Ctrl+Shift+E/T.
`ztty detach NAME` preserves its PTY; `ztty attach --observe NAME` opens an observing view.
[Commands, ownership and verified behavior](docs/workspaces.md).

`ztty save-layout NAME > workspace.json` records the live workspace.
`ztty restore-layout workspace.json` restores only matching, still-attached
session incarnations; it does not restart exited commands.

`ztty --observe -e COMMAND` starts a standalone read-only command window without
requesting focus. Closing that window ends the child it created.
`ztty attach --observe NAME` observes an existing session; closing that view
preserves its live PTY. External tools using `TERMINAL -e COMMAND`, including
`ssmcp --tty`, can use [tools/ztty-observer](tools/ztty-observer).
[Real integration, lifecycle and remaining platform boundaries](docs/ssmcp-observer.md).

MIT © LIghtJUNction
