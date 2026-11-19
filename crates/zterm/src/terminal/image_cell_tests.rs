use super::*;

fn virtual_image(t: &mut Terminal, image: u32, placement: u32) {
    t.feed(
        format!("\x1b_Ga=T,U=1,i={image},p={placement},s=2,v=1,c=2,r=1,C=1;/wAA/wD/AP8=\x1b\\")
            .as_bytes(),
    );
    t.clear_responses();
}

#[test]
fn placeholders_keep_32bit_image_and_underline_placement_ids_with_inheritance() {
    let mut t = Terminal::new(8, 3, 10);
    virtual_image(&mut t, 42 + (2 << 24), 7);
    t.feed("\x1b[38;5;42m\x1b[58;5;7m\u{10eeee}\u{305}\u{305}\u{30e}\u{10eeee}".as_bytes());
    let draws: Vec<_> = t.graphics_draws(0).collect();
    assert_eq!(draws.len(), 1);
    assert_eq!(
        (draws[0].image_id, draws[0].placement_id),
        (42 + (2 << 24), 7)
    );
    assert_eq!(draws[0].clip.unwrap().width, 16);
    assert_eq!(
        t.cell_chars(t.cell(0, 0)).collect::<String>(),
        "\u{10eeee}\u{305}\u{305}\u{30e}"
    );
    assert_eq!(core::mem::size_of::<Cell>(), 32);
}

#[test]
fn placeholder_omission_does_not_cross_rows_or_color_changes() {
    let mut t = Terminal::new(8, 3, 10);
    virtual_image(&mut t, 42, 0);
    virtual_image(&mut t, 43, 0);
    t.feed("\x1b[38;5;42m\u{10eeee}\u{305}\u{30d}\x1b[38;5;43m\u{10eeee}\r\n\u{10eeee}".as_bytes());
    let cells = t.image_placeholders(0);
    assert_eq!(
        cells
            .iter()
            .map(|p| (p.image_id, p.image_col, p.image_row))
            .collect::<Vec<_>>(),
        [(42, 1, 0), (43, 0, 0), (43, 0, 0)]
    );
}

#[test]
fn placeholder_id_keeps_index_even_when_palette_rgb_values_collide() {
    let mut t = Terminal::new(8, 3, 10);
    virtual_image(&mut t, 16, 0);
    virtual_image(&mut t, 0x000000, 0);
    t.feed("\x1b[38;5;16m\u{10eeee}".as_bytes());
    assert_eq!(t.cell(0, 0).fg, 16);
    assert_eq!(t.graphics_draws(0).next().unwrap().image_id, 16);
    t.set_theme(Theme {
        foreground: 0x334455,
        background: 0x778899,
        palette: [0x123456; 16],
    });
    assert_eq!(t.cell(0, 0).fg, 16);
    assert_eq!(t.graphics_draws(0).next().unwrap().image_id, 16);
}

#[test]
fn placeholder_metadata_cannot_open_an_existing_hyperlink() {
    let mut t = Terminal::new(8, 3, 10);
    t.feed(b"\x1b]8;;https://example.com\x1b\\A\x1b]8;;\x1b\\");
    t.feed("\x1b[38;5;42m\x1b[58;5;1m\u{10eeee}".as_bytes());
    assert_eq!(t.cell_hyperlink(t.cell(0, 0)), Some("https://example.com"));
    assert_eq!(t.cell(1, 0).link_id, 1);
    assert_eq!(t.cell_hyperlink(t.cell(1, 0)), None);
    t.feed(b"\x1b[0mB");
    assert_eq!(t.cell(2, 0).link_id, 0);
}

#[test]
fn fragmented_placeholder_retains_all_diacritics_and_cache_tracks_text_scroll() {
    let mut t = Terminal::new(8, 2, 10);
    virtual_image(&mut t, 42 + (2 << 24), 0);
    for byte in "\x1b[38;5;42m\u{10eeee}\u{305}\u{305}\u{30e}".as_bytes() {
        t.feed(&[*byte]);
    }
    assert_eq!(t.graphics_draws(0).next().unwrap().clip.unwrap().y, 0);
    t.feed(b"\r\n\r\n");
    assert!(t.graphics_draws(0).next().is_none());
    assert_eq!(t.graphics_draws(1).next().unwrap().clip.unwrap().y, 0);
}

#[test]
fn placeholder_cache_is_lazy_and_reentrant_for_per_row_rendering() {
    let mut t = Terminal::new(8, 3, 10);
    t.feed(b"plain output");
    assert!(t.graphics_draws(0).next().is_none());
    assert!(t.image_cache.borrow().is_none());
    virtual_image(&mut t, 42, 0);
    t.feed("\r\x1b[38;5;42m\u{10eeee}".as_bytes());
    let first = t.graphics_draws(0);
    let second = t.graphics_draws(0);
    assert_eq!(first.count(), second.count());
    assert!(t.image_cache.borrow().is_some());
    t.feed(b"\x1b_Ga=d,d=I,i=42\x1b\\");
    assert!(t.graphics_draws(0).next().is_none());
    assert!(t.image_cache.borrow().is_none());
}
