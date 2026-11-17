#![no_std]
#![no_main]
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

extern crate alloc;

mod appearance;
#[cfg(target_os = "linux")]
mod os;
#[cfg(target_os = "windows")]
#[path = "os_windows.rs"]
mod os;
#[cfg(target_os = "macos")]
#[path = "os_macos.rs"]
mod os;
#[cfg(target_os = "linux")]
#[path = "platform/linux.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "platform/windows.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "platform/macos.rs"]
mod platform;
#[cfg(target_os = "linux")]
mod x11;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("ztty supports Linux, macOS, and Windows");

use alloc::ffi::CString;
use alloc::string::String;
use alloc::vec::Vec;
use appearance::Appearance;
use core::ffi::{CStr, c_char, c_int};

#[global_allocator]
static ALLOCATOR: os::Allocator = os::Allocator;

// Stable Rust's precompiled alloc contains unwind cleanup references even for
// panic=abort consumers. Use the platform unwinder for those ABI references;
// this application's Rust panic/personality boundaries always terminate.
#[cfg(target_os = "linux")]
#[link(name = "gcc_s")]
unsafe extern "C" {}

#[unsafe(no_mangle)]
extern "C" fn rust_eh_personality(
    _: i32,
    _: i32,
    _: u64,
    _: *mut core::ffi::c_void,
    _: *mut core::ffi::c_void,
) -> i32 {
    unsafe { os::_exit(101) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    os::print(2, b"ztty: unrecoverable internal error\n");
    unsafe { os::_exit(101) }
}

const HELP: &str = "ztty — a small no_std Rust terminal\n\n\
Usage: ztty [OPTIONS] [-e COMMAND [ARG...]]\n\n\
  --config PATH        Load an appearance file\n\
  --font NAME          Fontconfig name (default: monospace:size=11)\n\
  --geometry COLSxROWS Initial character dimensions (default: 120x32)\n\
  --scrollback LINES   Maximum retained lines (default: 2000; 0 disables)\n\
  --help               Show this help\n\
  --version            Show the version\n\n\
Ctrl+Shift+C copies a selection; Ctrl+Shift+V pastes.\n\
Shift+PageUp/PageDown and the wheel browse scrollback.\n\
Shift+drag selects while applications use the mouse; Ctrl+click opens links.\n\
Native Linux X11/XWayland, macOS AppKit, and Windows Win32 backends.\n";

struct Config {
    appearance: Appearance,
    cols: usize,
    rows: usize,
    history: usize,
    command: Vec<CString>,
}

fn number(bytes: &[u8]) -> Option<usize> {
    core::str::from_utf8(bytes).ok()?.parse().ok()
}

fn config(args: &[&CStr]) -> Result<Option<Config>, String> {
    let mut config_path = None;
    let mut font = None;
    let mut cfg = Config {
        appearance: Appearance::default(),
        cols: 120,
        rows: 32,
        history: 2000,
        command: Vec::new(),
    };
    let mut i = 1;
    while i < args.len() {
        match args[i].to_bytes() {
            b"--help" | b"-h" => {
                os::print(1, HELP.as_bytes());
                return Ok(None);
            }
            b"--version" | b"-V" => {
                os::print(
                    1,
                    concat!("ztty ", env!("CARGO_PKG_VERSION"), "\n").as_bytes(),
                );
                return Ok(None);
            }
            b"-e" | b"--" => {
                if i + 1 == args.len() {
                    return Err("-e needs a command".into());
                }
                cfg.command
                    .extend(args[i + 1..].iter().map(|s| (*s).into()));
                break;
            }
            b"--config" => {
                i += 1;
                config_path = Some(*args.get(i).ok_or("--config needs a path")?);
            }
            b"--font" => {
                i += 1;
                font = Some(
                    args.get(i)
                        .ok_or("--font needs a value")?
                        .to_str()
                        .map_err(|_| "font name is not UTF-8")?,
                );
            }
            b"--geometry" => {
                i += 1;
                let text = args.get(i).ok_or("--geometry needs COLSxROWS")?.to_bytes();
                let split = text
                    .iter()
                    .position(|&c| c == b'x')
                    .ok_or("geometry must be COLSxROWS")?;
                cfg.cols = number(&text[..split]).ok_or("invalid column count")?;
                cfg.rows = number(&text[split + 1..]).ok_or("invalid row count")?;
                if !(2..=512).contains(&cfg.cols) || !(2..=256).contains(&cfg.rows) {
                    return Err("geometry must be 2..512 columns by 2..256 rows".into());
                }
            }
            b"--scrollback" => {
                i += 1;
                cfg.history = number(
                    args.get(i)
                        .ok_or("--scrollback needs a line count")?
                        .to_bytes(),
                )
                .ok_or("invalid scrollback line count")?;
                if cfg.history > 100_000 {
                    return Err("scrollback is limited to 100000 lines".into());
                }
            }
            _ => return Err("unknown argument; run ztty --help".into()),
        }
        i += 1;
    }
    if cfg.command.is_empty() {
        #[cfg(unix)]
        {
            let shell = os::env(c"SHELL")
                .filter(|s| !s.is_empty())
                .unwrap_or(b"/bin/sh");
            cfg.command
                .push(CString::new(shell).map_err(|_| "invalid shell path")?);
        }
        #[cfg(windows)]
        cfg.command.push(os::default_shell());
    }
    cfg.appearance = Appearance::load(config_path)?;
    if let Some(font) = font {
        cfg.appearance.set_font(font)?;
    }
    Ok(Some(cfg))
}

/// C runtime entry point. Rust's standard library and its runtime are absent.
///
/// # Safety
/// `argv` must point to `argc` valid, NUL-terminated argument strings, as supplied
/// by the platform C runtime at process startup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn main(argc: c_int, argv: *const *const c_char) -> c_int {
    #[cfg(not(windows))]
    let args: Vec<&CStr> = unsafe { core::slice::from_raw_parts(argv, argc.max(0) as usize) }
        .iter()
        .map(|&ptr| unsafe { CStr::from_ptr(ptr) })
        .collect();
    #[cfg(windows)]
    let owned_args = os::arguments();
    #[cfg(windows)]
    let args: Vec<&CStr> = owned_args.iter().map(|s| s.as_c_str()).collect();
    #[cfg(windows)]
    let _ = (argc, argv);
    let result = config(&args).and_then(|cfg| {
        if let Some(cfg) = cfg {
            platform::run(cfg).map_err(String::from)
        } else {
            Ok(0)
        }
    });
    match result {
        Ok(status) => status,
        Err(message) => {
            os::print(2, b"ztty: ");
            os::print(2, message.as_bytes());
            os::print(2, b"\n");
            1
        }
    }
}
