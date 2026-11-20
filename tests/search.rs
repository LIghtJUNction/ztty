extern crate alloc;
#[path = "../src/search.rs"]
mod search;
use search::Case::{Insensitive, Sensitive};
use zterm::Terminal;

#[test]
fn history_search_wraps_both_directions_and_counts_all_matches() {
    let mut terminal = Terminal::new(20, 3, 100);
    terminal.feed(b"alpha zero\r\nbeta\r\nalpha one\r\ngamma\r\nalpha two");
    let first = search::find(&terminal, "alpha", Sensitive, None, true);
    assert_eq!(first.count, 3);
    assert_eq!(first.ordinal, 1);
    let next = search::find(&terminal, "alpha", Sensitive, first.hit, true);
    assert_eq!(next.ordinal, 2);
    let previous = search::find(&terminal, "alpha", Sensitive, first.hit, false);
    assert_eq!(previous.ordinal, 3);
    assert_eq!(
        search::find(&terminal, "alpha", Sensitive, previous.hit, true).hit,
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
    let chinese = search::find(&terminal, "中", Sensitive, None, true)
        .hit
        .unwrap();
    assert_eq!((chinese.start, chinese.end), (1, 2));
    let accent = search::find(&terminal, "e\u{301}", Sensitive, None, true)
        .hit
        .unwrap();
    assert_eq!((accent.start, accent.end), (3, 3));
    let bits = search::highlights(&terminal, "中", Sensitive, 0);
    assert_ne!(bits[0] & (1 << 1), 0);
    assert_ne!(bits[0] & (1 << 2), 0);
    assert_eq!(bits[0] & (1 << 3), 0);
}

#[test]
fn empty_absent_and_oversized_queries_allocate_no_highlight_map() {
    let mut terminal = Terminal::new(10, 2, 0);
    terminal.feed(b"hello");
    assert_eq!(
        search::find(&terminal, "missing", Sensitive, None, true).count,
        0
    );
    assert!(
        search::find(&terminal, "", Sensitive, None, true)
            .hit
            .is_none()
    );
    assert!(search::highlights(&terminal, "", Sensitive, 0).is_empty());
    let long = "x".repeat(1025);
    assert!(search::highlights(&terminal, &long, Sensitive, 0).is_empty());
}

#[test]
fn visible_highlights_use_the_selected_scrollback_page() {
    let mut terminal = Terminal::new(12, 2, 8);
    terminal.feed(b"needle\r\nother\r\nlast");
    let hit = search::find(&terminal, "needle", Sensitive, None, true)
        .hit
        .unwrap();
    let (offset, y) = search::viewport(&terminal, hit);
    let bits = search::highlights(&terminal, "needle", Sensitive, offset);
    assert_ne!(bits[(y * 12) / 64] & (1 << ((y * 12) % 64)), 0);
    assert!(
        search::highlights(&terminal, "needle", Sensitive, 0)
            .iter()
            .all(|b| *b == 0)
    );
}

fn highlighted(terminal: &Terminal, query: &str, offset: usize) -> Vec<(usize, usize)> {
    let bits = search::highlights(terminal, query, Sensitive, offset);
    (0..terminal.cols() * terminal.rows())
        .filter(|bit| bits[*bit / 64] & (1 << (*bit % 64)) != 0)
        .map(|bit| (bit % terminal.cols(), bit / terminal.cols()))
        .collect()
}

#[test]
fn soft_wrapped_matches_count_and_navigate_across_history_and_screen() {
    let mut terminal = Terminal::new(4, 3, 100);
    terminal.feed(b"xxneedle\r\nneedle\r\nxxneedle");
    let first = search::find(&terminal, "needle", Sensitive, None, true);
    assert_eq!((first.count, first.ordinal), (3, 1));
    let hit = first.hit.unwrap();
    assert_eq!((hit.row, hit.start, hit.end_row, hit.end), (0, 2, 1, 3));
    assert_eq!(search::text(&terminal, hit).as_deref(), Some("needle"));
    let second = search::find(&terminal, "needle", Sensitive, first.hit, true);
    assert_eq!(second.ordinal, 2);
    let last = search::find(&terminal, "needle", Sensitive, first.hit, false);
    assert_eq!(last.ordinal, 3);
    assert_eq!(
        search::find(&terminal, "needle", Sensitive, last.hit, true).hit,
        first.hit
    );
    assert_eq!(
        search::find(&terminal, "needle", Sensitive, second.hit, false).hit,
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
    assert_eq!(
        search::find(&terminal, "cdef", Sensitive, None, true).count,
        0
    );
    assert!(highlighted(&terminal, "cdef", 0).is_empty());
    let mut wrapped = Terminal::new(4, 3, 10);
    wrapped.feed(b"abcdefgh");
    let hit = search::find(&wrapped, "cdef", Sensitive, None, true)
        .hit
        .unwrap();
    assert_eq!((hit.row, hit.start, hit.end_row, hit.end), (0, 2, 1, 1));
    assert_eq!(search::text(&wrapped, hit).as_deref(), Some("cdef"));
}

#[test]
fn wide_wrap_padding_is_skipped_but_real_spaces_are_retained() {
    let mut terminal = Terminal::new(4, 3, 10);
    terminal.feed("abc界d".as_bytes());
    let hit = search::find(&terminal, "c界d", Sensitive, None, true)
        .hit
        .unwrap();
    assert_eq!((hit.row, hit.start, hit.end_row, hit.end), (0, 2, 1, 2));
    assert_eq!(search::text(&terminal, hit).as_deref(), Some("c界d"));
    assert_eq!(
        highlighted(&terminal, "c界d", 0),
        [(2, 0), (0, 1), (1, 1), (2, 1)]
    );
    assert_eq!(
        search::find(&terminal, "c 界", Sensitive, None, true).count,
        0
    );

    let mut spaces = Terminal::new(4, 3, 10);
    spaces.feed("ab 界d".as_bytes());
    let hit = search::find(&spaces, "b 界", Sensitive, None, true)
        .hit
        .unwrap();
    assert_eq!(search::text(&spaces, hit).as_deref(), Some("b 界"));
    assert_eq!(search::find(&spaces, "b界", Sensitive, None, true).count, 0);
}

#[test]
fn wrapped_unicode_and_combining_matches_select_complete_cells() {
    let mut terminal = Terminal::new(4, 3, 10);
    terminal.feed("ab中\u{301}e\u{301}文".as_bytes());
    let query = "中\u{301}e\u{301}文";
    let hit = search::find(&terminal, query, Sensitive, None, true)
        .hit
        .unwrap();
    assert_eq!((hit.row, hit.start, hit.end_row, hit.end), (0, 2, 1, 2));
    assert_eq!(search::text(&terminal, hit).as_deref(), Some(query));
    assert_eq!(
        highlighted(&terminal, query, 0),
        [(2, 0), (3, 0), (0, 1), (1, 1), (2, 1)]
    );
    let accents = search::find(&terminal, "\u{301}", Sensitive, None, true);
    assert_eq!(accents.count, 2);
    let next = search::find(&terminal, "\u{301}", Sensitive, accents.hit, true);
    assert_eq!((next.hit.unwrap().row, next.hit.unwrap().start), (1, 0));

    let mut same_cell = Terminal::new(2, 2, 0);
    same_cell.feed("e\u{301}\u{301}".as_bytes());
    let first = search::find(&same_cell, "\u{301}", Sensitive, None, true);
    let second = search::find(&same_cell, "\u{301}", Sensitive, first.hit, true);
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
    let hit = search::find(&terminal, query, Sensitive, None, true)
        .hit
        .unwrap();
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
    assert_eq!(
        search::find(&terminal, "aa", Sensitive, None, true).count,
        3
    );
    assert_eq!(highlighted(&terminal, "aa", 1), [(0, 0), (1, 0), (2, 0)]);
    assert!(highlighted(&terminal, "aa", 0).is_empty());
}

#[test]
fn maximum_query_spans_many_rows_without_building_a_logical_line() {
    let query = "x".repeat(1024);
    let mut terminal = Terminal::new(7, 2, 200);
    terminal.feed(query.as_bytes());
    terminal.feed(b"y");
    let result = search::find(&terminal, &query, Sensitive, None, true);
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
    let hit = search::find(&terminal, "cdef", Sensitive, None, true)
        .hit
        .unwrap();
    terminal.feed(b"");
    terminal.resize(4, 2);
    assert_eq!(search::text(&terminal, hit).as_deref(), Some("cdef"));
    terminal.feed(b"ijklmnop");
    assert!(search::text(&terminal, hit).is_none());
    let fresh = search::find(&terminal, "klmn", Sensitive, None, true)
        .hit
        .unwrap();
    terminal.resize(8, 2);
    assert!(search::text(&terminal, fresh).is_none());
    let fresh = search::find(&terminal, "klmn", Sensitive, None, true)
        .hit
        .unwrap();
    assert_eq!(search::text(&terminal, fresh).as_deref(), Some("klmn"));
}

#[test]
fn reflowed_history_preserves_text_without_inventing_a_contiguous_match() {
    let mut terminal = Terminal::new(5, 2, 10);
    terminal.feed(b"abcdefghijk");
    assert_eq!(terminal.history_len(), 1);
    assert_eq!(
        search::find(&terminal, "defg", Sensitive, None, true).count,
        1
    );
    terminal.resize(4, 2);
    assert_eq!(
        search::find(&terminal, "dfg", Sensitive, None, true).count,
        0
    );
    assert_eq!(
        search::find(&terminal, "defg", Sensitive, None, true).count,
        1
    );
    terminal.resize(6, 2);
    assert_eq!(
        search::find(&terminal, "defg", Sensitive, None, true).count,
        1
    );
}

#[test]
fn case_switching_keeps_counts_navigation_and_original_text_consistent() {
    let mut terminal = Terminal::new(12, 2, 12);
    terminal.feed(b"Alpha\r\nalpha\r\nALPHA");
    let exact = search::find(&terminal, "alpha", Sensitive, None, true);
    assert_eq!((exact.count, exact.ordinal), (1, 1));
    assert_eq!(
        search::text(&terminal, exact.hit.unwrap()).as_deref(),
        Some("alpha")
    );

    let first = search::find(&terminal, "alpha", Insensitive, None, true);
    assert_eq!((first.count, first.ordinal), (3, 1));
    assert_eq!(
        search::text(&terminal, first.hit.unwrap()).as_deref(),
        Some("Alpha")
    );
    let last = search::find(&terminal, "alpha", Insensitive, first.hit, false);
    assert_eq!((last.count, last.ordinal), (3, 3));
    assert_eq!(
        search::text(&terminal, last.hit.unwrap()).as_deref(),
        Some("ALPHA")
    );
    assert_eq!(
        search::find(&terminal, "alpha", Insensitive, last.hit, true).hit,
        first.hit
    );
    assert_eq!(
        search::find(&terminal, "Alpha", Insensitive, None, true).count,
        3
    );
}

#[test]
fn unicode_lowercase_matches_preserve_wide_and_combining_cell_coordinates() {
    let mut terminal = Terminal::new(4, 3, 20);
    terminal.feed("xÉ中I\u{301}Å".as_bytes());
    let query = "é中i\u{301}å";
    let result = search::find(&terminal, query, Insensitive, None, true);
    assert_eq!(result.count, 1);
    let hit = result.hit.unwrap();
    assert_eq!((hit.row, hit.start, hit.end_row, hit.end), (0, 1, 1, 1));
    assert_eq!(
        search::text(&terminal, hit).as_deref(),
        Some("É中I\u{301}Å")
    );
    let bits = search::highlights(&terminal, query, Insensitive, 0);
    assert_eq!(bits[0], 0b0011_1110);
}

#[test]
fn lowercase_expansion_search_and_viewport_suffix_use_normalized_bytes() {
    let mut terminal = Terminal::new(2, 1, 12);
    terminal.feed("xxAİZ".as_bytes());
    let query = "ai\u{307}z";
    let result = search::find(&terminal, query, Insensitive, None, true);
    assert_eq!(result.count, 1);
    let hit = result.hit.unwrap();
    assert_eq!(search::text(&terminal, hit).as_deref(), Some("AİZ"));
    assert_eq!(search::highlights(&terminal, query, Insensitive, 1)[0], 3);
    assert_eq!(search::highlights(&terminal, query, Insensitive, 0)[0], 1);
    assert_eq!(
        search::find(&terminal, "i\u{307}", Insensitive, None, true).count,
        1
    );

    let mut exact_cells = Terminal::new(8, 2, 0);
    exact_cells.feed("İİ\r\nẞ".as_bytes());
    let first = search::find(&exact_cells, "i", Insensitive, None, true);
    let next = search::find(&exact_cells, "i", Insensitive, first.hit, true);
    assert_eq!((first.count, next.ordinal), (2, 2));
    assert_eq!(
        search::text(&exact_cells, next.hit.unwrap()).as_deref(),
        Some("İ")
    );
    assert_eq!(
        search::find(&exact_cells, "ß", Insensitive, None, true).count,
        1
    );
    // Lowercase comparison is intentionally not accent normalization or full
    // Unicode case folding: sharp S is not the two-character string "ss".
    assert_eq!(
        search::find(&exact_cells, "ss", Insensitive, None, true).count,
        0
    );
}

#[test]
fn ignore_case_keeps_line_boundaries_and_query_limits() {
    let mut terminal = Terminal::new(4, 2, 10);
    terminal.feed(b"ABCD\r\nEFGH");
    assert_eq!(
        search::find(&terminal, "cdef", Insensitive, None, true).count,
        0
    );
    assert!(
        search::find(&terminal, "", Insensitive, None, true)
            .hit
            .is_none()
    );
    assert!(search::highlights(&terminal, &"X".repeat(1025), Insensitive, 0).is_empty());

    let query = "İ".repeat(512);
    let mut wrapped = Terminal::new(80, 3, 100);
    wrapped.feed(query.as_bytes());
    let result = search::find(&wrapped, &query, Insensitive, None, true);
    assert_eq!(result.count, 1);
    assert_eq!(
        search::text(&wrapped, result.hit.unwrap()).as_deref(),
        Some(query.as_str())
    );
}

#[test]
fn emoji_substring_search_selects_and_copies_the_complete_cluster() {
    let mut terminal = Terminal::new(16, 3, 20);
    terminal.feed("A👩🏽‍💻B".as_bytes());
    let hit = search::find(&terminal, "💻", Sensitive, None, true)
        .hit
        .unwrap();
    assert_eq!((hit.start, hit.end), (1, 2));
    assert_eq!(search::text(&terminal, hit).as_deref(), Some("👩🏽‍💻"));
    let bits = search::highlights(&terminal, "💻", Sensitive, 0);
    assert_ne!(bits[0] & (1 << 1), 0);
    assert_ne!(bits[0] & (1 << 2), 0);
    let hit = search::find(&terminal, "🏽‍💻B", Sensitive, None, true)
        .hit
        .unwrap();
    assert_eq!(search::text(&terminal, hit).as_deref(), Some("👩🏽‍💻B"));
}

/// The literal string oracle is independent of the streaming matcher. Narrow
/// cells force soft wraps while z noise exercises bytes outside any prefix.
#[test]
fn sparse_and_repetitive_ascii_search_matches_literal_string_oracle() {
    const ALPHABET: &[u8] = b"abz";
    const QUERIES: &[&str] = &[
        "a", "b", "aa", "ab", "ba", "bb", "aba", "abab", "baba", "zz",
    ];
    for length in 0..=6u32 {
        for mut encoded in 0..3usize.pow(length) {
            let mut input = String::new();
            for _ in 0..length {
                input.push(ALPHABET[encoded % ALPHABET.len()] as char);
                encoded /= ALPHABET.len();
            }
            let mut terminal = Terminal::new(3, 2, 16);
            terminal.feed(input.as_bytes());
            for &query in QUERIES {
                let expected: Vec<_> = input.match_indices(query).map(|(at, _)| at).collect();
                let mut result = search::find(&terminal, query, Sensitive, None, true);
                assert_eq!(result.count, expected.len(), "{input:?} / {query:?}");
                for (index, &at) in expected.iter().enumerate() {
                    assert_eq!(result.ordinal, index + 1, "{input:?} / {query:?}");
                    let hit = result.hit.unwrap();
                    assert_eq!(
                        (hit.row, hit.start, hit.end_row, hit.end),
                        (
                            at / 3,
                            at % 3,
                            (at + query.len() - 1) / 3,
                            (at + query.len() - 1) % 3
                        ),
                        "{input:?} / {query:?}",
                    );
                    assert_eq!(search::text(&terminal, hit).as_deref(), Some(query));
                    result = search::find(&terminal, query, Sensitive, Some(hit), true);
                }
                if expected.is_empty() {
                    assert!(result.hit.is_none());
                } else {
                    assert_eq!(result.ordinal, 1, "{input:?} / {query:?}");
                    let last = search::find(&terminal, query, Sensitive, result.hit, false);
                    assert_eq!(last.ordinal, expected.len(), "{input:?} / {query:?}");
                }
            }
        }
    }
}

#[test]
fn sparse_unicode_lowercase_and_graphemes_keep_positions_after_idle_bytes() {
    let mut terminal = Terminal::new(7, 2, 64);
    terminal.feed("zzzzzzzzzzzAİ中e\u{301}zzzzzzzzAİ中e\u{301}zz".as_bytes());
    let query = "ai\u{307}中e\u{301}";
    let first = search::find(&terminal, query, Insensitive, None, true);
    assert_eq!((first.count, first.ordinal), (2, 1));
    let first_hit = first.hit.unwrap();
    assert_eq!(
        (
            first_hit.row,
            first_hit.start,
            first_hit.end_row,
            first_hit.end
        ),
        (1, 4, 2, 2)
    );
    assert_eq!(
        search::text(&terminal, first_hit).as_deref(),
        Some("Aİ中e\u{301}")
    );
    let second = search::find(&terminal, query, Insensitive, first.hit, true);
    assert_eq!((second.count, second.ordinal), (2, 2));
    let second_hit = second.hit.unwrap();
    assert_ne!(first_hit, second_hit);
    assert_eq!(
        search::text(&terminal, second_hit).as_deref(),
        Some("Aİ中e\u{301}")
    );
    assert_eq!(
        search::find(&terminal, query, Insensitive, second.hit, true).hit,
        first.hit,
    );
    let (offset, _) = search::viewport(&terminal, first_hit);
    let highlights = search::highlights(&terminal, query, Insensitive, offset);
    assert!(highlights.iter().any(|&bits| bits != 0));
}
