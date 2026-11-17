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
