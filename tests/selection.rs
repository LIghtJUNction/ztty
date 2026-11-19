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

#[test]
fn reflow_view_keeps_selected_unicode_text_in_both_directions() {
    for reversed in [false, true] {
        let mut terminal = Terminal::new(6, 4, 128);
        terminal.feed("ab👩🏽‍💻cdEFGH\r\nhard".as_bytes());
        let (start, end) = if reversed {
            ((3, 1), (1, 0))
        } else {
            ((1, 0), (3, 1))
        };
        let expected = selection::text(&terminal, start, end, 0);
        assert_eq!(expected, "b👩🏽‍💻cdEFGH");
        let mut points = Some((start, end));
        let mut history = 0;
        for cols in [1, 3, 8, 6] {
            let mut view = selection::ViewAnchor::capture(&terminal, history, points);
            terminal.resize_anchored(cols, 4, view.anchors_mut());
            let restored = view.restore(&terminal);
            history = restored.history;
            points = restored.selection;
            let (start, end) = points.expect("retained selection is remapped");
            assert_eq!((start.1, start.0) > (end.1, end.0), reversed);
            assert_eq!(
                selection::text(&terminal, start, end, history),
                expected,
                "width {cols}"
            );
        }
    }
}

#[test]
fn reflow_view_scroll_anchor_tracks_text_instead_of_old_row_count() {
    let mut terminal = Terminal::new(4, 2, 64);
    terminal.feed(b"abcdefghijklmnopqrstuvwx");
    let old_history = terminal.history_len();
    let mut view = selection::ViewAnchor::capture(&terminal, old_history - 1, None);
    assert_eq!(terminal.view_cell(0, 0, old_history - 1).ch, 'e');
    terminal.resize_anchored(2, 2, view.anchors_mut());
    let restored = view.restore(&terminal);
    assert_eq!(terminal.view_cell(0, 0, restored.history).ch, 'e');
    assert!(restored.history > old_history - 1);
    assert_eq!(restored.selection, None);
}

#[test]
fn reflow_view_without_selection_continues_following_the_bottom() {
    let mut terminal = Terminal::new(6, 2, 64);
    terminal.feed(b"abcdefghijklmnopqrstuvwx");
    let mut view = selection::ViewAnchor::capture(&terminal, 0, None);
    terminal.resize_anchored(2, 2, view.anchors_mut());
    let restored = view.restore(&terminal);
    assert_eq!(restored.history, 0);
    assert_eq!(restored.selection, None);
}

#[test]
fn reflow_view_evicted_selection_is_cleared_not_reassigned() {
    let mut terminal = Terminal::new(8, 2, 2);
    terminal.feed(b"abcdefghijklmnop");
    let mut view = selection::ViewAnchor::capture(&terminal, 0, Some(((0, 0), (3, 0))));
    terminal.resize_anchored(1, 1, view.anchors_mut());
    let restored = view.restore(&terminal);
    assert_eq!(restored.selection, None);
    assert_eq!(restored.history, 0);
    assert_eq!(terminal.cell(0, 0).ch, 'p');
}

#[test]
fn reflow_view_line_end_boundary_does_not_select_the_previous_glyph() {
    let mut terminal = Terminal::new(8, 4, 32);
    terminal.feed(b"abc\r\nDEF");
    let expected = selection::text(&terminal, (5, 0), (1, 1), 0);
    assert_eq!(expected, "\nDE");
    let mut view = selection::ViewAnchor::capture(&terminal, 0, Some(((5, 0), (1, 1))));
    terminal.resize_anchored(3, 4, view.anchors_mut());
    let restored = view.restore(&terminal);
    let (start, end) = restored.selection.unwrap();
    assert_eq!(start, (3, 0));
    assert_eq!(
        selection::text(&terminal, start, end, restored.history),
        expected
    );
}

#[test]
fn reflow_view_height_resize_preserves_history_and_a_complete_wide_cell() {
    let mut terminal = Terminal::new(5, 4, 32);
    terminal.feed("a界b\r\nsecond\r\nlast".as_bytes());
    let expected = selection::text(&terminal, (2, 0), (2, 0), 0);
    assert_eq!(expected, "界");
    let mut view = selection::ViewAnchor::capture(&terminal, 0, Some(((2, 0), (2, 0))));
    terminal.resize_anchored(5, 2, view.anchors_mut());
    let restored = view.restore(&terminal);
    let (start, end) = restored.selection.unwrap();
    assert!(restored.history > 0);
    assert_eq!(
        selection::text(&terminal, start, end, restored.history),
        expected
    );
}

#[test]
fn reflow_trims_logical_line_padding_kept_for_a_saved_cursor() {
    let mut terminal = Terminal::new(12, 6, 20);
    terminal.feed(b"ab\x1b[1;9H\x1b7\r\ncd");
    assert_eq!(selection::text(&terminal, (0, 0), (1, 1), 0), "ab\ncd");
    terminal.resize(3, 6);
    assert_eq!(terminal.view_row_wrap(0, 0), Some(3));
    assert_eq!(terminal.view_row_wrap(1, 0), Some(3));
    assert_eq!(selection::text(&terminal, (0, 0), (1, 3), 0), "ab\ncd");
}

#[test]
fn a_selection_ending_inside_a_soft_line_retains_its_spaces() {
    let mut terminal = Terminal::new(4, 3, 20);
    terminal.feed(b"ab  c");
    assert_eq!(selection::text(&terminal, (0, 0), (3, 0), 0), "ab  ");
    assert_eq!(selection::text(&terminal, (0, 0), (0, 1), 0), "ab  c");
}
