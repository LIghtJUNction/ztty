//! Bounded raw PTY output with independent, absolute byte cursors.
use alloc::{collections::VecDeque, string::String, vec::Vec};

const LOCAL_LIMIT: usize = 64 * 1024;
const PERSISTENT_LIMIT: usize = 256 * 1024;
const DEFAULT_READ: u64 = 16384;
const MAX_READ: u64 = 65536;

#[derive(Default)]
pub struct OutputTail {
    bytes: VecDeque<u8>,
    end: u64,
}

pub struct OutputChunk {
    pub bytes: Vec<u8>,
    pub next_cursor: u64,
    pub first_cursor: u64,
    pub truncated: bool,
    pub has_more: bool,
}

impl OutputChunk {
    /// Lossless standard Base64 for this page's original PTY bytes. Encoding is
    /// performed only on an explicit read, never while retaining PTY output.
    pub fn output_base64(&self) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut encoded = String::with_capacity(self.bytes.len().div_ceil(3) * 4);
        for chunk in self.bytes.chunks(3) {
            let word = ((chunk[0] as u32) << 16)
                | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
                | chunk.get(2).copied().unwrap_or(0) as u32;
            encoded.push(ALPHABET[(word >> 18) as usize] as char);
            encoded.push(ALPHABET[((word >> 12) & 63) as usize] as char);
            encoded.push(if chunk.len() > 1 {
                ALPHABET[((word >> 6) & 63) as usize] as char
            } else {
                '='
            });
            encoded.push(if chunk.len() > 2 {
                ALPHABET[(word & 63) as usize] as char
            } else {
                '='
            });
        }
        encoded
    }
}

impl OutputTail {
    pub(crate) fn checkpoint(&self) -> (Vec<u8>, u64) {
        (self.bytes.iter().copied().collect(), self.end)
    }

    pub(crate) fn restore_checkpoint(bytes: Vec<u8>, end: u64) -> Self {
        Self {
            bytes: bytes.into(),
            end,
        }
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// Changing retention applies to currently retained and newly appended
    /// bytes; increasing the limit cannot recover data already discarded.
    pub fn append(&mut self, bytes: &[u8], persistent: bool) {
        let limit = if persistent {
            PERSISTENT_LIMIT
        } else {
            LOCAL_LIMIT
        };
        self.end = self.end.saturating_add(bytes.len() as u64);
        if bytes.len() >= limit {
            self.bytes.clear();
            self.bytes.extend(&bytes[bytes.len() - limit..]);
        } else {
            let remove = self
                .bytes
                .len()
                .saturating_add(bytes.len())
                .saturating_sub(limit);
            self.bytes.drain(..remove);
            self.bytes.extend(bytes);
        }
    }

    /// A read never advances shared state. Callers retain their own cursors.
    /// The byte limit may extend by up to three bytes to finish a UTF-8 codepoint.
    pub fn read(
        &self,
        cursor: u64,
        max_bytes: Option<u64>,
        eof: bool,
    ) -> Result<OutputChunk, &'static str> {
        let max = max_bytes.unwrap_or(DEFAULT_READ);
        if max == 0 || max > MAX_READ {
            return Err("max_bytes must be 1..65536");
        }
        if cursor > self.end {
            return Err("cursor is beyond the available output");
        }
        let max = max as usize;
        let first = self.end - self.bytes.len() as u64;
        let mut start = cursor.max(first);
        if cursor < first {
            while start < self.end && self.bytes[(start - first) as usize] & 0xc0 == 0x80 {
                start += 1;
            }
        }
        let mut bytes: Vec<u8> = self
            .bytes
            .iter()
            .skip((start - first) as usize)
            .take(max + 3)
            .copied()
            .collect();
        if !eof {
            // Validate the lookahead before cutting a page: in E2 | 58, the
            // already available 58 proves E2 is malformed, not a pending suffix.
            // An earlier malformed sequence does not make a later incomplete
            // suffix safe to consume. Skip complete errors and keep scanning;
            // every byte is visited at most once.
            let mut checked = 0;
            while let Err(error) = core::str::from_utf8(&bytes[checked..]) {
                checked += error.valid_up_to();
                if let Some(invalid) = error.error_len() {
                    checked += invalid;
                } else {
                    bytes.truncate(checked);
                    break;
                }
            }
        }
        if bytes.len() > max {
            let mut end = max;
            while end < bytes.len() && bytes[end] & 0xc0 == 0x80 {
                end += 1;
            }
            bytes.truncate(end);
        }
        let next = start + bytes.len() as u64;
        Ok(OutputChunk {
            bytes,
            next_cursor: next,
            first_cursor: first,
            truncated: cursor < first,
            has_more: next < self.end,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn storage_and_read_allocations_remain_bounded_through_wraps_and_limit_changes() {
        let mut tail = OutputTail::new();
        assert_eq!(tail.bytes.capacity(), 0);
        let data = vec![b'x'; PERSISTENT_LIMIT + 19];
        for round in 0..64 {
            let persistent = round % 3 == 0;
            let limit = if persistent {
                PERSISTENT_LIMIT
            } else {
                LOCAL_LIMIT
            };
            for size in [1, 4099, 16385, LOCAL_LIMIT - 1, PERSISTENT_LIMIT + 19] {
                tail.append(&data[..size], persistent);
                assert!(tail.bytes.len() <= limit);
                // VecDeque grows geometrically; its reserved storage stays
                // below twice the largest retention limit even after shrinking.
                assert!(tail.bytes.capacity() <= PERSISTENT_LIMIT * 2);
                let chunk = tail.read(0, Some(MAX_READ), false).unwrap();
                assert!(chunk.bytes.len() <= MAX_READ as usize + 3);
                assert!(chunk.bytes.capacity() <= MAX_READ as usize + 3);
            }
        }
    }
}
