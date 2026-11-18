//! Bounded input waiting to be accepted by the platform PTY.
use alloc::vec::Vec;

pub struct InputQueue {
    bytes: Vec<u8>,
    offset: usize,
    limit: usize,
}

impl InputQueue {
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
        self.bytes.clear();
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
