//! Shared session state for the native ztty frontends.
//!
//! The core uses alloc and the platform PTY ABI; it does not require Rust std.
//! Window resources, event loops and per-view interaction stay in each frontend.
#![no_std]

extern crate alloc;

pub mod actions;
mod graphics_io;
pub mod input_queue;
pub mod json;
pub mod layout;
pub mod native_resize;
pub mod native_ui;
pub mod output_tail;
pub mod scroll_prompt;
pub mod selection;
pub mod session_control;
pub mod session_core;
pub mod session_identity;
pub mod workspace_state;

pub mod text_blink;
pub mod text_plugins;

#[cfg(target_os = "linux")]
pub mod gpu;
