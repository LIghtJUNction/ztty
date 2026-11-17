# zterm

A bounded terminal screen and VT state machine in Rust. `zterm` converts bytes
from a process into character cells, cursor state, protocol replies, and input
modes. A host application supplies the PTY, event loop, font rendering, window,
and clipboard policy.

The production library is `no_std` with `alloc` and forbids unsafe code. It uses
[`zvte`](https://crates.io/crates/zvte) for streaming parsing,
[`zinput`](https://crates.io/crates/zinput) for input encoding, and
[`unicode-width`](https://crates.io/crates/unicode-width) for character widths.
Default features are disabled on every dependency. Applications must provide an
allocator; tests and the development benchmark use `std`.

## Use independently

```toml
[dependencies]
zterm = "0.1.0"
```

```rust
use zterm::{Terminal, input::{self, Key, KeyEvent, Modifiers}};

let mut terminal = Terminal::new(80, 24, 2_000);
terminal.feed(b"\x1b[38;2;99;179;237mhello\x1b[0m");
assert_eq!(terminal.cell(0, 0).ch, 'h');
assert_eq!(terminal.cell(0, 0).fg, 0x63b3ed);

// Paint only the rows marked dirty, then acknowledge the rendered frame.
assert!(terminal.row_dirty(0));
terminal.clear_dirty();

// A child can negotiate input modes through its output stream.
terminal.feed(b"\x1b[?1h");
let encoded = input::encode_key(
    Key::Up,
    Modifiers::default(),
    KeyEvent::Press,
    "",
    &terminal.modes().input(),
);
assert_eq!(encoded, b"\x1bOA");

// Write response bytes back to the child before clearing the queue.
terminal.feed(b"\x1b[6n");
assert_eq!(terminal.responses(), b"\x1b[1;6R");
terminal.clear_responses();
```

`feed` accepts arbitrary chunk boundaries, including split UTF-8 characters and
control sequences. `cell` addresses the live screen; `view_cell` addresses the
same viewport with a scrollback offset. Both return a blank cell for invalid
coordinates. `resize` clamps dimensions to `1..=1024`, keeps the cursor row
visible, and marks all rows dirty. Primary-screen rows removed from the top
while shrinking are retained within the configured scrollback limits.

`Modes::input()` snapshots the negotiated keyboard, mouse, focus, and paste
modes into the independent `zinput::Modes` value. The input crate is re-exported
as `zterm::input`; it is also usable without this terminal model.

## Screen and storage

The visible screen is a flat cell array with a circular row origin. Full-screen
scrolling rotates that origin and clears the recycled row. A partial scrolling
region moves only its own rows. Dirty bits are tracked per row, including the
old and new cursor rows.

Scrollback is allocated as output arrives and is limited by both the requested
line count and an 8 MiB cell-storage budget. Once full, old line buffers are
reused. The alternate screen is created on first use and retained for reuse.
It does not append to scrollback; the primary screen's existing history is
preserved while the alternate screen is active.

Additional bounds are explicit:

| Storage | Bound |
| --- | --- |
| Screen dimensions | 1–1024 columns and 1–1024 rows |
| Requested history | At most 65,536 lines, subject to the 8 MiB cell budget |
| OSC parser payload | 4,096 bytes; overflowing commands are rejected |
| Pending protocol responses | 65,536 bytes |
| Hyperlink identities | 1,024 entries |
| Hyperlink URI | 2,048 UTF-8 bytes |
| Window title | 256 UTF-8 bytes |
| Working-directory value | 2,048 UTF-8 bytes |
| Kitty keyboard stack | 32 entries per screen |
| Combining scalars | Two inline scalars per character cell |

The history byte limit covers the backing cell vectors. Screen arrays,
container metadata, parser state, titles, hyperlinks, and queued responses are
additional bounded allocations. Passing a history limit of zero disables
scrollback allocation.

## Implemented protocol state

- Cursor positioning, saved cursor and attributes, scrolling regions, origin
  mode, delayed autowrap, insert/delete characters and lines, erasing, tabs,
  alternate screen, and DEC line-drawing characters.
- SGR attributes and 16-color, 256-color, and RGB colors. RGB accepts both
  semicolon and colon parameter forms; cells expose resolved RGB values.
- UTF-8, double-width characters, and two inline combining scalars. Overwriting,
  erasing, inserting, deleting, and resizing repair split double-width cells.
- OSC titles, working-directory metadata, indexed/default color queries, and
  OSC 8 hyperlinks attached to cells and retained in scrollback. The host
  decides whether to open a URI.
- Application cursor/keypad modes, bracketed paste, focus events, independent
  mouse tracking and SGR mouse encoding, and Kitty keyboard protocol flags with
  separate primary/alternate screen stacks.
- Synchronized-output mode, DSR cursor/status replies, device attributes,
  DECRQM mode queries, text-area size queries, and basic XTGETTCAP queries for
  `TN`, `Co`, and `Tc`. Unknown capabilities return an unsupported response.

The startup compatibility environment is `TERM=xterm-256color`, using the
standard system terminfo entry. The `TN` capability query returns that same
name; the library does not report a `ztty` terminal identity.

While synchronized output is enabled, the host should keep displaying its last
completed frame and continue feeding bytes to the model. After reset of mode
2026, the accumulated dirty rows can be rendered together. A host with a
documented timeout can call `force_end_sync()` to finish a stalled frame.

Protocol references:
[xterm control sequences](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html),
[Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/),
[synchronized output](https://github.com/contour-terminal/vt-extensions/blob/master/synchronized-output.md),
and [OSC 8 hyperlinks](https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda).

## Current boundaries

Resizing preserves the cursor row and crops columns that no longer fit; it
does not reflow paragraphs or historical lines. A cell holds a base character and
up to two additional combining scalars. Full grapheme-cluster layout, complex
emoji ZWJ sequences, bidirectional shaping, image protocols, and OSC 52
clipboard operations are outside the current implementation.

The library changes screen state and returns protocol bytes. It does not
perform I/O, invoke programs, render text, or open links.

## Validation and measurements

From the workspace:

```sh
cargo test -p zterm
cargo check -p zterm --target x86_64-unknown-none
cargo run -p zterm --release --example benchmark
```

The core tests cover fragmented streams, wide-cell repair, scroll regions,
bounded history, screen switching, color forms, hyperlink lifetime, oversized
OSC rejection, mode queries, keyboard stacks, malformed input, and response
queue limits. The benchmark reports parser-plus-screen throughput, sampled
latency, allocations after warmup, and initial core heap use. PTY scheduling,
rendering, compositor behavior, and physical display latency are separate
measurements.

## License

MIT. See [LICENSE](LICENSE).
