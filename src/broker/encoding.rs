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
    if !value.len().is_multiple_of(2) || value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err("invalid byte string".into());
    }
    let mut bytes = alloc::vec![0; value.len() / 2];
    zhex::decode(value.as_bytes(), &mut bytes).map_err(|_| String::from("invalid byte string"))?;
    Ok(bytes)
}
