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

fn selected_unit(terminal: &Terminal, point: (usize, usize), unit: selection::Unit) -> String {
    let (start, end) = selection::unit(terminal, point, 0, unit);
    selection::text(terminal, start, end, 0)
}

#[test]
fn word_selection_keeps_paths_and_stops_at_punctuation_and_spaces() {
    let mut terminal = Terminal::new(40, 3, 20);
    terminal.feed(b"(src/main.rs), next_word");
    assert_eq!(
        selected_unit(&terminal, (5, 0), selection::Unit::Word),
        "src/main.rs"
    );
    assert_eq!(
        selected_unit(&terminal, (19, 0), selection::Unit::Word),
        "next_word"
    );
}

#[test]
fn word_selection_handles_wide_continuations_and_combining_marks() {
    let mut terminal = Terminal::new(20, 3, 20);
    terminal.feed("A 中e\u{301}文 Z".as_bytes());
    assert_eq!(
        selected_unit(&terminal, (3, 0), selection::Unit::Cell),
        "中"
    );
    assert_eq!(
        selected_unit(&terminal, (3, 0), selection::Unit::Word),
        "中e\u{301}文"
    );
    assert_eq!(
        selected_unit(&terminal, (6, 0), selection::Unit::Word),
        "中e\u{301}文"
    );
}

#[test]
fn word_and_line_selection_cross_soft_wraps_but_not_hard_newlines() {
    let mut terminal = Terminal::new(6, 5, 20);
    terminal.feed(b"abcdefghij\r\nnext");
    assert_eq!(
        selected_unit(&terminal, (1, 1), selection::Unit::Word),
        "abcdefghij"
    );
    assert_eq!(
        selected_unit(&terminal, (1, 1), selection::Unit::Line),
        "abcdefghij"
    );
    assert_eq!(
        selected_unit(&terminal, (1, 2), selection::Unit::Line),
        "next"
    );
}

#[test]
fn word_selection_skips_wide_character_wrap_padding() {
    let mut terminal = Terminal::new(5, 4, 20);
    terminal.feed("abcd中e\u{301}".as_bytes());
    assert_eq!(
        selected_unit(&terminal, (1, 1), selection::Unit::Word),
        "abcd中e\u{301}"
    );
}

#[test]
fn selection_units_read_the_scrolled_view_and_clamp_pointer_coordinates() {
    let mut terminal = Terminal::new(6, 2, 20);
    terminal.feed(b"abcdefghi\r\nnext\r\nlast");
    let offset = terminal.history_len();
    let (start, end) = selection::unit(&terminal, (1, 1), offset, selection::Unit::Line);
    assert_eq!(selection::text(&terminal, start, end, offset), "abcdefghi");
    let (start, end) = selection::unit(
        &terminal,
        (usize::MAX, usize::MAX),
        0,
        selection::Unit::Cell,
    );
    assert_eq!(start, (5, 1));
    assert_eq!(end, (5, 1));
}

#[test]
fn full_emoji_clusters_copy_from_either_cell_and_across_wraps() {
    let mut terminal = Terminal::new(4, 3, 20);
    terminal.feed("abc👩🏽‍💻Z".as_bytes());
    assert_eq!(selection::text(&terminal, (0, 0), (2, 1), 0), "abc👩🏽‍💻Z");
    assert_eq!(selection::text(&terminal, (1, 1), (1, 1), 0), "👩🏽‍💻");
    assert_eq!(
        selection::unit(&terminal, (1, 1), 0, selection::Unit::Cell),
        ((0, 1), (1, 1))
    );
}
