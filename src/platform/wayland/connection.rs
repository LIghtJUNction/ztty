use super::*;

impl Native {
    pub(super) fn ptr(&self) -> *mut c_void {
        self.ptr.as_ptr()
    }
    pub(super) fn id(&self) -> Xid {
        self.ptr() as usize as Xid
    }
    pub(super) fn ready(&self) -> bool {
        unsafe { ztty_wl_ready(self.ptr()) != 0 }
    }
    pub(super) fn pump(&self) {
        if self.closed.get() {
            return;
        }
        if unsafe { ztty_wl_pump(self.ptr(), -1, 0, 0) } < 0
            || (os::milliseconds().saturating_sub(self.created) > 5000
                && unsafe { ztty_wl_deadline(self.ptr()) } == 1)
        {
            self.failed.set(true);
        }
    }
    pub(super) fn pending(&self) -> bool {
        self.pump();
        (self.failed.get() && !self.closed.get())
            || self.scroll.get() != 0
            || unsafe { ztty_wl_pending(self.ptr()) != 0 }
    }
    pub(super) fn event(&self, display: *mut x11::Display) -> Option<x11::Event> {
        self.pump();
        let mut out = x11::Event { pad: [0; 24] };
        if self.closed.get() {
            return None;
        }
        let mut e = Event::default();
        if self.failed.get() {
            e.kind = 6;
        } else if self.scroll.get() != 0 {
            e = self.last.get();
            e.kind = 5;
            let remaining = self.scroll.get();
            e.state = remaining.signum() as u32;
            self.scroll.set(remaining - remaining.signum());
        } else if unsafe { ztty_wl_next(self.ptr(), &mut e) } == 0 {
            return None;
        }
        self.last.set(e);
        let base = x11::KeyEvent {
            kind: 0,
            serial: 0,
            send_event: 0,
            display,
            window: self.id(),
            root: 0,
            subwindow: 0,
            time: os::milliseconds() as c_ulong,
            x: e.x,
            y: e.y,
            x_root: e.x,
            y_root: e.y,
            state: x_state(e.modifiers) | self.buttons.get(),
            keycode: e.keycode,
            same_screen: 1,
        };
        out.key = base;
        match e.kind {
            1 => {
                out.configure = x11::ConfigureEvent {
                    kind: x11::CONFIGURE_NOTIFY,
                    serial: 0,
                    send_event: 0,
                    display,
                    event: self.id(),
                    window: self.id(),
                    x: 0,
                    y: 0,
                    width: e.x,
                    height: e.y,
                    border_width: 0,
                    above: 0,
                    override_redirect: 0,
                };
            }
            2 => {
                out.key.kind = if e.state == 0 {
                    x11::KEY_RELEASE
                } else {
                    x11::KEY_PRESS
                };
            }
            3 => {
                self.focused.set(e.state != 0);
                {
                    out.key.kind = if e.state != 0 {
                        x11::FOCUS_IN
                    } else {
                        x11::FOCUS_OUT
                    };
                }
            }
            4 | 5 => {
                let button = if e.kind == 5 {
                    if (e.state as i32) > 0 { 5 } else { 4 }
                } else {
                    match e.symbol {
                        272 => 1,
                        273 => 3,
                        274 => 2,
                        275..=278 => e.symbol - 267,
                        _ => return self.event(display),
                    }
                };
                if e.kind == 5 && (e.state as i32).unsigned_abs() > 1 {
                    self.scroll.set(e.state as i32 - (e.state as i32).signum());
                }
                {
                    out.button.kind = if e.kind == 5 || e.state != 0 {
                        x11::BUTTON_PRESS
                    } else {
                        x11::BUTTON_RELEASE
                    };
                    out.button.button = button;
                }
                if e.kind == 4 && button <= 5 {
                    let bit = 1 << (7 + button);
                    self.buttons.set(if e.state != 0 {
                        self.buttons.get() | bit
                    } else {
                        self.buttons.get() & !bit
                    });
                }
            }
            6 => {
                self.closed.set(true);
                self.focused.set(false);
                {
                    out.configure.kind = x11::DESTROY_NOTIFY;
                    out.configure.window = self.id();
                }
            }
            7 => {
                out.motion.kind = x11::MOTION_NOTIFY;
            }
            8 => {
                out.selection = x11::SelectionEvent {
                    kind: x11::SELECTION_NOTIFY,
                    serial: e.state as c_ulong,
                    send_event: 0,
                    display,
                    requestor: self.id(),
                    selection: 1,
                    target: 1,
                    property: 1,
                    time: 0,
                };
            }
            9 => {
                out.key.kind = x11::KEY_PRESS;
                out.key.state = 0;
            }
            10 => {
                out.key.kind = x11::EXPOSE;
            }
            _ => return self.event(display),
        }
        Some(out)
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            ztty_wl_destroy(self.ptr());
        }
    }
}
fn x_state(bits: u32) -> u32 {
    (bits & 1)
        | ((bits & 2) << 2)
        | (bits & 4)
        | ((bits & 8) << 3)
        | ((bits & 64) >> 5)
        | ((bits & 128) >> 3)
}
impl Connection {
    pub fn new(env: &[CString]) -> Result<Self, &'static str> {
        let get = |prefix: &[u8]| env.iter().find_map(|s| s.to_bytes().strip_prefix(prefix));
        let name = get(b"WAYLAND_DISPLAY=").unwrap_or(b"wayland-0");
        let mut path = Vec::new();
        if !name.starts_with(b"/") {
            let runtime = get(b"XDG_RUNTIME_DIR=")
                .filter(|p| p.starts_with(b"/"))
                .ok_or("Wayland needs an absolute XDG_RUNTIME_DIR")?;
            path.extend_from_slice(runtime);
            path.push(b'/');
        }
        path.extend_from_slice(name);
        if path.len() > 4095 {
            return Err("Wayland display path is too long");
        }
        let locale = get(b"LC_ALL=")
            .or_else(|| get(b"LC_CTYPE="))
            .or_else(|| get(b"LANG="))
            .unwrap_or(b"C.UTF-8");
        Ok(Self {
            endpoint: CString::new(path).map_err(|_| "invalid Wayland path")?,
            locale: CString::new(locale).map_err(|_| "invalid locale")?,
            windows: RefCell::new(Vec::new()),
        })
    }
    pub fn pending(&self) -> bool {
        self.windows
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .any(|w| w.pending())
    }
    pub fn next_event(&self, display: *mut x11::Display) -> Option<x11::Event> {
        self.windows
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .find_map(|w| w.event(display))
    }
    pub fn input_focus(&self) -> Xid {
        self.windows
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .find(|w| w.focused.get())
            .map_or(0, |w| w.id())
    }
    pub fn descriptors(&self, out: &mut Vec<os::PollFd>) {
        for w in self
            .windows
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|w| !w.closed.get())
        {
            out.push(os::PollFd {
                fd: unsafe { ztty_wl_fd(w.ptr()) },
                events: unsafe { ztty_wl_events(w.ptr()) } as _,
                revents: 0,
            });
            let mut extra: [os::PollFd; 17] = core::array::from_fn(|_| os::PollFd {
                fd: -1,
                events: 0,
                revents: 0,
            });
            let n = unsafe { ztty_wl_service_fds(w.ptr(), extra.as_mut_ptr(), extra.len()) };
            out.extend(extra.into_iter().take(n.min(17)));
        }
    }
    pub fn deadline(&self) -> Option<u64> {
        self.windows
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|w| !w.closed.get())
            .filter_map(|w| match unsafe { ztty_wl_deadline(w.ptr()) } {
                0 => None,
                1 => Some(w.created + 5000),
                n => Some(n),
            })
            .min()
    }
}
