use zterm::Terminal;
use ztty::selection;

#[test]
fn autowrapped_text_copies_as_one_logical_line_in_either_direction() {
    let mut terminal = Terminal::new(6, 3, 20);
    terminal.feed(b"hello world");
    assert_eq!(selection::text(&terminal, (0, 0), (4, 1), 0), "hello world");
    assert_eq!(selection::text(&terminal, (4, 1), (0, 0), 0), "hello world");
}

#[test]
fn explicit_newlines_stay_separate_and_only_hard_line_padding_is_trimmed() {
    let mut terminal = Terminal::new(6, 4, 20);
    terminal.feed(b"one\r\ntwo \r\nlast");
    assert_eq!(
        selection::text(&terminal, (0, 0), (3, 2), 0),
        "one\ntwo\nlast"
    );
}

#[test]
fn wide_wrap_padding_is_omitted_and_partial_glyphs_copy_whole_cells() {
    let mut terminal = Terminal::new(5, 3, 20);
    terminal.feed("abcd中e\u{301}".as_bytes());
    assert_eq!(
        selection::text(&terminal, (0, 0), (2, 1), 0),
        "abcd中e\u{301}"
    );
    assert_eq!(selection::text(&terminal, (1, 1), (1, 1), 0), "中");
    assert_eq!(selection::text(&terminal, (2, 1), (2, 1), 0), "e\u{301}");
}

#[test]
fn selection_can_span_retained_history_and_more_than_one_viewport() {
    let mut terminal = Terminal::new(4, 2, 20);
    terminal.feed(b"abcdefghijklmnopqrst");
    assert_eq!(
        selection::text(&terminal, (0, 0), (3, 4), terminal.history_len()),
        "abcdefghijklmnopqrst",
    );
}

#[test]
fn invalid_or_outside_selection_coordinates_are_bounded() {
    let mut terminal = Terminal::new(4, 2, 0);
    terminal.feed(b"abcd\r\nef");
    assert_eq!(
        selection::text(&terminal, (usize::MAX, 0), (usize::MAX, 0), 0),
        "d"
    );
    assert!(selection::text(&terminal, (0, usize::MAX), (1, usize::MAX), 0).is_empty());
    assert_eq!(
        selection::text(&terminal, (0, 0), (usize::MAX, usize::MAX), 0),
        "abcd\nef"
    );
}
