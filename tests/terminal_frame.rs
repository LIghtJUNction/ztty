extern crate alloc;

use zterm::{BLINK, BOLD, DIM, HIDDEN, INVERSE, ITALIC, RAPID_BLINK, STRIKE, Terminal, UNDERLINE};
use ztty::{
    json::{self, Value},
    terminal_frame::{self, Renderer},
};

const STYLE: u16 =
    BOLD | DIM | ITALIC | UNDERLINE | BLINK | RAPID_BLINK | INVERSE | HIDDEN | STRIKE;

fn assert_cells(source: &Terminal, mirror: &Terminal) {
    assert_eq!(
        (source.cols(), source.rows()),
        (mirror.cols(), mirror.rows())
    );
    for y in 0..source.rows() {
        for x in 0..source.cols() {
            let a = source.cell(x, y);
            let b = mirror.cell(x, y);
            assert_eq!(
                source.cell_chars(a).collect::<String>(),
                mirror.cell_chars(b).collect::<String>(),
                "cell {x},{y}"
            );
            assert_eq!(
                (a.width, a.fg, a.bg, a.flags & STYLE),
                (b.width, b.fg, b.bg, b.flags & STYLE),
                "style {x},{y}"
            );
            assert_eq!(
                source.cell_hyperlink(a),
                mirror.cell_hyperlink(b),
                "link {x},{y}"
            );
        }
    }
    assert_eq!(source.cursor(), mirror.cursor());
    assert_eq!(
        source.modes().alternate_screen,
        mirror.modes().alternate_screen
    );
}

fn feed_frame(
    renderer: &mut Renderer,
    source: &Terminal,
    mirror: &mut Terminal,
    history: Option<u64>,
) -> Vec<u8> {
    let frame = terminal_frame::capture(source, history).unwrap();
    assert!(json::stringify(&frame).len() <= terminal_frame::MAX_FRAME_BYTES);
    let wire = renderer.render(&frame).unwrap();
    mirror.feed(&wire);
    assert_cells(source, mirror);
    wire
}

#[test]
fn reconnect_restores_styled_unicode_cells_cursor_and_title() {
    let mut source = Terminal::new(40, 8, 100);
    source.feed("plain\r\n\x1b[1;3;4;38;2;17;34;51;48;2;68;85;102m彩色 é 👩‍💻\x1b[0m\r\n\x1b[2;5;7;9m dim blink inverse strike\x1b[0m\x1b]2;remote build\x1b\\\x1b[2;7H".as_bytes());
    let mut mirror = Terminal::new(40, 8, 100);
    feed_frame(&mut Renderer::new(), &source, &mut mirror, None);
    assert_eq!(mirror.title(), "remote build");
}

#[test]
fn hyperlink_regions_are_preserved_and_closed_between_rows() {
    let mut source = Terminal::new(20, 4, 10);
    source.feed(b"\x1b]8;;https://example.org/path?q=1\x1b\\link\x1b]8;;\x1b\\ plain\r\nnext");
    let mut mirror = Terminal::new(20, 4, 10);
    feed_frame(&mut Renderer::new(), &source, &mut mirror, None);
    assert!(mirror.cell_hyperlink(mirror.cell(0, 1)).is_none());
}

#[test]
fn a_single_changed_row_uses_less_than_a_quarter_of_the_full_snapshot() {
    let mut source = Terminal::new(100, 32, 0);
    for y in 0..32 {
        source.feed(format!("\x1b[{};1H{}", y + 1, "x".repeat(100)).as_bytes());
    }
    let mut mirror = Terminal::new(100, 32, 0);
    let mut renderer = Renderer::new();
    let full = feed_frame(&mut renderer, &source, &mut mirror, None);
    source.feed(b"\x1b[7;3Hchanged");
    let delta = feed_frame(&mut renderer, &source, &mut mirror, Some(0));
    assert!(
        delta.len() * 4 < full.len(),
        "full={} delta={}",
        full.len(),
        delta.len()
    );
    let idle = feed_frame(&mut renderer, &source, &mut mirror, Some(0));
    assert!(idle.is_empty());
}

#[test]
fn blanking_cells_and_colored_backgrounds_do_not_leave_old_text() {
    let mut source = Terminal::new(20, 5, 0);
    source.feed(b"\x1b[41mabcdefghijklmnopqrst\x1b[0m\x1b[2;1Hsecond");
    let mut renderer = Renderer::new();
    let mut mirror = Terminal::new(20, 5, 0);
    feed_frame(&mut renderer, &source, &mut mirror, None);
    source.feed(b"\x1b[1;3H\x1b[44m\x1b[K\x1b[0m\x1b[2;1H\x1b[2K");
    feed_frame(&mut renderer, &source, &mut mirror, Some(0));
}

#[test]
fn fullscreen_application_and_primary_screen_round_trip() {
    let mut source = Terminal::new(30, 6, 50);
    source.feed(b"shell prompt\r\nwork");
    let mut renderer = Renderer::new();
    let mut mirror = Terminal::new(30, 6, 50);
    feed_frame(&mut renderer, &source, &mut mirror, None);
    source.feed(b"\x1b[?1049h\x1b[2J\x1b[3;4H\x1b[35mfull screen\x1b[0m\x1b[?25l");
    feed_frame(&mut renderer, &source, &mut mirror, Some(0));
    assert!(!mirror.modes().cursor_visible);
    source.feed(b"\x1b[?1049l\x1b[?25h\r\nfinished");
    feed_frame(&mut renderer, &source, &mut mirror, None);
    assert!(mirror.modes().cursor_visible);
}

#[test]
fn reconnect_during_alternate_screen_then_return_restores_primary_history() {
    let mut source = Terminal::new(24, 4, 100);
    for index in 0..12 {
        source.feed(format!("row {index}\r\n").as_bytes());
    }
    source.feed(b"\x1b[?1049h\x1b[2J\x1b[1;1Heditor");
    let mut renderer = Renderer::new();
    let mut mirror = Terminal::new(24, 4, 100);
    feed_frame(&mut renderer, &source, &mut mirror, None);
    source.feed(b"\x1b[?1049l");
    feed_frame(&mut renderer, &source, &mut mirror, None);
    assert_eq!(source.history_len(), mirror.history_len());
}

#[test]
fn input_modes_are_forwarded_without_device_query_replies() {
    let mut source = Terminal::new(40, 6, 0);
    source.feed(
        b"\x1b[?1h\x1b=\x1b[?2004h\x1b[?1004h\x1b[?1002h\x1b[?1006h\x1b[=31u\x1b[6 q\x1b[6n\x1b[c",
    );
    assert!(!source.responses().is_empty());
    let mut mirror = Terminal::new(40, 6, 0);
    let mut renderer = Renderer::new();
    feed_frame(&mut renderer, &source, &mut mirror, None);
    assert_eq!(source.modes().input(), mirror.modes().input());
    assert_eq!(source.modes().cursor_shape, mirror.modes().cursor_shape);
    assert_eq!(source.modes().cursor_blink, mirror.modes().cursor_blink);
    assert!(
        mirror.responses().is_empty(),
        "the transport must not duplicate device replies"
    );
    source.feed(b"\x1b[?1l\x1b>\x1b[?2004l\x1b[?1004l\x1b[?1002l\x1b[?1006l\x1b[=0u");
    feed_frame(&mut renderer, &source, &mut mirror, Some(0));
    assert_eq!(source.modes().input(), mirror.modes().input());
}

#[test]
fn reconnect_and_incremental_scroll_restore_the_same_retained_rows() {
    let mut source = Terminal::new(30, 4, 100);
    for index in 0..10 {
        source.feed(format!("line {index}\r\n").as_bytes());
    }
    let mut renderer = Renderer::new();
    let mut mirror = Terminal::new(30, 4, 100);
    feed_frame(&mut renderer, &source, &mut mirror, None);
    assert_eq!(source.history_len(), mirror.history_len());
    let previous = source.history_position();
    for index in 10..13 {
        source.feed(format!("line {index}\r\n").as_bytes());
    }
    feed_frame(&mut renderer, &source, &mut mirror, Some(previous));
    assert_eq!(source.history_len(), mirror.history_len());
    for offset in 1..=source.history_len() {
        for x in 0..source.cols() {
            let a = source.view_cell(x, 0, offset);
            let b = mirror.view_cell(x, 0, offset);
            assert_eq!(
                source.cell_chars(a).collect::<String>(),
                mirror.cell_chars(b).collect::<String>(),
                "history {offset} col {x}"
            );
        }
    }
}

#[test]
fn resize_reconstructs_cells_and_history_after_reflow() {
    let mut source = Terminal::new(30, 6, 100);
    source.feed(b"012345678901234567890123456789abcdefghijklmnop\r\nsecond\r\nthird\r\nfourth\r\nfifth\r\nsixth");
    let mut mirror = Terminal::new(30, 6, 100);
    let mut renderer = Renderer::new();
    feed_frame(&mut renderer, &source, &mut mirror, None);
    source.resize(18, 4);
    mirror.resize(18, 4);
    feed_frame(&mut renderer, &source, &mut mirror, None);
    assert_eq!(source.history_len(), mirror.history_len());
}

#[test]
fn large_history_is_clipped_while_the_current_screen_remains_complete() {
    let mut source = Terminal::new(80, 8, 4000);
    for index in 0..2300 {
        source.feed(format!("{index}: {}\r\n", "x".repeat(70)).as_bytes());
    }
    let frame = terminal_frame::capture(&source, None).unwrap();
    assert_eq!(
        frame.get("history_truncated").and_then(Value::as_bool),
        Some(true)
    );
    assert!(
        frame.get("history").unwrap().as_array().unwrap().len() <= terminal_frame::MAX_HISTORY_ROWS
    );
    assert!(json::stringify(&frame).len() <= terminal_frame::MAX_FRAME_BYTES);
    let mut mirror = Terminal::new(80, 8, 4000);
    mirror.feed(&Renderer::new().render(&frame).unwrap());
    assert_cells(&source, &mirror);
}

#[test]
fn malformed_frame_is_rejected_before_changing_the_renderer() {
    let mut source = Terminal::new(20, 4, 0);
    source.feed(b"stable");
    let frame = terminal_frame::capture(&source, None).unwrap();
    let mut renderer = Renderer::new();
    renderer.render(&frame).unwrap();
    let mut invalid = frame.clone();
    invalid.set("cursor_x", 20usize.into());
    assert!(renderer.render(&invalid).is_err());
    assert!(renderer.render(&frame).unwrap().is_empty());
    invalid = frame.clone();
    invalid.set("title", "bad\x1b[2J".into());
    assert!(renderer.render(&invalid).is_err());
    assert!(renderer.render(&frame).unwrap().is_empty());
}

#[test]
fn attachment_resets_local_margins_charset_and_insert_state() {
    let mut source = Terminal::new(20, 5, 0);
    source.feed(b"hello\r\nsecond line");
    let mut mirror = Terminal::new(20, 5, 0);
    mirror.feed(b"\x1b[2;4r\x1b[?6h\x1b[4h\x1b(0qqqq");
    feed_frame(&mut Renderer::new(), &source, &mut mirror, None);
}

fn row_text(terminal: &Terminal, row: usize, offset: usize) -> String {
    (0..terminal.cols())
        .filter_map(|x| {
            let cell = terminal.view_cell(x, row, offset);
            (cell.width != 0).then(|| terminal.cell_chars(cell).collect::<String>())
        })
        .collect()
}

#[test]
fn smaller_observer_clips_without_resizing_the_writer_and_clamps_cursor() {
    let mut source = Terminal::new(16, 6, 20);
    source
        .feed("\x1b[1;1H1234567界tail\x1b[2;1Hsecond-line\x1b[3;1Hthird-line\x1b[6;16H".as_bytes());
    let before_revision = source.text_revision();
    let frame = terminal_frame::capture_view(&source, None, 8, 3).unwrap();
    let mut mirror = Terminal::new(8, 3, 20);
    mirror.feed(&Renderer::new().render(&frame).unwrap());
    assert_eq!(row_text(&mirror, 0, 0), "1234567 ");
    assert_eq!(row_text(&mirror, 1, 0), "second-l");
    assert_eq!(row_text(&mirror, 2, 0), "third-li");
    assert_eq!(mirror.cursor(), (7, 2));
    assert_eq!(
        (source.cols(), source.rows(), source.cursor()),
        (16, 6, (15, 5))
    );
    assert_eq!(source.text_revision(), before_revision);
    assert_eq!(source.cell(7, 0).ch, '界');
}

#[test]
fn larger_observer_pads_columns_and_rows_without_changing_writer_state() {
    let mut source = Terminal::new(8, 3, 20);
    source.feed(b"\x1b[31mhello\x1b[0m\r\nworld");
    let before_revision = source.text_revision();
    let frame = terminal_frame::capture_view(&source, None, 14, 6).unwrap();
    let mut mirror = Terminal::new(14, 6, 20);
    mirror.feed(&Renderer::new().render(&frame).unwrap());
    assert_eq!(row_text(&mirror, 0, 0), "hello         ");
    assert_eq!(row_text(&mirror, 1, 0), "world         ");
    for y in 2..6 {
        assert_eq!(row_text(&mirror, y, 0), "              ");
    }
    assert_eq!(mirror.cell(0, 0).fg, source.cell(0, 0).fg);
    assert_eq!(mirror.cursor(), source.cursor());
    assert_eq!(
        (source.cols(), source.rows(), source.text_revision()),
        (8, 3, before_revision)
    );
}

#[test]
fn observer_history_is_cropped_and_delta_updates_stay_within_its_size() {
    let mut source = Terminal::new(12, 4, 30);
    for index in 0..10 {
        source.feed(format!("{index:02}-abcdefgh\r\n").as_bytes());
    }
    let mut renderer = Renderer::new();
    let mut mirror = Terminal::new(6, 2, 30);
    let frame = terminal_frame::capture_view(&source, None, 6, 2).unwrap();
    mirror.feed(&renderer.render(&frame).unwrap());
    assert_eq!(mirror.history_len(), source.history_len());
    for offset in 1..=source.history_len() {
        assert_eq!(
            row_text(&mirror, 0, offset),
            row_text(&source, 0, offset)
                .chars()
                .take(6)
                .collect::<String>()
        );
    }
    let previous = source.history_position();
    source.feed(b"10-abcdefgh\r\n");
    let frame = terminal_frame::capture_view(&source, Some(previous), 6, 2).unwrap();
    mirror.feed(&renderer.render(&frame).unwrap());
    for y in 0..2 {
        assert_eq!(
            row_text(&mirror, y, 0),
            row_text(&source, y, 0).chars().take(6).collect::<String>()
        );
    }
    assert_eq!((source.cols(), source.rows()), (12, 4));
    assert_eq!(mirror.history_len(), source.history_len());
}

#[test]
fn observer_rejects_out_of_bounds_view_sizes() {
    let source = Terminal::new(20, 4, 0);
    for (cols, rows) in [(1, 4), (20, 1), (513, 4), (20, 257)] {
        assert!(terminal_frame::capture_view(&source, None, cols, rows).is_err());
    }
    assert_eq!((source.cols(), source.rows()), (20, 4));
}

#[test]
fn oversized_colorful_view_returns_a_bounded_error() {
    let mut source = Terminal::new(512, 256, 0);
    for y in 0..256 {
        source.feed(format!("\x1b[{};1H", y + 1).as_bytes());
        for x in 0..512 {
            source.feed(format!("\x1b[38;2;{};{};1mx", x % 256, y % 256).as_bytes());
        }
    }
    assert!(
        terminal_frame::capture(&source, None)
            .unwrap_err()
            .contains("bounded size")
    );
}
