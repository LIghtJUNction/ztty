//! Native window lifetime, configuration, and workspace geometry.
use super::*;

/// Find a TrueColor visual whose depth includes an actual alpha channel.
unsafe fn alpha_visual(display: *mut Display, screen: c_int) -> Option<*mut c_void> {
    unsafe {
        let mut filter: VisualInfo = core::mem::zeroed();
        filter.screen = screen;
        filter.depth = 32;
        filter.class = 4; // TrueColor
        let mut count = 0;
        let infos = XGetVisualInfo(display, 2 | 4 | 8, &mut filter, &mut count);
        if infos.is_null() {
            return None;
        }
        let visual = core::slice::from_raw_parts(infos, count as usize)
            .iter()
            .find(|info| {
                let format = XRenderFindVisualFormat(display, info.visual);
                !format.is_null() && (*format).kind == 1 && (*format).direct[7] != 0
            })
            .map(|info| info.visual);
        XFree(infos.cast());
        visual
    }
}

impl Container {
    pub fn new(window: &Window) -> Self {
        let connection = window.fonts.connection.clone();
        let display = connection.display;
        unsafe {
            let screen = connection.screen;
            let mut attrs: WindowAttributes = core::mem::zeroed();
            attrs.background_pixel = 0;
            let id = XCreateWindow(
                display,
                XRootWindow(display, screen),
                0,
                0,
                window.width as u32,
                window.height as u32,
                0,
                XDefaultDepth(display, screen),
                1,
                XDefaultVisual(display, screen),
                1 << 1,
                &mut attrs,
            );
            XSelectInput(display, id, 131072 | 2097152 | 32768);
            let mut wm_delete = XInternAtom(display, c"WM_DELETE_WINDOW".as_ptr(), 0);
            XSetWMProtocols(display, id, &mut wm_delete, 1);
            let mut hint = ClassHint {
                name: c"ztty".as_ptr().cast_mut(),
                class: c"Ztty".as_ptr().cast_mut(),
            };
            XSetClassHint(display, id, &mut hint);
            let pid = crate::os::getpid() as c_ulong;
            let pid_atom = XInternAtom(display, c"_NET_WM_PID".as_ptr(), 0);
            XChangeProperty(
                display,
                id,
                pid_atom,
                6,
                32,
                0,
                (&pid as *const c_ulong).cast(),
                1,
            );
            XMapWindow(display, id);
            XFlush(display);
            Self {
                connection,
                id,
                width: window.width,
                height: window.height,
                wm_delete,
                configure_serial: 0,
            }
        }
    }

    pub fn display(&self) -> *mut Display {
        self.connection.display
    }

    pub fn resize(&mut self, width: i32, height: i32) {
        self.width = width.max(1);
        self.height = height.max(1);
        unsafe {
            self.configure_serial = XNextRequest(self.display());
            XResizeWindow(
                self.display(),
                self.id,
                self.width as u32,
                self.height as u32,
            );
            XFlush(self.display());
        }
    }

    pub fn title(&self, text: &str) {
        if self.id == 0 {
            return;
        }
        let Ok(title) = CString::new(text) else {
            return;
        };
        unsafe {
            let display = self.display();
            XStoreName(display, self.id, title.as_ptr());
            let name = XInternAtom(display, c"_NET_WM_NAME".as_ptr(), 0);
            let utf8 = XInternAtom(display, c"UTF8_STRING".as_ptr(), 0);
            XChangeProperty(
                display,
                self.id,
                name,
                utf8,
                8,
                0,
                text.as_ptr(),
                text.len() as i32,
            );
        }
    }

    pub fn raise(&self) {
        if self.id != 0 {
            unsafe {
                XMapRaised(self.display(), self.id);
                XFlush(self.display());
            }
        }
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        if self.id != 0 {
            unsafe {
                XDestroyWindow(self.display(), self.id);
            }
        }
    }
}

impl Window {
    pub fn new_shared(
        cols: usize,
        rows: usize,
        appearance: &Appearance,
        connection: &Rc<Connection>,
    ) -> Result<Self, &'static str> {
        Self::new_view(cols, rows, appearance, connection, true)
    }

    pub fn new_view(
        cols: usize,
        rows: usize,
        appearance: &Appearance,
        connection: &Rc<Connection>,
        activate: bool,
    ) -> Result<Self, &'static str> {
        let padding = appearance.padding;
        let background = appearance.theme.background;
        let fonts = connection.fonts(&appearance.font)?;
        let font = fonts.font;
        unsafe {
            let display = connection.display;
            let screen = connection.screen;
            let root = XRootWindow(display, screen);
            let alpha = if appearance.opacity < 100 {
                alpha_visual(display, screen)
            } else {
                None
            };
            let (visual, colormap, depth) = if let Some(visual) = alpha {
                (visual, XCreateColormap(display, root, visual, 0), 32)
            } else {
                (
                    XDefaultVisual(display, screen),
                    XDefaultColormap(display, screen),
                    XDefaultDepth(display, screen) as u32,
                )
            };
            let mut ext: GlyphInfo = core::mem::zeroed();
            XftTextExtentsUtf8(display, font, b"M".as_ptr(), 1, &mut ext);
            let cw = i32::from(ext.x_off).max(1);
            let ch = ((*font).ascent + (*font).descent + 3).max(1);
            let width = cols as i32 * cw + padding * 2;
            let height = rows as i32 * ch + padding * 2;
            let mut attrs: WindowAttributes = core::mem::zeroed();
            attrs.colormap = colormap;
            let id = XCreateWindow(
                display,
                root,
                0,
                0,
                width as u32,
                height as u32,
                0,
                depth as c_int,
                1,
                visual,
                (1 << 3) | (1 << 13),
                &mut attrs,
            );
            if alpha.is_some() && appearance.blur {
                let blur = XInternAtom(display, c"_KDE_NET_WM_BLUR_BEHIND_REGION".as_ptr(), 0);
                // An empty CARDINAL region requests blur across the window.
                XChangeProperty(display, id, blur, 6, 32, 0, core::ptr::null(), 0);
            }
            let events = 1 | 2 | 4 | 8 | 64 | 32768 | 131072 | 2097152 | 4194304;
            XSelectInput(display, id, events);
            let mut wm_delete = XInternAtom(display, c"WM_DELETE_WINDOW".as_ptr(), 0);
            XSetWMProtocols(display, id, &mut wm_delete, 1);
            let mut hint = ClassHint {
                name: c"ztty".as_ptr().cast_mut(),
                class: c"Ztty".as_ptr().cast_mut(),
            };
            XSetClassHint(display, id, &mut hint);
            let pid = crate::os::getpid() as c_ulong;
            let pid_atom = XInternAtom(display, c"_NET_WM_PID".as_ptr(), 0);
            XChangeProperty(
                display,
                id,
                pid_atom,
                6,
                32,
                0,
                (&pid as *const c_ulong).cast(),
                1,
            );
            if !activate {
                // EWMH user-time zero requests an initial map without activation.
                // Explicit clicks may still focus the view for copy/search.
                let user_time = XInternAtom(display, c"_NET_WM_USER_TIME".as_ptr(), 0);
                let zero: c_ulong = 0;
                XChangeProperty(
                    display,
                    id,
                    user_time,
                    6,
                    32,
                    0,
                    (&zero as *const c_ulong).cast(),
                    1,
                );
            }
            let pixmap = XCreatePixmap(display, id, width as u32, height as u32, depth);
            let gc = XCreateGC(display, id, 0, core::ptr::null_mut());
            let draw = XftDrawCreate(display, pixmap, visual, colormap);
            let ic = connection.input_context(id, events);
            let fd = connection.fd;
            let mut out = Self {
                state: crate::display::WindowState {
                    display,
                    id,
                    fd,
                    cw,
                    ch,
                    width,
                    height,
                    wm_delete,
                    configure_serial: core::cell::Cell::new(0),
                    focused: activate,
                },
                depth,
                visual,
                colormap,
                owns_colormap: alpha.is_some(),
                requested_opacity: appearance.opacity,
                pixmap,
                retained_valid: false,
                pending_damage: None,
                gc,
                draw,
                gpu: None,
                renderer_kind: appearance.renderer,
                gpu_repaint: false,
                gpu_notice: None,
                text_blink_visible: [true; 2],
                font,
                fonts,
                colors: Vec::with_capacity(256),
                next_color: 0,
                ic,
                clipboard: XInternAtom(display, c"CLIPBOARD".as_ptr(), 0),
                utf8: XInternAtom(display, c"UTF8_STRING".as_ptr(), 0),
                targets: XInternAtom(display, c"TARGETS".as_ptr(), 0),
                transfer: XInternAtom(display, c"ZTTY_SELECTION".as_ptr(), 0),
                osc_requestor: 0,
                paste_limit: MAX_SELECTION_BYTES,
                incr: XInternAtom(display, c"INCR".as_ptr(), 0),
                title_atom: XInternAtom(display, c"_NET_WM_NAME".as_ptr(), 0),
                clipboard_text: String::new(),
                primary_text: String::new(),
                incoming: Vec::new(),
                incoming_type: 0,
                receiving: false,
                paste_selection: 0,
                paste_target: 0,
                paste_error: None,
                last_cursor: None,
                padding,
                background,
                background_fill: alpha.map(|_| {
                    let opacity = u32::from(appearance.opacity);
                    let component = |shift: u32| {
                        let channel = (background >> shift) & 255;
                        (channel * 257 * opacity / 100) as u16
                    };
                    RenderColor {
                        red: component(16),
                        green: component(8),
                        blue: component(0),
                        alpha: (65535 * opacity / 100) as u16,
                    }
                }),
                cursor_color: appearance.cursor_color,
                selection_colors: (
                    appearance.selection_foreground,
                    appearance.selection_background,
                ),
                images: Vec::new(),
                image_bytes: 0,
                image_clock: 0,
                graphics_revision: 0,
                graphics_alternate: false,
                suspended: false,
                search_cols: 0,
                search_bits: Vec::new(),
            };
            out.title("ztty");
            out.fill_background(background, true, 0, 0, width as u32, height as u32);
            XMapWindow(display, id);
            XFlush(display);
            out.configure_gpu(appearance);
            Ok(out)
        }
    }
    pub fn title(&mut self, text: &str) {
        let text = if text.is_empty() { "ztty" } else { text };
        let Ok(title) = CString::new(text) else {
            return;
        };
        unsafe {
            XStoreName(self.state.display, self.state.id, title.as_ptr());
            XChangeProperty(
                self.state.display,
                self.state.id,
                self.title_atom,
                self.utf8,
                8,
                0,
                text.as_ptr(),
                text.len() as i32,
            );
        }
    }
    pub fn coordinates(&self, x: i32, y: i32, cols: usize, rows: usize) -> (usize, usize) {
        (
            (((x - self.padding).max(0) / self.state.cw) as usize).min(cols.saturating_sub(1)),
            (((y - self.padding).max(0) / self.state.ch) as usize).min(rows.saturating_sub(1)),
        )
    }

    pub fn reconfigure(
        &mut self,
        appearance: &Appearance,
        before: impl FnOnce(usize, usize, i32, i32) -> Result<(), String>,
    ) -> Result<(usize, usize), String> {
        if !self.owns_colormap && appearance.opacity < 100 && self.requested_opacity == 100 {
            return Err(
                "this opaque window needs detach/attach to enable a transparency visual".into(),
            );
        }
        let fonts = self.fonts.connection.fonts(&appearance.font)?;
        let font = fonts.font;
        let mut metrics: GlyphInfo = unsafe { core::mem::zeroed() };
        unsafe {
            XftTextExtentsUtf8(self.state.display, font, b"M".as_ptr(), 1, &mut metrics);
        }
        let cw = i32::from(metrics.x_off).max(1);
        let ch = unsafe { (*font).ascent + (*font).descent + 3 }.max(1);
        if cw > 512 || ch > 1024 {
            return Err("font cell size exceeds the supported limit".into());
        }
        let cols = (((self.state.width - appearance.padding * 2) / cw).max(1) as usize).min(512);
        let rows = (((self.state.height - appearance.padding * 2) / ch).max(1) as usize).min(256);
        // Font validation and the host's fallible PTY update happen before any
        // live window state changes, so rejected reloads retain the old view.
        before(cols, rows, cw, ch)?;
        self.fonts = fonts;
        self.font = font;
        self.state.cw = cw;
        self.state.ch = ch;
        self.requested_opacity = appearance.opacity;
        self.padding = appearance.padding;
        self.background = appearance.theme.background;
        self.cursor_color = appearance.cursor_color;
        self.selection_colors = (
            appearance.selection_foreground,
            appearance.selection_background,
        );
        if self.owns_colormap {
            let opacity = u32::from(appearance.opacity);
            let component =
                |shift: u32| (((self.background >> shift) & 255) * 257 * opacity / 100) as u16;
            self.background_fill = Some(RenderColor {
                red: component(16),
                green: component(8),
                blue: component(0),
                alpha: (65535 * opacity / 100) as u16,
            });
            unsafe {
                let blur = XInternAtom(
                    self.state.display,
                    c"_KDE_NET_WM_BLUR_BEHIND_REGION".as_ptr(),
                    0,
                );
                if appearance.blur {
                    XChangeProperty(
                        self.state.display,
                        self.state.id,
                        blur,
                        6,
                        32,
                        0,
                        core::ptr::null(),
                        0,
                    );
                } else {
                    XDeleteProperty(self.state.display, self.state.id, blur);
                }
            }
        }
        self.last_cursor = None;
        self.graphics_revision = 0;
        self.configure_gpu(appearance);
        Ok((cols, rows))
    }
    /// Request a character-grid resize. ConfigureNotify applies the resulting
    /// pixel size to the backing pixmap and the host's terminal model.
    pub fn resize_grid(&self, cols: usize, rows: usize) {
        if self.state.id == 0 {
            return;
        }
        let width = cols
            .saturating_mul(self.state.cw as usize)
            .saturating_add((self.padding * 2) as usize)
            .clamp(1, u16::MAX as usize);
        let height = rows
            .saturating_mul(self.state.ch as usize)
            .saturating_add((self.padding * 2) as usize)
            .clamp(1, u16::MAX as usize);
        unsafe {
            self.state
                .configure_serial
                .set(XNextRequest(self.state.display));
            XResizeWindow(
                self.state.display,
                self.state.id,
                width as c_uint,
                height as c_uint,
            );
            XFlush(self.state.display);
        }
    }

    pub fn resize(&mut self, width: i32, height: i32) -> (usize, usize) {
        if self.state.width != width.max(1) || self.state.height != height.max(1) {
            self.state.width = width.max(1);
            self.state.height = height.max(1);
            self.retained_valid = false;
            self.pending_damage = None;
            self.resize_gpu();
            if !self.suspended {
                unsafe {
                    let pixmap = XCreatePixmap(
                        self.state.display,
                        self.state.id,
                        self.state.width as u32,
                        self.state.height as u32,
                        self.depth,
                    );
                    XftDrawChange(self.draw, pixmap);
                    XFreePixmap(self.state.display, self.pixmap);
                    self.pixmap = pixmap;
                }
            }
        }
        (
            (((self.state.width - self.padding * 2) / self.state.cw).max(1) as usize).min(512),
            (((self.state.height - self.padding * 2) / self.state.ch).max(1) as usize).min(256),
        )
    }

    pub fn parent(&mut self, parent: Option<Xid>) {
        if self.state.id == 0 {
            return;
        }
        unsafe {
            let root = XRootWindow(self.state.display, self.fonts.connection.screen);
            XUnmapWindow(self.state.display, self.state.id);
            self.state
                .configure_serial
                .set(XNextRequest(self.state.display));
            XReparentWindow(
                self.state.display,
                self.state.id,
                parent.unwrap_or(root),
                0,
                0,
            );
            if !self.suspended {
                XMapWindow(self.state.display, self.state.id);
            }
            XFlush(self.state.display);
        }
    }

    pub fn place(&mut self, rect: crate::layout::Rect) -> (usize, usize) {
        unsafe {
            self.state
                .configure_serial
                .set(XNextRequest(self.state.display));
            XMoveResizeWindow(
                self.state.display,
                self.state.id,
                rect.x,
                rect.y,
                rect.width.max(1) as u32,
                rect.height.max(1) as u32,
            );
        }
        self.resize(rect.width, rect.height)
    }

    pub fn visible(&mut self, visible: bool) {
        if self.state.id == 0 || visible != self.suspended {
            return;
        }
        self.retained_valid = false;
        self.pending_damage = None;
        unsafe {
            if visible {
                let pixmap = XCreatePixmap(
                    self.state.display,
                    self.state.id,
                    self.state.width as u32,
                    self.state.height as u32,
                    self.depth,
                );
                XftDrawChange(self.draw, pixmap);
                XFreePixmap(self.state.display, self.pixmap);
                self.pixmap = pixmap;
                self.last_cursor = None;
                XMapWindow(self.state.display, self.state.id);
            } else {
                XUnmapWindow(self.state.display, self.state.id);
                // Hidden tabs keep canonical pixels but release all rebuildable
                // server uploads and their full-size retained surface.
                while !self.images.is_empty() {
                    self.discard_upload(self.images.len() - 1);
                }
                let pixmap = XCreatePixmap(self.state.display, self.state.id, 1, 1, self.depth);
                XftDrawChange(self.draw, pixmap);
                XFreePixmap(self.state.display, self.pixmap);
                self.pixmap = pixmap;
                self.focus(false);
            }
            self.suspended = !visible;
            self.visible_gpu(visible);
            XFlush(self.state.display);
        }
    }
    /// Bring an already attached session back without opening a second window.
    pub fn raise(&self) {
        if self.state.id == 0 {
            return;
        }
        unsafe {
            XMapRaised(self.state.display, self.state.id);
            XSetInputFocus(self.state.display, self.state.id, 1, 0);
            XFlush(self.state.display);
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.close_osc_requestor();
        self.gpu = None;
        while !self.images.is_empty() {
            self.discard_upload(self.images.len() - 1);
        }
        unsafe {
            self.ic.close();
            XftDrawDestroy(self.draw);
            for color in &mut self.colors {
                XftColorFree(
                    self.state.display,
                    self.visual,
                    self.colormap,
                    &mut color.color,
                );
            }
            XFreeGC(self.state.display, self.gc);
            XFreePixmap(self.state.display, self.pixmap);
            if self.state.id != 0 {
                XDestroyWindow(self.state.display, self.state.id);
            }
            if self.owns_colormap {
                XFreeColormap(self.state.display, self.colormap);
            }
        }
    }
}

impl core::ops::Deref for Window {
    type Target = crate::display::WindowState;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}
impl core::ops::DerefMut for Window {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}
