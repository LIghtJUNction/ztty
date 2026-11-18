//! Aggregate ConPTY readiness without exceeding Win32's 63-handle UI wait limit.
//! Callbacks touch only their owned notification state, never the window/App.
use alloc::{boxed::Box, sync::Arc};
use core::{
    ffi::c_void,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

type Handle = *mut c_void;

pub struct Wake {
    handle: Handle,
}

// Native event operations are synchronized by Windows; Arc keeps the event
// alive until the last outstanding wait callback has been cancelled/joined.
unsafe impl Send for Wake {}
unsafe impl Sync for Wake {}

impl Wake {
    pub fn new() -> Result<Arc<Self>, &'static str> {
        let handle = unsafe { ffi::CreateEventW(ptr::null(), 0, 0, ptr::null()) };
        if handle.is_null() {
            return Err("cannot create the Windows session wake event");
        }
        Ok(Arc::new(Self { handle }))
    }

    pub fn wait(&self, timeout: u32) -> Result<(), &'static str> {
        let result =
            unsafe { ffi::MsgWaitForMultipleObjectsEx(1, &self.handle, timeout, 0x04ff, 4) };
        if result == u32::MAX {
            Err("cannot wait for Windows session events")
        } else {
            Ok(())
        }
    }
}

impl Drop for Wake {
    fn drop(&mut self) {
        unsafe {
            ffi::CloseHandle(self.handle);
        }
    }
}

struct Notification {
    wake: Arc<Wake>,
    fired: AtomicBool,
}

pub struct Wait {
    handle: Handle,
    source: Handle,
    notification: Box<Notification>,
}

unsafe extern "system" fn ready(_: Handle, context: Handle, _: Handle, _: u32) {
    // The Box remains alive until Drop cancels and joins every callback.
    let notification = unsafe { &*context.cast::<Notification>() };
    notification.fired.store(true, Ordering::Release);
    unsafe {
        ffi::SetEvent(notification.wake.handle);
    }
}

impl Wait {
    /// The source handle must remain owned by its PTY until this wait is dropped.
    pub fn new(source: Handle, wake: &Arc<Wake>) -> Result<Self, &'static str> {
        if source.is_null() {
            return Err("cannot register a missing Windows session event");
        }
        let mut notification = Box::new(Notification {
            wake: Arc::clone(wake),
            fired: AtomicBool::new(false),
        });
        let handle = unsafe {
            ffi::CreateThreadpoolWait(
                Some(ready),
                (&mut *notification as *mut Notification).cast(),
                ptr::null(),
            )
        };
        if handle.is_null() {
            return Err("cannot register the Windows session wait");
        }
        unsafe {
            ffi::SetThreadpoolWait(handle, source, ptr::null());
        }
        Ok(Self {
            handle,
            source,
            notification,
        })
    }

    /// Wait callbacks are one-shot. Rearm only after the UI has pumped the PTY,
    /// so a still-signaled manual-reset output event cannot flood callbacks.
    pub fn rearm(&self) {
        if self.notification.fired.swap(false, Ordering::AcqRel) {
            unsafe {
                ffi::SetThreadpoolWait(self.handle, self.source, ptr::null());
            }
        }
    }
}

impl Drop for Wait {
    fn drop(&mut self) {
        unsafe {
            ffi::SetThreadpoolWait(self.handle, ptr::null_mut(), ptr::null());
            ffi::WaitForThreadpoolWaitCallbacks(self.handle, 1);
            ffi::CloseThreadpoolWait(self.handle);
        }
    }
}

#[allow(non_snake_case)]
mod ffi {
    use super::Handle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn CreateEventW(
            attributes: *const core::ffi::c_void,
            manual: i32,
            initial: i32,
            name: *const u16,
        ) -> Handle;
        pub fn SetEvent(event: Handle) -> i32;
        pub fn CloseHandle(handle: Handle) -> i32;
        pub fn CreateThreadpoolWait(
            callback: Option<unsafe extern "system" fn(Handle, Handle, Handle, u32)>,
            context: Handle,
            environment: *const core::ffi::c_void,
        ) -> Handle;
        pub fn SetThreadpoolWait(wait: Handle, source: Handle, timeout: *const core::ffi::c_void);
        pub fn WaitForThreadpoolWaitCallbacks(wait: Handle, cancel: i32);
        pub fn CloseThreadpoolWait(wait: Handle);
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn MsgWaitForMultipleObjectsEx(
            count: u32,
            handles: *const Handle,
            timeout: u32,
            mask: u32,
            flags: u32,
        ) -> u32;
    }
}
