//! Native display facade. Session ownership, input policy and PTYs stay in Pool.
use crate::{appearance::Appearance, os, x11};
use alloc::{ffi::CString, rc::Rc, string::String, vec::Vec};
use core::ffi::{CStr, c_int, c_ulong};
use x11::{
    Display, KeyEvent, Lookup, PropertyEvent, Selection, SelectionEvent, SelectionRequest, Xid,
};
use zterm::{CursorShape, Terminal};
type DrawnCursor = ((usize, usize), CursorShape);
pub struct WindowState {
    pub display: *mut crate::x11::Display,
    pub id: crate::x11::Xid,
    pub fd: i32,
    pub cw: i32,
    pub ch: i32,
    pub width: i32,
    pub height: i32,
    pub wm_delete: crate::x11::Atom,
    pub configure_serial: core::cell::Cell<core::ffi::c_ulong>,
    pub focused: bool,
}

pub enum Window {
    X11(alloc::boxed::Box<x11::Window>),
    #[cfg(feature = "wayland")]
    Wayland(alloc::boxed::Box<crate::wayland::Window>),
}
impl core::ops::Deref for Window {
    type Target = WindowState;
    fn deref(&self) -> &WindowState {
        match self {
            Self::X11(w) => &w.state,
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => &w.state,
        }
    }
}
impl core::ops::DerefMut for Window {
    fn deref_mut(&mut self) -> &mut WindowState {
        match self {
            Self::X11(w) => &mut w.state,
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => &mut w.state,
        }
    }
}
impl Window {
    pub fn backend(&self) -> &'static str {
        match self {
            Self::X11(_) => "x11",
            #[cfg(feature = "wayland")]
            Self::Wayland(_) => "wayland",
        }
    }
    pub fn failed(&self) -> bool {
        match self {
            Self::X11(_) => false,
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.failed(),
        }
    }

    pub fn x11(&self) -> Result<&x11::Window, &'static str> {
        match self {
            Self::X11(w) => Ok(w),
            #[cfg(feature = "wayland")]
            Self::Wayland(_) => {
                Err("Wayland workspaces are not available; use independent session windows")
            }
        }
    }
    pub fn new_shared(
        cols: usize,
        rows: usize,
        appearance: &Appearance,
        connection: &Rc<Connection>,
    ) -> Result<Self, &'static str> {
        match &connection.kind {
            ConnectionKind::X11(c) => x11::Window::new_shared(cols, rows, appearance, c)
                .map(|window| Self::X11(alloc::boxed::Box::new(window))),
            #[cfg(feature = "wayland")]
            ConnectionKind::Wayland(c) => {
                crate::wayland::Window::new(cols, rows, appearance, c, connection.display, true)
                    .map(|window| Self::Wayland(alloc::boxed::Box::new(window)))
            }
        }
    }
    pub fn new_view(
        cols: usize,
        rows: usize,
        appearance: &Appearance,
        connection: &Rc<Connection>,
        activate: bool,
    ) -> Result<Self, &'static str> {
        match &connection.kind {
            ConnectionKind::X11(c) => x11::Window::new_view(cols, rows, appearance, c, activate)
                .map(|window| Self::X11(alloc::boxed::Box::new(window))),
            #[cfg(feature = "wayland")]
            ConnectionKind::Wayland(c) => {
                crate::wayland::Window::new(cols, rows, appearance, c, connection.display, activate)
                    .map(|window| Self::Wayland(alloc::boxed::Box::new(window)))
            }
        }
    }
    pub fn title(&mut self, text: &str) {
        match self {
            Self::X11(w) => w.title(text),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.title(text),
        }
    }
    pub fn focus(&mut self, focused: bool) {
        match self {
            Self::X11(w) => w.focus(focused),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.focus(focused),
        }
    }
    pub fn coordinates(&self, x: i32, y: i32, cols: usize, rows: usize) -> (usize, usize) {
        match self {
            Self::X11(w) => w.coordinates(x, y, cols, rows),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.coordinates(x, y, cols, rows),
        }
    }
    pub fn reconfigure(
        &mut self,
        appearance: &Appearance,
        before: impl FnOnce(usize, usize, i32, i32) -> Result<(), String>,
    ) -> Result<(usize, usize), String> {
        match self {
            Self::X11(w) => w.reconfigure(appearance, before),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.reconfigure(appearance, before),
        }
    }
    pub fn present(&self) {
        match self {
            Self::X11(w) => w.present(),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.present(),
        }
    }
    pub fn finish_frame(&mut self) {
        match self {
            Self::X11(w) => w.finish_frame(),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.finish_frame(),
        }
    }
    pub fn resize_grid(&self, cols: usize, rows: usize) {
        match self {
            Self::X11(w) => w.resize_grid(cols, rows),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.resize_grid(cols, rows),
        }
    }
    pub fn resize(&mut self, width: i32, height: i32) -> (usize, usize) {
        match self {
            Self::X11(w) => w.resize(width, height),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.resize(width, height),
        }
    }
    pub fn parent(&mut self, parent: Option<Xid>) {
        match self {
            Self::X11(w) => w.parent(parent),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.parent(parent),
        }
    }
    pub fn place(&mut self, rect: crate::layout::Rect) -> (usize, usize) {
        match self {
            Self::X11(w) => w.place(rect),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.place(rect),
        }
    }
    pub fn visible(&mut self, visible: bool) {
        match self {
            Self::X11(w) => w.visible(visible),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.visible(visible),
        }
    }
    pub fn stage_visible(&mut self) {
        match self {
            Self::X11(w) => w.stage_visible(),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.visible(true),
        }
    }
    pub fn input_symbol(&self, event: &mut KeyEvent) -> c_ulong {
        match self {
            Self::X11(w) => w.input_symbol(event),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.input_symbol(event),
        }
    }
    pub fn key_symbol(&self, event: &mut KeyEvent, shifted: bool) -> c_ulong {
        match self {
            Self::X11(w) => w.key_symbol(event, shifted),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.key_symbol(event, shifted),
        }
    }
    pub fn lookup(&self, event: &mut KeyEvent, buffer: &mut Vec<u8>) -> Lookup {
        match self {
            Self::X11(w) => w.lookup(event, buffer),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.lookup(event, buffer),
        }
    }
    pub fn raise(&self) {
        match self {
            Self::X11(w) => w.raise(),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.raise(),
        }
    }
    pub fn cursor_changed(&self, cursor: Option<DrawnCursor>) -> bool {
        match self {
            Self::X11(w) => w.cursor_changed(cursor),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.cursor_changed(cursor),
        }
    }
    pub fn search_matches(&mut self, cols: usize, bits: Vec<u64>) {
        match self {
            Self::X11(w) => w.search_matches(cols, bits),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.search_matches(cols, bits),
        }
    }
    pub fn clipboard_menu_bounds(&self, x: i32, y: i32) -> crate::layout::Rect {
        match self {
            Self::X11(w) => w.clipboard_menu_bounds(x, y),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.clipboard_menu_bounds(x, y),
        }
    }
    pub fn clipboard_menu(
        &mut self,
        bounds: crate::layout::Rect,
        selected: usize,
        enabled: [bool; 2],
        shortcuts: [&str; 2],
    ) {
        match self {
            Self::X11(w) => w.clipboard_menu(bounds, selected, enabled, shortcuts),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.clipboard_menu(bounds, selected, enabled, shortcuts),
        }
    }
    pub fn overlay(&mut self, text: &str) {
        match self {
            Self::X11(w) => w.overlay(text),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.overlay(text),
        }
    }
    pub fn overlay_lines(&mut self, lines: &[String]) {
        match self {
            Self::X11(w) => w.overlay_lines(lines),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.overlay_lines(lines),
        }
    }
    pub fn scroll_prompt(
        &mut self,
        new_output: bool,
        overlay_rows: usize,
    ) -> Option<crate::layout::Rect> {
        match self {
            Self::X11(w) => w.scroll_prompt(new_output, overlay_rows),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.scroll_prompt(new_output, overlay_rows),
        }
    }
    pub fn native_ui_geometry(&self) -> ztty::native_ui::Geometry {
        match self {
            Self::X11(w) => w.native_ui_geometry(),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.native_ui_geometry(),
        }
    }
    pub fn native_ui(
        &mut self,
        terminal: &Terminal,
        view: &ztty::native_ui::View,
        interactive: bool,
    ) {
        match self {
            Self::X11(w) => w.native_ui(terminal, view, interactive),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.native_ui(terminal, view, interactive),
        }
    }
    pub fn graphics_changed(&self, terminal: &Terminal) -> bool {
        match self {
            Self::X11(w) => w.graphics_changed(terminal),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.graphics_changed(terminal),
        }
    }
    pub fn render(
        &mut self,
        terminal: &mut Terminal,
        offset: usize,
        selection: Option<Selection>,
        force: bool,
        cursor: Option<DrawnCursor>,
    ) {
        match self {
            Self::X11(w) => w.render(terminal, offset, selection, force, cursor),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.render(terminal, offset, selection, force, cursor),
        }
    }
    pub fn copy(&mut self, text: String, primary: bool) {
        match self {
            Self::X11(w) => w.copy(text, primary),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.copy(text, primary),
        }
    }
    pub fn cancel_paste(&mut self) {
        match self {
            Self::X11(w) => w.cancel_paste(),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.cancel_paste(),
        }
    }
    pub fn paste(&mut self, primary: bool) {
        match self {
            Self::X11(w) => w.paste(primary),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.paste(primary),
        }
    }
    pub fn selection_request(&self, request: SelectionRequest) {
        match self {
            Self::X11(w) => w.selection_request(request),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.selection_request(request),
        }
    }
    pub fn take_paste_error(&mut self) -> Option<&'static str> {
        match self {
            Self::X11(w) => w.take_paste_error(),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.take_paste_error(),
        }
    }
    pub fn selection_notify(&mut self, event: SelectionEvent) -> Option<Vec<u8>> {
        match self {
            Self::X11(w) => w.selection_notify(event),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.selection_notify(event),
        }
    }
    pub fn property(&mut self, event: PropertyEvent) -> Option<Vec<u8>> {
        match self {
            Self::X11(w) => w.property(event),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.property(event),
        }
    }
    pub fn take_renderer_notice(&mut self) -> Option<String> {
        match self {
            Self::X11(w) => w.take_renderer_notice(),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.take_renderer_notice(),
        }
    }
    pub fn needs_repaint(&self) -> bool {
        match self {
            Self::X11(w) => w.needs_repaint(),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.needs_repaint(),
        }
    }
    pub fn set_text_blink_visible(&mut self, visible: [bool; 2]) {
        match self {
            Self::X11(w) => w.set_text_blink_visible(visible),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.set_text_blink_visible(visible),
        }
    }
    pub fn accepts_event(&self, window: Xid, kind: c_int) -> bool {
        match self {
            Self::X11(w) => w.accepts_event(window, kind),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.accepts_event(window, kind),
        }
    }
    pub fn paste_pending(&self) -> bool {
        match self {
            Self::X11(w) => w.paste_pending(),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.paste_pending(),
        }
    }
    pub fn clipboard_available(&self, primary: bool) -> bool {
        match self {
            Self::X11(w) => w.clipboard_available(primary),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.clipboard_available(primary),
        }
    }
    pub fn read_osc_clipboard(&mut self, primary: bool) -> bool {
        match self {
            Self::X11(w) => w.read_osc_clipboard(primary),
            #[cfg(feature = "wayland")]
            Self::Wayland(w) => w.read_osc_clipboard(primary),
        }
    }
}

enum ConnectionKind {
    X11(Rc<x11::Connection>),
    #[cfg(feature = "wayland")]
    Wayland(Rc<crate::wayland::Connection>),
}
pub struct Connection {
    // Identity only. Native Wayland pointers are never passed to Xlib.
    pub display: *mut Display,
    kind: ConnectionKind,
}
impl Connection {
    pub fn new_for_display(name: Option<&CStr>) -> Result<Rc<Self>, &'static str> {
        let c = x11::Connection::new_for_display(name)?;
        Ok(Rc::new(Self {
            display: c.display,
            kind: ConnectionKind::X11(c),
        }))
    }
    pub fn wayland(env: &[CString]) -> Result<Rc<Self>, &'static str> {
        #[cfg(feature = "wayland")]
        {
            let c = Rc::new(crate::wayland::Connection::new(env)?);
            Ok(Rc::new(Self {
                display: Rc::as_ptr(&c).cast_mut().cast(),
                kind: ConnectionKind::Wayland(c),
            }))
        }
        #[cfg(not(feature = "wayland"))]
        {
            let _ = env;
            Err("native Wayland is optional; build with --features wayland")
        }
    }
    pub fn next_event(&self) -> Option<x11::Event> {
        match &self.kind {
            ConnectionKind::X11(c) => unsafe {
                while x11::XPending(c.display) != 0 {
                    let mut event = x11::Event { pad: [0; 24] };
                    x11::XNextEvent(c.display, &mut event);
                    if x11::XFilterEvent(&mut event, 0) == 0 {
                        return Some(event);
                    }
                }
                None
            },
            #[cfg(feature = "wayland")]
            ConnectionKind::Wayland(c) => c.next_event(self.display),
        }
    }
    pub fn pending(&self) -> bool {
        match &self.kind {
            ConnectionKind::X11(c) => unsafe { x11::XPending(c.display) != 0 },
            #[cfg(feature = "wayland")]
            ConnectionKind::Wayland(c) => c.pending(),
        }
    }
    pub fn input_focus(&self) -> Xid {
        match &self.kind {
            ConnectionKind::X11(c) => x11::input_focus(c.display),
            #[cfg(feature = "wayland")]
            ConnectionKind::Wayland(c) => c.input_focus(),
        }
    }
    pub fn descriptors(&self, out: &mut Vec<os::PollFd>) {
        match &self.kind {
            ConnectionKind::X11(c) => out.push(os::PollFd {
                fd: c.fd,
                events: os::POLLIN,
                revents: 0,
            }),
            #[cfg(feature = "wayland")]
            ConnectionKind::Wayland(c) => c.descriptors(out),
        }
    }
    pub fn deadline(&self) -> Option<u64> {
        match &self.kind {
            ConnectionKind::X11(_) => None,
            #[cfg(feature = "wayland")]
            ConnectionKind::Wayland(c) => c.deadline(),
        }
    }
}
