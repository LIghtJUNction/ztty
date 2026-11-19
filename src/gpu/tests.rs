extern crate std;
use super::*;
use std::sync::Mutex;

static EGL_TEST: Mutex<()> = Mutex::new(());

fn pixel(bytes: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
    bytes[(y * width + x) * 4..][..4].try_into().unwrap()
}

#[test]
#[ignore = "requires installed native fonts and a real EGL implementation"]
fn native_fonts_shape_joining_scripts_and_complete_emoji_clusters() {
    let mut fonts = font::Fonts::new("DejaVu Sans:size=18").unwrap();
    let metrics = fonts.metrics();
    assert!(metrics.cell_width > 0.0 && metrics.cell_height >= metrics.ascent);
    let arabic = "سلام";
    let face = fonts.select(arabic, false, false).unwrap();
    let together = fonts.shape(face, arabic).unwrap();
    assert!(together.iter().all(|glyph| glyph.glyph != 0));
    let separate: Vec<u32> = arabic
        .chars()
        .flat_map(|character| {
            let mut utf8 = [0; 4];
            fonts
                .shape(face, character.encode_utf8(&mut utf8))
                .unwrap()
                .iter()
                .map(|glyph| glyph.glyph)
                .collect::<Vec<_>>()
        })
        .collect();
    assert_ne!(
        together.iter().map(|glyph| glyph.glyph).collect::<Vec<_>>(),
        separate
    );
    for text in ["👩🏽‍💻", "👨‍👩‍👧‍👦", "🇨🇳"] {
        let font = fonts.select(text, false, false).unwrap();
        let shaped = fonts.shape(font, text).unwrap();
        assert_eq!(shaped.len(), 1, "emoji did not form one glyph: {text}");
        assert_ne!(shaped[0].glyph, 0, "missing emoji glyph: {text}");
        let bitmap = fonts.rasterize(font, shaped[0].glyph).unwrap();
        assert!(
            bitmap.color && bitmap.width > 0 && bitmap.height > 0,
            "no colored bitmap: {text}"
        );
    }
    let plain = fonts.select("A", false, false).unwrap();
    let italic = fonts.select("A", false, true).unwrap();
    assert_ne!(plain, italic);
    let plain_glyph = fonts.shape(plain, "A").unwrap()[0].glyph;
    let italic_glyph = fonts.shape(italic, "A").unwrap()[0].glyph;
    let plain_bitmap = fonts.rasterize(plain, plain_glyph).unwrap();
    let italic_bitmap = fonts.rasterize(italic, italic_glyph).unwrap();
    assert_ne!(plain_bitmap.rgba, italic_bitmap.rgba);
    assert!(fonts.shape(plain, &"a".repeat(16_385)).is_err());
    let cached = fonts.shape(plain, "bounded cache").unwrap();
    let repeated = fonts.shape(plain, "bounded cache").unwrap();
    assert!(Rc::ptr_eq(&cached, &repeated));
    for index in 0..600 {
        fonts
            .shape(plain, &std::format!("distinct row {index}"))
            .unwrap();
        assert!(fonts.shape_cache_bytes() <= 512 * 1024);
    }
}

#[test]
#[ignore = "requires a real EGL implementation; run with --ignored"]
fn font_reload_keeps_the_surface_and_image_cache_and_updates_ui_metrics() {
    let _guard = EGL_TEST.lock().unwrap();
    let display = Display::headless().unwrap();
    let mut renderer =
        Renderer::new(display, Target::Offscreen, 240, 100, "DejaVu Sans:size=12").unwrap();
    renderer.begin_frame(0, 255).unwrap();
    let style = Style {
        bold: false,
        italic: true,
    };
    let width = renderer.measure_text("office", style).unwrap();
    let drawn_width = renderer
        .draw_text("office", 5.0, 35.0, 0xffffff, style)
        .unwrap();
    assert!((drawn_width - width).abs() < 0.01);
    renderer
        .draw_image(
            91,
            1,
            (1, 1),
            &[255, 0, 0, 255],
            Rect {
                width: 1.0,
                height: 1.0,
                ..Rect::default()
            },
            Rect {
                x: 220.0,
                y: 80.0,
                width: 10.0,
                height: 10.0,
            },
        )
        .unwrap();
    let before = renderer.read_pixels().unwrap();
    assert!(renderer.glyph_cache_entries() > 0);
    assert_eq!(renderer.image_cache_bytes(), 4);
    renderer.set_font("DejaVu Sans:size=24").unwrap();
    assert_eq!(renderer.glyph_cache_entries(), 0);
    assert_eq!(renderer.shape_cache_bytes(), 0);
    assert_eq!(renderer.image_cache_bytes(), 4);
    assert!(renderer.measure_text("office", style).unwrap() > width * 1.5);
    renderer.begin_frame(0, 255).unwrap();
    renderer
        .draw_text("office", 5.0, 50.0, 0xffffff, style)
        .unwrap();
    let after = renderer.read_pixels().unwrap();
    assert_ne!(before, after);
    renderer.present().unwrap();
}

#[test]
#[ignore = "requires a real EGL implementation; run with --ignored"]
fn image_texture_coordinates_preserve_every_column_at_four_k_width() {
    let _guard = EGL_TEST.lock().unwrap();
    let display = Display::headless().unwrap();
    let mut renderer =
        Renderer::new(display, Target::Offscreen, 4096, 4, "monospace:size=11").unwrap();
    let mut source = Vec::with_capacity(4096 * 4 * 4);
    for y in 0..4usize {
        for x in 0..4096usize {
            source.extend_from_slice(&[
                (x % 256) as u8,
                ((x / 16) % 256) as u8,
                (y * 63) as u8,
                255,
            ]);
        }
    }
    renderer.begin_frame(0, 255).unwrap();
    let rectangle = Rect {
        x: 0.0,
        y: 0.0,
        width: 4096.0,
        height: 4.0,
    };
    renderer
        .draw_image(1, 1, (4096, 4), &source, rectangle, rectangle)
        .unwrap();
    let actual = renderer.read_pixels().unwrap();
    assert_eq!(source, actual);
}

#[test]
#[ignore = "requires a real EGL implementation; run with --ignored"]
fn egl_composites_clipping_alpha_and_replaced_images_with_exact_pixels() {
    let _guard = EGL_TEST.lock().unwrap();
    let display = Display::headless().unwrap();
    let mut renderer =
        Renderer::new(display, Target::Offscreen, 32, 24, "monospace:size=11").unwrap();
    std::println!("OpenGL renderer: {}", renderer.driver());
    renderer.begin_frame(0x000000, 255).unwrap();
    renderer
        .draw_rect(
            Rect {
                x: 2.0,
                y: 3.0,
                width: 16.0,
                height: 12.0,
            },
            0xff0000,
            255,
        )
        .unwrap();
    renderer
        .clip(Some(Rect {
            x: 5.0,
            y: 6.0,
            width: 3.0,
            height: 4.0,
        }))
        .unwrap();
    renderer
        .draw_rect(
            Rect {
                x: 0.0,
                y: 0.0,
                width: 32.0,
                height: 24.0,
            },
            0x00ff00,
            255,
        )
        .unwrap();
    renderer.clip(None).unwrap();
    let image = [0u8, 0, 255, 255];
    renderer
        .draw_image(
            7,
            1,
            (1, 1),
            &image,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            Rect {
                x: 20.0,
                y: 2.0,
                width: 4.0,
                height: 4.0,
            },
        )
        .unwrap();
    let pixels = renderer.read_pixels().unwrap();
    for y in 0..24 {
        for x in 0..32 {
            let expected = if (5..8).contains(&x) && (6..10).contains(&y) {
                [0, 255, 0, 255]
            } else if (20..24).contains(&x) && (2..6).contains(&y) {
                [0, 0, 255, 255]
            } else if (2..18).contains(&x) && (3..15).contains(&y) {
                [255, 0, 0, 255]
            } else {
                [0, 0, 0, 255]
            };
            assert_eq!(pixel(&pixels, 32, x, y), expected, "pixel {x},{y}");
        }
    }
    renderer.begin_frame(0xffffff, 255).unwrap();
    renderer
        .draw_image(
            7,
            2,
            (1, 1),
            &[255, 0, 0, 128],
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            Rect {
                x: 0.0,
                y: 0.0,
                width: 32.0,
                height: 24.0,
            },
        )
        .unwrap();
    let p = pixel(&renderer.read_pixels().unwrap(), 32, 10, 10);
    assert_eq!(p[0], 255);
    assert!((126..=128).contains(&p[1]));
    assert_eq!(p[1], p[2]);
    assert_eq!(p[3], 255);
    assert_eq!(renderer.image_cache_bytes(), 4);
    renderer.remove_image(7).unwrap();
    assert_eq!(renderer.image_cache_bytes(), 0);
    renderer.present().unwrap();
}

#[test]
#[ignore = "requires a real EGL implementation; run with --ignored"]
fn shared_display_survives_interleaved_windows_resize_and_context_destruction() {
    let _guard = EGL_TEST.lock().unwrap();
    let display = Display::headless().unwrap();
    let mut first = Renderer::new(
        display.clone(),
        Target::Offscreen,
        32,
        24,
        "monospace:size=11",
    )
    .unwrap();
    let mut second =
        Renderer::new(display, Target::Offscreen, 24, 20, "monospace:size=11").unwrap();
    first.begin_frame(0x000000, 255).unwrap();
    first
        .draw_rect(
            Rect {
                x: 0.0,
                y: 0.0,
                width: 32.0,
                height: 24.0,
            },
            0xff0000,
            255,
        )
        .unwrap();
    second.begin_frame(0x0000ff, 255).unwrap();
    assert_eq!(
        pixel(&first.read_pixels().unwrap(), 32, 4, 4),
        [255, 0, 0, 255]
    );
    assert_eq!(
        pixel(&second.read_pixels().unwrap(), 24, 4, 4),
        [0, 0, 255, 255]
    );
    drop(second);
    first.resize(48, 30).unwrap();
    first.begin_frame(0x00ff00, 255).unwrap();
    let pixels = first.read_pixels().unwrap();
    assert_eq!(pixels.len(), 48 * 30 * 4);
    assert!(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [0, 255, 0, 255])
    );
    assert!(first.resize(8193, 30).is_err());
    assert!(first.resize(8192, 8192).is_err());
}

#[test]
#[ignore = "requires native fonts and a real EGL implementation"]
fn shaped_terminal_cells_render_whole_clusters_and_reuse_the_bounded_atlas() {
    let _guard = EGL_TEST.lock().unwrap();
    let mut renderer = Renderer::new(
        Display::headless().unwrap(),
        Target::Offscreen,
        360,
        96,
        "DejaVu Sans Mono:size=16",
    )
    .unwrap();
    let text = "A👩🏽‍💻B";
    let cells = [
        TextCell {
            byte: 0,
            x: 0.0,
            width: 20.0,
        },
        TextCell {
            byte: 1,
            x: 20.0,
            width: 40.0,
        },
        TextCell {
            byte: (text.len() - 1) as u32,
            x: 60.0,
            width: 20.0,
        },
    ];
    let mut first = Vec::new();
    let mut count = 0;
    for iteration in 0..3 {
        renderer.begin_frame(0x000000, 255).unwrap();
        renderer
            .draw_cells(text, &cells, 8.0, 42.0, 0xffffff, Style::default())
            .unwrap();
        renderer
            .draw_text(
                "سلام",
                110.0,
                42.0,
                0xffffff,
                Style {
                    italic: true,
                    bold: false,
                },
            )
            .unwrap();
        let pixels = renderer.read_pixels().unwrap();
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[0] != p[1] || p[1] != p[2]),
            "emoji lost its colors"
        );
        if iteration == 0 {
            first = pixels;
            count = renderer.glyph_cache_entries();
        } else {
            assert_eq!(pixels, first);
            assert_eq!(renderer.glyph_cache_entries(), count);
        }
    }
    assert!(count > 0 && count <= 4096);
    let bad = [TextCell {
        byte: 2,
        x: 0.0,
        width: 2.0,
    }];
    assert!(
        renderer
            .draw_cells(text, &bad, 0.0, 0.0, 0, Style::default())
            .is_err()
    );
}
