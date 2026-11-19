use crate::{Cell, ResizeAnchor, Terminal, TextPosition};
use alloc::{string::String, vec, vec::Vec};

fn retained_text(terminal: &Terminal) -> String {
    let history = terminal.history_len();
    let mut output = String::new();
    for index in 0..history + terminal.rows() {
        let (row, offset) = if index < history {
            (0, history - index)
        } else {
            (index - history, 0)
        };
        let wrapped = terminal.view_row_wrap(row, offset);
        let start = output.len();
        for col in 0..wrapped.unwrap_or(terminal.cols()) {
            let cell = terminal.view_cell(col, row, offset);
            if cell.width != 0 {
                output.extend(terminal.cell_chars(cell));
            }
        }
        if wrapped.is_none() {
            while output.len() > start && output.ends_with(' ') {
                output.pop();
            }
            output.push('\n');
        }
    }
    while output.ends_with('\n') {
        output.pop();
    }
    output
}

fn complete_cells(terminal: &Terminal) {
    let history = terminal.history_len();
    for index in 0..history + terminal.rows() {
        let (row, offset) = if index < history {
            (0, history - index)
        } else {
            (index - history, 0)
        };
        for col in 0..terminal.cols() {
            let cell = terminal.view_cell(col, row, offset);
            if cell.width == 2 {
                assert!(col + 1 < terminal.cols());
                let next = terminal.view_cell(col + 1, row, offset);
                assert_eq!((next.width, next.ch, next.grapheme_id), (0, '\0', 0));
            } else if cell.width == 0 {
                assert!(col != 0);
                assert_eq!(terminal.view_cell(col - 1, row, offset).width, 2);
            }
        }
    }
}

#[test]
fn reflow_keeps_soft_lines_hard_breaks_spaces_and_complete_graphemes() {
    let mut terminal = Terminal::new(6, 4, 128);
    terminal.feed("ab  界👩🏽‍💻Z\r\nhard\r\nnext".as_bytes());
    let expected = "ab  界👩🏽‍💻Z\nhard\nnext";
    assert_eq!(retained_text(&terminal), expected);
    for cols in [3, 1, 9, 4, 6] {
        terminal.resize(cols, 4);
        assert_eq!(retained_text(&terminal), expected, "width {cols}");
        complete_cells(&terminal);
    }
}

#[test]
fn reflow_delayed_wrap_cursor_does_not_overwrite_ascii_or_wide_text() {
    for text in ["abcd", "ab界", "👨‍👩‍👧‍👦🇨🇳"] {
        let mut terminal = Terminal::new(4, 4, 64);
        terminal.feed(text.as_bytes());
        terminal.resize(1, 4);
        complete_cells(&terminal);
        terminal.resize(4, 4);
        assert_eq!(retained_text(&terminal), text);
        terminal.feed(b"Q");
        assert_eq!(retained_text(&terminal), alloc::format!("{text}Q"));
        complete_cells(&terminal);
    }
    let mut terminal = Terminal::new(4, 3, 32);
    terminal.feed(b"abcd");
    terminal.resize(2, 3);
    assert_eq!(terminal.cursor(), (1, 1));
    terminal.feed(b"E");
    assert_eq!(terminal.cursor(), (1, 2));
    assert_eq!(retained_text(&terminal), "abcdE");
}

#[test]
fn reflow_saved_cursor_tracks_a_logical_cell_before_later_output() {
    let mut terminal = Terminal::new(6, 3, 32);
    terminal.feed(b"abcdefghi\x1b7jk");
    terminal.resize(4, 4);
    terminal.feed(b"\x1b8!");
    assert_eq!(terminal.cursor(), (2, 2));
    assert_eq!(retained_text(&terminal), "abcdefghi!k");
}

#[test]
fn reflow_sparse_boundaries_cover_empty_rows_and_paragraph_ends() {
    let mut terminal = Terminal::new(4, 4, 32);
    terminal.feed(b"abcd\r\n\r\nZ");
    let mut anchors = [
        ResizeAnchor::boundary(terminal.view_position(4, 0, 0)),
        ResizeAnchor::boundary(terminal.view_position(0, 1, 0)),
        ResizeAnchor::cell(terminal.view_position(0, 2, 0)),
    ];
    terminal.resize_anchored(2, 5, &mut anchors);
    assert_eq!(
        anchors[0].position,
        Some(TextPosition { line: 1, column: 2 })
    );
    assert_eq!(
        anchors[1].position,
        Some(TextPosition { line: 2, column: 0 })
    );
    assert_eq!(
        terminal
            .position_cell(anchors[2].position.unwrap())
            .unwrap()
            .ch,
        'Z'
    );
    assert_eq!(retained_text(&terminal), "abcd\n\nZ");
}

#[test]
fn reflow_lost_history_anchors_are_not_reassigned_to_new_text() {
    let mut terminal = Terminal::new(8, 2, 2);
    terminal.feed(b"abcdefghijklmnop");
    let mut anchors = [
        ResizeAnchor::cell(terminal.view_position(0, 0, 0)),
        ResizeAnchor::cell(terminal.view_position(7, 1, 0)),
    ];
    terminal.resize_anchored(2, 2, &mut anchors);
    assert_eq!(terminal.history_len(), 2);
    assert_eq!(anchors[0].position, None);
    assert_eq!(
        terminal
            .position_cell(anchors[1].position.unwrap())
            .unwrap()
            .ch,
        'p'
    );
    assert_eq!(retained_text(&terminal), "ijklmnop");
}

#[test]
fn reflow_existing_byte_budget_evicts_wider_rows_without_growing_the_limit() {
    let mut terminal = Terminal::new(512, 2, 65536);
    let mut line = vec![b'x'; 512];
    line.extend_from_slice(b"\r\n");
    for _ in 0..600 {
        terminal.feed(&line);
    }
    let old_history = terminal.history_len();
    let mut anchor = [ResizeAnchor::cell(terminal.view_position(
        0,
        0,
        old_history,
    ))];
    terminal.resize_anchored(1024, 2, &mut anchor);
    assert!(terminal.history_len() < old_history);
    assert!(terminal.history_len() * 1024 * core::mem::size_of::<Cell>() <= 8 * 1024 * 1024);
    assert_eq!(anchor[0].position, None);
    complete_cells(&terminal);
}

#[test]
fn reflow_addressed_cursor_clips_lower_tail_without_inventing_history() {
    let mut terminal = Terminal::new(4, 4, 32);
    terminal.feed(b"abcdefghijklmnop\x1b[H");
    let mut anchor = [ResizeAnchor::cell(terminal.view_position(1, 3, 0))];
    terminal.resize_anchored(2, 2, &mut anchor);
    assert_eq!(terminal.cursor(), (0, 0));
    assert_eq!(terminal.history_len(), 0);
    assert_eq!(retained_text(&terminal), "abcd");
    assert_eq!(terminal.view_row_wrap(1, 0), None);
    assert_eq!(anchor[0].position, None);
}

#[test]
fn reflow_primary_text_and_alternate_grid_remain_separate() {
    let mut terminal = Terminal::new(4, 3, 32);
    terminal.feed(b"abcdefghij\x1b[?1049hZZZZZZ");
    let mut anchor = [ResizeAnchor::cell(terminal.view_position(1, 1, 0))];
    terminal.resize_anchored(6, 2, &mut anchor);
    assert!(terminal.modes().alternate_screen);
    assert_eq!(terminal.cell(0, 0).ch, 'Z');
    assert_eq!(terminal.cell(4, 0).ch, ' ');
    assert_eq!(
        anchor[0].position,
        Some(TextPosition { line: 1, column: 1 })
    );
    terminal.feed(b"\x1b[?1049l");
    assert_eq!(retained_text(&terminal), "abcdefghij");
    assert_eq!(terminal.view_row_wrap(0, 0), Some(6));
}

#[test]
fn reflow_osc133_ranges_and_ids_survive_line_base_retreat_and_new_output() {
    let mut terminal = Terminal::new(4, 2, 64);
    terminal
        .feed(b"\x1b]133;A\x07\x1b]133;B\x07\x1b]133;C\x07abcdefghijklmnopqrst\x1b]133;D;0\x07");
    let id = terminal.commands().next().unwrap().id;
    let before = terminal.command_output(id, 1024).unwrap();
    let base = terminal.history_position();
    terminal.resize(20, 2);
    assert!(terminal.history_position() < base);
    assert_eq!(terminal.command_output(id, 1024).unwrap(), before);
    terminal
        .feed(b"\r\n\x1b]133;A\x07$ \x1b]133;B\x07go\r\n\x1b]133;C\x07next\r\n\x1b]133;D;0\x07");
    let next = terminal.commands().next_back().unwrap().id;
    assert!(next > id);
    assert_eq!(terminal.command_output(next, 1024).unwrap().text, "next\n");
    assert_eq!(terminal.command_input(next, 1024).unwrap().text, "go\n");
    assert_eq!(terminal.command_output(id, 1024).unwrap(), before);
    terminal.resize(3, 3);
    assert_eq!(terminal.command_output(next, 1024).unwrap().text, "next\n");
    let exact = terminal.command_output(next, 5).unwrap();
    assert_eq!(exact.text, "next\n");
    assert!(
        !exact.truncated,
        "discarded padding must not consume the byte limit"
    );
    assert!(exact.complete);
    assert_eq!(terminal.command_output(id, 1024).unwrap(), before);
}

#[test]
fn reflow_graphics_anchor_uses_the_same_sparse_mapping_as_its_text() {
    let mut terminal = Terminal::new(8, 3, 2);
    terminal.feed(b"ab\x1b_Ga=T,f=32,s=1,v=1,i=7,C=1;/////w==\x1b\\cdefghij");
    assert_eq!(terminal.graphics().anchors().collect::<Vec<_>>(), [(2, 0)]);
    let mut anchor = [ResizeAnchor::cell(terminal.view_position(2, 0, 0))];
    terminal.resize_anchored(2, 3, &mut anchor);
    let point = anchor[0].position.unwrap();
    let expected = (
        point.column as i32,
        (point.line as i64 - terminal.history_position() as i64) as i32,
    );
    assert_eq!(
        terminal.graphics().anchors().collect::<Vec<_>>(),
        [expected]
    );
    assert_eq!(terminal.position_cell(point).unwrap().ch, 'c');
    let draw = terminal.graphics_draws(1).next().unwrap();
    assert_eq!(
        (draw.dest.x, draw.dest.y, draw.dest.width, draw.dest.height),
        (0, 0, 1, 1)
    );
    terminal.resize_anchored(1, 1, &mut anchor);
    assert_eq!(anchor[0].position, None);
    assert_eq!(terminal.graphics().anchors().count(), 0);
    assert_eq!(terminal.graphics().image_count(), 1);
}

#[test]
fn reflow_height_only_resize_keeps_absolute_primary_anchors_and_drops_alt_rows() {
    let mut terminal = Terminal::new(4, 4, 32);
    terminal.feed(b"one\r\ntwo\r\nlast");
    let mut anchor = [ResizeAnchor::cell(terminal.view_position(0, 0, 0))];
    let old = anchor[0];
    terminal.resize_anchored(4, 2, &mut anchor);
    assert_eq!(anchor[0], old);
    assert_eq!(
        terminal
            .position_cell(anchor[0].position.unwrap())
            .unwrap()
            .ch,
        'o'
    );
    terminal.feed(b"\x1b[?1049h1111\r\n2222");
    let mut anchors = [
        ResizeAnchor::cell(terminal.view_position(0, 0, 0)),
        ResizeAnchor::cell(terminal.view_position(0, 1, 0)),
    ];
    terminal.resize_anchored(4, 1, &mut anchors);
    assert_eq!(anchors[0].position, None);
    assert_eq!(
        anchors[1].position,
        Some(TextPosition { line: 0, column: 0 })
    );
}

#[test]
fn reflow_alternate_grid_invalidates_clipped_wide_cells_and_dropped_rows() {
    let mut terminal = Terminal::new(5, 3, 16);
    terminal.feed("primary\x1b[?1049habc界\r\nlast".as_bytes());
    let mut anchors = [
        ResizeAnchor::cell(terminal.view_position(3, 0, 0)),
        ResizeAnchor::boundary(terminal.view_position(4, 1, 0)),
    ];
    terminal.resize_anchored(4, 1, &mut anchors);
    assert_eq!(anchors[0].position, None);
    assert_eq!(
        anchors[1].position,
        Some(TextPosition { line: 0, column: 4 })
    );
    complete_cells(&terminal);
    assert_eq!(retained_text(&terminal), "last");
    let mut clipped = Terminal::new(5, 2, 16);
    clipped.feed("\x1b[?1049habc界".as_bytes());
    let mut anchor = [ResizeAnchor::cell(clipped.view_position(3, 0, 0))];
    clipped.resize_anchored(4, 2, &mut anchor);
    assert_eq!(anchor[0].position, None);
    assert_eq!(clipped.cell(3, 0).ch, ' ');
    complete_cells(&clipped);
}
