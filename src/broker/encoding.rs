//! Byte strings used by process metadata and broker replies.

use alloc::{ffi::CString, string::String, vec::Vec};

pub fn hex(bytes: &[u8]) -> String {
    let mut out = alloc::vec![0; zhex::encoded_len(bytes.len()).expect("hex length overflow")];
    zhex::encode_lower(bytes, &mut out).expect("exact hex output capacity");
    String::from_utf8(out).expect("hex output is ASCII")
}

pub fn unhex(value: &str) -> Result<CString, String> {
    CString::new(unhex_bytes(value)?).map_err(|_| "NUL in process metadata".into())
}

pub fn unhex_bytes(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("invalid byte string".into());
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().as_chunks::<2>().0 {
        let digit = |b: u8| -> Option<u8> {
            match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                _ => None,
            }
        };
        bytes.push(
            (digit(pair[0]).ok_or("invalid byte string")? << 4)
                | digit(pair[1]).ok_or("invalid byte string")?,
        );
    }
    Ok(bytes)
}
