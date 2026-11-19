use super::*;
use alloc::string::{String, ToString};

fn text(terminal: &Terminal, column: usize, row: usize) -> String {
    terminal.cell_chars(terminal.cell(column, row)).collect()
}

#[test]
fn emoji_clusters_keep_complete_text_in_two_cells_at_every_pty_split() {
    let clusters = [
        "👨‍👩‍👧‍👦",
        "👩🏽‍💻",
        "👩🏾‍❤️‍💋‍👨🏻",
        "🇺🇳",
        "1️⃣",
        "#️⃣",
        "❤️",
        "🏳️‍🌈",
        "🏴\u{e0067}\u{e0062}\u{e0065}\u{e006e}\u{e0067}\u{e007f}",
    ];
    for cluster in clusters {
        for split in 0..=cluster.len() {
            let mut terminal = Terminal::new(8, 2, 8);
            terminal.feed(&cluster.as_bytes()[..split]);
            terminal.feed(&cluster.as_bytes()[split..]);
            assert_eq!(text(&terminal, 0, 0), cluster, "split={split}: {cluster}");
            assert_eq!(terminal.cell(0, 0).width, 2, "{cluster}");
            assert_eq!(terminal.cell(1, 0).width, 0, "{cluster}");
            assert_eq!(terminal.cursor(), (2, 0), "{cluster}");
            terminal.feed(b"X");
            assert_eq!(terminal.cell(2, 0).ch, 'X');
        }
        let mut terminal = Terminal::new(8, 2, 0);
        for byte in cluster.as_bytes() {
            terminal.feed(core::slice::from_ref(byte));
        }
        assert_eq!(text(&terminal, 0, 0), cluster);
        assert_eq!(terminal.cursor(), (2, 0));
    }
}

#[test]
fn long_combining_cluster_is_preserved_without_normalization() {
    let cluster = "e\u{301}\u{308}\u{327}\u{20dd}\u{334}";
    let mut terminal = Terminal::new(8, 2, 0);
    terminal.feed(cluster.as_bytes());
    assert_eq!(text(&terminal, 0, 0), cluster);
    assert_eq!(terminal.cell(0, 0).width, 1);
    assert_eq!(terminal.cursor(), (1, 0));
    assert_eq!(terminal.cell(0, 0).combining, ['\u{301}', '\u{308}']);
    terminal.feed(b"\x1b[1;1HX");
    assert_eq!(text(&terminal, 0, 0), "X");
    assert_eq!(terminal.cell(0, 0).grapheme_id, 0);
}

#[test]
fn late_emoji_width_growth_moves_the_whole_margin_cluster() {
    for cluster in ["1️⃣", "❤️", "🇺🇳"] {
        let mut terminal = Terminal::new(4, 3, 4);
        terminal.feed(b"abc");
        terminal.feed(cluster.as_bytes());
        terminal.feed(b"X");
        assert_eq!(terminal.view_row_wrap(0, 0), Some(3), "{cluster}");
        assert_eq!(terminal.cell(3, 0).ch, ' ');
        assert_eq!(text(&terminal, 0, 1), cluster);
        assert_eq!(terminal.cell(1, 1).width, 0);
        assert_eq!(terminal.cell(2, 1).ch, 'X');
        assert_eq!(terminal.cursor(), (3, 1));
    }
}

#[test]
fn presentation_selector_can_shrink_a_wide_cell_without_leaving_a_trailer() {
    let mut terminal = Terminal::new(8, 2, 0);
    terminal.feed("♈".as_bytes());
    terminal.feed("\u{fe0e}X".as_bytes());
    assert_eq!(text(&terminal, 0, 0), "♈\u{fe0e}");
    assert_eq!(terminal.cell(0, 0).width, 1);
    assert_eq!(terminal.cell(1, 0).ch, 'X');
    assert_eq!(terminal.cell(1, 0).width, 1);
    assert_eq!(terminal.cursor(), (2, 0));
}

#[test]
fn sgr_keeps_the_cluster_but_cursor_addressing_starts_a_new_one() {
    let mut terminal = Terminal::new(12, 2, 0);
    terminal.feed("\x1b[31m👩\x1b[32m‍💻".as_bytes());
    assert_eq!(text(&terminal, 0, 0), "👩‍💻");
    assert_eq!(terminal.cell(0, 0).fg, crate::terminal::ANSI_PALETTE[1]);
    terminal.feed("\x1b[1;3H👨".as_bytes());
    assert_eq!(text(&terminal, 0, 0), "👩‍💻");
    assert_eq!(text(&terminal, 2, 0), "👨");
    assert_eq!(terminal.cursor(), (4, 0));
}

#[test]
fn placeholder_protocol_scalars_are_not_normalized_or_dropped() {
    let cluster = "\u{10eeee}\u{305}\u{30d}\u{30e}";
    let mut terminal = Terminal::new(8, 2, 0);
    terminal.feed(cluster.as_bytes());
    assert_eq!(text(&terminal, 0, 0), cluster);
    assert_eq!(terminal.cursor(), (1, 0));
}

#[test]
fn single_column_keeps_a_complete_cluster_and_ascii_overwrite_clears_it() {
    let mut terminal = Terminal::new(1, 3, 4);
    terminal.feed("👩🏽‍💻".as_bytes());
    assert_eq!(text(&terminal, 0, 0), "👩🏽‍💻");
    assert_eq!(terminal.cell(0, 0).width, 1);
    terminal.feed(b"\rX");
    assert_eq!(text(&terminal, 0, 0), "X");
    assert_eq!(terminal.cell(0, 0).grapheme_id, 0);
}

#[test]
fn overlong_combining_input_has_a_visible_bounded_replacement() {
    let mut terminal = Terminal::new(8, 2, 0);
    let source = "e".to_string() + &"\u{301}".repeat(4096);
    terminal.feed(source.as_bytes());
    assert_eq!(text(&terminal, 0, 0), "\u{fffd}");
    assert_eq!(terminal.cursor(), (1, 0));
    terminal.feed("X\u{308}".as_bytes());
    assert_eq!(text(&terminal, 1, 0), "X\u{308}");
    assert_eq!(core::mem::size_of::<Cell>(), 32);
}

#[test]
fn shell_command_text_keeps_complete_emoji_and_does_not_split_at_a_byte_limit() {
    let mut terminal = Terminal::new(32, 3, 4);
    terminal
        .feed("\x1b]133;A\x07$ \x1b]133;B\x07echo\r\n\x1b]133;C\x07👩🏽‍💻\x1b]133;D;0\x07".as_bytes());
    let id = terminal.commands().next().unwrap().id;
    let result = terminal.command_output(id, 128).unwrap();
    assert_eq!(result.text, "👩🏽‍💻");
    assert!(!result.truncated);
    let result = terminal.command_output(id, 4).unwrap();
    assert_eq!(result.text, "");
    assert!(result.truncated);
}
