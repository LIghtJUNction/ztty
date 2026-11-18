extern crate alloc;
#[path = "../src/search.rs"]
mod search;
use zterm::Terminal;

#[test]
fn history_search_wraps_both_directions_and_counts_all_matches() {
    let mut terminal = Terminal::new(20, 3, 100);
    terminal.feed(b"alpha zero\r\nbeta\r\nalpha one\r\ngamma\r\nalpha two");
    let first = search::find(&terminal, "alpha", None, true);
    assert_eq!(first.count, 3);
    assert_eq!(first.ordinal, 1);
    let next = search::find(&terminal, "alpha", first.hit, true);
    assert_eq!(next.ordinal, 2);
    let previous = search::find(&terminal, "alpha", first.hit, false);
    assert_eq!(previous.ordinal, 3);
    assert_eq!(
        search::find(&terminal, "alpha", previous.hit, true).hit,
        first.hit
    );
    let (offset, y) = search::viewport(&terminal, first.hit.unwrap());
    assert!(offset > 0);
    assert!(y < terminal.rows());
}

#[test]
fn unicode_matches_select_complete_wide_and_combining_cells() {
    let mut terminal = Terminal::new(20, 3, 100);
    terminal.feed("a中e\u{301} b中".as_bytes());
    let chinese = search::find(&terminal, "中", None, true).hit.unwrap();
    assert_eq!((chinese.start, chinese.end), (1, 2));
    let accent = search::find(&terminal, "e\u{301}", None, true).hit.unwrap();
    assert_eq!((accent.start, accent.end), (3, 3));
    let bits = search::highlights(&terminal, "中", 0);
    assert_ne!(bits[0] & (1 << 1), 0);
    assert_ne!(bits[0] & (1 << 2), 0);
    assert_eq!(bits[0] & (1 << 3), 0);
}

#[test]
fn empty_absent_and_oversized_queries_allocate_no_highlight_map() {
    let mut terminal = Terminal::new(10, 2, 0);
    terminal.feed(b"hello");
    assert_eq!(search::find(&terminal, "missing", None, true).count, 0);
    assert!(search::find(&terminal, "", None, true).hit.is_none());
    assert!(search::highlights(&terminal, "", 0).is_empty());
    let long = "x".repeat(1025);
    assert!(search::highlights(&terminal, &long, 0).is_empty());
}

#[test]
fn visible_highlights_use_the_selected_scrollback_page() {
    let mut terminal = Terminal::new(12, 2, 8);
    terminal.feed(b"needle\r\nother\r\nlast");
    let hit = search::find(&terminal, "needle", None, true).hit.unwrap();
    let (offset, y) = search::viewport(&terminal, hit);
    let bits = search::highlights(&terminal, "needle", offset);
    assert_ne!(bits[(y * 12) / 64] & (1 << ((y * 12) % 64)), 0);
    assert!(
        search::highlights(&terminal, "needle", 0)
            .iter()
            .all(|b| *b == 0)
    );
}

fn highlighted(terminal: &Terminal, query: &str, offset: usize) -> Vec<(usize, usize)> {
    let bits = search::highlights(terminal, query, offset);
    (0..terminal.cols() * terminal.rows())
        .filter(|bit| bits[*bit / 64] & (1 << (*bit % 64)) != 0)
        .map(|bit| (bit % terminal.cols(), bit / terminal.cols()))
        .collect()
}

#[test]
fn soft_wrapped_matches_count_and_navigate_across_history_and_screen() {
    let mut terminal = Terminal::new(4, 3, 100);
    terminal.feed(b"xxneedle\r\nneedle\r\nxxneedle");
    let first = search::find(&terminal, "needle", None, true);
    assert_eq!((first.count, first.ordinal), (3, 1));
    let hit = first.hit.unwrap();
    assert_eq!((hit.row, hit.start, hit.end_row, hit.end), (0, 2, 1, 3));
    assert_eq!(search::text(&terminal, hit).as_deref(), Some("needle"));
    let second = search::find(&terminal, "needle", first.hit, true);
    assert_eq!(second.ordinal, 2);
    let last = search::find(&terminal, "needle", first.hit, false);
    assert_eq!(last.ordinal, 3);
    assert_eq!(
        search::find(&terminal, "needle", last.hit, true).hit,
        first.hit
    );
    assert_eq!(
        search::find(&terminal, "needle", second.hit, false).hit,
        first.hit
    );
    let (offset, y) = search::viewport(&terminal, hit);
    assert!(y + hit.end_row - hit.row < terminal.rows());
    assert_eq!(
        highlighted(&terminal, "needle", offset),
        [
            (2, 0),
            (3, 0),
            (0, 1),
            (1, 1),
            (2, 1),
            (3, 1),
            (0, 2),
            (1, 2),
            (2, 2),
            (3, 2)
        ]
    );
}

#[test]
fn explicit_line_breaks_never_create_a_cross_row_match() {
    let mut terminal = Terminal::new(4, 3, 10);
    terminal.feed(b"abcd");
    assert_eq!(terminal.view_row_wrap(0, 0), None);
    terminal.feed(b"\r\nefgh");
    assert_eq!(search::find(&terminal, "cdef", None, true).count, 0);
    assert!(highlighted(&terminal, "cdef", 0).is_empty());
    let mut wrapped = Terminal::new(4, 3, 10);
    wrapped.feed(b"abcdefgh");
    let hit = search::find(&wrapped, "cdef", None, true).hit.unwrap();
    assert_eq!((hit.row, hit.start, hit.end_row, hit.end), (0, 2, 1, 1));
    assert_eq!(search::text(&wrapped, hit).as_deref(), Some("cdef"));
}

#[test]
fn wide_wrap_padding_is_skipped_but_real_spaces_are_retained() {
    let mut terminal = Terminal::new(4, 3, 10);
    terminal.feed("abc界d".as_bytes());
    let hit = search::find(&terminal, "c界d", None, true).hit.unwrap();
    assert_eq!((hit.row, hit.start, hit.end_row, hit.end), (0, 2, 1, 2));
    assert_eq!(search::text(&terminal, hit).as_deref(), Some("c界d"));
    assert_eq!(
        highlighted(&terminal, "c界d", 0),
        [(2, 0), (0, 1), (1, 1), (2, 1)]
    );
    assert_eq!(search::find(&terminal, "c 界", None, true).count, 0);

    let mut spaces = Terminal::new(4, 3, 10);
    spaces.feed("ab 界d".as_bytes());
    let hit = search::find(&spaces, "b 界", None, true).hit.unwrap();
    assert_eq!(search::text(&spaces, hit).as_deref(), Some("b 界"));
    assert_eq!(search::find(&spaces, "b界", None, true).count, 0);
}

#[test]
fn wrapped_unicode_and_combining_matches_select_complete_cells() {
    let mut terminal = Terminal::new(4, 3, 10);
    terminal.feed("ab中\u{301}e\u{301}文".as_bytes());
    let query = "中\u{301}e\u{301}文";
    let hit = search::find(&terminal, query, None, true).hit.unwrap();
    assert_eq!((hit.row, hit.start, hit.end_row, hit.end), (0, 2, 1, 2));
    assert_eq!(search::text(&terminal, hit).as_deref(), Some(query));
    assert_eq!(
        highlighted(&terminal, query, 0),
        [(2, 0), (3, 0), (0, 1), (1, 1), (2, 1)]
    );
    let accents = search::find(&terminal, "\u{301}", None, true);
    assert_eq!(accents.count, 2);
    let next = search::find(&terminal, "\u{301}", accents.hit, true);
    assert_eq!((next.hit.unwrap().row, next.hit.unwrap().start), (1, 0));

    let mut same_cell = Terminal::new(2, 2, 0);
    same_cell.feed("e\u{301}\u{301}".as_bytes());
    let first = search::find(&same_cell, "\u{301}", None, true);
    let second = search::find(&same_cell, "\u{301}", first.hit, true);
    assert_eq!((first.count, second.ordinal), (2, 2));
    assert_ne!(first.hit, second.hit);
    assert_eq!(
        search::text(&same_cell, second.hit.unwrap()).as_deref(),
        Some("e\u{301}\u{301}")
    );
}

#[test]
fn partial_matches_at_both_viewport_edges_are_highlighted_and_copy_in_full() {
    let mut terminal = Terminal::new(3, 2, 20);
    terminal.feed(b"xxABCDEFGHIJKLzz");
    let query = "ABCDEFGHIJKL";
    let hit = search::find(&terminal, query, None, true).hit.unwrap();
    assert_eq!((hit.row, hit.start, hit.end_row, hit.end), (0, 2, 4, 1));
    assert_eq!(search::text(&terminal, hit).as_deref(), Some(query));
    assert_eq!(search::viewport(&terminal, hit), (4, 0));
    assert_eq!(
        highlighted(&terminal, query, 4),
        [(2, 0), (0, 1), (1, 1), (2, 1)]
    );
    assert_eq!(highlighted(&terminal, query, 2).len(), 6);
    assert_eq!(highlighted(&terminal, query, 0), [(0, 0), (1, 0)]);
}

#[test]
fn visible_non_overlapping_matches_keep_the_offscreen_logical_line_alignment() {
    let mut terminal = Terminal::new(3, 1, 5);
    terminal.feed(b"aaaaaaa");
    assert_eq!(search::find(&terminal, "aa", None, true).count, 3);
    assert_eq!(highlighted(&terminal, "aa", 1), [(0, 0), (1, 0), (2, 0)]);
    assert!(highlighted(&terminal, "aa", 0).is_empty());
}

#[test]
fn maximum_query_spans_many_rows_without_building_a_logical_line() {
    let query = "x".repeat(1024);
    let mut terminal = Terminal::new(7, 2, 200);
    terminal.feed(query.as_bytes());
    terminal.feed(b"y");
    let result = search::find(&terminal, &query, None, true);
    assert_eq!(result.count, 1);
    assert_eq!(
        search::text(&terminal, result.hit.unwrap()).as_deref(),
        Some(query.as_str())
    );
    assert_eq!(highlighted(&terminal, &query, 0).len(), 9);
}

#[test]
fn copied_matches_reject_stale_history_and_resized_coordinates() {
    let mut terminal = Terminal::new(4, 2, 1);
    terminal.feed(b"abcdefgh");
    let hit = search::find(&terminal, "cdef", None, true).hit.unwrap();
    terminal.feed(b"");
    terminal.resize(4, 2);
    assert_eq!(search::text(&terminal, hit).as_deref(), Some("cdef"));
    terminal.feed(b"ijklmnop");
    assert!(search::text(&terminal, hit).is_none());
    let fresh = search::find(&terminal, "klmn", None, true).hit.unwrap();
    terminal.resize(8, 2);
    assert!(search::text(&terminal, fresh).is_none());
    let fresh = search::find(&terminal, "klmn", None, true).hit.unwrap();
    assert_eq!(search::text(&terminal, fresh).as_deref(), Some("klmn"));
}

#[test]
fn clipped_history_does_not_invent_a_contiguous_match() {
    let mut terminal = Terminal::new(5, 2, 10);
    terminal.feed(b"abcdefghijk");
    assert_eq!(terminal.history_len(), 1);
    assert_eq!(search::find(&terminal, "defg", None, true).count, 1);
    terminal.resize(4, 2);
    assert_eq!(search::find(&terminal, "dfg", None, true).count, 0);
    assert_eq!(search::find(&terminal, "defg", None, true).count, 0);
    terminal.resize(6, 2);
    assert_eq!(search::find(&terminal, "defg", None, true).count, 1);
}
