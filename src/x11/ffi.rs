//! Native X11, XRender, and Xft ABI declarations.
use core::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void};

#[repr(C)]
pub struct Display {
    _private: [u8; 0],
}
#[repr(C)]
pub struct XftDraw {
    _private: [u8; 0],
}
#[repr(C)]
pub(super) struct ClipRectangle {
    pub(super) x: i16,
    pub(super) y: i16,
    pub(super) width: u16,
    pub(super) height: u16,
}
pub type Atom = c_ulong;
pub type Xid = c_ulong;

#[repr(C)]
pub(super) struct ErrorEvent {
    kind: c_int,
    display: *mut Display,
    resource: Xid,
    serial: c_ulong,
    pub(super) error_code: u8,
    pub(super) request_code: u8,
    minor_code: u8,
}
pub(super) type ErrorHandler = unsafe extern "C" fn(*mut Display, *mut ErrorEvent) -> c_int;

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
pub(super) struct GlyphInfo {
    width: u16,
    height: u16,
    x: i16,
    y: i16,
    pub(super) x_off: i16,
    y_off: i16,
}
#[repr(C)]
pub(super) struct ClassHint {
    pub(super) name: *mut c_char,
    pub(super) class: *mut c_char,
}

#[repr(C)]
pub(super) struct VisualInfo {
    pub(super) visual: *mut c_void,
    id: Xid,
    pub(super) screen: c_int,
    pub(super) depth: c_int,
    pub(super) class: c_int,
    red_mask: c_ulong,
    green_mask: c_ulong,
    blue_mask: c_ulong,
    colormap_size: c_int,
    bits_per_rgb: c_int,
}

#[repr(C)]
pub(super) struct WindowAttributes {
    background_pixmap: Xid,
    pub(super) background_pixel: c_ulong,
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
    pub(super) colormap: Xid,
    cursor: Xid,
}

#[repr(C)]
pub(super) struct PictureFormat {
    id: Xid,
    pub(super) kind: c_int,
    depth: c_int,
    // Shift and mask pairs in R, G, B, A order.
    pub(super) direct: [i16; 8],
    colormap: Xid,
}

#[repr(C)]
pub(super) struct Transform {
    pub(super) matrix: [[i32; 3]; 3],
}

#[repr(C)]
pub(super) struct XImage {
    _private: [u8; 0],
}

#[link(name = "c")]
unsafe extern "C" {
    pub(super) fn malloc(bytes: usize) -> *mut c_void;
    pub(super) fn free(pointer: *mut c_void);
}

#[link(name = "Xrender")]
unsafe extern "C" {
    pub(super) fn XRenderFindVisualFormat(
        display: *mut Display,
        visual: *mut c_void,
    ) -> *mut PictureFormat;
    pub(super) fn XRenderFindStandardFormat(
        display: *mut Display,
        format: c_int,
    ) -> *mut PictureFormat;
    pub(super) fn XRenderCreatePicture(
        display: *mut Display,
        drawable: Xid,
        format: *const PictureFormat,
        mask: c_ulong,
        attributes: *const c_void,
    ) -> Xid;
    pub(super) fn XRenderFreePicture(display: *mut Display, picture: Xid);
    pub(super) fn XRenderSetPictureTransform(
        display: *mut Display,
        picture: Xid,
        transform: *const Transform,
    );
    pub(super) fn XRenderSetPictureFilter(
        display: *mut Display,
        picture: Xid,
        filter: *const c_char,
        parameters: *const i32,
        count: c_int,
    );
    pub(super) fn XRenderComposite(
        display: *mut Display,
        op: c_int,
        source: Xid,
        mask: Xid,
        destination: Xid,
        source_x: c_int,
        source_y: c_int,
        mask_x: c_int,
        mask_y: c_int,
        destination_x: c_int,
        destination_y: c_int,
        width: c_uint,
        height: c_uint,
    );
    pub(super) fn XRenderFillRectangle(
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
    pub(super) fn XCreateImage(
        display: *mut Display,
        visual: *mut c_void,
        depth: c_uint,
        format: c_int,
        offset: c_int,
        data: *mut c_char,
        width: c_uint,
        height: c_uint,
        bitmap_pad: c_int,
        bytes_per_line: c_int,
    ) -> *mut XImage;
    pub(super) fn XDestroyImage(image: *mut XImage) -> c_int;
    pub(super) fn XPutImage(
        display: *mut Display,
        drawable: Xid,
        gc: *mut c_void,
        image: *mut XImage,
        source_x: c_int,
        source_y: c_int,
        destination_x: c_int,
        destination_y: c_int,
        width: c_uint,
        height: c_uint,
    ) -> c_int;
    pub(super) fn XOpenDisplay(name: *const c_char) -> *mut Display;
    pub(super) fn XCloseDisplay(display: *mut Display) -> c_int;
    pub(super) fn XSetErrorHandler(handler: Option<ErrorHandler>) -> Option<ErrorHandler>;
    pub(super) fn XDefaultScreen(display: *mut Display) -> c_int;
    pub(super) fn XDefaultDepth(display: *mut Display, screen: c_int) -> c_int;
    pub(super) fn XDefaultVisual(display: *mut Display, screen: c_int) -> *mut c_void;
    pub(super) fn XVisualIDFromVisual(visual: *mut c_void) -> c_ulong;
    pub(super) fn XDefaultColormap(display: *mut Display, screen: c_int) -> c_ulong;
    pub(super) fn XRootWindow(display: *mut Display, screen: c_int) -> Xid;
    pub(super) fn XGetVisualInfo(
        display: *mut Display,
        mask: c_long,
        filter: *mut VisualInfo,
        count: *mut c_int,
    ) -> *mut VisualInfo;
    pub(super) fn XCreateColormap(
        display: *mut Display,
        window: Xid,
        visual: *mut c_void,
        alloc: c_int,
    ) -> Xid;
    pub(super) fn XCreateWindow(
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
    pub(super) fn XDestroyWindow(display: *mut Display, window: Xid) -> c_int;
    pub(super) fn XSelectInput(display: *mut Display, window: Xid, mask: c_long) -> c_int;
    pub(super) fn XMapWindow(display: *mut Display, window: Xid) -> c_int;
    pub(super) fn XUnmapWindow(display: *mut Display, window: Xid) -> c_int;
    pub(super) fn XReparentWindow(
        display: *mut Display,
        window: Xid,
        parent: Xid,
        x: c_int,
        y: c_int,
    ) -> c_int;
    pub(super) fn XMoveResizeWindow(
        display: *mut Display,
        window: Xid,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
    ) -> c_int;
    pub(super) fn XMapRaised(display: *mut Display, window: Xid) -> c_int;
    pub(super) fn XLowerWindow(display: *mut Display, window: Xid) -> c_int;
    pub(super) fn XResizeWindow(
        display: *mut Display,
        window: Xid,
        width: c_uint,
        height: c_uint,
    ) -> c_int;
    pub(super) fn XSetInputFocus(
        display: *mut Display,
        window: Xid,
        revert_to: c_int,
        time: c_ulong,
    ) -> c_int;
    pub(super) fn XGetInputFocus(
        display: *mut Display,
        window: *mut Xid,
        revert_to: *mut c_int,
    ) -> c_int;
    pub(super) fn XNextRequest(display: *mut Display) -> c_ulong;
    pub(super) fn XFreeColormap(display: *mut Display, colormap: Xid) -> c_int;
    pub(super) fn XStoreName(display: *mut Display, window: Xid, title: *const c_char) -> c_int;
    pub(super) fn XSetClassHint(display: *mut Display, window: Xid, hint: *mut ClassHint) -> c_int;
    pub(super) fn XInternAtom(
        display: *mut Display,
        name: *const c_char,
        only_exists: c_int,
    ) -> Atom;
    pub(super) fn XSetWMProtocols(
        display: *mut Display,
        window: Xid,
        protocols: *mut Atom,
        count: c_int,
    ) -> c_int;
    pub fn XPending(display: *mut Display) -> c_int;
    pub fn XNextEvent(display: *mut Display, event: *mut Event) -> c_int;
    pub fn XFilterEvent(event: *mut Event, window: Xid) -> c_int;
    pub(super) fn XFlush(display: *mut Display) -> c_int;
    pub(super) fn XConnectionNumber(display: *mut Display) -> c_int;
    pub(super) fn XCreateGC(
        display: *mut Display,
        drawable: Xid,
        mask: c_ulong,
        values: *mut c_void,
    ) -> *mut c_void;
    pub(super) fn XFreeGC(display: *mut Display, gc: *mut c_void) -> c_int;
    pub(super) fn XCreatePixmap(
        display: *mut Display,
        drawable: Xid,
        width: c_uint,
        height: c_uint,
        depth: c_uint,
    ) -> Xid;
    pub(super) fn XFreePixmap(display: *mut Display, pixmap: Xid) -> c_int;
    pub(super) fn XCopyArea(
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
    pub(super) fn XkbLookupKeySym(
        display: *mut Display,
        keycode: u8,
        modifiers: c_uint,
        consumed: *mut c_uint,
        symbol: *mut c_ulong,
    ) -> c_int;
    pub(super) fn XLookupString(
        event: *mut KeyEvent,
        data: *mut c_char,
        capacity: c_int,
        symbol: *mut c_ulong,
        compose: *mut c_void,
    ) -> c_int;
    pub(super) fn XOpenIM(
        display: *mut Display,
        database: *mut c_void,
        name: *mut c_char,
        class: *mut c_char,
    ) -> *mut c_void;
    pub(super) fn XCloseIM(im: *mut c_void) -> c_int;
    pub(super) fn XSetIMValues(im: *mut c_void, ...) -> *mut c_char;
    pub(super) fn XRegisterIMInstantiateCallback(
        display: *mut Display,
        database: *mut c_void,
        name: *mut c_char,
        class: *mut c_char,
        callback: unsafe extern "C" fn(*mut Display, *mut c_void, *mut c_void),
        data: *mut c_void,
    ) -> c_int;
    pub(super) fn XUnregisterIMInstantiateCallback(
        display: *mut Display,
        database: *mut c_void,
        name: *mut c_char,
        class: *mut c_char,
        callback: unsafe extern "C" fn(*mut Display, *mut c_void, *mut c_void),
        data: *mut c_void,
    ) -> c_int;
    pub(super) fn XCreateIC(im: *mut c_void, ...) -> *mut c_void;
    pub(super) fn XGetICValues(ic: *mut c_void, ...) -> *mut c_char;
    pub(super) fn XDestroyIC(ic: *mut c_void);
    pub(super) fn XSetICFocus(ic: *mut c_void);
    pub(super) fn XUnsetICFocus(ic: *mut c_void);
    pub(super) fn Xutf8LookupString(
        ic: *mut c_void,
        event: *mut KeyEvent,
        data: *mut c_char,
        capacity: c_int,
        symbol: *mut c_ulong,
        status: *mut c_int,
    ) -> c_int;
    pub(super) fn XSetLocaleModifiers(modifiers: *const c_char) -> *mut c_char;
    pub(super) fn XkbSetDetectableAutoRepeat(
        display: *mut Display,
        detectable: c_int,
        supported: *mut c_int,
    ) -> c_int;
    pub(super) fn XGetSelectionOwner(display: *mut Display, selection: Atom) -> Xid;
    pub(super) fn XSetSelectionOwner(
        display: *mut Display,
        selection: Atom,
        owner: Xid,
        time: c_ulong,
    ) -> c_int;
    pub(super) fn XConvertSelection(
        display: *mut Display,
        selection: Atom,
        target: Atom,
        property: Atom,
        requestor: Xid,
        time: c_ulong,
    ) -> c_int;
    pub(super) fn XChangeProperty(
        display: *mut Display,
        window: Xid,
        property: Atom,
        kind: Atom,
        format: c_int,
        mode: c_int,
        data: *const u8,
        count: c_int,
    ) -> c_int;
    pub(super) fn XDeleteProperty(display: *mut Display, window: Xid, property: Atom) -> c_int;
    pub(super) fn XGetWindowProperty(
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
    pub(super) fn XSendEvent(
        display: *mut Display,
        window: Xid,
        propagate: c_int,
        mask: c_long,
        event: *mut Event,
    ) -> c_int;
    pub(super) fn XFree(data: *mut c_void) -> c_int;
}
#[link(name = "Xft")]
unsafe extern "C" {
    pub(super) fn XftFontOpenName(
        display: *mut Display,
        screen: c_int,
        name: *const c_char,
    ) -> *mut Font;
    pub(super) fn XftFontClose(display: *mut Display, font: *mut Font);
    pub(super) fn XftCharExists(display: *mut Display, font: *mut Font, character: c_uint)
    -> c_int;
    pub(super) fn XftTextExtentsUtf8(
        display: *mut Display,
        font: *mut Font,
        text: *const u8,
        length: c_int,
        extents: *mut GlyphInfo,
    );
    pub(super) fn XftDrawPicture(draw: *mut XftDraw) -> Xid;
    pub(super) fn XftDrawCreate(
        display: *mut Display,
        drawable: Xid,
        visual: *mut c_void,
        colormap: c_ulong,
    ) -> *mut XftDraw;
    pub(super) fn XftDrawSetClip(draw: *mut XftDraw, region: *mut c_void) -> c_int;
    pub(super) fn XftDrawSetClipRectangles(
        draw: *mut XftDraw,
        x_origin: c_int,
        y_origin: c_int,
        rectangles: *const ClipRectangle,
        count: c_int,
    ) -> c_int;
    pub(super) fn XftDrawDestroy(draw: *mut XftDraw);
    pub(super) fn XftDrawChange(draw: *mut XftDraw, drawable: Xid);
    pub(super) fn XftDrawStringUtf8(
        draw: *mut XftDraw,
        color: *const Color,
        font: *mut Font,
        x: c_int,
        y: c_int,
        text: *const u8,
        length: c_int,
    );
    pub(super) fn XftDrawRect(
        draw: *mut XftDraw,
        color: *const Color,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
    );
    pub(super) fn XftColorAllocValue(
        display: *mut Display,
        visual: *mut c_void,
        colormap: c_ulong,
        value: *const RenderColor,
        color: *mut Color,
    ) -> c_int;
    pub(super) fn XftColorFree(
        display: *mut Display,
        visual: *mut c_void,
        colormap: c_ulong,
        color: *mut Color,
    );
}
