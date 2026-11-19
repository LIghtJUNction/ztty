use alloc::rc::Rc;
use core::{
    ffi::{c_int, c_uint, c_void},
    ptr,
};

type Handle = *mut c_void;
const NONE: c_int = 0x3038;

unsafe extern "C" {
    fn eglGetPlatformDisplay(platform: c_uint, display: Handle, attributes: *const isize)
    -> Handle;
    fn eglInitialize(display: Handle, major: *mut c_int, minor: *mut c_int) -> c_uint;
    fn eglTerminate(display: Handle) -> c_uint;
    fn eglBindAPI(api: c_uint) -> c_uint;
    fn eglChooseConfig(
        display: Handle,
        attributes: *const c_int,
        configs: *mut Handle,
        size: c_int,
        count: *mut c_int,
    ) -> c_uint;
    fn eglGetConfigAttrib(
        display: Handle,
        config: Handle,
        attribute: c_int,
        value: *mut c_int,
    ) -> c_uint;
    fn eglCreateContext(
        display: Handle,
        config: Handle,
        share: Handle,
        attributes: *const c_int,
    ) -> Handle;
    fn eglDestroyContext(display: Handle, context: Handle) -> c_uint;
    fn eglCreateWindowSurface(
        display: Handle,
        config: Handle,
        window: usize,
        attributes: *const c_int,
    ) -> Handle;
    fn eglCreatePbufferSurface(display: Handle, config: Handle, attributes: *const c_int)
    -> Handle;
    fn eglDestroySurface(display: Handle, surface: Handle) -> c_uint;
    fn eglMakeCurrent(display: Handle, draw: Handle, read: Handle, context: Handle) -> c_uint;
    fn eglGetCurrentContext() -> Handle;
    fn eglSwapBuffers(display: Handle, surface: Handle) -> c_uint;
    fn eglSwapInterval(display: Handle, interval: c_int) -> c_uint;
}

/// The owning platform must keep the native display alive until this value and
/// every renderer using it have been dropped. One value is shared per connection.
pub struct Display {
    handle: Handle,
}

impl Display {
    /// # Safety
    /// `display` must be a live Xlib Display and outlive all clones of the result.
    pub unsafe fn x11(display: Handle) -> Result<Rc<Self>, &'static str> {
        unsafe { Self::open(0x31d5, display) }
    }

    /// # Safety
    /// `display` must be a live wl_display and outlive all clones of the result.
    pub unsafe fn wayland(display: Handle) -> Result<Rc<Self>, &'static str> {
        unsafe { Self::open(0x31d8, display) }
    }

    pub fn headless() -> Result<Rc<Self>, &'static str> {
        unsafe { Self::open(0x31dd, ptr::null_mut()) }
    }

    unsafe fn open(platform: u32, native: Handle) -> Result<Rc<Self>, &'static str> {
        let display = unsafe { eglGetPlatformDisplay(platform, native, [NONE as isize].as_ptr()) };
        if display.is_null() {
            return Err("EGL platform is unavailable");
        }
        if unsafe { eglInitialize(display, ptr::null_mut(), ptr::null_mut()) } == 0 {
            return Err("cannot initialize the EGL display");
        }
        if unsafe { eglBindAPI(0x30a0) } == 0 {
            unsafe {
                eglTerminate(display);
            }
            return Err("OpenGL ES is unavailable");
        }
        Ok(Rc::new(Self { handle: display }))
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        unsafe {
            eglTerminate(self.handle);
        }
    }
}

#[derive(Clone, Copy)]
pub enum Target {
    /// X11 window XID and its XVisualID. Must outlive the renderer.
    X11 {
        window: usize,
        visual: i32,
    },
    /// wl_egl_window pointer. Must outlive the renderer.
    Wayland {
        window: usize,
    },
    Offscreen,
}

pub struct Surface {
    display: Rc<Display>,
    config: Handle,
    context: Handle,
    surface: Handle,
    target: Target,
    width: u32,
    height: u32,
}

pub fn dimensions(width: u32, height: u32) -> Result<(), &'static str> {
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || u64::from(width) * u64::from(height) > 16_777_216
    {
        return Err("GPU surface exceeds the 16-megapixel frame budget");
    }
    Ok(())
}

impl Surface {
    pub fn new(
        display: Rc<Display>,
        target: Target,
        width: u32,
        height: u32,
    ) -> Result<Self, &'static str> {
        dimensions(width, height)?;
        let surface_type = if matches!(target, Target::Offscreen) {
            1
        } else {
            4
        };
        let attributes = [
            0x3033,
            surface_type,
            0x3040,
            4,
            0x3024,
            8,
            0x3023,
            8,
            0x3022,
            8,
            0x3021,
            if matches!(target, Target::X11 { .. }) {
                0
            } else {
                8
            },
            NONE,
        ];
        let mut configs = [ptr::null_mut(); 128];
        let mut count = 0;
        if unsafe {
            eglChooseConfig(
                display.handle,
                attributes.as_ptr(),
                configs.as_mut_ptr(),
                configs.len() as i32,
                &mut count,
            )
        } == 0
            || count <= 0
        {
            return Err("no compatible OpenGL ES surface configuration");
        }
        let count = usize::try_from(count).unwrap_or(0).min(configs.len());
        let config = configs[..count]
            .iter()
            .copied()
            .find(|config| {
                if let Target::X11 { visual, .. } = target {
                    let mut found = 0;
                    unsafe {
                        eglGetConfigAttrib(display.handle, *config, 0x302e, &mut found) != 0
                            && found == visual
                    }
                } else {
                    true
                }
            })
            .ok_or("EGL has no configuration for the window visual")?;
        let context = unsafe {
            eglCreateContext(
                display.handle,
                config,
                ptr::null_mut(),
                [0x3098, 2, NONE].as_ptr(),
            )
        };
        if context.is_null() {
            return Err("cannot create the OpenGL ES context");
        }
        let surface = unsafe { Self::create_surface(&display, config, target, width, height) };
        if surface.is_null() {
            unsafe {
                eglDestroyContext(display.handle, context);
            }
            return Err("cannot create the EGL rendering surface");
        }
        let result = Self {
            display,
            config,
            context,
            surface,
            target,
            width,
            height,
        };
        result.activate()?;
        // The broker already coalesces repaint deadlines. A GPU swap must not
        // block PTY reads while waiting for the monitor's vertical retrace.
        unsafe {
            eglSwapInterval(result.display.handle, 0);
        }
        Ok(result)
    }

    unsafe fn create_surface(
        display: &Display,
        config: Handle,
        target: Target,
        width: u32,
        height: u32,
    ) -> Handle {
        match target {
            Target::Offscreen => unsafe {
                eglCreatePbufferSurface(
                    display.handle,
                    config,
                    [0x3057, width as i32, 0x3056, height as i32, NONE].as_ptr(),
                )
            },
            Target::X11 { window, .. } | Target::Wayland { window } => unsafe {
                eglCreateWindowSurface(display.handle, config, window, [NONE].as_ptr())
            },
        }
    }

    pub fn activate(&self) -> Result<(), &'static str> {
        if unsafe {
            eglMakeCurrent(
                self.display.handle,
                self.surface,
                self.surface,
                self.context,
            )
        } == 0
        {
            Err("lost the EGL rendering context")
        } else {
            Ok(())
        }
    }

    pub fn is_current(&self) -> bool {
        unsafe { eglGetCurrentContext() == self.context }
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), &'static str> {
        dimensions(width, height)?;
        if matches!(self.target, Target::Offscreen) && (width, height) != (self.width, self.height)
        {
            let replacement = unsafe {
                Self::create_surface(&self.display, self.config, self.target, width, height)
            };
            if replacement.is_null() {
                return Err("cannot resize the EGL pixel buffer");
            }
            if unsafe {
                eglMakeCurrent(self.display.handle, replacement, replacement, self.context)
            } == 0
            {
                unsafe {
                    eglDestroySurface(self.display.handle, replacement);
                }
                return Err("cannot bind the resized EGL pixel buffer");
            }
            unsafe {
                eglDestroySurface(self.display.handle, self.surface);
            }
            self.surface = replacement;
        }
        self.width = width;
        self.height = height;
        Ok(())
    }

    pub fn present(&self) -> Result<(), &'static str> {
        if unsafe { eglSwapBuffers(self.display.handle, self.surface) } == 0 {
            Err("cannot present the GPU frame")
        } else {
            Ok(())
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            eglMakeCurrent(
                self.display.handle,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
            eglDestroySurface(self.display.handle, self.surface);
            eglDestroyContext(self.display.handle, self.context);
        }
    }
}
