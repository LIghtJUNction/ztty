#![cfg(target_os = "linux")]
#![allow(dead_code)]
extern crate alloc;

use ztty::json;
#[path = "../src/mcp.rs"]
mod mcp;
#[path = "../src/os.rs"]
mod os;

// Unit tests validate the exact broker request without touching a user's daemon.
mod broker {
    pub fn request(value: &crate::json::Value) -> Result<crate::json::Value, String> {
        Ok(value.clone())
    }
}
