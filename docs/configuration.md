# Configuration

ztty reads a UTF-8 file with one `key = value` per line.
Lines beginning with `#` are comments. Omitted keys use built-in defaults.

## File locations

| Platform | Default path |
| --- | --- |
| Linux | `~/.config/ztty/ztty.conf` |
| macOS | `~/Library/Application Support/ztty/ztty.conf` |
| Windows | `%APPDATA%\ztty\ztty.conf` |

On Linux and macOS, setting `XDG_CONFIG_HOME` selects
`$XDG_CONFIG_HOME/ztty/ztty.conf`. `ztty --config PATH` selects a file explicitly.
`--font` overrides the configured font.

The Linux X11 backend reloads the file when it changes, including atomic saves.
Ctrl+Shift+R reloads it manually. Invalid changes leave the previous configuration
active and show a temporary error. macOS and Windows read settings at startup.

## Fonts, colors and cursor

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
palette = 1=#e06c75
palette = 4=#61afef
```

| Setting | Values |
| --- | --- |
| `window-padding` | 0–128 pixels |
| `background-opacity` | 0–100 percent |
| `background-blur` | `true` or `false` |
| `cursor-shape` | `block`, `underline`, `bar` |
| `cursor-blink` | `true` or `false` |
| `cursor-blink-interval` | 100–5000 ms per phase |
| `palette` | `INDEX=#RRGGBB`, with index 0–15 |

Terminal applications can change the active cursor through DECSCUSR and
DEC private mode 12.

On Linux X11, opacity applies to the default background; explicit ANSI backgrounds,
selection and glyph colors retain their own opacity. Blur is supplied by the
compositor. `background-blur` sends the KWin-compatible X11 request; Hyprland
uses its own window rules. Set opacity to 100 for an opaque window.

macOS uses a native visual-effect view for blur. Windows uses an opaque GDI
surface. The platform backends implement these settings through different
native APIs.

## Linux X11 renderer

The current source supports these values:

| Setting | Behavior |
| --- | --- |
| `renderer = auto` | Default; try hardware EGL, use Xft when only software GL is available |
| `renderer = gpu` | Use EGL/GLES, including software GL drivers |
| `renderer = software` | Use Xft/XRender |

The GPU path uses HarfBuzz for shaping and FreeType/Fontconfig for fonts.
Linux X11 can switch renderer through a configuration reload while keeping its
window and PTY. A GPU failure schedules a complete software redraw.
See the [renderer verification](reports/x11-gpu-20261119/report.zh-CN.md)
for tested text and frame behavior.

## Native Wayland (experimental)

This section describes the backend included in ztty 0.5.2; its published
artifacts and acceptance are recorded in the
[release report](releases/0.5.2/report.zh-CN.md).

Native Wayland uses xdg-shell, EGL/GLES and XKB, with the same Linux session
broker and PTY ownership as X11. Enable the Cargo feature when building from
source. The official Linux package build enables it; runtime startup defaults
to X11.

Install these dependencies in addition to the Linux build dependencies in the
[README](../README.md#build-and-run):

```sh
# Arch Linux
sudo pacman -S --needed wayland wayland-protocols libxkbcommon

# Debian / Ubuntu
sudo apt install libwayland-dev libwayland-bin wayland-protocols libxkbcommon-dev
```

A separate configuration such as `wayland.conf` can select the renderer:

```ini
font = monospace:pixelsize=16
renderer = gpu
window-padding = 8
background-opacity = 100
```

```sh
cargo build --locked --release --features wayland
./target/release/ztty --backend wayland --config ./wayland.conf --name work
./target/release/ztty attach --backend wayland --observe work
./target/release/ztty attach --backend wayland work
```

Each session has its own PTY. Detach and reattach preserve the session;
X11 and Wayland views can coexist on one broker. MCP retains the existing
input ownership checks. Tabs, splits and workspace containers are rejected
on Wayland; open separate session windows instead.

Clipboard and primary selection use bounded asynchronous transfers.
Text-input-v3 handles focus, cursor position, visible preedit and UTF-8 commits.
The terminal does not own the shell's editable buffer, so it does not invent
surrounding text or turn IME deletion requests into arbitrary backspaces.

Both `renderer = auto` and `gpu` use EGL/GLES on Wayland; `software` is rejected.
Mesa software GL still uses that EGL/GLES path. Renderer selection is a
configuration setting, not a `--renderer` command-line option.

The [shared Wayland report](reports/native-wayland-shared-20261119/README.zh-CN.md)
records the earlier feature-owner candidate. The
[0.5.2 release report](releases/0.5.2/report.zh-CN.md) records 65 Wayland checks
for each released Linux architecture, bound to its own source and ELF. Those
checks use headless Sway/software EGL and an IME protocol peer.
Physical GPU behavior and real desktop IME combinations remain unverified.
Output-scale and fractional-scale protocol handling are not implemented;
HiDPI and cross-monitor scale changes are therefore not accepted features.
Native Wayland also remains outside the supported `ztty reload` handover scope.

## Key and mouse bindings

```ini
keybind = ctrl+shift+x = search
keybind = alt+f12 = sessions
keybind = ctrl+shift+t = none
mousebind = ctrl+button3 = palette
```

A custom binding takes priority over the default for that chord.
`none` consumes the chord. Supported modifiers are `ctrl`, `shift`,
`alt` and `super`; the command palette shows active shortcuts.
See [binding names and input rules](daily-workflow.md#custom-keys-and-mouse-buttons).

## Text actions on Linux X11

`ztty-plugin-api` 0.1.0 is published; the
[registry verification record](reports/plugin-api-registry-20261120/README.md)
records the archive and independent consumer checks. The host and shared API
both use [zplugin 0.3.1](https://crates.io/crates/zplugin/0.3.1);
the locked graph includes [zsys 0.1.1](https://crates.io/crates/zsys/0.1.1).
These library releases are separate from the ztty application release.

Register a trusted native library with:

```ini
text-plugin = dedent=/absolute/path/libdedent.so
```

Open Ctrl+Shift+P and choose **Copy using dedent**. The action transforms the
selection, or the visible text when there is no selection, and copies the result.
The [text extension guide](text-plugins.zh-CN.md) documents the interface,
worker limits and an independently buildable example.
