use super::*;

impl Window {
    pub fn accepts_event(&self, window: Xid, _: c_int) -> bool {
        window == self.state.id
    }
    pub fn paste_pending(&self) -> bool {
        self.paste_pending
    }
    pub fn clipboard_available(&self, primary: bool) -> bool {
        self.state.id != 0
            && unsafe { ztty_wl_clip_available(self.native.ptr(), c_int::from(primary)) != 0 }
    }
    pub fn copy(&mut self, text: String, primary: bool) {
        if unsafe {
            ztty_wl_copy(
                self.native.ptr(),
                text.as_ptr(),
                text.len(),
                c_int::from(primary),
            )
        } < 0
        {
            self.notice = Some(String::from(
                "Clipboard unavailable: focus the window and try Copy again",
            ));
        }
    }
    pub fn cancel_paste(&mut self) {
        unsafe {
            ztty_wl_cancel_receive(self.native.ptr());
        }
        self.paste_pending = false;
        self.paste_error = None;
    }
    pub(super) fn receive(&mut self, primary: bool, limit: usize) -> bool {
        self.cancel_paste();
        self.paste_generation = self.paste_generation.wrapping_add(1).max(1);
        if unsafe {
            ztty_wl_receive(
                self.native.ptr(),
                c_int::from(primary),
                limit,
                self.paste_generation,
            )
        } < 0
        {
            return false;
        }
        self.paste_pending = true;
        true
    }
    pub fn paste(&mut self, primary: bool) {
        if !self.receive(primary, 4 * 1024 * 1024) {
            self.notice = Some(String::from("Clipboard has no available UTF-8 text"));
        }
    }
    pub fn read_osc_clipboard(&mut self, primary: bool) -> bool {
        self.receive(primary, zterm::MAX_CLIPBOARD_BYTES)
    }
    pub fn selection_request(&self, _: SelectionRequest) {}
    pub fn selection_notify(&mut self, event: SelectionEvent) -> Option<Vec<u8>> {
        if !self.paste_pending || event.serial != c_ulong::from(self.paste_generation) {
            return None;
        }
        let mut len = 0;
        let mut error = 0;
        let data = unsafe {
            ztty_wl_clip_result(
                self.native.ptr(),
                self.paste_generation,
                &mut len,
                &mut error,
            )
        };
        let result = if error == 0 {
            Some(if len == 0 {
                Vec::new()
            } else if !data.is_null() {
                unsafe { core::slice::from_raw_parts(data, len) }.to_vec()
            } else {
                Vec::new()
            })
        } else {
            None
        };
        self.cancel_paste();
        if error != 0 {
            self.paste_error = Some(if error == 2 {
                "Clipboard text exceeds the transfer limit"
            } else {
                "Clipboard transfer failed or timed out"
            });
        }
        result
    }
    pub fn property(&mut self, _: PropertyEvent) -> Option<Vec<u8>> {
        None
    }
    pub fn take_paste_error(&mut self) -> Option<&'static str> {
        self.paste_error.take()
    }
    pub(super) fn draw_preedit(&mut self) {
        let raw = unsafe { ztty_wl_preedit(self.native.ptr()) };
        if raw.is_null() {
            return;
        }
        let text = unsafe { CStr::from_ptr(raw) }.to_str().unwrap_or("");
        if text.is_empty() {
            return;
        }
        let text = String::from(text);
        let (col, row) = self.last_cursor.map_or((0, 0), |c| c.0);
        let x = (self.appearance.padding + col as i32 * self.state.cw)
            .min((self.state.width - self.state.cw).max(0));
        let y = (self.appearance.padding + row as i32 * self.state.ch)
            .min((self.state.height - self.state.ch).max(0));
        let width = (self.overlay_width(&text) + 2).min(self.state.width - x);
        let r = crate::layout::Rect {
            x,
            y,
            width,
            height: self.state.ch,
        };
        self.native_ui_fill(r, self.appearance.theme.background);
        self.native_ui_text(&text, r, self.appearance.selection_foreground);
        self.native_ui_fill(
            crate::layout::Rect {
                y: y + self.state.ch - 1,
                height: 1,
                ..r
            },
            self.appearance.cursor_color,
        );
    }
}
