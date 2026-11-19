use super::*;

impl core::ops::Deref for Window {
    type Target = WindowState;
    fn deref(&self) -> &WindowState {
        &self.state
    }
}
impl core::ops::DerefMut for Window {
    fn deref_mut(&mut self) -> &mut WindowState {
        &mut self.state
    }
}
impl Window {
    pub fn failed(&self) -> bool {
        self.native.failed.get()
    }

    pub fn new(
        cols: usize,
        rows: usize,
        appearance: &Appearance,
        connection: &Rc<Connection>,
        identity: *mut x11::Display,
        activate: bool,
    ) -> Result<Self, &'static str> {
        if appearance.renderer == RendererKind::Software {
            return Err("native Wayland requires renderer=auto or renderer=gpu");
        }
        let native = Rc::new(Native {
            ptr: NonNull::new(unsafe {
                ztty_wl_create(
                    640,
                    480,
                    connection.endpoint.as_ptr(),
                    connection.locale.as_ptr(),
                )
            })
            .ok_or("cannot create native Wayland window")?,
            last: Cell::new(Event::default()),
            focused: Cell::new(false),
            failed: Cell::new(false),
            closed: Cell::new(false),
            buttons: Cell::new(0),
            scroll: Cell::new(0),
            created: os::milliseconds(),
        });
        let display = unsafe { Display::wayland(ztty_wl_display(native.ptr())) }?;
        let mut renderer = Renderer::new(
            display,
            Target::Wayland {
                window: unsafe { ztty_wl_window(native.ptr()) },
            },
            640,
            480,
            &appearance.font,
        )?;
        let metrics = renderer.metrics();
        let cw = (metrics.cell_width + 0.999) as i32;
        let ch = (metrics.cell_height + 0.999) as i32;
        let width = (cols as i32 * cw + appearance.padding * 2).clamp(32, 8192);
        let height = (rows as i32 * ch + appearance.padding * 2).clamp(32, 8192);
        if unsafe { ztty_wl_resize(native.ptr(), width, height) } < 0 {
            return Err("Wayland window exceeds the render budget");
        }
        renderer.resize(width as u32, height as u32)?;
        unsafe {
            ztty_wl_grid_size(native.ptr(), width, height);
        }
        let state = WindowState {
            display: identity,
            id: native.id(),
            fd: unsafe { ztty_wl_fd(native.ptr()) },
            cw,
            ch,
            width,
            height,
            wm_delete: 0,
            configure_serial: Cell::new(0),
            focused: activate,
        };
        connection
            .windows
            .borrow_mut()
            .retain(|w| w.strong_count() != 0);
        connection.windows.borrow_mut().push(Rc::downgrade(&native));
        Ok(Self {
            state,
            renderer,
            native,
            appearance: appearance.clone(),
            ascent: metrics.ascent + 1.0,
            repaint: Cell::new(true),
            drawing: false,
            suspended: false,
            notice: None,
            last_cursor: None,
            text_blink_visible: [true; 2],
            search_cols: 0,
            search_bits: Vec::new(),
            graphics_revision: 0,
            graphics_alternate: false,
            paste_generation: 0,
            paste_pending: false,
            paste_error: None,
        })
    }
    pub(super) fn failure(&mut self, e: &'static str) {
        self.notice = Some(alloc::format!("Wayland renderer: {e}"));
        self.native.failed.set(true);
        self.drawing = false;
    }
    pub fn title(&mut self, text: &str) {
        if let Ok(text) = CString::new(if text.is_empty() { "ztty" } else { text }) {
            unsafe {
                ztty_wl_title(self.native.ptr(), text.as_ptr());
            }
        }
    }
    pub fn focus(&mut self, focused: bool) {
        self.state.focused = focused;
        unsafe {
            ztty_wl_ime_focus(self.native.ptr(), c_int::from(focused));
        }
    }
    pub fn coordinates(&self, x: i32, y: i32, cols: usize, rows: usize) -> (usize, usize) {
        (
            (((x - self.appearance.padding).max(0) / self.state.cw) as usize).min(cols - 1),
            (((y - self.appearance.padding).max(0) / self.state.ch) as usize).min(rows - 1),
        )
    }

    pub fn reconfigure(
        &mut self,
        appearance: &Appearance,
        before: impl FnOnce(usize, usize, i32, i32) -> Result<(), String>,
    ) -> Result<(usize, usize), String> {
        if appearance.renderer == RendererKind::Software {
            return Err("native Wayland requires renderer=auto or renderer=gpu".into());
        }
        let old = self.appearance.font.clone();
        self.renderer
            .set_font(&appearance.font)
            .map_err(String::from)?;
        let m = self.renderer.metrics();
        let cw = (m.cell_width + 0.999) as i32;
        let ch = (m.cell_height + 0.999) as i32;
        let cols =
            ((self.state.width - 2 * appearance.padding).max(cw) / cw).clamp(2, 512) as usize;
        let rows =
            ((self.state.height - 2 * appearance.padding).max(ch) / ch).clamp(2, 256) as usize;
        if let Err(e) = before(cols, rows, cw, ch) {
            let _ = self.renderer.set_font(&old);
            return Err(e);
        }
        self.appearance = appearance.clone();
        self.state.cw = cw;
        self.state.ch = ch;
        self.ascent = m.ascent + 1.0;
        self.repaint.set(true);
        Ok((cols, rows))
    }
    pub fn present(&self) {
        self.repaint.set(true);
    }
    pub fn finish_frame(&mut self) {
        if self.drawing {
            self.draw_preedit();
            self.drawing = false;
            if unsafe { ztty_wl_request_frame(self.native.ptr()) } < 0 {
                self.repaint.set(true);
                return;
            }
            if let Err(e) = self.renderer.present() {
                self.failure(e);
            } else {
                self.repaint.set(false);
            }
        }
    }
    pub fn resize_grid(&self, cols: usize, rows: usize) {
        unsafe {
            ztty_wl_grid_size(
                self.native.ptr(),
                cols as i32 * self.state.cw + 2 * self.appearance.padding,
                rows as i32 * self.state.ch + 2 * self.appearance.padding,
            );
        }
    }
    pub fn resize(&mut self, width: i32, height: i32) -> (usize, usize) {
        if width != self.state.width || height != self.state.height {
            if unsafe { ztty_wl_resize(self.native.ptr(), width, height) } < 0 {
                self.failure("window exceeds render budget");
            } else if let Err(e) = self.renderer.resize(width as u32, height as u32) {
                self.failure(e);
            } else {
                self.state.width = width;
                self.state.height = height;
                self.repaint.set(true);
            }
        }
        (
            ((self.state.width - 2 * self.appearance.padding).max(0) / self.state.cw).clamp(2, 512)
                as usize,
            ((self.state.height - 2 * self.appearance.padding).max(0) / self.state.ch).clamp(2, 256)
                as usize,
        )
    }
    pub fn parent(&mut self, parent: Option<Xid>) {
        debug_assert!(parent.is_none());
    }
    pub fn place(&mut self, r: crate::layout::Rect) -> (usize, usize) {
        self.resize(r.width, r.height)
    }
    pub fn visible(&mut self, visible: bool) {
        self.suspended = !visible;
        self.repaint.set(visible);
    }
    pub fn input_symbol(&self, _: &mut KeyEvent) -> c_ulong {
        self.native.last.get().symbol as c_ulong
    }
    pub fn key_symbol(&self, _: &mut KeyEvent, shifted: bool) -> c_ulong {
        let e = self.native.last.get();
        if shifted {
            e.shifted as c_ulong
        } else {
            e.symbol as c_ulong
        }
    }
    pub fn lookup(&self, _: &mut KeyEvent, buffer: &mut Vec<u8>) -> Lookup {
        let e = self.native.last.get();
        let len = e.text.iter().position(|&b| b == 0).unwrap_or(e.text.len());
        buffer.clear();
        buffer.extend_from_slice(&e.text[..len]);
        Lookup {
            len,
            symbol: e.x as c_ulong,
            text_only: e.symbol == 0,
            ignored: false,
        }
    }
    pub fn raise(&self) {
        self.repaint.set(true);
    }
    pub fn cursor_changed(&self, cursor: Option<DrawnCursor>) -> bool {
        self.last_cursor != cursor
    }
    pub fn search_matches(&mut self, cols: usize, bits: Vec<u64>) {
        self.search_cols = cols;
        self.search_bits = bits;
        self.repaint.set(true);
    }
    pub fn graphics_changed(&self, t: &Terminal) -> bool {
        self.graphics_revision != t.graphics().revision()
            || self.graphics_alternate != t.modes().alternate_screen
    }
    pub fn render(
        &mut self,
        t: &mut Terminal,
        history: usize,
        selection: Option<Selection>,
        _: bool,
        cursor: Option<DrawnCursor>,
    ) {
        if self.suspended || !self.native.ready() {
            self.repaint.set(true);
            return;
        }
        let a = &self.appearance;
        let cols = t.cols();
        let search_cols = self.search_cols;
        let bits = &self.search_bits;
        let style = GridStyle {
            padding: a.padding as f32,
            cell_width: self.state.cw as f32,
            cell_height: self.state.ch as f32,
            ascent: self.ascent,
            background: a.theme.background,
            background_alpha: (u16::from(a.opacity) * 255 / 100) as u8,
            selection_foreground: a.selection_foreground,
            selection_background: a.selection_background,
            cursor,
            cursor_color: a.cursor_color,
            slow_blink_visible: self.text_blink_visible[0],
            rapid_blink_visible: self.text_blink_visible[1],
        };
        let selected = |col, row: usize| {
            let bit = row.saturating_mul(search_cols).saturating_add(col);
            selection.is_some_and(|s| s.contains(col, row, cols))
                || bits
                    .get(bit / 64)
                    .is_some_and(|v| v & (1 << (bit % 64)) != 0)
        };
        if let Err(e) = self.renderer.draw_terminal(t, history, style, selected) {
            self.failure(e);
            return;
        }
        self.drawing = true;
        if let Some(((col, row), _)) = cursor {
            unsafe {
                ztty_wl_ime_cursor(
                    self.native.ptr(),
                    self.appearance.padding + col as i32 * self.state.cw,
                    self.appearance.padding + row as i32 * self.state.ch,
                    self.state.cw,
                    self.state.ch,
                );
            }
        }
        self.last_cursor = cursor;
        self.graphics_revision = t.graphics().revision();
        self.graphics_alternate = t.modes().alternate_screen;
        t.clear_dirty();
    }
    pub fn take_renderer_notice(&mut self) -> Option<String> {
        self.notice.take()
    }
    pub fn needs_repaint(&self) -> bool {
        !self.suspended && self.repaint.get() && self.native.ready()
    }
    pub fn set_text_blink_visible(&mut self, v: [bool; 2]) {
        self.text_blink_visible = v;
    }
}
