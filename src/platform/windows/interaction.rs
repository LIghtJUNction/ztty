//! Keyboard, pointer, IME, clipboard and selection handling.

use super::*;

impl Selection {
    pub(super) fn contains(self, col: usize, row: usize, cols: usize) -> bool {
        let a = self.start.1 * cols + self.start.0;
        let b = self.end.1 * cols + self.end.0;
        (a.min(b)..=a.max(b)).contains(&(row * cols + col))
    }
    pub(super) fn text(self, terminal: &Terminal, history: usize) -> String {
        ztty::selection::text(terminal, self.start, self.end, history)
    }
}

impl App {
    pub(super) fn enqueue(&mut self, index: usize, bytes: &[u8]) {
        if !self.panes[index].core.input.push(bytes) {
            os::print(2, b"ztty: input queue full; paste was rejected\n");
        }
    }
    pub(super) fn enqueue_encoded(&mut self) {
        self.enqueue_encoded_for(self.active);
    }
    pub(super) fn enqueue_encoded_for(&mut self, index: usize) {
        let encoded = core::mem::take(&mut self.encoded);
        self.enqueue(index, &encoded);
        self.encoded = encoded;
        self.encoded.clear();
    }
    pub(super) fn input(&mut self, key: Key, modifiers: Modifiers, event: KeyEvent, text: &str) {
        self.encoded.clear();
        input::encode_key_into(
            key,
            modifiers,
            event,
            text,
            &self.panes[self.active].core.terminal.modes().input(),
            &mut self.encoded,
        );
        // Silent modifier/release events must not undo a scroll or selection
        // action from the same shortcut. Negotiated keyboard events still
        // return to live output when the encoder actually sends them.
        if self.encoded.is_empty() {
            return;
        }
        self.enqueue_encoded();
        self.panes[self.active].history = 0;
        self.panes[self.active].selection = None;
        self.reset_cursor();
    }
    pub(super) fn reset_cursor(&mut self) {
        self.panes[self.active].cursor_on = true;
        self.panes[self.active].blink_deadline =
            os::milliseconds() + self.appearance.cursor_blink_interval;
        self.panes[self.active].dirty = true;
    }
    pub(super) fn cell_at(&self, index: usize, x: i32, y: i32) -> (usize, usize) {
        (
            (((x - self.panes[index].rect.x - self.appearance.padding).max(0) / self.fonts.width)
                as usize)
                .min(self.panes[index].core.terminal.cols() - 1),
            (((y - self.panes[index].rect.y - self.appearance.padding).max(0) / self.fonts.height)
                as usize)
                .min(self.panes[index].core.terminal.rows() - 1),
        )
    }
    pub(super) fn ime_position(&self) {
        let context = unsafe { ffi::ImmGetContext(self.hwnd) };
        if context.is_null() {
            return;
        }
        let (col, row) = self.panes[self.active].core.terminal.cursor();
        let form = ffi::CompositionForm {
            style: 2,
            position: ffi::Point {
                x: self.panes[self.active].rect.x
                    + self.appearance.padding
                    + col as i32 * self.fonts.width,
                y: self.panes[self.active].rect.y
                    + self.appearance.padding
                    + (row + 1) as i32 * self.fonts.height,
            },
            area: ffi::Rect::default(),
        };
        unsafe {
            ffi::ImmSetCompositionWindow(context, &form);
            ffi::ImmReleaseContext(self.hwnd, context);
        }
    }
    pub(super) fn copy(&self) {
        let Some(selection) = self.panes[self.active].selection else {
            return;
        };
        let text = wide(&selection.text(
            &self.panes[self.active].core.terminal,
            self.panes[self.active].history,
        ));
        if unsafe { ffi::OpenClipboard(self.hwnd) } == 0 {
            return;
        }
        let memory = unsafe { ffi::GlobalAlloc(2, text.len() * 2) };
        if !memory.is_null() {
            let target = unsafe { ffi::GlobalLock(memory) }.cast::<u16>();
            if !target.is_null() {
                unsafe {
                    ptr::copy_nonoverlapping(text.as_ptr(), target, text.len());
                    ffi::GlobalUnlock(memory);
                    ffi::EmptyClipboard();
                }
                if unsafe { ffi::SetClipboardData(13, memory) }.is_null() {
                    unsafe {
                        ffi::GlobalFree(memory);
                    }
                }
            } else {
                unsafe {
                    ffi::GlobalFree(memory);
                }
            }
        }
        unsafe {
            ffi::CloseClipboard();
        }
    }
    pub(super) fn paste(&mut self) {
        if unsafe { ffi::OpenClipboard(self.hwnd) } == 0 {
            return;
        }
        let memory = unsafe { ffi::GetClipboardData(13) };
        if !memory.is_null() {
            let size = unsafe { ffi::GlobalSize(memory) };
            if size != 0 && size <= 16 * 1024 * 1024 {
                let text = unsafe { ffi::GlobalLock(memory) }.cast::<u16>();
                if !text.is_null() {
                    let units = unsafe { core::slice::from_raw_parts(text, size / 2) };
                    let length = units.iter().position(|&c| c == 0).unwrap_or(units.len());
                    let text = String::from_utf16_lossy(&units[..length]);
                    self.encoded.clear();
                    input::encode_paste_into(
                        &text,
                        &self.panes[self.active].core.terminal.modes().input(),
                        &mut self.encoded,
                    );
                    self.enqueue_encoded();
                    unsafe {
                        ffi::GlobalUnlock(memory);
                    }
                }
            }
        }
        unsafe {
            ffi::CloseClipboard();
        }
        self.panes[self.active].history = 0;
        self.reset_cursor();
    }
    pub(super) fn key(&mut self, message: u32, vk: usize, flags: isize) -> bool {
        if vk >= 256 {
            return false;
        }
        let pressed = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
        let mut mods = modifiers();
        // Windows represents AltGr as synthetic Ctrl + right Alt. Let the
        // keyboard layout commit its text instead of encoding a Ctrl shortcut.
        if unsafe { ffi::GetKeyState(0xa5) } < 0 && mods.contains(Modifiers::CTRL) {
            mods.0 &= !(Modifiers::CTRL | Modifiers::ALT);
        }
        if pressed && vk == 0x73 && mods.contains(Modifiers::ALT) {
            return false;
        }
        if pressed {
            self.suppress_char = false;
            self.pending_key = None;
            let logical = virtual_key(vk as u32, flags);
            if let Some(action) = actions::key_action(
                &self.appearance.bindings,
                binding_symbol(logical),
                binding_state(mods),
            ) && self.action(action)
            {
                self.intercepted[vk] = true;
                self.suppress_char = true;
                return true;
            }
        } else if self.intercepted[vk] {
            self.intercepted[vk] = false;
            self.held[vk] = None;
            return true;
        }
        let key = if !pressed {
            self.held[vk].unwrap_or_else(|| virtual_key(vk as u32, flags))
        } else {
            virtual_key(vk as u32, flags)
        };
        self.held[vk] = if pressed { Some(key) } else { None };
        let event = if !pressed {
            KeyEvent::Release
        } else if flags & (1 << 30) != 0 {
            KeyEvent::Repeat
        } else {
            KeyEvent::Press
        };
        if !pressed
            || !matches!(key, Key::Character(_) | Key::Unidentified)
            || mods.contains(Modifiers::CTRL)
        {
            self.input(key, mods, event, "");
            if pressed {
                self.suppress_char = true;
            }
        } else {
            self.pending_key = Some(PendingKey {
                key,
                modifiers: mods,
                event,
            });
        }
        true
    }
    pub(super) fn character(&mut self, unit: u16) {
        if self.suppress_char {
            return;
        }
        if (0xd800..=0xdbff).contains(&unit) {
            self.surrogate = unit;
            return;
        }
        let scalar = if (0xdc00..=0xdfff).contains(&unit) && self.surrogate != 0 {
            let scalar =
                0x10000 + (((self.surrogate as u32 - 0xd800) << 10) | (unit as u32 - 0xdc00));
            self.surrogate = 0;
            char::from_u32(scalar).unwrap_or('\u{fffd}')
        } else {
            self.surrogate = 0;
            char::from_u32(unit as u32).unwrap_or('\u{fffd}')
        };
        let mut buffer = [0u8; 4];
        let text = scalar.encode_utf8(&mut buffer);
        if let Some(key) = self.pending_key.take() {
            self.input(key.key, key.modifiers, key.event, text);
        } else {
            self.encoded.clear();
            input::encode_text_into(
                text,
                &self.panes[self.active].core.terminal.modes().input(),
                &mut self.encoded,
            );
            self.enqueue_encoded();
            self.panes[self.active].history = 0;
            self.reset_cursor();
        }
    }
    pub(super) fn cancel_mouse_capture(&mut self, release: bool) {
        if let Some((_, buttons)) = self.mouse_capture.take() {
            self.cancelled_mouse_buttons |= buttons;
        }
        for pane in &mut self.panes {
            if pane.dragging {
                self.cancelled_mouse_buttons |= 1;
                pane.dragging = false;
            }
        }
        if release {
            unsafe {
                ffi::ReleaseCapture();
            }
        }
    }
    pub(super) fn mouse(&mut self, message: u32, wparam: usize, lparam: isize) {
        if let Some((button, pressed)) = mouse_button(message) {
            let bit = 1 << button;
            let cancelled = self.cancelled_mouse_buttons & bit != 0;
            self.cancelled_mouse_buttons &= !bit;
            if !pressed && cancelled {
                return;
            }
        }
        let mut point = ffi::Point {
            x: lparam as i16 as i32,
            y: (lparam >> 16) as i16 as i32,
        };
        if message == 0x020a {
            unsafe {
                ffi::ScreenToClient(self.hwnd, &mut point);
            }
        }
        let hit = self.panes.iter().position(|pane| {
            pane.visible
                && point.x >= pane.rect.x
                && point.y >= pane.rect.y
                && point.x < pane.rect.x.saturating_add(pane.rect.width)
                && point.y < pane.rect.y.saturating_add(pane.rect.height)
        });
        let capture = self
            .mouse_capture
            .and_then(|(id, _)| self.panes.iter().position(|pane| pane.id == id));
        let index = if let Some(index) = capture {
            index
        } else if self.panes[self.active].dragging {
            self.active
        } else if let Some(index) = hit {
            index
        } else {
            return;
        };
        if matches!(message, 0x0201 | 0x0204 | 0x0207) && index != self.active {
            let mut layout = self.layout.clone();
            layout.focus(self.panes[index].id);
            if let Err(error) = self.arrange(layout) {
                self.message(error);
                return;
            }
        }
        self.mouse_at(index, message, wparam, lparam);
    }

    pub(super) fn mouse_at(&mut self, index: usize, message: u32, wparam: usize, lparam: isize) {
        let mut point = ffi::Point {
            x: lparam as i16 as i32,
            y: (lparam >> 16) as i16 as i32,
        };
        if message == 0x020a {
            unsafe {
                ffi::ScreenToClient(self.hwnd, &mut point);
            }
        }
        let (col, row) = self.cell_at(index, point.x, point.y);
        let mods = modifiers();
        let app_tracking = (self.panes[index].core.terminal.modes().mouse_tracking != 0
            && !mods.contains(Modifiers::SHIFT))
            || self
                .mouse_capture
                .is_some_and(|(owner, _)| owner == self.panes[index].id);
        let event = match message {
            0x0201 => Some(MouseEvent::Press {
                button: 0,
                col,
                row,
            }),
            0x0202 => Some(MouseEvent::Release {
                button: 0,
                col,
                row,
            }),
            0x0204 => Some(MouseEvent::Press {
                button: 2,
                col,
                row,
            }),
            0x0205 => Some(MouseEvent::Release {
                button: 2,
                col,
                row,
            }),
            0x0207 => Some(MouseEvent::Press {
                button: 1,
                col,
                row,
            }),
            0x0208 => Some(MouseEvent::Release {
                button: 1,
                col,
                row,
            }),
            0x0200 => Some(MouseEvent::Motion {
                button: if wparam & 1 != 0 && self.cancelled_mouse_buttons & 1 == 0 {
                    Some(0)
                } else if wparam & 0x10 != 0 && self.cancelled_mouse_buttons & 2 == 0 {
                    Some(1)
                } else if wparam & 2 != 0 && self.cancelled_mouse_buttons & 4 == 0 {
                    Some(2)
                } else {
                    None
                },
                col,
                row,
            }),
            0x020a => Some(MouseEvent::Scroll {
                up: (wparam >> 16) as i16 > 0,
                col,
                row,
            }),
            _ => None,
        };
        if app_tracking {
            if let Some((button, true)) = mouse_button(message) {
                let capture = self.mouse_capture.get_or_insert((self.panes[index].id, 0));
                capture.1 |= 1 << button;
                unsafe {
                    ffi::SetCapture(self.hwnd);
                }
            }
            if let Some(event) = event {
                self.encoded.clear();
                input::encode_mouse_into(
                    event,
                    mods,
                    &self.panes[index].core.terminal.modes().input(),
                    &mut self.encoded,
                );
                self.enqueue_encoded_for(index);
            }
            if let Some((button, false)) = mouse_button(message)
                && let Some((_, buttons)) = self.mouse_capture.as_mut()
            {
                *buttons &= !(1 << button);
                if *buttons == 0 {
                    self.mouse_capture = None;
                    unsafe {
                        ffi::ReleaseCapture();
                    }
                }
            }
            return;
        }
        match message {
            0x0201 => {
                if mods.contains(Modifiers::CTRL) {
                    self.open_link(col, row);
                    return;
                }
                self.panes[index].selection = Some(Selection {
                    start: (col, row),
                    end: (col, row),
                });
                self.panes[index].dragging = true;
                self.panes[index].dirty = true;
                unsafe {
                    ffi::SetCapture(self.hwnd);
                }
            }
            0x0200 if self.panes[index].dragging => {
                if let Some(selection) = self.panes[index].selection.as_mut() {
                    selection.end = (col, row);
                    self.panes[index].dirty = true;
                }
            }
            0x0202 if self.panes[index].dragging => {
                self.panes[index].dragging = false;
                unsafe {
                    ffi::ReleaseCapture();
                }
            }
            0x0207 => self.paste(),
            0x020a => {
                let delta = (wparam >> 16) as i16 as i32;
                let lines = (delta.unsigned_abs() as usize / 120).max(1) * 3;
                self.panes[index].history = if delta > 0 {
                    self.panes[index]
                        .history
                        .saturating_add(lines)
                        .min(self.panes[index].core.terminal.history_len())
                } else {
                    self.panes[index].history.saturating_sub(lines)
                };
                self.panes[index].selection = None;
                self.panes[index].dirty = true;
            }
            _ => {}
        }
    }
    pub(super) fn open_link(&self, col: usize, row: usize) {
        let terminal = &self.panes[self.active].core.terminal;
        let cell = terminal.view_cell(col, row, self.panes[self.active].history);
        let Some(uri) = terminal.cell_hyperlink(cell) else {
            return;
        };
        if uri.len() > 8192
            || uri.chars().any(char::is_control)
            || !(uri.starts_with("https://")
                || uri.starts_with("http://")
                || uri.starts_with("mailto:"))
        {
            return;
        }
        unsafe {
            ffi::ShellExecuteW(
                self.hwnd,
                wide("open").as_ptr(),
                wide(uri).as_ptr(),
                ptr::null(),
                ptr::null(),
                1,
            );
        }
    }
}

pub(super) fn mouse_button(message: u32) -> Option<(u8, bool)> {
    match message {
        0x0201 => Some((0, true)),
        0x0202 => Some((0, false)),
        0x0204 => Some((2, true)),
        0x0205 => Some((2, false)),
        0x0207 => Some((1, true)),
        0x0208 => Some((1, false)),
        _ => None,
    }
}

pub(crate) fn action_supported(action: &str) -> bool {
    matches!(
        action,
        "new-tab"
            | "split-right"
            | "split-down"
            | "next-tab"
            | "previous-tab"
            | "next-pane"
            | "previous-pane"
            | "swap-next"
            | "swap-previous"
            | "maximize"
            | "grow"
            | "shrink"
            | "close"
            | "copy"
            | "paste"
            | "font-increase"
            | "font-decrease"
            | "font-reset"
            | "scroll-up"
            | "scroll-down"
            | "none"
    )
}

fn binding_state(modifiers: Modifiers) -> u32 {
    u32::from(modifiers.contains(Modifiers::SHIFT))
        | (u32::from(modifiers.contains(Modifiers::CTRL)) << 2)
        | (u32::from(modifiers.contains(Modifiers::ALT)) << 3)
        | (u32::from(modifiers.contains(Modifiers::SUPER)) << 6)
}

fn binding_symbol(key: Key) -> u64 {
    match key {
        Key::Character(ch) => u64::from(ch as u32),
        Key::KeypadAdd => 0xffab,
        Key::KeypadSubtract => 0xffad,
        Key::KeypadDigit(0) => 0xffb0,
        Key::Escape => 0xff1b,
        Key::Enter | Key::KeypadEnter => 0xff0d,
        Key::Tab => 0xff09,
        Key::Backspace => 0xff08,
        Key::Left => 0xff51,
        Key::Right => 0xff53,
        Key::Up => 0xff52,
        Key::Down => 0xff54,
        Key::PageUp => 0xff55,
        Key::PageDown => 0xff56,
        Key::Home => 0xff50,
        Key::End => 0xff57,
        Key::Function(number) => 0xffbd + u64::from(number),
        _ => 0,
    }
}

pub(super) fn modifiers() -> Modifiers {
    let down = |vk| unsafe { ffi::GetKeyState(vk) } < 0;
    Modifiers(
        u8::from(down(0x10))
            | (u8::from(down(0x12)) << 1)
            | (u8::from(down(0x11)) << 2)
            | (u8::from(down(0x5b) || down(0x5c)) << 3)
            | (u8::from(unsafe { ffi::GetKeyState(0x14) } & 1 != 0) << 6)
            | (u8::from(unsafe { ffi::GetKeyState(0x90) } & 1 != 0) << 7),
    )
}

fn virtual_key(vk: u32, flags: isize) -> Key {
    match vk {
        0x1b => Key::Escape,
        0x0d if flags & (1 << 24) != 0 => Key::KeypadEnter,
        0x0d => Key::Enter,
        0x09 => Key::Tab,
        0x08 => Key::Backspace,
        0x25 => Key::Left,
        0x26 => Key::Up,
        0x27 => Key::Right,
        0x28 => Key::Down,
        0x24 => Key::Home,
        0x23 => Key::End,
        0x2d => Key::Insert,
        0x2e => Key::Delete,
        0x21 => Key::PageUp,
        0x22 => Key::PageDown,
        0x70..=0x87 => Key::Function((vk - 0x70 + 1) as u8),
        0x14 => Key::CapsLock,
        0x90 => Key::NumLock,
        0x91 => Key::ScrollLock,
        0x2c => Key::PrintScreen,
        0x13 => Key::Pause,
        0x5d => Key::Menu,
        0x10 => Key::Modifier(if (flags >> 16) & 0xff == 0x36 {
            ModifierKey::RightShift
        } else {
            ModifierKey::LeftShift
        }),
        0x11 => Key::Modifier(if flags & (1 << 24) != 0 {
            ModifierKey::RightControl
        } else {
            ModifierKey::LeftControl
        }),
        0x12 => Key::Modifier(if flags & (1 << 24) != 0 {
            ModifierKey::RightAlt
        } else {
            ModifierKey::LeftAlt
        }),
        0x5b => Key::Modifier(ModifierKey::LeftSuper),
        0x5c => Key::Modifier(ModifierKey::RightSuper),
        0x60..=0x69 => Key::KeypadDigit((vk - 0x60) as u8),
        0x6e => Key::KeypadDecimal,
        0x6f => Key::KeypadDivide,
        0x6a => Key::KeypadMultiply,
        0x6d => Key::KeypadSubtract,
        0x6b => Key::KeypadAdd,
        0x6c => Key::KeypadSeparator,
        _ => {
            let scalar = unsafe { ffi::MapVirtualKeyW(vk, 2) } & 0x7fffffff;
            char::from_u32(scalar)
                .filter(|c| !c.is_control())
                .map(|c| Key::Character(c.to_ascii_lowercase()))
                .unwrap_or(Key::Unidentified)
        }
    }
}
