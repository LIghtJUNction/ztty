//! Preserve resize and paint requests across nested Win32 window messages.
use core::cell::Cell;

#[derive(Default)]
pub struct Dispatch {
    active: Cell<bool>,
    resize_pending: Cell<bool>,
    paint_pending: Cell<bool>,
    deferred_paints: Cell<u64>,
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

    /// Take one resize before a commit; a request during that commit stays queued.
    pub fn take_resize(&self) -> bool {
        self.resize_pending.replace(false)
    }

    pub fn resize_pending(&self) -> bool {
        self.resize_pending.get()
    }

    /// DefWindowProc may validate a paint rejected by the mutable App guard.
    /// Keep a separate obligation to invalidate it after that guard is released.
    pub fn defer_paint(&self) {
        self.paint_pending.set(true);
        self.deferred_paints
            .set(self.deferred_paints.get().saturating_add(1));
    }

    /// Take before calling Win32 so a nested request survives this attempt.
    pub fn take_paint(&self) -> bool {
        self.paint_pending.replace(false)
    }

    /// Failed invalidation is a retry, not another rejected WM_PAINT.
    pub fn retry_paint(&self) {
        self.paint_pending.set(true);
    }

    pub fn paint_pending(&self) -> bool {
        self.paint_pending.get()
    }

    pub fn deferred_paints(&self) -> u64 {
        self.deferred_paints.get()
    }
}
