# Protocol scope

The implementation follows public protocol documentation. The table describes
implemented scope, with narrower behavior called out explicitly. The acceptance
coverage below distinguishes implementation from environments actually tested.

| Protocol / behavior | Scope |
| --- | --- |
| ANSI / VT controls | Cursor motion, save/restore, tabs, scrolling regions, erase, insert/delete, main/alternate screen, DEC line drawing |
| SGR | 16/256/RGB colors, semicolon and colon parameters, core attributes; renderer uses its configured font face |
| UTF-8 | Streaming decode across arbitrary chunks; CJK widths; two inline combining scalars |
| Kitty keyboard | Flags 1/2/4/8/16, event reports, shifted alternatives, associated text, independent screen stacks; the frontend does not supply a base-layout alternative |
| Mouse | DEC 1000/1002/1003 tracking; legacy and SGR 1006 encoding |
| Focus | DEC 1004 |
| Bracketed paste | DEC 2004; bounded paste with control-character sanitization |
| Synchronized output | DEC 2026, DECRQM, retained last frame, 150 ms recovery timeout |
| OSC 0/2 | Window title |
| OSC 7 | Working-directory metadata retained in the model |
| OSC 8 | Cell identity, history retention, underline, explicit Ctrl+click opening |
| OSC 4/10/11 | Color queries; reply with the request's BEL or ST terminator |
| DSR / DA / DECRQM | Implemented state reports; unknown modes report unsupported |
| XTGETTCAP | TN=xterm-256color, Co=256, Tc; unknown capabilities report failure |
| X11 clipboard | Clipboard and primary selection, UTF-8 and Latin-1 fallback, INCR receiving |

OSC 52, Sixel, Kitty graphics, native Wayland text-input protocols, complex
script shaping, full emoji ZWJ grapheme layout, and history reflow are outside
the current release. Each cell can retain two combining scalars; additional
scalars are ignored. Core state can retain an italic attribute, while the
initial renderer uses the configured face. Blink does not add a redraw timer.

## Acceptance coverage

The automated suite has passed 11 checks under an isolated Xvfb/X11 server:
legacy keyboard input; Kitty disambiguation; all five Kitty flags with modifier,
repeat, and release events; resize and SIGWINCH; ASCII/CJK selection and clipboard;
bracketed paste with control stripping; a 139,264-byte paste under backpressure;
SGR mouse input; Shift gesture ownership; explicit OSC 8 opening; and synchronized
output across writes and Expose events. The large-paste assertion includes the
12 bytes of bracketed-paste delimiters, for 139,276 delivered bytes.

Unit tests additionally cover screen state and query semantics. The current GUI
suite does not exercise a real IBus/Fcitx XIM session, a deliberately chunked INCR
clipboard transfer, or an actual Wayland/XWayland desktop session. These remain
separate integration acceptance tasks.

## References

- [xterm control sequences](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)
- [Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)
- [Synchronized output](https://github.com/contour-terminal/vt-extensions/blob/master/synchronized-output.md)
- [OSC 8 hyperlinks](https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda)
- [Unicode width rules](https://docs.rs/unicode-width/0.2.3/unicode_width/)
- [zvte parser API](https://docs.rs/zvte/0.1.3/zvte/)
