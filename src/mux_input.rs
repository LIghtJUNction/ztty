//! Preserve terminal input while recognizing the mux's Ctrl+] detach key.
use alloc::vec::Vec;

const ESCAPE_LIMIT: usize = 128;

/// A bounded streaming recognizer for legacy GS and Kitty Ctrl+] events.
///
/// Only an unfinished ESC/CSI sequence is retained. The caller should flush it
/// after a short timeout so a standalone Escape key is delivered promptly.
#[derive(Default)]
pub struct DetachInput {
    escape: Vec<u8>,
}

impl DetachInput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return input preceding the first detach key and whether it was found.
    /// Bytes after that key are discarded because the view is being detached.
    /// A Kitty key-release event is ordinary input; press and repeat can detach.
    pub fn feed(&mut self, bytes: &[u8]) -> (Vec<u8>, bool) {
        let mut output = Vec::with_capacity(bytes.len());
        for &byte in bytes {
            if byte == 0x1d {
                output.append(&mut self.escape);
                return (output, true);
            }
            let continuation = match self.escape.len() {
                0 => false,
                1 => byte == b'[',
                _ => byte.is_ascii_digit() || matches!(byte, b':' | b';' | b'u'),
            };
            if continuation && self.escape.len() < ESCAPE_LIMIT {
                self.escape.push(byte);
                if byte == b'u' {
                    if kitty_detach(&self.escape) {
                        self.escape.clear();
                        return (output, true);
                    }
                    output.append(&mut self.escape);
                }
                continue;
            }
            // A malformed or oversized sequence is literal input. An Escape
            // that terminated it can also begin the next candidate sequence.
            output.append(&mut self.escape);
            if byte == 0x1b {
                self.escape.push(byte);
            } else {
                output.push(byte);
            }
        }
        (output, false)
    }

    pub fn pending(&self) -> bool {
        !self.escape.is_empty()
    }

    /// Deliver an unfinished escape literally and reset the recognizer.
    pub fn flush(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.escape)
    }
}

fn number(value: &str) -> Option<u32> {
    value.parse().ok()
}

fn scalar(value: &str) -> bool {
    number(value).and_then(char::from_u32).is_some()
}

fn detach_key(value: &str) -> bool {
    let mut keys = value.split(':');
    if keys.next().and_then(number) != Some(93) {
        return false;
    }
    if let Some(shifted) = keys.next() {
        let base = keys.next();
        // Kitty permits an omitted shifted key when the base-layout key is
        // present, for example 93::93. Other omitted key fields are malformed.
        if (shifted.is_empty() && base.is_none())
            || (!shifted.is_empty() && !scalar(shifted))
            || base.is_some_and(|key| !scalar(key))
        {
            return false;
        }
    }
    keys.next().is_none()
}

fn kitty_detach(sequence: &[u8]) -> bool {
    let Ok(parameters) = core::str::from_utf8(&sequence[2..sequence.len() - 1]) else {
        return false;
    };
    let mut fields = parameters.split(';');
    if !fields.next().is_some_and(detach_key) {
        return false;
    }
    let Some(modifiers) = fields.next() else {
        return false;
    };
    let mut modifiers = modifiers.split(':');
    let Some(bits) = modifiers
        .next()
        .and_then(number)
        .and_then(|n| n.checked_sub(1))
    else {
        return false;
    };
    // Modifier values are one plus the bitset. Caps/Num Lock do not change the
    // shortcut; Shift, Alt, Super, Hyper and Meta do.
    if bits & !(64 | 128) != 4 {
        return false;
    }
    let event = modifiers.next().map_or(Some(1), number);
    if !matches!(event, Some(1 | 2)) || modifiers.next().is_some() {
        return false;
    }
    if let Some(text) = fields.next()
        && !text.split(':').all(scalar)
    {
        return false;
    }
    fields.next().is_none()
}
