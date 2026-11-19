//! AppKit keyboard, composition, pointer and clipboard input.
use super::{
    CellPosition, Host, Id, MAX_INPUT, MAX_MARKED, NIL, NOT_FOUND, PendingKey, Point, Range, Rect,
    Sel, Selection, ffi::*, host_ptr, wake,
};
use crate::os;
use alloc::{string::String, vec::Vec};
use zterm::input::{self, Key, KeyEvent, ModifierKey, Modifiers, MouseEvent};

impl Host {
    pub(super) fn send_key(&mut self, key: Key, mods: Modifiers, event: KeyEvent, text: &str) {
        self.encoded.clear();
        input::encode_key_into(
            key,
            mods,
            event,
            text,
            &self.core.terminal.modes().input(),
            &mut self.encoded,
        );
        let encoded = core::mem::take(&mut self.encoded);
        let sent = !encoded.is_empty();
        self.queue(&encoded);
        self.encoded = encoded;
        if sent && event != KeyEvent::Release && !matches!(key, Key::Modifier(_)) {
            self.history = 0;
            self.selection = None;
            self.blink_visible = true;
            self.blink_at =
                os::milliseconds().saturating_add(self.appearance.cursor_blink_interval);
            self.force = true;
        }
    }
    pub(super) fn send_text(&mut self, text: &str) {
        self.encoded.clear();
        input::encode_text_into(text, &self.core.terminal.modes().input(), &mut self.encoded);
        let encoded = core::mem::take(&mut self.encoded);
        self.queue(&encoded);
        self.encoded = encoded;
        self.history = 0;
        self.selection = None;
        self.force = true;
    }
    pub(super) fn send_mouse(&mut self, event: MouseEvent, mods: Modifiers) {
        self.encoded.clear();
        input::encode_mouse_into(
            event,
            mods,
            &self.core.terminal.modes().input(),
            &mut self.encoded,
        );
        let encoded = core::mem::take(&mut self.encoded);
        self.queue(&encoded);
        self.encoded = encoded;
    }
    pub(super) fn position(&self, event: Id) -> CellPosition {
        let point = send!(event, c"locationInWindow", Point);
        let point = send!(self.view, c"convertPoint:fromView:", Point, Point => point, Id => NIL);
        let bounds = rect0(self.view, c"bounds");
        let x = (point.x - self.appearance.padding as f64).max(0.0);
        let y = (bounds.size.height - point.y - self.appearance.padding as f64).max(0.0);
        CellPosition {
            col: (x / self.renderer.cell_width) as usize,
            row: (y / self.renderer.cell_height) as usize,
        }
    }
    pub(super) fn selected_text(&self) -> String {
        let Some(selection) = self.selection else {
            return String::new();
        };
        ztty::selection::text(
            &self.core.terminal,
            (selection.start.col, selection.start.row),
            (selection.end.col, selection.end.row),
            self.history,
        )
    }
}

pub(super) fn modifiers(event: Id) -> Modifiers {
    let flags = send!(event, c"modifierFlags", usize);
    Modifiers(
        u8::from(flags & (1 << 17) != 0)
            | (u8::from(flags & (1 << 19) != 0) << 1)
            | (u8::from(flags & (1 << 18) != 0) << 2)
            | (u8::from(flags & (1 << 20) != 0) << 3)
            | (u8::from(flags & (1 << 16) != 0) << 6),
    )
}

pub(super) fn event_key(event: Id) -> Key {
    let code = send!(event, c"keyCode", u16);
    match code {
        36 => Key::Enter,
        48 => Key::Tab,
        51 => Key::Backspace,
        53 => Key::Escape,
        123 => Key::Left,
        124 => Key::Right,
        125 => Key::Down,
        126 => Key::Up,
        115 => Key::Home,
        119 => Key::End,
        116 => Key::PageUp,
        121 => Key::PageDown,
        117 => Key::Delete,
        122 => Key::Function(1),
        120 => Key::Function(2),
        99 => Key::Function(3),
        118 => Key::Function(4),
        96 => Key::Function(5),
        97 => Key::Function(6),
        98 => Key::Function(7),
        100 => Key::Function(8),
        101 => Key::Function(9),
        109 => Key::Function(10),
        103 => Key::Function(11),
        111 => Key::Function(12),
        105 => Key::Function(13),
        107 => Key::Function(14),
        113 => Key::Function(15),
        106 => Key::Function(16),
        64 => Key::Function(17),
        79 => Key::Function(18),
        80 => Key::Function(19),
        90 => Key::Function(20),
        65 => Key::KeypadDecimal,
        67 => Key::KeypadMultiply,
        69 => Key::KeypadAdd,
        75 => Key::KeypadDivide,
        76 => Key::KeypadEnter,
        78 => Key::KeypadSubtract,
        81 => Key::KeypadEqual,
        82 => Key::KeypadDigit(0),
        83..=89 => Key::KeypadDigit((code - 82) as u8),
        91 => Key::KeypadDigit(8),
        92 => Key::KeypadDigit(9),
        _ => {
            let string = send!(event, c"charactersIgnoringModifiers", Id);
            text_from_object(string, 128)
                .chars()
                .next()
                .filter(|c| !c.is_control() && (*c as u32) < 0xf700)
                .map(|c| Key::Character(c.to_ascii_lowercase()))
                .unwrap_or(Key::Unidentified)
        }
    }
}

pub(super) fn copy_selection() {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let text = unsafe { (&*pointer).selected_text() };
    if text.is_empty() {
        return;
    }
    let pasteboard = send!(class(c"NSPasteboard"), c"generalPasteboard", Id);
    send!(pasteboard, c"clearContents", isize);
    let string = ns_string(&text);
    let kind = ns_string("public.utf8-plain-text");
    send!(pasteboard, c"setString:forType:", bool, Id => string, Id => kind);
    release(kind);
    release(string);
}

pub(super) fn paste_clipboard() {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let pasteboard = send!(class(c"NSPasteboard"), c"generalPasteboard", Id);
    let kind = ns_string("public.utf8-plain-text");
    let string = send!(pasteboard, c"stringForType:", Id, Id => kind);
    let text = text_from_object(string, MAX_INPUT / 2);
    release(kind);
    if text.is_empty() {
        return;
    }
    unsafe {
        let host = &mut *pointer;
        let encoded = input::encode_paste(&text, &host.core.terminal.modes().input());
        host.queue(&encoded);
        host.history = 0;
        host.selection = None;
        host.force = true;
    }
    wake();
}

pub(super) unsafe extern "C" fn key_down(view: Id, _: Sel, event: Id) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let key = event_key(event);
    let mods = modifiers(event);
    let code = send!(event, c"keyCode", u16) as usize;
    let repeated = send!(event, c"isARepeat", bool);
    if code < 128 {
        unsafe { (*pointer).held[code] = Some(key) };
    }
    let command = mods.contains(Modifiers::SUPER);
    let control_shift =
        mods.0 & (Modifiers::CTRL | Modifiers::SHIFT) == (Modifiers::CTRL | Modifiers::SHIFT);
    if (command || control_shift) && key == Key::Character('c') {
        copy_selection();
        if code < 128 {
            unsafe { (*pointer).intercepted[code] = true };
        }
        return;
    }
    if (command || control_shift) && key == Key::Character('v') {
        paste_clipboard();
        if code < 128 {
            unsafe { (*pointer).intercepted[code] = true };
        }
        return;
    }
    if command && key == Key::Character('q') {
        unsafe { (*pointer).running = false };
        wake();
        return;
    }
    if mods.contains(Modifiers::SHIFT) && matches!(key, Key::PageUp | Key::PageDown) {
        unsafe {
            let host = &mut *pointer;
            let step = host.core.terminal.rows().saturating_sub(1).max(1);
            host.history = if key == Key::PageUp {
                host.history
                    .saturating_add(step)
                    .min(host.core.terminal.history_len())
            } else {
                host.history.saturating_sub(step)
            };
            host.selection = None;
            host.force = true;
            if code < 128 {
                host.intercepted[code] = true;
            }
        }
        return;
    }
    let transition = if repeated {
        KeyEvent::Repeat
    } else {
        KeyEvent::Press
    };
    let composing = unsafe { !(&*pointer).marked.is_empty() };
    if !composing
        && (mods.contains(Modifiers::CTRL | Modifiers::SUPER)
            || !matches!(key, Key::Character(_) | Key::Unidentified))
    {
        let text = text_from_object(send!(event, c"characters", Id), 65536);
        unsafe { (&mut *pointer).send_key(key, mods, transition, &text) };
        return;
    }
    unsafe {
        (*pointer).composing = composing;
        (*pointer).pending_key = Some(PendingKey {
            key,
            modifiers: mods,
            event: transition,
        });
    }
    // No Rust borrow of Host crosses AppKit's synchronous IME callbacks.
    let context = send!(view, c"inputContext", Id);
    let handled = send!(context, c"handleEvent:", bool, Id => event);
    if !handled {
        let text = text_from_object(send!(event, c"characters", Id), 65536);
        unsafe { (&mut *pointer).send_key(key, mods, transition, &text) };
    }
    unsafe {
        if handled && (*pointer).composing && code < 128 {
            (*pointer).intercepted[code] = true;
        }
        (*pointer).pending_key = None;
    }
}

pub(super) unsafe extern "C" fn key_up(_: Id, _: Sel, event: Id) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let code = send!(event, c"keyCode", u16) as usize;
    let mods = modifiers(event);
    unsafe {
        let host = &mut *pointer;
        let key = if code < 128 {
            host.held[code].take().unwrap_or_else(|| event_key(event))
        } else {
            event_key(event)
        };
        if code < 128 && core::mem::take(&mut host.intercepted[code]) {
            return;
        }
        host.send_key(key, mods, KeyEvent::Release, "");
    }
}

pub(super) unsafe extern "C" fn flags_changed(_: Id, _: Sel, event: Id) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let code = send!(event, c"keyCode", u16) as usize;
    let mods = modifiers(event);
    let (key, bit) = match code {
        56 => (Key::Modifier(ModifierKey::LeftShift), Modifiers::SHIFT),
        60 => (Key::Modifier(ModifierKey::RightShift), Modifiers::SHIFT),
        59 => (Key::Modifier(ModifierKey::LeftControl), Modifiers::CTRL),
        62 => (Key::Modifier(ModifierKey::RightControl), Modifiers::CTRL),
        58 => (Key::Modifier(ModifierKey::LeftAlt), Modifiers::ALT),
        61 => (Key::Modifier(ModifierKey::RightAlt), Modifiers::ALT),
        55 => (Key::Modifier(ModifierKey::LeftSuper), Modifiers::SUPER),
        54 => (Key::Modifier(ModifierKey::RightSuper), Modifiers::SUPER),
        57 => (Key::CapsLock, Modifiers::CAPS_LOCK),
        _ => return,
    };
    unsafe {
        let host = &mut *pointer;
        let pressed = mods.contains(bit) && host.held[code].is_none();
        host.held[code] = if pressed { Some(key) } else { None };
        host.send_key(
            key,
            mods,
            if pressed {
                KeyEvent::Press
            } else {
                KeyEvent::Release
            },
            "",
        );
    }
}

pub(super) unsafe extern "C" fn insert_text(_: Id, _: Sel, value: Id, _: Range) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let text = text_from_object(value, 65536);
    if text.is_empty() {
        return;
    }
    unsafe {
        let host = &mut *pointer;
        if let Some(pending) = host
            .pending_key
            .filter(|_| !host.composing && host.marked.is_empty())
        {
            host.send_key(pending.key, pending.modifiers, pending.event, &text);
        } else {
            host.send_text(&text);
        }
        host.marked.clear();
        host.marked_selection = Range::default();
        host.force = true;
    }
}

pub(super) unsafe extern "C" fn set_marked(_: Id, _: Sel, value: Id, selected: Range, _: Range) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let text = text_from_object(value, MAX_MARKED);
    unsafe {
        let host = &mut *pointer;
        let length = text.encode_utf16().count();
        host.marked = text;
        host.marked_selection = Range {
            location: selected.location.min(length),
            length: selected
                .length
                .min(length.saturating_sub(selected.location)),
        };
        host.composing = true;
        host.force = true;
    }
}

pub(super) unsafe extern "C" fn unmark(_: Id, _: Sel) {
    let pointer = host_ptr();
    if !pointer.is_null() {
        unsafe {
            (&mut *pointer).marked.clear();
            (*pointer).force = true;
        }
    }
}

pub(super) unsafe extern "C" fn has_marked(_: Id, _: Sel) -> bool {
    let pointer = host_ptr();
    !pointer.is_null() && unsafe { !(&*pointer).marked.is_empty() }
}

pub(super) unsafe extern "C" fn marked_range(_: Id, _: Sel) -> Range {
    let pointer = host_ptr();
    if !pointer.is_null() && unsafe { !(&*pointer).marked.is_empty() } {
        Range {
            location: 0,
            length: unsafe { (&*pointer).marked.encode_utf16().count() },
        }
    } else {
        Range {
            location: NOT_FOUND,
            length: 0,
        }
    }
}

pub(super) unsafe extern "C" fn selected_range(_: Id, _: Sel) -> Range {
    let pointer = host_ptr();
    if !pointer.is_null() && unsafe { !(&*pointer).marked.is_empty() } {
        unsafe { (*pointer).marked_selection }
    } else {
        Range {
            location: 0,
            length: 0,
        }
    }
}

pub(super) unsafe extern "C" fn valid_attributes(_: Id, _: Sel) -> Id {
    send!(class(c"NSArray"), c"array", Id)
}

pub(super) unsafe extern "C" fn attributed_substring(
    _: Id,
    _: Sel,
    range: Range,
    actual: *mut Range,
) -> Id {
    let pointer = host_ptr();
    if pointer.is_null() {
        return NIL;
    }
    let marked = unsafe { &(*pointer).marked };
    let units: Vec<u16> = marked.encode_utf16().collect();
    if range.location >= units.len() {
        return NIL;
    }
    let length = range.length.min(units.len() - range.location);
    if !actual.is_null() {
        unsafe {
            *actual = Range {
                location: range.location,
                length,
            }
        };
    }
    let string = send!(send!(class(c"NSString"), c"alloc", Id), c"initWithCharacters:length:", Id, *const u16 => units[range.location..].as_ptr(), usize => length);
    let attributed = send!(send!(class(c"NSAttributedString"), c"alloc", Id), c"initWithString:", Id, Id => string);
    release(string);
    send!(attributed, c"autorelease", Id)
}

pub(super) unsafe extern "C" fn character_index(_: Id, _: Sel, _: Point) -> usize {
    NOT_FOUND
}

pub(super) unsafe extern "C" fn first_rect(
    view: Id,
    _: Sel,
    range: Range,
    actual: *mut Range,
) -> Rect {
    let pointer = host_ptr();
    if pointer.is_null() {
        return Rect::default();
    }
    if !actual.is_null() {
        unsafe { *actual = range };
    }
    let (window, rect) = unsafe {
        let host = &*pointer;
        let (col, row) = host.core.terminal.cursor();
        let bounds = rect0(view, c"bounds");
        (
            host.window,
            Rect::new(
                host.appearance.padding as f64 + col as f64 * host.renderer.cell_width,
                bounds.size.height
                    - host.appearance.padding as f64
                    - (row + 1) as f64 * host.renderer.cell_height,
                host.renderer.cell_width,
                host.renderer.cell_height,
            ),
        )
    };
    let in_window = rect_convert(view, c"convertRect:toView:", rect, Some(NIL));
    rect_convert(window, c"convertRectToScreen:", in_window, None)
}

pub(super) unsafe extern "C" fn command(_: Id, _: Sel, _: Sel) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    unsafe {
        let host = &mut *pointer;
        if let Some(pending) = host.pending_key {
            host.send_key(pending.key, pending.modifiers, pending.event, "");
        }
    }
}

pub(super) unsafe extern "C" fn mouse(view: Id, _: Sel, event: Id) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let kind = send!(event, c"type", usize);
    let mods = modifiers(event);
    let position = unsafe { (&*pointer).position(event) };
    let button = match send!(event, c"buttonNumber", isize) {
        0 => 0,
        1 => 2,
        2 => 1,
        _ => return,
    };
    if kind == 1 {
        send!(send!(view, c"window", Id), c"makeFirstResponder:", bool, Id => view);
    }
    let pressing = matches!(kind, 1 | 3 | 25);
    let releasing = matches!(kind, 2 | 4 | 26);
    let uri = if mods.contains(Modifiers::SUPER | Modifiers::CTRL) && pressing && button == 0 {
        unsafe {
            let host = &*pointer;
            let col = position.col.min(host.core.terminal.cols() - 1);
            let row = position.row.min(host.core.terminal.rows() - 1);
            let cell = host.core.terminal.view_cell(col, row, host.history);
            host.core
                .terminal
                .cell_hyperlink(cell)
                .filter(|uri| {
                    uri.len() <= 8192
                        && !uri.chars().any(char::is_control)
                        && (uri.starts_with("https://")
                            || uri.starts_with("http://")
                            || uri.starts_with("mailto:"))
                })
                .map(String::from)
        }
    } else {
        None
    };
    if let Some(uri) = uri {
        unsafe {
            (*pointer).swallowed_buttons[button as usize] = true;
        }
        // Opening a URL can run framework code. No Host borrow crosses it.
        let string = ns_string(&uri);
        let url = send!(class(c"NSURL"), c"URLWithString:", Id, Id => string);
        send!(send!(class(c"NSWorkspace"), c"sharedWorkspace", Id), c"openURL:", bool, Id => url);
        release(string);
        return;
    }
    unsafe {
        let host = &mut *pointer;
        let col = position.col.min(host.core.terminal.cols() - 1);
        let row = position.row.min(host.core.terminal.rows() - 1);
        if releasing && core::mem::take(&mut host.swallowed_buttons[button as usize]) {
            return;
        }
        if host.core.terminal.modes().mouse_tracking != 0
            && !mods.contains(Modifiers::SHIFT)
            && host.history == 0
        {
            let report = if pressing {
                host.mouse_button = Some(button);
                MouseEvent::Press { button, col, row }
            } else if releasing {
                host.mouse_button = None;
                MouseEvent::Release { button, col, row }
            } else {
                MouseEvent::Motion {
                    button: host.mouse_button,
                    col,
                    row,
                }
            };
            host.send_mouse(report, mods);
        } else if button == 0 {
            if pressing {
                let position = CellPosition { col, row };
                host.selection = Some(Selection {
                    start: position,
                    end: position,
                });
                host.selecting = true;
            } else if host.selecting {
                if let Some(selection) = host.selection.as_mut() {
                    selection.end = CellPosition { col, row };
                }
                if releasing {
                    host.selecting = false;
                }
            }
            host.force = true;
        }
    }
}

pub(super) unsafe extern "C" fn wheel(_: Id, _: Sel, event: Id) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let delta = send!(event, c"scrollingDeltaY", f64);
    let precise = send!(event, c"hasPreciseScrollingDeltas", bool);
    let mods = modifiers(event);
    unsafe {
        let host = &mut *pointer;
        host.scroll_accumulator += if precise {
            delta / host.renderer.cell_height
        } else {
            delta
        };
        let lines = host.scroll_accumulator as i32;
        host.scroll_accumulator -= lines as f64;
        if lines == 0 {
            return;
        }
        if host.core.terminal.modes().mouse_tracking != 0
            && !mods.contains(Modifiers::SHIFT)
            && host.history == 0
        {
            let position = host.position(event);
            for _ in 0..lines.unsigned_abs().min(64) {
                host.send_mouse(
                    MouseEvent::Scroll {
                        up: lines > 0,
                        col: position.col.min(host.core.terminal.cols() - 1),
                        row: position.row.min(host.core.terminal.rows() - 1),
                    },
                    mods,
                );
            }
        } else {
            host.history = if lines > 0 {
                host.history
                    .saturating_add(lines as usize * 3)
                    .min(host.core.terminal.history_len())
            } else {
                host.history
                    .saturating_sub(lines.unsigned_abs() as usize * 3)
            };
            host.selection = None;
            host.force = true;
        }
    }
}
