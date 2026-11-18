# Daily workflow on Linux

The Linux shared runtime keeps the PTY and terminal model independent of its
window. Search, command actions and configuration changes operate on that live
session; they do not restart its child. The native macOS and Windows frontends
do not yet implement these view actions or the shared runtime.

## Keyboard and command palette

| Shortcut | Action |
| --- | --- |
| Ctrl+Shift+F | Open history search; seed it with a single-line selection |
| Enter / Shift+Enter in search | Next / previous match, wrapping at the ends |
| F3 / Shift+F3 in search | Next / previous match |
| Ctrl+U or Ctrl+A in search/palette | Clear the query |
| Esc | Close the temporary search/palette |
| Ctrl+Shift+P | Open the command palette; type to filter, arrows to select, Enter to run |
| Ctrl+Shift+S | Find a session by name, title or URI; focus its view or explicitly attach |
| Ctrl+Shift+K / J | Previous / next retained shell command |
| Ctrl+Shift+Y | Copy the selected command's output; otherwise copy the latest output |
| Ctrl+Equal / Ctrl+Minus / Ctrl+0 | Increase, decrease or reset font size |
| Ctrl+Shift+R | Reload the session's configuration file |
| Ctrl+Shift+C / V | Copy selection / paste clipboard |
| Shift+PageUp / PageDown | Scroll history |

The palette appears only when requested. It uses the existing window and does
not send its query or action keystrokes to the child. It exposes the same named
actions as custom bindings and `ztty action NAME ACTION`. Filters accept several
case-insensitive words in any order, and action rows show a currently effective
shortcut, including custom overrides. Workspace shortcuts
are listed in [workspaces](workspaces.md).

The session switcher labels existing views and detached sessions separately.
Selecting an existing view focuses its actual window, tab and pane without
changing its PTY or input owner. A detached session requires an explicit Attach
entry. Observing views and sessions controlled by another actor remain read-only;
switching does not take input ownership. The selected URI distinguishes sessions
with the same display name. If a Focus target detaches while the list is open,
the action reports that change rather than silently creating a new view.

Search is case-sensitive, literal UTF-8 matching across rows joined by terminal
autowrap. Explicit line breaks remain boundaries. It highlights complete wide
and combining cells, including the visible portion of a match extending outside
the viewport. Queries are limited to 1,024 bytes; the streaming matcher retains
one query's positions, not a copy of the history. The highlight map uses one bit
per visible cell, at most 16 KiB for a 512×256 grid.

Copying a selection joins automatic line wraps while retaining explicit newlines.
Wide-glyph wrap padding is omitted, meaningful spaces are retained, and UTF-8
cells remain whole within the 4 MiB selection limit. An active search can copy
its complete match even when it spans more than a viewport. Output or a resize
invalidates saved match coordinates; copying refreshes the search first.

Search does not provide regular expressions. Current column width bounds the
searchable part of older rows; a clipped soft wrap stops being a logical link.
Match counts refresh when editing, navigating or copying the query. Full history
reflow remains separate work. Native macOS and Windows share the selection-copy
rules, while their search and command palette interfaces remain unimplemented.

## Returning to live output

Scrolling into history shows a small **Back to live ↓** control near the lower
right corner. Clicking it returns to the newest output and hides the control.
The terminal keeps its full row count; there is no permanent toolbar at the live
screen. Shift+PageUp/PageDown and the existing scroll actions remain available.

When the child produces more output while you are reading history, the visible
rows stay anchored and the control changes to **New output · Back to live ↓**
with a blue border. The indicator remains until the view returns to the bottom.
Scrolling up again starts with the ordinary label, so an earlier output arrival
does not appear as a new notification.

The control sits above an open search row or command palette. Clicking it keeps
that interface open and preserves its input mode. It also works in an observing
view without acquiring control or resizing the child. Its mouse press, drag and
release stay within the view controls, including when the application requests
mouse reports. In narrow views the label becomes **Live ↓** or **↓**; the control
is hidden if a complete target cannot fit above the active interface.

## Shell command regions

Enable the appropriate script using [shell/README.md](../shell/README.md).
`ztty shell-integration bash|zsh|fish|nu` prints the packaged script, so it can be
loaded explicitly without changing unrelated shell configuration. The scripts
send OSC 133 prompt/input/output/end marks and OSC 7 working-directory metadata.

```sh
ztty commands work
ztty command-output work 1
```

The CLI returns JSON. Command-output responses contain `text`, `complete`,
`truncated` and `command_id`. MCP additionally accepts `part: "input"` and a
`max_bytes` bound through `session_command_read`. At most 128 metadata records
are retained; extracted text is limited to 65,536 UTF-8 bytes. Copying through
the GUI refuses truncated output, so an incomplete region is not silently
presented as a complete command result.

Command regions point into the existing terminal history. They are rendered
text, not immutable stdout, and physical lines become newline separators.
Overwriting/erasing text, changing the width of a live region or exhausting
history can invalidate a region. Navigation skips unavailable records. Already
retained, intact historical commands remain usable after a font change.

## Bash hook acceptance

The Bash integration preserves existing PROMPT_COMMAND strings or array entries, their exit-status behavior, and an inherited PS0 suffix. Repeated sourcing and nested shells must not duplicate execution marks. The dedicated PTY gate is part of the local Linux check; the four-shell gate also checks Bash syntax and the first source result independently of the second. [Bash regression, old-script failure and exact versions](shell-resume-2026-11-18.md) record the current evidence. Bash before 5.1 has not been executed in this acceptance run.

## Custom keys and mouse buttons

Bindings are lines in the same configuration file as fonts and colors:

```ini
keybind = ctrl+shift+x = search
keybind = ctrl+shift+p = palette
keybind = ctrl+shift+t = none
mousebind = ctrl+button3 = palette
```

Modifiers are `ctrl`, `shift`, `alt`, `super`. Keys include ASCII characters,
`f1`–`f35`, `enter`, `escape`, `tab`, `backspace`, `space`, arrows, `home`, `end`,
`pageup`, `pagedown`, `equal`/`plus`, and `minus`. Mouse buttons are `button1`–
`button11`. Caps Lock and Num Lock do not change binding matching. Custom bindings
replace defaults for the exact chord. `none` consumes a chord without running an
action. Each matched press and its release are withheld from application input,
including buttons while an application has enabled mouse reports. There are at
most 128 custom chords; unknown actions and malformed chords reject the config.

The palette lists action descriptions and effective shortcuts; exact binding names are in
[src/actions.rs](../src/actions.rs). Selection, copy and navigation remain
available while observing; paste requires local input ownership.

In ordinary shell mode, Caps Lock, Num Lock and Scroll Lock change keyboard state
without sending private-use escape sequences. Programs that explicitly enable
the enhanced keyboard protocol still receive negotiated lock-key events. With
Num Lock enabled, keypad digits type numbers. With it disabled, keypad zero is
the standard Insert key (`ESC [ 2 ~`), whose handling belongs to the application.

## Live configuration

The session remembers its explicit `--config` path, otherwise the default config
path. Linux inotify watches the containing directory, including atomic saves.
It follows replacement/recreation of that directory through the nearest
existing ancestor and releases watches after their last session closes.
There is no periodic idle polling. Manual reload is Ctrl+Shift+R.

Valid changes update fonts, padding, theme, cursor and selection colors, custom
bindings and supported opacity/blur settings in the existing window. Invalid
configuration retains the last valid state and shows a temporary error line.
Font validation and PTY resizing precede the live appearance commit. A window
created with an opaque visual needs detach/attach before enabling a supported
transparent visual; an X server without an alpha visual retains its opaque
fallback. Theme changes preserve explicit RGB colors and update default/ANSI
colors using compact source flags in the existing 28-byte cell.

Font reset returns to the latest successfully loaded config font. A font change
preserves the PTY PID and window. Observing views cannot resize the child; their
window appearance can still change independently.

## Reproducible checks

```sh
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --release --locked
python3 tests/daily_workflow.py --binary ./target/release/ztty
python3 tests/session_switcher.py --binary ./target/release/ztty
python3 tests/input_gui.py --binary ./target/release/ztty --source-root .
python3 tests/install_test.py
python3 tests/mcp_transport.py --binary ./target/release/ztty
python3 tests/mcp_sessions.py --binary ./target/release/ztty
python3 tests/mcp_security.py --binary ./target/release/ztty
cargo build --locked -p zterm --example shell_probe
python3 tests/shell_integration.py
python3 tests/bash_integration.py
```

The daily workflow test uses private Xvfb, XTEST key/button events, a real raw
PTY, actual clipboard transfers and pixel captures. It records binary/source
hashes and cleans only its own processes. Its coverage is Linux X11; hardware
GPU, native Wayland, macOS/Windows feature parity and IME accessibility are
independent acceptance gates.

The current harness also exercises the scrollback return control with real
mouse input and additional PTY output. It checks history anchoring, clearing the
new-output indication, search/palette coexistence and read-only ownership. Its
screenshots include `scrollback-return`, `scrollback-new-output`,
`scrollback-live`, `scrollback-search`, `scrollback-palette` and
`scrollback-observer`. The archived run below predates these additional checks.

The 2026-11-18 run passed all 12 daily GUI scenarios and the existing workspace
GUI regression. [GUI evidence](measurements/daily-workflow-x11-2026-11-18.json),
[final build/Clippy/no_std log](measurements/daily-workflow-gates-2026-11-18.txt)
and [stdio/HTTP ownership/security evidence](measurements/mcp-roadmap-2026-11-18.json)
record the exact binary and source hashes. Earlier failed checks led to the
invalid-region navigation fix and the visible config-error diagnostic; the
linked final records are the passing reruns.

![Actual history search](assets/daily-workflow/history-search.png)

![Actual temporary command palette](assets/daily-workflow/command-palette.png)

## Configuration-directory race regression

A real GUI run exposed a missed reload when the config directory was renamed
and rebuilt. A replacement directory could appear after the direct watch failed
but before its ancestor watch was installed. If its file was then written after
reconnect, the ancestor did not receive that child-directory write.

Reconnect now installs the nearest existing ancestor watch before probing each
child directory again. It either reaches the new parent or keeps the watch that
will report its future creation. This adds no polling timer or permanent extra
watch. Shared descriptors remain alive until their last configured path is removed.

[Three real inotify tests](../tests/config_watch.rs) schedule this filesystem
race precisely and use the real libc watch calls. The old implementation fails
both race cases; the fixed implementation passes all three cases. The third
case checks shared-descriptor retention and final release. See the
[source/test hashes and comparison record](measurements/config-watch-regression-2026-11-18.json),
[old failure log](measurements/config-watch-before-fix-2026-11-18.log), and
[fixed trace](measurements/config-watch-after-fix-2026-11-18.log).
The normal local gate now runs all Rust integration tests and MCP security tests.
