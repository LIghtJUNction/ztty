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
        let mode = z64::Mode::Standard;
        let length = z64::encoded_len(self.bytes.len(), mode).expect("Base64 length overflow");
        let mut encoded = alloc::vec![0; length];
        z64::encode(&self.bytes, &mut encoded, mode).expect("exact Base64 output capacity");
        String::from_utf8(encoded).expect("Base64 output is ASCII")
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

    fn encoded(bytes: &[u8]) -> String {
        OutputChunk {
            bytes: bytes.to_vec(),
            next_cursor: bytes.len() as u64,
            first_cursor: 0,
            truncated: false,
            has_more: false,
        }
        .output_base64()
    }

    #[test]
    fn base64_matches_standard_alphabet_padding_and_all_bytes() {
        for (input, expected) in [
            (b"".as_slice(), ""),
            (b"f".as_slice(), "Zg=="),
            (b"fo".as_slice(), "Zm8="),
            (b"foo".as_slice(), "Zm9v"),
            (b"foob".as_slice(), "Zm9vYg=="),
            (b"fooba".as_slice(), "Zm9vYmE="),
            (b"foobar".as_slice(), "Zm9vYmFy"),
            (b"\xfb\xff\xff".as_slice(), "+///"),
            (b"\x00\xff".as_slice(), "AP8="),
        ] {
            assert_eq!(encoded(input), expected);
        }
        let all_bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(
            encoded(&all_bytes),
            concat!(
                "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4v",
                "MDEyMzQ1Njc4OTo7PD0+P0BBQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWltcXV5f",
                "YGFiY2RlZmdoaWprbG1ub3BxcnN0dXZ3eHl6e3x9fn+AgYKDhIWGh4iJiouMjY6P",
                "kJGSk5SVlpeYmZqbnJ2en6ChoqOkpaanqKmqq6ytrq+wsbKztLW2t7i5uru8vb6/",
                "wMHCw8TFxsfIycrLzM3Oz9DR0tPU1dbX2Nna29zd3t/g4eLj5OXm5+jp6uvs7e7v",
                "8PHy8/T19vf4+fr7/P3+/w==",
            )
        );
    }

    #[test]
    fn base64_pages_keep_byte_cursors_and_read_limits() {
        let mut history: Vec<u8> = (0..PERSISTENT_LIMIT + 64)
            .map(|index| index as u8)
            .collect();
        let mut tail = OutputTail::new();
        tail.append(&history, true);
        let suffix = [0; 37];
        tail.append(&suffix, true);
        history.extend_from_slice(&suffix);
        let first = (history.len() - PERSISTENT_LIMIT) as u64;
        for max in [1, 2, 3, 7, 31, MAX_READ] {
            let chunk = tail.read(0, Some(max), true).unwrap();
            assert_eq!(chunk.first_cursor, first);
            assert!(chunk.truncated && chunk.has_more);
            assert_eq!(
                chunk.bytes,
                history[first as usize..chunk.next_cursor as usize]
            );
            assert!(chunk.bytes.len() <= max as usize + 3);
            assert!(chunk.bytes.capacity() <= max as usize + 3);
            let text = chunk.output_base64();
            assert_eq!(text.len(), chunk.bytes.len().div_ceil(3) * 4);
            let mut decoded = vec![0; chunk.bytes.len()];
            let written = z64::decode(text.as_bytes(), &mut decoded, z64::Mode::Standard).unwrap();
            assert_eq!(&decoded[..written], chunk.bytes);
            let next = tail.read(chunk.next_cursor, Some(4), true).unwrap();
            assert!(!next.truncated);
            assert_eq!(
                next.bytes,
                history[chunk.next_cursor as usize..next.next_cursor as usize]
            );
        }
        assert_eq!(tail.bytes.len(), PERSISTENT_LIMIT);
        assert!(tail.bytes.capacity() <= PERSISTENT_LIMIT * 2);
    }

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
