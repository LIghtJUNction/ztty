# ztty

A compact native terminal with shared Linux sessions and a reusable Rust core.
ztty runs your shell through an operating-system PTY, keeps screen and history
state in bounded buffers, and draws through the platform's native window system.

[中文](README.zh-CN.md) · [Downloads](https://github.com/LIghtJUNction/ztty/releases)
· [Daily workflow](docs/daily-workflow.md) · [Sessions and MCP](docs/sessions.md)

![ztty displaying a photograph and a transparent image](docs/assets/kitty-graphics-hyprland.png)

## Install

On Arch Linux:

```sh
yay -S ztty
```

For a published Linux binary:

```sh
curl -fsSLO https://github.com/LIghtJUNction/ztty/releases/latest/download/install.sh
sh install.sh
```

The installer defaults to `~/.local/bin`, verifies the downloaded archive, and
keeps one previous executable for rollback. An upgrade takes effect for new
processes; a running shared session keeps its loaded executable.
[Installer options and running-session upgrades](docs/sessions.md#upgrading-a-running-broker).

This README describes the source tree. Downloads contain the latest published
release; build from source to use changes that have not yet been released.

## Build on Linux

Use the Rust toolchain pinned in `rust-toolchain.toml`. On Debian or Ubuntu,
install the native build dependencies:

```sh
sudo apt-get install build-essential pkg-config libx11-dev libxft-dev libxrender-dev \
  libfontconfig1-dev libfreetype-dev libharfbuzz-dev libegl1-mesa-dev libgles2-mesa-dev \
  libwayland-dev libwayland-bin wayland-protocols libxkbcommon-dev \
  fonts-dejavu-core fonts-noto-cjk fonts-noto-color-emoji
git clone https://github.com/LIghtJUNction/ztty.git
cd ztty
cargo build --locked --release --features wayland
```

```sh
./target/release/ztty
./target/release/ztty --backend wayland
./target/release/ztty --font 'monospace:size=12' --geometry 100x28
./target/release/ztty --cwd /path/to/project -e /bin/bash
```

Linux defaults to X11, including XWayland. Native Wayland is selected explicitly
with `--backend wayland`; omit the Cargo feature for an X11-only build.
To install your own build, run
`sh install.sh --from-binary target/release/ztty`.

## Features

- Unicode graphemes, soft-wrap-aware history reflow, selection and history search.
  Linux GPU text uses FreeType and HarfBuzz for shaping, fallback fonts and color
  emoji, with italic text and timed blinking.
- GPU and software X11 rendering, plus native Wayland rendering through EGL/GLES.
  Dirty rows, bounded caches and batched redraws limit repeated work.
- Kitty graphics with file/shared-memory transport, animation, Unicode
  placeholders and relative placements, plus Sixel and OSC 52 clipboard handling.
  See the [graphics verification and platform limits](docs/reports/graphics-extensions-20261120/report.zh-CN.md).
- Shared Linux sessions, attach/observe views and a native MCP server operating
  on the same PTYs that the windows display.
- A temporary command palette, session switcher, configurable key/mouse
  bindings, live configuration reload and optional shell integration.

### Everyday controls

| Action | Linux shortcut |
| --- | --- |
| Copy / paste | Ctrl+Shift+C / Ctrl+Shift+V |
| Search history | Ctrl+Shift+F |
| Next / previous search result | Enter / Shift+Enter |
| Toggle search case sensitivity | Alt+C |
| Command palette / session switcher | Ctrl+Shift+P / Ctrl+Shift+S |
| Increase / decrease / reset font size | Ctrl++ / Ctrl+- / Ctrl+0 |
| Browse history | Shift+PageUp / Shift+PageDown, or wheel |
| Open a link | Ctrl+click |

Hold Shift to use local mouse selection when an application captures the mouse.
macOS uses Command+C and Command+V for clipboard actions.
See [daily workflow](docs/daily-workflow.md) and
[shell integration](shell/README.md) for the complete controls.

### Configuration

The default configuration is `${XDG_CONFIG_HOME:-~/.config}/ztty/config`.
Renderer selection is a configuration setting:

```ini
renderer = auto
```

X11 accepts `auto`, `gpu` and `software`. Native Wayland requires `auto` or
`gpu` with an EGL/GLES implementation; Mesa software GL is also supported.
GPU driver selection and fallback are reported by the actual renderer.
There is no `--renderer` command-line option.

### Shared sessions and MCP

```sh
ztty --name work
ztty attach work
ztty attach --observe --backend wayland work
ztty mcp stdio
```

The Linux broker owns the PTY, terminal state and history. Multiple views can
attach to one session, and X11 and Wayland windows can share the same broker.
MCP exposes nine terminal tools with session identity checks, bounded reads and
explicit input/control permissions. Read [sessions and MCP](docs/sessions.md)
before connecting a client.

## Platform scope

| Platform | Window and rendering path | Current scope |
| --- | --- | --- |
| Linux X11 / XWayland | X11; GPU or software | Shared sessions, attach, MCP, tabs and splits |
| Linux native Wayland | Wayland; EGL/GLES | Shared sessions, attach/observe, clipboard and IME; independent windows; tabs and splits are not yet supported |
| macOS | AppKit; CoreText / CoreGraphics | Native terminal; Linux broker features are not available |
| Windows | Win32; ConPTY | Native terminal; Linux broker features are not available |

Features and performance are verified against specific source commits, binaries
and workloads. See [performance](docs/performance.md),
[the shared session core](docs/session-core.md), and
[native Wayland](docs/native-wayland.zh-CN.md) for evidence and remaining limits.
Logical PTY throughput measures parsing progress; it does not measure physical
input-to-display latency.

## Development

```sh
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets --features wayland -- -D warnings
```

The workspace contains `ztty`, `zterm`, `zpty`, `zinput`, `zgraphics`,
`ztty-ui` and `ztty-plugin-api`. The reusable terminal libraries support
`no_std`; platform I/O and the native application are kept at the boundary.
Local GUI acceptance starts at `tools/native_ui_acceptance.py`.

MIT license. See [LICENSE](LICENSE).
