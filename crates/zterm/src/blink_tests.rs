use super::*;

#[test]
fn sgr_blink_slow_and_rapid_are_exclusive_and_25_preserves_other_style() {
    let mut terminal = Terminal::new(16, 2, 0);
    terminal.feed(b"\x1b[1;3;31;5mA\x1b[6mB\x1b[5mC\x1b[25mD\x1b[6mE\x1b[0mF");
    for (col, expected) in [
        (0, BLINK),
        (1, RAPID_BLINK),
        (2, BLINK),
        (3, 0),
        (4, RAPID_BLINK),
    ] {
        let cell = terminal.cell(col, 0);
        assert_eq!(cell.flags & (BLINK | RAPID_BLINK), expected);
        assert_ne!(cell.flags & BOLD, 0);
        assert_ne!(cell.flags & ITALIC, 0);
        assert_eq!(cell.fg, crate::terminal::ANSI_PALETTE[1]);
    }
    assert_eq!(terminal.cell(5, 0).flags, 0);
    assert_eq!(terminal.modes().cursor_blink, None);
    assert!(terminal.modes().cursor_visible);
}

#[test]
fn sgr_blink_is_independent_of_pty_chunk_boundaries() {
    let bytes = b"\x1b[5mA\x1b[6mB\x1b[25mC\x1b[5;6mD\x1b[6;5mE\x1b[mF";
    for split in 0..=bytes.len() {
        let mut terminal = Terminal::new(10, 2, 0);
        terminal.feed(&bytes[..split]);
        terminal.feed(&bytes[split..]);
        for (col, expected) in [BLINK, RAPID_BLINK, 0, RAPID_BLINK, BLINK, 0]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                terminal.cell(col, 0).flags & (BLINK | RAPID_BLINK),
                expected
            );
        }
    }
}

#[test]
fn sgr_blink_survives_saved_styles_wide_clusters_and_history() {
    let mut terminal = Terminal::new(8, 2, 8);
    terminal.feed("\x1b[6m👩🏽‍💻\x1b7\x1b[25mX\x1b8Z\r\nline\r\nnext".as_bytes());
    assert_eq!(terminal.history_len(), 1);
    assert_eq!(
        terminal.view_cell(0, 0, 1).flags & (BLINK | RAPID_BLINK),
        RAPID_BLINK
    );
    assert_eq!(
        terminal.view_cell(1, 0, 1).flags & (BLINK | RAPID_BLINK),
        RAPID_BLINK
    );
    assert_eq!(terminal.view_cell(2, 0, 1).ch, 'Z');
    assert_eq!(
        terminal.view_cell(2, 0, 1).flags & (BLINK | RAPID_BLINK),
        RAPID_BLINK
    );
    assert_eq!(
        terminal
            .cell_chars(terminal.view_cell(0, 0, 1))
            .collect::<alloc::string::String>(),
        "👩🏽‍💻"
    );
}

#[test]
fn sgr_blink_is_not_retained_by_erased_blank_cells() {
    let mut terminal = Terminal::new(8, 2, 0);
    terminal.feed(b"\x1b[5;41mabc\r\x1b[2K");
    for col in 0..8 {
        assert_eq!(terminal.cell(col, 0).flags & (BLINK | RAPID_BLINK), 0);
        assert_eq!(terminal.cell(col, 0).bg, crate::terminal::ANSI_PALETTE[1]);
    }
    terminal.feed(b"X");
    assert_eq!(terminal.cell(0, 0).flags & (BLINK | RAPID_BLINK), BLINK);
}
