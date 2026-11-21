extern crate alloc;

#[path = "../src/mux_input.rs"]
mod mux_input;

use mux_input::DetachInput;

#[test]
fn ordinary_binary_and_utf8_input_is_preserved() {
    let mut bytes: Vec<u8> = (0..=255).filter(|&byte| byte != 0x1d).collect();
    bytes.extend_from_slice("命令 🦀 e\u{301}\n".as_bytes());
    let mut input = DetachInput::new();
    assert_eq!(input.feed(&bytes), (bytes, false));
    assert!(!input.pending());
}

#[test]
fn legacy_detach_returns_the_complete_prefix() {
    let mut input = DetachInput::new();
    assert_eq!(
        input.feed(b"printf READY\n\x1dignored"),
        (b"printf READY\n".to_vec(), true)
    );
    assert!(!input.pending());
    assert!(input.flush().is_empty());

    assert_eq!(input.feed(b"\x1b["), (Vec::new(), false));
    assert_eq!(input.feed(b"\x1dignored"), (b"\x1b[".to_vec(), true));
}

#[test]
fn kitty_press_repeat_and_lock_modifiers_detach() {
    for sequence in [
        "\x1b[93;5u",
        "\x1b[93;5:1u",
        "\x1b[93;5:2u",
        "\x1b[93;69u",
        "\x1b[93;133:1u",
        "\x1b[93;197:2u",
        "\x1b[93:125;5u",
        "\x1b[93::93;5u",
        "\x1b[93:125:93;5:1u",
        "\x1b[93;5:1;93u",
        "\x1b[093;005u",
    ] {
        let bytes = format!("before{sequence}after");
        let mut input = DetachInput::new();
        assert_eq!(
            input.feed(bytes.as_bytes()),
            (b"before".to_vec(), true),
            "{sequence:?}"
        );
        assert!(!input.pending());
    }
}

#[test]
fn kitty_detach_survives_every_read_boundary() {
    for sequence in [b"\x1b[93;5u".as_slice(), b"\x1b[93::93;197:2u".as_slice()] {
        for split in 0..=sequence.len() {
            let mut input = DetachInput::new();
            let (prefix, detached) = input.feed(&sequence[..split]);
            assert!(prefix.is_empty());
            if split == sequence.len() {
                assert!(detached);
            } else {
                assert!(!detached);
                assert_eq!(input.feed(&sequence[split..]), (Vec::new(), true));
            }
            assert!(!input.pending());
        }
        let mut input = DetachInput::new();
        for (index, byte) in sequence.iter().enumerate() {
            assert_eq!(
                input.feed(&[*byte]),
                (Vec::new(), index + 1 == sequence.len())
            );
        }
    }
}

#[test]
fn other_keys_modifiers_and_release_events_are_literal() {
    for sequence in [
        "\x1b[93;5:3u",
        "\x1b[93;197:3u",
        "\x1b[94;5u",
        "\x1b[93u",
        "\x1b[93;1u",
        "\x1b[93;6u",
        "\x1b[93;13u",
        "\x1b[93;21u",
        "\x1b[93;37u",
        "\x1b[93;261u",
        "\x1b[125:93;5u",
    ] {
        let mut input = DetachInput::new();
        assert_eq!(
            input.feed(sequence.as_bytes()),
            (sequence.as_bytes().to_vec(), false),
            "{sequence:?}"
        );
        assert!(!input.pending());
    }
}

#[test]
fn normal_escape_sequences_remain_byte_exact() {
    let bytes = b"\x1ba\x1b[A\x1b[1;5D\x1bOP\x1b[200~paste\x1b[201~\x1b[I\x1b[O\x1b[<0;1;2M";
    let mut input = DetachInput::new();
    assert_eq!(input.feed(bytes), (bytes.to_vec(), false));
    assert!(!input.pending());
}

#[test]
fn flush_releases_a_standalone_escape_or_unfinished_csi() {
    for bytes in [
        b"\x1b".as_slice(),
        b"\x1b[".as_slice(),
        b"\x1b[93;".as_slice(),
    ] {
        let mut input = DetachInput::new();
        assert_eq!(input.feed(bytes), (Vec::new(), false));
        assert!(input.pending());
        assert_eq!(input.flush(), bytes);
        assert!(!input.pending());
        assert!(input.flush().is_empty());
        assert_eq!(input.feed(b"5u"), (b"5u".to_vec(), false));
    }
}

#[test]
fn malformed_kitty_events_are_preserved() {
    for sequence in [
        "\x1b[;5u",
        "\x1b[93;0u",
        "\x1b[93;5:0u",
        "\x1b[93;5:4u",
        "\x1b[93;5:u",
        "\x1b[93;5:1:2u",
        "\x1b[93:;5u",
        "\x1b[93::;5u",
        "\x1b[93:125:93:1;5u",
        "\x1b[93;5;u",
        "\x1b[93;5;93;93u",
        "\x1b[93;5;55296u",
        "\x1b[93;42949672960u",
        "\x1b[93;-5u",
    ] {
        let mut input = DetachInput::new();
        assert_eq!(
            input.feed(sequence.as_bytes()),
            (sequence.as_bytes().to_vec(), false),
            "{sequence:?}"
        );
        assert!(!input.pending());
    }
}

#[test]
fn a_new_escape_can_start_after_an_unfinished_sequence() {
    let mut input = DetachInput::new();
    assert_eq!(input.feed(b"prefix\x1b[93;"), (b"prefix".to_vec(), false));
    assert_eq!(input.feed(b"\x1b[93;5u"), (b"\x1b[93;".to_vec(), true));

    assert_eq!(input.feed(b"\x1b\x1b[93;5u"), (b"\x1b".to_vec(), true));
}

#[test]
fn escape_storage_is_bounded_and_oversized_events_stay_literal() {
    for length in [126, 127, 128, 1024] {
        let bytes = format!("\x1b[{}93;5u", "0".repeat(length)).into_bytes();
        for chunk_size in [1, 7, 33, 128] {
            let mut input = DetachInput::new();
            let mut delivered = Vec::new();
            let mut received = 0;
            for chunk in bytes.chunks(chunk_size) {
                let (output, detached) = input.feed(chunk);
                assert!(!detached);
                received += chunk.len();
                delivered.extend_from_slice(&output);
                assert!(received - delivered.len() <= 128);
            }
            delivered.extend(input.flush());
            assert_eq!(delivered, bytes);
        }
    }
}

#[test]
fn ordinary_sequences_survive_one_byte_reads_without_reordering() {
    let bytes = b"before\x1b[93;5:3u\xff\0\x1b[1;5Dafter\x1b";
    let mut input = DetachInput::new();
    let mut delivered = Vec::new();
    for byte in bytes {
        let (output, detached) = input.feed(&[*byte]);
        assert!(!detached);
        delivered.extend(output);
    }
    assert!(input.pending());
    delivered.extend(input.flush());
    assert_eq!(delivered, bytes);
}
