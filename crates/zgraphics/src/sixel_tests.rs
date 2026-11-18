use super::*;
use alloc::vec;

fn context() -> Context {
    Context {
        col: 0,
        row: 0,
        cols: 80,
        rows: 24,
        cell_width: 8,
        cell_height: 16,
    }
}

fn send(graphics: &mut Graphics, body: &[u8], transparent: bool) -> Option<SixelOutcome> {
    graphics.begin_sixel(7, transparent, [11, 22, 33], None);
    for &byte in body {
        graphics.sixel_put(byte);
    }
    graphics.finish_sixel(context())
}

#[test]
fn sixel_bits_repeat_and_color_overprint_are_exact() {
    let mut graphics = Graphics::default();
    let result = send(
        &mut graphics,
        b"\"1;1;3;6#1;2;100;0;0!3@#2;2;0;100;0$!3A",
        false,
    )
    .unwrap();
    assert_eq!((result.width, result.height), (3, 6));
    let draw = graphics.draws(0).next().unwrap();
    assert_eq!(&draw.image.pixels[..12], [255, 0, 0, 255].repeat(3));
    assert_eq!(&draw.image.pixels[12..24], [0, 255, 0, 255].repeat(3));
    assert_eq!(&draw.image.pixels[24..], [11, 22, 33, 255].repeat(12));
    assert_eq!((draw.dest.width, draw.dest.height), (3, 6));
}

#[test]
fn sixel_transparency_raster_extent_and_growth_preserve_pixels() {
    let mut graphics = Graphics::default();
    send(&mut graphics, b"\"1;1;5;2#3;2;0;0;100@-!9~", true).unwrap();
    let draw = graphics.draws(0).next().unwrap();
    assert_eq!((draw.image.width, draw.image.height), (9, 12));
    assert_eq!(&draw.image.pixels[..4], &[0, 0, 255, 255]);
    assert_eq!(&draw.image.pixels[4..9 * 6 * 4], vec![0; 9 * 6 * 4 - 4]);
    assert_eq!(
        &draw.image.pixels[9 * 6 * 4..],
        [0, 0, 255, 255].repeat(9 * 6)
    );
}

#[test]
fn sixel_hls_uses_dec_hue_origin_and_palette_redefinitions() {
    let mut graphics = Graphics::default();
    send(
        &mut graphics,
        b"#1;1;0;50;100~#2;1;120;50;100~#3;1;240;50;100~#4;1;0;50;0~#1;2;100;100;0",
        true,
    )
    .unwrap();
    let draw = graphics.draws(0).next().unwrap();
    assert_eq!(
        &draw.image.pixels[..16],
        &[
            255, 255, 0, 255, 255, 0, 0, 255, 0, 255, 0, 255, 128, 128, 128, 255
        ]
    );
}

#[test]
fn sixel_macro_and_raster_aspect_ratios_change_only_destination() {
    let mut graphics = Graphics::default();
    graphics.begin_sixel(2, true, [0; 3], None);
    graphics.sixel_put(b'~');
    assert_eq!(graphics.finish_sixel(context()).unwrap().height, 30);
    let draw = graphics.draws(0).next().unwrap();
    assert_eq!((draw.image.height, draw.dest.height), (6, 30));
    graphics.clear();
    send(&mut graphics, b"\"3;2~", true).unwrap();
    assert_eq!(graphics.draws(0).next().unwrap().dest.height, 12);
}

#[test]
fn malformed_sixel_cannot_replace_an_existing_image() {
    let mut graphics = Graphics::default();
    send(&mut graphics, b"~", true).unwrap();
    let revision = graphics.revision();
    for invalid in [
        b"!42949672960~".as_slice(),
        b"!8193~",
        b"#256~",
        b"#1;2;1~",
        b"#1;9;1;2;3~",
        b"\"1;1;8193;1~",
        b"~\"1;1;2;2",
        b"!3",
        b"\"65;1~",
        b"#1;2;1;1;1;1~",
    ] {
        assert!(send(&mut graphics, invalid, true).is_none(), "{invalid:?}");
        assert_eq!(
            (
                graphics.image_count(),
                graphics.placement_count(),
                graphics.pending_bytes()
            ),
            (1, 1, 0)
        );
        assert_eq!(graphics.revision(), revision);
    }
}

#[test]
fn sixel_transfer_painting_and_scene_limits_are_independent() {
    let mut graphics = Graphics::new(Limits {
        max_image_bytes: 64,
        max_total_bytes: 64,
        max_transfer_bytes: 16,
        max_images: 1,
        ..Limits::default()
    });
    assert!(send(&mut graphics, b"\"1;1;5;5", true).is_none());
    assert!(send(&mut graphics, b"#1;2;100;0;0!!!!!!!!", true).is_none());
    send(&mut graphics, b"~", true).unwrap();
    assert!(send(&mut graphics, b"~", true).is_none());
    graphics.clear();
    graphics.begin_sixel(7, true, [0; 3], None);
    for _ in 0..10 {
        for &byte in b"!4~$" {
            graphics.sixel_put(byte);
        }
    }
    assert!(graphics.finish_sixel(context()).is_none());
    assert_eq!(graphics.pending_bytes(), 0);
    // A separate high-input/low-work scene isolates the expansion work quota.
    let mut graphics = Graphics::new(Limits {
        max_image_bytes: 128,
        max_total_bytes: 128,
        max_transfer_bytes: 4096,
        ..Limits::default()
    });
    graphics.begin_sixel(7, true, [0; 3], None);
    for _ in 0..100 {
        for &byte in b"!4~$" {
            graphics.sixel_put(byte);
        }
    }
    assert!(graphics.finish_sixel(context()).is_none());
}

#[test]
fn aborted_sixel_releases_storage_and_palette_only_streams_are_valid() {
    let mut graphics = Graphics::default();
    graphics.begin_sixel(7, true, [0; 3], None);
    for &byte in b"\"1;1;100;100~" {
        graphics.sixel_put(byte);
    }
    assert!(graphics.pending_bytes() > 0);
    graphics.abort_sixel();
    assert_eq!(graphics.pending_bytes(), 0);
    assert!(graphics.finish_sixel(context()).is_none());
    let colors = send(&mut graphics, b"#7;2;100;0;0", true).unwrap();
    assert_eq!(
        (colors.width, colors.height, graphics.image_count()),
        (0, 0, 0)
    );
    graphics.begin_sixel(7, true, [0; 3], Some(&colors.palette));
    for &byte in b"#7~" {
        graphics.sixel_put(byte);
    }
    graphics.finish_sixel(context()).unwrap();
    assert_eq!(
        &graphics.draws(0).next().unwrap().image.pixels[..4],
        &[255, 0, 0, 255]
    );
}

#[test]
fn sixel_default_palette_and_embedded_controls() {
    let mut graphics = Graphics::default();
    send(&mut graphics, b"\"1;1;2;6#2!\n2\t~", true).unwrap();
    let draw = graphics.draws(0).next().unwrap();
    assert_eq!(draw.image.pixels.as_slice(), [204, 33, 33, 255].repeat(12));
}
