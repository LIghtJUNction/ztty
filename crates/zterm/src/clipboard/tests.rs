use super::*;
use crate::Terminal;
use alloc::{format, vec};

fn set_frame(text: &[u8]) -> Vec<u8> {
    let mut queue = Queue::default();
    queue.receive(b"c;?", false);
    queue.pop().unwrap().reply(b'c', Some(text))
}
#[test]
fn osc52_and_other_osc_remain_independent_of_every_pty_split() {
    let data = b"\x1b]2;title\x07A\x1b]52;c;5Lit5paH8J+YgA==\x1b\\B\x1b]52;p;?\x07C";
    for split in 0..=data.len() {
        let mut t = Terminal::new(20, 2, 0);
        t.feed(&data[..split]);
        t.feed(&data[split..]);
        assert_eq!(t.title(), "title");
        let set = t.next_clipboard_request().unwrap();
        assert_eq!(set.text.as_deref(), Some("中文😀"));
        assert_eq!(set.selectors(), b"c");
        let query = t.next_clipboard_request().unwrap();
        assert!(query.text.is_none() && query.bell_terminated);
        assert_eq!(query.selectors(), b"p");
        assert!(t.next_clipboard_request().is_none());
        assert_eq!(
            (t.cell(0, 0).ch, t.cell(1, 0).ch, t.cell(2, 0).ch),
            ('A', 'B', 'C')
        );
    }
}
#[test]
fn osc52_large_text_uses_the_full_documented_byte_budget() {
    let text = vec![b'x'; MAX_CLIPBOARD_BYTES];
    let bytes = set_frame(&text);
    let mut t = Terminal::new(4, 2, 0);
    for chunk in bytes.chunks(997) {
        t.feed(chunk);
    }
    assert_eq!(
        t.next_clipboard_request().unwrap().text.unwrap().as_bytes(),
        text
    );
    assert!(t.next_clipboard_request().is_none());
    // The fixed title/OSC parser retains its old overflow behavior.
    t.feed(b"\x1b]2;");
    t.feed(&vec![b'z'; 5000]);
    t.feed(b"\x07");
    assert_eq!(t.title(), "");
}
#[test]
fn aborted_or_overlimit_requests_never_clear_or_replace_clipboard() {
    for tail in [b"\x18".as_slice(), b"\x1a", b"\x1b[31m"] {
        let mut t = Terminal::new(8, 2, 0);
        t.feed(b"\x1b]52;c;YWJj");
        t.feed(tail);
        t.feed(b"OK");
        assert!(t.next_clipboard_request().is_none());
        assert_eq!((t.cell(0, 0).ch, t.cell(1, 0).ch), ('O', 'K'));
    }
    let mut t = Terminal::new(8, 2, 0);
    t.feed(b"\x1b]52;c;");
    t.feed(&vec![b'A'; MAX_ENCODED + 1]);
    t.feed(b"\x1b\\OK");
    assert!(t.next_clipboard_request().is_none());
    assert_eq!(t.cell(0, 0).ch, 'O');
}
#[test]
fn invalid_base64_clears_but_invalid_utf8_or_nul_is_rejected() {
    let mut t = Terminal::new(4, 2, 0);
    for bad in [b"/w==".as_slice(), b"AA=="] {
        t.feed(b"\x1b]52;c;");
        t.feed(bad);
        t.feed(b"\x07");
        assert!(t.next_clipboard_request().is_none());
    }
    t.feed(b"\x1b]52;c;!\x07");
    assert_eq!(
        t.next_clipboard_request().unwrap().text.as_deref(),
        Some("")
    );
    t.feed(b"\x1b]52;c;\x1b\\");
    assert_eq!(
        t.next_clipboard_request().unwrap().text.as_deref(),
        Some("")
    );
}
#[test]
fn queue_bounds_count_and_aggregate_decoded_bytes() {
    let mut q = Queue::default();
    for _ in 0..MAX_CLIPBOARD_REQUESTS + 3 {
        q.receive(b"c;?", false);
    }
    assert_eq!(q.pending.len(), MAX_CLIPBOARD_REQUESTS);
    q.cancel();
    let mut t = Terminal::new(4, 2, 0);
    let frame = set_frame(&vec![b'x'; MAX_CLIPBOARD_BYTES / 2]);
    for _ in 0..3 {
        t.feed(&frame);
    }
    assert_eq!(
        t.next_clipboard_request().unwrap().text.unwrap().len(),
        MAX_CLIPBOARD_BYTES / 2
    );
    assert_eq!(
        t.next_clipboard_request().unwrap().text.unwrap().len(),
        MAX_CLIPBOARD_BYTES / 2
    );
    assert!(t.next_clipboard_request().is_none());
}
#[test]
fn reset_and_host_cancellation_invalidate_delayed_requests() {
    let mut t = Terminal::new(4, 2, 0);
    let original = t.clipboard_epoch();
    t.feed(b"\x1b]52;c;?\x07\x1bc");
    assert!(t.next_clipboard_request().is_none());
    assert_ne!(t.clipboard_epoch(), original);
    let original = t.clipboard_epoch();
    t.feed(b"\x1b]52;c;YW");
    t.cancel_clipboard_requests();
    t.feed(b"Jj\x1b\\OK");
    assert!(t.next_clipboard_request().is_none());
    assert_ne!(t.clipboard_epoch(), original);
    assert_eq!(t.cell(0, 0).ch, 'O');
}
#[test]
fn replies_preserve_terminator_and_never_return_partial_invalid_text() {
    let mut q = Queue::default();
    q.receive(b"c;?", true);
    let query = q.pop().unwrap();
    assert_eq!(
        query.reply(b'c', Some("中文😀".as_bytes())),
        b"\x1b]52;c;5Lit5paH8J+YgA==\x07"
    );
    for bad in [vec![0], vec![255], vec![b'x'; MAX_CLIPBOARD_BYTES + 1]] {
        assert_eq!(query.reply(b'c', Some(&bad)), b"\x1b]52;c;\x07");
    }
    q.receive(b"p;?", false);
    assert_eq!(
        q.pop().unwrap().reply(b'p', Some(b"abc")),
        b"\x1b]52;p;YWJj\x1b\\"
    );
}
#[test]
fn selector_order_defaults_and_deduplication_are_bounded() {
    let mut q = Queue::default();
    q.receive(b";?", false);
    assert_eq!(q.pop().unwrap().selectors(), b"s0");
    q.receive(b"cppq01234567s;?", false);
    assert_eq!(q.pop().unwrap().selectors(), b"cpq01234567s");
    q.receive(b"unknown;?", false);
    assert!(q.pop().is_none());
    for (raw, expected) in [
        ("Zg==", "f"),
        ("Zm8=", "fo"),
        ("Zm9v", "foo"),
        ("Zm9vYg==", "foob"),
        ("Zm9vYmE=", "fooba"),
        ("Zm9vYmFy", "foobar"),
    ] {
        q.receive(format!("c;{raw}").as_bytes(), false);
        assert_eq!(q.pop().unwrap().text.as_deref(), Some(expected));
    }
}
