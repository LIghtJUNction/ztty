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

Linux reloads the file when it changes, including atomic saves. Ctrl+Shift+R
reloads it manually. Invalid changes leave the previous configuration active
and show a temporary error. macOS and Windows read settings at startup.

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

On Linux, opacity applies to the default background; explicit ANSI backgrounds,
selection and glyph colors retain their own opacity. Blur is supplied by the
compositor. `background-blur` sends the KWin-compatible X11 request; Hyprland
uses its own window rules. Set opacity to 100 for an opaque window.

macOS uses a native visual-effect view for blur. Windows uses an opaque GDI
surface. The platform backends implement these settings through different
native APIs.

## Linux renderer

The current source supports these values:

| Setting | Behavior |
| --- | --- |
| `renderer = auto` | Default; try hardware EGL, use Xft when only software GL is available |
| `renderer = gpu` | Use EGL/GLES, including software GL drivers |
| `renderer = software` | Use Xft/XRender |

The GPU path uses HarfBuzz for shaping and FreeType/Fontconfig for fonts.
Linux can switch renderer through a configuration reload while keeping its
window and PTY. A GPU failure schedules a complete software redraw.
See the [renderer verification](reports/x11-gpu-20261119/report.zh-CN.md)
for tested text and frame behavior.

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

## Text actions on Linux

Register a trusted native library with:

```ini
text-plugin = dedent=/absolute/path/libdedent.so
```

Open Ctrl+Shift+P and choose **Copy using dedent**. The action transforms the
selection, or the visible text when there is no selection, and copies the result.
The [text extension guide](text-plugins.zh-CN.md) documents the interface,
worker limits and an independently buildable example.
