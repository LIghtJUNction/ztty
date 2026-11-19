//! Native Wayland windows hosted by the same broker, UI and PTY pool as X11.
//! This module owns display resources only; it never spawns a shell.
use crate::{
    appearance::{Appearance, RendererKind},
    display::WindowState,
    os, x11,
};
use alloc::{
    ffi::CString,
    rc::{Rc, Weak},
    string::String,
    vec::Vec,
};
use core::{
    cell::{Cell, RefCell},
    ffi::{CStr, c_char, c_int, c_ulong, c_void},
    ptr::NonNull,
};
use x11::{KeyEvent, Lookup, PropertyEvent, Selection, SelectionEvent, SelectionRequest, Xid};
use zterm::{CursorShape, Terminal};
use ztty::gpu::{Display, GridStyle, Rect, Renderer, Style, Target};
#[path = "wayland/clipboard.rs"]
mod clipboard;
#[path = "wayland/connection.rs"]
mod connection;
#[path = "wayland/overlays.rs"]
mod overlays;
#[path = "wayland/painting.rs"]
mod painting;
#[path = "wayland/window.rs"]
mod window;

type DrawnCursor = ((usize, usize), CursorShape);

#[repr(C)]
#[derive(Clone, Copy)]
struct Event {
    kind: u32,
    symbol: u32,
    modifiers: u32,
    state: u32,
    x: i32,
    y: i32,
    keycode: u32,
    shifted: u32,
    text: [u8; 64],
}
impl Default for Event {
    fn default() -> Self {
        unsafe { core::mem::zeroed() }
    }
}
unsafe extern "C" {
    fn ztty_wl_create(
        width: i32,
        height: i32,
        endpoint: *const c_char,
        locale: *const c_char,
    ) -> *mut c_void;
    fn ztty_wl_destroy(w: *mut c_void);
    fn ztty_wl_display(w: *mut c_void) -> *mut c_void;
    fn ztty_wl_window(w: *mut c_void) -> usize;
    fn ztty_wl_next(w: *mut c_void, e: *mut Event) -> c_int;
    fn ztty_wl_resize(w: *mut c_void, width: i32, height: i32) -> c_int;
    fn ztty_wl_title(w: *mut c_void, title: *const c_char);
    fn ztty_wl_ready(w: *mut c_void) -> c_int;
    fn ztty_wl_request_frame(w: *mut c_void) -> c_int;
    fn ztty_wl_pump(w: *mut c_void, fd: c_int, write: c_int, timeout: c_int) -> c_int;
    fn ztty_wl_fd(w: *mut c_void) -> c_int;
    fn ztty_wl_events(w: *mut c_void) -> c_int;
    fn ztty_wl_deadline(w: *mut c_void) -> u64;
    fn ztty_wl_pending(w: *mut c_void) -> c_int;
    fn ztty_wl_grid_size(w: *mut c_void, width: i32, height: i32);
    fn ztty_wl_service_fds(w: *mut c_void, fds: *mut os::PollFd, cap: usize) -> usize;
    fn ztty_wl_copy(w: *mut c_void, text: *const u8, len: usize, primary: c_int) -> c_int;
    fn ztty_wl_clip_available(w: *mut c_void, primary: c_int) -> c_int;
    fn ztty_wl_receive(w: *mut c_void, primary: c_int, limit: usize, generation: u32) -> c_int;
    fn ztty_wl_cancel_receive(w: *mut c_void);
    fn ztty_wl_clip_result(
        w: *mut c_void,
        generation: u32,
        len: *mut usize,
        error: *mut c_int,
    ) -> *const u8;
    fn ztty_wl_ime_focus(w: *mut c_void, focus: c_int);
    fn ztty_wl_ime_cursor(w: *mut c_void, x: i32, y: i32, width: i32, height: i32);
    fn ztty_wl_preedit(w: *mut c_void) -> *const c_char;

}
struct Native {
    ptr: NonNull<c_void>,
    last: Cell<Event>,
    focused: Cell<bool>,
    failed: Cell<bool>,
    closed: Cell<bool>,
    buttons: Cell<u32>,
    scroll: Cell<i32>,
    created: u64,
}
pub struct Connection {
    endpoint: CString,
    locale: CString,
    windows: RefCell<Vec<Weak<Native>>>,
}
pub struct Window {
    pub state: WindowState,
    // EGL surface/context must die before the native display/window.
    renderer: Renderer,
    native: Rc<Native>,
    appearance: Appearance,
    ascent: f32,
    repaint: Cell<bool>,
    drawing: bool,
    suspended: bool,
    notice: Option<String>,
    last_cursor: Option<DrawnCursor>,
    text_blink_visible: [bool; 2],
    search_cols: usize,
    search_bits: Vec<u64>,
    graphics_revision: u64,
    graphics_alternate: bool,
    paste_generation: u32,
    paste_pending: bool,
    paste_error: Option<&'static str>,
}
