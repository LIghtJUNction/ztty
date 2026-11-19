//! Bounded OSC 52 requests. The host supplies clipboard ownership and I/O.
use alloc::{collections::VecDeque, string::String, vec::Vec};

pub const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
pub const MAX_CLIPBOARD_REQUESTS: usize = 8;
pub(crate) const MAX_ENCODED: usize = MAX_CLIPBOARD_BYTES.div_ceil(3) * 4 + 17;

#[derive(Debug)]
pub struct ClipboardRequest {
    selectors: [u8; 16],
    count: u8,
    /// None requests text; Some("") clears the selected clipboard.
    pub text: Option<String>,
    pub bell_terminated: bool,
}
impl ClipboardRequest {
    pub fn selectors(&self) -> &[u8] {
        &self.selectors[..usize::from(self.count)]
    }
    /// A denied/unavailable query returns an empty payload, never partial text.
    /// Responses are sent through the host's bounded PTY response queue, not paste.
    pub fn reply(&self, selector: u8, text: Option<&[u8]>) -> Vec<u8> {
        let text = text
            .filter(|bytes| {
                bytes.len() <= MAX_CLIPBOARD_BYTES
                    && !bytes.contains(&0)
                    && core::str::from_utf8(bytes).is_ok()
            })
            .unwrap_or_default();
        let selector = if self.selectors().contains(&selector) {
            selector
        } else {
            self.selectors[0]
        };
        let mut bytes = Vec::with_capacity(9 + text.len().div_ceil(3) * 4);
        bytes.extend_from_slice(b"\x1b]52;");
        bytes.push(selector);
        bytes.push(b';');
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        for group in text.chunks(3) {
            let a = group[0];
            let b = group.get(1).copied().unwrap_or(0);
            let c = group.get(2).copied().unwrap_or(0);
            bytes.push(ALPHABET[usize::from(a >> 2)]);
            bytes.push(ALPHABET[usize::from((a & 3) << 4 | b >> 4)]);
            bytes.push(if group.len() > 1 {
                ALPHABET[usize::from((b & 15) << 2 | c >> 6)]
            } else {
                b'='
            });
            bytes.push(if group.len() > 2 {
                ALPHABET[usize::from(c & 63)]
            } else {
                b'='
            });
        }
        if self.bell_terminated {
            bytes.push(7);
        } else {
            bytes.extend_from_slice(b"\x1b\\");
        }
        bytes
    }
}
#[derive(Default)]
pub(crate) struct Queue {
    pending: VecDeque<ClipboardRequest>,
    bytes: usize,
    pub epoch: u64,
}
impl Queue {
    pub fn cancel(&mut self) {
        self.pending.clear();
        self.bytes = 0;
        self.epoch = self.epoch.wrapping_add(1);
    }
    pub fn pop(&mut self) -> Option<ClipboardRequest> {
        let request = self.pending.pop_front()?;
        self.bytes -= request.text.as_ref().map_or(0, String::len);
        Some(request)
    }
    pub fn receive(&mut self, body: &[u8], bell_terminated: bool) {
        if body.len() > MAX_ENCODED || self.pending.len() >= MAX_CLIPBOARD_REQUESTS {
            return;
        }
        let Some(split) = body.iter().position(|&b| b == b';') else {
            return;
        };
        let selected = &body[..split];
        if selected.len() > 16 {
            return;
        }
        let selected = if selected.is_empty() {
            b"s0".as_slice()
        } else {
            selected
        };
        let mut selectors = [0; 16];
        let mut count = 0;
        for &selector in selected {
            if !matches!(selector, b'c' | b'p' | b'q' | b's' | b'0'..=b'7') {
                return;
            }
            if !selectors[..count].contains(&selector) {
                selectors[count] = selector;
                count += 1;
            }
        }
        let encoded = &body[split + 1..];
        let text = if encoded == b"?" {
            None
        } else {
            // xterm specifies that a non-base64 payload clears the selection.
            let decoded = match decode(encoded) {
                Ok(value) => value,
                Err(DecodeError::Invalid) => Vec::new(),
                Err(DecodeError::Limit) => return,
            };
            if decoded.contains(&0) {
                return;
            }
            let Ok(text) = String::from_utf8(decoded) else {
                return;
            };
            if self.bytes.saturating_add(text.len()) > MAX_CLIPBOARD_BYTES {
                return;
            }
            Some(text)
        };
        self.bytes += text.as_ref().map_or(0, String::len);
        self.pending.push_back(ClipboardRequest {
            selectors,
            count: count as u8,
            text,
            bell_terminated,
        });
    }
}
pub(crate) struct Frame {
    bytes: Vec<u8>,
    discarded: bool,
}
impl Frame {
    pub fn new() -> Self {
        Self {
            bytes: Vec::new(),
            discarded: false,
        }
    }
    pub fn push(&mut self, bytes: &[u8]) {
        if self.discarded {
            return;
        }
        if self.bytes.len().saturating_add(bytes.len()) > MAX_ENCODED {
            self.abort();
            return;
        }
        self.bytes.extend_from_slice(bytes);
    }
    pub fn abort(&mut self) {
        self.discarded = true;
        self.bytes = Vec::new();
    }
    pub fn finish(self, queue: &mut Queue, bell: bool) {
        if !self.discarded {
            queue.receive(&self.bytes, bell);
        }
    }
}
enum DecodeError {
    Invalid,
    Limit,
}
fn digit(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}
fn decode(bytes: &[u8]) -> Result<Vec<u8>, DecodeError> {
    if bytes.len() > MAX_CLIPBOARD_BYTES.div_ceil(3) * 4 {
        return Err(DecodeError::Limit);
    }
    if !bytes.len().is_multiple_of(4) {
        return Err(DecodeError::Invalid);
    }
    let padding = usize::from(bytes.last() == Some(&b'='))
        + usize::from(bytes.len() > 1 && bytes.get(bytes.len() - 2) == Some(&b'='));
    let size = (bytes.len() / 4 * 3).saturating_sub(padding);
    if size > MAX_CLIPBOARD_BYTES {
        return Err(DecodeError::Limit);
    }
    let mut out = Vec::with_capacity(size);
    for (i, group) in bytes.as_chunks::<4>().0.iter().enumerate() {
        let a = digit(group[0]).ok_or(DecodeError::Invalid)?;
        let b = digit(group[1]).ok_or(DecodeError::Invalid)?;
        let last = i + 1 == bytes.len() / 4;
        let c = if group[2] == b'=' && last {
            0
        } else {
            digit(group[2]).ok_or(DecodeError::Invalid)?
        };
        let d = if group[3] == b'=' && last {
            0
        } else {
            digit(group[3]).ok_or(DecodeError::Invalid)?
        };
        if group[2] == b'=' && (group[3] != b'=' || b & 15 != 0)
            || group[3] == b'=' && group[2] != b'=' && c & 3 != 0
        {
            return Err(DecodeError::Invalid);
        }
        out.push(a << 2 | b >> 4);
        if group[2] != b'=' {
            out.push(b << 4 | c >> 2);
        }
        if group[3] != b'=' {
            out.push(c << 6 | d);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
