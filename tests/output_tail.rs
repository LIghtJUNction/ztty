extern crate alloc;

#[path = "../src/output_tail.rs"]
mod output_tail;

use output_tail::OutputTail;

const LOCAL: usize = 64 * 1024;
const PERSISTENT: usize = 256 * 1024;

#[test]
fn empty_output_and_end_cursor_have_no_more_data() {
    let mut tail = OutputTail::new();
    tail.append(b"", false);
    let empty = tail.read(0, None, false).unwrap();
    assert!(empty.bytes.is_empty());
    assert_eq!((empty.first_cursor, empty.next_cursor), (0, 0));
    assert!(!empty.truncated && !empty.has_more);
    tail.append(b"abc", false);
    let end = tail.read(3, None, false).unwrap();
    assert!(end.bytes.is_empty());
    assert_eq!((end.first_cursor, end.next_cursor), (0, 3));
    assert!(!end.truncated && !end.has_more);
}

#[test]
fn request_limits_and_default_page_size_match_the_wire_contract() {
    let mut tail = OutputTail::new();
    for max in [0, 65537, u64::MAX] {
        assert_eq!(
            tail.read(1, Some(max), false).err(),
            Some("max_bytes must be 1..65536")
        );
    }
    assert_eq!(
        tail.read(1, None, false).err(),
        Some("cursor is beyond the available output")
    );
    tail.append(&vec![b'a'; 70000], true);
    let default = tail.read(0, None, false).unwrap();
    assert_eq!(default.bytes.len(), 16384);
    assert_eq!(default.next_cursor, 16384);
    assert!(default.has_more);
    let largest = tail.read(0, Some(65536), false).unwrap();
    assert_eq!(largest.bytes.len(), 65536);
    assert_eq!(largest.next_cursor, 65536);
    assert!(largest.has_more);
    assert_eq!(
        tail.read(70001, Some(1), false).err(),
        Some("cursor is beyond the available output")
    );
}

#[test]
fn consumers_can_disconnect_and_replay_without_advancing_each_other() {
    let mut tail = OutputTail::new();
    tail.append(b"first\nsecond\n", false);
    let first = tail.read(0, Some(6), false).unwrap();
    assert_eq!(first.bytes, b"first\n");
    let disconnected_cursor = first.next_cursor;
    let other = tail.read(0, None, false).unwrap();
    assert_eq!(other.bytes, b"first\nsecond\n");
    tail.append(b"third\n", false);
    let resumed = tail.read(disconnected_cursor, None, false).unwrap();
    assert_eq!(resumed.bytes, b"second\nthird\n");
    assert_eq!(
        tail.read(disconnected_cursor, None, false).unwrap().bytes,
        resumed.bytes
    );
    assert_eq!(
        tail.read(0, None, false).unwrap().bytes,
        b"first\nsecond\nthird\n"
    );
}

#[test]
fn disconnected_consumers_get_explicit_truncation_after_eviction() {
    let mut tail = OutputTail::new();
    tail.append(b"previous", false);
    let saved = tail.read(0, None, false).unwrap().next_cursor;
    tail.append(&vec![b'n'; LOCAL + 17], false);
    let resumed = tail.read(saved, Some(65536), false).unwrap();
    assert!(resumed.truncated);
    assert_eq!(resumed.first_cursor, 25);
    assert_eq!(resumed.next_cursor, 25 + LOCAL as u64);
    assert_eq!(resumed.bytes, vec![b'n'; LOCAL]);
    assert!(!resumed.has_more);
    let repeated = tail.read(saved, Some(65536), false).unwrap();
    assert_eq!(repeated.bytes, resumed.bytes);
    assert_eq!(repeated.next_cursor, resumed.next_cursor);
}

#[test]
fn persistent_retention_is_exactly_256_kib_across_independent_pages() {
    let mut tail = OutputTail::new();
    tail.append(&vec![b'p'; PERSISTENT + 7], true);
    let mut cursor = 0;
    let mut received = Vec::new();
    loop {
        let page = tail.read(cursor, Some(65536), false).unwrap();
        assert_eq!(page.first_cursor, 7);
        assert_eq!(page.truncated, cursor < 7);
        received.extend_from_slice(&page.bytes);
        cursor = page.next_cursor;
        if !page.has_more {
            break;
        }
    }
    assert_eq!(received, vec![b'p'; PERSISTENT]);
    assert_eq!(cursor, PERSISTENT as u64 + 7);
}

#[test]
fn evicted_utf8_continuation_prefix_is_skipped_without_losing_byte_offsets() {
    let mut input = b"x".to_vec();
    input.extend_from_slice("🚀".as_bytes());
    input.extend(vec![b'y'; LOCAL - 2]);
    let mut tail = OutputTail::new();
    tail.append(&input, false);
    let page = tail.read(0, Some(1), false).unwrap();
    assert_eq!(page.first_cursor, 3);
    assert_eq!(page.bytes, b"y");
    assert_eq!(page.next_cursor, 6);
    assert!(page.truncated && page.has_more);
}

#[test]
fn wholly_evicted_continuation_runs_advance_to_the_available_end() {
    let mut tail = OutputTail::new();
    tail.append(&vec![0x80; LOCAL + 2], false);
    let page = tail.read(0, None, false).unwrap();
    assert!(page.bytes.is_empty());
    assert_eq!(page.first_cursor, 2);
    assert_eq!(page.next_cursor, LOCAL as u64 + 2);
    assert!(page.truncated);
    assert!(!page.has_more);
}

#[test]
fn four_byte_utf8_fed_one_byte_at_a_time_waits_then_extends_a_one_byte_page() {
    let mut tail = OutputTail::new();
    let encoded = "🚀".as_bytes();
    for (index, byte) in encoded.iter().enumerate() {
        tail.append(core::slice::from_ref(byte), false);
        let page = tail.read(0, Some(1), false).unwrap();
        if index + 1 < encoded.len() {
            assert!(page.bytes.is_empty());
            assert_eq!(page.next_cursor, 0);
            assert!(page.has_more);
        } else {
            assert_eq!(page.bytes, encoded);
            assert_eq!(page.next_cursor, 4);
            assert!(!page.has_more);
        }
        assert!(!page.truncated);
    }
    tail.append(b"!", false);
    assert_eq!(tail.read(4, Some(1), false).unwrap().bytes, b"!");
}

#[test]
fn complete_utf8_pages_preserve_characters_and_extend_at_most_three_bytes() {
    let bytes = "a¢€🚀z".as_bytes();
    let mut tail = OutputTail::new();
    tail.append(bytes, false);
    for max in 1..=4 {
        let mut cursor = 0;
        let mut reconstructed = Vec::new();
        loop {
            let page = tail.read(cursor, Some(max), false).unwrap();
            assert!(page.bytes.len() <= max as usize + 3);
            assert!(core::str::from_utf8(&page.bytes).is_ok());
            assert!(page.next_cursor > cursor);
            reconstructed.extend_from_slice(&page.bytes);
            cursor = page.next_cursor;
            if !page.has_more {
                break;
            }
        }
        assert_eq!(reconstructed, bytes);
        assert_eq!(cursor, bytes.len() as u64);
    }
}

#[test]
fn invalid_prefix_does_not_consume_a_later_incomplete_utf8_suffix() {
    let mut tail = OutputTail::new();
    tail.append(b"\xff\xf0\x9f", false);
    let prefix = tail.read(0, None, false).unwrap();
    assert_eq!(prefix.bytes, b"\xff");
    assert_eq!(String::from_utf8_lossy(&prefix.bytes), "�");
    assert_eq!(prefix.next_cursor, 1);
    assert!(prefix.has_more);
    let pending = tail.read(1, Some(1), false).unwrap();
    assert!(pending.bytes.is_empty());
    assert_eq!(pending.next_cursor, 1);
    assert!(pending.has_more);
    tail.append(b"\x9a\x80", false);
    let completed = tail.read(1, Some(1), false).unwrap();
    assert_eq!(completed.bytes, "🚀".as_bytes());
    assert_eq!(completed.next_cursor, 5);
    assert!(!completed.has_more);
}

#[test]
fn multiple_malformed_sequences_before_an_incomplete_suffix_are_consumed_once() {
    let mut tail = OutputTail::new();
    tail.append(b"a\xc0\x80b\xe2\x82", false);
    let prefix = tail.read(0, None, false).unwrap();
    assert_eq!(prefix.bytes, b"a\xc0\x80b");
    assert_eq!(String::from_utf8_lossy(&prefix.bytes), "a��b");
    assert_eq!(prefix.next_cursor, 4);
    assert!(prefix.has_more);
    tail.append(b"\xacZ", false);
    let completed = tail.read(prefix.next_cursor, None, false).unwrap();
    assert_eq!(completed.bytes, "€Z".as_bytes());
    assert_eq!(completed.next_cursor, 8);
    assert!(!completed.has_more);
}

#[test]
fn known_invalid_sequences_do_not_wait_for_output_that_cannot_repair_them() {
    for bytes in [
        b"\xff".as_slice(),
        b"\x80",
        b"\xc0\x80",
        b"\xf4\x90",
        b"\xe2(",
    ] {
        let mut tail = OutputTail::new();
        tail.append(bytes, false);
        let page = tail.read(0, None, false).unwrap();
        assert_eq!(page.bytes, bytes);
        assert_eq!(page.next_cursor, bytes.len() as u64);
        assert!(!page.has_more);
    }
}

#[test]
fn eof_flushes_an_incomplete_suffix_lossily_without_changing_byte_cursors() {
    let mut tail = OutputTail::new();
    tail.append(b"\xff\xe2\x82", false);
    let live = tail.read(0, None, false).unwrap();
    assert_eq!(live.bytes, b"\xff");
    assert_eq!(live.next_cursor, 1);
    assert!(live.has_more);
    let final_page = tail.read(live.next_cursor, Some(1), true).unwrap();
    assert_eq!(final_page.bytes, b"\xe2\x82");
    assert_eq!(String::from_utf8_lossy(&final_page.bytes), "�");
    assert_eq!(final_page.next_cursor, 3);
    assert!(!final_page.has_more);
    let replay = tail.read(0, None, true).unwrap();
    assert_eq!(String::from_utf8_lossy(&replay.bytes), "��");
    assert_eq!(replay.next_cursor, 3);
}

#[test]
fn increasing_retention_never_resurrects_already_evicted_bytes() {
    let mut tail = OutputTail::new();
    tail.append(&vec![b'a'; LOCAL], false);
    tail.append(b"B", false);
    let before = tail.read(0, Some(1), false).unwrap();
    assert_eq!(before.first_cursor, 1);
    assert!(before.truncated);
    tail.append(b"suffix", true);
    let grown = tail.read(0, Some(1), false).unwrap();
    assert_eq!(grown.first_cursor, 1);
    assert!(grown.truncated);
    assert_eq!(grown.bytes, b"a");
    let suffix = tail.read(LOCAL as u64, None, false).unwrap();
    assert_eq!(suffix.bytes, b"Bsuffix");
    assert_eq!(suffix.next_cursor, LOCAL as u64 + 7);
    assert!(!suffix.has_more);
    let full_cursor = suffix.next_cursor;
    tail.append(&vec![b'p'; PERSISTENT], true);
    let overwritten = tail.read(0, Some(1), false).unwrap();
    assert_eq!(overwritten.first_cursor, full_cursor);
    assert_eq!(overwritten.bytes, b"p");
    assert!(overwritten.truncated);
}

#[test]
fn lowering_retention_drops_only_the_oldest_bytes() {
    let mut tail = OutputTail::new();
    tail.append(&vec![b'p'; PERSISTENT], true);
    tail.append(b"x", false);
    let page = tail.read(0, Some(65536), false).unwrap();
    assert_eq!(page.first_cursor, (PERSISTENT + 1 - LOCAL) as u64);
    assert_eq!(page.bytes.len(), LOCAL);
    assert!(page.bytes[..LOCAL - 1].iter().all(|&byte| byte == b'p'));
    assert_eq!(page.bytes[LOCAL - 1], b'x');
    assert_eq!(page.next_cursor, PERSISTENT as u64 + 1);
    assert!(page.truncated);
    assert!(!page.has_more);
}

#[test]
fn an_explicit_cursor_inside_a_codepoint_keeps_existing_raw_byte_semantics() {
    let mut tail = OutputTail::new();
    tail.append("🚀".as_bytes(), false);
    let page = tail.read(1, Some(1), false).unwrap();
    assert_eq!(page.bytes, [0x9f, 0x9a, 0x80]);
    assert_eq!(page.next_cursor, 4);
    assert!(!page.truncated && !page.has_more);
}
