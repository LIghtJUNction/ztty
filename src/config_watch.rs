//! A bounded inotify source follows atomic file and directory replacement.
use crate::{broker::Fd, os};
use alloc::{ffi::CString, vec::Vec};
use core::ffi::{CStr, c_char, c_int};

#[link(name = "c")]
unsafe extern "C" {
    fn inotify_init1(flags: c_int) -> c_int;
    fn inotify_add_watch(fd: c_int, path: *const c_char, mask: u32) -> c_int;
    fn inotify_rm_watch(fd: c_int, descriptor: c_int) -> c_int;
}

struct Watch {
    descriptor: i32,
    path: CString,
    name: Vec<u8>,
    direct: bool,
}
pub struct Watcher {
    fd: Fd,
    watches: Vec<Watch>,
}

impl Watcher {
    pub fn new() -> Self {
        Self {
            fd: Fd(unsafe { inotify_init1(0x800 | 0x80000) }),
            watches: Vec::new(),
        }
    }
    pub fn fd(&self) -> i32 {
        self.fd.0
    }

    fn connect(&self, path: &CStr) -> Option<(i32, Vec<u8>, bool)> {
        let bytes = path.to_bytes();
        let mut split = bytes.iter().rposition(|&b| b == b'/')?;
        let direct_split = split;
        loop {
            let directory = CString::new(&bytes[..split.max(1)]).ok()?;
            let descriptor = unsafe {
                inotify_add_watch(
                    self.fd.0,
                    directory.as_ptr(),
                    0x8 | 0x80 | 0x100 | 0x200 | 0x400 | 0x800,
                )
            };
            if descriptor >= 0 {
                let rest = &bytes[split + 1..];
                let end = rest.iter().position(|&b| b == b'/').unwrap_or(rest.len());
                return Some((descriptor, rest[..end].to_vec(), split == direct_split));
            }
            if split == 0 {
                return None;
            }
            // Watch the nearest existing ancestor, even before the default
            // config directory exists, without timers or idle wakeups.
            split = bytes[..split].iter().rposition(|&b| b == b'/')?;
        }
    }
    fn release(&self, descriptor: i32) {
        if descriptor >= 0 && !self.watches.iter().any(|w| w.descriptor == descriptor) {
            unsafe {
                inotify_rm_watch(self.fd.0, descriptor);
            }
        }
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&CStr) -> bool) {
        for index in (0..self.watches.len()).rev() {
            if !keep(&self.watches[index].path) {
                let removed = self.watches.swap_remove(index);
                self.release(removed.descriptor);
            }
        }
    }
    pub fn watch(&mut self, path: &CStr) {
        if self.fd.0 < 0
            || self.watches.len() >= 128
            || self.watches.iter().any(|w| w.path.as_c_str() == path)
        {
            return;
        }
        if let Some((descriptor, name, direct)) = self.connect(path) {
            self.watches.push(Watch {
                descriptor,
                path: path.into(),
                name,
                direct,
            });
        }
    }
    pub fn changed(&mut self) -> Vec<CString> {
        let mut changed = Vec::new();
        if self.fd.0 < 0 {
            return changed;
        }
        let mut bytes = [0u8; 16384];
        let mut reconnect = Vec::new();
        // A busy writer cannot starve PTYs; unread events remain ready.
        for _ in 0..4 {
            let count = unsafe { os::read(self.fd.0, bytes.as_mut_ptr().cast(), bytes.len()) };
            if count <= 0 {
                break;
            }
            let mut offset = 0;
            while offset + 16 <= count as usize {
                let descriptor = i32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap());
                let mask = u32::from_ne_bytes(bytes[offset + 4..offset + 8].try_into().unwrap());
                let size = u32::from_ne_bytes(bytes[offset + 12..offset + 16].try_into().unwrap())
                    as usize;
                let end = offset.saturating_add(16).saturating_add(size);
                if end > count as usize {
                    break;
                }
                let name = &bytes[offset + 16..end];
                let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
                for (index, watch) in self.watches.iter().enumerate() {
                    let lost = mask & 0x4000 != 0
                        || watch.descriptor == descriptor && mask & (0x400 | 0x800 | 0x8000) != 0;
                    if lost || watch.descriptor == descriptor && watch.name == name {
                        if !changed.contains(&watch.path) {
                            changed.push(watch.path.clone());
                        }
                        if (lost || !watch.direct) && !reconnect.contains(&index) {
                            reconnect.push(index);
                        }
                    }
                }
                offset = end;
            }
        }
        let mut released = Vec::new();
        for index in reconnect {
            if let Some((descriptor, name, direct)) = self.connect(&self.watches[index].path) {
                let old = core::mem::replace(&mut self.watches[index].descriptor, descriptor);
                self.watches[index].name = name;
                self.watches[index].direct = direct;
                if old != descriptor && !released.contains(&old) {
                    released.push(old);
                }
            }
        }
        for descriptor in released {
            self.release(descriptor);
        }
        changed
    }
}
