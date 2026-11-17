# zterm

A bounded terminal screen and VT state machine in Rust. `zterm` converts bytes
from a process into character cells, cursor state, protocol replies, and input
modes. A host application supplies the PTY, event loop, font rendering, window,
and clipboard policy.

The production library is `no_std` with `alloc` and forbids unsafe code. It uses
[`zvte`](https://crates.io/crates/zvte) for streaming parsing,
[`zinput`](https://crates.io/crates/zinput) for input encoding,
[`zgraphics`](https://crates.io/crates/zgraphics) for static Kitty graphics, and
[`unicode-width`](https://crates.io/crates/unicode-width) for character widths.
Default features are disabled on every dependency. Applications must provide an
allocator; tests and the development benchmark use `std`.

## Use independently

```toml
[dependencies]
zterm = "0.2.0"
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

## Runtime themes

`Terminal::set_theme` applies configured default colors and the 16 ANSI palette
entries to retained cells in place. It updates the live screen, scrollback,
alternate screens, and saved SGR styles, then marks every screen row dirty.
`Terminal::theme()` returns the current configuration. Applying an identical
theme does no work; changing one does not allocate or restart the child process.

```rust
use zterm::Terminal;

let mut terminal = Terminal::new(80, 24, 2_000);
terminal.feed(b"normal \x1b[31mred\x1b[0m");
let mut theme = terminal.theme();
theme.foreground = 0xe8eaed;
theme.background = 0x000000;
theme.palette[1] = 0xff6666;
terminal.set_theme(theme);
assert_eq!(terminal.cell(0, 0).fg, 0xe8eaed);
assert_eq!(terminal.cell(7, 0).fg, 0xff6666);
assert!(terminal.row_dirty(0));
```

SGR default colors and ANSI palette references follow the new configuration.
Explicit truecolor and indexed colors 16 through 255 keep their values, even
when their RGB values happen to equal an old default or palette entry. Color
provenance occupies existing cell padding and flag bits; `Cell` remains 28 bytes
and `fg`/`bg` remain resolved RGB values for renderers. The frontend still owns
window padding, cursor colors, and redraw scheduling. OSC 4/10/11 queries report
the current colors with their original terminator; OSC color writes remain
unsupported. The SGR defaults and OSC query format follow the
[xterm control-sequence reference](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html).

## Shell command metadata

OSC 133 A/B/C/D marks expose prompt, input, output, and completion boundaries
through `commands()`. Records have stable session-local IDs, absolute history
coordinates, and optional exit codes. `command_input(id, max_bytes)` and
`command_output(id, max_bytes)` extract current rendered text from retained
primary-screen cells; `command_scroll_offset(id)` locates a retained prompt.
The result reports completion and truncation explicitly.

```rust
use zterm::Terminal;

let mut terminal = Terminal::new(80, 24, 2_000);
// These marks normally arrive from the configured shell through the PTY.
terminal.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07printf hello\r\n\x1b]133;C\x07hello\r\n\x1b]133;D;0\x07");
let command = terminal.commands().next_back().unwrap();
assert_eq!(command.exit_status, Some(0));
let output = terminal.command_output(command.id, 4_096).unwrap();
assert_eq!(output.text, "hello\n");
assert!(output.complete && !output.truncated);
assert_eq!(terminal.command_scroll_offset(command.id), Some(0));
```

Use `commands().rev()` to locate recent executed commands and copy their IDs
before mutating the terminal. A missing command ID or output boundary returns
`None`. A returned `CommandText` may be incomplete while the process is running,
or truncated by the requested byte limit, history eviction, or destructive
screen edits. Check both fields before labeling a copied result complete.
`command_scroll_offset` returns a viewport offset that can be passed to
`view_cell`; it returns `None` for unavailable prompts or an active alternate
screen. Offsets should be queried again after more terminal output arrives.

Metadata is allocated on the first marker and retains at most 128 records.
Reads return at most 64 KiB of UTF-8 without duplicating the whole history.
Scrolling keeps retained anchors stable. History eviction, width changes,
and destructive screen operations can make a range unavailable; a reset clears
records without reusing IDs. These are untrusted display markers, not proof
of command identity or authorization. See the
[shell setup and acceptance guide](../../shell/README.md).

## Static graphics

Call `set_cell_size(width, height)` whenever physical text-cell metrics change.
This supplies the geometry used by image placements and CSI pixel-size replies.
`graphics_draws(history_offset)` returns an allocation-free iterator over the
current screen's visible image placements, in z-order. Each item borrows a
straight-alpha RGBA image and specifies source and destination rectangles.
The frontend performs scaling, viewport clipping and alpha compositing. Negative
z-indices are drawn behind text; values below -1,073,741,824 also go behind cells
with explicit background colors. The graphics types are re-exported as
`zterm::graphics`.

PTY inline RGB, RGBA and PNG are supported, with optional zlib compression,
capability queries, chunked transfers, image IDs/numbers, placement IDs, cropping,
aspect-preserving scaling, z-order and deletion. PNG includes palette, grayscale,
transparency, 16-bit and Adam7 input. Nonessential metadata is discarded before
decoding; invalid CRCs, oversized dimensions and decompression-limit failures
produce protocol errors. Filesystem/shared-memory transports, animation,
Unicode placeholders and relative placements are not advertised as supported.

Primary and alternate screens own independent bounded image scenes. Images move
with full-screen scrolling and remain available within retained history. Resizing
keeps their row anchors in step with text; clearing the display removes visible
placements. Dirty rows include graphics changes, so deleting an image causes the
underlying text and background to be repainted. A new non-continuation command or
deletion aborts a previous incomplete transfer, except that feature queries leave
it intact. Normal text between completed APC chunks
continues to render. An unterminated APC can be cancelled by CAN, SUB or a new ESC
sequence. APC headers are capped and encoded payloads stream through a fixed-size
staging buffer. Larger single-APC transfers emitted by existing `kitten icat`
versions and unpadded final base64 are accepted without retaining the whole
encoded body.

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
| Shell command records | 128, allocated lazily |
| Command-text read | 65,536 UTF-8 bytes |
| Graphics APC staging | At most a 512-byte header plus 4,096 encoded payload bytes; larger bodies stream through it |
| Decoded graphics | 16 MiB per image; 32 MiB per screen, counting vector capacity |
| Pending graphics transfer | 16 MiB per screen; one transfer at a time |
| Graphics dimensions | At most 8,192 pixels in either dimension, also subject to byte limit |
| Stored images / placements | 128 / 512 per screen |
| Combining scalars | Two inline scalars per character cell |

The history byte limit covers the backing cell vectors. Screen arrays,
container metadata, parser state, titles, hyperlinks, and queued responses are
additional bounded allocations. Passing a history limit of zero disables
scrollback allocation.
Image decoding uses additional bounded temporary storage. Renderer copies and
buffers retained through `Arc` are outside the model's decoded-storage quota;
frontends must bound and release those resources separately.

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
  DECRQM mode queries, character/text-area/cell pixel-size queries, and basic XTGETTCAP queries for
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
[Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/),
[synchronized output](https://github.com/contour-terminal/vt-extensions/blob/master/synchronized-output.md),
and [OSC 8 hyperlinks](https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda).

## Current boundaries

Resizing preserves the cursor row and crops columns that no longer fit; it
does not reflow paragraphs or historical lines. A cell holds a base character and
up to two additional combining scalars. Full grapheme-cluster layout, complex
emoji ZWJ sequences, bidirectional shaping, Sixel, image animation, and OSC 52
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

## Sixel parsing

`Terminal::feed` decodes bounded Sixel images from DCS streams and exposes them
through `graphics_draws`, alongside Kitty images. DECSDM 80, private/shared
palette mode 1070 and cursor-right mode 8452 are queryable with DECRQM. Sixel
input commits only at ST; cancellation drops the partial decoder without moving
the cursor or publishing pixels. The [graphics protocol scope](../../docs/protocols.md#sixel)
distinguishes tested decoding/cursor behavior from framebuffer and native-GUI
compatibility limits.
