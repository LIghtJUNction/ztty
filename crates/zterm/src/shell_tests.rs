use super::*;

fn command(terminal: &mut Terminal, output: &[u8]) -> u64 {
    terminal.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07run\r\n\x1b]133;C\x07");
    let id = terminal.commands().next_back().unwrap().id;
    terminal.feed(output);
    terminal.feed(b"\x1b]133;D;7\x1b\\");
    id
}

#[test]
fn semantic_boundaries_extract_input_and_output_and_status() {
    let mut terminal = Terminal::new(80, 8, 40);
    let id = command(&mut terminal, b"first\r\nsecond\r\n");
    terminal.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07");
    let record = terminal.commands().find(|record| record.id == id).unwrap();
    assert_eq!(record.exit_status, Some(7));
    assert!(record.end.is_some());
    assert_eq!(terminal.command_input(id, 100).unwrap().text, "run\n");
    let output = terminal.command_output(id, 100).unwrap();
    assert_eq!(output.text, "first\nsecond\n");
    assert!(output.complete);
    assert!(!output.truncated);
    assert_eq!(terminal.shell_phase(), ShellPhase::Input);
    assert_eq!(terminal.commands().len(), 2);
}

#[test]
fn semantic_markers_are_independent_of_chunk_boundaries() {
    let bytes = b"\x1b]133;A\x1b\\$ \x1b]133;B\x07x\r\n\x1b]133;C\x07done\r\n\x1b]133;D;0\x07";
    let mut whole = Terminal::new(40, 4, 10);
    whole.feed(bytes);
    let expected = whole.commands().cloned().collect::<std::vec::Vec<_>>();
    for boundary in 0..=bytes.len() {
        let mut split = Terminal::new(40, 4, 10);
        split.feed(&bytes[..boundary]);
        split.feed(&bytes[boundary..]);
        assert_eq!(
            split.commands().cloned().collect::<std::vec::Vec<_>>(),
            expected
        );
        assert_eq!(split.command_output(1, 100), whole.command_output(1, 100));
    }
}

#[test]
fn semantic_output_anchors_survive_scroll_and_history_eviction() {
    let mut terminal = Terminal::new(20, 2, 2);
    let id = command(&mut terminal, b"a\r\nb\r\nc\r\nd\r\n");
    let output = terminal.command_output(id, 100).unwrap();
    assert_eq!(output.text, "b\nc\nd\n");
    assert!(output.truncated);
    assert!(terminal.command_scroll_offset(id).is_none());
    let mut no_history = Terminal::new(20, 2, 0);
    let id = command(&mut no_history, b"a\r\nb\r\nc\r\n");
    let output = no_history.command_output(id, 100).unwrap();
    assert_eq!(output.text, "c\n");
    assert!(output.truncated);
}

#[test]
fn semantic_byte_limit_never_splits_utf8() {
    let mut terminal = Terminal::new(20, 4, 2);
    let id = command(&mut terminal, "a界é".as_bytes());
    let output = terminal.command_output(id, 4).unwrap();
    assert_eq!(output.text, "a界");
    assert!(output.truncated);
    assert_eq!(terminal.command_output(id, 0).unwrap().text, "");
    assert!(terminal.command_output(id, 0).unwrap().truncated);
    assert!(!terminal.command_output(id, 16).unwrap().truncated);
}

#[test]
fn semantic_prompt_redraw_secondary_prompt_and_duplicate_marks_are_bounded() {
    let mut terminal = Terminal::new(20, 4, 2);
    terminal.feed(b"\x1b]133;A\x07p\x1b]133;B\x07\x1b]133;A;k=s\x07");
    assert_eq!(terminal.shell_phase(), ShellPhase::Input);
    assert_eq!(terminal.commands().len(), 1);
    terminal.feed(b"\x1b]133;A\x07p\x1b]133;B\x07");
    assert_eq!(terminal.commands().len(), 1);
    terminal.feed(b"\x1b]133;C\x07\x1b]133;C\x07x\x1b]133;D;1\x07\x1b]133;D;2\x07");
    assert_eq!(terminal.commands().len(), 1);
    assert_eq!(terminal.commands().next().unwrap().exit_status, Some(1));
    for _ in 0..300 {
        terminal.feed(b"\x1b]133;C\x07x\x1b]133;D\x07");
    }
    assert_eq!(terminal.commands().len(), MAX_COMMANDS);
}

#[test]
fn semantic_invalid_exit_status_and_unknown_marks_do_not_end_command() {
    let mut terminal = Terminal::new(20, 4, 2);
    terminal.feed(b"\x1b]133;C\x07x");
    for marker in [
        b"\x1b]133;D;-1\x07".as_slice(),
        b"\x1b]133;D;4294967296\x07",
        b"\x1b]133;D;no\x07",
        b"\x1b]133;X\x07",
    ] {
        terminal.feed(marker);
        assert_eq!(terminal.shell_phase(), ShellPhase::Output);
    }
    terminal.feed(b"\x1b]133;D;0\x07");
    assert_eq!(terminal.commands().next().unwrap().exit_status, Some(0));
}

#[test]
fn semantic_alt_screen_marks_are_ignored_and_primary_output_survives() {
    let mut terminal = Terminal::new(20, 4, 8);
    let id = command(&mut terminal, b"primary\r\n");
    terminal.feed(b"\x1b[?1049h\x1b]133;A\x07\x1b]133;C\x07fake\x1b]133;D;9\x07");
    assert_eq!(terminal.commands().len(), 1);
    assert!(terminal.command_scroll_offset(id).is_none());
    assert_eq!(terminal.command_output(id, 100).unwrap().text, "primary\n");
    terminal.feed(b"\x1b[?1049l");
    assert_eq!(terminal.command_output(id, 100).unwrap().text, "primary\n");
}

#[test]
fn semantic_resize_clear_and_reset_never_return_reassigned_ranges() {
    let mut terminal = Terminal::new(20, 6, 10);
    let id = command(&mut terminal, b"alpha\r\nbeta\r\n");
    terminal.resize(20, 2);
    assert_eq!(
        terminal.command_output(id, 100).unwrap().text,
        "alpha\nbeta\n"
    );
    assert!(!terminal.command_output(id, 100).unwrap().truncated);
    terminal.resize(10, 2);
    let output = terminal.command_output(id, 100).unwrap();
    assert!(!output.truncated);
    assert_eq!(output.text, "alpha\nbeta\n");
    terminal.feed(b"\x1bc");
    assert_eq!(terminal.commands().len(), 0);
    let next = command(&mut terminal, b"new");
    assert!(next > id);
    terminal.feed(b"\x1b[2J");
    assert!(terminal.command_output(next, 100).unwrap().truncated);
    assert!(terminal.command_output(next, 100).unwrap().text.is_empty());
}

#[test]
fn semantic_next_prompt_closes_output_without_inventing_exit_status() {
    let mut terminal = Terminal::new(20, 4, 8);
    terminal.feed(b"\x1b]133;C\x07output\r\n\x1b]133;A\x07");
    let record = terminal.commands().next().unwrap();
    assert_eq!(record.exit_status, None);
    assert_eq!(
        terminal.command_output(record.id, 100).unwrap().text,
        "output\n"
    );
    assert!(terminal.command_output(record.id, 100).unwrap().complete);
}

#[test]
fn semantic_history_clear_marks_evicted_output_truncated() {
    let mut terminal = Terminal::new(20, 2, 8);
    let id = command(&mut terminal, b"first\r\nsecond\r\nthird\r\n");
    terminal.feed(b"\x1b[3J");
    let output = terminal.command_output(id, 100).unwrap();
    assert!(output.truncated);
    assert_eq!(output.text, "third\n");
}
