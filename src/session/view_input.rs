//! Keyboard and pointer routing for one desktop view.
use super::{ClipboardMenu, InputBuffers, PaletteTarget, Ui, pty_size};
use crate::{
    key, key_modifiers, modifiers, os, queue_input, search,
    session_core::SessionCore,
    x11::{self, Selection},
};
use alloc::{format, vec::Vec};
use zterm::input::{self, Key, KeyEvent, MouseEvent};

impl Ui {
    // The view borrows shared session state; window ownership stays separate
    // from the PTY and its input queue.
    pub(super) fn event(
        &mut self,
        event: &mut x11::Event,
        core: &mut SessionCore,
        openers: &mut os::Openers,
        buffers: &mut InputBuffers,
        allow_input: bool,
    ) -> Result<(), &'static str> {
        let InputBuffers {
            text: text_buffer,
            encoded,
        } = buffers;
        match unsafe { event.kind } {
            x11::KEY_PRESS | x11::KEY_RELEASE => {
                let mut e = unsafe { event.key };
                let code = (e.keycode as usize).min(255);
                let press = e.kind == x11::KEY_PRESS;
                if !press && self.held[code] == 0 {
                    return Ok(());
                }
                self.reset_blink |= press;
                let symbol = if self.held[code] != 0 {
                    self.held[code]
                } else {
                    self.window.input_symbol(&mut e)
                };
                let repeated = self.held[code] != 0;
                self.held[code] = if press { symbol } else { 0 };
                let mods = key_modifiers(e.state, symbol, press, &self.held);
                if !press && self.intercepted[code] {
                    self.intercepted[code] = false;
                    return Ok(());
                }
                if let Some(menu) = &mut self.clipboard_menu {
                    if press {
                        self.intercepted[code] = true;
                        match symbol {
                            0xff1b => self.clipboard_menu = None,
                            0xff52 | 0xff54 | 0xff97 | 0xff99 => {
                                let next = 1 - menu.selected;
                                if menu.enabled[next] {
                                    menu.selected = next;
                                }
                            }
                            0xff0d | 0xff8d => {
                                self.pending_action = menu.action();
                                self.clipboard_menu = None;
                            }
                            _ => {}
                        }
                        self.force = true;
                    }
                    return Ok(());
                }
                if self.search.is_some() {
                    if !press {
                        return Ok(());
                    }
                    self.intercepted[code] = true;
                    match symbol {
                        0xff1b => {
                            self.search = None;
                            self.window.search_matches(0, Vec::new());
                            self.force = true;
                        }
                        0xff0d | 0xff8d | 0xffc0 => {
                            self.find(&core.terminal, mods.0 & 1 == 0, false)
                        }
                        0x63 | 0x43
                            if mods.0
                                & !(input::Modifiers::CAPS_LOCK | input::Modifiers::NUM_LOCK)
                                == input::Modifiers::ALT =>
                        {
                            if !repeated {
                                let search = self.search.as_mut().unwrap();
                                search.case = match search.case {
                                    search::Case::Sensitive => search::Case::Insensitive,
                                    search::Case::Insensitive => search::Case::Sensitive,
                                };
                                self.find(&core.terminal, false, true);
                            }
                        }
                        0xff08 => {
                            self.search.as_mut().unwrap().query.pop();
                            self.find(&core.terminal, false, true);
                        }
                        0x61 | 0x41 | 0x75 | 0x55 if mods.0 & 4 != 0 => {
                            self.search.as_mut().unwrap().query.clear();
                            self.find(&core.terminal, false, true);
                        }
                        _ => {
                            let lookup = self.window.lookup(&mut e, text_buffer);
                            if !lookup.ignored {
                                let text =
                                    core::str::from_utf8(&text_buffer[..lookup.len]).unwrap_or("");
                                let query = &mut self.search.as_mut().unwrap().query;
                                let previous_len = query.len();
                                for ch in text.chars().filter(|ch| !ch.is_control()) {
                                    if query.len() + ch.len_utf8() <= 1024 {
                                        query.push(ch);
                                    }
                                }
                                if query.len() != previous_len {
                                    self.find(&core.terminal, false, true);
                                }
                            }
                        }
                    }
                    return Ok(());
                }
                if let Some(palette) = &mut self.palette {
                    if !press {
                        return Ok(());
                    }
                    self.intercepted[code] = true;
                    match symbol {
                        0xff1b => self.palette = None,
                        0xff0d | 0xff8d => {
                            let target = palette
                                .matching()
                                .nth(palette.selected)
                                .map(|item| item.target);
                            if target.is_some() {
                                self.pending_action = target;
                                self.palette = None;
                            }
                        }
                        0xff52 | 0xff54 | 0xff97 | 0xff99 => {
                            let count = palette.matching().count();
                            if count != 0 {
                                palette.selected = (palette.selected
                                    + if matches!(symbol, 0xff54 | 0xff99) {
                                        1
                                    } else {
                                        count - 1
                                    })
                                    % count;
                            }
                        }
                        0xff08 => {
                            palette.query.pop();
                            palette.selected = 0;
                        }
                        0x61 | 0x41 | 0x75 | 0x55 if mods.0 & 4 != 0 => {
                            palette.query.clear();
                            palette.selected = 0;
                        }
                        _ => {
                            let lookup = self.window.lookup(&mut e, text_buffer);
                            if !lookup.ignored {
                                let previous_len = palette.query.len();
                                for ch in core::str::from_utf8(&text_buffer[..lookup.len])
                                    .unwrap_or("")
                                    .chars()
                                    .filter(|c| !c.is_control())
                                {
                                    if palette.query.len() + ch.len_utf8() <= 128 {
                                        palette.query.push(ch);
                                    }
                                }
                                if palette.query.len() != previous_len {
                                    palette.selected = 0;
                                }
                            }
                        }
                    }
                    self.force = true;
                    return Ok(());
                }
                if press && symbol == 0xff1b && self.plugin_running {
                    self.intercepted[code] = true;
                    self.pending_action = Some(PaletteTarget::CancelPlugin);
                    return Ok(());
                }
                if press {
                    let geometry_current =
                        self.native_ui.layout.as_ref().is_none_or(|layout| {
                            layout.geometry == self.window.native_ui_geometry()
                        });
                    let interactive = geometry_current
                        && allow_input
                        && self.visible
                        && self.open
                        && !core.eof()
                        && self.history == 0;
                    let (handled, action) = self.native_ui.key(
                        &mut core.terminal,
                        symbol,
                        mods.0 & 1 != 0,
                        repeated,
                        interactive,
                    );
                    if handled {
                        self.intercepted[code] = true;
                        self.force = true;
                        self.native_action(core, action);
                        return Ok(());
                    }
                }
                let lookup = if press {
                    self.window.lookup(&mut e, text_buffer)
                } else {
                    x11::Lookup {
                        len: 0,
                        symbol,
                        text_only: false,
                        ignored: false,
                    }
                };
                if lookup.ignored {
                    self.intercepted[code] = true;
                    return Ok(());
                }
                if !allow_input {
                    return Ok(());
                }
                let text = core::str::from_utf8(&text_buffer[..lookup.len]).unwrap_or("");
                let input_key = if symbol == 0 {
                    key(lookup.symbol)
                } else {
                    key(symbol)
                };
                encoded.clear();
                if lookup.text_only {
                    input::encode_text_into(text, &core.terminal.modes().input(), encoded);
                    self.intercepted[code] = true;
                } else {
                    let shifted = if mods.0 & 1 != 0 {
                        if let Key::Character(ch) = key(self.window.key_symbol(&mut e, true)) {
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
                        &core.terminal.modes().input(),
                        encoded,
                    );
                }
                if !encoded.is_empty() {
                    if self.history != 0 {
                        self.scroll_bottom();
                    }
                    if self.selection.is_some() {
                        self.selection = None;
                        self.force = true;
                    }
                    self.repaint.input_sent();
                    queue_input(&mut core.input, encoded);
                }
            }
            x11::BUTTON_PRESS | x11::BUTTON_RELEASE => {
                let e = unsafe { event.button };
                if !(1..=11).contains(&e.button) {
                    return Ok(());
                }
                let (col, row) =
                    self.window
                        .coordinates(e.x, e.y, core.terminal.cols(), core.terminal.rows());
                let mods = modifiers(e.state);
                let pressed = e.kind == x11::BUTTON_PRESS;
                let button_index = e.button as usize;
                let mask = 1u16 << button_index;
                let was_reported = self.reported_buttons & mask != 0;
                self.reported_buttons &= !mask;
                if !pressed && self.swallowed_buttons[button_index] {
                    self.swallowed_buttons[button_index] = false;
                    return Ok(());
                }
                let mut local = !allow_input
                    || mods.0 & 1 != 0
                    || core.terminal.modes().mouse_tracking == 0
                    || self.history != 0
                        && ztty::scroll_prompt::contains(self.scroll_prompt, e.x, e.y);
                if pressed && local {
                    self.local_buttons[button_index] = true;
                }
                if !pressed {
                    // A release belongs to the destination of its press,
                    // even when Shift changes while the button is held.
                    local =
                        !allow_input || self.local_buttons[button_index] || !was_reported && local;
                    self.local_buttons[button_index] = false;
                }
                if pressed && e.button == 1 && mods.0 & 4 != 0 {
                    let cell = core.terminal.view_cell(col, row, self.history);
                    if let Some(uri) = core.terminal.cell_hyperlink(cell) {
                        openers.open(uri, self.window.fd, core.pty.fd());
                        self.swallowed_buttons[button_index] = true;
                        self.local_buttons[button_index] = false;
                        return Ok(());
                    }
                }
                if local {
                    if pressed && e.button == 1 {
                        let now = os::milliseconds();
                        let count = self
                            .clicks
                            .filter(|(at, point, count)| {
                                now.saturating_sub(*at) <= 400 && *point == (col, row) && *count < 3
                            })
                            .map_or(1, |(_, _, count)| count + 1);
                        self.clicks = Some((now, (col, row), count));
                        self.selection_unit = match count {
                            2 => ztty::selection::Unit::Word,
                            3 => ztty::selection::Unit::Line,
                            _ => ztty::selection::Unit::Cell,
                        };
                        let (start, end) = ztty::selection::unit(
                            &core.terminal,
                            (col, row),
                            self.history,
                            self.selection_unit,
                        );
                        self.selection_anchor = (start, end);
                        self.selection = Some(Selection {
                            start,
                            end,
                            dragging: true,
                        });
                        self.message = None;
                        self.force = true;
                    } else if !pressed
                        && e.button == 1
                        && self.selection.is_some_and(|selection| selection.dragging)
                    {
                        self.extend_selection(&core.terminal, (col, row));
                        if let Some(ref mut s) = self.selection {
                            s.dragging = false;
                            self.window.copy(s.text(&core.terminal, self.history), true);
                            let selected = s.start != s.end
                                || self.selection_unit != ztty::selection::Unit::Cell;
                            if selected {
                                self.notice("Selection ready · Right-click to copy");
                            }
                            self.force = true;
                        }
                    } else if pressed && e.button == 3 {
                        let enabled = [self.selection.is_some(), allow_input && !core.eof()];
                        self.clipboard_menu = Some(ClipboardMenu {
                            bounds: self.window.clipboard_menu_bounds(e.x, e.y),
                            selected: usize::from(!enabled[0] && enabled[1]),
                            enabled,
                        });
                        self.palette = None;
                        self.native_ui.blur();
                        self.message = None;
                        self.swallowed_buttons[button_index] = true;
                        self.local_buttons[button_index] = false;
                        self.clicks = None;
                        self.force = true;
                    } else if pressed && e.button == 2 {
                        if allow_input {
                            self.manual_paste(true);
                        }
                    } else if pressed && (e.button == 4 || e.button == 5) {
                        self.set_history(if e.button == 4 {
                            self.history
                                .saturating_add(3)
                                .min(core.terminal.history_len())
                        } else {
                            self.history.saturating_sub(3)
                        });
                        self.selection = None;
                        self.force = true;
                    }
                } else {
                    let mouse = if pressed {
                        MouseEvent::Press {
                            button: (e.button - 1) as u8,
                            col,
                            row,
                        }
                    } else {
                        if !was_reported {
                            return Ok(());
                        }
                        MouseEvent::Release {
                            button: (e.button - 1) as u8,
                            col,
                            row,
                        }
                    };
                    encoded.clear();
                    input::encode_mouse_into(mouse, mods, &core.terminal.modes().input(), encoded);
                    if pressed && !(4..=7).contains(&e.button) && !encoded.is_empty() {
                        self.reported_buttons |= mask;
                    }
                    if !encoded.is_empty() {
                        self.repaint.input_sent();
                    }
                    queue_input(&mut core.input, encoded);
                }
            }
            x11::MOTION_NOTIFY => {
                let e = unsafe { event.motion };
                let (col, row) =
                    self.window
                        .coordinates(e.x, e.y, core.terminal.cols(), core.terminal.rows());
                if self.selection.is_some_and(|selection| selection.dragging) {
                    if self.clicks.is_some_and(|(_, point, _)| point != (col, row)) {
                        self.clicks = None;
                    }
                    self.extend_selection(&core.terminal, (col, row));
                    return Ok(());
                }
                if !allow_input {
                    return Ok(());
                }
                let button = (self.reported_buttons != 0)
                    .then(|| self.reported_buttons.trailing_zeros() as u8 - 1);
                encoded.clear();
                input::encode_mouse_into(
                    MouseEvent::Motion { button, col, row },
                    modifiers(e.state),
                    &core.terminal.modes().input(),
                    encoded,
                );
                if !encoded.is_empty() {
                    self.repaint.input_sent();
                }
                queue_input(&mut core.input, encoded);
            }
            x11::CONFIGURE_NOTIFY => {
                let e = unsafe { event.configure };
                // Ignore geometry queued before the latest layout request.
                // This uses Xlib sequence numbers without a server round trip.
                if e.serial < self.window.configure_serial.get() {
                    return Ok(());
                }
                self.native_ui.invalidate();
                self.clipboard_menu = None;
                let (cols, rows) = self.window.resize(e.width, e.height);
                self.scroll_prompt = None;
                if allow_input && (cols != core.terminal.cols() || rows != core.terminal.rows()) {
                    let mut anchor = self.capture_view_anchor(&core.terminal);
                    match core.resize_anchored(
                        pty_size(cols, rows, Some(&self.window)),
                        anchor.anchors_mut(),
                    ) {
                        Ok(()) => self.restore_view_anchor(&core.terminal, anchor),
                        Err(error) => self.error(format!("cannot resize the PTY: {error}")),
                    }
                }
                self.force = true;
            }
            x11::FOCUS_IN | x11::FOCUS_OUT => {
                let focused = unsafe { event.kind } == x11::FOCUS_IN;
                self.window.focus(focused);
                self.reset_blink = true;
                self.force = true;
                if !focused {
                    self.reset_input();
                }
                if allow_input {
                    let encoded_focus =
                        input::encode_focus(focused, &core.terminal.modes().input());
                    if !encoded_focus.is_empty() {
                        self.repaint.input_sent();
                    }
                    queue_input(&mut core.input, &encoded_focus);
                }
            }
            x11::EXPOSE => {
                self.window.present();
                self.force = true;
            }
            x11::CLIENT_MESSAGE => {
                if unsafe { event.client.data.longs[0] as u64 } == self.window.wm_delete {
                    self.open = false;
                }
            }
            x11::DESTROY_NOTIFY => {
                self.window.id = 0;
                self.open = false;
            }
            x11::SELECTION_REQUEST => {
                self.window.selection_request(unsafe { event.request });
            }
            x11::SELECTION_NOTIFY => {
                let data = self.window.selection_notify(unsafe { event.selection });
                self.paste_result(data, core, allow_input);
            }
            x11::PROPERTY_NOTIFY => {
                let data = self.window.property(unsafe { event.property });
                self.paste_result(data, core, allow_input);
            }
            x11::SELECTION_CLEAR => {}
            _ => {}
        }
        Ok(())
    }
}
