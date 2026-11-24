//! Shared display lifetime and Xlib error handling.
use super::*;

// All Xlib access belongs to the single-threaded session host.
static mut ERROR_HANDLER_INSTALLED: bool = false;
static mut PREVIOUS_ERROR_HANDLER: Option<ErrorHandler> = None;

unsafe extern "C" fn handle_error(display: *mut Display, event: *mut ErrorEvent) -> c_int {
    unsafe {
        // Another X client can destroy a window after Expose was queued.
        // XCopyArea then reports BadDrawable before DestroyNotify is handled.
        // Leave the normal event path to retire that view, without terminating
        // every other session hosted by this process.
        if (*event).error_code == 9 && (*event).request_code == 62 {
            return 0;
        }
        if let Some(previous) = PREVIOUS_ERROR_HANDLER {
            return previous(display, event);
        }
        crate::os::print(2, b"ztty: unhandled X11 protocol error\n");
        crate::os::_exit(1);
    }
}
impl Connection {
    pub fn new_for_display(name: Option<&CStr>) -> Result<Rc<Self>, &'static str> {
        unsafe {
            crate::os::setlocale(0, c"".as_ptr());
            XSetLocaleModifiers(c"".as_ptr());
            if !ERROR_HANDLER_INSTALLED {
                PREVIOUS_ERROR_HANDLER = XSetErrorHandler(Some(handle_error));
                ERROR_HANDLER_INSTALLED = true;
            }
            let display = XOpenDisplay(name.map_or(core::ptr::null(), CStr::as_ptr));
            if display.is_null() {
                return Err("cannot open DISPLAY; ztty needs X11 or XWayland");
            }
            let screen = XDefaultScreen(display);
            let mut supported = 0;
            XkbSetDetectableAutoRepeat(display, 1, &mut supported);
            let fd = XConnectionNumber(display);
            crate::os::fcntl(fd, 2, 1);
            let connection = Rc::new(Self {
                display,
                fd,
                screen,
                im: core::cell::Cell::new(core::ptr::null_mut()),
                im_registered: core::cell::Cell::new(false),
                im_opening: core::cell::Cell::new(false),
                contexts: RefCell::new(Vec::new()),
                fonts: RefCell::new(Vec::new()),
            });
            if !connection.open_im() {
                connection.watch_im();
            }
            Ok(connection)
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        unsafe {
            self.unwatch_im();
            let im = self.im.replace(core::ptr::null_mut());
            if !im.is_null() {
                XCloseIM(im);
            }
            XCloseDisplay(self.display);
        }
    }
}
