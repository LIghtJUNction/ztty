#![no_std]
#![no_main]

extern crate alloc;

mod appearance;
mod os;
mod x11;

use alloc::ffi::CString;
use alloc::string::String;
use alloc::vec::Vec;
use appearance::Appearance;
use core::ffi::{CStr, c_char, c_int};
use input::{Key, KeyEvent, ModifierKey, Modifiers, MouseEvent};
use x11::{Selection, Window};
use zterm::{Terminal, input};

#[global_allocator]
static ALLOCATOR: os::Allocator = os::Allocator;

// Stable Rust's precompiled alloc contains unwind cleanup references even for
// panic=abort consumers. Use the platform unwinder for those ABI references;
// this application's Rust panic/personality boundaries always terminate.
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

const HELP: &str = "ztty — a small no_std Rust terminal for Linux\n\n\
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
Runs on X11 and XWayland. Fonts and desktop integration use system libraries.\n";

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
        let value = unsafe { os::getenv(c"SHELL".as_ptr()) };
        let shell = if value.is_null() {
            c"/bin/sh"
        } else {
            unsafe { CStr::from_ptr(value) }
        };
        cfg.command.push(if shell.is_empty() {
            c"/bin/sh".into()
        } else {
            shell.into()
        });
    }
    cfg.appearance = Appearance::load(config_path)?;
    if let Some(font) = font {
        cfg.appearance.set_font(font)?;
    }
    Ok(Some(cfg))
}

struct Queue {
    bytes: Vec<u8>,
    offset: usize,
}
impl Queue {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            offset: 0,
        }
    }
    fn push(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if self.offset != 0 {
            self.bytes.drain(..self.offset);
            self.offset = 0;
        }
        if self.bytes.len().saturating_add(bytes.len()) <= 8 * 1024 * 1024 {
            self.bytes.extend_from_slice(bytes);
        } else {
            os::print(2, b"ztty: input queue full; paste was rejected\n");
        }
    }
    fn pending(&self) -> bool {
        self.offset < self.bytes.len()
    }
    fn flush(&mut self, pty: &zpty::Pty) {
        while self.pending() {
            let bytes = &self.bytes[self.offset..];
            match pty.write(bytes) {
                Ok(0) => break,
                Ok(n) => self.offset += n,
                Err(_) => break,
            }
        }
        if !self.pending() {
            self.bytes.clear();
            self.offset = 0;
        }
    }
}

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

fn paste(bytes: Vec<u8>, terminal: &Terminal, queue: &mut Queue) {
    let text = String::from_utf8_lossy(&bytes);
    queue.push(&input::encode_paste(&text, &terminal.modes().input()));
}

fn run(cfg: Config) -> Result<i32, &'static str> {
    let mut window = Window::new(cfg.cols, cfg.rows, &cfg.appearance)?;
    let (program, environment) = os::child_setup(&cfg.command[0])?;
    let arguments: Vec<&CStr> = cfg.command.iter().map(|s| s.as_c_str()).collect();
    let environment: Vec<&CStr> = environment.iter().map(|s| s.as_c_str()).collect();
    // ztty deliberately uses one thread. Arguments, environment, and executable
    // lookup are complete before forkpty; the child only closes FDs and execs.
    let mut pty = unsafe {
        zpty::Pty::spawn(
            &program,
            &arguments,
            &environment,
            zpty::Size::new(cfg.cols as u16, cfg.rows as u16),
            &[window.fd],
        )
    }
    .map_err(|_| "cannot create a PTY")?;
    let mut openers = os::Openers::new();
    let mut terminal = Terminal::with_theme(cfg.cols, cfg.rows, cfg.history, cfg.appearance.theme);
    let mut queue = Queue::new();
    let mut selection: Option<Selection> = None;
    let mut history = 0usize;
    let mut force = true;
    let mut running = true;
    let mut title = String::new();
    let mut held = [0u64; 256];
    let mut intercepted = [false; 256];
    let mut local_buttons = [false; 8];
    let mut swallowed_buttons = [false; 8];
    let mut sync_since: Option<u64> = None;
    let mut blink_deadline = None;
    let mut cursor_visible = true;
    let mut last_cursor = None;
    let mut reset_blink = false;
    let mut read_buffer = [0u8; 65536];
    let mut text_buffer = alloc::vec![0u8; 4096];
    let mut encoded = Vec::with_capacity(128);
    while running {
        let mut events = 0;
        while events < 512 && unsafe { x11::XPending(window.display) } != 0 {
            events += 1;
            let mut event = x11::Event { pad: [0; 24] };
            unsafe {
                x11::XNextEvent(window.display, &mut event);
            }
            // XIM transport events can target Xlib's private windows.
            // Let Xlib use the event destination when selecting a filter.
            if unsafe { x11::XFilterEvent(&mut event, 0) } != 0 {
                continue;
            }
            match unsafe { event.kind } {
                x11::KEY_PRESS | x11::KEY_RELEASE => {
                    let mut e = unsafe { event.key };
                    let code = (e.keycode as usize).min(255);
                    let press = e.kind == x11::KEY_PRESS;
                    reset_blink |= press;
                    let symbol = if !press && held[code] != 0 {
                        held[code]
                    } else {
                        window.key_symbol(&mut e, false)
                    };
                    let repeated = held[code] != 0;
                    held[code] = if press { symbol } else { 0 };
                    let mods = key_modifiers(e.state, symbol, press, &held);
                    if !press && intercepted[code] {
                        intercepted[code] = false;
                        continue;
                    }
                    if press && mods.0 & 5 == 5 && (symbol == b'c' as u64 || symbol == b'C' as u64)
                    {
                        if let Some(s) = selection {
                            window.copy(s.text(&terminal, history), false);
                        }
                        intercepted[code] = true;
                        continue;
                    }
                    if press && mods.0 & 5 == 5 && (symbol == b'v' as u64 || symbol == b'V' as u64)
                    {
                        window.paste(false);
                        intercepted[code] = true;
                        continue;
                    }
                    if press && mods.0 & 1 != 0 && (symbol == 0xff55 || symbol == 0xff56) {
                        let step = terminal.rows().saturating_sub(1).max(1);
                        history = if symbol == 0xff55 {
                            history.saturating_add(step).min(terminal.history_len())
                        } else {
                            history.saturating_sub(step)
                        };
                        selection = None;
                        force = true;
                        intercepted[code] = true;
                        continue;
                    }
                    let lookup = if press {
                        window.lookup(&mut e, &mut text_buffer)
                    } else {
                        x11::Lookup {
                            len: 0,
                            symbol,
                            text_only: false,
                            ignored: false,
                        }
                    };
                    if lookup.ignored {
                        intercepted[code] = true;
                        continue;
                    }
                    let text = core::str::from_utf8(&text_buffer[..lookup.len]).unwrap_or("");
                    let input_key = if symbol == 0 {
                        key(lookup.symbol)
                    } else {
                        key(symbol)
                    };
                    encoded.clear();
                    if lookup.text_only {
                        input::encode_text_into(text, &terminal.modes().input(), &mut encoded);
                        intercepted[code] = true;
                    } else {
                        let shifted = if mods.0 & 1 != 0 {
                            if let Key::Character(ch) = key(window.key_symbol(&mut e, true)) {
                                Some(ch)
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        input::encode_key_with_alternates_into(
                            input_key,
                            mods,
                            if !press {
                                KeyEvent::Release
                            } else if repeated {
                                KeyEvent::Repeat
                            } else {
                                KeyEvent::Press
                            },
                            text,
                            input::KeyAlternates {
                                shifted,
                                base_layout: None,
                            },
                            &terminal.modes().input(),
                            &mut encoded,
                        );
                    }
                    if !encoded.is_empty() {
                        if history != 0 {
                            history = 0;
                            force = true;
                        }
                        if selection.is_some() {
                            selection = None;
                            force = true;
                        }
                        queue.push(&encoded);
                    }
                }
                x11::BUTTON_PRESS | x11::BUTTON_RELEASE => {
                    let e = unsafe { event.button };
                    let (col, row) = window.coordinates(e.x, e.y, terminal.cols(), terminal.rows());
                    let mods = modifiers(e.state);
                    let pressed = e.kind == x11::BUTTON_PRESS;
                    let button_index = (e.button as usize).min(7);
                    if !pressed && swallowed_buttons[button_index] {
                        swallowed_buttons[button_index] = false;
                        continue;
                    }
                    let mut local = mods.0 & 1 != 0 || terminal.modes().mouse_tracking == 0;
                    if pressed && local {
                        local_buttons[button_index] = true;
                    }
                    if !pressed {
                        local |= local_buttons[button_index];
                        local_buttons[button_index] = false;
                    }
                    if pressed && e.button == 1 && mods.0 & 4 != 0 {
                        let id = terminal.view_cell(col, row, history).link_id;
                        if let Some(uri) = terminal.hyperlink(id) {
                            openers.open(uri, window.fd, pty.fd());
                            swallowed_buttons[button_index] = true;
                            local_buttons[button_index] = false;
                            continue;
                        }
                    }
                    if local {
                        if pressed && e.button == 1 {
                            selection = Some(Selection {
                                start: (col, row),
                                end: (col, row),
                                dragging: true,
                            });
                            force = true;
                        } else if !pressed && e.button == 1 {
                            if let Some(ref mut s) = selection {
                                s.end = (col, row);
                                s.dragging = false;
                                window.copy(s.text(&terminal, history), true);
                                force = true;
                            }
                        } else if pressed && e.button == 2 {
                            window.paste(true);
                        } else if pressed && (e.button == 4 || e.button == 5) {
                            history = if e.button == 4 {
                                history.saturating_add(3).min(terminal.history_len())
                            } else {
                                history.saturating_sub(3)
                            };
                            selection = None;
                            force = true;
                        }
                    } else {
                        let mouse = if e.button == 4 || e.button == 5 {
                            if !pressed {
                                continue;
                            }
                            MouseEvent::Scroll {
                                up: e.button == 4,
                                col,
                                row,
                            }
                        } else if (1..=3).contains(&e.button) {
                            if pressed {
                                MouseEvent::Press {
                                    button: (e.button - 1) as u8,
                                    col,
                                    row,
                                }
                            } else {
                                MouseEvent::Release {
                                    button: (e.button - 1) as u8,
                                    col,
                                    row,
                                }
                            }
                        } else {
                            continue;
                        };
                        encoded.clear();
                        input::encode_mouse_into(
                            mouse,
                            mods,
                            &terminal.modes().input(),
                            &mut encoded,
                        );
                        queue.push(&encoded);
                    }
                }
                x11::MOTION_NOTIFY => {
                    let e = unsafe { event.motion };
                    let (col, row) = window.coordinates(e.x, e.y, terminal.cols(), terminal.rows());
                    if let Some(ref mut s) = selection
                        && s.dragging
                    {
                        s.end = (col, row);
                        force = true;
                        continue;
                    }
                    let button = if e.state & 256 != 0 {
                        Some(0)
                    } else if e.state & 512 != 0 {
                        Some(1)
                    } else if e.state & 1024 != 0 {
                        Some(2)
                    } else {
                        None
                    };
                    encoded.clear();
                    input::encode_mouse_into(
                        MouseEvent::Motion { button, col, row },
                        modifiers(e.state),
                        &terminal.modes().input(),
                        &mut encoded,
                    );
                    queue.push(&encoded);
                }
                x11::CONFIGURE_NOTIFY => {
                    let e = unsafe { event.configure };
                    if e.width != window.width || e.height != window.height {
                        let (cols, rows) = window.resize(e.width, e.height);
                        terminal.resize(cols, rows);
                        pty.resize(zpty::Size {
                            rows: rows as u16,
                            cols: cols as u16,
                            pixel_width: e.width.clamp(0, 65535) as u16,
                            pixel_height: e.height.clamp(0, 65535) as u16,
                        })
                        .map_err(|_| "cannot resize the PTY")?;
                        history = history.min(terminal.history_len());
                        selection = None;
                        force = true;
                    }
                }
                x11::FOCUS_IN | x11::FOCUS_OUT => {
                    let focused = unsafe { event.kind } == x11::FOCUS_IN;
                    window.focus(focused);
                    reset_blink = true;
                    force = true;
                    if !focused {
                        held.fill(0);
                        intercepted.fill(false);
                    }
                    queue.push(&input::encode_focus(focused, &terminal.modes().input()));
                }
                x11::EXPOSE => {
                    window.present();
                    force = true;
                }
                x11::CLIENT_MESSAGE => {
                    if unsafe { event.client.data.longs[0] as u64 } == window.wm_delete {
                        running = false;
                    }
                }
                x11::DESTROY_NOTIFY => {
                    window.id = 0;
                    running = false;
                }
                x11::SELECTION_REQUEST => {
                    window.selection_request(unsafe { event.request });
                }
                x11::SELECTION_NOTIFY => {
                    if let Some(data) = window.selection_notify(unsafe { event.selection }) {
                        paste(data, &terminal, &mut queue);
                    }
                }
                x11::PROPERTY_NOTIFY => {
                    if let Some(data) = window.property(unsafe { event.property }) {
                        paste(data, &terminal, &mut queue);
                    }
                }
                x11::SELECTION_CLEAR => {}
                _ => {}
            }
        }
        if !running {
            break;
        }
        if !terminal.responses().is_empty() {
            queue.push(terminal.responses());
            terminal.clear_responses();
        }
        queue.flush(&pty);
        let now = os::milliseconds();
        if terminal.modes().synchronized_output {
            let start = sync_since.get_or_insert(now);
            if now.saturating_sub(*start) >= 150 {
                terminal.force_end_sync();
                sync_since = None;
            }
        } else {
            sync_since = None;
        }
        if terminal.title() != title {
            title.clear();
            title.push_str(terminal.title());
            window.title(&title);
        }
        let modes = terminal.modes();
        let cursor = (history == 0 && modes.cursor_visible).then(|| {
            (
                terminal.cursor(),
                modes.cursor_shape.unwrap_or(cfg.appearance.cursor_shape),
            )
        });
        let blinking = window.focused
            && cursor.is_some()
            && modes.cursor_blink.unwrap_or(cfg.appearance.cursor_blink);
        let synchronized = modes.synchronized_output;
        if !blinking || synchronized {
            blink_deadline = None;
            cursor_visible = true;
        } else if blink_deadline.is_none() || reset_blink || cursor != last_cursor {
            blink_deadline = Some(now + cfg.appearance.cursor_blink_interval);
            cursor_visible = true;
        } else if blink_deadline.is_some_and(|deadline| deadline <= now) {
            blink_deadline = Some(now + cfg.appearance.cursor_blink_interval);
            cursor_visible = !cursor_visible;
        }
        last_cursor = cursor;
        reset_blink = false;
        let cursor = cursor.filter(|_| cursor_visible);
        if !synchronized && (force || terminal.dirty() || window.cursor_changed(cursor)) {
            window.render(&mut terminal, history, selection, force, cursor);
            force = false;
        }
        let _ = pty.reap();
        openers.reap();
        let mut fds = [
            os::PollFd {
                fd: window.fd,
                events: os::POLLIN,
                revents: 0,
            },
            os::PollFd {
                fd: pty.fd(),
                events: os::POLLIN | if queue.pending() { os::POLLOUT } else { 0 },
                revents: 0,
            },
        ];
        let timeout = if unsafe { x11::XPending(window.display) } != 0 {
            0
        } else if let Some(deadline) = sync_since.map(|start| start + 150).or(blink_deadline) {
            deadline.saturating_sub(os::milliseconds()) as i32
        } else {
            -1
        };
        let timeout = if openers.pending() {
            if timeout < 0 { 50 } else { timeout.min(50) }
        } else {
            timeout
        };
        let count = unsafe { os::poll(fds.as_mut_ptr(), fds.len(), timeout) };
        if count < 0 {
            if os::errno() == os::EINTR {
                continue;
            }
            return Err("poll failed");
        }
        if fds[0].revents & (os::POLLERR | os::POLLHUP) != 0 {
            break;
        }
        if fds[1].revents & os::POLLOUT != 0 {
            queue.flush(&pty);
        }
        if fds[1].revents & (os::POLLIN | os::POLLHUP | os::POLLERR) != 0 {
            for _ in 0..16 {
                let n = match pty.read(&mut read_buffer) {
                    Ok(0) => {
                        running = false;
                        break;
                    }
                    Ok(n) => n,
                    Err(error) if error.is_would_block() => break,
                    Err(_) => {
                        running = false;
                        break;
                    }
                };
                if n > 0 {
                    let before = terminal.history_len();
                    terminal.feed(&read_buffer[..n]);
                    if history != 0 {
                        history = history
                            .saturating_add(terminal.history_len().saturating_sub(before))
                            .min(terminal.history_len());
                        force = true;
                    }
                }
            }
        }
    }
    let _ = pty.wait_timeout(20);
    let status = pty
        .status()
        .map(|status| status.code().unwrap_or(128 + status.signal().unwrap_or(0)))
        .unwrap_or(0);
    Ok(status)
}

/// C runtime entry point. Rust's standard library and its runtime are absent.
///
/// # Safety
/// `argv` must point to `argc` valid, NUL-terminated argument strings, as supplied
/// by the Linux C runtime at process startup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn main(argc: c_int, argv: *const *const c_char) -> c_int {
    let args: Vec<&CStr> = unsafe { core::slice::from_raw_parts(argv, argc.max(0) as usize) }
        .iter()
        .map(|&ptr| unsafe { CStr::from_ptr(ptr) })
        .collect();
    let result = config(&args).and_then(|cfg| {
        if let Some(cfg) = cfg {
            run(cfg).map_err(String::from)
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
