#![no_std]
#![no_main]
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

extern crate alloc;

mod appearance;
#[cfg(target_os = "linux")]
mod broker;
#[cfg(target_os = "linux")]
mod config_watch;
#[cfg(target_os = "linux")]
mod display;
#[cfg(target_os = "linux")]
mod mcp;
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
mod plugin_host;
#[cfg(target_os = "linux")]
mod repaint;
#[cfg(target_os = "linux")]
mod search;
#[cfg(target_os = "linux")]
mod session;
#[cfg(all(target_os = "linux", feature = "wayland"))]
#[path = "platform/wayland.rs"]
mod wayland;
#[cfg(target_os = "linux")]
mod working_directory;
#[cfg(target_os = "linux")]
mod x11;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("ztty supports Linux, macOS, and Windows");

use alloc::ffi::CString;
use alloc::string::String;
use alloc::vec::Vec;
use appearance::Appearance;
use core::ffi::{CStr, c_char, c_int};
#[cfg(target_os = "linux")]
use input::{Key, ModifierKey, Modifiers};
#[cfg(target_os = "linux")]
use input_queue::InputQueue;
#[cfg(target_os = "linux")]
use zterm::{Terminal, input};
#[cfg(target_os = "linux")]
use ztty::{
    actions, input_queue, json, layout, session_control, session_core, session_identity,
    workspace_state,
};

#[global_allocator]
static ALLOCATOR: os::Allocator = os::Allocator;

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

#[cfg(target_os = "linux")]
const HELP: &str = "ztty — shared no_std terminal sessions for Linux\n\n\
Usage: ztty [OPTIONS] [-e COMMAND [ARG...]]\n\
       ztty attach [NAME | ztty://ADDRESS]\n\
       ztty attach --observe NAME\n\
       ztty detach NAME | close NAME | rename NAME NEW_NAME | list [FILTER]\n\
       ztty action NAME ACTION | move NAME TARGET [tab|right|down]\n\
       ztty control NAME acquire|release [--takeover]\n\
       ztty commands NAME | command-output NAME COMMAND_ID\n\
       ztty shell-integration bash|zsh|fish|nu\n\
       ztty plugin check|run /absolute/library.so\n\
       ztty save-layout NAME > workspace.json\n\
       ztty restore-layout workspace.json\n\
       ztty mcp [--transport stdio|http] [--host HOST] [--port PORT]\n\n\
  --backend x11|wayland Select Linux host (default: x11; Wayland is experimental)\n\
  --name NAME          Name this local session\n\
  --observe            Open a standalone read-only view without requesting focus\n\
  --tab NAME           Open a new tab in NAME's window\n\
  --split-right NAME   Open a pane to the right of NAME\n\
  --split-down NAME    Open a pane below NAME\n\
  --cwd PATH           Start the command in PATH\n\
  --config PATH        Load an appearance file\n\
  --text-plugin NAME=PATH Register a trusted text extension (repeatable, Linux)\n\
  --font NAME          Fontconfig name (default: monospace:size=11)\n\
  --geometry COLSxROWS Initial character dimensions (default: 120x32)\n\
  --scrollback LINES   Maximum retained lines (default: 2000; 0 disables)\n\
  --help               Show this help\n\
  --version            Show the version\n\n\
Ctrl+Shift+C copies a selection; Ctrl+Shift+V pastes.\n\
Shift+PageUp/PageDown and the wheel browse scrollback.\n\
Shift+drag selects while applications use the mouse; Ctrl+click opens links.\n\
Multiple windows share one process, display connection and font cache.\n\
Ctrl+Shift+T adds a tab; E/O splits right/down; PageUp/PageDown switches tabs.\n\
Ctrl+Tab / Ctrl+Shift+Tab also switches to the next / previous tab.\n\
Ctrl+Shift+Left/Right focuses panes; Up/Down swaps; Enter maximizes/restores.\n\
Ctrl+Alt+Shift+Equal/Minus grows or shrinks the nearest split.\n\
Ctrl+Shift+D detaches; U moves a pane to a new window; G takes input control.\n\
Ctrl+Shift+S opens the session switcher (action: sessions).\n\
Type name/title/URI words; Up/Down selects; Enter focuses or explicitly attaches.\n\
Switching preserves the PTY and input owner; observing attaches stay read-only.\n\
Ctrl+Shift+F searches across soft-wrapped text; Alt+C toggles case, Enter/Shift+Enter navigates.\n\
Copy joins soft-wrapped rows and preserves explicit line breaks.\n\
Ctrl+Shift+P opens actions: filter with multiple words and see effective bindings.\n\
Registered plugins add Copy using NAME actions for a selection or visible text.\n\
Ctrl+U clears an overlay query; Esc closes it. Ctrl+Shift+R reloads configuration.\n\
Ctrl+Shift+K/J browses shell commands; Y copies the selected command output.\n\
Ctrl+Plus/Equal or Ctrl+Shift+Equal increases font size; Ctrl+Minus decreases it.\n\
Ctrl+Shift+Minus also decreases; Ctrl+0 or Ctrl+Shift+0 resets to the config font.\n\
Ctrl+keypad Add/Subtract/0 adjusts or resets; keypad 0 requires Num Lock on.\n\
Lock keys stay silent in legacy mode.\n\
With Num Lock off, keypad zero sends Insert in normal keypad mode.\n\
attach without a name lists all local and MCP sessions.\n\
MCP defaults to stdio; HTTP defaults to 127.0.0.1:8732/mcp.\n\
Runs on X11 and XWayland. Use ztty mcp --help for transport options.\n";

#[cfg(not(target_os = "linux"))]
const HELP: &str = "ztty — a small no_std Rust terminal\n\n\
Usage: ztty [OPTIONS] [-e COMMAND [ARG...]]\n\n\
  --cwd PATH           Start the command in PATH\n\
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
    #[cfg(target_os = "linux")]
    name: Option<String>,
    #[cfg(target_os = "linux")]
    wayland: bool,
    #[cfg(target_os = "linux")]
    observe: bool,
    #[cfg(target_os = "linux")]
    target: Option<(String, &'static str)>,
    cwd: Option<String>,
    #[cfg(target_os = "linux")]
    config_path: Option<CString>,
}

fn number(bytes: &[u8]) -> Option<usize> {
    core::str::from_utf8(bytes).ok()?.parse().ok()
}

fn config(args: &[&CStr]) -> Result<Option<Config>, String> {
    let mut config_path = None;
    let mut font = None;
    #[cfg(target_os = "linux")]
    let mut text_plugins = Vec::new();
    let mut cfg = Config {
        appearance: Appearance::default(),
        cols: 120,
        rows: 32,
        history: 2000,
        command: Vec::new(),
        #[cfg(target_os = "linux")]
        name: None,
        #[cfg(target_os = "linux")]
        wayland: false,
        #[cfg(target_os = "linux")]
        observe: false,
        #[cfg(target_os = "linux")]
        target: None,
        cwd: None,
        #[cfg(target_os = "linux")]
        config_path: None,
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
            #[cfg(target_os = "linux")]
            b"--text-plugin" => {
                i += 1;
                text_plugins.push(
                    args.get(i)
                        .ok_or("--text-plugin needs NAME=/absolute/library.so")?
                        .to_str()
                        .map_err(|_| "text plugin registration must be UTF-8")?,
                );
            }
            #[cfg(target_os = "linux")]
            b"--observe" => cfg.observe = true,
            #[cfg(target_os = "linux")]
            b"--backend" => {
                i += 1;
                cfg.wayland = match args
                    .get(i)
                    .ok_or("--backend needs x11 or wayland")?
                    .to_bytes()
                {
                    b"x11" => false,
                    b"wayland" => true,
                    _ => return Err("--backend must be x11 or wayland".into()),
                };
            }
            #[cfg(target_os = "linux")]
            b"--tab" | b"--split-right" | b"--split-down" => {
                let mode = match args[i].to_bytes() {
                    b"--tab" => "tab",
                    b"--split-right" => "right",
                    _ => "down",
                };
                i += 1;
                let name = args
                    .get(i)
                    .ok_or("layout option needs a target session")?
                    .to_str()
                    .map_err(|_| "session selector must be UTF-8")?;
                cfg.target = Some((name.into(), mode));
            }
            b"--cwd" => {
                i += 1;
                cfg.cwd = Some(
                    args.get(i)
                        .ok_or("--cwd needs a path")?
                        .to_str()
                        .map_err(|_| "cwd must be UTF-8")?
                        .into(),
                );
            }
            #[cfg(target_os = "linux")]
            b"--name" => {
                i += 1;
                cfg.name = Some(
                    args.get(i)
                        .ok_or("--name needs a value")?
                        .to_str()
                        .map_err(|_| "session name is not UTF-8")?
                        .into(),
                );
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
    #[cfg(target_os = "linux")]
    if cfg.observe && cfg.target.is_some() {
        return Err(
            "--observe opens a standalone view; it cannot be combined with tab/split options"
                .into(),
        );
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
    #[cfg(target_os = "linux")]
    for plugin in text_plugins {
        cfg.appearance.set_text_plugin(plugin)?;
    }
    #[cfg(target_os = "linux")]
    {
        cfg.config_path = config_path
            .map(CString::from)
            .or_else(os::default_config_path);
    }
    if let Some(font) = font {
        cfg.appearance.set_font(font)?;
    }
    Ok(Some(cfg))
}

#[cfg(target_os = "linux")]
fn queue_input(queue: &mut InputQueue, bytes: &[u8]) {
    if !queue.push(bytes) {
        os::print(2, b"ztty: input queue full; paste was rejected\n");
    }
}

#[cfg(target_os = "linux")]
fn key(symbol: u64) -> Key {
    match symbol {
        0xff1b => Key::Escape,
        0xff0d => Key::Enter,
        0xff09 | 0xfe20 => Key::Tab,
        0xff08 => Key::Backspace,
        0xff51 => Key::Left,
        0xff52 => Key::Up,
        0xff53 => Key::Right,
        0xff54 => Key::Down,
        0xff50 => Key::Home,
        0xff57 => Key::End,
        0xff63 => Key::Insert,
        0xffff => Key::Delete,
        0xff55 => Key::PageUp,
        0xff56 => Key::PageDown,
        0xffbe..=0xffe0 => Key::Function((symbol - 0xffbe + 1) as u8),
        0xffe5 => Key::CapsLock,
        0xff14 => Key::ScrollLock,
        0xff7f => Key::NumLock,
        0xff61 => Key::PrintScreen,
        0xff13 => Key::Pause,
        0xff67 => Key::Menu,
        0xffe1 => Key::Modifier(ModifierKey::LeftShift),
        0xffe2 => Key::Modifier(ModifierKey::RightShift),
        0xffe3 => Key::Modifier(ModifierKey::LeftControl),
        0xffe4 => Key::Modifier(ModifierKey::RightControl),
        0xffe9 => Key::Modifier(ModifierKey::LeftAlt),
        0xffea => Key::Modifier(ModifierKey::RightAlt),
        0xffeb => Key::Modifier(ModifierKey::LeftSuper),
        0xffec => Key::Modifier(ModifierKey::RightSuper),
        0xffed => Key::Modifier(ModifierKey::LeftHyper),
        0xffee => Key::Modifier(ModifierKey::RightHyper),
        0xffe7 => Key::Modifier(ModifierKey::LeftMeta),
        0xffe8 => Key::Modifier(ModifierKey::RightMeta),
        0xfe03 => Key::Modifier(ModifierKey::IsoLevel3Shift),
        0xfe11 => Key::Modifier(ModifierKey::IsoLevel5Shift),
        0xffb0..=0xffb9 => Key::KeypadDigit((symbol - 0xffb0) as u8),
        0xffae => Key::KeypadDecimal,
        0xffaf => Key::KeypadDivide,
        0xffaa => Key::KeypadMultiply,
        0xffad => Key::KeypadSubtract,
        0xffab => Key::KeypadAdd,
        0xff8d => Key::KeypadEnter,
        0xffbd => Key::KeypadEqual,
        0xffac => Key::KeypadSeparator,
        0xff96 => Key::KeypadLeft,
        0xff98 => Key::KeypadRight,
        0xff97 => Key::KeypadUp,
        0xff99 => Key::KeypadDown,
        0xff9a => Key::KeypadPageUp,
        0xff9b => Key::KeypadPageDown,
        0xff95 => Key::KeypadHome,
        0xff9c => Key::KeypadEnd,
        0xff9e => Key::KeypadInsert,
        0xff9f => Key::KeypadDelete,
        0xff9d => Key::KeypadBegin,
        0x20..=0x7e | 0xa0..=0xff => {
            Key::Character(char::from_u32(symbol as u32).unwrap_or('\u{fffd}'))
        }
        0x01000100..=0x0110ffff => {
            Key::Character(char::from_u32((symbol - 0x01000000) as u32).unwrap_or('\u{fffd}'))
        }
        _ => Key::Unidentified,
    }
}

#[cfg(target_os = "linux")]
fn modifiers(state: u32) -> Modifiers {
    Modifiers(
        ((state & 1 != 0) as u8)
            | (((state & 8 != 0) as u8) << 1)
            | (((state & 4 != 0) as u8) << 2)
            | (((state & 64 != 0) as u8) << 3)
            | (((state & 2 != 0) as u8) << 6)
            | (((state & 16 != 0) as u8) << 7),
    )
}

#[cfg(target_os = "linux")]
fn key_modifiers(state: u32, symbol: u64, pressed: bool, held: &[u64; 256]) -> Modifiers {
    let mut result = modifiers(state);
    for (bit, symbols) in [
        (1, [0xffe1, 0xffe2]),
        (2, [0xffe9, 0xffea]),
        (4, [0xffe3, 0xffe4]),
        (8, [0xffeb, 0xffec]),
        (16, [0xffed, 0xffee]),
        (32, [0xffe7, 0xffe8]),
    ] {
        if symbols.contains(&symbol) {
            if held.iter().any(|s| symbols.contains(s)) {
                result.0 |= bit;
            } else {
                result.0 &= !bit;
            }
        }
    }
    if pressed && symbol == 0xffe5 {
        result.0 ^= 64;
    }
    if pressed && symbol == 0xff7f {
        result.0 ^= 128;
    }
    result
}

#[cfg(target_os = "linux")]
fn paste(
    bytes: Vec<u8>,
    terminal: &Terminal,
    queue: &mut InputQueue,
) -> Result<usize, &'static str> {
    let text = String::from_utf8_lossy(&bytes);
    let encoded = input::encode_paste(&text, &terminal.modes().input());
    let payload_len = encoded.len()
        - if terminal.modes().bracketed_paste {
            12
        } else {
            0
        };
    if payload_len == 0 {
        return Ok(0);
    }
    if queue.push(&encoded) {
        Ok(payload_len)
    } else {
        Err("Input queue is full; paste was not sent")
    }
}

fn launch(args: &[&CStr]) -> Result<i32, String> {
    config(args).and_then(|cfg| {
        if let Some(cfg) = cfg {
            #[cfg(target_os = "linux")]
            {
                if cfg.wayland {
                    #[cfg(not(feature = "wayland"))]
                    {
                        return Err(
                            "native Wayland is optional; build with --features wayland".into()
                        );
                    }
                }
                platform::run(cfg)
            }
            #[cfg(not(target_os = "linux"))]
            {
                // Native single-PTY processes enter cwd before command lookup,
                // PTY helpers or UI frameworks can start threads. Linux's
                // shared broker applies cwd separately for each new session.
                if let Some(path) = cfg.cwd.as_deref() {
                    os::enter_startup_directory(path).map_err(|error| {
                        alloc::format!("cannot enter command working directory (OS error {error})")
                    })?;
                }
                platform::run(cfg).map_err(String::from)
            }
        } else {
            Ok(0)
        }
    })
}

fn shell_integration(args: &[&CStr]) -> Result<i32, String> {
    let script = match args {
        [name] => match name.to_bytes() {
            b"bash" => include_str!("../shell/ztty.bash"),
            b"zsh" => include_str!("../shell/ztty.zsh"),
            b"fish" => include_str!("../shell/ztty.fish"),
            b"nu" | b"nushell" => include_str!("../shell/ztty.nu"),
            _ => return Err("shell integration supports bash, zsh, fish or nu".into()),
        },
        _ => return Err("usage: ztty shell-integration bash|zsh|fish|nu".into()),
    };
    os::print(1, script.as_bytes());
    Ok(0)
}

#[cfg(target_os = "linux")]
pub(crate) fn serve() -> Result<(), String> {
    session::serve()
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
    #[cfg(target_os = "macos")]
    zpty::run_spawn_helper(&args);
    #[cfg(windows)]
    let owned_args = os::arguments();
    #[cfg(windows)]
    let args: Vec<&CStr> = owned_args.iter().map(|s| s.as_c_str()).collect();
    #[cfg(windows)]
    let _ = (argc, argv);
    #[cfg(target_os = "linux")]
    let result = match args.get(1).map(|s| s.to_bytes()) {
        Some(b"mcp") => mcp::run(&args[2..]),
        Some(b"plugin") => plugin_host::command(&args[2..]),
        Some(b"__plugin-worker") => plugin_host::worker(&args[2..]),
        Some(b"shell-integration") => shell_integration(&args[2..]),
        Some(b"attach") => platform::attach(&args[2..]),
        Some(
            b"list" | b"info" | b"detach" | b"close" | b"rename" | b"action" | b"move" | b"control"
            | b"commands" | b"command-output" | b"save-layout" | b"restore-layout",
        ) => platform::command(&args[1..]),
        _ => launch(&args),
    };
    #[cfg(not(target_os = "linux"))]
    let result = if args
        .get(1)
        .is_some_and(|arg| arg.to_bytes() == b"shell-integration")
    {
        shell_integration(&args[2..])
    } else {
        launch(&args)
    };
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
