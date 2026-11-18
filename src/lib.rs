//! Shared session state for the native ztty frontends.
//!
//! The core uses alloc and the platform PTY ABI; it does not require Rust std.
//! Window resources, event loops and per-view interaction stay in each frontend.
#![no_std]

extern crate alloc;

pub mod actions;
pub mod input_queue;
pub mod json;
pub mod layout;
pub mod output_tail;
pub mod session_control;
pub mod session_core;
pub mod workspace_state;
