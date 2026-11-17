//! X11/XWayland window and Xft text rendering, accessed through a small C ABI.
use alloc::ffi::CString;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void};
use zterm::{BOLD, Cell, DEFAULT_BG, DIM, HIDDEN, INVERSE, STRIKE, Terminal, UNDERLINE};

#[repr(C)]
pub struct Display {
    _private: [u8; 0],
}
#[repr(C)]
pub struct XftDraw {
    _private: [u8; 0],
}
pub type Atom = c_ulong;
pub type Xid = c_ulong;
pub const KEY_PRESS: i32 = 2;
pub const KEY_RELEASE: i32 = 3;
pub const BUTTON_PRESS: i32 = 4;
pub const BUTTON_RELEASE: i32 = 5;
pub const MOTION_NOTIFY: i32 = 6;
pub const FOCUS_IN: i32 = 9;
pub const FOCUS_OUT: i32 = 10;
pub const EXPOSE: i32 = 12;
pub const DESTROY_NOTIFY: i32 = 17;
pub const CONFIGURE_NOTIFY: i32 = 22;
pub const PROPERTY_NOTIFY: i32 = 28;
pub const SELECTION_CLEAR: i32 = 29;
pub const SELECTION_REQUEST: i32 = 30;
pub const SELECTION_NOTIFY: i32 = 31;
pub const CLIENT_MESSAGE: i32 = 33;
pub const PAD: i32 = 12;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KeyEvent {
    pub kind: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub window: Xid,
    pub root: Xid,
    pub subwindow: Xid,
    pub time: c_ulong,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub state: c_uint,
    pub keycode: c_uint,
    pub same_screen: c_int,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ButtonEvent {
    pub kind: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub window: Xid,
    pub root: Xid,
    pub subwindow: Xid,
    pub time: c_ulong,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub state: c_uint,
    pub button: c_uint,
    pub same_screen: c_int,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MotionEvent {
    pub kind: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub window: Xid,
    pub root: Xid,
    pub subwindow: Xid,
    pub time: c_ulong,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub state: c_uint,
    pub is_hint: c_char,
    pub same_screen: c_int,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ConfigureEvent {
    pub kind: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub event: Xid,
    pub window: Xid,
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub border_width: c_int,
    pub above: Xid,
    pub override_redirect: c_int,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub union ClientData {
    pub bytes: [c_char; 20],
    pub shorts: [i16; 10],
    pub longs: [c_long; 5],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClientEvent {
    pub kind: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub window: Xid,
    pub message_type: Atom,
    pub format: c_int,
    pub data: ClientData,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SelectionRequest {
    pub kind: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub owner: Xid,
    pub requestor: Xid,
    pub selection: Atom,
    pub target: Atom,
    pub property: Atom,
    pub time: c_ulong,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SelectionEvent {
    pub kind: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub requestor: Xid,
    pub selection: Atom,
    pub target: Atom,
    pub property: Atom,
    pub time: c_ulong,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PropertyEvent {
    pub kind: c_int,
    pub serial: c_ulong,
    pub send_event: c_int,
    pub display: *mut Display,
    pub window: Xid,
    pub atom: Atom,
    pub time: c_ulong,
    pub state: c_int,
}
#[repr(C)]
pub union Event {
    pub kind: c_int,
    pub key: KeyEvent,
    pub button: ButtonEvent,
    pub motion: MotionEvent,
    pub configure: ConfigureEvent,
    pub client: ClientEvent,
    pub request: SelectionRequest,
    pub selection: SelectionEvent,
    pub property: PropertyEvent,
    pub pad: [c_long; 24],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RenderColor {
    pub red: u16,
    pub green: u16,
    pub blue: u16,
    pub alpha: u16,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Color {
    pub pixel: c_ulong,
    pub value: RenderColor,
}
#[repr(C)]
pub struct Font {
    pub ascent: c_int,
    pub descent: c_int,
    pub height: c_int,
    pub max_advance_width: c_int,
    pub charset: *mut c_void,
    pub pattern: *mut c_void,
}
#[repr(C)]
struct GlyphInfo {
    width: u16,
    height: u16,
    x: i16,
    y: i16,
    x_off: i16,
    y_off: i16,
}
#[repr(C)]
struct ClassHint {
    name: *mut c_char,
    class: *mut c_char,
}

#[link(name = "X11")]
unsafe extern "C" {
    fn XOpenDisplay(name: *const c_char) -> *mut Display;
    fn XCloseDisplay(display: *mut Display) -> c_int;
    fn XDefaultScreen(display: *mut Display) -> c_int;
    fn XDefaultDepth(display: *mut Display, screen: c_int) -> c_int;
    fn XDefaultVisual(display: *mut Display, screen: c_int) -> *mut c_void;
    fn XDefaultColormap(display: *mut Display, screen: c_int) -> c_ulong;
    fn XRootWindow(display: *mut Display, screen: c_int) -> Xid;
    fn XCreateSimpleWindow(
        display: *mut Display,
        parent: Xid,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
        border_width: c_uint,
        border: c_ulong,
        background: c_ulong,
    ) -> Xid;
    fn XDestroyWindow(display: *mut Display, window: Xid) -> c_int;
    fn XSelectInput(display: *mut Display, window: Xid, mask: c_long) -> c_int;
    fn XMapWindow(display: *mut Display, window: Xid) -> c_int;
    fn XStoreName(display: *mut Display, window: Xid, title: *const c_char) -> c_int;
    fn XSetClassHint(display: *mut Display, window: Xid, hint: *mut ClassHint) -> c_int;
    fn XInternAtom(display: *mut Display, name: *const c_char, only_exists: c_int) -> Atom;
    fn XSetWMProtocols(
        display: *mut Display,
        window: Xid,
        protocols: *mut Atom,
        count: c_int,
    ) -> c_int;
    pub fn XPending(display: *mut Display) -> c_int;
    pub fn XNextEvent(display: *mut Display, event: *mut Event) -> c_int;
    pub fn XFilterEvent(event: *mut Event, window: Xid) -> c_int;
    fn XFlush(display: *mut Display) -> c_int;
    fn XConnectionNumber(display: *mut Display) -> c_int;
    fn XCreateGC(
        display: *mut Display,
        drawable: Xid,
        mask: c_ulong,
        values: *mut c_void,
    ) -> *mut c_void;
    fn XFreeGC(display: *mut Display, gc: *mut c_void) -> c_int;
    fn XCreatePixmap(
        display: *mut Display,
        drawable: Xid,
        width: c_uint,
        height: c_uint,
        depth: c_uint,
    ) -> Xid;
    fn XFreePixmap(display: *mut Display, pixmap: Xid) -> c_int;
    fn XCopyArea(
        display: *mut Display,
        src: Xid,
        dst: Xid,
        gc: *mut c_void,
        sx: c_int,
        sy: c_int,
        width: c_uint,
        height: c_uint,
        dx: c_int,
        dy: c_int,
    ) -> c_int;
    pub fn XLookupKeysym(event: *mut KeyEvent, index: c_int) -> c_ulong;
    fn XkbLookupKeySym(
        display: *mut Display,
        keycode: u8,
        modifiers: c_uint,
        consumed: *mut c_uint,
        symbol: *mut c_ulong,
    ) -> c_int;
    fn XLookupString(
        event: *mut KeyEvent,
        data: *mut c_char,
        capacity: c_int,
        symbol: *mut c_ulong,
        compose: *mut c_void,
    ) -> c_int;
    fn XOpenIM(
        display: *mut Display,
        database: *mut c_void,
        name: *mut c_char,
        class: *mut c_char,
    ) -> *mut c_void;
    fn XCloseIM(im: *mut c_void) -> c_int;
    fn XCreateIC(im: *mut c_void, ...) -> *mut c_void;
    fn XDestroyIC(ic: *mut c_void);
    fn XSetICFocus(ic: *mut c_void);
    fn XUnsetICFocus(ic: *mut c_void);
    fn Xutf8LookupString(
        ic: *mut c_void,
        event: *mut KeyEvent,
        data: *mut c_char,
        capacity: c_int,
        symbol: *mut c_ulong,
        status: *mut c_int,
    ) -> c_int;
    fn XSetLocaleModifiers(modifiers: *const c_char) -> *mut c_char;
    fn XkbSetDetectableAutoRepeat(
        display: *mut Display,
        detectable: c_int,
        supported: *mut c_int,
    ) -> c_int;
    fn XSetSelectionOwner(
        display: *mut Display,
        selection: Atom,
        owner: Xid,
        time: c_ulong,
    ) -> c_int;
    fn XConvertSelection(
        display: *mut Display,
        selection: Atom,
        target: Atom,
        property: Atom,
        requestor: Xid,
        time: c_ulong,
    ) -> c_int;
    fn XChangeProperty(
        display: *mut Display,
        window: Xid,
        property: Atom,
        kind: Atom,
        format: c_int,
        mode: c_int,
        data: *const u8,
        count: c_int,
    ) -> c_int;
    fn XDeleteProperty(display: *mut Display, window: Xid, property: Atom) -> c_int;
    fn XGetWindowProperty(
        display: *mut Display,
        window: Xid,
        property: Atom,
        offset: c_long,
        length: c_long,
        delete: c_int,
        request_type: Atom,
        actual_type: *mut Atom,
        format: *mut c_int,
        count: *mut c_ulong,
        remaining: *mut c_ulong,
        data: *mut *mut u8,
    ) -> c_int;
    fn XSendEvent(
        display: *mut Display,
        window: Xid,
        propagate: c_int,
        mask: c_long,
        event: *mut Event,
    ) -> c_int;
    fn XFree(data: *mut c_void) -> c_int;
}
#[link(name = "Xft")]
unsafe extern "C" {
    fn XftFontOpenName(display: *mut Display, screen: c_int, name: *const c_char) -> *mut Font;
    fn XftFontClose(display: *mut Display, font: *mut Font);
    fn XftCharExists(display: *mut Display, font: *mut Font, character: c_uint) -> c_int;
    fn XftTextExtentsUtf8(
        display: *mut Display,
        font: *mut Font,
        text: *const u8,
        length: c_int,
        extents: *mut GlyphInfo,
    );
    fn XftDrawCreate(
        display: *mut Display,
        drawable: Xid,
        visual: *mut c_void,
        colormap: c_ulong,
    ) -> *mut XftDraw;
    fn XftDrawDestroy(draw: *mut XftDraw);
    fn XftDrawChange(draw: *mut XftDraw, drawable: Xid);
    fn XftDrawStringUtf8(
        draw: *mut XftDraw,
        color: *const Color,
        font: *mut Font,
        x: c_int,
        y: c_int,
        text: *const u8,
        length: c_int,
    );
    fn XftDrawRect(
        draw: *mut XftDraw,
        color: *const Color,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
    );
    fn XftColorAllocValue(
        display: *mut Display,
        visual: *mut c_void,
        colormap: c_ulong,
        value: *const RenderColor,
        color: *mut Color,
    ) -> c_int;
    fn XftColorFree(
        display: *mut Display,
        visual: *mut c_void,
        colormap: c_ulong,
        color: *mut Color,
    );
}

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
        let cols = terminal.cols();
        let a = self.start.1 * cols + self.start.0;
        let b = self.end.1 * cols + self.end.0;
        let mut text = String::new();
        let mut line = String::new();
        for at in a.min(b)..=a.max(b) {
            let cell = terminal.view_cell(at % cols, at / cols, offset);
            if cell.width != 0 {
                line.push(if cell.ch == '\0' { ' ' } else { cell.ch });
                for c in cell.combining {
                    if c != '\0' {
                        line.push(c);
                    }
                }
            }
            if at % cols == cols - 1 || at == a.max(b) {
                text.push_str(line.trim_end_matches(' '));
                if at != a.max(b) {
                    text.push('\n');
                }
                line.clear();
            }
            if text.len() + line.len() > 4 * 1024 * 1024 {
                break;
            }
        }
        text
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
pub struct Window {
    pub display: *mut Display,
    pub id: Xid,
    pub fd: i32,
    pub cw: i32,
    pub ch: i32,
    pub width: i32,
    pub height: i32,
    pub wm_delete: Atom,
    screen: i32,
    depth: u32,
    visual: *mut c_void,
    colormap: c_ulong,
    pixmap: Xid,
    gc: *mut c_void,
    draw: *mut XftDraw,
    font: *mut Font,
    font_name: String,
    fallbacks: Vec<*mut Font>,
    colors: Vec<ColorEntry>,
    next_color: usize,
    im: *mut c_void,
    ic: *mut c_void,
    clipboard: Atom,
    utf8: Atom,
    targets: Atom,
    transfer: Atom,
    incr: Atom,
    title_atom: Atom,
    clipboard_text: String,
    primary_text: String,
    incoming: Vec<u8>,
    receiving: bool,
    paste_selection: Atom,
    last_cursor: (usize, usize),
    pub focused: bool,
}

impl Window {
    pub fn new(cols: usize, rows: usize, font_name: &str) -> Result<Self, &'static str> {
        let name = CString::new(font_name).map_err(|_| "invalid font name")?;
        unsafe {
            crate::os::setlocale(0, c"".as_ptr());
            XSetLocaleModifiers(c"".as_ptr());
            let display = XOpenDisplay(core::ptr::null());
            if display.is_null() {
                return Err("cannot open DISPLAY; ztty needs X11 or XWayland");
            }
            let screen = XDefaultScreen(display);
            let visual = XDefaultVisual(display, screen);
            let colormap = XDefaultColormap(display, screen);
            let depth = XDefaultDepth(display, screen) as u32;
            let font = XftFontOpenName(display, screen, name.as_ptr());
            if font.is_null() {
                XCloseDisplay(display);
                return Err("cannot load the requested font");
            }
            let mut ext: GlyphInfo = core::mem::zeroed();
            XftTextExtentsUtf8(display, font, b"M".as_ptr(), 1, &mut ext);
            let cw = i32::from(ext.x_off).max(1);
            let ch = ((*font).ascent + (*font).descent + 3).max(1);
            let width = cols as i32 * cw + PAD * 2;
            let height = rows as i32 * ch + PAD * 2;
            let root = XRootWindow(display, screen);
            let id = XCreateSimpleWindow(
                display,
                root,
                0,
                0,
                width as u32,
                height as u32,
                0,
                0,
                DEFAULT_BG as c_ulong,
            );
            XSelectInput(
                display,
                id,
                1 | 2 | 4 | 8 | 64 | 32768 | 131072 | 2097152 | 4194304,
            );
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
            let pixmap = XCreatePixmap(display, id, width as u32, height as u32, depth);
            let gc = XCreateGC(display, id, 0, core::ptr::null_mut());
            let draw = XftDrawCreate(display, pixmap, visual, colormap);
            let im = XOpenIM(
                display,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            );
            let ic = if im.is_null() {
                core::ptr::null_mut()
            } else {
                XCreateIC(
                    im,
                    c"inputStyle".as_ptr(),
                    0x408 as c_ulong,
                    c"clientWindow".as_ptr(),
                    id,
                    c"focusWindow".as_ptr(),
                    id,
                    core::ptr::null::<c_char>(),
                )
            };
            let mut supported = 0;
            XkbSetDetectableAutoRepeat(display, 1, &mut supported);
            let fd = XConnectionNumber(display);
            crate::os::fcntl(fd, 2, 1);
            let mut out = Self {
                display,
                id,
                fd,
                cw,
                ch,
                width,
                height,
                wm_delete,
                screen,
                depth,
                visual,
                colormap,
                pixmap,
                gc,
                draw,
                font,
                font_name: String::from(font_name),
                fallbacks: Vec::new(),
                colors: Vec::with_capacity(256),
                next_color: 0,
                im,
                ic,
                clipboard: XInternAtom(display, c"CLIPBOARD".as_ptr(), 0),
                utf8: XInternAtom(display, c"UTF8_STRING".as_ptr(), 0),
                targets: XInternAtom(display, c"TARGETS".as_ptr(), 0),
                transfer: XInternAtom(display, c"ZTTY_SELECTION".as_ptr(), 0),
                incr: XInternAtom(display, c"INCR".as_ptr(), 0),
                title_atom: XInternAtom(display, c"_NET_WM_NAME".as_ptr(), 0),
                clipboard_text: String::new(),
                primary_text: String::new(),
                incoming: Vec::new(),
                receiving: false,
                paste_selection: 0,
                last_cursor: (usize::MAX, usize::MAX),
                focused: true,
            };
            out.title("ztty");
            let background = out.color(DEFAULT_BG);
            XftDrawRect(draw, &background, 0, 0, width as u32, height as u32);
            XMapWindow(display, id);
            XFlush(display);
            Ok(out)
        }
    }
    pub fn title(&mut self, text: &str) {
        let text = if text.is_empty() { "ztty" } else { text };
        let Ok(title) = CString::new(text) else {
            return;
        };
        unsafe {
            XStoreName(self.display, self.id, title.as_ptr());
            XChangeProperty(
                self.display,
                self.id,
                self.title_atom,
                self.utf8,
                8,
                0,
                text.as_ptr(),
                text.len() as i32,
            );
        }
    }
    pub fn focus(&mut self, focused: bool) {
        self.focused = focused;
        if !self.ic.is_null() {
            unsafe {
                if focused {
                    XSetICFocus(self.ic);
                } else {
                    XUnsetICFocus(self.ic);
                }
            }
        }
    }
    pub fn coordinates(&self, x: i32, y: i32, cols: usize, rows: usize) -> (usize, usize) {
        (
            (((x - PAD).max(0) / self.cw) as usize).min(cols.saturating_sub(1)),
            (((y - PAD).max(0) / self.ch) as usize).min(rows.saturating_sub(1)),
        )
    }
    pub fn present(&self) {
        unsafe {
            XCopyArea(
                self.display,
                self.pixmap,
                self.id,
                self.gc,
                0,
                0,
                self.width as u32,
                self.height as u32,
                0,
                0,
            );
            XFlush(self.display);
        }
    }
    pub fn resize(&mut self, width: i32, height: i32) -> (usize, usize) {
        if self.width != width.max(1) || self.height != height.max(1) {
            self.width = width.max(1);
            self.height = height.max(1);
            unsafe {
                let pixmap = XCreatePixmap(
                    self.display,
                    self.id,
                    self.width as u32,
                    self.height as u32,
                    self.depth,
                );
                XftDrawChange(self.draw, pixmap);
                XFreePixmap(self.display, self.pixmap);
                self.pixmap = pixmap;
            }
        }
        (
            (((self.width - PAD * 2) / self.cw).max(1) as usize).min(512),
            (((self.height - PAD * 2) / self.ch).max(1) as usize).min(256),
        )
    }
    /// Resolve the current keyboard group, preserving AltGr/layout selection.
    pub fn key_symbol(&self, event: &mut KeyEvent, shifted: bool) -> c_ulong {
        let mut symbol = 0;
        let mut consumed = 0;
        let state = (event.state & !3) | u32::from(shifted);
        if unsafe {
            XkbLookupKeySym(
                self.display,
                event.keycode as u8,
                state,
                &mut consumed,
                &mut symbol,
            )
        } != 0
        {
            symbol
        } else {
            unsafe { XLookupKeysym(event, i32::from(shifted)) }
        }
    }
    pub fn lookup(&self, event: &mut KeyEvent, buffer: &mut Vec<u8>) -> Lookup {
        let mut symbol = 0;
        let mut status = 4;
        let mut count = unsafe {
            if self.ic.is_null() {
                XLookupString(
                    event,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as i32,
                    &mut symbol,
                    core::ptr::null_mut(),
                )
            } else {
                Xutf8LookupString(
                    self.ic,
                    event,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as i32,
                    &mut symbol,
                    &mut status,
                )
            }
        };
        if status == -1 && count > 0 && count <= 65536 {
            buffer.resize(count as usize + 1, 0);
            count = unsafe {
                Xutf8LookupString(
                    self.ic,
                    event,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as i32,
                    &mut symbol,
                    &mut status,
                )
            };
        }
        Lookup {
            len: count.clamp(0, buffer.len() as i32) as usize,
            symbol,
            text_only: status == 2,
            ignored: status == -1 || status == 1,
        }
    }
    fn color(&mut self, rgb: u32) -> Color {
        if let Some(c) = self.colors.iter().find(|c| c.rgb == rgb) {
            return c.color;
        }
        let value = RenderColor {
            red: (((rgb >> 16) & 255) * 257) as u16,
            green: (((rgb >> 8) & 255) * 257) as u16,
            blue: ((rgb & 255) * 257) as u16,
            alpha: 65535,
        };
        let mut color = Color {
            pixel: rgb as c_ulong,
            value,
        };
        unsafe {
            XftColorAllocValue(self.display, self.visual, self.colormap, &value, &mut color);
        }
        if self.colors.len() < 256 {
            self.colors.push(ColorEntry { rgb, color });
        } else {
            let entry = &mut self.colors[self.next_color];
            unsafe {
                XftColorFree(self.display, self.visual, self.colormap, &mut entry.color);
            }
            *entry = ColorEntry { rgb, color };
            self.next_color = (self.next_color + 1) % 256;
        }
        color
    }
    fn glyph_font(&mut self, ch: char) -> *mut Font {
        unsafe {
            if ch.is_ascii() || XftCharExists(self.display, self.font, ch as u32) != 0 {
                return self.font;
            }
            for &font in &self.fallbacks {
                if XftCharExists(self.display, font, ch as u32) != 0 {
                    return font;
                }
            }
            if self.fallbacks.len() < 16
                && let Ok(name) =
                    CString::new(format!("{}:charset={:x}", self.font_name, ch as u32))
            {
                let font = XftFontOpenName(self.display, self.screen, name.as_ptr());
                if !font.is_null() {
                    if XftCharExists(self.display, font, ch as u32) != 0 {
                        self.fallbacks.push(font);
                        return font;
                    }
                    XftFontClose(self.display, font);
                }
            }
        }
        self.font
    }
    fn cell_colors(cell: &Cell, selected: bool) -> (u32, u32) {
        let (mut fg, mut bg) = if cell.flags & INVERSE != 0 {
            (cell.bg, cell.fg)
        } else {
            (cell.fg, cell.bg)
        };
        if cell.flags & DIM != 0 {
            fg = ((fg & 0xfefefe) >> 1) + ((bg & 0xfefefe) >> 1);
        }
        if selected {
            bg = 0x24405b;
            fg = 0xf2f7ff;
        }
        if cell.flags & HIDDEN != 0 {
            fg = bg;
        }
        (fg, bg)
    }
    pub fn render(
        &mut self,
        terminal: &mut Terminal,
        offset: usize,
        selection: Option<Selection>,
        force: bool,
    ) {
        let cols = terminal.cols();
        let rows = terminal.rows();
        let cursor = terminal.cursor();
        let cursor_changed = cursor != self.last_cursor;
        if force {
            let bg = self.color(DEFAULT_BG);
            unsafe {
                XftDrawRect(self.draw, &bg, 0, 0, self.width as u32, self.height as u32);
            }
        }
        let mut changed = force;
        for row in 0..rows {
            if !(force
                || terminal.row_dirty(row)
                || (cursor_changed && (row == cursor.1 || row == self.last_cursor.1)))
            {
                continue;
            }
            changed = true;
            let y = PAD + row as i32 * self.ch;
            let mut col = 0;
            while col < cols {
                let cell = terminal.view_cell(col, row, offset);
                let selected = selection.is_some_and(|s| s.contains(col, row, cols));
                let (_, rgb) = Self::cell_colors(cell, selected);
                let mut end = col + 1;
                while end < cols {
                    let next = terminal.view_cell(end, row, offset);
                    if Self::cell_colors(
                        next,
                        selection.is_some_and(|s| s.contains(end, row, cols)),
                    )
                    .1 != rgb
                    {
                        break;
                    }
                    end += 1;
                }
                let color = self.color(rgb);
                unsafe {
                    XftDrawRect(
                        self.draw,
                        &color,
                        PAD + col as i32 * self.cw,
                        y,
                        ((end - col) as i32 * self.cw) as u32,
                        self.ch as u32,
                    );
                }
                col = end;
            }
            col = 0;
            while col < cols {
                let cell = terminal.view_cell(col, row, offset);
                if cell.width == 0 {
                    col += 1;
                    continue;
                }
                let (fg, _) =
                    Self::cell_colors(cell, selection.is_some_and(|s| s.contains(col, row, cols)));
                let color = self.color(fg);
                let x = PAD + col as i32 * self.cw;
                let baseline = y + unsafe { (*self.font).ascent } + 1;
                let mut text = [0u8; 2048];
                let mut len = 0;
                let mut cells = 1;
                let mut chars = [cell.ch, cell.combining[0], cell.combining[1]];
                if chars[0] == '\0' {
                    chars[0] = ' ';
                }
                for ch in chars {
                    if ch != '\0' {
                        len += ch.encode_utf8(&mut text[len..]).len();
                    }
                }
                let font = self.glyph_font(cell.ch);
                // Adjacent simple ASCII cells share a single Xft draw call.
                if cell.ch.is_ascii() && cell.width == 1 && cell.combining == ['\0'; 2] {
                    while col + cells < cols && len < text.len() - 4 {
                        let next = terminal.view_cell(col + cells, row, offset);
                        if !next.ch.is_ascii()
                            || next.width != 1
                            || next.combining != ['\0'; 2]
                            || next.flags != cell.flags
                            || next.link_id != cell.link_id
                            || Self::cell_colors(
                                next,
                                selection.is_some_and(|s| s.contains(col + cells, row, cols)),
                            )
                            .0 != fg
                        {
                            break;
                        }
                        text[len] = if next.ch == '\0' { b' ' } else { next.ch as u8 };
                        len += 1;
                        cells += 1;
                    }
                }
                unsafe {
                    XftDrawStringUtf8(
                        self.draw,
                        &color,
                        font,
                        x,
                        baseline,
                        text.as_ptr(),
                        len as i32,
                    );
                    if cell.flags & BOLD != 0 {
                        XftDrawStringUtf8(
                            self.draw,
                            &color,
                            font,
                            x + 1,
                            baseline,
                            text.as_ptr(),
                            len as i32,
                        );
                    }
                    let width = (cells.max(cell.width as usize) as i32 * self.cw) as u32;
                    if cell.flags & UNDERLINE != 0 || cell.link_id != 0 {
                        XftDrawRect(self.draw, &color, x, baseline + 2, width, 1);
                    }
                    if cell.flags & STRIKE != 0 {
                        XftDrawRect(self.draw, &color, x, y + self.ch / 2, width, 1);
                    }
                }
                col += cells;
            }
            if offset == 0 && row == cursor.1 && terminal.modes().cursor_visible {
                let color = self.color(if self.focused { 0x8ab4f8 } else { 0x58687b });
                unsafe {
                    XftDrawRect(
                        self.draw,
                        &color,
                        PAD + cursor.0.min(cols - 1) as i32 * self.cw,
                        y + self.ch - 2,
                        self.cw as u32,
                        2,
                    );
                }
            }
            // Publish complete rows from the retained pixmap; an expose can
            // redraw without reparsing or moving terminal screen contents.
            unsafe {
                XCopyArea(
                    self.display,
                    self.pixmap,
                    self.id,
                    self.gc,
                    0,
                    y,
                    self.width as u32,
                    self.ch as u32,
                    0,
                    y,
                );
            }
        }
        if force {
            unsafe {
                XCopyArea(
                    self.display,
                    self.pixmap,
                    self.id,
                    self.gc,
                    0,
                    0,
                    self.width as u32,
                    self.height as u32,
                    0,
                    0,
                );
            }
        }
        if changed {
            unsafe {
                XFlush(self.display);
            }
        }
        self.last_cursor = cursor;
        terminal.clear_dirty();
    }
    pub fn copy(&mut self, text: String, primary: bool) {
        let selection = if primary {
            self.primary_text = text;
            1
        } else {
            self.clipboard_text = text;
            self.clipboard
        };
        unsafe {
            XSetSelectionOwner(self.display, selection, self.id, 0);
            XFlush(self.display);
        }
    }
    pub fn paste(&mut self, primary: bool) {
        self.receiving = false;
        self.incoming.clear();
        self.paste_selection = if primary { 1 } else { self.clipboard };
        unsafe {
            XConvertSelection(
                self.display,
                self.paste_selection,
                self.utf8,
                self.transfer,
                self.id,
                0,
            );
            XFlush(self.display);
        }
    }
    pub fn selection_request(&self, request: SelectionRequest) {
        let mut notify = SelectionEvent {
            kind: SELECTION_NOTIFY,
            serial: 0,
            send_event: 1,
            display: self.display,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: 0,
            time: request.time,
        };
        let property = if request.property == 0 {
            request.target
        } else {
            request.property
        };
        unsafe {
            if request.target == self.targets {
                let targets = [self.targets, self.utf8, 31];
                XChangeProperty(
                    self.display,
                    request.requestor,
                    property,
                    4,
                    32,
                    0,
                    targets.as_ptr().cast(),
                    targets.len() as i32,
                );
                notify.property = property;
            } else if request.target == self.utf8 || request.target == 31 {
                let text = if request.selection == 1 {
                    &self.primary_text
                } else {
                    &self.clipboard_text
                };
                let latin: Vec<u8>;
                let data = if request.target == 31 {
                    latin = text
                        .chars()
                        .map(|c| if (c as u32) <= 255 { c as u8 } else { b'?' })
                        .collect();
                    &latin[..]
                } else {
                    text.as_bytes()
                };
                XChangeProperty(
                    self.display,
                    request.requestor,
                    property,
                    request.target,
                    8,
                    0,
                    data.as_ptr(),
                    0,
                );
                for chunk in data.chunks(32768) {
                    XChangeProperty(
                        self.display,
                        request.requestor,
                        property,
                        request.target,
                        8,
                        2,
                        chunk.as_ptr(),
                        chunk.len() as i32,
                    );
                }
                notify.property = property;
            }
            let mut event = Event { selection: notify };
            XSendEvent(self.display, request.requestor, 0, 0, &mut event);
            XFlush(self.display);
        }
    }
    fn read_property(&self) -> (Atom, Vec<u8>) {
        let mut kind = 0;
        let mut format = 0;
        let mut count = 0;
        let mut remaining = 0;
        let mut data = core::ptr::null_mut();
        let mut bytes = Vec::new();
        unsafe {
            let code = XGetWindowProperty(
                self.display,
                self.id,
                self.transfer,
                0,
                1024 * 1024,
                1,
                0,
                &mut kind,
                &mut format,
                &mut count,
                &mut remaining,
                &mut data,
            );
            if code == 0
                && !data.is_null()
                && format == 8
                && remaining == 0
                && count <= 4 * 1024 * 1024
            {
                bytes.extend_from_slice(core::slice::from_raw_parts(data, count as usize));
            }
            if !data.is_null() {
                XFree(data.cast());
            }
            if remaining != 0 {
                XDeleteProperty(self.display, self.id, self.transfer);
            }
        }
        (kind, bytes)
    }
    pub fn selection_notify(&mut self, event: SelectionEvent) -> Option<Vec<u8>> {
        if self.paste_selection == 0
            || event.requestor != self.id
            || event.selection != self.paste_selection
        {
            return None;
        }
        if event.property == 0 {
            if event.target == self.utf8 {
                unsafe {
                    XConvertSelection(
                        self.display,
                        self.paste_selection,
                        31,
                        self.transfer,
                        self.id,
                        0,
                    );
                    XFlush(self.display);
                }
            }
            return None;
        }
        if event.property != self.transfer {
            return None;
        }
        let (kind, data) = self.read_property();
        if kind == self.incr {
            self.receiving = true;
            self.incoming.clear();
            unsafe {
                XDeleteProperty(self.display, self.id, self.transfer);
                XFlush(self.display);
            }
            None
        } else if kind == 31 {
            self.paste_selection = 0;
            let text: String = data.into_iter().map(char::from).collect();
            Some(text.into_bytes())
        } else {
            self.paste_selection = 0;
            Some(data)
        }
    }
    pub fn property(&mut self, event: PropertyEvent) -> Option<Vec<u8>> {
        if !self.receiving
            || event.window != self.id
            || event.atom != self.transfer
            || event.state != 0
        {
            return None;
        }
        let (_, data) = self.read_property();
        if data.is_empty() {
            self.receiving = false;
            self.paste_selection = 0;
            return Some(core::mem::take(&mut self.incoming));
        }
        if self.incoming.len().saturating_add(data.len()) > 4 * 1024 * 1024 {
            self.receiving = false;
            self.paste_selection = 0;
            self.incoming.clear();
            return None;
        }
        self.incoming.extend_from_slice(&data);
        unsafe {
            XFlush(self.display);
        }
        None
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            if !self.ic.is_null() {
                XDestroyIC(self.ic);
            }
            if !self.im.is_null() {
                XCloseIM(self.im);
            }
            XftDrawDestroy(self.draw);
            for &font in &self.fallbacks {
                XftFontClose(self.display, font);
            }
            XftFontClose(self.display, self.font);
            for color in &mut self.colors {
                XftColorFree(self.display, self.visual, self.colormap, &mut color.color);
            }
            XFreeGC(self.display, self.gc);
            XFreePixmap(self.display, self.pixmap);
            if self.id != 0 {
                XDestroyWindow(self.display, self.id);
            }
            XCloseDisplay(self.display);
        }
    }
}
