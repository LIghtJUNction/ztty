//! Native AppKit frontend. All Objective-C and Core Foundation objects stay on
//! the main thread. PTY creation uses a fresh-exec helper and is safe after
//! AppKit starts worker threads.
//! File-descriptor readiness is integrated with the AppKit run loop; no polling
//! thread or periodic read timer is required.

use self::{
    ffi::*,
    window::{menu, register_classes},
};
use crate::{Config, appearance::Appearance, os};
use alloc::{boxed::Box, collections::BTreeMap, ffi::CString, string::String, vec, vec::Vec};
use core::{
    ffi::{CStr, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use zterm::{
    Terminal,
    input::{Key, KeyEvent, Modifiers},
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

struct Renderer {
    fonts: [Id; 4],
    lines: BTreeMap<String, Id>,
    color_space: Id,
    cell_width: f64,
    cell_height: f64,
    descent: f64,
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
    fn read_ready(&mut self) {
        let mut budget = 1024usize * 1024;
        while budget != 0 {
            match self.core.read_once(&mut self.read_buffer, false) {
                Ok(ReadStep::Data {
                    bytes,
                    history_scrolled,
                    history_after,
                    ..
                }) => {
                    budget = budget.saturating_sub(bytes);
                    if self.history > 0 {
                        self.history = self
                            .history
                            .saturating_add(history_scrolled)
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
        let mut anchor = ztty::selection::ViewAnchor::capture(
            &self.core.terminal,
            self.history,
            self.selection.map(|selection| {
                (
                    (selection.start.col, selection.start.row),
                    (selection.end.col, selection.end.row),
                )
            }),
        );
        if self
            .core
            .resize_with_cell_size_anchored(size, (cw, ch), anchor.anchors_mut())
            .is_err()
        {
            os::print(2, b"ztty: PTY resize failed; retaining the terminal grid\n");
            self.force = true;
            return;
        }
        self.scale = scale;
        let restored = anchor.restore(&self.core.terminal);
        self.history = restored.history;
        self.selection = restored.selection.map(|(start, end)| Selection {
            start: CellPosition {
                col: start.0,
                row: start.1,
            },
            end: CellPosition {
                col: end.0,
                row: end.1,
            },
        });
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
        let visible = !self.core.eof()
            && send!(self.window, c"isVisible", bool)
            && !send!(self.window, c"isMiniaturized", bool);
        self.core
            .terminal
            .advance_graphics(now, self.history, visible);
        osc52::process(&mut self.core, visible);
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
    // Keep the backing store in the same space as terminal RGB and image
    // pixels. A display-profile backing store adds an avoidable 8-bit color
    // conversion before WindowServer composites or captures the window.
    send!(window, c"setColorSpace:", (), Id => send!(class(c"NSColorSpace"), c"sRGBColorSpace", Id));
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
        if let Some(animation) = host.core.terminal.graphics().animation_deadline() {
            deadline = deadline.min(animation);
        }
        // The deadline drives visual state only. CFFileDescriptor wakes
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

#[path = "macos/osc52.rs"]
mod osc52;

#[path = "macos/drawing.rs"]
mod drawing;
#[path = "macos/ffi.rs"]
mod ffi;
#[path = "macos/input_events.rs"]
mod input_events;
#[path = "macos/window.rs"]
mod window;
