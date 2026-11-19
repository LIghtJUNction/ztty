# Protocol scope

Version 0.2 implements the following subset of public terminal protocols.
The shared core is used by Linux, macOS, and Windows; native window integration
and its verification status are described separately below.

## Text and input protocols

| Protocol / behavior | Scope |
| --- | --- |
| ANSI / VT controls | Cursor motion, save/restore, tabs, scrolling regions, erase, insert/delete, main/alternate screen, DEC line drawing |
| SGR | 16/256/RGB colors, semicolon and colon parameters, core attributes; renderer uses its configured font face |
| UTF-8 | Streaming decode across arbitrary chunks; CJK widths; two inline combining scalars |
| Kitty keyboard | Flags 1/2/4/8/16, event reports, shifted alternatives, associated text, independent screen stacks; the Linux frontend does not supply a base-layout alternative |
| Mouse | DEC 9 (X10)/1000/1002/1003 tracking; legacy and SGR 1006 encoding |
| Focus | DEC 1004 |
| Cursor appearance | DECSCUSR block, underline, and bar with steady or blinking variants; DEC private modes 12 and 25 |
| Bracketed paste | DEC 2004; bounded paste with control-character sanitization |
| Synchronized output | DEC 2026, DECRQM, retained last frame, 150 ms recovery timeout |
| OSC 0/2 | Window title |
| OSC 7 | Working-directory metadata retained in the model |
| OSC 8 | Cell identity, history retention, underline, explicit Ctrl+click opening |
| OSC 4/10/11 | Color queries; reply with the request's BEL or ST terminator |
| DSR / DA / DECRQM | Implemented state reports; unknown modes report unsupported |
| Window and cell size | CSI 14 t reports text-area pixels; CSI 16 t reports cell pixels; CSI 18 t reports character dimensions |
| XTGETTCAP | TN=xterm-256color, Co=256, Tc; unknown capabilities report failure |
| X11 clipboard | Clipboard and primary selection, UTF-8 and Latin-1 fallback, INCR receiving |

Each cell can retain two combining scalars; additional scalars are ignored.
Core state retains italic attributes, while the Linux renderer uses its configured
font face. Text blink attributes are not animated. Cursor blinking is separate
and runs while the cursor is focused and visible. Resizing preserves and crops
rows, keeps the cursor visible, and can move displaced primary-screen rows into
history; it does not reflow historical lines.

Primary DA uses `CSI ? 62 ; 4 c`: a VT220-style identification with the Sixel
feature field. No other optional DA features are asserted. This also satisfies
the minimum response length used by [Kitty's graphics probe](https://github.com/kovidgoyal/kitty/blob/master/kittens/icat/detect.go).
Kitty graphics support and transport choices are established by their own queries.

## Kitty graphics

[`zgraphics`](../crates/zgraphics/README.md) implements the static image lifecycle:
receive image data, store decoded pixels, place an image, and delete its placements
or data. [`zterm`](../crates/zterm/src/terminal.rs) frames APC sequences and attaches
placements to terminal content. The native frontend composites the resulting
RGBA images; no Kitty renderer is embedded.

| Feature | Implemented scope |
| --- | --- |
| Transport | Direct PTY data, `t=d`; base64 encoded |
| Pixel formats | `f=24` RGB, `f=32` RGBA, `f=100` PNG; canonical storage is straight-alpha RGBA8 |
| Compression | Optional zlib, `o=z`, with bounded output |
| Actions | Capability query `a=q`, transmit `a=t`, transmit and display `a=T`, place `a=p`, delete `a=d` |
| Transfer chunks | `m=1` continuation and `m=0` final chunk; arbitrary PTY read boundaries; larger single APCs and repeated-action continuation headers from current clients |
| Base64 endings | Padded or unpadded final payload; nonfinal protocol chunks must contain complete groups without padding |
| Identity | Explicit image IDs `i`, image numbers `I`, automatic IDs, placement IDs `p`, replacement under an existing ID |
| Geometry | Source crop `x/y/w/h`, cell-relative offset `X/Y`, requested columns/rows `c/r`, viewport clipping |
| Scaling | Aspect ratio preserved; when both `c` and `r` are set, the image is centered within that box |
| Cursor | Placement advances the cursor according to its occupied box; `C=1` leaves it unchanged |
| Layers | `z >= 0` above text; negative `z` behind text; `z < -1073741824` also behind explicitly colored backgrounds |
| Deletion | Visible placements, ID/number/placement, cursor or cell, row, column, z-index, and ID range selectors; uppercase forms free selected data once unreferenced |
| Replies | Success/error responses for identified requests; `q=1` suppresses success, `q=2` suppresses all replies; queries do not place or retain images |
| Content lifetime | Full-screen scroll can retain images in history; partial-region scroll moves placements anchored in that region; primary and alternate screens have independent scenes |

An image appears only after its final transfer completes successfully. Replacing
an image ID removes the old placements; changing a placement ID replaces that
placement. Cell metrics supplied by the frontend determine pixel geometry and
size replies. Both image IDs and their generation values are used when deciding
whether a renderer's cached pixels are still current.

Existing `kitten icat` clients can emit image payloads larger than 4096 encoded
bytes in one APC and use unpadded base64. The streaming receiver accepts that
shape while retaining at most a 512-byte header and a 4096-byte encoded staging
buffer. It incrementally decodes into a separately bounded pending transfer.
This avoids accumulating an entire encoded APC in a general control-string
buffer. Client emission behavior can be inspected in
[Kitty's graphics command writer](https://github.com/kovidgoyal/kitty/blob/master/tools/tui/graphics/command.go).

### Image limits and recovery

These are default limits per graphics scene, as defined by
[`zgraphics::Limits`](../crates/zgraphics/src/lib.rs):

| Resource | Default bound |
| --- | --- |
| Width or height | 8192 pixels |
| One decoded image | 32 MiB |
| Retained decoded image storage | 32 MiB, counting vector capacity |
| Pending transfer | 16 MiB |
| Stored images | 128 |
| Placements | 512 |

Both dimensional and byte limits apply. A nominally permitted width and height
can still exceed the decoded-image byte limit. Primary and alternate scenes
have separate quotas; these figures are not a process-wide memory ceiling.
PNG decoding uses temporary buffers, and frontends retain display resources.
The [memory accounting section](architecture.md#memory-accounting) explains those
additional costs. Empty scenes allocate no image storage.

PNG headers, chunk lengths, CRCs, and dimensions are checked before decoding;
nonessential metadata is removed. Malformed PNG, incorrect pixel lengths, bad
base64, unsupported options, and quota overruns are rejected. A pending `m=1`
transfer does not consume ordinary text between APCs. CAN, SUB, or a new escape
sequence can cancel an unfinished APC, so a missing terminator does not trap
subsequent escape sequences. Tests exercise valid text, keyboard input, and a
new valid image after rejection. The test sources are
[`zgraphics` tests](../crates/zgraphics/src/tests.rs),
[`zterm` tests](../crates/zterm/src/tests.rs), and
[the graphics acceptance suite](../tests/graphics_smoke.py).

### Deferred capabilities

| Area | Outside the current scope |
| --- | --- |
| Kitty transport | File paths, temporary files, shared memory |
| Kitty rendering extensions | Animation, Unicode placeholders, relative placements |
| Other image protocols | ReGIS |
| Clipboard and window protocols | OSC 52, native Wayland text-input protocols |
| Text layout | Full complex-script shaping, complete emoji ZWJ layout, history reflow |

Applications should use direct transmission and query capabilities instead of
inferring full Kitty compatibility from successful static-image display.

## Sixel

The shared `no_std` core accepts `ESC P ... q` Sixel images, commits only at ST,
and reuses the existing RGBA scene and native renderers. RGB/HLS color registers,
repeat compression, raster extents, aspect ratio, transparent/opaque backgrounds,
color overprinting, and arbitrary PTY chunk boundaries are implemented. Modes
80, 1070 and 8452 control display origin/scrolling, palette scope and cursor
placement. Default scrolling starts at the text cursor and ends at column zero
on the following text row; display mode preserves the cursor. Details and
resource limits are in [zgraphics](../crates/zgraphics/README.md#sixel).

Sixel images use the scene's behind-text layer and remain anchored to cell rows.
The implementation does not emulate a VT340's framebuffer for later mixed text
and image overpainting: text remains visible over images, and partial-region
scrolling follows the existing anchor policy. Shared palettes seed subsequent
images; changing a register does not recolor an already completed RGBA image.
These are explicit rendering limits, so successful decoding does not establish
complete VT340 compatibility.

Decoder validation uses the [DEC manual](https://vt100.net/docs/vt3xx-gp/chapter14.html),
the modern [xterm control reference](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html),
and an independent [libsixel pixel comparison](../tests/sixel_reference.py).
The [measurement](measurements/sixel-linux.json) records source/binary hashes,
chunk sizes, exact pixel results and reference-decoder discrepancies. Native GUI
presentation and macOS/Windows acceptance must be recorded separately.

## Acceptance coverage

Newer roadmap candidates are tracked separately in the
[native acceptance record](native-candidates-2026-11-18.md). At `2dc6769`,
Linux x86_64 passed four guarded GUI suites and Windows x86_64 passed 25 checks,
including six Sixel groups. The two ARM64 gates still failed; macOS only
completed its eight protocol checks. Historical tables and their immutable
source reports below retain their original scope. The new macOS capture gate
and native startup-directory additions require their own subsequent execution.

The [GUI suite](../tests/gui_smoke.py) has passed 12 checks under an isolated Xvfb/X11 server,
as recorded in [the GUI result](measurements/gui-graphics-2026-11-18.json):
legacy keyboard input; Kitty disambiguation; all five Kitty flags with modifier,
repeat, and release events; resize and SIGWINCH; ASCII/CJK selection and clipboard;
bracketed paste with control stripping; a 139,264-byte paste under backpressure;
SGR mouse input; Shift gesture ownership; explicit OSC 8 opening; configured ARGB background and cursor rendering; and synchronized
output across writes and Expose events. The large-paste assertion includes the
12 bytes of bracketed-paste delimiters, for 139,276 delivered bytes.

Unit tests also cover screen state and query semantics. The optional
`--fcitx` test runs Fcitx 5 with Pinyin on Xvfb and a private D-Bus session,
checking ordinary key forwarding and Chinese text committed to the PTY.
IBus and deliberately fragmented INCR transfers remain separate integration tests.

The [graphics suite](../tests/graphics_smoke.py) has passed all seven groups on
Linux/Xvfb through a real PTY and window-pixel readback, recorded in
[the graphics result](measurements/graphics-xvfb-2026-11-18.json):

1. Graphics capability and cell/window-size replies, with no image placed by a query.
2. RGB, RGBA, and PNG colors, including four alpha levels and split PNG transmission.
3. Chunked transmission, placement, soft deletion, image replacement, and data deletion.
4. Source crop, clipping to the text viewport, and images below explicit cell backgrounds.
5. Scrolling, window resize, primary/alternate isolation, and deletion without residual pixels.
6. Malformed images, excessive dimensions, invalid base64, interrupted transfers, and subsequent keyboard/image recovery.
7. Unmodified `kitten icat` 0.50 detecting `stream`, displaying a photo with explicit inline transport, and displaying a transparent-edge PNG through automatic detection.

The interrupted-transfer check observed **0 bytes of RSS growth** between its
before/after samples across 80 aborted uploads after earlier image tests warmed
the process. This is an observation for that run, not a measurement of peak
allocation or a general zero-growth guarantee. The test enforces a finite
growth allowance and records the actual value in its JSON result.

The [live Hyprland/XWayland run](measurements/graphics-hyprland-2026-11-18.json)
also passed on Hyprland 0.57.2: `kitten icat --detect-support` selected `stream`,
and the real tools displayed both the photo and transparent-edge PNG. Only the
owned test window was captured. The session's normal X0 pathname was absent;
the probe used and removed its own temporary socket alias to the existing
same-user XWayland endpoint, without changing compositor or authorization
settings. This was the real desktop server, separate from Xvfb.

Native artifact verification also passed in
[workflow run 44165748395](https://github.com/LIghtJUNction/ztty/actions/runs/44165748395),
on source `7535aa29f1a2728a78a446ffea6784dcbc2aae92`:

| Native environment | Observed coverage | Evidence |
| --- | --- | --- |
| macOS Intel | 8 session/protocol checks through a real AppKit application and Darwin PTY: geometry, cursor reply, image query, chunked RGBA, PNG, alternate screen, malformed-input recovery, deletion | [Native report](measurements/ztty-macos-x86_64.native-7535aa2.json) |
| macOS Apple Silicon | The same 8 session/protocol checks through a native application | [Native report](measurements/ztty-macos-aarch64.native-7535aa2.json) |
| Windows x86_64 | 17 checks: native child/Unicode arguments, ConPTY query round trip, RGB/RGBA/PNG pixel readback, alpha, scrolling, clipping, resize, alternate screen, deletion, keyboard recovery | [Native report](measurements/ztty-windows-x86_64.native-7535aa2.json) |

Both macOS reports explicitly record `pixel_readback: not_performed` and
`ime_keyboard: not_performed`; those checks remain open. The Windows result uses
the actual packaged ConPTY runtime and native window pixels, with the binary
hash recorded in its report. This final native run includes the DA1 serialization
fix described above.
Formal packaging and native checks are defined in
[the release workflow](../.github/workflows/release.yml).

## References

- [xterm control sequences](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)
- [Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)
- [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
- [Kitty `icat`](https://sw.kovidgoyal.net/kitty/kittens/icat/)
- [ECMA-48 numeric parameter format](https://ecma-international.org/wp-content/uploads/ECMA-48_5th_edition_june_1991.pdf)
- [Synchronized output](https://github.com/contour-terminal/vt-extensions/blob/master/synchronized-output.md)
- [OSC 8 hyperlinks](https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda)
- [Unicode width rules](https://docs.rs/unicode-width/0.2.3/unicode_width/)
- [zvte parser API](https://docs.rs/zvte/0.1.3/zvte/)
