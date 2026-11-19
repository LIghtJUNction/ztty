//! The only filesystem boundary of the protocol: a host-supplied reader.
//!
//! Names and byte ranges are untrusted. The host must validate a regular file
//! or an actual shared memory object, bound every allocation/read, and apply
//! its own unlink policy. A single opaque error prevents filesystem probing
//! through differences in protocol replies. This crate performs no I/O.
use alloc::vec::Vec;

use crate::{Graphics, decode, protocol::Command};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Medium {
    File,
    TemporaryFile,
    SharedMemory,
}

#[derive(Clone, Copy, Debug)]
pub struct ExternalRead<'a> {
    pub medium: Medium,
    pub name: &'a [u8],
    pub offset: u32,
    /// Zero means read through EOF, subject to `limit`.
    pub size: u32,
    pub limit: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct ReadError;

pub type ExternalReader = fn(ExternalRead<'_>) -> Result<Vec<u8>, ReadError>;

impl Graphics {
    /// Install the platform's validated file/shared-memory implementation.
    /// Without a reader the scene neither advertises nor performs host I/O.
    pub fn set_external_reader(&mut self, reader: Option<ExternalReader>) {
        self.external_reader = reader;
    }

    pub(crate) fn external(
        &self,
        command: &Command,
        payload: &[u8],
    ) -> Result<Vec<u8>, &'static str> {
        let medium = match command.transport {
            b'f' => Medium::File,
            b't' => Medium::TemporaryFile,
            b's' => Medium::SharedMemory,
            _ => return Err("ENOTSUP:unknown transmission medium"),
        };
        let reader = self
            .external_reader
            .ok_or("ENOTSUP:host image I/O unavailable")?;
        if command.more {
            return Err("EINVAL:external names cannot be chunked");
        }
        let mut name = Vec::new();
        decode::base64_append(payload, &mut name, 4096)?;
        if name.is_empty()
            || name.contains(&0)
            || command.size as usize > self.limits.max_transfer_bytes
        {
            return Err("EBADF:Failed to read image file");
        }
        let bytes = reader(ExternalRead {
            medium,
            name: &name,
            offset: command.offset,
            size: command.size,
            limit: self.limits.max_transfer_bytes,
        })
        .map_err(|_| "EBADF:Failed to read image file")?;
        if bytes.len() > self.limits.max_transfer_bytes
            || bytes.capacity() > self.limits.max_transfer_bytes
            || (command.size != 0 && bytes.len() != command.size as usize)
        {
            return Err("EBADF:Failed to read image file");
        }
        Ok(bytes)
    }
}
