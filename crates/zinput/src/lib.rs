//! Keyboard, mouse, focus and clipboard encoding without platform dependencies.
//!
//! Character keys identify the unshifted key. `text` is the complete UTF-8 text
//! committed by the platform, including multi-character input-method commits.
//! Modifier masks describe the state after the event. Mouse coordinates are
//! zero-based; this module converts them to the protocol's one-based values.
//! The `_into` variants append to caller-owned buffers so event loops can reuse
//! their allocation. They do not clear existing bytes.

#![no_std]
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;

/// Application-negotiated input modes. The host owns protocol negotiation and
/// supplies the active modes for each event; this codec has no global state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modes {
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub bracketed_paste: bool,
    pub focus_events: bool,
    pub sgr_mouse: bool,
    /// Zero disables tracking; supported values are 9, 1000, 1002 and 1003.
    pub mouse_tracking: u16,
    /// Kitty progressive enhancement flags: 1, 2, 4, 8 and 16.
    pub kitty_keyboard: u8,
}

/// Active modifiers. Values match the Kitty keyboard protocol's bit positions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers(pub u8);

impl Modifiers {
    pub const SHIFT: u8 = 1;
    pub const ALT: u8 = 2;
    pub const CTRL: u8 = 4;
    pub const SUPER: u8 = 8;
    pub const HYPER: u8 = 16;
    pub const META: u8 = 32;
    pub const CAPS_LOCK: u8 = 64;
    pub const NUM_LOCK: u8 = 128;

    pub const fn contains(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    const fn keyboard(self) -> usize {
        self.0 as usize + 1
    }

    const fn legacy_keyboard(self) -> usize {
        (self.0 & 63) as usize + 1
    }

    const fn mouse(self) -> u8 {
        (if self.contains(Self::SHIFT) { 4 } else { 0 })
            | (if self.contains(Self::ALT | Self::META) {
                8
            } else {
                0
            })
            | (if self.contains(Self::CTRL) { 16 } else { 0 })
    }
}

/// Key transition. Legacy applications receive press and repeat identically.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyEvent {
    Press,
    Repeat,
    Release,
}

impl KeyEvent {
    const fn number(self) -> usize {
        match self {
            Self::Press => 1,
            Self::Repeat => 2,
            Self::Release => 3,
        }
    }
}

/// Physical modifier keys, reported when Kitty's report-all-keys flag is set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModifierKey {
    LeftShift,
    LeftControl,
    LeftAlt,
    LeftSuper,
    LeftHyper,
    LeftMeta,
    RightShift,
    RightControl,
    RightAlt,
    RightSuper,
    RightHyper,
    RightMeta,
    IsoLevel3Shift,
    IsoLevel5Shift,
}

/// Platform-independent key identity. Character values must be unshifted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Key {
    Character(char),
    /// Text committed by an input method without an associated physical key.
    Unidentified,
    Escape,
    Enter,
    Tab,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,
    Function(u8),
    CapsLock,
    ScrollLock,
    NumLock,
    PrintScreen,
    Pause,
    Menu,
    Modifier(ModifierKey),
    KeypadDigit(u8),
    KeypadDecimal,
    KeypadDivide,
    KeypadMultiply,
    KeypadSubtract,
    KeypadAdd,
    KeypadEnter,
    KeypadEqual,
    KeypadSeparator,
    KeypadLeft,
    KeypadRight,
    KeypadUp,
    KeypadDown,
    KeypadPageUp,
    KeypadPageDown,
    KeypadHome,
    KeypadEnd,
    KeypadInsert,
    KeypadDelete,
    KeypadBegin,
}

/// Optional layout information for Kitty's alternate-key reporting.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KeyAlternates {
    /// Shifted character in the active keyboard layout; emitted only with Shift.
    pub shifted: Option<char>,
    /// Character at this physical key's position on the standard PC-101 layout.
    pub base_layout: Option<char>,
}

/// Mouse coordinates and button numbers are zero-based: 0=left, 1=middle,
/// 2=right, 3/4=wheel up/down, 5/6=horizontal wheel, and 7..=10=extra buttons.
/// Wheel releases/motion and buttons beyond xterm's unambiguous range are
/// ignored. `Scroll` is a convenience alias for wheel up/down presses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseEvent {
    Press {
        button: u8,
        col: usize,
        row: usize,
    },
    Release {
        button: u8,
        col: usize,
        row: usize,
    },
    Motion {
        button: Option<u8>,
        col: usize,
        row: usize,
    },
    Scroll {
        up: bool,
        col: usize,
        row: usize,
    },
}

const DISAMBIGUATE: u8 = 1;
const REPORT_EVENTS: u8 = 2;
const REPORT_ALTERNATES: u8 = 4;
const REPORT_ALL: u8 = 8;
const REPORT_TEXT: u8 = 16;

/// Encode one key event. Use `encode_key_into` to reuse a buffer in an event loop.
pub fn encode_key(
    key: Key,
    modifiers: Modifiers,
    event: KeyEvent,
    text: &str,
    modes: &Modes,
) -> Vec<u8> {
    let mut output = Vec::new();
    encode_key_into(key, modifiers, event, text, modes, &mut output);
    output
}

/// Append an event, using available committed text to infer a shifted character.
pub fn encode_key_into(
    key: Key,
    modifiers: Modifiers,
    event: KeyEvent,
    text: &str,
    modes: &Modes,
    output: &mut Vec<u8>,
) {
    let shifted = if modifiers.contains(Modifiers::SHIFT) {
        single_printable(text).or_else(|| match key {
            Key::Character(c) if c.is_ascii_lowercase() => Some(c.to_ascii_uppercase()),
            _ => None,
        })
    } else {
        None
    };
    encode_key_with_alternates_into(
        key,
        modifiers,
        event,
        text,
        KeyAlternates {
            shifted,
            base_layout: None,
        },
        modes,
        output,
    );
}

/// Encode an event with explicit layout alternatives supplied by the platform.
pub fn encode_key_with_alternates(
    key: Key,
    modifiers: Modifiers,
    event: KeyEvent,
    text: &str,
    alternates: KeyAlternates,
    modes: &Modes,
) -> Vec<u8> {
    let mut output = Vec::new();
    encode_key_with_alternates_into(key, modifiers, event, text, alternates, modes, &mut output);
    output
}

/// Append an event with explicit layout alternatives to a reusable buffer.
pub fn encode_key_with_alternates_into(
    key: Key,
    modifiers: Modifiers,
    event: KeyEvent,
    text: &str,
    alternates: KeyAlternates,
    modes: &Modes,
    output: &mut Vec<u8>,
) {
    let flags = modes.kitty_keyboard & 31;
    let all = flags & REPORT_ALL != 0;
    let events = flags & REPORT_EVENTS != 0;

    // Input methods can commit more than one character and may expose no key.
    // Preserve that complete text even in report-all mode without report-text.
    if key == Key::Unidentified {
        if event != KeyEvent::Release {
            encode_text_into(text, modes, output);
        }
        return;
    }
    if matches!(key, Key::Modifier(_)) && !all {
        return;
    }

    let legacy_exception = matches!(key, Key::Enter | Key::Tab | Key::Backspace);
    let text_key = matches!(key, Key::Character(_)) || keypad_text(key).is_some();
    // Disambiguation keeps ordinary printable keypad input as text. Explicit
    // application-keypad mode and report-all still request functional events.
    let enhanced = all
        || (flags & DISAMBIGUATE != 0
            && !legacy_exception
            && (key == Key::Escape
                || (modes.app_keypad && keypad_text(key).is_some())
                || !text_key
                || modifiers.contains(Modifiers::CTRL | Modifiers::ALT)))
        || (events && !text_key && !legacy_exception)
        || (text_key
            && modifiers.contains(Modifiers::SUPER | Modifiers::HYPER | Modifiers::META))
        // Kitty and Ghostty preserve Ctrl+Shift text-key shortcuts even in
        // legacy mode. Space retains its documented C0/NUL exception.
        || (matches!(key, Key::Character(c) if c != ' ' && printable(c))
            && modifiers.0 & (Modifiers::CTRL | Modifiers::SHIFT)
                == (Modifiers::CTRL | Modifiers::SHIFT));

    if event == KeyEvent::Release && (!events || !enhanced || (!all && legacy_exception)) {
        return;
    }

    if enhanced {
        if let Some((number, suffix)) = canonical(key) {
            let associated = all
                && flags & REPORT_TEXT != 0
                && event != KeyEvent::Release
                && text.chars().any(printable);
            // Legacy combinations omit lock state. Enhanced non-text/control
            // keys retain it; report-all also retains it for text keys.
            let modifier = if all
                || (flags & (DISAMBIGUATE | REPORT_EVENTS) != 0
                    && (!text_key || modifiers.contains(Modifiers::CTRL)))
            {
                modifiers.keyboard()
            } else {
                modifiers.legacy_keyboard()
            };
            output.extend_from_slice(b"\x1b[");
            if number != 1 || matches!(suffix, b'u' | b'~') || modifier != 1 || events || associated
            {
                decimal(number, output);
            }
            if flags & REPORT_ALTERNATES != 0 && matches!(key, Key::Character(_)) {
                let shifted = alternates
                    .shifted
                    .filter(|_| modifiers.contains(Modifiers::SHIFT));
                if shifted.is_some() || alternates.base_layout.is_some() {
                    output.push(b':');
                    if let Some(c) = shifted {
                        decimal(c as usize, output);
                    }
                    if let Some(c) = alternates.base_layout {
                        output.push(b':');
                        decimal(c as usize, output);
                    }
                }
            }
            if modifier != 1 || events || associated {
                output.push(b';');
                decimal(modifier, output);
                if events {
                    output.push(b':');
                    decimal(event.number(), output);
                }
            }
            if associated {
                output.push(b';');
                codepoints(text, output);
            }
            output.push(suffix);
        }
        return;
    }

    if event == KeyEvent::Release {
        return;
    }
    legacy_key(key, modifiers, text, modes, output);
}

/// Encode a pure input-method commit, preserving every printable character.
/// With both Kitty text flags enabled it uses key 0 and embedded codepoints;
/// otherwise it remains UTF-8, including when no physical key is available.
pub fn encode_text(text: &str, modes: &Modes) -> Vec<u8> {
    let mut output = Vec::new();
    encode_text_into(text, modes, &mut output);
    output
}

pub fn encode_text_into(text: &str, modes: &Modes, output: &mut Vec<u8>) {
    if !text.chars().any(printable) {
        return;
    }
    if modes.kitty_keyboard & (REPORT_ALL | REPORT_TEXT) == (REPORT_ALL | REPORT_TEXT) {
        output.extend_from_slice(b"\x1b[0;1;");
        codepoints(text, output);
        output.push(b'u');
    } else {
        printable_text(text, output);
    }
}

/// Encode a mouse event only when a tracking mode is enabled.
/// The caller should suppress motion that stays inside the same character cell.
pub fn encode_mouse(event: MouseEvent, modifiers: Modifiers, modes: &Modes) -> Vec<u8> {
    let mut output = Vec::new();
    encode_mouse_into(event, modifiers, modes, &mut output);
    output
}

pub fn encode_mouse_into(
    event: MouseEvent,
    modifiers: Modifiers,
    modes: &Modes,
    output: &mut Vec<u8>,
) {
    if !matches!(modes.mouse_tracking, 9 | 1000 | 1002 | 1003) {
        return;
    }
    let x10 = modes.mouse_tracking == 9;
    if x10 && !matches!(event, MouseEvent::Press { button: 0..=2, .. }) {
        return;
    }
    let (button, col, row, release) = match event {
        MouseEvent::Press { button, col, row } => {
            let Some(button) = mouse_button(button) else {
                return;
            };
            (button, col, row, false)
        }
        MouseEvent::Release { button, col, row } if !wheel_button(button) => {
            let Some(button) = mouse_button(button) else {
                return;
            };
            (button, col, row, true)
        }
        MouseEvent::Motion { button, col, row }
            if (modes.mouse_tracking == 1003
                || modes.mouse_tracking == 1002 && button.is_some())
                && button.is_none_or(|b| !wheel_button(b)) =>
        {
            let button = match button {
                Some(button) => {
                    let Some(button) = mouse_button(button) else {
                        return;
                    };
                    button
                }
                None => 3,
            };
            (32 | button, col, row, false)
        }
        MouseEvent::Scroll { up, col, row } => (if up { 64 } else { 65 }, col, row, false),
        _ => return,
    };
    let (Some(x), Some(y)) = (col.checked_add(1), row.checked_add(1)) else {
        return;
    };
    let modifiers = if x10 { 0 } else { modifiers.mouse() };
    if modes.sgr_mouse {
        output.extend_from_slice(b"\x1b[<");
        decimal((button | modifiers) as usize, output);
        output.push(b';');
        decimal(x, output);
        output.push(b';');
        decimal(y, output);
        output.push(if release { b'm' } else { b'M' });
    } else if x <= 223 && y <= 223 {
        let button = if release { 3 } else { button };
        output.extend_from_slice(&[
            0x1b,
            b'[',
            b'M',
            (button | modifiers) + 32,
            x as u8 + 32,
            y as u8 + 32,
        ]);
    }
}

fn wheel_button(button: u8) -> bool {
    (3..=6).contains(&button)
}

fn mouse_button(button: u8) -> Option<u8> {
    Some(match button {
        0..=2 => button,
        3..=6 => 64 + button - 3,
        7..=10 => 128 + button - 7,
        _ => return None,
    })
}

/// Encode application focus changes if DEC mode 1004 is enabled.
pub fn encode_focus(focused: bool, modes: &Modes) -> Vec<u8> {
    let mut output = Vec::new();
    if modes.focus_events {
        output.extend_from_slice(if focused { b"\x1b[I" } else { b"\x1b[O" });
    }
    output
}

/// Encode clipboard text. Tabs and line endings are retained; other control
/// characters are removed, so clipboard data cannot close its own paste bracket.
pub fn encode_paste(text: &str, modes: &Modes) -> Vec<u8> {
    let mut output = Vec::new();
    encode_paste_into(text, modes, &mut output);
    output
}

pub fn encode_paste_into(text: &str, modes: &Modes, output: &mut Vec<u8>) {
    if modes.bracketed_paste {
        output.extend_from_slice(b"\x1b[200~");
    }
    for c in text.chars() {
        if printable(c) || matches!(c, '\t' | '\n' | '\r') {
            character(c, output);
        }
    }
    if modes.bracketed_paste {
        output.extend_from_slice(b"\x1b[201~");
    }
}

fn legacy_key(key: Key, modifiers: Modifiers, text: &str, modes: &Modes, output: &mut Vec<u8>) {
    if modes.app_keypad
        && let Some(suffix) = application_keypad(key)
    {
        ss3_or_modified(suffix, modifiers, output);
        return;
    }
    if let Some(c) = keypad_text(key) {
        legacy_key(Key::Character(c), modifiers, text, modes, output);
        return;
    }
    match key {
        Key::Character(c) => {
            if modifiers.contains(Modifiers::ALT) {
                output.push(0x1b);
            }
            if modifiers.contains(Modifiers::CTRL) {
                if let Some(byte) = control(c) {
                    output.push(byte);
                } else if text.chars().any(printable) {
                    printable_text(text, output);
                } else {
                    character(c, output);
                }
            } else if text.chars().any(printable) {
                printable_text(text, output);
            } else {
                let c = if modifiers.contains(Modifiers::SHIFT) && c.is_ascii_lowercase() {
                    c.to_ascii_uppercase()
                } else {
                    c
                };
                if printable(c) {
                    character(c, output);
                }
            }
        }
        Key::Escape | Key::Enter | Key::KeypadEnter | Key::Tab | Key::Backspace => {
            if modifiers.contains(Modifiers::ALT) {
                output.push(0x1b);
            }
            match key {
                Key::Escape => output.push(0x1b),
                Key::Enter | Key::KeypadEnter => output.push(b'\r'),
                Key::Backspace => output.push(if modifiers.contains(Modifiers::CTRL) {
                    8
                } else {
                    127
                }),
                _ if modifiers.contains(Modifiers::SHIFT) => output.extend_from_slice(b"\x1b[Z"),
                _ => output.push(b'\t'),
            }
        }
        Key::Up
        | Key::Down
        | Key::Left
        | Key::Right
        | Key::Home
        | Key::End
        | Key::KeypadUp
        | Key::KeypadDown
        | Key::KeypadLeft
        | Key::KeypadRight
        | Key::KeypadHome
        | Key::KeypadEnd => {
            let suffix = match key {
                Key::Up | Key::KeypadUp => b'A',
                Key::Down | Key::KeypadDown => b'B',
                Key::Right | Key::KeypadRight => b'C',
                Key::Left | Key::KeypadLeft => b'D',
                Key::Home | Key::KeypadHome => b'H',
                _ => b'F',
            };
            if modifiers.legacy_keyboard() != 1 {
                csi_key(1, suffix, modifiers, output);
            } else if modes.app_cursor {
                output.extend_from_slice(&[0x1b, b'O', suffix]);
            } else {
                output.extend_from_slice(&[0x1b, b'[', suffix]);
            }
        }
        Key::Function(n @ 1..=4) => ss3_or_modified(b'P' + n - 1, modifiers, output),
        Key::KeypadInsert => csi_key(2, b'~', modifiers, output),
        Key::KeypadDelete => csi_key(3, b'~', modifiers, output),
        Key::KeypadPageUp => csi_key(5, b'~', modifiers, output),
        Key::KeypadPageDown => csi_key(6, b'~', modifiers, output),
        Key::KeypadBegin => csi_key(1, b'E', modifiers, output),
        Key::Menu => csi_key(29, b'~', modifiers, output),
        // Lock keys change keyboard state; ordinary shells have no legacy
        // binding for their private-use CSI-u numbers. Report them only after
        // the child explicitly opts into the enhanced keyboard protocol.
        Key::CapsLock | Key::NumLock | Key::ScrollLock | Key::Modifier(_) | Key::Unidentified => {}
        _ => {
            if let Some((number, suffix)) = canonical(key) {
                csi_key(number, suffix, modifiers, output);
            }
        }
    }
}

fn canonical(key: Key) -> Option<(usize, u8)> {
    let pair = match key {
        Key::Character(c) => (c as usize, b'u'),
        Key::Unidentified => (0, b'u'),
        Key::Escape => (27, b'u'),
        Key::Enter => (13, b'u'),
        Key::Tab => (9, b'u'),
        Key::Backspace => (127, b'u'),
        Key::Up => (1, b'A'),
        Key::Down => (1, b'B'),
        Key::Right => (1, b'C'),
        Key::Left => (1, b'D'),
        Key::Home => (1, b'H'),
        Key::End => (1, b'F'),
        Key::Insert => (2, b'~'),
        Key::Delete => (3, b'~'),
        Key::PageUp => (5, b'~'),
        Key::PageDown => (6, b'~'),
        Key::Function(1) => (1, b'P'),
        Key::Function(2) => (1, b'Q'),
        // CSI R is reserved for cursor-position reports in enhanced mode.
        Key::Function(3) => (13, b'~'),
        Key::Function(4) => (1, b'S'),
        Key::Function(n @ 5..=12) => ([15, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize], b'~'),
        Key::Function(n @ 13..=35) => (57376 + (n - 13) as usize, b'u'),
        Key::Function(_) => return None,
        Key::CapsLock => (57358, b'u'),
        Key::ScrollLock => (57359, b'u'),
        Key::NumLock => (57360, b'u'),
        Key::PrintScreen => (57361, b'u'),
        Key::Pause => (57362, b'u'),
        Key::Menu => (57363, b'u'),
        Key::KeypadBegin => (1, b'E'),
        Key::Modifier(key) => (
            57441
                + match key {
                    ModifierKey::LeftShift => 0,
                    ModifierKey::LeftControl => 1,
                    ModifierKey::LeftAlt => 2,
                    ModifierKey::LeftSuper => 3,
                    ModifierKey::LeftHyper => 4,
                    ModifierKey::LeftMeta => 5,
                    ModifierKey::RightShift => 6,
                    ModifierKey::RightControl => 7,
                    ModifierKey::RightAlt => 8,
                    ModifierKey::RightSuper => 9,
                    ModifierKey::RightHyper => 10,
                    ModifierKey::RightMeta => 11,
                    ModifierKey::IsoLevel3Shift => 12,
                    ModifierKey::IsoLevel5Shift => 13,
                },
            b'u',
        ),
        _ => (keypad_code(key)? as usize, b'u'),
    };
    Some(pair)
}

fn keypad_code(key: Key) -> Option<u16> {
    Some(match key {
        Key::KeypadDigit(n @ 0..=9) => 57399 + n as u16,
        Key::KeypadDecimal => 57409,
        Key::KeypadDivide => 57410,
        Key::KeypadMultiply => 57411,
        Key::KeypadSubtract => 57412,
        Key::KeypadAdd => 57413,
        Key::KeypadEnter => 57414,
        Key::KeypadEqual => 57415,
        Key::KeypadSeparator => 57416,
        Key::KeypadLeft => 57417,
        Key::KeypadRight => 57418,
        Key::KeypadUp => 57419,
        Key::KeypadDown => 57420,
        Key::KeypadPageUp => 57421,
        Key::KeypadPageDown => 57422,
        Key::KeypadHome => 57423,
        Key::KeypadEnd => 57424,
        Key::KeypadInsert => 57425,
        Key::KeypadDelete => 57426,
        Key::KeypadBegin => 57427,
        _ => return None,
    })
}

fn keypad_text(key: Key) -> Option<char> {
    Some(match key {
        Key::KeypadDigit(n @ 0..=9) => (b'0' + n) as char,
        Key::KeypadDecimal => '.',
        Key::KeypadDivide => '/',
        Key::KeypadMultiply => '*',
        Key::KeypadSubtract => '-',
        Key::KeypadAdd => '+',
        Key::KeypadEqual => '=',
        Key::KeypadSeparator => ',',
        _ => return None,
    })
}

fn application_keypad(key: Key) -> Option<u8> {
    Some(match key {
        Key::KeypadDigit(n @ 0..=9) => b'p' + n,
        Key::KeypadDecimal | Key::KeypadDelete => b'n',
        Key::KeypadDivide => b'o',
        Key::KeypadMultiply => b'j',
        Key::KeypadSubtract => b'm',
        Key::KeypadAdd => b'k',
        Key::KeypadEnter => b'M',
        Key::KeypadEqual => b'X',
        Key::KeypadSeparator => b'l',
        Key::KeypadInsert => b'p',
        Key::KeypadEnd => b'q',
        Key::KeypadDown => b'r',
        Key::KeypadPageDown => b's',
        Key::KeypadLeft => b't',
        Key::KeypadBegin => b'u',
        Key::KeypadRight => b'v',
        Key::KeypadHome => b'w',
        Key::KeypadUp => b'x',
        Key::KeypadPageUp => b'y',
        _ => return None,
    })
}

fn control(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        ' ' | '@' | '`' | '2' => Some(0),
        '[' | '3' => Some(27),
        '\\' | '4' => Some(28),
        ']' | '5' => Some(29),
        '^' | '~' | '6' => Some(30),
        '_' | '/' | '7' => Some(31),
        '?' | '8' => Some(127),
        _ => None,
    }
}

fn csi_key(number: usize, suffix: u8, modifiers: Modifiers, output: &mut Vec<u8>) {
    output.extend_from_slice(b"\x1b[");
    decimal(number, output);
    let value = modifiers.legacy_keyboard();
    if value != 1 {
        output.push(b';');
        decimal(value, output);
    }
    output.push(suffix);
}

fn ss3_or_modified(suffix: u8, modifiers: Modifiers, output: &mut Vec<u8>) {
    if modifiers.legacy_keyboard() == 1 {
        output.extend_from_slice(&[0x1b, b'O', suffix]);
    } else {
        csi_key(1, suffix, modifiers, output);
    }
}

fn printable(c: char) -> bool {
    !c.is_control()
}

fn single_printable(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let first = chars.next()?;
    (printable(first) && chars.next().is_none()).then_some(first)
}

fn printable_text(text: &str, output: &mut Vec<u8>) {
    if text.chars().all(printable) {
        output.extend_from_slice(text.as_bytes());
    } else {
        for c in text.chars().filter(|c| printable(*c)) {
            character(c, output);
        }
    }
}

fn character(c: char, output: &mut Vec<u8>) {
    let mut bytes = [0; 4];
    output.extend_from_slice(c.encode_utf8(&mut bytes).as_bytes());
}

fn codepoints(text: &str, output: &mut Vec<u8>) {
    let mut first = true;
    for c in text.chars().filter(|c| printable(*c)) {
        if !first {
            output.push(b':');
        }
        decimal(c as usize, output);
        first = false;
    }
}

fn decimal(mut value: usize, output: &mut Vec<u8>) {
    let mut digits = [0; 20];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    output.extend_from_slice(&digits[at..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes(flags: u8) -> Modes {
        Modes {
            kitty_keyboard: flags,
            ..Modes::default()
        }
    }

    #[test]
    fn legacy_text_control_and_application_cursor() {
        let mut m = modes(0);
        assert_eq!(
            encode_key(
                Key::Character('c'),
                Modifiers(4),
                KeyEvent::Press,
                "\u{3}",
                &m
            ),
            b"\x03"
        );
        assert_eq!(
            encode_key(Key::Character('x'), Modifiers(2), KeyEvent::Press, "x", &m),
            b"\x1bx"
        );
        assert_eq!(
            encode_key(Key::Up, Modifiers(0), KeyEvent::Press, "", &m),
            b"\x1b[A"
        );
        m.app_cursor = true;
        assert_eq!(
            encode_key(Key::Up, Modifiers(0), KeyEvent::Press, "", &m),
            b"\x1bOA"
        );
        assert_eq!(
            encode_key(Key::Up, Modifiers(5), KeyEvent::Press, "", &m),
            b"\x1b[1;6A"
        );
        assert!(encode_key(Key::Up, Modifiers(0), KeyEvent::Release, "", &m).is_empty());
    }

    #[test]
    fn kitty_disambiguates_without_breaking_reset_keys() {
        let m = modes(1);
        assert_eq!(
            encode_key(
                Key::Character('c'),
                Modifiers(4),
                KeyEvent::Press,
                "\u{3}",
                &m
            ),
            b"\x1b[99;5u"
        );
        assert_eq!(
            encode_key(Key::Escape, Modifiers(0), KeyEvent::Press, "", &m),
            b"\x1b[27u"
        );
        assert_eq!(
            encode_key(Key::Enter, Modifiers(0), KeyEvent::Press, "\r", &m),
            b"\r"
        );
        assert_eq!(
            encode_key(Key::Tab, Modifiers(0), KeyEvent::Press, "\t", &m),
            b"\t"
        );
        assert_eq!(
            encode_key(Key::Backspace, Modifiers(0), KeyEvent::Press, "", &m),
            b"\x7f"
        );
        assert_eq!(
            encode_key(Key::Character('a'), Modifiers(1), KeyEvent::Press, "A", &m),
            b"A"
        );
        assert_eq!(
            encode_key(Key::Function(3), Modifiers(0), KeyEvent::Press, "", &m),
            b"\x1b[13~"
        );
    }

    #[test]
    fn kitty_event_reporting_and_release_exceptions() {
        let m = modes(3);
        assert_eq!(
            encode_key(Key::Up, Modifiers(0), KeyEvent::Repeat, "", &m),
            b"\x1b[1;1:2A"
        );
        assert_eq!(
            encode_key(Key::Up, Modifiers(0), KeyEvent::Release, "", &m),
            b"\x1b[1;1:3A"
        );
        assert!(encode_key(Key::Enter, Modifiers(0), KeyEvent::Release, "", &m).is_empty());
        assert!(
            encode_key(Key::Character('a'), Modifiers(0), KeyEvent::Release, "", &m).is_empty()
        );
        assert_eq!(
            encode_key(Key::Character('c'), Modifiers(4), KeyEvent::Release, "", &m),
            b"\x1b[99;5:3u"
        );
        assert!(
            encode_key(Key::KeypadDigit(1), Modifiers(0), KeyEvent::Release, "", &m).is_empty()
        );
        assert_eq!(
            encode_key(Key::Enter, Modifiers(0), KeyEvent::Release, "", &modes(11)),
            b"\x1b[13;1:3u"
        );
    }

    #[test]
    fn kitty_alternate_keys_and_complete_associated_text() {
        assert_eq!(
            encode_key(
                Key::Character('a'),
                Modifiers(1),
                KeyEvent::Press,
                "A",
                &modes(24)
            ),
            b"\x1b[97;2;65u"
        );
        assert_eq!(
            encode_key(
                Key::Character('a'),
                Modifiers(5),
                KeyEvent::Press,
                "A",
                &modes(31)
            ),
            b"\x1b[97:65;6:1;65u"
        );
        let a = KeyAlternates {
            shifted: None,
            base_layout: Some('c'),
        };
        assert_eq!(
            encode_key_with_alternates(
                Key::Character('с'),
                Modifiers(4),
                KeyEvent::Press,
                "",
                a,
                &modes(5)
            ),
            b"\x1b[1089::99;5u"
        );
        let a = KeyAlternates {
            shifted: Some('A'),
            base_layout: Some('a'),
        };
        assert_eq!(
            encode_key_with_alternates(
                Key::Character('a'),
                Modifiers(4),
                KeyEvent::Press,
                "",
                a,
                &modes(5)
            ),
            b"\x1b[97::97;5u"
        );
        assert_eq!(
            encode_key(
                Key::Character('a'),
                Modifiers(0),
                KeyEvent::Press,
                "a\u{301}",
                &modes(24)
            ),
            b"\x1b[97;1;97:769u"
        );
    }

    #[test]
    fn ime_commits_are_never_truncated_or_discarded() {
        for flags in [0, 1, 3, 8] {
            assert_eq!(
                encode_key(
                    Key::Unidentified,
                    Modifiers(0),
                    KeyEvent::Press,
                    "你好世界",
                    &modes(flags)
                ),
                "你好世界".as_bytes()
            );
        }
        assert_eq!(encode_text("你好", &modes(24)), b"\x1b[0;1;20320:22909u");
        assert_eq!(
            encode_key(
                Key::Character('x'),
                Modifiers(0),
                KeyEvent::Press,
                "你好",
                &modes(0)
            ),
            "你好".as_bytes()
        );
    }

    #[test]
    fn modifier_keys_keypad_and_lock_state() {
        let key = Key::Modifier(ModifierKey::LeftControl);
        assert!(encode_key(key, Modifiers(4), KeyEvent::Press, "", &modes(1)).is_empty());
        assert_eq!(
            encode_key(key, Modifiers(4), KeyEvent::Press, "", &modes(10)),
            b"\x1b[57442;5:1u"
        );
        assert_eq!(
            encode_key(key, Modifiers(0), KeyEvent::Release, "", &modes(10)),
            b"\x1b[57442;1:3u"
        );
        assert_eq!(
            encode_key(
                Key::Character('a'),
                Modifiers(64),
                KeyEvent::Press,
                "A",
                &modes(24)
            ),
            b"\x1b[97;65;65u"
        );
        assert_eq!(
            encode_key(
                Key::KeypadDigit(1),
                Modifiers(0),
                KeyEvent::Press,
                "1",
                &modes(1)
            ),
            b"1"
        );
        let m = Modes {
            app_keypad: true,
            ..modes(0)
        };
        assert_eq!(
            encode_key(Key::KeypadDigit(1), Modifiers(0), KeyEvent::Press, "1", &m),
            b"\x1bOq"
        );
        assert!(encode_key(Key::KeypadDigit(15), Modifiers(0), KeyEvent::Press, "", &m).is_empty());
    }

    #[test]
    fn printable_keypad_stays_text_without_report_all() {
        let keys = [
            (Key::KeypadDigit(0), "0"),
            (Key::KeypadDigit(1), "1"),
            (Key::KeypadDigit(2), "2"),
            (Key::KeypadDigit(3), "3"),
            (Key::KeypadDigit(4), "4"),
            (Key::KeypadDigit(5), "5"),
            (Key::KeypadDigit(6), "6"),
            (Key::KeypadDigit(7), "7"),
            (Key::KeypadDigit(8), "8"),
            (Key::KeypadDigit(9), "9"),
            (Key::KeypadDecimal, "."),
            (Key::KeypadDecimal, ","),
            (Key::KeypadAdd, "+"),
            (Key::KeypadSubtract, "-"),
            (Key::KeypadMultiply, "*"),
            (Key::KeypadDivide, "/"),
            (Key::KeypadEqual, "="),
            (Key::KeypadSeparator, ","),
        ];
        for flags in 0..REPORT_ALL {
            for (key, text) in keys {
                for locks in [
                    0,
                    Modifiers::NUM_LOCK,
                    Modifiers::NUM_LOCK | Modifiers::CAPS_LOCK,
                ] {
                    for event in [KeyEvent::Press, KeyEvent::Repeat] {
                        assert_eq!(
                            encode_key(key, Modifiers(locks), event, text, &modes(flags)),
                            text.as_bytes(),
                            "flags={flags}, key={key:?}, locks={locks}, event={event:?}"
                        );
                    }
                    assert!(
                        encode_key(key, Modifiers(locks), KeyEvent::Release, "", &modes(flags))
                            .is_empty()
                    );
                }
            }
        }
    }

    #[test]
    fn application_keypad_keeps_disambiguated_events() {
        let mut m = Modes {
            app_keypad: true,
            ..modes(1)
        };
        let key = Key::KeypadDigit(1);
        assert_eq!(
            encode_key(key, Modifiers(128), KeyEvent::Press, "1", &m),
            b"\x1b[57400u"
        );
        m.kitty_keyboard = 3;
        for (event, expected) in [
            (KeyEvent::Press, b"\x1b[57400;1:1u".as_slice()),
            (KeyEvent::Repeat, b"\x1b[57400;1:2u".as_slice()),
            (KeyEvent::Release, b"\x1b[57400;1:3u".as_slice()),
        ] {
            assert_eq!(encode_key(key, Modifiers(128), event, "1", &m), expected);
        }
    }

    #[test]
    fn keypad_identity_is_preserved_when_requested() {
        let digit = Key::KeypadDigit(1);
        for (modifier, expected) in [
            (Modifiers::SUPER, b"\x1b[57400;9u".as_slice()),
            (Modifiers::HYPER, b"\x1b[57400;17u".as_slice()),
            (Modifiers::META, b"\x1b[57400;33u".as_slice()),
        ] {
            for flags in [0, 1] {
                assert_eq!(
                    encode_key(
                        digit,
                        Modifiers(modifier),
                        KeyEvent::Press,
                        "1",
                        &modes(flags)
                    ),
                    expected
                );
            }
        }
        assert_eq!(
            encode_key(
                digit,
                Modifiers(Modifiers::CTRL),
                KeyEvent::Press,
                "",
                &modes(1)
            ),
            b"\x1b[57400;5u"
        );
        assert_eq!(
            encode_key(
                Key::KeypadLeft,
                Modifiers(0),
                KeyEvent::Press,
                "",
                &modes(1)
            ),
            b"\x1b[57417u"
        );
        assert_eq!(
            encode_key(digit, Modifiers(128), KeyEvent::Press, "1", &modes(8)),
            b"\x1b[57400;129u"
        );
        for (event, expected) in [
            (KeyEvent::Press, b"\x1b[57400;129:1u".as_slice()),
            (KeyEvent::Repeat, b"\x1b[57400;129:2u".as_slice()),
            (KeyEvent::Release, b"\x1b[57400;129:3u".as_slice()),
        ] {
            assert_eq!(
                encode_key(digit, Modifiers(128), event, "1", &modes(10)),
                expected
            );
        }
        assert_eq!(
            encode_key(digit, Modifiers(128), KeyEvent::Press, "1", &modes(24)),
            b"\x1b[57400;129;49u"
        );
    }

    #[test]
    fn lock_keys_are_silent_until_the_child_requests_enhanced_events() {
        for key in [Key::CapsLock, Key::NumLock, Key::ScrollLock] {
            for flags in [
                0,
                REPORT_ALTERNATES,
                REPORT_TEXT,
                REPORT_ALTERNATES | REPORT_TEXT,
            ] {
                for event in [KeyEvent::Press, KeyEvent::Repeat, KeyEvent::Release] {
                    for modifier in [0, 1, 64, 128] {
                        assert!(
                            encode_key(key, Modifiers(modifier), event, "", &modes(flags))
                                .is_empty()
                        );
                    }
                }
            }
        }
        assert_eq!(
            encode_key(Key::NumLock, Modifiers(0), KeyEvent::Press, "", &modes(1)),
            b"\x1b[57360u"
        );
        assert_eq!(
            encode_key(
                Key::NumLock,
                Modifiers(128),
                KeyEvent::Press,
                "",
                &modes(10)
            ),
            b"\x1b[57360;129:1u"
        );
        assert_eq!(
            encode_key(
                Key::NumLock,
                Modifiers(128),
                KeyEvent::Release,
                "",
                &modes(10)
            ),
            b"\x1b[57360;129:3u"
        );
        assert_eq!(
            encode_key(
                Key::KeypadDigit(0),
                Modifiers(128),
                KeyEvent::Press,
                "0",
                &modes(0)
            ),
            b"0"
        );
        assert_eq!(
            encode_key(
                Key::KeypadInsert,
                Modifiers(0),
                KeyEvent::Press,
                "",
                &modes(0)
            ),
            b"\x1b[2~"
        );
    }

    #[test]
    fn sgr_encoding_does_not_itself_enable_tracking() {
        let mut m = Modes {
            sgr_mouse: true,
            ..modes(0)
        };
        let e = MouseEvent::Press {
            button: 0,
            col: 0,
            row: 0,
        };
        assert!(encode_mouse(e, Modifiers(0), &m).is_empty());
        m.mouse_tracking = 1000;
        assert_eq!(encode_mouse(e, Modifiers(0), &m), b"\x1b[<0;1;1M");
        assert_eq!(
            encode_mouse(
                MouseEvent::Release {
                    button: 2,
                    col: 3,
                    row: 4
                },
                Modifiers(5),
                &m
            ),
            b"\x1b[<22;4;5m"
        );
        assert_eq!(
            encode_mouse(
                MouseEvent::Scroll {
                    up: true,
                    col: 0,
                    row: 0
                },
                Modifiers(0),
                &m
            ),
            b"\x1b[<64;1;1M"
        );
    }

    #[test]
    fn tracking_modes_filter_motion_and_legacy_coordinates_are_bounded() {
        let drag = MouseEvent::Motion {
            button: Some(0),
            col: 1,
            row: 2,
        };
        let hover = MouseEvent::Motion {
            button: None,
            col: 1,
            row: 2,
        };
        let mut m = Modes {
            sgr_mouse: true,
            mouse_tracking: 1000,
            ..modes(0)
        };
        assert!(encode_mouse(drag, Modifiers(0), &m).is_empty());
        m.mouse_tracking = 1002;
        assert_eq!(encode_mouse(drag, Modifiers(0), &m), b"\x1b[<32;2;3M");
        assert!(encode_mouse(hover, Modifiers(0), &m).is_empty());
        m.mouse_tracking = 1003;
        assert_eq!(encode_mouse(hover, Modifiers(0), &m), b"\x1b[<35;2;3M");
        m.sgr_mouse = false;
        assert_eq!(
            encode_mouse(
                MouseEvent::Release {
                    button: 0,
                    col: 0,
                    row: 0
                },
                Modifiers(0),
                &m
            ),
            [27, 91, 77, 35, 33, 33]
        );
        assert!(
            encode_mouse(
                MouseEvent::Press {
                    button: 0,
                    col: 223,
                    row: 0
                },
                Modifiers(0),
                &m
            )
            .is_empty()
        );
        m.sgr_mouse = true;
        assert!(
            encode_mouse(
                MouseEvent::Press {
                    button: 0,
                    col: usize::MAX,
                    row: 0
                },
                Modifiers(0),
                &m
            )
            .is_empty()
        );
    }

    #[test]
    fn clipboard_cannot_inject_a_bracket_end_or_control_keys() {
        let m = Modes {
            bracketed_paste: true,
            ..modes(0)
        };
        assert_eq!(
            encode_paste("a\x1b[201~\x03\u{9b}201~\nb\tc", &m),
            b"\x1b[200~a[201~201~\nb\tc\x1b[201~"
        );
        assert_eq!(encode_paste("你好\r\n", &modes(0)), "你好\r\n".as_bytes());
    }

    #[test]
    fn focus_and_reusable_buffer_append() {
        let m = Modes {
            focus_events: true,
            ..modes(0)
        };
        assert_eq!(encode_focus(true, &m), b"\x1b[I");
        assert_eq!(encode_focus(false, &m), b"\x1b[O");
        assert!(encode_focus(true, &modes(0)).is_empty());
        let mut buffer = Vec::with_capacity(64);
        buffer.extend_from_slice(b"prefix");
        let capacity = buffer.capacity();
        encode_key_into(Key::Up, Modifiers(0), KeyEvent::Press, "", &m, &mut buffer);
        assert_eq!(buffer, b"prefix\x1b[A");
        assert_eq!(buffer.capacity(), capacity);
    }

    #[test]
    fn legacy_ctrl_shift_shortcuts_do_not_collapse_to_control_characters() {
        for flags in [0, REPORT_ALTERNATES, REPORT_TEXT] {
            let m = modes(flags);
            for (key, expected) in [
                ('i', b"\x1b[105;6u".as_slice()),
                ('3', b"\x1b[51;6u".as_slice()),
                (';', b"\x1b[59;6u".as_slice()),
            ] {
                for locks in [0, Modifiers::CAPS_LOCK, Modifiers::NUM_LOCK, 192] {
                    let event = encode_key_with_alternates(
                        Key::Character(key),
                        Modifiers(Modifiers::CTRL | Modifiers::SHIFT | locks),
                        KeyEvent::Press,
                        "",
                        KeyAlternates::default(),
                        &m,
                    );
                    assert_eq!(event, expected, "flags={flags} key={key} locks={locks}");
                }
            }
            assert_eq!(
                encode_key(Key::Character('i'), Modifiers(7), KeyEvent::Repeat, "", &m),
                if flags & REPORT_ALTERNATES != 0 {
                    b"\x1b[105:73;8u".as_slice()
                } else {
                    b"\x1b[105;8u".as_slice()
                }
            );
            assert!(
                encode_key(Key::Character('i'), Modifiers(5), KeyEvent::Release, "", &m).is_empty()
            );
        }
        // Legacy control and reset-key exceptions are intentionally retained.
        for (key, modifiers, expected) in [
            (Key::Character('i'), 4, b"\t".as_slice()),
            (Key::Character(' '), 5, b"\0".as_slice()),
            (Key::Character(' '), 7, b"\x1b\0".as_slice()),
            (Key::Character('~'), 4, b"\x1e".as_slice()),
            (Key::Enter, 5, b"\r".as_slice()),
            (Key::Tab, 5, b"\x1b[Z".as_slice()),
            (Key::Backspace, 5, b"\x08".as_slice()),
        ] {
            assert_eq!(
                encode_key(key, Modifiers(modifiers), KeyEvent::Press, "", &modes(0)),
                expected
            );
        }
    }

    #[test]
    fn alternate_fields_and_release_text_obey_negotiated_flags() {
        let alternatives = KeyAlternates {
            shifted: Some('Ф'),
            base_layout: Some('a'),
        };
        assert_eq!(
            encode_key_with_alternates(
                Key::Character('ф'),
                Modifiers(5),
                KeyEvent::Repeat,
                "Ф",
                alternatives,
                &modes(31)
            ),
            b"\x1b[1092:1060:97;6:2;1060u"
        );
        assert_eq!(
            encode_key_with_alternates(
                Key::Character('ф'),
                Modifiers(5),
                KeyEvent::Release,
                "stale committed text",
                alternatives,
                &modes(31)
            ),
            b"\x1b[1092:1060:97;6:3u"
        );
        assert_eq!(
            encode_key_with_alternates(
                Key::Character('ф'),
                Modifiers(4),
                KeyEvent::Press,
                "",
                alternatives,
                &modes(5)
            ),
            b"\x1b[1092::97;5u"
        );
        // The alternate-key flag alone cannot turn ordinary text into a CSI.
        assert_eq!(
            encode_key_with_alternates(
                Key::Character('ф'),
                Modifiers(1),
                KeyEvent::Press,
                "Ф",
                alternatives,
                &modes(REPORT_ALTERNATES)
            ),
            "Ф".as_bytes()
        );
    }

    #[test]
    fn all_keyboard_flag_combinations_preserve_reset_key_release_rules() {
        for flags in 0..32 {
            for (key, legacy, code) in [
                (Key::Enter, b'\r', 13),
                (Key::Tab, b'\t', 9),
                (Key::Backspace, 127, 127),
            ] {
                let m = modes(flags);
                let press = encode_key(key, Modifiers(0), KeyEvent::Press, "", &m);
                if flags & REPORT_ALL == 0 {
                    assert_eq!(press, [legacy], "flags={flags} key={key:?}");
                } else if flags & REPORT_EVENTS != 0 {
                    assert_eq!(press, alloc::format!("\x1b[{code};1:1u").as_bytes());
                } else {
                    assert_eq!(press, alloc::format!("\x1b[{code}u").as_bytes());
                }
                let release = encode_key(key, Modifiers(0), KeyEvent::Release, "", &m);
                if flags & (REPORT_ALL | REPORT_EVENTS) == (REPORT_ALL | REPORT_EVENTS) {
                    assert_eq!(release, alloc::format!("\x1b[{code};1:3u").as_bytes());
                } else {
                    assert!(release.is_empty(), "flags={flags} key={key:?}");
                }
            }
        }
    }

    #[test]
    fn x10_tracking_sends_only_primary_presses_without_modifiers() {
        for sgr_mouse in [false, true] {
            let m = Modes {
                mouse_tracking: 9,
                sgr_mouse,
                ..Modes::default()
            };
            for button in 0..=2 {
                let bytes = encode_mouse(
                    MouseEvent::Press {
                        button,
                        col: 4,
                        row: 6,
                    },
                    Modifiers(255),
                    &m,
                );
                if sgr_mouse {
                    assert_eq!(bytes, alloc::format!("\x1b[<{button};5;7M").as_bytes());
                } else {
                    assert_eq!(bytes, [27, b'[', b'M', 32 + button, 37, 39]);
                }
            }
            for event in [
                MouseEvent::Release {
                    button: 0,
                    col: 0,
                    row: 0,
                },
                MouseEvent::Motion {
                    button: Some(0),
                    col: 1,
                    row: 1,
                },
                MouseEvent::Motion {
                    button: None,
                    col: 1,
                    row: 1,
                },
                MouseEvent::Scroll {
                    up: true,
                    col: 0,
                    row: 0,
                },
                MouseEvent::Press {
                    button: 3,
                    col: 0,
                    row: 0,
                },
                MouseEvent::Press {
                    button: 7,
                    col: 0,
                    row: 0,
                },
            ] {
                assert!(
                    encode_mouse(event, Modifiers(0), &m).is_empty(),
                    "{event:?}"
                );
            }
        }
    }

    #[test]
    fn all_xterm_mouse_buttons_have_distinct_press_codes() {
        let codes = [0, 1, 2, 64, 65, 66, 67, 128, 129, 130, 131];
        for sgr_mouse in [false, true] {
            let m = Modes {
                mouse_tracking: 1000,
                sgr_mouse,
                ..Modes::default()
            };
            for (button, code) in codes.into_iter().enumerate() {
                let bytes = encode_mouse(
                    MouseEvent::Press {
                        button: button as u8,
                        col: 1,
                        row: 2,
                    },
                    Modifiers(7),
                    &m,
                );
                if sgr_mouse {
                    assert_eq!(bytes, alloc::format!("\x1b[<{};2;3M", code | 28).as_bytes());
                } else {
                    assert_eq!(bytes, [27, b'[', b'M', (code | 28) + 32, 34, 35]);
                }
            }
            for (up, button) in [(true, 3), (false, 4)] {
                assert_eq!(
                    encode_mouse(MouseEvent::Scroll { up, col: 0, row: 0 }, Modifiers(0), &m),
                    encode_mouse(
                        MouseEvent::Press {
                            button,
                            col: 0,
                            row: 0
                        },
                        Modifiers(0),
                        &m
                    )
                );
            }
        }
    }

    #[test]
    fn wheel_releases_and_motion_are_suppressed_but_extra_buttons_can_drag() {
        for sgr_mouse in [false, true] {
            let mut m = Modes {
                mouse_tracking: 1002,
                sgr_mouse,
                ..Modes::default()
            };
            for button in 3..=6 {
                assert!(
                    encode_mouse(
                        MouseEvent::Release {
                            button,
                            col: 0,
                            row: 0
                        },
                        Modifiers(0),
                        &m
                    )
                    .is_empty()
                );
                assert!(
                    encode_mouse(
                        MouseEvent::Motion {
                            button: Some(button),
                            col: 0,
                            row: 0
                        },
                        Modifiers(0),
                        &m
                    )
                    .is_empty()
                );
            }
            for button in 7..=10 {
                let motion = encode_mouse(
                    MouseEvent::Motion {
                        button: Some(button),
                        col: 0,
                        row: 0,
                    },
                    Modifiers(4),
                    &m,
                );
                let release = encode_mouse(
                    MouseEvent::Release {
                        button,
                        col: 0,
                        row: 0,
                    },
                    Modifiers(4),
                    &m,
                );
                if sgr_mouse {
                    assert_eq!(
                        motion,
                        alloc::format!("\x1b[<{};1;1M", 176 + button - 7).as_bytes()
                    );
                    assert_eq!(
                        release,
                        alloc::format!("\x1b[<{};1;1m", 144 + button - 7).as_bytes()
                    );
                } else {
                    assert_eq!(motion, [27, b'[', b'M', 208 + button - 7, 33, 33]);
                    assert_eq!(release, [27, b'[', b'M', 51, 33, 33]);
                }
            }
            assert!(
                encode_mouse(
                    MouseEvent::Motion {
                        button: None,
                        col: 0,
                        row: 0
                    },
                    Modifiers(0),
                    &m
                )
                .is_empty()
            );
            m.mouse_tracking = 1003;
            assert!(
                !encode_mouse(
                    MouseEvent::Motion {
                        button: None,
                        col: 0,
                        row: 0
                    },
                    Modifiers(0),
                    &m
                )
                .is_empty()
            );
        }
    }

    #[test]
    fn invalid_mouse_inputs_never_append_partial_bytes() {
        let mut output = Vec::with_capacity(128);
        output.extend_from_slice(b"already queued");
        let capacity = output.capacity();
        let m = Modes {
            mouse_tracking: 1003,
            sgr_mouse: true,
            ..Modes::default()
        };
        for button in 11..=u8::MAX {
            for event in [
                MouseEvent::Press {
                    button,
                    col: 0,
                    row: 0,
                },
                MouseEvent::Release {
                    button,
                    col: 0,
                    row: 0,
                },
                MouseEvent::Motion {
                    button: Some(button),
                    col: 0,
                    row: 0,
                },
            ] {
                encode_mouse_into(event, Modifiers(255), &m, &mut output);
            }
        }
        for event in [
            MouseEvent::Press {
                button: 0,
                col: usize::MAX,
                row: 0,
            },
            MouseEvent::Release {
                button: 7,
                col: 0,
                row: usize::MAX,
            },
        ] {
            encode_mouse_into(event, Modifiers(255), &m, &mut output);
        }
        assert_eq!(output, b"already queued");
        assert_eq!(output.capacity(), capacity);
        encode_mouse_into(
            MouseEvent::Press {
                button: 10,
                col: 222,
                row: 222,
            },
            Modifiers(255),
            &Modes {
                sgr_mouse: false,
                ..m
            },
            &mut output,
        );
        assert_eq!(&output[14..], &[27, b'[', b'M', 191, 255, 255]);
        assert_eq!(output.capacity(), capacity);
    }
}
