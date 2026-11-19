//! Native Win32 frontend. GDI draws text into a retained DIB; the image path
//! composites decoded RGBA pixels itself. ConPTY pipe work lives in zpty.

use crate::{Config, appearance::Appearance, os};
use alloc::{boxed::Box, ffi::CString, format, string::String, sync::Arc, vec, vec::Vec};
use core::{
    cell::{Cell, UnsafeCell},
    ffi::c_void,
    ptr,
};
use zterm::{
    CursorShape, Terminal,
    input::{self, Key, KeyEvent, ModifierKey, Modifiers, MouseEvent},
};

use ztty::{
    actions,
    layout::{Axis, Layout, Rect},
    native_resize as session_resize,
    session_core::{ReadStep, SessionCore},
};

#[path = "windows/diagnostics.rs"]
mod diagnostics;
#[allow(non_snake_case)]
#[path = "windows/ffi.rs"]
mod ffi;
#[path = "windows/interaction.rs"]
mod interaction;
#[path = "windows/layout.rs"]
mod layout;
#[path = "windows/painting.rs"]
mod painting;
#[path = "windows/session.rs"]
mod session;
#[path = "windows/surface.rs"]
mod surface;
#[path = "windows/window.rs"]
mod window;

use diagnostics::{
    NativeEvents, NativeTrace, NcCalcEvent, NcCalcStage, PaintAfterEnd, PaintRecord, PaintTarget,
    PositionEvent, SurfaceResizeRecord,
};
pub(crate) use interaction::action_supported;
use surface::{Fonts, Surface};
pub use window::run;

#[path = "windows_dispatch.rs"]
mod session_dispatch;
#[path = "windows_frame.rs"]
mod session_frame;
#[path = "windows_wait.rs"]
mod session_wait;

type Handle = *mut c_void;
const NULL: Handle = ptr::null_mut();
const WINDOW_STYLE: u32 = 0x00cf0000;
const MAX_INPUT: usize = 8 * 1024 * 1024;
const MAX_SURFACE_PIXELS: usize = 16 * 1024 * 1024;
const GWLP_USERDATA: i32 = -21;
const WM_PAINT: u32 = 0x000f;
const WM_CLOSE: u32 = 0x0010;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_NCCALCSIZE: u32 = 0x0083;
const WM_WINDOWPOSCHANGING: u32 = 0x0046;
const WVR_VALIDRECTS: isize = 0x0400;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOREDRAW: u32 = 0x0008;
const SWP_NOCOPYBITS: u32 = 0x0100;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_SYSCHAR: u32 = 0x0106;
const WM_NATIVE_SNAPSHOT: u32 = 0x85a1;
const WM_NATIVE_PAINT_SAMPLE: u32 = 0x85a2;
const NATIVE_SNAPSHOT_MAGIC: usize = 0x5a545459;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}
fn colorref(rgb: u32) -> u32 {
    (rgb & 0xff00) | ((rgb >> 16) & 0xff) | ((rgb & 0xff) << 16)
}

#[derive(Clone, Copy)]
struct Selection {
    start: (usize, usize),
    end: (usize, usize),
}

#[derive(Clone, Copy)]
struct PendingKey {
    key: Key,
    modifiers: Modifiers,
    event: KeyEvent,
}

struct Pane {
    // Drop waits before the handles owned by core. Callbacks never touch App.
    output_wait: session_wait::Wait,
    _process_wait: session_wait::Wait,
    id: u64,
    visible: bool,
    pty_size: zpty::Size,
    title: String,
    rect: Rect,
    core: SessionCore,
    dirty: bool,
    history: usize,
    selection: Option<Selection>,
    dragging: bool,
    graphics_revision: u64,
    sync_since: Option<u64>,
    held_frame: Option<session_frame::Snapshot>,
    cursor_on: bool,
    blink_deadline: u64,
    last_cursor: (usize, usize),
}

struct Launch {
    shell: CString,
    environment: Vec<u16>,
    history: usize,
}

struct App {
    hwnd: Handle,
    panes: Vec<Pane>,
    active: usize,
    layout: Layout,
    next_id: u64,
    launch: Launch,
    wake: Arc<session_wait::Wake>,
    pump_cursor: usize,
    pump_again: bool,
    pending_fonts: Option<Fonts>,
    resize_pending: bool,
    resize_retry_at: u64,
    full_redraw: bool,
    caption_dirty: bool,
    notice: Option<(String, u64)>,
    last_status: i32,
    appearance: Appearance,
    surface: Surface,
    fonts: Fonts,
    encoded: Vec<u8>,
    running: bool,
    held: [Option<Key>; 256],
    intercepted: [bool; 256],
    pending_key: Option<PendingKey>,
    suppress_char: bool,
    surrogate: u16,
    mouse_capture: Option<(u64, u8)>,
    cancelled_mouse_buttons: u8,
    title: String,
    focused: bool,
    native_trace: NativeTrace,
    native_snapshot_path: Option<Vec<u16>>,
    surface_resize: Option<SurfaceResizeRecord>,
    last_paint: PaintRecord,
}

#[path = "windows/osc52.rs"]
mod osc52;
