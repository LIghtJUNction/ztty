use super::*;
use alloc::format;

#[test]
fn sixel_every_chunk_boundary_has_identical_pixels_and_cursor() {
    let stream = b"prefix\x1b[3;4H\x1bP7;1q\"1;1;3;6#1;2;100;0;0!3~\x1b\\after";
    for split in 0..=stream.len() {
        let mut terminal = Terminal::new(20, 8, 20);
        terminal.feed(&stream[..split]);
        terminal.feed(&stream[split..]);
        let draw = terminal.graphics_draws(0).next().unwrap();
        assert_eq!(
            (draw.dest.x, draw.dest.y, draw.dest.width, draw.dest.height),
            (24, 32, 3, 6),
            "split {split}"
        );
        assert_eq!(draw.image.pixels.as_slice(), [255, 0, 0, 255].repeat(18));
        assert_eq!(terminal.cursor(), (5, 3));
        assert_eq!(terminal.cell(0, 3).ch, 'a');
        assert_eq!(terminal.graphics().pending_bytes(), 0);
    }
}

#[test]
fn sixel_cancellation_and_truncation_never_display_partial_images() {
    for end in [
        b"\x18after".as_slice(),
        b"\x1aafter",
        b"\x1b[31mafter",
        b"\x1b_x\x1b\\after",
        b"\x1bP7q!8193~\x1b\\after",
    ] {
        let mut terminal = Terminal::new(20, 8, 20);
        terminal.feed(b"\x1bP7q~");
        assert!(terminal.graphics().pending_bytes() > 0);
        assert_eq!(terminal.graphics().image_count(), 0);
        terminal.feed(end);
        assert_eq!(
            (
                terminal.graphics().image_count(),
                terminal.graphics().pending_bytes()
            ),
            (0, 0),
            "{end:?}"
        );
        assert_eq!(terminal.cell(0, 0).ch, 'a');
        terminal.feed(b"\x1b\\");
        assert_eq!(terminal.graphics().image_count(), 0);
    }
    let mut terminal = Terminal::new(20, 8, 20);
    terminal.feed(b"\x1bP7q~\x1b");
    assert_eq!(terminal.graphics().image_count(), 0);
    terminal.feed(b"\\");
    assert_eq!(terminal.graphics().image_count(), 1);
}

#[test]
fn sixel_scrolls_with_history_and_respects_display_and_right_modes() {
    let mut terminal = Terminal::new(20, 3, 20);
    terminal.set_cell_size(8, 6);
    terminal.feed(b"top\r\nmiddle\r\nbottom\x1bP7q~\x1b\\");
    assert_eq!(terminal.cursor(), (0, 2));
    assert_eq!(terminal.history_len(), 1);
    assert_eq!(terminal.graphics_draws(0).next().unwrap().dest.y, 6);
    terminal.feed(b"\x1b[2J\x1b[?80h\x1b[2;4H\x1bP7q~-~\x1b\\");
    assert_eq!(terminal.cursor(), (3, 1));
    let draw = terminal.graphics_draws(0).next().unwrap();
    assert_eq!((draw.dest.x, draw.dest.y), (0, 0));
    terminal.feed(b"\x1b[2J\x1b[?80l\x1b[?8452h\x1b[1;2H\x1bP7q!16~\x1b\\");
    assert_eq!(terminal.cursor(), (3, 0));
}

#[test]
fn sixel_palette_scope_background_and_mode_queries_are_explicit() {
    let mut terminal = Terminal::new(20, 8, 20);
    terminal.feed(b"\x1b[?1070l\x1bP7q#1;2;100;0;0\x1b\\\x1bP7;1q#1~\x1b\\");
    assert_eq!(
        &terminal.graphics_draws(0).next().unwrap().image.pixels[..4],
        &[255, 0, 0, 255]
    );
    terminal.feed(b"\x1b[2J\x1b[?1070h\x1b[48;2;11;22;33m\x1bP7q\"1;1;2;1#1@\x1b\\");
    let draw = terminal.graphics_draws(0).next().unwrap();
    assert_eq!(
        draw.image.pixels.as_slice(),
        &[51, 51, 204, 255, 11, 22, 33, 255]
    );
    terminal.feed(b"\x1b[c\x1b[?80$p\x1b[?1070$p\x1b[?8452$p");
    assert_eq!(
        terminal.responses(),
        b"\x1b[?62;4c\x1b[?80;2$y\x1b[?1070;1$y\x1b[?8452;2$y"
    );
}

#[test]
fn sixel_interoperates_with_kitty_dcs_queries_and_alternate_screen() {
    let mut terminal = Terminal::new(20, 8, 20);
    terminal.feed(b"\x1bP+q544e\x1b\\\x1bP7q#2~\x1b\\\x1b_Ga=T,i=42,f=24,s=1,v=1,C=1;AP8A\x1b\\");
    assert_eq!(terminal.graphics().image_count(), 2);
    assert_eq!(
        &terminal.graphics().image(42).unwrap().pixels[..4],
        &[0, 255, 0, 255]
    );
    assert!(terminal.responses().starts_with(b"\x1bP1+r544e="));
    terminal.feed(b"\x1b[?1049h\x1bP7q#3~\x1b\\");
    assert_eq!(terminal.graphics().image_count(), 1);
    terminal.feed(b"\x1b[?1049l");
    assert_eq!(terminal.graphics().image_count(), 2);
    terminal.feed(b"\x1bc");
    assert_eq!(terminal.graphics().image_count(), 0);
}

#[test]
fn sixel_c1_terminator_and_large_scrolling_region_are_bounded() {
    let mut terminal = Terminal::new(20, 8, 20);
    terminal.set_cell_size(8, 1);
    terminal.feed(b"\x1bP7q~\x9c");
    assert_eq!(terminal.graphics().image_count(), 1);
    terminal.feed(b"\x1b[2;7r\x1b[2;1H\x1bP7q\"1;1;1;8192@\x1b\\text");
    assert_eq!(terminal.cursor(), (4, 6));
    assert_eq!(terminal.cell(0, 6).ch, 't');
}

fn preview_terminal() -> Terminal {
    let mut terminal = Terminal::new(20, 10, 20);
    terminal.set_cell_size(4, 6);
    terminal.feed(b"\x1b[3;3H\x1bP7;1q\"1;1;12;12#1;2;100;0;0!12~-!12~\x1b\\");
    terminal
}

#[test]
fn sixel_text_overwrite_erases_only_the_intersecting_cells() {
    for (text, erased_width) in [("x", 4), ("界", 8)] {
        let mut terminal = preview_terminal();
        let before = terminal.graphics().revision();
        terminal.feed(format!("\x1b[3;4H{text}").as_bytes());
        let draw = terminal.graphics_draws(0).next().unwrap();
        for y in 0..12 {
            for x in 0..12 {
                let pixel = &draw.image.pixels[(y * 12 + x) * 4..(y * 12 + x + 1) * 4];
                let erased = y < 6 && (4..4 + erased_width).contains(&x);
                assert_eq!(pixel, if erased { &[0; 4] } else { &[255, 0, 0, 255] });
            }
        }
        assert!(terminal.graphics().revision() > before);
        assert_eq!(draw.image.generation, terminal.graphics().revision());
    }
}

#[test]
fn sixel_local_erase_and_spaces_release_complete_previews() {
    for clear in [
        "\x1b[3;3H   \x1b[4;3H   ",
        "\x1b[3;3H\x1b[3X\x1b[4;3H\x1b[3X",
        "\x1b[3;3H\x1b[K\x1b[4;3H\x1b[K",
        "\x1b[3;3H\x1b[J",
        "\x1b[5;6H\x1b[1J",
    ] {
        let mut terminal = preview_terminal();
        terminal.feed(clear.as_bytes());
        assert_eq!(
            (
                terminal.graphics().image_count(),
                terminal.graphics().placement_count(),
                terminal.graphics().image_bytes()
            ),
            (0, 0, 0),
            "{clear:?}"
        );
    }
}

#[test]
fn sixel_erasure_preserves_kitty_and_nonintersecting_sixel() {
    let mut terminal = preview_terminal();
    terminal.feed(b"\x1b[7;10H\x1bP7;1q\"1;1;4;6#2;2;0;100;0!4~\x1b\\");
    terminal.feed(b"\x1b[3;3H\x1b_Ga=T,i=991,f=24,s=1,v=1,C=1,q=2;AAD/\x1b\\");
    terminal.feed(b"\x1b[3;3H   \x1b[4;3H   ");
    assert_eq!(terminal.graphics().image_count(), 2);
    assert_eq!(
        terminal.graphics().image(991).unwrap().pixels.as_slice(),
        &[0, 0, 255, 255]
    );
    let green = terminal
        .graphics_draws(0)
        .find(|draw| draw.image_id != 991)
        .unwrap();
    assert_eq!(green.image.pixels.as_slice(), [0, 255, 0, 255].repeat(24));
}

#[test]
fn sixel_erase_keeps_pixels_in_scrollback() {
    let mut terminal = Terminal::new(20, 4, 20);
    terminal.set_cell_size(4, 6);
    terminal.feed(b"\x1bP7;1q\"1;1;4;6#1;2;100;0;0!4~\x1b\\");
    terminal.feed(b"\x1b[4;1H\n\n");
    let revision = terminal.graphics().revision();
    terminal.feed(b"\x1b[H\x1b[J");
    assert_eq!(terminal.graphics().revision(), revision);
    let draw = terminal.graphics_draws(2).next().unwrap();
    assert_eq!(draw.dest.y, 0);
    assert_eq!(draw.image.pixels.as_slice(), [255, 0, 0, 255].repeat(24));
}

#[test]
fn sixel_scaled_erasure_rounds_out_to_source_pixels() {
    let mut terminal = Terminal::new(20, 10, 0);
    // Default aspect is 2:1; a five-pixel cell cuts its third source pixel.
    terminal.set_cell_size(4, 5);
    terminal.feed(b"\x1b[3;3H\x1bP0;1q#1;2;100;0;0!4~\x1b\\");
    terminal.feed(b"\x1b[3;3H\x1b[X");
    let draw = terminal.graphics_draws(0).next().unwrap();
    assert_eq!((draw.image.height, draw.dest.height), (6, 12));
    for y in 0..6 {
        for x in 0..4 {
            let alpha = draw.image.pixels[(y * 4 + x) * 4 + 3];
            assert_eq!(alpha, if y < 3 { 0 } else { 255 });
        }
    }
    // Source pixel 2 covers display rows 4 and 5: erasure of [0, 5)
    // intentionally also clears display row 5, but keeps the next source row.
}

#[test]
fn repeated_sixel_preview_replacement_reclaims_image_budget() {
    let mut terminal = Terminal::new(20, 10, 0);
    terminal.set_cell_size(4, 6);
    for _ in 0..256 {
        terminal.feed(b"\x1b[3;3H\x1bP7;1q\"1;1;4;12#1;2;100;0;0!4~-!4~\x1b\\");
        assert_eq!(terminal.graphics().image_count(), 1);
        terminal.feed(b"\x1b[3;3H \x1b[4;3H ");
        assert_eq!(terminal.graphics().image_count(), 0);
        assert_eq!(terminal.graphics().image_bytes(), 0);
    }
}

#[test]
fn sixel_combining_mark_redraw_erases_the_existing_base_cell() {
    let mut terminal = Terminal::new(20, 10, 0);
    terminal.set_cell_size(4, 6);
    terminal.feed(b"\x1b[3;3He\x1b[3;3H\x1bP7;1q\"1;1;8;6#1;2;100;0;0!8~\x1b\\");
    terminal.feed("\x1b[3;4H\u{301}".as_bytes());
    assert_eq!(terminal.cell(2, 2).combining[0], '\u{301}');
    let draw = terminal.graphics_draws(0).next().unwrap();
    for y in 0..6 {
        for x in 0..8 {
            assert_eq!(
                draw.image.pixels[(y * 8 + x) * 4 + 3],
                if x < 4 { 0 } else { 255 }
            );
        }
    }
}
