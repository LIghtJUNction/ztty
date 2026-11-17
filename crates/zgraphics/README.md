# zgraphics

Bounded Kitty Graphics Protocol processing for `no_std` terminals. The library
uses `alloc`, performs no operating-system calls, and does not select a window
system or renderer. All production Rust dependencies disable default features.

It accepts direct (`t=d`) RGB, RGBA and PNG image data, including zlib compression,
base64 transfers, capability queries, image IDs/numbers, source cropping,
placements, placement IDs, scaling, z-order and deletion. PNG decoding uses
`zune-png`; the container is checked before decoding and nonessential metadata is
removed. Animation, filesystem/shared-memory transfers, Unicode placeholders and
relative placements are explicitly unsupported.

```rust
use zgraphics::{Context, Graphics};

let mut images = Graphics::default();
let context = Context {
    col: 0, row: 0, cols: 80, rows: 24, cell_width: 8, cell_height: 16,
};
let mut replies = Vec::new();
images.command(b"Ga=T,i=1,f=24,s=1,v=1,C=1;/wAA", context, &mut replies);
let draw = images.draws(0).next().unwrap();
assert_eq!(draw.image.pixels.as_slice(), [255, 0, 0, 255]);
assert_eq!(replies, b"\x1b_Gi=1;OK\x1b\\");
```

`Graphics::command` accepts one protocol-sized APC body starting with `G`, without
the surrounding `ESC _` and `ESC \\`. For a byte stream, use `CommandStream`:
it retains at most a 512-byte header and 4096 encoded bytes, decoding larger bodies
incrementally into the bounded transfer. This also accepts existing clients such
as `kitten icat` which can emit an entire image in one APC. Both padded and
unpadded final base64 are accepted; an explicit nonfinal `m=1` chunk must still be
a multiple of four without padding. The host owns ESC/ST framing and passes
cursor/grid context. Images become visible only when `finish` receives ST.
`Graphics::draws` returns sorted, borrowed straight-alpha RGBA images and source
and destination rectangles. The renderer clips destinations to its viewport and
implements the documented z-order around text and explicit backgrounds.
Pixel buffers use `Arc<Vec<u8>>`; a native renderer can retain a buffer until its
graphics API has finished reading it. Renderers must release their retained
references and converted-image caches when no longer needed. The dependency on
`Arc` requires targets with pointer-width atomics.

Default limits per scene are 8192 pixels per dimension, 16 MiB per decoded image,
32 MiB total decoded storage, 16 MiB per pending transfer, 128 stored images and
512 placements. Limits are configurable. PNG decoding has additional temporary
buffers bounded by validated dimensions and compressed input; the decoded-store
quota is not a claim about peak process RSS. A terminal normally keeps independent
scenes for primary and alternate screens. No image storage is allocated while idle.
The decoded-storage accounting includes vector capacity, not only populated pixels.

`scroll`, `trim_history`, and `set_viewport` let the host keep placements attached
to terminal content. `revision` changes on scene mutations; `Image::generation`
distinguishes replacement pixel data under an existing ID for renderer caches.
Generation values are local to a scene; clear renderer caches when changing to a
different scene. Full-screen upward scrolls can retain images in history, while
partial-region scrolls move only placements whose anchors lie inside the region.

Protocol reference: <https://sw.kovidgoyal.net/kitty/graphics-protocol/>.
