# ztty

[中文](README.zh-CN.md) · [Documentation](docs/README.md) · [Releases](https://github.com/LIghtJUNction/ztty/releases)

ztty is a native terminal for Linux, macOS and Windows. Its terminal core is
written in Rust without the standard library. On Linux, the same live session
can be used from a window, the CLI or an MCP client.

This page documents **0.6.1**. Published binaries and their release notes are
available on the [download page](https://github.com/LIghtJUNction/ztty/releases/tag/v0.6.1).

## Install on Linux

On Arch Linux: `yay -S ztty`. For a published x86_64 or AArch64 binary:

```sh
curl -fsSLO https://github.com/LIghtJUNction/ztty/releases/latest/download/install.sh
sh install.sh
ztty
```

The installer verifies the archive and installs to `~/.local/bin`.
On Wayland desktops, the X11 backend runs through XWayland. Current source
uses `--backend auto`; see [backend selection](docs/configuration.md#native-wayland-experimental)
for its initial fallback and session compatibility rules.

## Build and run

Use Rust **1.100.0**, a C compiler and a monospace font. Install the current
Linux source's native dependencies with the command for your distribution:

```sh
# Arch Linux
sudo pacman -S --needed base-devel pkgconf libx11 libxft libxrender \
  fontconfig freetype2 harfbuzz libglvnd ttf-dejavu

# Debian / Ubuntu
sudo apt install build-essential pkg-config libx11-dev libxft-dev libxrender-dev \
  libfontconfig1-dev libfreetype-dev libharfbuzz-dev libegl1-mesa-dev \
  libgles2-mesa-dev fonts-dejavu-core
```

```sh
git clone --recurse-submodules https://github.com/LIghtJUNction/ztty.git
cd ztty
cargo build --locked --release
./target/release/ztty --font 'monospace:size=12' --geometry 100x28
./target/release/ztty --cwd /path/to/project -e /bin/bash
sh install.sh --from-binary ./target/release/ztty
```

For an existing clone, run `git submodule update --init --recursive` before building.

Linux/macOS use `SHELL` (falling back to `/bin/sh`); Windows uses `COMSPEC`
(falling back to `cmd.exe`). `-e` chooses another program. Installing an update
leaves existing brokers running their current code; see
[broker upgrades and reload](docs/sessions.md#upgrading-a-running-broker).
[Native build and packaging details](docs/releasing.md) cover macOS and Windows.

## What it does

- VT sequences, true color, mouse reports, bracketed paste and Kitty keyboard input.
- Unicode grapheme storage, history reflow and copy across wrapped lines.
- Kitty images and Sixel; Linux adds animation, Unicode placeholders and relative placement.
- Linux X11 Kitty file/shared-memory transport and bounded OSC 52 clipboard access.
- Linux shared sessions; X11 tabs, splits, history search and a command palette.
- Linux X11 GPU text shaping, Xft fallback, live configuration and XIM recovery.
- Optional [shell integration](shell/README.md) and [trusted text actions](docs/text-plugins.zh-CN.md).

The Linux X11 renderer defaults to `renderer = auto`: hardware EGL when available,
otherwise Xft. [Configuration](docs/configuration.md) and the
[protocol reference](docs/protocols.md) describe supported modes and limits.

## Linux X11 controls

| Action | Shortcut |
| --- | --- |
| Copy / paste | Ctrl+Shift+C / V |
| Select text inside a mouse-driven TUI | Shift+drag |
| Search history / open command palette | Ctrl+Shift+F / P |
| Switch sessions | Ctrl+Shift+S |
| New tab / split right | Ctrl+Shift+T / E |
| Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab |
| Increase / decrease / reset font | Ctrl+Plus / Minus / 0 |
| Scroll history | Shift+PageUp / PageDown or mouse wheel |
| Open a link | Ctrl+click |
| Reload configuration | Ctrl+Shift+R |

Double-click selects a word; triple-click selects a line. The **Back to live ↓**
button returns from history to current output.
[Daily workflow](docs/daily-workflow.md), [workspaces](docs/workspaces.md) and
[input compatibility](docs/input-compatibility.md) cover custom bindings and keypad behavior.

`Ctrl+Shift+V` pastes text. `Ctrl+V` goes to the foreground application;
Codex CLI 0.170.0 uses it to attach clipboard images. Since ztty 0.5.1, terminal text paste
rejects image-only selections. See
[image paste and format checks](docs/input-compatibility.md#clipboard-images-and-application-paste).

## Shared sessions and MCP (Linux)

Linux X11 and native Wayland windows use the same session broker. Each session
keeps its own PTY, screen and history. X11 windows also share a display connection
and font cache.

```sh
ztty --name work -e /bin/bash &
ztty new                         # Open one independent window and return
ztty new --count 3 --name workset # Open workset-1, workset-2 and workset-3
ztty attach                     # List sessions
ztty attach work                # Open or raise its window
ztty attach --observe work      # Read-only view
ztty mcp                        # MCP over stdio
ztty mcp --transport http --host 127.0.0.1 --port 8732
```

MCP can create sessions, read output, send input, resize and close PTYs, and
manage input ownership. MCP-created sessions survive transport disconnects;
closing an attached view preserves their PTY. HTTP outside loopback needs a token.
See [sessions and MCP](docs/sessions.md) for client setup, session identities,
JSON/SSE replies and command-output reads; [observer mode](docs/ssmcp-observer.md)
covers tools such as ssmcp.

## SSH terminal multiplexing (Linux)

ztty 0.6.1 keeps remote shells in a headless session broker. Closing an SSH
connection or its window leaves those shells running; reconnecting restores
their screen and bounded scrollback. Install ztty on both Linux machines, then:

```sh
ztty ssh dev main                         # SSH alias from ~/.ssh/config
ztty ssh ssh://alice@host:2222 build
ztty ssh --list dev
ztty ssh --observe dev build              # Read-only; does not resize the writer
ztty ssh --terminal dev main              # Attach in the current terminal
ztty ssh --close dev build                # Explicitly terminate a remote session
```

Ctrl+] detaches. Ctrl+Shift+T and the split shortcuts in an SSH window create
independent remote sessions on the same host. OpenSSH reuses one authenticated
connection for these panes and continues to manage keys, SSH config and host-key
checks. Input has one owner; `--takeover` explicitly transfers it.

[SSH sessions](docs/ssh-sessions.md) covers ports, identity/config files, native
tabs and splits, remote commands, local headless use and the text-view scope.

## Reload the running executable

A broker started from ztty 0.5.1 or later can replace its executable on Linux
X11/XWayland or without windows. Run this inside the session you want to update:

```sh
ztty reload
ztty reload --binary /absolute/path/to/ztty
```

`ZTTY_SOCKET` selects the broker. The default target is the executable of the CLI
you invoked; the command does not download an update. It keeps the broker PID,
existing shells and command PTYs, session identities, queued input, output byte
cursors, terminal screens and history, and X11 tabs/splits. Windows are rebuilt and may
briefly disappear. Active native Wayland views block this handover; detached
native sessions retain their backend for later reattachment. macOS and Windows
do not support this handover.

A broker from the published 0.5.0 release safely refuses reload, even when asked
by a newer CLI. Keep its windows until their work finishes and use a new source-built broker for new
work; that broker can support later reloads. The
[reload guide](docs/sessions.md#reload-the-running-broker) covers validation,
temporary refusal, display recovery and the one-time transition.
Ctrl+Shift+R continues to reload configuration only.

## Appearance

On Linux, edit `~/.config/ztty/ztty.conf`; use `--config PATH` for another file.

```ini
font = monospace:size=11
window-padding = 16
background = #161a23
foreground = #e1e7f0
background-opacity = 92
renderer = auto
```

Linux X11 reloads valid changes in place. [Configuration](docs/configuration.md)
covers platform paths, colors, cursor behavior, bindings and renderer selection.

## Platforms and verification

| Platform | Current source |
| --- | --- |
| Linux X11/XWayland | GPU or Xft; shared sessions, tabs/splits, attach and MCP |
| Linux native Wayland | Auto fallback or explicit EGL/GLES; shared sessions, attach/MCP, clipboard and text-input IME |
| macOS | Native AppKit/CoreText window and Darwin PTY; local sessions |
| Windows | Native Win32/GDI and ConPTY; local tabs and splits |

macOS runs without X11; Windows runs without WSL. Windows supports font and
workspace shortcuts; macOS uses Command+C/V or Ctrl+Shift+C/V for copy/paste.
Shared sessions, attach and MCP are Linux features.
[Platform verification](docs/protocols.md#acceptance-coverage) records the tested scope.

To try the experimental native Wayland backend:

```sh
cargo build --locked --release --features wayland
./target/release/ztty --backend wayland --name work
```

The official Linux package build enables Wayland. Current source defaults to
Auto and tries the complete X11/XWayland view before a compatible native fallback.
Wayland rejects tabs, splits and workspace containers. [Wayland setup](docs/configuration.md#native-wayland-experimental)
covers dependencies and the unverified physical GPU, desktop IME and scaling behavior.

## Performance and development

The [runtime iteration report](docs/benchmarks/runtime-iteration-20261120/report.zh-CN.md)
records the startup marker and INCR paste fixes, Auto selection, query-safe reload,
final native checks and installation. In its fixed Xvfb GPU-path test, lazy software
backing pixmaps save about 3.66 MiB per window of X-server resources; this is not
a whole-process RSS or hardware VRAM claim. Large-paste ZLE editing time remains.

After input is fully sent or explicitly cancelled, this source releases an
input buffer whose allocated capacity exceeds 64 KiB; smaller buffers remain
available for reuse. The [input-buffer regression](docs/benchmarks/input-buffer-memory-20261120/report.zh-CN.md)
records an 8 MiB queue retaining 0 bytes after clearing, compared with 8 MiB
before the change. This measures queue allocation capacity; process RSS/PSS and
throughput were not measured for this change.

<!-- terminal-benchmark:start -->
The [2026-11-20 performance and reload report](docs/benchmarks/performance-reload-2026-11-20.md)
includes fixed source and executable hashes, every raw sample, and test limits.
Compared with the retained baseline, software-rendered ASCII/color throughput
rose from 171.7 to 257.5 MiB/s with one window and from 158.5 to 229.2 MiB/s with
ten windows. A separate scrolling Unicode test improved by 53.4% using the
median of three paired ratios. Small negative headless ASCII results are also recorded.

The three-terminal comparison uses a private Xvfb display and GPU code paths
under a forced Mesa software-GL environment; ztty reports llvmpipe. Ten windows stay open, with one output producer
measured at a time. These are logical PTY/DSR timings, not physical frame rates.

| Ten-window shared session | Application PSS, MiB | Output, MiB/s |
| --- | ---: | ---: |
| ztty | 326.9 | 150.4 |
| Ghostty | 809.0 | 140.1 |
| Kitty | 354.6 | 53.0 |

![Ten-window comparison under Xvfb and llvmpipe](docs/benchmarks/performance-reload-20261120/final-terminals.svg)

Values are medians of three independent runs. Font metrics, pixel dimensions,
host memory pressure and renderer choice limit generalization; the report keeps
all ranges. Single-window ztty/Ghostty throughput overlaps and is not a stable
ranking. Software and GPU paths also pass the recorded complete-frame tab-switch
and synchronized-output checks; physical desktop compositors need separate validation.
<!-- terminal-benchmark:end -->

Run `sh tools/check.sh` for the local development checks.

## Reusable libraries

| Library | Role in ztty | Status |
| --- | --- | --- |
| [zgpui](https://github.com/LIghtJUNction/zgpui) | UI scenes and layout | Published [0.1.5](https://crates.io/crates/zgpui/0.1.5) |
| [zinput](https://github.com/LIghtJUNction/zinput) | Keyboard, mouse and paste encoding | Published [0.1.3](https://crates.io/crates/zinput/0.1.3) |
| [zpty](https://github.com/LIghtJUNction/zpty) | PTY / ConPTY process ownership | Published [0.2.3](https://crates.io/crates/zpty/0.2.3) |
| [zgraphics](https://github.com/LIghtJUNction/zgraphics) | Image protocol and placement | Published [0.2.0](https://crates.io/crates/zgraphics/0.2.0) |
| [zterm](https://github.com/LIghtJUNction/zterm) | Terminal screen and protocol state | Published [0.5.2](https://crates.io/crates/zterm/0.5.2) |

These published versions include the terminal checkpoint and PTY handover APIs
used by ztty 0.5.2. Development builds use the libraries' GitHub default branches,
with exact source commits recorded in Cargo.lock. Library submodule pins match
that snapshot for independent checks; the table records published registry versions.
The application keeps zgpui's `std` and
`native` features disabled.

The [architecture](docs/architecture.md) describes the reusable Project Z libraries;
the [documentation index](docs/README.md) links usage guides and verification records.

MIT © LIghtJUNction
