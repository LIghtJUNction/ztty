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
    /// Zero disables tracking; supported values are 1000, 1002 and 1003.
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

/// Mouse coordinates are zero-based; buttons are 0=left, 1=middle, 2=right.
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
    let keypad = keypad_code(key);
    let enhanced = all
        || (flags & DISAMBIGUATE != 0
            && !legacy_exception
            && (key == Key::Escape
                || keypad.is_some()
                || !text_key
                || modifiers.contains(Modifiers::CTRL | Modifiers::ALT)))
        || (events && !text_key && !legacy_exception)
        || (matches!(key, Key::Character(_))
            && modifiers.contains(Modifiers::SUPER | Modifiers::HYPER | Modifiers::META));

    if event == KeyEvent::Release && (!events || !enhanced || (!all && legacy_exception)) {
        return;
    }

    if enhanced {
        if let Some((number, suffix)) = canonical(key) {
            let associated = all
                && flags & REPORT_TEXT != 0
                && event != KeyEvent::Release
                && text.chars().any(printable);
            // Text-producing keys omit lock state unless report-all is enabled.
            let modifier = if all || !text_key || modifiers.contains(Modifiers::CTRL) {
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
    if !matches!(modes.mouse_tracking, 1000 | 1002 | 1003) {
        return;
    }
    let (button, col, row, release) = match event {
        MouseEvent::Press { button, col, row } if button < 3 => (button, col, row, false),
        MouseEvent::Release { button, col, row } if button < 3 => (button, col, row, true),
        MouseEvent::Motion { button, col, row }
            if (modes.mouse_tracking == 1003
                || modes.mouse_tracking == 1002 && button.is_some())
                && button.is_none_or(|b| b < 3) =>
        {
            (32 + button.unwrap_or(3), col, row, false)
        }
        MouseEvent::Scroll { up, col, row } => (if up { 64 } else { 65 }, col, row, false),
        _ => return,
    };
    let (Some(x), Some(y)) = (col.checked_add(1), row.checked_add(1)) else {
        return;
    };
    if modes.sgr_mouse {
        output.extend_from_slice(b"\x1b[<");
        decimal((button | modifiers.mouse()) as usize, output);
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
            (button | modifiers.mouse()) + 32,
            x as u8 + 32,
            y as u8 + 32,
        ]);
    }
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
        Key::Modifier(_) | Key::Unidentified => {}
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
        '^' | '6' => Some(30),
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
        assert_eq!(
            encode_key(Key::KeypadDigit(1), Modifiers(0), KeyEvent::Release, "", &m),
            b"\x1b[57400;1:3u"
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
            b"\x1b[57400u"
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
}
