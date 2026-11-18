use crate::Terminal;
use alloc::{vec, vec::Vec};

fn wraps(terminal: &Terminal, offset: usize) -> Vec<Option<usize>> {
    (0..terminal.rows())
        .map(|row| terminal.view_row_wrap(row, offset))
        .collect()
}

#[test]
fn soft_wrap_is_recorded_only_after_automatic_row_advance() {
    let mut terminal = Terminal::new(4, 3, 10);
    terminal.feed(b"abcd");
    assert_eq!(wraps(&terminal, 0), [None, None, None]);
    terminal.feed(b"e");
    assert_eq!(wraps(&terminal, 0), [Some(4), None, None]);
    terminal.feed(b"\r\nf");
    assert_eq!(wraps(&terminal, 0), [Some(4), None, None]);

    let mut wide = Terminal::new(4, 3, 10);
    wide.feed("abc界".as_bytes());
    assert_eq!(wraps(&wide, 0), [Some(3), None, None]);
    assert_eq!(wide.cell(3, 0).ch, ' ');
    let mut disabled = Terminal::new(4, 2, 0);
    disabled.feed(b"\x1b[?7labcdef");
    assert_eq!(wraps(&disabled, 0), [None, None]);
}

#[test]
fn wrapped_rows_follow_ring_rotation_history_and_eviction() {
    let mut terminal = Terminal::new(4, 2, 3);
    terminal.feed(b"abcdefghijklmnopqrst");
    assert_eq!(terminal.history_len(), 3);
    for offset in 1..=3 {
        assert_eq!(wraps(&terminal, offset), [Some(4), Some(4)]);
    }
    assert_eq!(wraps(&terminal, 0), [Some(4), None]);
    terminal.feed(b"\r\nu");
    assert_eq!(terminal.history_len(), 3);
    assert_eq!(wraps(&terminal, 0), [None, None]);
    assert_eq!(wraps(&terminal, 1), [Some(4), None]);
    assert_eq!(terminal.view_cell(0, 0, 3).ch, 'e');

    let mut single = Terminal::new(2, 1, 2);
    single.feed(b"abcdefg");
    assert_eq!(single.history_len(), 2);
    assert_eq!(single.view_row_wrap(0, 2), Some(2));
    assert_eq!(single.view_row_wrap(0, 1), Some(2));
    assert_eq!(single.view_row_wrap(0, 0), None);
}

#[test]
fn resize_retains_complete_wraps_and_severs_clipped_successors() {
    let mut terminal = Terminal::new(4, 3, 10);
    terminal.feed(b"abcdefghij");
    terminal.resize(6, 2);
    assert_eq!(terminal.history_len(), 1);
    assert_eq!(terminal.view_row_wrap(0, 1), Some(4));
    assert_eq!(wraps(&terminal, 0), [Some(4), None]);
    terminal.resize(3, 2);
    assert_eq!(terminal.view_row_wrap(0, 1), None);
    assert_eq!(wraps(&terminal, 0), [None, None]);
    terminal.resize(6, 2);
    assert_eq!(terminal.view_row_wrap(0, 1), Some(4));
    assert_eq!(wraps(&terminal, 0), [None, None]);

    let mut clipped = Terminal::new(4, 4, 10);
    clipped.feed(b"abcdefghijklmn\x1b[H");
    clipped.resize(4, 2);
    assert_eq!(wraps(&clipped, 0), [Some(4), None]);
}

#[test]
fn insert_delete_and_manual_scroll_sever_only_changed_row_adjacencies() {
    let mut inserted = Terminal::new(4, 4, 10);
    inserted.feed(b"abcdefghijklmn\x1b[2;3r\x1b[2;1H\x1b[L");
    assert_eq!(wraps(&inserted, 0), [None, None, None, None]);
    let mut deleted = Terminal::new(4, 4, 10);
    deleted.feed(b"abcdefghijklmn\x1b[2;1H\x1b[M");
    assert_eq!(wraps(&deleted, 0), [None, Some(4), None, None]);
    assert_eq!(deleted.cell(0, 1).ch, 'i');
    assert_eq!(deleted.cell(0, 2).ch, 'm');

    for sequence in [b"\x1b[S".as_slice(), b"\x1b[T".as_slice()] {
        let mut terminal = Terminal::new(4, 2, 10);
        terminal.feed(b"abcdefghijkl");
        assert_eq!(terminal.view_row_wrap(0, 1), Some(4));
        terminal.feed(sequence);
        assert_eq!(terminal.view_row_wrap(0, 1), None);
        assert_eq!(wraps(&terminal, 0), [None, None]);
    }
}

#[test]
fn erasure_and_character_shifts_invalidate_the_affected_wrap_endpoint() {
    for sequence in [
        b"\x1b[H\x1b[2K".as_slice(),
        b"\x1b[1;4H\x1b[X".as_slice(),
        b"\x1b[H\x1b[@".as_slice(),
        b"\x1b[H\x1b[P".as_slice(),
    ] {
        let mut terminal = Terminal::new(4, 3, 0);
        terminal.feed(b"abcdefghij");
        terminal.feed(sequence);
        assert_eq!(wraps(&terminal, 0), [None, Some(4), None]);
    }
    let mut middle = Terminal::new(4, 3, 0);
    middle.feed(b"abcdefghij\x1b[1;2H\x1b[X");
    assert_eq!(wraps(&middle, 0), [Some(4), Some(4), None]);
    let mut wide = Terminal::new(4, 2, 0);
    wide.feed("ab界z\x1b[1;3H\x1b[X".as_bytes());
    assert_eq!(wraps(&wide, 0), [None, None]);
    for sequence in [
        b"\x1b[2J".as_slice(),
        b"\x1b#8".as_slice(),
        b"\x1bc".as_slice(),
    ] {
        middle.feed(sequence);
        assert_eq!(wraps(&middle, 0), vec![None; middle.rows()]);
    }
}

#[test]
fn alternate_screens_keep_independent_wrap_metadata_through_resize_and_clear() {
    let mut terminal = Terminal::new(4, 2, 10);
    terminal.feed(b"abcdefghijkl");
    assert_eq!(terminal.history_len(), 1);
    terminal.feed(b"\x1b[?1049h");
    assert_eq!(terminal.history_len(), 0);
    assert_eq!(wraps(&terminal, 99), [None, None]);
    terminal.feed(b"uvwxyz");
    assert_eq!(wraps(&terminal, 0), [Some(4), None]);
    terminal.resize(6, 2);
    terminal.feed(b"\x1b[?1049l");
    assert_eq!(terminal.history_len(), 1);
    assert_eq!(terminal.view_row_wrap(0, 1), Some(4));
    assert_eq!(wraps(&terminal, 0), [Some(4), None]);
    terminal.feed(b"\x1b[?1049h");
    assert_eq!(wraps(&terminal, 0), [None, None]);
}

#[test]
fn replacing_screen_rows_breaks_history_links_without_mutating_inactive_history() {
    for sequence in [
        b"\x1b[H\x1b[S".as_slice(),
        b"\x1b[H\x1b[T".as_slice(),
        b"\x1b[H\x1b[2J".as_slice(),
        b"\x1b[H\x1b[J".as_slice(),
        b"\x1b[H\x1b[2K".as_slice(),
        b"\x1b[H\x1b[4X".as_slice(),
        b"\x1b#8".as_slice(),
    ] {
        let mut terminal = Terminal::new(4, 2, 10);
        terminal.feed(b"abcdefghijkl");
        terminal.feed(b"\x1b[?1049h");
        terminal.feed(b"uvwxyz");
        terminal.feed(sequence);
        terminal.feed(b"\x1b[?1049l");
        assert_eq!(terminal.view_row_wrap(0, 1), Some(4));
        terminal.feed(sequence);
        assert_eq!(terminal.view_row_wrap(0, 1), None);
    }
}

#[test]
fn soft_wrap_metadata_is_independent_of_pty_chunk_boundaries() {
    let input = "abc界\u{301}defghi\r\njklmnop\x1b[2;1H\x1b[P\x1b[3;1Hqrstuvwxyz";
    let mut whole = Terminal::new(4, 3, 8);
    whole.feed(input.as_bytes());
    for chunk in 1..=input.len() {
        let mut split = Terminal::new(4, 3, 8);
        for bytes in input.as_bytes().chunks(chunk) {
            split.feed(bytes);
        }
        assert_eq!(split.history_len(), whole.history_len());
        for offset in 0..=whole.history_len() {
            assert_eq!(wraps(&split, offset), wraps(&whole, offset));
            for row in 0..whole.rows() {
                for col in 0..whole.cols() {
                    assert_eq!(
                        split.view_cell(col, row, offset),
                        whole.view_cell(col, row, offset)
                    );
                }
            }
        }
    }
}
