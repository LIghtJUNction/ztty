#![cfg(target_os = "linux")]
#![allow(dead_code)]
extern crate alloc;

use ztty::{json, session_identity};
#[path = "../src/mcp.rs"]
mod mcp;
#[path = "../src/os.rs"]
mod os;

// Unit tests validate the exact broker request without touching a user's daemon.
mod broker {
    // Transport integration tests use the real binary. Any accidental transport
    // execution in these validation-only unit tests must fail immediately.
    pub struct PendingRequest;
    impl PendingRequest {
        pub fn new(_: &crate::json::Value) -> Result<Self, String> {
            panic!("unexpected broker I/O in protocol unit test")
        }
        pub fn poll_fd(&self) -> Option<crate::os::PollFd> {
            unreachable!()
        }
        pub fn next_deadline(&self) -> Option<u64> {
            unreachable!()
        }
        pub fn advance(&mut self) -> Option<Result<crate::json::Value, String>> {
            unreachable!()
        }
        pub fn buffered_bytes(&self) -> usize {
            unreachable!()
        }
    }
    pub fn request(value: &crate::json::Value) -> Result<crate::json::Value, String> {
        Ok(value.clone())
    }
}
