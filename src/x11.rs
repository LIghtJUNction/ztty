//! X11/XWayland window and Xft text rendering, accessed through a small C ABI.
use alloc::ffi::CString;
use alloc::format;
use alloc::rc::{Rc, Weak};
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use zterm::{
    BOLD, Cell, CursorShape, DIM, EXPLICIT_BG, HIDDEN, INVERSE, STRIKE, Terminal, UNDERLINE,
};

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
use crate::appearance::Appearance;

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
impl Event {
    /// Destination used after XFilterEvent has consumed input-method traffic.
    pub fn window(&self) -> Xid {
        // Configure/DestroyNotify place the subject after the event destination.
        unsafe {
            match self.kind {
                CONFIGURE_NOTIFY | DESTROY_NOTIFY => self.configure.window,
                SELECTION_REQUEST => self.request.owner,
                SELECTION_NOTIFY => self.selection.requestor,
                _ => self.key.window,
            }
        }
    }
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

#[repr(C)]
struct VisualInfo {
    visual: *mut c_void,
    id: Xid,
    screen: c_int,
    depth: c_int,
    class: c_int,
    red_mask: c_ulong,
    green_mask: c_ulong,
    blue_mask: c_ulong,
    colormap_size: c_int,
    bits_per_rgb: c_int,
}

#[repr(C)]
struct WindowAttributes {
    background_pixmap: Xid,
    background_pixel: c_ulong,
    border_pixmap: Xid,
    border_pixel: c_ulong,
    bit_gravity: c_int,
    win_gravity: c_int,
    backing_store: c_int,
    backing_planes: c_ulong,
    backing_pixel: c_ulong,
    save_under: c_int,
    event_mask: c_long,
    do_not_propagate_mask: c_long,
    override_redirect: c_int,
    colormap: Xid,
    cursor: Xid,
}

#[repr(C)]
struct PictureFormat {
    id: Xid,
    kind: c_int,
    depth: c_int,
    // Shift and mask pairs in R, G, B, A order.
    direct: [i16; 8],
    colormap: Xid,
}

#[link(name = "Xrender")]
unsafe extern "C" {
    fn XRenderFindVisualFormat(display: *mut Display, visual: *mut c_void) -> *mut PictureFormat;
    fn XRenderFillRectangle(
        display: *mut Display,
        op: c_int,
        dst: Xid,
        color: *const RenderColor,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
    );
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
    fn XGetVisualInfo(
        display: *mut Display,
        mask: c_long,
        filter: *mut VisualInfo,
        count: *mut c_int,
    ) -> *mut VisualInfo;
    fn XCreateColormap(
        display: *mut Display,
        window: Xid,
        visual: *mut c_void,
        alloc: c_int,
    ) -> Xid;
    fn XCreateWindow(
        display: *mut Display,
        parent: Xid,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
        border_width: c_uint,
        depth: c_int,
        class: c_uint,
        visual: *mut c_void,
        mask: c_ulong,
        attrs: *mut WindowAttributes,
    ) -> Xid;
    fn XDestroyWindow(display: *mut Display, window: Xid) -> c_int;
    fn XSelectInput(display: *mut Display, window: Xid, mask: c_long) -> c_int;
    fn XMapWindow(display: *mut Display, window: Xid) -> c_int;
    fn XMapRaised(display: *mut Display, window: Xid) -> c_int;
    fn XResizeWindow(display: *mut Display, window: Xid, width: c_uint, height: c_uint) -> c_int;
    fn XSetInputFocus(display: *mut Display, window: Xid, revert_to: c_int, time: c_ulong)
    -> c_int;
    fn XFreeColormap(display: *mut Display, colormap: Xid) -> c_int;
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
    fn XGetICValues(ic: *mut c_void, ...) -> *mut c_char;
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
    fn XftDrawPicture(draw: *mut XftDraw) -> Xid;
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

type DrawnCursor = ((usize, usize), CursorShape);

/// Shared X11 connection for a single-threaded host of terminal windows.
///
/// Font entries are weak: closing their last window releases the faces and
/// glyph cache even if headless sessions keep the host process alive.
pub struct Connection {
    pub display: *mut Display,
    pub fd: i32,
    screen: i32,
    im: *mut c_void,
    fonts: RefCell<Vec<Weak<Fonts>>>,
}

impl Connection {
    pub fn new() -> Result<Rc<Self>, &'static str> {
        Self::new_for_display(None)
    }

    pub fn new_for_display(name: Option<&CStr>) -> Result<Rc<Self>, &'static str> {
        unsafe {
            crate::os::setlocale(0, c"".as_ptr());
            XSetLocaleModifiers(c"".as_ptr());
            let display = XOpenDisplay(name.map_or(core::ptr::null(), CStr::as_ptr));
            if display.is_null() {
                return Err("cannot open DISPLAY; ztty needs X11 or XWayland");
            }
            let screen = XDefaultScreen(display);
            let im = XOpenIM(
                display,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            );
            let mut supported = 0;
            XkbSetDetectableAutoRepeat(display, 1, &mut supported);
            let fd = XConnectionNumber(display);
            crate::os::fcntl(fd, 2, 1);
            Ok(Rc::new(Self {
                display,
                fd,
                screen,
                im,
                fonts: RefCell::new(Vec::new()),
            }))
        }
    }

    fn fonts(self: &Rc<Self>, name: &str) -> Result<Rc<Fonts>, &'static str> {
        let mut cache = self.fonts.borrow_mut();
        cache.retain(|font| font.strong_count() != 0);
        for font in cache.iter().filter_map(Weak::upgrade) {
            if font.name == name {
                return Ok(font);
            }
        }
        let c_name = CString::new(name).map_err(|_| "invalid font name")?;
        let font = unsafe { XftFontOpenName(self.display, self.screen, c_name.as_ptr()) };
        if font.is_null() {
            return Err("cannot load the requested font");
        }
        let fonts = Rc::new(Fonts {
            connection: Rc::clone(self),
            name: name.into(),
            font,
            fallbacks: RefCell::new(Vec::new()),
        });
        cache.push(Rc::downgrade(&fonts));
        Ok(fonts)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        unsafe {
            if !self.im.is_null() {
                XCloseIM(self.im);
            }
            XCloseDisplay(self.display);
        }
    }
}

struct Fonts {
    connection: Rc<Connection>,
    name: String,
    font: *mut Font,
    fallbacks: RefCell<Vec<*mut Font>>,
}

impl Fonts {
    fn glyph(&self, ch: char) -> *mut Font {
        let display = self.connection.display;
        unsafe {
            if ch.is_ascii() || XftCharExists(display, self.font, ch as u32) != 0 {
                return self.font;
            }
            let mut fallbacks = self.fallbacks.borrow_mut();
            for &font in fallbacks.iter() {
                if XftCharExists(display, font, ch as u32) != 0 {
                    return font;
                }
            }
            if fallbacks.len() < 16
                && let Ok(name) = CString::new(format!("{}:charset={:x}", self.name, ch as u32))
            {
                let font = XftFontOpenName(display, self.connection.screen, name.as_ptr());
                if !font.is_null() {
                    if XftCharExists(display, font, ch as u32) != 0 {
                        fallbacks.push(font);
                        return font;
                    }
                    XftFontClose(display, font);
                }
            }
        }
        self.font
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        unsafe {
            for &font in self.fallbacks.get_mut().iter() {
                XftFontClose(self.connection.display, font);
            }
            XftFontClose(self.connection.display, self.font);
        }
    }
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
    depth: u32,
    visual: *mut c_void,
    colormap: c_ulong,
    owns_colormap: bool,
    pixmap: Xid,
    gc: *mut c_void,
    draw: *mut XftDraw,
    font: *mut Font,
    fonts: Rc<Fonts>,
    colors: Vec<ColorEntry>,
    next_color: usize,
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
    last_cursor: Option<DrawnCursor>,
    padding: i32,
    background: u32,
    background_fill: Option<RenderColor>,
    cursor_color: u32,
    selection_colors: (u32, u32),
    pub focused: bool,
}

impl Window {
    pub fn new(cols: usize, rows: usize, appearance: &Appearance) -> Result<Self, &'static str> {
        Self::new_shared(cols, rows, appearance, &Connection::new()?)
    }

    pub fn new_shared(
        cols: usize,
        rows: usize,
        appearance: &Appearance,
        connection: &Rc<Connection>,
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
            let pixmap = XCreatePixmap(display, id, width as u32, height as u32, depth);
            let gc = XCreateGC(display, id, 0, core::ptr::null_mut());
            let draw = XftDrawCreate(display, pixmap, visual, colormap);
            let im = connection.im;
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
            if !ic.is_null() {
                let mut filter_events: c_long = 0;
                if XGetICValues(
                    ic,
                    c"filterEvents".as_ptr(),
                    &mut filter_events as *mut c_long,
                    core::ptr::null::<c_char>(),
                )
                .is_null()
                {
                    XSelectInput(display, id, events | filter_events);
                }
            }
            let fd = connection.fd;
            let mut out = Self {
                display,
                id,
                fd,
                cw,
                ch,
                width,
                height,
                wm_delete,
                depth,
                visual,
                colormap,
                owns_colormap: alpha.is_some(),
                pixmap,
                gc,
                draw,
                font,
                fonts,
                colors: Vec::with_capacity(256),
                next_color: 0,
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
                focused: true,
            };
            out.title("ztty");
            out.fill_background(background, true, 0, 0, width as u32, height as u32);
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
            (((x - self.padding).max(0) / self.cw) as usize).min(cols.saturating_sub(1)),
            (((y - self.padding).max(0) / self.ch) as usize).min(rows.saturating_sub(1)),
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
    /// Request a character-grid resize. ConfigureNotify applies the resulting
    /// pixel size to the backing pixmap and the host's terminal model.
    pub fn resize_grid(&self, cols: usize, rows: usize) {
        if self.id == 0 {
            return;
        }
        let width = cols
            .saturating_mul(self.cw as usize)
            .saturating_add((self.padding * 2) as usize)
            .clamp(1, u16::MAX as usize);
        let height = rows
            .saturating_mul(self.ch as usize)
            .saturating_add((self.padding * 2) as usize)
            .clamp(1, u16::MAX as usize);
        unsafe {
            XResizeWindow(self.display, self.id, width as c_uint, height as c_uint);
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
            (((self.width - self.padding * 2) / self.cw).max(1) as usize).min(512),
            (((self.height - self.padding * 2) / self.ch).max(1) as usize).min(256),
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
        self.fonts.glyph(ch)
    }

    /// Bring an already attached session back without opening a second window.
    pub fn raise(&self) {
        if self.id == 0 {
            return;
        }
        unsafe {
            XMapRaised(self.display, self.id);
            XSetInputFocus(self.display, self.id, 1, 0);
            XFlush(self.display);
        }
    }

    fn cell_colors(&self, cell: &Cell, selected: bool) -> (u32, u32, bool) {
        let (mut fg, mut bg) = if cell.flags & INVERSE != 0 {
            (cell.bg, cell.fg)
        } else {
            (cell.fg, cell.bg)
        };
        if cell.flags & DIM != 0 {
            fg = ((fg & 0xfefefe) >> 1) + ((bg & 0xfefefe) >> 1);
        }
        if selected {
            (fg, bg) = self.selection_colors;
        }
        if cell.flags & HIDDEN != 0 {
            fg = bg;
        }
        let translucent = !selected && cell.flags & (INVERSE | EXPLICIT_BG) == 0;
        (fg, bg, translucent)
    }
    fn fill_background(&mut self, rgb: u32, translucent: bool, x: i32, y: i32, w: u32, h: u32) {
        if let Some(color) = self.background_fill.as_ref().filter(|_| translucent) {
            // Replace the retained pixels, including alpha, on every damaged row.
            unsafe {
                XRenderFillRectangle(
                    self.display,
                    1,
                    XftDrawPicture(self.draw),
                    color,
                    x,
                    y,
                    w,
                    h,
                )
            };
        } else {
            let color = self.color(rgb);
            unsafe { XftDrawRect(self.draw, &color, x, y, w, h) };
        }
    }
    pub fn cursor_changed(&self, cursor: Option<DrawnCursor>) -> bool {
        cursor != self.last_cursor
    }
    pub fn render(
        &mut self,
        terminal: &mut Terminal,
        offset: usize,
        selection: Option<Selection>,
        force: bool,
        cursor: Option<DrawnCursor>,
    ) {
        let cols = terminal.cols();
        let rows = terminal.rows();
        let cursor_changed = cursor != self.last_cursor;
        if force {
            self.fill_background(
                self.background,
                true,
                0,
                0,
                self.width as u32,
                self.height as u32,
            );
        }
        let mut changed = force;
        for row in 0..rows {
            if !(force
                || terminal.row_dirty(row)
                || (cursor_changed
                    && [cursor, self.last_cursor]
                        .into_iter()
                        .flatten()
                        .any(|((_, y), _)| y == row)))
            {
                continue;
            }
            changed = true;
            let y = self.padding + row as i32 * self.ch;
            let mut col = 0;
            while col < cols {
                let cell = terminal.view_cell(col, row, offset);
                let selected = selection.is_some_and(|s| s.contains(col, row, cols));
                let (_, rgb, translucent) = self.cell_colors(cell, selected);
                let mut end = col + 1;
                while end < cols {
                    let next = terminal.view_cell(end, row, offset);
                    let (_, next_rgb, next_translucent) = self
                        .cell_colors(next, selection.is_some_and(|s| s.contains(end, row, cols)));
                    if (next_rgb, next_translucent) != (rgb, translucent) {
                        break;
                    }
                    end += 1;
                }
                self.fill_background(
                    rgb,
                    translucent,
                    self.padding + col as i32 * self.cw,
                    y,
                    ((end - col) as i32 * self.cw) as u32,
                    self.ch as u32,
                );
                col = end;
            }
            col = 0;
            while col < cols {
                let cell = terminal.view_cell(col, row, offset);
                if cell.width == 0 {
                    col += 1;
                    continue;
                }
                let (fg, _, _) =
                    self.cell_colors(cell, selection.is_some_and(|s| s.contains(col, row, cols)));
                let color = self.color(fg);
                let x = self.padding + col as i32 * self.cw;
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
                            || self
                                .cell_colors(
                                    next,
                                    selection.is_some_and(|s| s.contains(col + cells, row, cols)),
                                )
                                .0
                                != fg
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
            if let Some(((column, _), shape)) = cursor.filter(|((_, y), _)| *y == row) {
                self.draw_cursor(terminal, column.min(cols - 1), row, y, shape);
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
    fn draw_cursor(
        &mut self,
        terminal: &Terminal,
        column: usize,
        row: usize,
        y: i32,
        shape: CursorShape,
    ) {
        let cell = terminal.cell(column, row);
        let rgb = if self.focused {
            self.cursor_color
        } else {
            ((self.cursor_color & 0xfefefe) >> 1) + ((self.background & 0xfefefe) >> 1)
        };
        let color = self.color(rgb);
        let x = self.padding + column as i32 * self.cw;
        let (top, width, height) = match shape {
            CursorShape::Block => (y, self.cw * i32::from(cell.width.max(1)), self.ch),
            CursorShape::Underline => (y + self.ch - 2, self.cw, 2),
            CursorShape::Bar => (y, self.cw.min(2), self.ch),
        };
        unsafe { XftDrawRect(self.draw, &color, x, top, width as u32, height as u32) };
        if shape == CursorShape::Block && cell.width > 0 && cell.flags & HIDDEN == 0 {
            let contrast =
                if ((rgb >> 16) * 299 + ((rgb >> 8) & 255) * 587 + (rgb & 255) * 114) >= 128000 {
                    0x000000
                } else {
                    0xffffff
                };
            let fg = self.color(contrast);
            let font = self.glyph_font(cell.ch);
            let mut bytes = [0; 12];
            let mut len = 0;
            for ch in [cell.ch, cell.combining[0], cell.combining[1]] {
                if ch != '\0' {
                    len += ch.encode_utf8(&mut bytes[len..]).len();
                }
            }
            let baseline = y + unsafe { (*self.font).ascent } + 1;
            unsafe {
                XftDrawStringUtf8(
                    self.draw,
                    &fg,
                    font,
                    x,
                    baseline,
                    bytes.as_ptr(),
                    len as i32,
                )
            };
        }
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
            XftDrawDestroy(self.draw);
            for color in &mut self.colors {
                XftColorFree(self.display, self.visual, self.colormap, &mut color.color);
            }
            XFreeGC(self.display, self.gc);
            XFreePixmap(self.display, self.pixmap);
            if self.id != 0 {
                XDestroyWindow(self.display, self.id);
            }
            if self.owns_colormap {
                XFreeColormap(self.display, self.colormap);
            }
        }
    }
}
