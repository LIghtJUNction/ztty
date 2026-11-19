#![no_std]

//! Shared interface for independently built ztty text extensions.
//!
//! Native implementations are trusted code. They must not retain input/output
//! references after a call. ztty validates the returned length and UTF-8 before
//! committing a result. Change the interface version when changing signatures.

/// The host accepts at most this many UTF-8 input or output bytes per action.
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;

/// Transform a selected string or the currently visible terminal text.
#[zplugin::interface(id = "dev.project-z.ztty.text", version = 1)]
pub trait TextAction {
    /// Write the result into `output` and return its byte length.
    /// Return `BufferTooSmall` without exceeding the supplied buffer.
    fn transform(&mut self, input: &str, output: &mut [u8]) -> Result<u64, zplugin::Error>;
}
