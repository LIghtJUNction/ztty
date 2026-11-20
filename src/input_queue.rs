//! Bounded input waiting to be accepted by the platform PTY.
use alloc::vec::Vec;

// Reuse ordinary input batches, but do not retain a large paste for the session.
const MAX_RETAINED_BYTES: usize = 64 * 1024;

pub struct InputQueue {
    bytes: Vec<u8>,
    offset: usize,
    limit: usize,
}

impl InputQueue {
    pub(crate) fn checkpoint(&self) -> (&[u8], usize) {
        (&self.bytes[self.offset..], self.limit)
    }

    pub(crate) fn restore_checkpoint(bytes: Vec<u8>, limit: usize) -> Self {
        Self {
            bytes,
            offset: 0,
            limit,
        }
    }

    pub fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            offset: 0,
            limit,
        }
    }

    /// Accept the whole write or leave the pending bytes unchanged.
    pub fn push(&mut self, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return true;
        }
        if self.pending_len().saturating_add(bytes.len()) > self.limit {
            return false;
        }
        if self.offset != 0 {
            self.bytes.drain(..self.offset);
            self.offset = 0;
        }
        self.bytes.extend_from_slice(bytes);
        true
    }

    pub fn pending_len(&self) -> usize {
        self.bytes.len() - self.offset
    }

    pub fn pending(&self) -> bool {
        self.offset < self.bytes.len()
    }

    /// This cannot revoke bytes already accepted by Pty::write, including
    /// bytes buffered by the Windows PTY writer thread.
    pub fn clear(&mut self) {
        if self.bytes.capacity() > MAX_RETAINED_BYTES {
            self.bytes = Vec::new();
        } else {
            self.bytes.clear();
        }
        self.offset = 0;
    }

    /// Preserve the unwritten suffix on backpressure or a fatal write error.
    pub fn flush(&mut self, pty: &zpty::Pty) -> Result<(), zpty::Error> {
        while self.pending() {
            match pty.write(&self.bytes[self.offset..]) {
                Ok(0) => return Ok(()),
                Ok(n) => self.offset += n,
                Err(error) if error.is_would_block() => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        self.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::InputQueue;

    #[test]
    fn clearing_large_paste_releases_retained_storage() {
        let mut queue = InputQueue::new(8 * 1024 * 1024);
        let input = alloc::vec![b'x'; 8 * 1024 * 1024];
        assert!(queue.push(&input));
        let before = queue.bytes.capacity();
        queue.clear();
        let after = queue.bytes.capacity();
        std::println!("input queue bytes: before={before}, after={after}");
        assert!(!queue.pending());
        assert!(
            after <= 64 * 1024,
            "large paste storage remains allocated: {after}"
        );
        assert!(queue.push(b"next input"));
        assert_eq!(queue.pending_len(), 10);
    }

    #[test]
    fn clearing_small_input_reuses_retained_storage() {
        let mut queue = InputQueue::new(8 * 1024 * 1024);
        assert!(queue.push(&[b'x'; 4096]));
        let before = queue.bytes.capacity();
        queue.clear();
        assert_eq!(queue.bytes.capacity(), before);
        assert!(queue.push(b"next input"));
        assert_eq!(queue.bytes.capacity(), before);
    }
}
