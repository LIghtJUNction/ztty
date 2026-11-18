//! Windows system boundary. Unicode paths and arguments use Win32 UTF-16 APIs;
//! Rust allocation uses the process heap and never links the Rust std runtime.

use alloc::{ffi::CString, string::String, vec, vec::Vec};
use core::{
    alloc::{GlobalAlloc, Layout},
    ffi::{CStr, c_int, c_void},
    ptr,
};

type Handle = *mut c_void;

pub struct Allocator;
// The process heap is synchronized by Windows. Each allocation records its
// original pointer before an aligned Rust payload, so arbitrary Layout
// alignments are honored without relying on CRT allocator variants.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align().max(core::mem::size_of::<usize>());
        let Some(total) = layout
            .size()
            .max(1)
            .checked_add(align)
            .and_then(|n| n.checked_add(core::mem::size_of::<usize>()))
        else {
            return ptr::null_mut();
        };
        let base = unsafe { HeapAlloc(GetProcessHeap(), 0, total) }.cast::<u8>();
        if base.is_null() {
            return ptr::null_mut();
        }
        let address = (base as usize + core::mem::size_of::<usize>() + align - 1) & !(align - 1);
        let result = address as *mut u8;
        unsafe {
            result.cast::<usize>().sub(1).write(base as usize);
        }
        result
    }
    unsafe fn dealloc(&self, pointer: *mut u8, _: Layout) {
        let base = unsafe { pointer.cast::<usize>().sub(1).read() } as Handle;
        unsafe {
            HeapFree(GetProcessHeap(), 0, base);
        }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let Ok(next) = Layout::from_size_align(size.max(1), layout.align()) else {
            return ptr::null_mut();
        };
        let result = unsafe { self.alloc(next) };
        if !result.is_null() {
            unsafe {
                ptr::copy_nonoverlapping(pointer, result, layout.size().min(size));
                self.dealloc(pointer, layout);
            }
        }
        result
    }
}

pub fn milliseconds() -> u64 {
    unsafe { GetTickCount64() }
}

pub fn print(fd: i32, bytes: &[u8]) {
    let handle = unsafe {
        GetStdHandle(if fd == 2 {
            -12i32 as u32
        } else {
            -11i32 as u32
        })
    };
    if handle.is_null() || handle as isize == -1 {
        return;
    }
    let mut mode = 0;
    if unsafe { GetConsoleMode(handle, &mut mode) } != 0
        && let Ok(text) = core::str::from_utf8(bytes)
    {
        // Diagnostics are also called from the panic boundary. Convert in a
        // stack buffer so reporting an allocation failure cannot allocate again.
        let mut buffer = [0u16; 512];
        let mut length = 0;
        for scalar in text.chars() {
            let mut units = [0u16; 2];
            let units = scalar.encode_utf16(&mut units);
            if length + units.len() > buffer.len() {
                console_write(handle, &buffer[..length]);
                length = 0;
            }
            buffer[length..length + units.len()].copy_from_slice(units);
            length += units.len();
        }
        console_write(handle, &buffer[..length]);
        return;
    }
    let mut offset = 0;
    while offset < bytes.len() {
        let mut count = 0;
        if unsafe {
            WriteFile(
                handle,
                bytes[offset..].as_ptr().cast(),
                (bytes.len() - offset).min(u32::MAX as usize) as u32,
                &mut count,
                ptr::null_mut(),
            )
        } == 0
            || count == 0
        {
            break;
        }
        offset += count as usize;
    }
}

fn console_write(handle: Handle, mut text: &[u16]) {
    while !text.is_empty() {
        let mut written = 0;
        if unsafe {
            WriteConsoleW(
                handle,
                text.as_ptr().cast(),
                text.len() as u32,
                &mut written,
                ptr::null_mut(),
            )
        } == 0
            || written == 0
        {
            break;
        }
        text = &text[written as usize..];
    }
}

/// Parse the actual Unicode command line instead of the C runtime's ACP argv.
pub fn arguments() -> Vec<CString> {
    unsafe {
        AttachConsole(u32::MAX);
    }
    let mut count = 0;
    let argv = unsafe { CommandLineToArgvW(GetCommandLineW(), &mut count) };
    if argv.is_null() {
        return Vec::new();
    }
    let mut result = Vec::new();
    for i in 0..count.max(0) as usize {
        let pointer = unsafe { *argv.add(i) };
        let mut length = 0;
        while unsafe { *pointer.add(length) } != 0 {
            length += 1;
        }
        let text =
            String::from_utf16_lossy(unsafe { core::slice::from_raw_parts(pointer, length) });
        if let Ok(argument) = CString::new(text) {
            result.push(argument);
        }
    }
    unsafe {
        LocalFree(argv.cast());
    }
    result
}

pub fn environment_value(name: &str) -> Option<Vec<u16>> {
    let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let size = unsafe { GetEnvironmentVariableW(name.as_ptr(), ptr::null_mut(), 0) };
    if size == 0 || size > 32768 {
        return None;
    }
    let mut value = vec![0u16; size as usize];
    let count = unsafe { GetEnvironmentVariableW(name.as_ptr(), value.as_mut_ptr(), size) };
    if count == 0 || count >= size {
        return None;
    }
    value.truncate(count as usize);
    Some(value)
}

pub fn default_config_path() -> Option<CString> {
    let mut directory = String::from_utf16_lossy(&environment_value("APPDATA")?);
    directory.push_str("\\ztty\\ztty.conf");
    CString::new(directory).ok()
}

pub fn default_shell() -> CString {
    let shell = environment_value("COMSPEC")
        .filter(|value| !value.is_empty())
        .map(|value| String::from_utf16_lossy(&value))
        .unwrap_or_else(|| String::from("cmd.exe"));
    CString::new(shell).unwrap_or_else(|_| c"cmd.exe".into())
}

pub fn read_config(path: &CStr) -> Result<Vec<u8>, i32> {
    let path: Vec<u16> = path
        .to_str()
        .map_err(|_| 87)?
        .encode_utf16()
        .chain([0])
        .collect();
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            0x80000000,
            1 | 2 | 4,
            ptr::null(),
            3,
            0x80,
            ptr::null_mut(),
        )
    };
    if handle as isize == -1 {
        let error = unsafe { GetLastError() } as i32;
        // The common appearance loader treats ENOENT/FILE_NOT_FOUND as an
        // absent optional config, including an absent parent directory.
        return Err(if error == 3 { 2 } else { error });
    }
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    let result = if unsafe { GetFileType(handle) } != 1 {
        Err(87)
    } else {
        loop {
            let mut count = 0;
            if unsafe {
                ReadFile(
                    handle,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &mut count,
                    ptr::null_mut(),
                )
            } == 0
            {
                break Err(unsafe { GetLastError() } as i32);
            }
            if count == 0 {
                break Ok(bytes);
            }
            if bytes.len() + count as usize > 65536 {
                break Err(223);
            }
            bytes.extend_from_slice(&buffer[..count as usize]);
        }
    };
    unsafe {
        CloseHandle(handle);
    }
    result
}

/// Prepare a Unicode child environment without mutating the host. Entries stay
/// sorted case-insensitively as required by CreateProcessW.
pub fn child_environment() -> Vec<u16> {
    let raw = unsafe { GetEnvironmentStringsW() };
    let mut entries: Vec<Vec<u16>> = Vec::new();
    if !raw.is_null() {
        let mut position = 0;
        while unsafe { *raw.add(position) } != 0 {
            let start = position;
            while unsafe { *raw.add(position) } != 0 {
                position += 1;
            }
            let entry = unsafe { core::slice::from_raw_parts(raw.add(start), position - start) };
            let name_end = entry
                .iter()
                .position(|&c| c == b'=' as u16)
                .unwrap_or(entry.len());
            let name = String::from_utf16_lossy(&entry[..name_end]);
            if !["TERM", "COLORTERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION"]
                .iter()
                .any(|value| name.eq_ignore_ascii_case(value))
            {
                entries.push(entry.to_vec());
            }
            position += 1;
        }
        unsafe {
            FreeEnvironmentStringsW(raw);
        }
    }
    for entry in [
        "TERM=xterm-256color",
        "COLORTERM=truecolor",
        "TERM_PROGRAM=ztty",
        concat!("TERM_PROGRAM_VERSION=", env!("CARGO_PKG_VERSION")),
    ] {
        entries.push(entry.encode_utf16().collect());
    }
    entries.sort_by(|a, b| {
        match unsafe {
            CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1)
        } {
            1 => core::cmp::Ordering::Less,
            3 => core::cmp::Ordering::Greater,
            _ => core::cmp::Ordering::Equal,
        }
    });
    let mut output = Vec::new();
    for entry in entries {
        output.extend(entry);
        output.push(0);
    }
    output.push(0);
    output
}

/// Called only at startup, before PTY or GUI helper threads exist.
pub fn enter_startup_directory(path: &str) -> Result<(), u32> {
    if path.contains('\0') {
        return Err(87); // ERROR_INVALID_PARAMETER
    }
    let path: Vec<u16> = path.encode_utf16().chain([0]).collect();
    if unsafe { SetCurrentDirectoryW(path.as_ptr()) } == 0 {
        Err(unsafe { GetLastError() })
    } else {
        Ok(())
    }
}

pub fn resolve(program: &CStr) -> Result<Vec<u16>, &'static str> {
    let name: Vec<u16> = program
        .to_str()
        .map_err(|_| "command must be UTF-8")?
        .encode_utf16()
        .chain([0])
        .collect();
    let extension = [b'.' as u16, b'e' as u16, b'x' as u16, b'e' as u16, 0];
    let mut buffer = vec![0u16; 32768];
    let count = unsafe {
        SearchPathW(
            ptr::null(),
            name.as_ptr(),
            extension.as_ptr(),
            buffer.len() as u32,
            buffer.as_mut_ptr(),
            ptr::null_mut(),
        )
    };
    if count == 0 || count as usize >= buffer.len() {
        return Err("command not found on PATH");
    }
    buffer.truncate(count as usize);
    Ok(buffer)
}

#[link(name = "msvcrt")]
unsafe extern "C" {
    pub fn _exit(status: c_int) -> !;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetProcessHeap() -> Handle;
    fn HeapAlloc(heap: Handle, flags: u32, bytes: usize) -> Handle;
    fn HeapFree(heap: Handle, flags: u32, memory: Handle) -> i32;
    fn GetTickCount64() -> u64;
    fn GetStdHandle(id: u32) -> Handle;
    fn GetConsoleMode(console: Handle, mode: *mut u32) -> i32;
    fn WriteConsoleW(
        console: Handle,
        bytes: *const c_void,
        count: u32,
        written: *mut u32,
        reserved: *mut c_void,
    ) -> i32;
    fn WriteFile(
        file: Handle,
        bytes: *const c_void,
        count: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn ReadFile(
        file: Handle,
        bytes: *mut c_void,
        count: u32,
        read: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn AttachConsole(process: u32) -> i32;
    fn GetCommandLineW() -> *const u16;
    fn LocalFree(memory: Handle) -> Handle;
    fn GetEnvironmentVariableW(name: *const u16, value: *mut u16, size: u32) -> u32;
    fn GetEnvironmentStringsW() -> *mut u16;
    fn FreeEnvironmentStringsW(environment: *mut u16) -> i32;
    fn CompareStringOrdinal(
        a: *const u16,
        a_len: i32,
        b: *const u16,
        b_len: i32,
        ignore_case: i32,
    ) -> i32;
    fn SearchPathW(
        path: *const u16,
        file: *const u16,
        extension: *const u16,
        size: u32,
        output: *mut u16,
        part: *mut *mut u16,
    ) -> u32;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        creation: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn SetCurrentDirectoryW(path: *const u16) -> i32;
    fn GetLastError() -> u32;
    fn GetFileType(file: Handle) -> u32;
    fn CloseHandle(handle: Handle) -> i32;
}
#[link(name = "shell32")]
unsafe extern "system" {
    fn CommandLineToArgvW(command: *const u16, count: *mut i32) -> *mut *mut u16;
}
