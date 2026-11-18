//! Native AppKit frontend. All Objective-C and Core Foundation objects stay on
//! the main thread. PTY creation uses a fresh-exec helper and is safe after
//! AppKit starts worker threads.
//! File-descriptor readiness is integrated with the AppKit run loop; no polling
//! thread or periodic read timer is required.

use crate::{Config, appearance::Appearance, os};
use alloc::{
    boxed::Box, collections::BTreeMap, ffi::CString, string::String, sync::Arc, vec, vec::Vec,
};
use core::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use zterm::{
    BOLD, Cell, CursorShape, DIM, EXPLICIT_BG, HIDDEN, INVERSE, ITALIC, STRIKE, Terminal,
    UNDERLINE,
    input::{self, Key, KeyEvent, ModifierKey, Modifiers, MouseEvent},
};

use ztty::session_core::{ReadStep, SessionCore};

type Id = *mut c_void;
type Sel = *mut c_void;
const NIL: Id = ptr::null_mut();
const NOT_FOUND: usize = isize::MAX as usize;
const MAX_INPUT: usize = 8 * 1024 * 1024;
const MAX_MARKED: usize = 4096;
const MAX_LINES: usize = 1024;

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Point {
    x: f64,
    y: f64,
}
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Size {
    width: f64,
    height: f64,
}
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Rect {
    origin: Point,
    size: Size,
}
impl Rect {
    fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            origin: Point { x, y },
            size: Size { width, height },
        }
    }
}
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Range {
    location: usize,
    length: usize,
}
#[repr(C)]
struct FdContext {
    version: isize,
    info: Id,
    retain: Option<unsafe extern "C" fn(Id) -> Id>,
    release: Option<unsafe extern "C" fn(Id)>,
    description: Option<unsafe extern "C" fn(Id) -> Id>,
}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn objc_getProtocol(name: *const c_char) -> Id;
    fn objc_allocateClassPair(parent: Id, name: *const c_char, extra: usize) -> Id;
    fn objc_registerClassPair(class: Id);
    fn class_addMethod(
        class: Id,
        selector: Sel,
        implementation: *const c_void,
        encoding: *const c_char,
    ) -> bool;
    fn class_addProtocol(class: Id, protocol: Id) -> bool;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn sel_isEqual(a: Sel, b: Sel) -> bool;
    fn objc_msgSend();
    #[cfg(target_arch = "x86_64")]
    fn objc_msgSend_stret();
}
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}
#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: Id);
    fn CFRetain(value: Id) -> Id;
    fn CFStringCreateWithBytes(
        allocator: Id,
        bytes: *const u8,
        count: isize,
        encoding: u32,
        external: bool,
    ) -> Id;
    fn CFDictionaryCreate(
        allocator: Id,
        keys: *const Id,
        values: *const Id,
        count: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> Id;
    fn CFAttributedStringCreate(allocator: Id, string: Id, attributes: Id) -> Id;
    static kCFBooleanTrue: Id;
    static kCFTypeDictionaryKeyCallBacks: [usize; 6];
    static kCFTypeDictionaryValueCallBacks: [usize; 5];
    static kCFRunLoopCommonModes: Id;
    static kCFRunLoopDefaultMode: Id;
    fn CFRunLoopGetMain() -> Id;
    fn CFRunLoopAddSource(run_loop: Id, source: Id, mode: Id);
    fn CFRunLoopRemoveSource(run_loop: Id, source: Id, mode: Id);
    fn CFFileDescriptorCreate(
        allocator: Id,
        fd: c_int,
        close_on_invalidate: bool,
        callback: unsafe extern "C" fn(Id, usize, Id),
        context: *const FdContext,
    ) -> Id;
    fn CFFileDescriptorCreateRunLoopSource(allocator: Id, fd: Id, order: isize) -> Id;
    fn CFFileDescriptorEnableCallBacks(fd: Id, types: usize);
    fn CFFileDescriptorDisableCallBacks(fd: Id, types: usize);
    fn CFFileDescriptorInvalidate(fd: Id);
}
#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    static kCTFontAttributeName: Id;
    static kCTForegroundColorFromContextAttributeName: Id;
    fn CTFontCreateWithName(name: Id, size: f64, matrix: *const c_void) -> Id;
    fn CTFontCreateCopyWithSymbolicTraits(
        font: Id,
        size: f64,
        matrix: *const c_void,
        value: u32,
        mask: u32,
    ) -> Id;
    fn CTFontGetAscent(font: Id) -> f64;
    fn CTFontGetDescent(font: Id) -> f64;
    fn CTFontGetLeading(font: Id) -> f64;
    fn CTFontGetGlyphsForCharacters(
        font: Id,
        characters: *const u16,
        glyphs: *mut u16,
        count: isize,
    ) -> bool;
    fn CTFontGetAdvancesForGlyphs(
        font: Id,
        orientation: u32,
        glyphs: *const u16,
        advances: *mut Size,
        count: isize,
    ) -> f64;
    fn CTLineCreateWithAttributedString(string: Id) -> Id;
    fn CTLineDraw(line: Id, context: Id);
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGContextSaveGState(context: Id);
    fn CGContextRestoreGState(context: Id);
    fn CGContextClearRect(context: Id, rect: Rect);
    fn CGContextClipToRect(context: Id, rect: Rect);
    fn CGContextSetRGBFillColor(context: Id, red: f64, green: f64, blue: f64, alpha: f64);
    fn CGContextFillRect(context: Id, rect: Rect);
    fn CGContextSetTextPosition(context: Id, x: f64, y: f64);
    fn CGContextSetShouldAntialias(context: Id, value: bool);
    fn CGContextSetInterpolationQuality(context: Id, quality: i32);
    fn CGContextDrawImage(context: Id, rect: Rect, image: Id);
    static kCGColorSpaceSRGB: Id;
    fn CGColorSpaceCreateWithName(name: Id) -> Id;
    fn CGColorSpaceRelease(space: Id);
    fn CGDataProviderCreateWithData(
        info: Id,
        data: *const c_void,
        bytes: usize,
        release: Option<unsafe extern "C" fn(Id, *const c_void, usize)>,
    ) -> Id;
    fn CGDataProviderRelease(provider: Id);
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: Id,
        bitmap_info: u32,
        provider: Id,
        decode: *const f64,
        interpolate: bool,
        intent: i32,
    ) -> Id;
    fn CGImageCreateWithImageInRect(image: Id, rect: Rect) -> Id;
    fn CGImageRelease(image: Id);
}

// Every call uses the exact Objective-C method signature. NSRect's 32-byte
// return uses objc_msgSend_stret on Intel and ordinary objc_msgSend on arm64.
macro_rules! send {
    ($object:expr, $name:expr, $result:ty $(, $ty:ty => $arg:expr)* $(,)?) => {{
        #[allow(unused_unsafe)]
        let f: unsafe extern "C" fn(Id, Sel $(, $ty)*) -> $result = unsafe { core::mem::transmute(objc_msgSend as *const ()) };
        #[allow(unused_unsafe)]
        let result = unsafe { f($object, selector($name) $(, $arg)*) };
        result
    }};
}
fn selector(name: &CStr) -> Sel {
    unsafe { sel_registerName(name.as_ptr()) }
}
fn class(name: &CStr) -> Id {
    unsafe { objc_getClass(name.as_ptr()) }
}
fn release(object: Id) {
    if !object.is_null() {
        send!(object, c"release", ());
    }
}
fn cf_release(object: Id) {
    if !object.is_null() {
        unsafe { CFRelease(object) };
    }
}
fn rect0(object: Id, name: &CStr) -> Rect {
    #[cfg(target_arch = "x86_64")]
    {
        let f: unsafe extern "C" fn(*mut Rect, Id, Sel) =
            unsafe { core::mem::transmute(objc_msgSend_stret as *const ()) };
        let mut result = Rect::default();
        unsafe { f(&mut result, object, selector(name)) };
        result
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        send!(object, name, Rect)
    }
}
fn rect_convert(object: Id, name: &CStr, rect: Rect, view: Option<Id>) -> Rect {
    #[cfg(target_arch = "x86_64")]
    {
        let mut result = Rect::default();
        if let Some(view) = view {
            let f: unsafe extern "C" fn(*mut Rect, Id, Sel, Rect, Id) =
                unsafe { core::mem::transmute(objc_msgSend_stret as *const ()) };
            unsafe { f(&mut result, object, selector(name), rect, view) };
        } else {
            let f: unsafe extern "C" fn(*mut Rect, Id, Sel, Rect) =
                unsafe { core::mem::transmute(objc_msgSend_stret as *const ()) };
            unsafe { f(&mut result, object, selector(name), rect) };
        }
        result
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        match view {
            Some(view) => send!(object, name, Rect, Rect => rect, Id => view),
            None => send!(object, name, Rect, Rect => rect),
        }
    }
}
fn ns_string(text: &str) -> Id {
    let object = send!(class(c"NSString"), c"alloc", Id);
    send!(object, c"initWithBytes:length:encoding:", Id, *const u8 => text.as_ptr(), usize => text.len(), usize => 4)
}
fn text_from_object(mut object: Id, limit: usize) -> String {
    if object.is_null() {
        return String::new();
    }
    if send!(object, c"isKindOfClass:", bool, Id => class(c"NSAttributedString")) {
        object = send!(object, c"string", Id);
    }
    let n = send!(object, c"lengthOfBytesUsingEncoding:", usize, usize => 4);
    if n > limit {
        return String::new();
    }
    let bytes = send!(object, c"UTF8String", *const u8);
    if bytes.is_null() {
        return String::new();
    }
    String::from_utf8_lossy(unsafe { core::slice::from_raw_parts(bytes, n) }).into_owned()
}
fn fill(context: Id, rect: Rect, color: u32, alpha: f64) {
    unsafe {
        CGContextSetRGBFillColor(
            context,
            ((color >> 16) & 255) as f64 / 255.0,
            ((color >> 8) & 255) as f64 / 255.0,
            (color & 255) as f64 / 255.0,
            alpha,
        );
        CGContextFillRect(context, rect);
    }
}

struct Renderer {
    fonts: [Id; 4],
    lines: BTreeMap<(u32, u32, u32, u8), Id>,
    color_space: Id,
    cell_width: f64,
    cell_height: f64,
    descent: f64,
}
impl Renderer {
    fn new(appearance: &Appearance) -> Result<Self, &'static str> {
        let name = appearance.font.split(':').next().unwrap_or("Menlo");
        let name = if name == "monospace" || name.is_empty() {
            "Menlo"
        } else {
            name
        };
        let points = appearance
            .font
            .split(':')
            .find_map(|s| s.strip_prefix("size="))
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|n| n.is_finite())
            .unwrap_or(11.0)
            .clamp(6.0, 72.0);
        let string = ns_string(name);
        let base = unsafe { CTFontCreateWithName(string, points, ptr::null()) };
        release(string);
        if base.is_null() {
            return Err("cannot create a CoreText font");
        }
        let mut fonts = [base; 4];
        for (i, font) in fonts.iter_mut().enumerate().skip(1) {
            let traits = ((i & 1) as u32) * 2 + (((i >> 1) & 1) as u32);
            let alternate =
                unsafe { CTFontCreateCopyWithSymbolicTraits(base, 0.0, ptr::null(), traits, 3) };
            *font = if alternate.is_null() {
                unsafe { CFRetain(base) }
            } else {
                alternate
            };
        }
        let mut glyph = 0u16;
        let mut advance = Size::default();
        unsafe {
            CTFontGetGlyphsForCharacters(base, &('M' as u16), &mut glyph, 1);
            CTFontGetAdvancesForGlyphs(base, 0, &glyph, &mut advance, 1);
        }
        let ascent = unsafe { CTFontGetAscent(base) };
        let descent = unsafe { CTFontGetDescent(base) };
        let leading = unsafe { CTFontGetLeading(base) };
        // Kitty and Sixel pixels use sRGB. Device RGB makes their displayed
        // colors depend on the monitor profile and changes captured pixels.
        let color_space = unsafe { CGColorSpaceCreateWithName(kCGColorSpaceSRGB) };
        if color_space.is_null() {
            for font in fonts {
                cf_release(font);
            }
            return Err("cannot create the CoreGraphics color space");
        }
        Ok(Self {
            fonts,
            lines: BTreeMap::new(),
            color_space,
            cell_width: (advance.width.max(points * 0.4) + 0.999) as u32 as f64,
            cell_height: (ascent + descent + leading.max(0.0) + 1.999) as u32 as f64,
            descent: (descent + 0.999) as u32 as f64,
        })
    }
    fn make_line(&self, text: &str, style: u8) -> Id {
        let string = unsafe {
            CFStringCreateWithBytes(NIL, text.as_ptr(), text.len() as isize, 0x08000100, false)
        };
        if string.is_null() {
            return NIL;
        }
        let keys = unsafe {
            [
                kCTFontAttributeName,
                kCTForegroundColorFromContextAttributeName,
            ]
        };
        let values = unsafe { [self.fonts[style as usize], kCFBooleanTrue] };
        let dictionary = unsafe {
            CFDictionaryCreate(
                NIL,
                keys.as_ptr(),
                values.as_ptr(),
                2,
                ptr::addr_of!(kCFTypeDictionaryKeyCallBacks).cast(),
                ptr::addr_of!(kCFTypeDictionaryValueCallBacks).cast(),
            )
        };
        let attributed = unsafe { CFAttributedStringCreate(NIL, string, dictionary) };
        let line = if attributed.is_null() {
            NIL
        } else {
            unsafe { CTLineCreateWithAttributedString(attributed) }
        };
        cf_release(attributed);
        cf_release(dictionary);
        cf_release(string);
        line
    }
    fn cell(&mut self, context: Id, cell: &Cell, x: f64, y: f64, color: u32) {
        if cell.width == 0
            || cell.flags & HIDDEN != 0
            || cell.ch == ' ' && cell.combining == ['\0'; 2]
        {
            return;
        }
        let style = u8::from(cell.flags & BOLD != 0) | (u8::from(cell.flags & ITALIC != 0) << 1);
        let key = (
            cell.ch as u32,
            cell.combining[0] as u32,
            cell.combining[1] as u32,
            style,
        );
        let line = if let Some(&line) = self.lines.get(&key) {
            line
        } else {
            if self.lines.len() >= MAX_LINES {
                for (_, line) in core::mem::take(&mut self.lines) {
                    cf_release(line);
                }
            }
            let mut bytes = [0u8; 12];
            let mut n = cell.ch.encode_utf8(&mut bytes[..4]).len();
            for &ch in &cell.combining {
                if ch != '\0' {
                    n += ch.encode_utf8(&mut bytes[n..n + 4]).len();
                }
            }
            let text = core::str::from_utf8(&bytes[..n]).unwrap_or("");
            let line = self.make_line(text, style);
            if !line.is_null() {
                self.lines.insert(key, line);
            }
            line
        };
        if !line.is_null() {
            unsafe {
                CGContextSetRGBFillColor(
                    context,
                    ((color >> 16) & 255) as f64 / 255.0,
                    ((color >> 8) & 255) as f64 / 255.0,
                    (color & 255) as f64 / 255.0,
                    1.0,
                );
                CGContextSetTextPosition(context, x, y + self.descent);
                CTLineDraw(line, context);
            }
        }
    }
}
impl Drop for Renderer {
    fn drop(&mut self) {
        for (_, line) in core::mem::take(&mut self.lines) {
            cf_release(line);
        }
        for font in self.fonts {
            cf_release(font);
        }
        unsafe { CGColorSpaceRelease(self.color_space) };
    }
}

#[derive(Clone, Copy, Default, Eq, Ord, PartialEq, PartialOrd)]
struct CellPosition {
    row: usize,
    col: usize,
}
#[derive(Clone, Copy)]
struct Selection {
    start: CellPosition,
    end: CellPosition,
}
impl Selection {
    fn contains(self, row: usize, col: usize) -> bool {
        let p = CellPosition { row, col };
        p >= self.start.min(self.end) && p <= self.start.max(self.end)
    }
}
#[derive(Clone, Copy)]
struct PendingKey {
    key: Key,
    modifiers: Modifiers,
    event: KeyEvent,
}
struct Host {
    app: Id,
    window: Id,
    view: Id,
    fd_source: Id,
    core: SessionCore,
    renderer: Renderer,
    appearance: Appearance,
    running: bool,
    force: bool,
    wake_posted: bool,
    history: usize,
    selection: Option<Selection>,
    selecting: bool,
    mouse_button: Option<u8>,
    swallowed_buttons: [bool; 3],
    read_buffer: Vec<u8>,
    encoded: Vec<u8>,
    marked: String,
    marked_selection: Range,
    pending_key: Option<PendingKey>,
    composing: bool,
    intercepted: [bool; 128],
    held: [Option<Key>; 128],
    scroll_accumulator: f64,
    focused: bool,
    blink_visible: bool,
    blink_at: u64,
    sync_since: Option<u64>,
    last_cursor: (usize, usize),
    last_revision: u64,
    title: String,
    scale: f64,
}
static ACTIVE: AtomicPtr<Host> = AtomicPtr::new(ptr::null_mut());
fn host_ptr() -> *mut Host {
    ACTIVE.load(Ordering::Relaxed)
}
impl Host {
    fn queue(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if self.core.input.push(bytes) {
            self.flush();
        } else {
            os::print(2, b"ztty: input queue full; paste rejected\n");
        }
    }
    fn flush(&mut self) {
        if self.core.flush_input().is_err() {
            self.running = false;
        }
        if !self.fd_source.is_null() {
            unsafe {
                if self.core.input.pending() {
                    CFFileDescriptorEnableCallBacks(self.fd_source, 2);
                } else {
                    CFFileDescriptorDisableCallBacks(self.fd_source, 2);
                }
            }
        }
    }
    fn send_key(&mut self, key: Key, mods: Modifiers, event: KeyEvent, text: &str) {
        self.encoded.clear();
        input::encode_key_into(
            key,
            mods,
            event,
            text,
            &self.core.terminal.modes().input(),
            &mut self.encoded,
        );
        let encoded = core::mem::take(&mut self.encoded);
        let sent = !encoded.is_empty();
        self.queue(&encoded);
        self.encoded = encoded;
        if sent && event != KeyEvent::Release && !matches!(key, Key::Modifier(_)) {
            self.history = 0;
            self.selection = None;
            self.blink_visible = true;
            self.blink_at =
                os::milliseconds().saturating_add(self.appearance.cursor_blink_interval);
            self.force = true;
        }
    }
    fn send_text(&mut self, text: &str) {
        self.encoded.clear();
        input::encode_text_into(text, &self.core.terminal.modes().input(), &mut self.encoded);
        let encoded = core::mem::take(&mut self.encoded);
        self.queue(&encoded);
        self.encoded = encoded;
        self.history = 0;
        self.selection = None;
        self.force = true;
    }
    fn send_mouse(&mut self, event: MouseEvent, mods: Modifiers) {
        self.encoded.clear();
        input::encode_mouse_into(
            event,
            mods,
            &self.core.terminal.modes().input(),
            &mut self.encoded,
        );
        let encoded = core::mem::take(&mut self.encoded);
        self.queue(&encoded);
        self.encoded = encoded;
    }
    fn read_ready(&mut self) {
        let mut budget = 1024usize * 1024;
        while budget != 0 {
            match self.core.read_once(&mut self.read_buffer, false) {
                Ok(ReadStep::Data {
                    bytes,
                    history_before,
                    history_after,
                }) => {
                    budget = budget.saturating_sub(bytes);
                    if self.history > 0 {
                        self.history = self
                            .history
                            .saturating_add(history_after.saturating_sub(history_before))
                            .min(history_after);
                        self.selection = None;
                    }
                    if !self.core.queue_responses() {
                        os::print(2, b"ztty: input queue full; terminal replies rejected\n");
                    }
                    self.flush();
                }
                Ok(ReadStep::Pending) => break,
                Ok(ReadStep::Eof) | Err(_) => {
                    self.running = false;
                    break;
                }
            }
        }
        let _ = self.core.pty.reap();
    }
    fn position(&self, event: Id) -> CellPosition {
        let point = send!(event, c"locationInWindow", Point);
        let point = send!(self.view, c"convertPoint:fromView:", Point, Point => point, Id => NIL);
        let bounds = rect0(self.view, c"bounds");
        let x = (point.x - self.appearance.padding as f64).max(0.0);
        let y = (bounds.size.height - point.y - self.appearance.padding as f64).max(0.0);
        CellPosition {
            col: (x / self.renderer.cell_width) as usize,
            row: (y / self.renderer.cell_height) as usize,
        }
    }
    fn selected_text(&self) -> String {
        let Some(selection) = self.selection else {
            return String::new();
        };
        ztty::selection::text(
            &self.core.terminal,
            (selection.start.col, selection.start.row),
            (selection.end.col, selection.end.row),
            self.history,
        )
    }
    fn resize(&mut self) {
        let bounds = rect0(self.view, c"bounds");
        let scale = send!(self.window, c"backingScaleFactor", f64).clamp(1.0, 8.0);
        let cols = ((bounds.size.width - 2.0 * self.appearance.padding as f64).max(1.0)
            / self.renderer.cell_width) as usize;
        let rows = ((bounds.size.height - 2.0 * self.appearance.padding as f64).max(1.0)
            / self.renderer.cell_height) as usize;
        let cols = cols.clamp(1, 1024);
        let rows = rows.clamp(1, 1024);
        let cw = (self.renderer.cell_width * scale) as u32;
        let ch = (self.renderer.cell_height * scale) as u32;
        let mut size = zpty::Size::new(cols as u16, rows as u16);
        size.pixel_width = ((cw as usize * cols).min(u16::MAX as usize)) as u16;
        size.pixel_height = ((ch as usize * rows).min(u16::MAX as usize)) as u16;
        if self.core.resize_with_cell_size(size, (cw, ch)).is_err() {
            os::print(2, b"ztty: PTY resize failed; retaining the terminal grid\n");
            self.force = true;
            return;
        }
        self.scale = scale;
        self.history = self.history.min(self.core.terminal.history_len());
        self.selection = None;
        self.force = true;
    }
    fn tick(&mut self) {
        let now = os::milliseconds();
        let modes = *self.core.terminal.modes();
        if modes.synchronized_output {
            let start = self.sync_since.get_or_insert(now);
            if now.saturating_sub(*start) >= 150 {
                self.core.terminal.force_end_sync();
                self.sync_since = None;
                self.force = true;
            }
        } else {
            self.sync_since = None;
        }
        let cursor = self.core.terminal.cursor();
        if cursor != self.last_cursor {
            self.last_cursor = cursor;
            self.blink_visible = true;
            self.blink_at = now + self.appearance.cursor_blink_interval;
            self.force = true;
        }
        if self.focused
            && modes.cursor_visible
            && modes.cursor_blink.unwrap_or(self.appearance.cursor_blink)
            && now >= self.blink_at
        {
            self.blink_visible = !self.blink_visible;
            self.blink_at = now + self.appearance.cursor_blink_interval;
            self.force = true;
        }
        let revision = self.core.terminal.graphics().revision();
        if revision != self.last_revision {
            self.last_revision = revision;
            self.force = true;
        }
        if self.title != self.core.terminal.title() {
            self.title = String::from(self.core.terminal.title());
            let title = ns_string(if self.title.is_empty() {
                "ztty"
            } else {
                &self.title
            });
            send!(self.window, c"setTitle:", (), Id => title);
            release(title);
        }
        if !self.core.terminal.modes().synchronized_output
            && (self.force || self.core.terminal.dirty())
        {
            send!(self.view, c"setNeedsDisplay:", (), bool => true);
            self.force = false;
        }
    }
    fn draw(&mut self) {
        let context = send!(
            send!(class(c"NSGraphicsContext"), c"currentContext", Id),
            c"CGContext",
            Id
        );
        if context.is_null() {
            return;
        }
        let bounds = rect0(self.view, c"bounds");
        let padding = self.appearance.padding as f64;
        let width = self.renderer.cell_width;
        let height = self.renderer.cell_height;
        unsafe {
            CGContextSaveGState(context);
            CGContextClearRect(context, bounds);
            CGContextSetShouldAntialias(context, true);
        }
        fill(
            context,
            bounds,
            self.appearance.theme.background,
            self.appearance.opacity as f64 / 100.0,
        );
        let viewport = Rect::new(
            padding,
            bounds.size.height - padding - height * self.core.terminal.rows() as f64,
            width * self.core.terminal.cols() as f64,
            height * self.core.terminal.rows() as f64,
        );
        unsafe {
            CGContextClipToRect(context, viewport);
        }
        self.draw_images(context, bounds.size.height, i32::MIN, -1_073_741_824);
        for row in 0..self.core.terminal.rows() {
            for col in 0..self.core.terminal.cols() {
                let cell = self.core.terminal.view_cell(col, row, self.history);
                let selected = self.selection.is_some_and(|s| s.contains(row, col));
                if cell.flags & (EXPLICIT_BG | INVERSE) != 0 || selected {
                    let bg = if selected {
                        self.appearance.selection_background
                    } else if cell.flags & INVERSE != 0 {
                        cell.fg
                    } else {
                        cell.bg
                    };
                    fill(
                        context,
                        Rect::new(
                            padding + col as f64 * width,
                            bounds.size.height - padding - (row + 1) as f64 * height,
                            width,
                            height,
                        ),
                        bg,
                        1.0,
                    );
                }
            }
        }
        self.draw_images(context, bounds.size.height, -1_073_741_824, 0);
        for row in 0..self.core.terminal.rows() {
            for col in 0..self.core.terminal.cols() {
                let cell = self.core.terminal.view_cell(col, row, self.history);
                let selected = self.selection.is_some_and(|s| s.contains(row, col));
                let mut fg = if selected {
                    self.appearance.selection_foreground
                } else if cell.flags & INVERSE != 0 {
                    cell.bg
                } else {
                    cell.fg
                };
                if cell.flags & DIM != 0 {
                    fg = ((fg & 0xfefefe) >> 1)
                        + ((self.appearance.theme.background & 0xfefefe) >> 1);
                }
                let x = padding + col as f64 * width;
                let y = bounds.size.height - padding - (row + 1) as f64 * height;
                self.renderer.cell(context, cell, x, y, fg);
                if cell.flags & UNDERLINE != 0 {
                    fill(
                        context,
                        Rect::new(x, y + 1.0, width * cell.width.max(1) as f64, 1.0),
                        fg,
                        1.0,
                    );
                }
                if cell.flags & STRIKE != 0 {
                    fill(
                        context,
                        Rect::new(x, y + height * 0.45, width * cell.width.max(1) as f64, 1.0),
                        fg,
                        1.0,
                    );
                }
            }
        }
        self.draw_images(context, bounds.size.height, 0, i32::MAX);
        let (col, row) = self.core.terminal.cursor();
        let cx = padding + col as f64 * width;
        let cy = bounds.size.height - padding - (row + 1) as f64 * height;
        if !self.marked.is_empty() {
            let line = self.renderer.make_line(&self.marked, 0);
            fill(
                context,
                Rect::new(
                    cx,
                    cy,
                    (self.marked.chars().count() as f64 * width)
                        .min(viewport.size.width - col as f64 * width),
                    height,
                ),
                self.appearance.selection_background,
                1.0,
            );
            unsafe {
                let color = self.appearance.theme.foreground;
                CGContextSetRGBFillColor(
                    context,
                    ((color >> 16) & 255) as f64 / 255.0,
                    ((color >> 8) & 255) as f64 / 255.0,
                    (color & 255) as f64 / 255.0,
                    1.0,
                );
                CGContextSetTextPosition(context, cx, cy + self.renderer.descent);
                if !line.is_null() {
                    CTLineDraw(line, context);
                }
            }
            cf_release(line);
            fill(
                context,
                Rect::new(
                    cx,
                    cy + 1.0,
                    width * self.marked.chars().count() as f64,
                    1.0,
                ),
                self.appearance.cursor_color,
                1.0,
            );
        } else if self.history == 0
            && self.core.terminal.modes().cursor_visible
            && (self.blink_visible || !self.focused)
        {
            let shape = self
                .core
                .terminal
                .modes()
                .cursor_shape
                .unwrap_or(self.appearance.cursor_shape);
            let rect = match shape {
                CursorShape::Block => Rect::new(cx, cy, width, height),
                CursorShape::Underline => Rect::new(cx, cy, width, 2.0),
                CursorShape::Bar => Rect::new(cx, cy, 2.0, height),
            };
            fill(
                context,
                rect,
                self.appearance.cursor_color,
                if shape == CursorShape::Block {
                    0.5
                } else {
                    1.0
                },
            );
        }
        unsafe {
            CGContextRestoreGState(context);
        }
        self.core.terminal.clear_dirty();
    }
    fn draw_images(&self, context: Id, view_height: f64, min_z: i32, max_z: i32) {
        unsafe {
            CGContextSetInterpolationQuality(context, 1);
        }
        for draw in self.core.terminal.graphics_draws(self.history) {
            if draw.z < min_z || (draw.z >= max_z && max_z != i32::MAX) {
                continue;
            }
            if draw.dest.width == 0
                || draw.dest.height == 0
                || draw.source.width == 0
                || draw.source.height == 0
            {
                continue;
            }
            // The provider owns an Arc reference, not a borrowed pixel pointer.
            // CoreGraphics may retain it beyond this callback; scene deletion
            // cannot invalidate its bytes. No second RGBA buffer is allocated.
            // We retain no image cache and release our native references here.
            unsafe {
                let image = draw.image;
                let owner = Arc::into_raw(Arc::clone(&image.pixels));
                let provider = CGDataProviderCreateWithData(
                    owner.cast_mut().cast(),
                    image.pixels.as_ptr().cast(),
                    image.pixels.len(),
                    Some(release_pixels),
                );
                if provider.is_null() {
                    drop(Arc::from_raw(owner));
                    continue;
                }
                let whole = CGImageCreate(
                    image.width as usize,
                    image.height as usize,
                    8,
                    32,
                    image.width as usize * 4,
                    self.renderer.color_space,
                    3 | (4 << 12),
                    provider,
                    ptr::null(),
                    false,
                    0,
                );
                if whole.is_null() {
                    CGDataProviderRelease(provider);
                    continue;
                }
                let crop = CGImageCreateWithImageInRect(
                    whole,
                    Rect::new(
                        draw.source.x as f64,
                        draw.source.y as f64,
                        draw.source.width as f64,
                        draw.source.height as f64,
                    ),
                );
                if !crop.is_null() {
                    let x = self.appearance.padding as f64 + draw.dest.x as f64 / self.scale;
                    let y = view_height
                        - self.appearance.padding as f64
                        - (draw.dest.y as f64 + draw.dest.height as f64) / self.scale;
                    CGContextDrawImage(
                        context,
                        Rect::new(
                            x,
                            y,
                            draw.dest.width as f64 / self.scale,
                            draw.dest.height as f64 / self.scale,
                        ),
                        crop,
                    );
                    CGImageRelease(crop);
                }
                CGImageRelease(whole);
                CGDataProviderRelease(provider);
            }
        }
    }
}

// CoreGraphics invokes this only when the provider's final owner releases it.
// Arc permits that callback to run on a CoreGraphics worker thread safely.
unsafe extern "C" fn release_pixels(info: Id, _: *const c_void, _: usize) {
    if !info.is_null() {
        unsafe {
            drop(Arc::from_raw(info.cast::<Vec<u8>>()));
        }
    }
}

fn modifiers(event: Id) -> Modifiers {
    let flags = send!(event, c"modifierFlags", usize);
    Modifiers(
        u8::from(flags & (1 << 17) != 0)
            | (u8::from(flags & (1 << 19) != 0) << 1)
            | (u8::from(flags & (1 << 18) != 0) << 2)
            | (u8::from(flags & (1 << 20) != 0) << 3)
            | (u8::from(flags & (1 << 16) != 0) << 6),
    )
}
fn event_key(event: Id) -> Key {
    let code = send!(event, c"keyCode", u16);
    match code {
        36 => Key::Enter,
        48 => Key::Tab,
        51 => Key::Backspace,
        53 => Key::Escape,
        123 => Key::Left,
        124 => Key::Right,
        125 => Key::Down,
        126 => Key::Up,
        115 => Key::Home,
        119 => Key::End,
        116 => Key::PageUp,
        121 => Key::PageDown,
        117 => Key::Delete,
        122 => Key::Function(1),
        120 => Key::Function(2),
        99 => Key::Function(3),
        118 => Key::Function(4),
        96 => Key::Function(5),
        97 => Key::Function(6),
        98 => Key::Function(7),
        100 => Key::Function(8),
        101 => Key::Function(9),
        109 => Key::Function(10),
        103 => Key::Function(11),
        111 => Key::Function(12),
        105 => Key::Function(13),
        107 => Key::Function(14),
        113 => Key::Function(15),
        106 => Key::Function(16),
        64 => Key::Function(17),
        79 => Key::Function(18),
        80 => Key::Function(19),
        90 => Key::Function(20),
        65 => Key::KeypadDecimal,
        67 => Key::KeypadMultiply,
        69 => Key::KeypadAdd,
        75 => Key::KeypadDivide,
        76 => Key::KeypadEnter,
        78 => Key::KeypadSubtract,
        81 => Key::KeypadEqual,
        82 => Key::KeypadDigit(0),
        83..=89 => Key::KeypadDigit((code - 82) as u8),
        91 => Key::KeypadDigit(8),
        92 => Key::KeypadDigit(9),
        _ => {
            let string = send!(event, c"charactersIgnoringModifiers", Id);
            text_from_object(string, 128)
                .chars()
                .next()
                .filter(|c| !c.is_control() && (*c as u32) < 0xf700)
                .map(|c| Key::Character(c.to_ascii_lowercase()))
                .unwrap_or(Key::Unidentified)
        }
    }
}
fn wake() {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let app = unsafe {
        let host = &mut *pointer;
        if host.wake_posted {
            return;
        }
        host.wake_posted = true;
        host.app
    };
    let event = send!(class(c"NSEvent"), c"otherEventWithType:location:modifierFlags:timestamp:windowNumber:context:subtype:data1:data2:", Id,
        usize => 15, Point => Point::default(), usize => 0, f64 => 0.0, isize => 0, Id => NIL, i16 => 0, isize => 0, isize => 0);
    if !event.is_null() {
        send!(app, c"postEvent:atStart:", (), Id => event, bool => false);
    }
}
unsafe extern "C" fn fd_ready(fd: Id, types: usize, info: Id) {
    if info.is_null() {
        return;
    }
    let running = unsafe {
        let host = &mut *info.cast::<Host>();
        if types & 1 != 0 {
            host.read_ready();
        }
        if types & 2 != 0 {
            host.flush();
        }
        host.running
    };
    if running {
        unsafe { CFFileDescriptorEnableCallBacks(fd, 1) };
    }
    wake();
}
unsafe extern "C" fn yes(_: Id, _: Sel) -> bool {
    true
}
unsafe extern "C" fn no(_: Id, _: Sel) -> bool {
    false
}
unsafe extern "C" fn yes_event(_: Id, _: Sel, _: Id) -> bool {
    true
}
unsafe extern "C" fn draw_rect(_: Id, _: Sel, _: Rect) {
    let pointer = host_ptr();
    if !pointer.is_null() {
        unsafe { (&mut *pointer).draw() };
    }
}
unsafe extern "C" fn window_close(_: Id, _: Sel, _: Id) -> bool {
    let pointer = host_ptr();
    if !pointer.is_null() {
        unsafe { (*pointer).running = false };
        wake();
    }
    true
}
unsafe extern "C" fn quit(_: Id, _: Sel, _: Id) {
    let pointer = host_ptr();
    if !pointer.is_null() {
        unsafe { (*pointer).running = false };
        wake();
    }
}
unsafe extern "C" fn window_resize(_: Id, _: Sel, _: Id) {
    let pointer = host_ptr();
    if !pointer.is_null() {
        let view = unsafe {
            (&mut *pointer).resize();
            (*pointer).view
        };
        let context = send!(view, c"inputContext", Id);
        send!(context, c"invalidateCharacterCoordinates", ());
        wake();
    }
}
unsafe extern "C" fn window_focus(_: Id, selector: Sel, _: Id) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let focus = unsafe { sel_isEqual(selector, self::selector(c"windowDidBecomeKey:")) };
    unsafe {
        let host = &mut *pointer;
        host.focused = focus;
        host.blink_visible = true;
        host.blink_at = os::milliseconds() + host.appearance.cursor_blink_interval;
        host.force = true;
        let encoded = input::encode_focus(focus, &host.core.terminal.modes().input());
        host.queue(&encoded);
    }
    wake();
}
fn copy_selection() {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let text = unsafe { (&*pointer).selected_text() };
    if text.is_empty() {
        return;
    }
    let pasteboard = send!(class(c"NSPasteboard"), c"generalPasteboard", Id);
    send!(pasteboard, c"clearContents", isize);
    let string = ns_string(&text);
    let kind = ns_string("public.utf8-plain-text");
    send!(pasteboard, c"setString:forType:", bool, Id => string, Id => kind);
    release(kind);
    release(string);
}
fn paste_clipboard() {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let pasteboard = send!(class(c"NSPasteboard"), c"generalPasteboard", Id);
    let kind = ns_string("public.utf8-plain-text");
    let string = send!(pasteboard, c"stringForType:", Id, Id => kind);
    let text = text_from_object(string, MAX_INPUT / 2);
    release(kind);
    if text.is_empty() {
        return;
    }
    unsafe {
        let host = &mut *pointer;
        let encoded = input::encode_paste(&text, &host.core.terminal.modes().input());
        host.queue(&encoded);
        host.history = 0;
        host.selection = None;
        host.force = true;
    }
    wake();
}
unsafe extern "C" fn key_down(view: Id, _: Sel, event: Id) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let key = event_key(event);
    let mods = modifiers(event);
    let code = send!(event, c"keyCode", u16) as usize;
    let repeated = send!(event, c"isARepeat", bool);
    if code < 128 {
        unsafe { (*pointer).held[code] = Some(key) };
    }
    let command = mods.contains(Modifiers::SUPER);
    let control_shift =
        mods.0 & (Modifiers::CTRL | Modifiers::SHIFT) == (Modifiers::CTRL | Modifiers::SHIFT);
    if (command || control_shift) && key == Key::Character('c') {
        copy_selection();
        if code < 128 {
            unsafe { (*pointer).intercepted[code] = true };
        }
        return;
    }
    if (command || control_shift) && key == Key::Character('v') {
        paste_clipboard();
        if code < 128 {
            unsafe { (*pointer).intercepted[code] = true };
        }
        return;
    }
    if command && key == Key::Character('q') {
        unsafe { (*pointer).running = false };
        wake();
        return;
    }
    if mods.contains(Modifiers::SHIFT) && matches!(key, Key::PageUp | Key::PageDown) {
        unsafe {
            let host = &mut *pointer;
            let step = host.core.terminal.rows().saturating_sub(1).max(1);
            host.history = if key == Key::PageUp {
                host.history
                    .saturating_add(step)
                    .min(host.core.terminal.history_len())
            } else {
                host.history.saturating_sub(step)
            };
            host.selection = None;
            host.force = true;
            if code < 128 {
                host.intercepted[code] = true;
            }
        }
        return;
    }
    let transition = if repeated {
        KeyEvent::Repeat
    } else {
        KeyEvent::Press
    };
    let composing = unsafe { !(&*pointer).marked.is_empty() };
    if !composing
        && (mods.contains(Modifiers::CTRL | Modifiers::SUPER)
            || !matches!(key, Key::Character(_) | Key::Unidentified))
    {
        let text = text_from_object(send!(event, c"characters", Id), 65536);
        unsafe { (&mut *pointer).send_key(key, mods, transition, &text) };
        return;
    }
    unsafe {
        (*pointer).composing = composing;
        (*pointer).pending_key = Some(PendingKey {
            key,
            modifiers: mods,
            event: transition,
        });
    }
    // No Rust borrow of Host crosses AppKit's synchronous IME callbacks.
    let context = send!(view, c"inputContext", Id);
    let handled = send!(context, c"handleEvent:", bool, Id => event);
    if !handled {
        let text = text_from_object(send!(event, c"characters", Id), 65536);
        unsafe { (&mut *pointer).send_key(key, mods, transition, &text) };
    }
    unsafe {
        if handled && (*pointer).composing && code < 128 {
            (*pointer).intercepted[code] = true;
        }
        (*pointer).pending_key = None;
    }
}
unsafe extern "C" fn key_up(_: Id, _: Sel, event: Id) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let code = send!(event, c"keyCode", u16) as usize;
    let mods = modifiers(event);
    unsafe {
        let host = &mut *pointer;
        let key = if code < 128 {
            host.held[code].take().unwrap_or_else(|| event_key(event))
        } else {
            event_key(event)
        };
        if code < 128 && core::mem::take(&mut host.intercepted[code]) {
            return;
        }
        host.send_key(key, mods, KeyEvent::Release, "");
    }
}
unsafe extern "C" fn flags_changed(_: Id, _: Sel, event: Id) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let code = send!(event, c"keyCode", u16) as usize;
    let mods = modifiers(event);
    let (key, bit) = match code {
        56 => (Key::Modifier(ModifierKey::LeftShift), Modifiers::SHIFT),
        60 => (Key::Modifier(ModifierKey::RightShift), Modifiers::SHIFT),
        59 => (Key::Modifier(ModifierKey::LeftControl), Modifiers::CTRL),
        62 => (Key::Modifier(ModifierKey::RightControl), Modifiers::CTRL),
        58 => (Key::Modifier(ModifierKey::LeftAlt), Modifiers::ALT),
        61 => (Key::Modifier(ModifierKey::RightAlt), Modifiers::ALT),
        55 => (Key::Modifier(ModifierKey::LeftSuper), Modifiers::SUPER),
        54 => (Key::Modifier(ModifierKey::RightSuper), Modifiers::SUPER),
        57 => (Key::CapsLock, Modifiers::CAPS_LOCK),
        _ => return,
    };
    unsafe {
        let host = &mut *pointer;
        let pressed = mods.contains(bit) && host.held[code].is_none();
        host.held[code] = if pressed { Some(key) } else { None };
        host.send_key(
            key,
            mods,
            if pressed {
                KeyEvent::Press
            } else {
                KeyEvent::Release
            },
            "",
        );
    }
}
unsafe extern "C" fn insert_text(_: Id, _: Sel, value: Id, _: Range) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let text = text_from_object(value, 65536);
    if text.is_empty() {
        return;
    }
    unsafe {
        let host = &mut *pointer;
        if let Some(pending) = host
            .pending_key
            .filter(|_| !host.composing && host.marked.is_empty())
        {
            host.send_key(pending.key, pending.modifiers, pending.event, &text);
        } else {
            host.send_text(&text);
        }
        host.marked.clear();
        host.marked_selection = Range::default();
        host.force = true;
    }
}
unsafe extern "C" fn set_marked(_: Id, _: Sel, value: Id, selected: Range, _: Range) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let text = text_from_object(value, MAX_MARKED);
    unsafe {
        let host = &mut *pointer;
        let length = text.encode_utf16().count();
        host.marked = text;
        host.marked_selection = Range {
            location: selected.location.min(length),
            length: selected
                .length
                .min(length.saturating_sub(selected.location)),
        };
        host.composing = true;
        host.force = true;
    }
}
unsafe extern "C" fn unmark(_: Id, _: Sel) {
    let pointer = host_ptr();
    if !pointer.is_null() {
        unsafe {
            (&mut *pointer).marked.clear();
            (*pointer).force = true;
        }
    }
}
unsafe extern "C" fn has_marked(_: Id, _: Sel) -> bool {
    let pointer = host_ptr();
    !pointer.is_null() && unsafe { !(&*pointer).marked.is_empty() }
}
unsafe extern "C" fn marked_range(_: Id, _: Sel) -> Range {
    let pointer = host_ptr();
    if !pointer.is_null() && unsafe { !(&*pointer).marked.is_empty() } {
        Range {
            location: 0,
            length: unsafe { (&*pointer).marked.encode_utf16().count() },
        }
    } else {
        Range {
            location: NOT_FOUND,
            length: 0,
        }
    }
}
unsafe extern "C" fn selected_range(_: Id, _: Sel) -> Range {
    let pointer = host_ptr();
    if !pointer.is_null() && unsafe { !(&*pointer).marked.is_empty() } {
        unsafe { (*pointer).marked_selection }
    } else {
        Range {
            location: 0,
            length: 0,
        }
    }
}
unsafe extern "C" fn valid_attributes(_: Id, _: Sel) -> Id {
    send!(class(c"NSArray"), c"array", Id)
}
unsafe extern "C" fn attributed_substring(_: Id, _: Sel, range: Range, actual: *mut Range) -> Id {
    let pointer = host_ptr();
    if pointer.is_null() {
        return NIL;
    }
    let marked = unsafe { &(*pointer).marked };
    let units: Vec<u16> = marked.encode_utf16().collect();
    if range.location >= units.len() {
        return NIL;
    }
    let length = range.length.min(units.len() - range.location);
    if !actual.is_null() {
        unsafe {
            *actual = Range {
                location: range.location,
                length,
            }
        };
    }
    let string = send!(send!(class(c"NSString"), c"alloc", Id), c"initWithCharacters:length:", Id, *const u16 => units[range.location..].as_ptr(), usize => length);
    let attributed = send!(send!(class(c"NSAttributedString"), c"alloc", Id), c"initWithString:", Id, Id => string);
    release(string);
    send!(attributed, c"autorelease", Id)
}
unsafe extern "C" fn character_index(_: Id, _: Sel, _: Point) -> usize {
    NOT_FOUND
}
unsafe extern "C" fn first_rect(view: Id, _: Sel, range: Range, actual: *mut Range) -> Rect {
    let pointer = host_ptr();
    if pointer.is_null() {
        return Rect::default();
    }
    if !actual.is_null() {
        unsafe { *actual = range };
    }
    let (window, rect) = unsafe {
        let host = &*pointer;
        let (col, row) = host.core.terminal.cursor();
        let bounds = rect0(view, c"bounds");
        (
            host.window,
            Rect::new(
                host.appearance.padding as f64 + col as f64 * host.renderer.cell_width,
                bounds.size.height
                    - host.appearance.padding as f64
                    - (row + 1) as f64 * host.renderer.cell_height,
                host.renderer.cell_width,
                host.renderer.cell_height,
            ),
        )
    };
    let in_window = rect_convert(view, c"convertRect:toView:", rect, Some(NIL));
    rect_convert(window, c"convertRectToScreen:", in_window, None)
}
unsafe extern "C" fn command(_: Id, _: Sel, _: Sel) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    unsafe {
        let host = &mut *pointer;
        if let Some(pending) = host.pending_key {
            host.send_key(pending.key, pending.modifiers, pending.event, "");
        }
    }
}
unsafe extern "C" fn mouse(view: Id, _: Sel, event: Id) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let kind = send!(event, c"type", usize);
    let mods = modifiers(event);
    let position = unsafe { (&*pointer).position(event) };
    let button = match send!(event, c"buttonNumber", isize) {
        0 => 0,
        1 => 2,
        2 => 1,
        _ => return,
    };
    if kind == 1 {
        send!(send!(view, c"window", Id), c"makeFirstResponder:", bool, Id => view);
    }
    let pressing = matches!(kind, 1 | 3 | 25);
    let releasing = matches!(kind, 2 | 4 | 26);
    let uri = if mods.contains(Modifiers::SUPER | Modifiers::CTRL) && pressing && button == 0 {
        unsafe {
            let host = &*pointer;
            let col = position.col.min(host.core.terminal.cols() - 1);
            let row = position.row.min(host.core.terminal.rows() - 1);
            let cell = host.core.terminal.view_cell(col, row, host.history);
            host.core
                .terminal
                .hyperlink(cell.link_id)
                .filter(|uri| {
                    uri.len() <= 8192
                        && !uri.chars().any(char::is_control)
                        && (uri.starts_with("https://")
                            || uri.starts_with("http://")
                            || uri.starts_with("mailto:"))
                })
                .map(String::from)
        }
    } else {
        None
    };
    if let Some(uri) = uri {
        unsafe {
            (*pointer).swallowed_buttons[button as usize] = true;
        }
        // Opening a URL can run framework code. No Host borrow crosses it.
        let string = ns_string(&uri);
        let url = send!(class(c"NSURL"), c"URLWithString:", Id, Id => string);
        send!(send!(class(c"NSWorkspace"), c"sharedWorkspace", Id), c"openURL:", bool, Id => url);
        release(string);
        return;
    }
    unsafe {
        let host = &mut *pointer;
        let col = position.col.min(host.core.terminal.cols() - 1);
        let row = position.row.min(host.core.terminal.rows() - 1);
        if releasing && core::mem::take(&mut host.swallowed_buttons[button as usize]) {
            return;
        }
        if host.core.terminal.modes().mouse_tracking != 0
            && !mods.contains(Modifiers::SHIFT)
            && host.history == 0
        {
            let report = if pressing {
                host.mouse_button = Some(button);
                MouseEvent::Press { button, col, row }
            } else if releasing {
                host.mouse_button = None;
                MouseEvent::Release { button, col, row }
            } else {
                MouseEvent::Motion {
                    button: host.mouse_button,
                    col,
                    row,
                }
            };
            host.send_mouse(report, mods);
        } else if button == 0 {
            if pressing {
                let position = CellPosition { col, row };
                host.selection = Some(Selection {
                    start: position,
                    end: position,
                });
                host.selecting = true;
            } else if host.selecting {
                if let Some(selection) = host.selection.as_mut() {
                    selection.end = CellPosition { col, row };
                }
                if releasing {
                    host.selecting = false;
                }
            }
            host.force = true;
        }
    }
}
unsafe extern "C" fn wheel(_: Id, _: Sel, event: Id) {
    let pointer = host_ptr();
    if pointer.is_null() {
        return;
    }
    let delta = send!(event, c"scrollingDeltaY", f64);
    let precise = send!(event, c"hasPreciseScrollingDeltas", bool);
    let mods = modifiers(event);
    unsafe {
        let host = &mut *pointer;
        host.scroll_accumulator += if precise {
            delta / host.renderer.cell_height
        } else {
            delta
        };
        let lines = host.scroll_accumulator as i32;
        host.scroll_accumulator -= lines as f64;
        if lines == 0 {
            return;
        }
        if host.core.terminal.modes().mouse_tracking != 0
            && !mods.contains(Modifiers::SHIFT)
            && host.history == 0
        {
            let position = host.position(event);
            for _ in 0..lines.unsigned_abs().min(64) {
                host.send_mouse(
                    MouseEvent::Scroll {
                        up: lines > 0,
                        col: position.col.min(host.core.terminal.cols() - 1),
                        row: position.row.min(host.core.terminal.rows() - 1),
                    },
                    mods,
                );
            }
        } else {
            host.history = if lines > 0 {
                host.history
                    .saturating_add(lines as usize * 3)
                    .min(host.core.terminal.history_len())
            } else {
                host.history
                    .saturating_sub(lines.unsigned_abs() as usize * 3)
            };
            host.selection = None;
            host.force = true;
        }
    }
}

fn add_method(
    class: Id,
    name: &CStr,
    function: *const (),
    encoding: &CStr,
) -> Result<(), &'static str> {
    if unsafe { class_addMethod(class, selector(name), function.cast(), encoding.as_ptr()) } {
        Ok(())
    } else {
        Err("cannot register native view methods")
    }
}
fn register_classes() -> Result<(Id, Id), &'static str> {
    let view = unsafe { objc_allocateClassPair(class(c"NSView"), c"ZttyTerminalView".as_ptr(), 0) };
    let delegate =
        unsafe { objc_allocateClassPair(class(c"NSObject"), c"ZttyWindowDelegate".as_ptr(), 0) };
    if view.is_null() || delegate.is_null() {
        return Err("cannot register AppKit classes");
    }
    let protocol = unsafe { objc_getProtocol(c"NSTextInputClient".as_ptr()) };
    if protocol.is_null() || !unsafe { class_addProtocol(view, protocol) } {
        return Err("NSTextInputClient is unavailable");
    }
    add_method(view, c"acceptsFirstResponder", yes as *const (), c"c@:")?;
    add_method(view, c"acceptsFirstMouse:", yes_event as *const (), c"c@:@")?;
    add_method(view, c"isOpaque", no as *const (), c"c@:")?;
    add_method(
        view,
        c"drawRect:",
        draw_rect as *const (),
        c"v@:{CGRect={CGPoint=dd}{CGSize=dd}}",
    )?;
    add_method(view, c"keyDown:", key_down as *const (), c"v@:@")?;
    add_method(view, c"keyUp:", key_up as *const (), c"v@:@")?;
    add_method(view, c"flagsChanged:", flags_changed as *const (), c"v@:@")?;
    for name in [
        c"mouseDown:",
        c"mouseUp:",
        c"mouseDragged:",
        c"mouseMoved:",
        c"rightMouseDown:",
        c"rightMouseUp:",
        c"rightMouseDragged:",
        c"otherMouseDown:",
        c"otherMouseUp:",
        c"otherMouseDragged:",
    ] {
        add_method(view, name, mouse as *const (), c"v@:@")?;
    }
    add_method(view, c"scrollWheel:", wheel as *const (), c"v@:@")?;
    add_method(
        view,
        c"insertText:replacementRange:",
        insert_text as *const (),
        c"v@:@{_NSRange=QQ}",
    )?;
    add_method(
        view,
        c"setMarkedText:selectedRange:replacementRange:",
        set_marked as *const (),
        c"v@:@{_NSRange=QQ}{_NSRange=QQ}",
    )?;
    add_method(view, c"unmarkText", unmark as *const (), c"v@:")?;
    add_method(view, c"hasMarkedText", has_marked as *const (), c"c@:")?;
    add_method(
        view,
        c"markedRange",
        marked_range as *const (),
        c"{_NSRange=QQ}@:",
    )?;
    add_method(
        view,
        c"selectedRange",
        selected_range as *const (),
        c"{_NSRange=QQ}@:",
    )?;
    add_method(
        view,
        c"validAttributesForMarkedText",
        valid_attributes as *const (),
        c"@@:",
    )?;
    add_method(
        view,
        c"attributedSubstringForProposedRange:actualRange:",
        attributed_substring as *const (),
        c"@@:{_NSRange=QQ}^{_NSRange=QQ}",
    )?;
    add_method(
        view,
        c"characterIndexForPoint:",
        character_index as *const (),
        c"Q@:{CGPoint=dd}",
    )?;
    add_method(
        view,
        c"firstRectForCharacterRange:actualRange:",
        first_rect as *const (),
        c"{CGRect={CGPoint=dd}{CGSize=dd}}@:{_NSRange=QQ}^{_NSRange=QQ}",
    )?;
    add_method(view, c"doCommandBySelector:", command as *const (), c"v@::")?;
    add_method(
        delegate,
        c"windowShouldClose:",
        window_close as *const (),
        c"c@:@",
    )?;
    add_method(
        delegate,
        c"windowDidResize:",
        window_resize as *const (),
        c"v@:@",
    )?;
    add_method(
        delegate,
        c"windowDidChangeBackingProperties:",
        window_resize as *const (),
        c"v@:@",
    )?;
    add_method(
        delegate,
        c"windowDidBecomeKey:",
        window_focus as *const (),
        c"v@:@",
    )?;
    add_method(
        delegate,
        c"windowDidResignKey:",
        window_focus as *const (),
        c"v@:@",
    )?;
    add_method(delegate, c"quit:", quit as *const (), c"v@:@")?;
    unsafe {
        objc_registerClassPair(view);
        objc_registerClassPair(delegate);
    }
    Ok((view, delegate))
}
fn menu(app: Id, target: Id) {
    let title = ns_string("ztty");
    let menubar = send!(send!(class(c"NSMenu"), c"alloc", Id), c"initWithTitle:", Id, Id => title);
    let application = send!(class(c"NSMenuItem"), c"new", Id);
    let submenu = send!(send!(class(c"NSMenu"), c"alloc", Id), c"initWithTitle:", Id, Id => title);
    let quit_title = ns_string("Quit ztty");
    let equivalent = ns_string("q");
    let quit_item = send!(send!(class(c"NSMenuItem"), c"alloc", Id), c"initWithTitle:action:keyEquivalent:", Id, Id => quit_title, Sel => selector(c"quit:"), Id => equivalent);
    send!(quit_item, c"setTarget:", (), Id => target);
    send!(submenu, c"addItem:", (), Id => quit_item);
    send!(application, c"setSubmenu:", (), Id => submenu);
    send!(menubar, c"addItem:", (), Id => application);
    send!(app, c"setMainMenu:", (), Id => menubar);
    for value in [
        quit_item,
        equivalent,
        quit_title,
        submenu,
        application,
        menubar,
        title,
    ] {
        release(value);
    }
}

pub fn run(cfg: Config) -> Result<i32, &'static str> {
    let helper = zpty::current_executable().map_err(|_| "cannot resolve the Darwin PTY helper")?;
    let (program, environment) = os::child_setup(&cfg.command[0])?;
    let arguments: Vec<&CStr> = cfg.command.iter().map(CString::as_c_str).collect();
    let environment: Vec<&CStr> = environment.iter().map(CString::as_c_str).collect();
    let pty = zpty::Pty::spawn_with_helper(
        &helper,
        &program,
        &arguments,
        &environment,
        zpty::Size::new(cfg.cols as u16, cfg.rows as u16),
    )
    .map_err(|_| "cannot create a Darwin PTY")?;
    let outer_pool = send!(class(c"NSAutoreleasePool"), c"new", Id);
    let result = run_app(cfg, pty);
    release(outer_pool);
    result
}
fn run_app(cfg: Config, pty: zpty::Pty) -> Result<i32, &'static str> {
    let renderer = Renderer::new(&cfg.appearance)?;
    let (view_class, delegate_class) = register_classes()?;
    let app = send!(class(c"NSApplication"), c"sharedApplication", Id);
    if app.is_null() {
        return Err("cannot initialize AppKit");
    }
    send!(app, c"setActivationPolicy:", bool, isize => 0);
    let bounds = Rect::new(
        0.0,
        0.0,
        cfg.cols as f64 * renderer.cell_width + 2.0 * cfg.appearance.padding as f64,
        cfg.rows as f64 * renderer.cell_height + 2.0 * cfg.appearance.padding as f64,
    );
    let window = send!(send!(class(c"NSWindow"), c"alloc", Id), c"initWithContentRect:styleMask:backing:defer:", Id, Rect => bounds, usize => 15, usize => 2, bool => false);
    if window.is_null() {
        return Err("cannot create the AppKit window");
    }
    send!(window, c"setReleasedWhenClosed:", (), bool => false);
    send!(window, c"setOpaque:", (), bool => false);
    send!(window, c"setBackgroundColor:", (), Id => send!(class(c"NSColor"), c"clearColor", Id));
    let title = ns_string("ztty");
    send!(window, c"setTitle:", (), Id => title);
    release(title);
    send!(window, c"setContentMinSize:", (), Size => Size { width: renderer.cell_width * 2.0 + 2.0 * cfg.appearance.padding as f64, height: renderer.cell_height * 2.0 + 2.0 * cfg.appearance.padding as f64 });
    let view = send!(send!(view_class, c"alloc", Id), c"initWithFrame:", Id, Rect => bounds);
    let delegate = send!(delegate_class, c"new", Id);
    if view.is_null() || delegate.is_null() {
        release(view);
        release(delegate);
        release(window);
        return Err("cannot create native view");
    }
    send!(view, c"setAutoresizingMask:", (), usize => 18);
    if cfg.appearance.blur {
        let effect = send!(send!(class(c"NSVisualEffectView"), c"alloc", Id), c"initWithFrame:", Id, Rect => bounds);
        if !effect.is_null() {
            send!(effect, c"setBlendingMode:", (), isize => 0);
            send!(effect, c"setMaterial:", (), isize => 21);
            send!(effect, c"setState:", (), isize => 1);
            send!(effect, c"addSubview:", (), Id => view);
            send!(window, c"setContentView:", (), Id => effect);
            release(effect);
        } else {
            send!(window, c"setContentView:", (), Id => view);
        }
    } else {
        send!(window, c"setContentView:", (), Id => view);
    }
    send!(window, c"setDelegate:", (), Id => delegate);
    send!(window, c"setAcceptsMouseMovedEvents:", (), bool => true);
    send!(window, c"makeFirstResponder:", bool, Id => view);
    menu(app, delegate);
    let terminal = Terminal::with_theme(cfg.cols, cfg.rows, cfg.history, cfg.appearance.theme);
    let mut host = Box::new(Host {
        app,
        window,
        view,
        fd_source: NIL,
        core: SessionCore::new(pty, terminal, String::from("local"), MAX_INPUT),
        renderer,
        appearance: cfg.appearance,
        running: true,
        force: true,
        wake_posted: false,
        history: 0,
        selection: None,
        selecting: false,
        mouse_button: None,
        swallowed_buttons: [false; 3],
        read_buffer: vec![0; 65536],
        encoded: Vec::with_capacity(256),
        marked: String::new(),
        marked_selection: Range::default(),
        pending_key: None,
        composing: false,
        intercepted: [false; 128],
        held: [None; 128],
        scroll_accumulator: 0.0,
        focused: true,
        blink_visible: true,
        blink_at: os::milliseconds() + 500,
        sync_since: None,
        last_cursor: (0, 0),
        last_revision: 0,
        title: String::new(),
        scale: 1.0,
    });
    let pointer: *mut Host = &mut *host;
    ACTIVE.store(pointer, Ordering::Relaxed);
    let context = FdContext {
        version: 0,
        info: pointer.cast(),
        retain: None,
        release: None,
        description: None,
    };
    let fd = unsafe { CFFileDescriptorCreate(NIL, host.core.pty.fd(), false, fd_ready, &context) };
    let source = if fd.is_null() {
        NIL
    } else {
        unsafe { CFFileDescriptorCreateRunLoopSource(NIL, fd, 0) }
    };
    if source.is_null() {
        cf_release(fd);
        ACTIVE.store(ptr::null_mut(), Ordering::Relaxed);
        send!(window, c"setDelegate:", (), Id => NIL);
        release(delegate);
        release(view);
        release(window);
        return Err("cannot register PTY with the AppKit run loop");
    }
    host.fd_source = fd;
    unsafe {
        CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
        CFFileDescriptorEnableCallBacks(fd, 1);
    }
    send!(window, c"center", ());
    send!(window, c"makeKeyAndOrderFront:", (), Id => NIL);
    send!(app, c"finishLaunching", ());
    send!(app, c"activateIgnoringOtherApps:", (), bool => true);
    host.resize();
    host.read_ready();
    while host.running {
        let pool = send!(class(c"NSAutoreleasePool"), c"new", Id);
        host.tick();
        // This can re-enter drawRect. No &mut Host borrow is kept across it.
        send!(view, c"displayIfNeeded", ());
        let now = os::milliseconds();
        let blink = host.focused
            && host.core.terminal.modes().cursor_visible
            && host
                .core
                .terminal
                .modes()
                .cursor_blink
                .unwrap_or(host.appearance.cursor_blink);
        let mut deadline = if blink {
            host.blink_at.max(now + 1)
        } else {
            now + 1000
        };
        if let Some(start) = host.sync_since {
            deadline = deadline.min(start.saturating_add(150));
        }
        // The deadline drives cursor/sync state only. CFFileDescriptor wakes
        // nextEvent immediately for PTY output and bounded queued input.
        let interval = deadline.saturating_sub(now).min(1000) as f64 / 1000.0;
        let date = send!(class(c"NSDate"), c"dateWithTimeIntervalSinceNow:", Id, f64 => interval);
        let event = send!(app, c"nextEventMatchingMask:untilDate:inMode:dequeue:", Id, usize => usize::MAX, Id => date, Id => unsafe { kCFRunLoopDefaultMode }, bool => true);
        host.wake_posted = false;
        if !event.is_null() {
            send!(app, c"sendEvent:", (), Id => event);
        }
        send!(app, c"updateWindows", ());
        release(pool);
    }
    unsafe {
        CFFileDescriptorDisableCallBacks(fd, 3);
        CFRunLoopRemoveSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
        CFFileDescriptorInvalidate(fd);
    }
    host.fd_source = NIL;
    cf_release(source);
    cf_release(fd);
    ACTIVE.store(ptr::null_mut(), Ordering::Relaxed);
    send!(window, c"setDelegate:", (), Id => NIL);
    send!(window, c"orderOut:", (), Id => NIL);
    send!(window, c"close", ());
    release(delegate);
    release(view);
    release(window);
    let _ = host.core.pty.hangup();
    let _ = host.core.pty.wait_timeout(100);
    Ok(host
        .core
        .pty
        .status()
        .map(|s| s.code().unwrap_or(128 + s.signal().unwrap_or(0)))
        .unwrap_or(0))
}
