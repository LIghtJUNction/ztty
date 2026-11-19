//! Validated host I/O for Kitty file and shared-memory transfers.
//! The protocol crate never opens paths. Keep the OS boundary here.
#[cfg(target_os = "linux")]
use alloc::vec::Vec;
use zterm::graphics::ExternalReader;
#[cfg(target_os = "linux")]
use zterm::graphics::{ExternalRead, ReadError};

#[cfg(target_os = "linux")]
mod linux;

pub fn reader() -> Option<ExternalReader> {
    #[cfg(target_os = "linux")]
    {
        Some(linux::read)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}
