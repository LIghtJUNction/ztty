#![no_std]
extern crate alloc;

pub fn exercise() {
    let modes = zinput::Modes { kitty_keyboard: 1, ..Default::default() };
    let bytes = zinput::encode_key(zinput::Key::Character('c'),
        zinput::Modifiers(4), zinput::KeyEvent::Press, "", &modes);
    assert_eq!(bytes, b"\x1b[99;5u");
    assert_eq!(zinput::encode_key(zinput::Key::KeypadDigit(1),
        zinput::Modifiers(zinput::Modifiers::NUM_LOCK), zinput::KeyEvent::Press,
        "1", &modes), b"1");
    let mut scene = zgraphics::Graphics::default();
    let mut replies = alloc::vec::Vec::new();
    let context = zgraphics::Context { col: 0, row: 0, cols: 80, rows: 24,
        cell_width: 8, cell_height: 16 };
    scene.command(b"Ga=T,f=32,s=1,v=1,i=7,C=1;/wAA/w==", context, &mut replies);
    assert_eq!(scene.image(7).unwrap().pixels.as_slice(), &[255,0,0,255]);
    assert_eq!(scene.draws(0).count(), 1);
    scene.command(b"Ga=d,d=I,i=7", context, &mut replies);
    assert_eq!(scene.image_count(), 0);
    let mut ui = zgpui::State::new();
    assert_eq!(ui.apply(br#"{"v":1,"op":"capabilities"}"#),
        Ok(zgpui::Change::Capabilities));
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
    let _: zinput::Modes = terminal.modes().input();
    let size = zpty::Size::new(80, 24);
    assert_eq!((size.cols, size.rows), (80, 24));
    assert_eq!(ztty_plugin_api::MAX_TEXT_BYTES, 1024 * 1024);
}

pub fn transform_text<T: ztty_plugin_api::TextAction>(
    plugin: &mut T, input: &str, output: &mut [u8],
) -> Option<u64> {
    plugin.transform(input, output).ok()
}
