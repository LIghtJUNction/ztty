#![no_std]
extern crate alloc;

pub fn exercise() {
    let mut terminal = zterm::Terminal::new(80, 24, 20);
    terminal.feed("\x1b[31m你a\x1b[6n".as_bytes());
    assert_eq!(terminal.cell(0, 0).ch, '你');
    assert_eq!(terminal.cell(0, 0).width, 2);
    assert_eq!(terminal.responses(), b"\x1b[1;4R");
    assert!(!terminal.modes().input().bracketed_paste);
    terminal.set_ui_host_enabled(true);
    terminal.feed(b"\x1b_ZUI;1;{\"v\":1,\"op\":\"replace\",\"surface\":1,\"rev\":1,\"root\":1,\"nodes\":[{\"id\":1,\"type\":\"text\",\"text\":\"registry-ui-ok\"}]}\x1b\\");
    assert_eq!(terminal.ui_scene().unwrap().surface, 1);
    assert_eq!(zterm::ui::VERSION, 1);
    terminal.reset_ui();
    assert!(terminal.ui_scene().is_none());
}
