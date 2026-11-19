use super::*;
use alloc::format;

fn changed_theme() -> Theme {
    Theme {
        foreground: 0x123456,
        background: 0x654321,
        palette: core::array::from_fn(|index| 0x112200 + index as u32),
    }
}

#[test]
fn provenance_and_complete_cluster_id_fit_compact_copy_storage() {
    assert_eq!(core::mem::size_of::<Cell>(), 32);
    // Cursor styles retain raw Kitty color IDs; per-cell storage stays 32 B.
    assert_eq!(core::mem::size_of::<crate::screen::Style>(), 24);
}

#[test]
fn defaults_change_but_identical_explicit_rgb_does_not() {
    let old = changed_theme();
    let new = Theme::default();
    let mut t = Terminal::with_theme(8, 2, 0, old);
    // The truecolor cell deliberately equals BOTH old defaults.
    t.feed(b"D\x1b[38;2;18;52;86;48;2;101;67;33mT\x1b[39;49mR");
    let explicit = *t.cell(1, 0);
    let cursor = t.cursor();
    t.clear_dirty();
    t.set_theme(new);
    assert_eq!(t.theme(), new);
    assert_eq!(t.cursor(), cursor);
    assert_eq!(*t.cell(1, 0), explicit);
    assert_eq!(
        explicit.flags & (EXPLICIT_FG | EXPLICIT_BG),
        EXPLICIT_FG | EXPLICIT_BG
    );
    for x in [0, 2, 3, 7] {
        assert_eq!(t.cell(x, 0).fg, new.foreground);
        assert_eq!(t.cell(x, 0).bg, new.background);
    }
    assert!(t.row_dirty(0) && t.row_dirty(1));
    t.feed(b"N");
    assert_eq!(t.cell(3, 0).fg, new.foreground);
    assert_eq!(t.cell(3, 0).bg, new.background);
}

#[test]
fn every_ansi_index_follows_the_palette_for_all_sgr_encodings() {
    let new = changed_theme();
    for encoding in 0..3 {
        let mut t = Terminal::new(32, 2, 0);
        for index in 0..16 {
            let other = 15 - index;
            let sequence = match encoding {
                0 => {
                    let fg = if index < 8 {
                        30 + index
                    } else {
                        90 + index - 8
                    };
                    let bg = if other < 8 {
                        40 + other
                    } else {
                        100 + other - 8
                    };
                    format!("\x1b[{fg};{bg}mX")
                }
                1 => format!("\x1b[38;5;{index};48;5;{other}mX"),
                _ => format!("\x1b[38:5:{index};48:5:{other}mX"),
            };
            t.feed(sequence.as_bytes());
        }
        t.set_theme(new);
        for index in 0..16 {
            let cell = t.cell(index, 0);
            assert_eq!(
                cell.fg, new.palette[index],
                "encoding={encoding}, index={index}"
            );
            assert_eq!(cell.bg, new.palette[15 - index]);
            assert_eq!(
                cell.palette_indices,
                index as u8 | ((15 - index) as u8) << 4
            );
            assert_eq!(
                cell.flags & (PALETTE_FG | PALETTE_BG),
                PALETTE_FG | PALETTE_BG
            );
        }
        // The active style must also resolve through the updated palette.
        t.feed(b"Y");
        assert_eq!(t.cell(16, 0).fg, new.palette[15]);
        assert_eq!(t.cell(16, 0).bg, new.palette[0]);
    }
}

#[test]
fn truecolor_and_extended_indices_never_become_palette_references() {
    let old = Theme {
        foreground: 0xcd0000,
        background: 0x0000ee,
        ..Theme::default()
    };
    let mut t = Terminal::with_theme(16, 2, 0, old);
    t.feed(b"\x1b[31;44mP\x1b[38:2::205:0:0;48:2::0:0:238mT");
    t.feed(b"\x1b[38;5;196;48;5;21mE\x1b[38:5:16;48:5:255mF");
    let fixed = [*t.cell(1, 0), *t.cell(2, 0), *t.cell(3, 0)];
    t.set_theme(changed_theme());
    assert_eq!(t.cell(0, 0).fg, changed_theme().palette[1]);
    assert_eq!(t.cell(0, 0).bg, changed_theme().palette[4]);
    for (index, expected) in fixed.iter().enumerate() {
        assert_eq!(t.cell(index + 1, 0), expected);
        assert_eq!(expected.flags & (PALETTE_FG | PALETTE_BG), 0);
        assert_eq!(expected.palette_indices, 0);
    }
    t.feed(b"G");
    assert_eq!(t.cell(4, 0).fg, fixed[2].fg);
    assert_eq!(t.cell(4, 0).bg, fixed[2].bg);
}

#[test]
fn color_resets_clear_only_the_corresponding_provenance() {
    let mut t = Terminal::new(16, 2, 0);
    t.feed(b"\x1b[1;31;44mP\x1b[39mF\x1b[31;49mB\x1b[0mD");
    t.set_theme(changed_theme());
    let theme = t.theme();
    assert_eq!(t.cell(0, 0).fg, theme.palette[1]);
    assert_eq!(t.cell(0, 0).bg, theme.palette[4]);
    assert_eq!(t.cell(1, 0).fg, theme.foreground);
    assert_eq!(t.cell(1, 0).bg, theme.palette[4]);
    assert_eq!(t.cell(1, 0).flags, BOLD | EXPLICIT_BG | PALETTE_BG);
    assert_eq!(t.cell(1, 0).palette_indices, 0x40);
    assert_eq!(t.cell(2, 0).fg, theme.palette[1]);
    assert_eq!(t.cell(2, 0).bg, theme.background);
    assert_eq!(t.cell(2, 0).flags, BOLD | EXPLICIT_FG | PALETTE_FG);
    assert_eq!(t.cell(2, 0).palette_indices, 1);
    assert_eq!(t.cell(3, 0).fg, theme.foreground);
    assert_eq!(t.cell(3, 0).bg, theme.background);
    assert_eq!(t.cell(3, 0).flags, 0);
    assert_eq!(t.cell(3, 0).palette_indices, 0);
}

#[test]
fn history_and_rotated_screen_rows_update_without_losing_text() {
    let mut t = Terminal::new(4, 2, 16);
    t.feed(b"D\r\n\x1b[31;44mP\r\n\x1b[0mL");
    assert_eq!(t.history_len(), 1);
    t.set_theme(changed_theme());
    let theme = t.theme();
    assert_eq!(t.history_len(), 1);
    assert_eq!(t.view_cell(0, 0, 1).ch, 'D');
    assert_eq!(t.view_cell(0, 0, 1).fg, theme.foreground);
    assert_eq!(t.view_cell(0, 0, 1).bg, theme.background);
    assert_eq!(t.cell(0, 0).ch, 'P');
    assert_eq!(t.cell(0, 0).fg, theme.palette[1]);
    assert_eq!(t.cell(0, 0).bg, theme.palette[4]);
    assert_eq!(t.cell(0, 1).ch, 'L');
    assert_eq!(t.cell(0, 1).fg, theme.foreground);
    assert_eq!(t.cell(0, 1).bg, theme.background);
}

#[test]
fn saved_styles_update_in_both_primary_and_cached_alternate_screens() {
    let mut t = Terminal::new(8, 2, 8);
    t.feed(b"\x1b[31mP\x1b[?1049h\x1b[0mA");
    t.set_theme(changed_theme());
    assert_eq!(t.cell(0, 0).fg, changed_theme().foreground);
    t.feed(b"\x1b[?1049lQ");
    assert_eq!(t.cell(0, 0).ch, 'P');
    assert_eq!(t.cell(0, 0).fg, changed_theme().palette[1]);
    assert_eq!(t.cell(1, 0).ch, 'Q');
    assert_eq!(t.cell(1, 0).fg, changed_theme().palette[1]);

    t.feed(b"\x1b[?47h\x1b[32;43mB\x1b7\x1b[0m\x1b[?47l");
    let mut next = changed_theme();
    next.foreground = 0xabcdef;
    next.background = 0xfedcba;
    next.palette[2] = 0x001122;
    next.palette[3] = 0x334455;
    t.set_theme(next);
    t.feed(b"\x1b[?47h");
    assert_eq!(t.cell(0, 0).ch, 'A');
    assert_eq!(t.cell(0, 0).fg, next.foreground);
    assert_eq!(t.cell(1, 0).ch, 'B');
    assert_eq!(t.cell(1, 0).fg, next.palette[2]);
    assert_eq!(t.cell(1, 0).bg, next.palette[3]);
    t.feed(b"\x1b8C");
    assert_eq!(t.cell(2, 0).ch, 'C');
    assert_eq!(t.cell(2, 0).fg, next.palette[2]);
    assert_eq!(t.cell(2, 0).bg, next.palette[3]);
}

#[test]
fn erased_cells_keep_color_sources_and_resized_blanks_use_current_defaults() {
    let mut t = Terminal::new(8, 2, 8);
    t.feed(b"\x1b[31;44m\x1b[2K");
    t.set_theme(changed_theme());
    assert_eq!(t.cell(7, 0).ch, ' ');
    assert_eq!(t.cell(7, 0).fg, changed_theme().palette[1]);
    assert_eq!(t.cell(7, 0).bg, changed_theme().palette[4]);
    t.resize(10, 3);
    assert_eq!(t.cell(9, 2).fg, changed_theme().foreground);
    assert_eq!(t.cell(9, 2).bg, changed_theme().background);
    t.feed(b"\x1b[0m\x1b[2J");
    assert_eq!(t.cell(0, 0).fg, changed_theme().foreground);
    assert_eq!(t.cell(0, 0).bg, changed_theme().background);
}

#[test]
fn wide_cells_combining_marks_hyperlinks_and_rendition_survive() {
    let mut t = Terminal::new(12, 2, 0);
    t.feed(b"\x1b]8;;https://example.com\x1b\\\x1b[1;3;4;7;9;31;44m");
    t.feed("界e\u{301}".as_bytes());
    let before = [*t.cell(0, 0), *t.cell(1, 0), *t.cell(2, 0)];
    t.set_theme(changed_theme());
    for (index, cell) in before.into_iter().enumerate() {
        let expected = Cell {
            fg: changed_theme().palette[1],
            bg: changed_theme().palette[4],
            ..cell
        };
        assert_eq!(*t.cell(index, 0), expected);
    }
    assert_ne!(t.cell(0, 0).link_id, 0);
    assert_eq!(t.cell(2, 0).combining[0], '\u{301}');
}

#[test]
fn no_op_does_not_dirty_rows_and_shell_metadata_survives_theme_change() {
    let mut t = Terminal::new(40, 3, 8);
    t.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07echo x\r\n\x1b]133;C\x07x\r\n\x1b]133;D;0\x07");
    let command = t.commands().next().unwrap().clone();
    let output = t.command_output(command.id, 1024).unwrap();
    let input = t.command_input(command.id, 1024).unwrap();
    let cursor = t.cursor();
    let modes = *t.modes();
    t.clear_dirty();
    t.set_theme(t.theme());
    assert!(!(0..t.rows()).any(|row| t.row_dirty(row)));
    t.set_theme(changed_theme());
    assert_eq!(t.commands().next(), Some(&command));
    assert_eq!(t.command_output(command.id, 1024).unwrap(), output);
    assert_eq!(t.command_input(command.id, 1024).unwrap(), input);
    assert_eq!(t.cursor(), cursor);
    assert_eq!(*t.modes(), modes);
}

#[test]
fn osc_queries_report_current_colors_with_unchanged_terminators() {
    let mut t = Terminal::new(8, 2, 0);
    t.set_theme(changed_theme());
    t.feed(b"\x1b]10;?;?\x07\x1b]4;1;?\x1b\\");
    assert_eq!(t.responses(), b"\x1b]10;rgb:1212/3434/5656\x07\x1b]11;rgb:6565/4343/2121\x07\x1b]4;1;rgb:1111/2222/0101\x1b\\");
    t.clear_responses();
    // OSC color writes remain unsupported; adding the host-side API does not
    // silently expand the set of controls accepted from an untrusted process.
    t.feed(b"\x1b]10;#ffffff\x07\x1b]11;rgb:00/00/00\x07\x1b]4;1;#ffffff\x07");
    assert_eq!(t.theme(), changed_theme());
    assert!(t.responses().is_empty());
}

#[test]
fn invalid_color_sequences_preserve_the_previous_source() {
    let mut t = Terminal::new(16, 2, 0);
    t.feed(b"\x1b[31;44m\x1b[38:5:256m\x1b[48:2:999:0:0mX");
    t.set_theme(changed_theme());
    assert_eq!(t.cell(0, 0).fg, changed_theme().palette[1]);
    assert_eq!(t.cell(0, 0).bg, changed_theme().palette[4]);
    // Resetting the terminal retains the newly configured theme.
    t.feed(b"\x1bcD");
    assert_eq!(t.theme(), changed_theme());
    assert_eq!(t.cell(0, 0).fg, changed_theme().foreground);
    assert_eq!(t.cell(0, 0).bg, changed_theme().background);
    assert_eq!(t.cell(0, 0).flags, 0);
}
