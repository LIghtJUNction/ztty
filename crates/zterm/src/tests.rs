use super::*;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

fn line(terminal: &Terminal, row: usize) -> String {
    (0..terminal.cols())
        .map(|x| terminal.cell(x, row).ch)
        .collect()
}

#[test]
fn ascii_delayed_wrap_and_carriage_return() {
    let mut t = Terminal::new(4, 2, 0);
    t.feed(b"abcd");
    assert_eq!(t.cursor(), (3, 0));
    assert_eq!(line(&t, 0), "abcd");
    t.feed(b"\rX");
    assert_eq!(line(&t, 0), "Xbcd");
    t.feed(b"YZ12");
    assert_eq!(line(&t, 0), "XYZ1");
    assert_eq!(line(&t, 1), "2   ");
}

#[test]
fn utf8_and_sequences_are_independent_of_chunk_boundaries() {
    let bytes =
        "\x1b[38:2::3:7:11mA界e\u{301}\x1b[0m\r\nnext\x1b[?2004h\x1b]2;标题\x1b\\".as_bytes();
    let mut whole = Terminal::new(12, 3, 8);
    whole.feed(bytes);
    for split in 0..=bytes.len() {
        let mut split_terminal = Terminal::new(12, 3, 8);
        split_terminal.feed(&bytes[..split]);
        split_terminal.feed(&bytes[split..]);
        for y in 0..3 {
            for x in 0..12 {
                assert_eq!(
                    split_terminal.cell(x, y),
                    whole.cell(x, y),
                    "split={split}, x={x}, y={y}"
                );
            }
        }
        assert_eq!(split_terminal.cursor(), whole.cursor());
        assert_eq!(split_terminal.title(), whole.title());
        assert_eq!(split_terminal.modes(), whole.modes());
    }
}

#[test]
fn wide_glyphs_and_combining_marks_occupy_correct_cells() {
    let mut t = Terminal::new(8, 2, 0);
    t.feed("界e\u{301}".as_bytes());
    assert_eq!(t.cell(0, 0).width, 2);
    assert_eq!(t.cell(1, 0).width, 0);
    assert_eq!(t.cell(2, 0).combining[0], '\u{301}');
    assert_eq!(t.cursor(), (3, 0));
    t.feed(b"\x1b[1;2HX");
    assert_eq!(t.cell(0, 0).ch, ' ');
    assert_eq!(t.cell(1, 0).ch, 'X');
    assert_eq!(t.cell(1, 0).width, 1);
}

#[test]
fn ascii_batch_repairs_both_wide_boundaries_without_touching_other_glyphs() {
    let mut t = Terminal::new(12, 2, 0);
    t.feed("甲乙丙丁戊Z".as_bytes());
    // Begin in 甲's trailing cell and end in 丁's leading cell. The two
    // outside halves must be cleared; 戊 and Z must survive unchanged.
    t.feed(b"\x1b[48;2;1;2;3m\x1b[1;2Habcdef");
    assert_eq!(t.cell(0, 0).ch, ' ');
    for (x, ch) in "abcdef".chars().enumerate() {
        assert_eq!(t.cell(x + 1, 0).ch, ch);
        assert_eq!(t.cell(x + 1, 0).width, 1);
        assert_eq!(t.cell(x + 1, 0).combining, ['\0'; 2]);
    }
    assert_eq!(t.cell(7, 0).ch, ' ');
    assert_eq!(t.cell(0, 0).bg, 0x010203);
    assert_eq!(t.cell(7, 0).bg, 0x010203);
    assert_eq!(t.cell(0, 0).width, 1);
    assert_eq!(t.cell(7, 0).width, 1);
    assert_eq!(t.cell(8, 0).ch, '戊');
    assert_eq!(t.cell(8, 0).width, 2);
    assert_eq!(t.cell(9, 0).width, 0);
    assert_eq!(t.cell(10, 0).ch, 'Z');
    assert_eq!(t.cursor(), (7, 0));
}

#[test]
fn ascii_batch_overwrites_complete_wide_glyphs_and_handles_the_last_column() {
    let mut t = Terminal::new(10, 2, 0);
    t.feed("X甲乙丙Y".as_bytes());
    t.feed(b"\x1b[1;2Habcdef");
    assert_eq!(line(&t, 0), "XabcdefY  ");
    let mut edge = Terminal::new(4, 2, 0);
    edge.feed("甲乙".as_bytes());
    edge.feed(b"\x1b[1;2HxyzQ");
    assert_eq!(line(&edge, 0), " xyz");
    assert_eq!(line(&edge, 1), "Q   ");
}

#[test]
fn wide_character_wraps_before_last_column() {
    let mut t = Terminal::new(4, 2, 0);
    t.feed("abc界".as_bytes());
    assert_eq!(line(&t, 0), "abc ");
    assert_eq!(t.cell(0, 1).ch, '界');
    assert_eq!(t.cell(1, 1).width, 0);
}

#[test]
fn erase_half_of_wide_character_clears_both_halves() {
    let mut t = Terminal::new(5, 1, 0);
    t.feed("a界b".as_bytes());
    t.feed(b"\x1b[1;3H\x1b[X");
    assert_eq!(line(&t, 0), "a  b ");
    assert_eq!(t.cell(1, 0).width, 1);
    assert_eq!(t.cell(2, 0).width, 1);
}

#[test]
fn resize_preserves_cells_and_removes_split_wide_glyph() {
    let mut t = Terminal::new(5, 3, 4);
    t.feed("ab界\r\nline".as_bytes());
    t.resize(3, 2);
    assert_eq!(line(&t, 0), "ab ");
    assert_eq!(line(&t, 1), "lin");
    assert!(t.cursor().0 < 3);
    assert!(t.cursor().1 < 2);
    assert!(t.row_dirty(0));
    t.resize(0, 0);
    assert_eq!((t.cols(), t.rows()), (1, 1));
}

#[test]
fn history_is_bounded_and_view_orders_oldest_to_newest() {
    let mut t = Terminal::new(4, 2, 3);
    t.feed(b"1111\r\n2222\r\n3333\r\n4444\r\n5555\r\n6666");
    assert_eq!(t.history_len(), 3);
    assert_eq!(line(&t, 0), "5555");
    assert_eq!(line(&t, 1), "6666");
    assert_eq!(t.view_cell(0, 0, 3).ch, '2');
    assert_eq!(t.view_cell(0, 1, 3).ch, '3');
    assert_eq!(t.view_cell(0, 0, 1).ch, '4');
    assert_eq!(t.view_cell(0, 1, 1).ch, '5');
    assert_eq!(t.view_cell(100, 100, usize::MAX), &Cell::BLANK);
    t.feed(b"\x1b[3J");
    assert_eq!(t.history_len(), 0);
    assert_eq!(line(&t, 0), "5555");
}

#[test]
fn scroll_region_preserves_outside_rows_and_does_not_enter_history() {
    let mut t = Terminal::new(4, 4, 100);
    t.feed(b"1111\r\n2222\r\n3333\r\n4444");
    t.feed(b"\x1b[2;3r\x1b[3;1H\n");
    assert_eq!(line(&t, 0), "1111");
    assert_eq!(line(&t, 1), "3333");
    assert_eq!(line(&t, 2), "    ");
    assert_eq!(line(&t, 3), "4444");
    assert_eq!(t.history_len(), 0);
    t.feed(b"\x1b[2;1H\x1bM");
    assert_eq!(line(&t, 1), "    ");
    assert_eq!(line(&t, 2), "3333");
}

#[test]
fn origin_mode_coordinates_and_cursor_report_follow_margins() {
    let mut t = Terminal::new(10, 8, 0);
    t.feed(b"\x1b[3;6r\x1b[?6h\x1b[2;4H\x1b[6n");
    assert_eq!(t.cursor(), (3, 3));
    assert_eq!(t.responses(), b"\x1b[2;4R");
    t.feed(b"\x1b[999B");
    assert_eq!(t.cursor().1, 5);
    t.feed(b"\x1b[999A");
    assert_eq!(t.cursor().1, 2);
}

#[test]
fn alternate_screen_restores_primary_cursor_style_and_content() {
    let mut t = Terminal::new(8, 2, 4);
    t.feed(b"\x1b[31mmain\x1b[?1049hALT\x1b[32m\x1b[?1049l!");
    assert!(!t.modes().alternate_screen);
    assert_eq!(line(&t, 0), "main!   ");
    assert_eq!(t.cell(4, 0).fg, 0xcd0000);
    assert_eq!(t.cursor(), (5, 0));
}

#[test]
fn insert_delete_characters_and_lines_are_bounded() {
    let mut t = Terminal::new(8, 3, 0);
    t.feed(b"abcdef\r\x1b[3C\x1b[2@XY");
    assert_eq!(line(&t, 0), "abcXYdef");
    t.feed(b"\r\x1b[3C\x1b[2P");
    assert_eq!(line(&t, 0), "abcdef  ");
    t.feed(b"\r\n222\r\n333\x1b[2;1H\x1b[L");
    assert_eq!(line(&t, 1), "        ");
    assert_eq!(line(&t, 2), "222     ");
    t.feed(b"\x1b[M");
    assert_eq!(line(&t, 1), "222     ");
}

#[test]
fn sgr_supports_indexed_rgb_and_colon_forms() {
    let mut t = Terminal::new(12, 1, 0);
    t.feed(b"\x1b[1;3;4;9mA\x1b[38;5;196mB\x1b[48;2;1;2;3mC\x1b[38:2::10:20:30mD\x1b[0mE");
    assert_eq!(t.cell(0, 0).flags, BOLD | ITALIC | UNDERLINE | STRIKE);
    assert_eq!(t.cell(1, 0).fg, 0xff0000);
    assert_eq!(t.cell(2, 0).bg, 0x010203);
    assert_eq!(t.cell(3, 0).fg, 0x0a141e);
    assert_eq!(t.cell(4, 0).fg, DEFAULT_FG);
    assert_eq!(t.cell(4, 0).flags, 0);
    t.feed(b"\x1b[38;2;999;1;2mF");
    assert_eq!(t.cell(5, 0).fg, DEFAULT_FG);
}

#[test]
fn hyperlink_survives_closure_sgr_reset_and_scroll() {
    let mut t = Terminal::new(8, 2, 4);
    t.feed(b"\x1b]8;id=source;https://example.com/a;b\x1b\\link\x1b[0mX\x1b]8;;\x1b\\Y");
    let id = t.cell(0, 0).link_id;
    assert_ne!(id, 0);
    assert_eq!(t.hyperlink(id), Some("https://example.com/a;b"));
    assert_eq!(t.cell(4, 0).link_id, id);
    assert_eq!(t.cell(5, 0).link_id, 0);
    t.feed(b"\r\nnext\r\nend");
    assert_eq!(t.view_cell(0, 0, 1).link_id, id);
}

#[test]
fn overflowing_osc_is_rejected_as_a_whole() {
    let mut t = Terminal::new(8, 2, 0);
    t.feed(b"\x1b]2;trusted\x07");
    let mut oversized = Vec::from(&b"\x1b]2;"[..]);
    oversized.extend(core::iter::repeat_n(b'x', 5000));
    oversized.extend_from_slice(b"\x07OK");
    for chunk in oversized.chunks(13) {
        t.feed(chunk);
    }
    assert_eq!(t.title(), "trusted");
    assert_eq!(line(&t, 0), "OK      ");
    t.feed(b"\x1b]2;fresh\x07");
    assert_eq!(t.title(), "fresh");
}

#[test]
fn mouse_encoding_and_tracking_modes_are_independent() {
    let mut t = Terminal::new(8, 2, 0);
    t.feed(b"\x1b[?1006h");
    assert!(t.modes().sgr_mouse);
    assert_eq!(t.modes().mouse_tracking, 0);
    t.feed(b"\x1b[?1002h\x1b[?1000l");
    assert_eq!(t.modes().mouse_tracking, 1002);
    t.feed(b"\x1b[?1002l");
    assert_eq!(t.modes().mouse_tracking, 0);
}

#[test]
fn sync_output_and_mode_queries_report_actual_state() {
    let mut t = Terminal::new(10, 2, 0);
    t.feed(b"\x1b[?2026$p\x1b[?2026hX\x1b[?2026$p\x1b[?9999$p");
    assert!(t.modes().synchronized_output);
    assert_eq!(t.cell(0, 0).ch, 'X');
    assert_eq!(t.responses(), b"\x1b[?2026;2$y\x1b[?2026;1$y\x1b[?9999;0$y");
    t.force_end_sync();
    assert!(!t.modes().synchronized_output);
    t.feed(b"\x1b[?2004h\x1b[?1004h");
    assert!(t.modes().bracketed_paste && t.modes().focus_events);
}

#[test]
fn kitty_keyboard_flags_and_stacks_are_screen_local() {
    let mut t = Terminal::new(10, 2, 0);
    t.feed(b"\x1b[>1u\x1b[=2;2u\x1b[?u");
    assert_eq!(t.modes().kitty_keyboard, 3);
    assert_eq!(t.responses(), b"\x1b[?3u");
    t.feed(b"\x1b[?1049h");
    assert_eq!(t.modes().kitty_keyboard, 0);
    t.feed(b"\x1b[>8u\x1b[?1049l");
    assert_eq!(t.modes().kitty_keyboard, 3);
    t.feed(b"\x1b[<u");
    assert_eq!(t.modes().kitty_keyboard, 0);
    t.feed(b"\x1b[>31u\x1b[=2;3u");
    assert_eq!(t.modes().kitty_keyboard, 29);
    t.feed(b"\x1b[<999u");
    assert_eq!(t.modes().kitty_keyboard, 0);
}

#[test]
fn terminal_capability_queries_reject_unknown_names() {
    let mut t = Terminal::new(10, 2, 0);
    t.feed(b"\x1bP+q544e;436f;7878\x1b\\");
    assert_eq!(t.responses(), b"\x1bP1+r544e=787465726d2d323536636f6c6f72\x1b\\\x1bP1+r436f=323536\x1b\\\x1bP0+r7878\x1b\\");
}

#[test]
fn repeated_alternate_screen_enable_keeps_primary_saved_state() {
    let mut t = Terminal::new(12, 3, 4);
    t.feed(b"\x1b[31mprimary\x1b[?1049hALT\x1b[?1049h!");
    assert_eq!(line(&t, 0), "ALT!        ");
    t.feed(b"\x1b[?1049l?");
    assert_eq!(line(&t, 0), "primary?    ");
    assert_eq!(t.cell(7, 0).fg, 0xcd0000);
}

#[test]
fn resize_keeps_the_cursor_row_and_moves_discarded_rows_into_history() {
    let mut t = Terminal::new(4, 4, 8);
    t.feed(b"1111\r\n2222\r\n3333\r\n4444");
    t.resize(4, 2);
    assert_eq!(line(&t, 0), "3333");
    assert_eq!(line(&t, 1), "4444");
    assert_eq!(t.cursor(), (3, 1));
    assert_eq!(t.history_len(), 2);
    assert_eq!(t.view_cell(0, 0, 2).ch, '1');
    assert_eq!(t.view_cell(0, 1, 2).ch, '2');
}

#[test]
fn resize_in_alternate_screen_preserves_primary_cursor_and_history() {
    let mut t = Terminal::new(4, 4, 8);
    t.feed(b"1111\r\n2222\r\n3333\r\n4444\x1b[?1049hALT");
    t.resize(4, 2);
    assert_eq!(t.history_len(), 0);
    assert_eq!(line(&t, 0), "ALT ");
    t.feed(b"\x1b[?1049l");
    assert_eq!(line(&t, 0), "3333");
    assert_eq!(line(&t, 1), "4444");
    assert_eq!(t.cursor(), (3, 1));
    assert_eq!(t.history_len(), 2);
}

#[test]
fn osc_color_queries_reply_with_resolved_rgb_and_valid_indices() {
    let mut t = Terminal::new(8, 2, 0);
    t.feed(b"\x1b]10;?;?\x1b\\\x1b]4;196;?;256;?;bad;?\x07");
    assert_eq!(t.responses(), b"\x1b]10;rgb:d9d9/e2e2/efef\x1b\\\x1b]11;rgb:0b0b/0f0f/1414\x1b\\\x1b]4;196;rgb:ffff/0000/0000\x07");
}

#[test]
fn semicolons_in_osc_metadata_are_preserved() {
    let mut t = Terminal::new(8, 2, 0);
    t.feed(b"\x1b]2;one;two\x07\x1b]7;file:///tmp/a;b\x1b\\");
    assert_eq!(t.title(), "one;two");
    assert_eq!(t.working_directory(), "file:///tmp/a;b");
}

#[test]
fn tabs_and_dec_line_drawing_work_without_allocated_glyphs() {
    let mut t = Terminal::new(20, 2, 0);
    t.feed(b"x\ty\x1b(0lqk\x1b(Bz");
    assert_eq!(t.cell(8, 0).ch, 'y');
    assert_eq!(t.cell(9, 0).ch, '┌');
    assert_eq!(t.cell(10, 0).ch, '─');
    assert_eq!(t.cell(11, 0).ch, '┐');
    assert_eq!(t.cell(12, 0).ch, 'z');
}

#[test]
fn malformed_input_cannot_escape_grid_bounds() {
    let mut t = Terminal::new(12, 4, 7);
    let mut seed = 0x9e37_79b9u32;
    let mut data = [0; 257];
    for _ in 0..256 {
        for value in &mut data {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *value = seed as u8;
        }
        t.feed(&data);
        let (x, y) = t.cursor();
        assert!(x < t.cols() && y < t.rows());
        assert!(t.history_len() <= 7);
    }
    t.feed(b"\x18\x1bcOK");
    assert_eq!(t.cell(0, 0).ch, 'O');
    assert_eq!(t.cell(1, 0).ch, 'K');
}

#[test]
fn query_reply_storage_is_bounded() {
    let mut t = Terminal::new(8, 2, 0);
    for _ in 0..25000 {
        t.feed(b"\x1b[6n");
    }
    assert!(t.responses().len() <= 65536);
    assert!(t.responses().ends_with(b"\x1b[1;1R"));
    let full = t.responses().len();
    t.feed(b"\x1b[?2026$p\x1b[?u");
    assert_eq!(
        t.responses().len(),
        full,
        "an overflowing response must be dropped whole"
    );
    t.clear_responses();
    t.feed(b"\x1b[5n");
    assert_eq!(t.responses(), b"\x1b[0n");
}

#[test]
fn damage_tracking_marks_changed_rows_and_cursor_rows() {
    let mut t = Terminal::new(12, 4, 0);
    t.clear_dirty();
    assert!(!t.dirty());
    t.feed(b"hello");
    assert!(t.row_dirty(0));
    assert!(!t.row_dirty(1));
    t.clear_dirty();
    t.feed(b"\x1b[3;1H");
    assert!(t.row_dirty(0));
    assert!(t.row_dirty(2));
    assert!(!t.row_dirty(1));
}

#[test]
fn cell_storage_has_no_heap_pointer_and_is_compact() {
    assert!(core::mem::size_of::<Cell>() <= 32);
    let mut t = Terminal::new(80, 24, 0);
    for index in 0..100 {
        t.feed(format!("line {index}\r\n").as_bytes());
    }
    assert_eq!(t.history_len(), 0);
}

#[test]
fn kitty_graphics_framing_is_independent_of_every_pty_chunk_boundary() {
    let bytes = b"before\x1b_Ga=T,i=12,p=4,f=24,s=1,v=1,C=1;/wAA\x1b\\after\x1b[6n";
    for split in 0..=bytes.len() {
        let mut t = Terminal::new(20, 4, 8);
        t.set_cell_size(10, 20);
        t.feed(&bytes[..split]);
        t.feed(&bytes[split..]);
        assert_eq!(line(&t, 0), "beforeafter         ");
        assert_eq!(
            t.graphics().image(12).unwrap().pixels.as_slice(),
            [255, 0, 0, 255]
        );
        let draw = t.graphics_draws(0).next().unwrap();
        assert_eq!((draw.dest.x, draw.dest.y, draw.placement_id), (60, 0, 4));
        assert_eq!(t.responses(), b"\x1b_Gi=12,p=4;OK\x1b\\\x1b[1;12R");
    }
}

#[test]
fn graphics_query_precedes_device_attributes_and_reports_actual_cell_size() {
    let mut t = Terminal::new(20, 4, 8);
    t.set_cell_size(9, 18);
    t.feed(b"\x1b_Ga=q,i=42,f=24,s=1,v=1;AAAA\x1b\\\x1b[c\x1b[14t\x1b[16t\x1b[18t");
    assert!(t.responses().starts_with(b"\x1b_Gi=42;OK\x1b\\\x1b[?"));
    assert!(
        t.responses()
            .ends_with(b"\x1b[4;72;180t\x1b[6;18;9t\x1b[8;4;20t")
    );
    assert_eq!(t.graphics().image_count(), 0);
    t.feed(b"\x1bc\x1b[16t");
    assert_eq!(t.responses(), b"\x1b[6;18;9t");
}

#[test]
fn icat_probe_finishes_on_da1_and_advertises_only_sixel() {
    let probe = b"\x1b_Ga=q,f=24,s=1,v=1,S=3,i=1;MTIz\x1b\\\
                  \x1b_Ga=q,t=t,f=24,s=1,v=1,i=2;L3RtcC9pY2F0\x1b\\\
                  \x1b_Ga=q,t=s,f=24,s=1,v=1,i=3;aWNhdA==\x1b\\\x1b[c";
    for chunk_size in [1, 2, 7, probe.len()] {
        let mut t = Terminal::new(20, 4, 8);
        for chunk in probe.chunks(chunk_size) {
            t.feed(chunk);
        }
        assert_eq!(
            t.responses(),
            b"\x1b_Gi=1;OK\x1b\\\
              \x1b_Gi=2;ENOTSUP:only direct transmission is supported\x1b\\\
              \x1b_Gi=3;ENOTSUP:only direct transmission is supported\x1b\\\x1b[?62;4c"
        );
        let start = t
            .responses()
            .iter()
            .rposition(|byte| *byte == 0x1b)
            .unwrap();
        let da1 = &t.responses()[start + 2..];
        // Existing icat recognizes this final reply only when len > 3.
        assert!(da1.len() > 3 && da1[0] == b'?' && da1[da1.len() - 1] == b'c');
        let capabilities: Vec<u16> = core::str::from_utf8(&da1[1..da1.len() - 1])
            .unwrap()
            .split(';')
            .map(|number| number.parse().unwrap())
            .collect();
        assert_eq!(capabilities, [62, 4]);
        assert_eq!(t.graphics().image_count(), 0);
        assert_eq!(t.cursor(), (0, 0));
    }
}

#[test]
fn incomplete_graphics_transfers_do_not_consume_following_text() {
    let mut t = Terminal::new(20, 4, 8);
    t.feed(b"\x1b_Ga=T,i=1,s=2,v=1,m=1,C=1;/wAA\x1b\\hello");
    assert_eq!(&line(&t, 0)[..5], "hello");
    assert_eq!(t.graphics().pending_bytes(), 3);
    assert_eq!(t.graphics().image_count(), 0);
    t.feed(b"\x1b_Gm=0;/wD/AIA=\x1b\\!");
    assert_eq!(&line(&t, 0)[..6], "hello!");
    assert_eq!(t.graphics_draws(0).next().unwrap().dest.x, 40);
    assert_eq!(t.graphics().pending_bytes(), 0);
}

#[test]
fn missing_st_unknown_apc_and_oversize_apc_recover_on_cancellation() {
    for ending in [b"\x18OK".as_slice(), b"\x1aOK", b"\x1b[0mOK"] {
        let mut t = Terminal::new(20, 4, 8);
        t.feed(b"\x1b_Ga=T,i=1,s=1,v=1;AAAA");
        t.feed(ending);
        assert_eq!(&line(&t, 0)[..2], "OK");
        assert_eq!(t.graphics().image_count(), 0);
    }
    let mut t = Terminal::new(20, 4, 8);
    t.feed(b"\x1b_unknown;ignored\x1b\\OK");
    assert_eq!(&line(&t, 0)[..2], "OK");
    t.feed(b"\r\x1b_Ga=T,i=1,s=1,v=1;");
    for _ in 0..20 {
        t.feed(&[b'A'; 1024]);
    }
    t.feed(b"\x1b\\good\x1b[6n");
    assert_eq!(&line(&t, 0)[..4], "good");
    assert_eq!(
        (t.graphics().image_count(), t.graphics().pending_bytes()),
        (0, 0)
    );
    assert!(t.responses().ends_with(b"\x1b[1;5R"));
}

#[test]
fn graphics_dispatch_cancels_previous_osc_and_dcs_without_text_leakage() {
    let mut t = Terminal::new(20, 4, 8);
    t.feed(b"\x1b]2;title\x1b_Ga=q,i=1,f=24,s=1,v=1;AAAA\x1b\\");
    assert_eq!(t.title(), "title");
    assert_eq!(line(&t, 0), "                    ");
    assert_eq!(t.responses(), b"\x1b_Gi=1;OK\x1b\\");
    t.clear_responses();
    t.feed(b"\x1bP+q544e\x1b_Ga=q,i=2,f=24,s=1,v=1;AAAA\x1b\\OK");
    assert_eq!(&line(&t, 0)[..2], "OK");
    assert!(t.responses().ends_with(b"\x1b_Gi=2;OK\x1b\\"));
}

#[test]
fn graphics_follow_scroll_resize_alternate_screen_and_deletion() {
    let mut t = Terminal::new(20, 4, 8);
    t.set_cell_size(8, 16);
    t.feed(b"\x1b[2;1H\x1b_Ga=T,i=1,s=1,v=1,r=2,C=1;AQIDBA==\x1b\\");
    assert_eq!(t.graphics_draws(0).next().unwrap().dest.y, 16);
    t.feed(b"\x1b[4;1H\n");
    assert_eq!(t.graphics_draws(0).next().unwrap().dest.y, 0);
    t.resize(20, 2);
    assert!(t.graphics_draws(0).next().is_none());
    assert_eq!(t.graphics_draws(2).next().unwrap().dest.y, 0);
    t.feed(b"\x1b[?1049h");
    assert_eq!(t.graphics().image_count(), 0);
    t.feed(b"\x1b_Ga=T,i=1,s=1,v=1,C=1;BQYHCA==\x1b\\");
    assert_eq!(
        t.graphics().image(1).unwrap().pixels.as_slice(),
        [5, 6, 7, 8]
    );
    t.feed(b"\x1b[?1049l");
    assert_eq!(
        t.graphics().image(1).unwrap().pixels.as_slice(),
        [1, 2, 3, 4]
    );
    t.clear_dirty();
    t.feed(b"\x1b_Ga=d,d=I,i=1\x1b\\");
    assert_eq!(t.graphics().image_count(), 0);
    assert!(t.dirty());
    t.feed(b"\x1b[?1049h");
    assert_eq!(t.graphics().image_count(), 0);
}

#[test]
fn graphics_cursor_policy_and_reset_generation_are_stable() {
    let mut t = Terminal::new(20, 4, 8);
    t.set_cell_size(8, 16);
    t.feed(b"\x1b_Ga=T,i=1,s=1,v=1,c=2,r=2;AQIDBA==\x1b\\");
    assert_eq!(t.cursor(), (2, 2));
    let before = t.graphics().image(1).unwrap().generation;
    t.feed(b"\x1bc\x1b_Ga=T,i=1,s=1,v=1,C=1;AQIDBA==\x1b\\");
    assert_eq!(t.cursor(), (0, 0));
    assert!(t.graphics().image(1).unwrap().generation > before);
    t.feed(b"\x1b[2J");
    assert_eq!(t.graphics().placement_count(), 0);
}

#[test]
fn large_inline_apc_from_an_existing_client_does_not_require_m_chunks_or_padding() {
    // A raw RGB body over 51 KiB, with a final unpadded RGBA tail for a second
    // command. The compressed, large single-APC case is tested by zgraphics.
    let mut bytes = b"\x1b_Ga=T,q=2,f=24,C=1,s=128,v=128;".to_vec();
    bytes.extend_from_slice(&b"/wAA".repeat(128 * 128));
    bytes.extend_from_slice(b"\x1b\\\x1b[2;1H\x1b_Ga=T,i=9,C=1,s=1,v=1;AQIDBA\x1b\\OK");
    for chunk in [1, 17, 4096, 65536] {
        let mut t = Terminal::new(80, 24, 8);
        for part in bytes.chunks(chunk) {
            t.feed(part);
        }
        assert_eq!(t.graphics().image_count(), 2);
        assert_eq!(
            t.graphics_draws(0).next().unwrap().image.pixels.len(),
            128 * 128 * 4
        );
        assert_eq!(
            t.graphics().image(9).unwrap().pixels.as_slice(),
            [1, 2, 3, 4]
        );
        assert_eq!(&line(&t, 1)[..2], "OK");
    }
}
