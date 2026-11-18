use super::*;

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
