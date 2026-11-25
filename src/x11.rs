//! X11/XWayland windows with a shared GPU renderer and Xft fallback.

mod clipboard;
mod connection;
mod ffi;
mod fonts;
mod gpu;
mod images;
mod input;
mod osc52;
mod render;
mod ui;
mod window;

pub use ffi::*;
pub use input::input_focus;

use alloc::ffi::CString;
use alloc::format;
use alloc::rc::{Rc, Weak};
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use zterm::{
    BLINK, BOLD, Cell, CursorShape, DIM, EXPLICIT_BG, HIDDEN, INVERSE, ITALIC, RAPID_BLINK, STRIKE,
    Terminal, UNDERLINE,
};

use crate::appearance::Appearance;

#[derive(Clone, Copy)]
pub struct Selection {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub dragging: bool,
}
impl Selection {
    pub fn contains(&self, col: usize, row: usize, cols: usize) -> bool {
        let a = self.start.1 * cols + self.start.0;
        let b = self.end.1 * cols + self.end.0;
        let at = row * cols + col;
        (a.min(b)..=a.max(b)).contains(&at)
    }
    pub fn text(&self, terminal: &Terminal, offset: usize) -> String {
        ztty::selection::text(terminal, self.start, self.end, offset)
    }
}

struct ColorEntry {
    rgb: u32,
    color: Color,
}
pub struct Lookup {
    pub len: usize,
    pub symbol: c_ulong,
    pub text_only: bool,
    pub ignored: bool,
}

type DrawnCursor = ((usize, usize), CursorShape);

/// A workspace adds one native parent, without a duplicate terminal backing
/// pixmap, font set or terminal model. Children retain their PTYs when moved.
pub struct Container {
    connection: Rc<Connection>,
    pub id: Xid,
    pub width: i32,
    pub height: i32,
    pub wm_delete: Atom,
    pub configure_serial: c_ulong,
}

/// Shared X11 connection for a single-threaded host of terminal windows.
///
/// Font entries are weak: closing their last window releases the faces and
/// glyph cache even if headless sessions keep the host process alive.
pub struct Connection {
    pub display: *mut Display,
    pub fd: i32,
    screen: i32,
    im: core::cell::Cell<*mut c_void>,
    im_root: Xid,
    im_servers: Atom,
    im_retry: core::cell::Cell<bool>,
    contexts: RefCell<Vec<Weak<InputContext>>>,
    fonts: RefCell<Vec<Weak<Fonts>>>,
}

#[repr(C)]
struct ImCallback {
    data: *mut c_void,
    callback: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void),
}

struct InputContext {
    connection: Rc<Connection>,
    window: Xid,
    events: c_long,
    handle: core::cell::Cell<*mut c_void>,
    focused: core::cell::Cell<bool>,
}

struct Fonts {
    connection: Rc<Connection>,
    name: String,
    font: *mut Font,
    fallbacks: RefCell<Vec<*mut Font>>,
    italic: RefCell<Vec<*mut Font>>,
}

/// The canonical image stays in zgraphics. This bounded server-side cache only
/// owns premultiplied ARGB uploads, reused by every placement and damaged row.
struct ImageUpload {
    image_id: u32,
    generation: u64,
    picture: Xid,
    pixmap: Xid,
    bytes: usize,
    used: u64,
}

const MAX_SELECTION_BYTES: usize = 4 * 1024 * 1024;

pub struct Window {
    pub state: crate::display::WindowState,
    depth: u32,
    visual: *mut c_void,
    colormap: c_ulong,
    owns_colormap: bool,
    requested_opacity: u8,
    pixmap: Xid,
    // GPU views keep only a placeholder; software frames require this exact size.
    pixmap_size: (i32, i32),
    // Only a completed software frame may be copied on an Expose event.
    retained_valid: bool,
    // A workspace keeps its previous tab above this mapped view until paint.
    reveal_pending: bool,
    pending_damage: Option<(i32, i32)>,
    gc: *mut c_void,
    draw: *mut XftDraw,
    gpu: Option<crate::gpu_worker::Renderer>,
    renderer_kind: crate::appearance::RendererKind,
    gpu_repaint: bool,
    gpu_notice: Option<&'static str>,
    text_blink_visible: [bool; 2],
    font: *mut Font,
    fonts: Rc<Fonts>,
    colors: Vec<ColorEntry>,
    next_color: usize,
    ic: Rc<InputContext>,
    clipboard: Atom,
    utf8: Atom,
    utf8_mime: Atom,
    text_mime: Atom,
    targets: Atom,
    transfer: Atom,
    osc_requestor: Xid,
    paste_limit: usize,
    incr: Atom,
    title_atom: Atom,
    clipboard_text: String,
    primary_text: String,
    incoming: Vec<u8>,
    incoming_type: Atom,
    receiving: bool,
    paste_selection: Atom,
    paste_target: Atom,
    paste_error: Option<&'static str>,
    last_cursor: Option<DrawnCursor>,
    padding: i32,
    background: u32,
    background_fill: Option<RenderColor>,
    cursor_color: u32,
    selection_colors: (u32, u32),
    images: Vec<ImageUpload>,
    image_bytes: usize,
    image_clock: u64,
    graphics_revision: u64,
    graphics_alternate: bool,
    suspended: bool,
    search_cols: usize,
    search_bits: Vec<u64>,
}
