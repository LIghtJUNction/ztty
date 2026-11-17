//! Native pseudo-terminal ownership without the Rust standard library.
//! Linux and macOS use PTYs; Windows uses ConPTY.
#![no_std]
extern crate alloc;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("zpty supports Linux, macOS, and Windows");
