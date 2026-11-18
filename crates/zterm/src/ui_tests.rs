use crate::{Terminal, ui};
use alloc::format;
use alloc::vec::Vec;

fn scene(rev: u64) -> alloc::string::String {
    format!(
        r#"{{"v":1,"op":"replace","surface":9,"rev":{rev},"root":1,"events":true,"nodes":[{{"id":1,"type":"column","children":[2,3]}},{{"id":2,"type":"text","text":"Native UI"}},{{"id":3,"type":"button","label":"Run","action":"run"}}]}}"#
    )
}

fn frame(json: &str) -> Vec<u8> {
    let mut bytes = b"\x1b_ZUI;1;".to_vec();
    bytes.extend_from_slice(json.as_bytes());
    bytes.extend_from_slice(b"\x1b\\");
    bytes
}

const IMAGE: &[u8] = b"\x1b_Ga=t,f=32,s=1,v=1,i=42,q=2;/wAA/w==\x1b\\";

#[test]
fn ui_every_split_is_transactional_and_preserves_text_and_kgp() {
    let mut bytes = b"A".to_vec();
    bytes.extend(frame(&scene(1)));
    bytes.extend(IMAGE);
    bytes.extend_from_slice(b"B");
    for split in 0..=bytes.len() {
        let mut t = terminal();
        t.feed(&bytes[..split]);
        t.feed(&bytes[split..]);
        assert_eq!(t.ui_scene().unwrap().revision, 1);
        assert_eq!(t.ui_revision(), 1);
        assert_eq!(
            t.graphics().image(42).unwrap().pixels.as_slice(),
            [255, 0, 0, 255]
        );
        assert_eq!(t.cell(0, 0).ch, 'A');
        assert_eq!(t.cell(1, 0).ch, 'B');
        assert_eq!(t.cursor(), (2, 0));
    }
    let mut t = terminal();
    for byte in &bytes {
        t.feed(core::slice::from_ref(byte));
    }
    assert!(t.ui_scene().is_some());
    assert!(t.graphics().image(42).is_some());
}

#[test]
fn interleaved_ui_does_not_cancel_chunked_kgp_transfer() {
    let mut t = terminal();
    t.feed(b"\x1b_Ga=t,f=32,s=1,v=1,i=42,q=2,m=1;/wAA\x1b\\");
    t.feed(&frame(&scene(1)));
    t.feed(b"\x1b_Gm=0;/w==\x1b\\");
    assert_eq!(
        t.graphics().image(42).unwrap().pixels.as_slice(),
        [255, 0, 0, 255]
    );
    assert!(t.ui_scene().is_some());
}

#[test]
fn cancelled_ui_and_unrecognized_namespaces_do_not_leak_or_mutate() {
    for cancel in [0x18, 0x1a] {
        let mut t = terminal();
        t.feed(&frame(&scene(1)));
        let mut partial = frame(&scene(2));
        partial.truncate(partial.len() - 2);
        partial.push(cancel);
        partial.push(b'Q');
        t.feed(&partial);
        assert_eq!(t.ui_scene().unwrap().revision, 1);
        assert_eq!(t.cell(0, 0).ch, 'Q');
        t.feed(b"\x1b_ZUI;2;unsupported\x1b\\\x1b_Xnot-ui\x1b\\R");
        assert_eq!(t.cell(1, 0).ch, 'R');
        assert_eq!(t.ui_revision(), 1);
    }
    let mut t = terminal();
    t.feed(&frame(&scene(1)));
    t.feed(b"\x1b_ZUI;1;{\"unfinished\":\x1b[31mQ");
    assert_eq!(t.cell(0, 0).ch, 'Q');
    assert_eq!(t.ui_scene().unwrap().revision, 1);
}

#[test]
fn oversized_frame_is_discarded_until_st_then_normal_text_recovers() {
    let mut t = terminal();
    t.feed(&frame(&scene(1)));
    t.feed(b"\x1b_ZUI;1;");
    for _ in 0..65 {
        t.feed(&[b'x'; 1024]);
    }
    t.feed(b"\x1b\\OK");
    assert_eq!(t.ui_scene().unwrap().revision, 1);
    assert_eq!(t.cursor(), (2, 0));
    assert_eq!(t.cell(0, 0).ch, 'O');
    assert_eq!(t.cell(1, 0).ch, 'K');
}

#[test]
fn ris_and_real_screen_transitions_clear_ui_without_revision_reuse() {
    let mut t = terminal();
    t.feed(&frame(&scene(1)));
    let painted = t.ui_revision();
    t.feed(b"\x1bc");
    assert!(t.ui_scene().is_none());
    assert!(t.ui_revision() > painted);
    t.feed(&frame(&scene(1)));
    assert!(t.ui_scene().is_none());
    t.feed(&frame(&scene(2)));
    assert!(t.ui_scene().is_some());
    t.feed(b"\x1b[?1049h");
    assert!(t.ui_scene().is_none());
    t.feed(&frame(&scene(3)));
    t.feed(b"\x1b[?1049h");
    assert!(t.ui_scene().is_some());
    t.feed(b"\x1b[?1049l");
    assert!(t.ui_scene().is_none());
    t.feed(&frame(&scene(4)));
    t.reset_ui();
    assert!(t.ui_scene().is_none());
    assert!(t.ui_action(3, 9, 4).is_none());
}

#[test]
fn capabilities_reply_is_explicit_and_actions_are_never_auto_written() {
    let mut t = terminal();
    t.feed(&frame(&scene(5)));
    assert!(t.responses().is_empty());
    let event = t.ui_action(3, 9, 5).unwrap();
    assert_eq!(event.action, "run");
    assert!(t.responses().is_empty());
    assert!(t.ui_action(3, 9, 4).is_none());
    t.feed(&frame(r#"{"v":1,"op":"capabilities"}"#));
    let reply = core::str::from_utf8(t.responses()).unwrap();
    assert!(reply.starts_with("\x1b_ZUI;1;"));
    assert!(reply.ends_with("\x1b\\"));
    assert!(reply.contains("\"last_rev\":5"));
    assert!(reply.contains("\"max_frame\":65536"));
    assert!(
        matches!(&t.ui_scene().unwrap().node(2).unwrap().kind, ui::Kind::Text{text,..} if text=="Native UI")
    );
}

fn terminal() -> Terminal {
    let mut terminal = Terminal::new(20, 8, 8);
    terminal.set_ui_host_enabled(true);
    terminal
}

#[test]
fn unsupported_host_never_claims_renderer_or_accepts_hidden_actions() {
    let mut t = Terminal::new(20, 8, 8);
    t.feed(&frame(&scene(1)));
    assert!(t.ui_scene().is_none());
    t.feed(&frame(r#"{"v":1,"op":"capabilities"}"#));
    let reply = core::str::from_utf8(t.responses()).unwrap();
    assert!(reply.contains("\"supported\":false"));
    assert!(reply.contains("\"events\":false"));
    assert!(reply.contains("\"components\":[]"));
    t.clear_responses();
    t.set_ui_host_enabled(true);
    t.feed(&frame(r#"{"v":1,"op":"capabilities"}"#));
    let reply = core::str::from_utf8(t.responses()).unwrap();
    assert!(reply.contains("\"supported\":true"));
    assert!(reply.contains("\"last_rev\":0"));
    t.feed(&frame(&scene(1)));
    assert!(t.ui_action(3, 9, 1).is_some());
    let pending = frame(&scene(2));
    t.feed(&pending[..pending.len() - 2]);
    t.set_ui_host_enabled(false);
    assert!(t.ui_scene().is_none());
    assert!(t.ui_action(3, 9, 1).is_none());
    t.set_ui_host_enabled(true);
    t.feed(&pending[pending.len() - 2..]);
    assert!(t.ui_scene().is_none());
    t.feed(&frame(&scene(1)));
    assert!(t.ui_scene().is_none());
    t.feed(&frame(&scene(2)));
    assert_eq!(t.ui_scene().unwrap().revision, 2);
}
