//! XIM lifecycle, focus, and XKB key lookup.
use super::*;

/// Focus events can remain queued after reparenting or an explicit focus
/// change. Consult the current server owner before acting on such an event.
pub fn input_focus(display: *mut Display) -> Xid {
    let mut window = 0;
    let mut revert = 0;
    unsafe {
        XGetInputFocus(display, &mut window, &mut revert);
    }
    window
}

impl Connection {
    // Xlib keeps these callback pointers until unregister/close. Connection is
    // always Rc-allocated, so its address stays stable even when windows move.
    fn callback_data(&self) -> *mut c_void {
        (self as *const Self).cast_mut().cast()
    }

    pub(super) fn watch_im(&self) {
        if self.im_registered.replace(true) {
            return;
        }
        let registered = unsafe {
            XRegisterIMInstantiateCallback(
                self.display,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                im_available,
                self.callback_data(),
            )
        };
        if registered == 0 {
            self.im_registered.set(false);
        }
    }

    pub(super) fn unwatch_im(&self) {
        if self.im_registered.replace(false) {
            unsafe {
                XUnregisterIMInstantiateCallback(
                    self.display,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    im_available,
                    self.callback_data(),
                );
            }
        }
    }

    pub(super) fn open_im(&self) -> bool {
        if !self.im.get().is_null() {
            return true;
        }
        if self.im_opening.replace(true) {
            return false;
        }
        unsafe {
            let im = XOpenIM(
                self.display,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            );
            if !im.is_null() {
                self.im.set(im);
                let callback = ImCallback {
                    data: self.callback_data(),
                    callback: im_destroyed,
                };
                if !XSetIMValues(
                    im,
                    c"destroyCallback".as_ptr(),
                    &callback as *const ImCallback,
                    core::ptr::null::<c_char>(),
                )
                .is_null()
                {
                    self.im.set(core::ptr::null_mut());
                    XCloseIM(im);
                }
            }
        }
        if !self.im.get().is_null() {
            self.unwatch_im();
            // Xlib can call the destroy callback while creating an IC. Do not
            // hold a RefCell borrow across any Xlib call.
            let contexts = self.contexts.borrow().clone();
            for context in contexts.iter().filter_map(Weak::upgrade) {
                context.create();
            }
        }
        self.im_opening.set(false);
        !self.im.get().is_null()
    }

    pub(super) fn input_context(self: &Rc<Self>, window: Xid, events: c_long) -> Rc<InputContext> {
        let context = Rc::new(InputContext {
            connection: Rc::clone(self),
            window,
            events,
            handle: core::cell::Cell::new(core::ptr::null_mut()),
            focused: core::cell::Cell::new(false),
        });
        {
            let mut contexts = self.contexts.borrow_mut();
            contexts.retain(|entry| entry.strong_count() != 0);
            contexts.push(Rc::downgrade(&context));
        }
        context.create();
        context
    }
}

unsafe extern "C" fn im_available(_display: *mut Display, data: *mut c_void, _call: *mut c_void) {
    let connection = unsafe { &*data.cast::<Connection>() };
    connection.open_im();
}

unsafe extern "C" fn im_destroyed(im: *mut c_void, data: *mut c_void, _call: *mut c_void) {
    let connection = unsafe { &*data.cast::<Connection>() };
    if connection.im.get() != im {
        return;
    }
    connection.im.set(core::ptr::null_mut());
    // Xlib destroys this IM and all its ICs after this callback. Calling
    // XDestroyIC/XCloseIM here, or retaining their pointers, is invalid.
    for context in connection
        .contexts
        .borrow()
        .iter()
        .filter_map(Weak::upgrade)
    {
        context.handle.set(core::ptr::null_mut());
    }
    connection.watch_im();
}

impl InputContext {
    fn create(&self) {
        let im = self.connection.im.get();
        if im.is_null() || !self.handle.get().is_null() {
            return;
        }
        unsafe {
            let ic = XCreateIC(
                im,
                c"inputStyle".as_ptr(),
                0x408 as c_ulong,
                c"clientWindow".as_ptr(),
                self.window,
                c"focusWindow".as_ptr(),
                self.window,
                core::ptr::null::<c_char>(),
            );
            if ic.is_null() || self.connection.im.get() != im {
                return;
            }
            self.handle.set(ic);
            let mut filter_events: c_long = 0;
            if XGetICValues(
                ic,
                c"filterEvents".as_ptr(),
                &mut filter_events as *mut c_long,
                core::ptr::null::<c_char>(),
            )
            .is_null()
            {
                XSelectInput(
                    self.connection.display,
                    self.window,
                    self.events | filter_events,
                );
            }
            if self.focused.get() && self.handle.get() == ic {
                XSetICFocus(ic);
            }
        }
    }

    pub(super) fn focus(&self, focused: bool) {
        self.focused.set(focused);
        let ic = self.handle.get();
        if !ic.is_null() {
            unsafe {
                if focused {
                    XSetICFocus(ic);
                } else {
                    XUnsetICFocus(ic);
                }
            }
        }
    }

    pub(super) fn close(&self) {
        self.focused.set(false);
        let ic = self.handle.replace(core::ptr::null_mut());
        if !ic.is_null() {
            unsafe { XDestroyIC(ic) };
        }
    }
}

impl Drop for InputContext {
    fn drop(&mut self) {
        self.close();
    }
}

impl Window {
    pub fn focus(&mut self, focused: bool) {
        self.state.focused = focused;
        self.ic.focus(focused);
    }
    /// Keep character identity unshifted, but let XKB choose the keypad layer.
    pub fn input_symbol(&self, event: &mut KeyEvent) -> c_ulong {
        let symbol = self.key_symbol(event, false);
        if (0xff80..=0xffbd).contains(&symbol) {
            let state = event.state;
            self.key_symbol_with_state(event, state)
        } else {
            symbol
        }
    }
    /// Resolve the current keyboard group, preserving AltGr/layout selection.
    pub fn key_symbol(&self, event: &mut KeyEvent, shifted: bool) -> c_ulong {
        let state = (event.state & !3) | u32::from(shifted);
        self.key_symbol_with_state(event, state)
    }
    fn key_symbol_with_state(&self, event: &mut KeyEvent, state: u32) -> c_ulong {
        let mut symbol = 0;
        let mut consumed = 0;
        if unsafe {
            XkbLookupKeySym(
                self.state.display,
                event.keycode as u8,
                state,
                &mut consumed,
                &mut symbol,
            )
        } != 0
        {
            symbol
        } else {
            unsafe { XLookupKeysym(event, i32::from(state & 1 != 0)) }
        }
    }
    pub fn lookup(&self, event: &mut KeyEvent, buffer: &mut Vec<u8>) -> Lookup {
        let mut symbol = 0;
        let mut status = 4;
        let ic = self.ic.handle.get();
        let mut count = unsafe {
            if ic.is_null() {
                XLookupString(
                    event,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as i32,
                    &mut symbol,
                    core::ptr::null_mut(),
                )
            } else {
                Xutf8LookupString(
                    ic,
                    event,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as i32,
                    &mut symbol,
                    &mut status,
                )
            }
        };
        if status == -1
            && count > 0
            && count <= 65536
            && !ic.is_null()
            && self.ic.handle.get() == ic
        {
            buffer.resize(count as usize + 1, 0);
            count = unsafe {
                Xutf8LookupString(
                    ic,
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
}
