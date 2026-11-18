//! Defer native resizes while Win32 is dispatching nested window messages.
use core::cell::Cell;

#[derive(Default)]
pub struct Dispatch {
    active: Cell<bool>,
    resize_pending: Cell<bool>,
}

impl Dispatch {
    pub fn with<R>(&self, operation: impl FnOnce() -> R) -> Option<R> {
        if self.active.replace(true) {
            return None;
        }
        struct Guard<'a>(&'a Cell<bool>);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _guard = Guard(&self.active);
        Some(operation())
    }

    /// Coalesce to the current size; minimizing retains the previous grid.
    pub fn resize_event(&self, minimized: bool) {
        self.resize_pending.set(!minimized);
    }

    /// Consume at most once per outer turn. A nested request stays queued.
    pub fn take_resize(&self) -> bool {
        self.resize_pending.replace(false)
    }

    pub fn resize_pending(&self) -> bool {
        self.resize_pending.get()
    }
}
