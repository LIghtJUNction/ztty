extern crate alloc;

#[path = "../src/broker/encoding.rs"]
mod encoding;

use encoding::{hex, unhex, unhex_bytes};

#[test]
fn lowercase_hex_matches_wire_format() {
    let bytes: [u8; 4097] = core::array::from_fn(|index| index as u8);
    for length in [
        0, 1, 15, 16, 17, 20, 31, 32, 33, 63, 64, 65, 255, 256, 257, 4096, 4097,
    ] {
        let input = &bytes[..length];
        let expected: String = input.iter().map(|byte| format!("{byte:02x}")).collect();
        let encoded = hex(input);
        assert_eq!(encoded, expected, "input length {length}");
        assert_eq!(unhex_bytes(&encoded).unwrap(), input);
    }
    assert_eq!(hex(b"\x00\x0f\x10\x7f\x80\xff"), "000f107f80ff");
}

#[test]
fn process_metadata_retains_arbitrary_bytes_and_nul_rules() {
    for input in [b"".as_slice(), b"/tmp/\xff\xfe\n", b"KEY=\x80\x01\t"] {
        assert_eq!(unhex(&hex(input)).unwrap().as_bytes(), input);
    }
    assert_eq!(unhex_bytes("0061ff").unwrap(), b"\x00a\xff");
    assert_eq!(unhex("0061ff").unwrap_err(), "NUL in process metadata");
}

#[test]
fn decoding_keeps_lowercase_only_validation() {
    for text in ["0", "123", "FF", "aF", "g0", "0x00", "ff ", "é"] {
        assert_eq!(unhex_bytes(text).unwrap_err(), "invalid byte string");
        assert_eq!(unhex(text).unwrap_err(), "invalid byte string");
    }
}
