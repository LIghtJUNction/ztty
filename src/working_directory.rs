//! Temporarily enter a session directory without requiring read access to cwd.
use crate::{broker::Fd, os};
use alloc::{format, string::String};
use core::ffi::CStr;

// Linux asm-generic bits shared by x86_64 and aarch64. O_DIRECTORY differs
// between those ABIs; O_PATH needs no read permission and supports fchdir.
const O_CLOEXEC: i32 = 1 << 19;
const O_PATH: i32 = 1 << 21;

pub struct WorkingDirectory(Fd);
impl WorkingDirectory {
    pub fn enter(path: &CStr) -> Result<Self, String> {
        let fd = unsafe { os::open(c".".as_ptr(), O_PATH | O_CLOEXEC) };
        if fd < 0 {
            let error = os::errno();
            return Err(format!(
                "cannot save session working directory (OS error {error})"
            ));
        }
        let saved = Fd(fd);
        if unsafe { os::chdir(path.as_ptr()) } < 0 {
            let error = os::errno();
            return Err(format!(
                "cannot enter session working directory (OS error {error})"
            ));
        }
        Ok(Self(saved))
    }
}
impl Drop for WorkingDirectory {
    fn drop(&mut self) {
        unsafe {
            os::fchdir(self.0.0);
        }
    }
}
