//! X11 GPU driver isolation. Only bounded command bytes cross this channel;
//! PTYs, terminal state, XIM and input ownership stay in the broker.
use crate::os;
use alloc::{collections::BTreeMap, ffi::CString, string::String, vec::Vec};
use core::{
    ffi::{CStr, c_char, c_int, c_void},
    sync::atomic::{AtomicI32, Ordering},
};
use ztty::gpu::{
    self, GridStyle, Rect, Style, TextCell,
    grid::{GridScratch, Painter},
};

const TIMEOUT_MS: u64 = 500;
const STARTUP_MS: u64 = 2000;
const LIMIT: usize = 40 * 1024 * 1024;
const IMAGE_LIMIT: usize = 32 * 1024 * 1024;
const FAILURE: &str = "isolated GPU worker failed or exceeded its deadline";
// Positive entries are live owners, negative entries await nonblocking reap.
static CHILDREN: [AtomicI32; 192] = [const { AtomicI32::new(0) }; 192];
unsafe extern "C" {
    fn socketpair(domain: c_int, kind: c_int, protocol: c_int, fds: *mut c_int) -> c_int;
    fn send(fd: c_int, bytes: *const c_void, count: usize, flags: c_int) -> isize;
    fn dup2(old: c_int, new: c_int) -> c_int;
    fn execve(path: *const c_char, args: *const *const c_char, env: *const *const c_char) -> c_int;
    fn syscall(number: core::ffi::c_long, ...) -> core::ffi::c_long;
    fn prctl(option: c_int, ...) -> c_int;
    fn getppid() -> c_int;
    fn XOpenDisplay(name: *const c_char) -> *mut crate::x11::Display;
    fn XDisplayString(display: *mut crate::x11::Display) -> *const c_char;
    fn XSync(display: *mut crate::x11::Display, discard: c_int) -> c_int;
}

pub fn reap() {
    for slot in &CHILDREN {
        let value = slot.load(Ordering::Relaxed);
        if value < 0 {
            let result = unsafe { os::waitpid(-value, core::ptr::null_mut(), 1) };
            if result > 0 || (result < 0 && os::errno() == 10) {
                slot.store(0, Ordering::Relaxed);
            }
        }
    }
}

// The broker keeps its PID across runtime reload. Carry child ownership across
// exec too, so closed old channels cannot leave unreaped GPU helpers behind.
pub fn checkpoint() -> crate::json::Value {
    crate::json::Value::Array(
        CHILDREN
            .iter()
            .filter_map(|slot| {
                let pid = slot.load(Ordering::Relaxed);
                (pid != 0).then(|| crate::json::Value::from(pid.unsigned_abs() as u64))
            })
            .collect(),
    )
}
pub fn validate_checkpoint(value: Option<&crate::json::Value>) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    let rows = value.as_array().ok_or("invalid GPU worker checkpoint")?;
    if rows.len() > CHILDREN.len()
        || rows
            .iter()
            .any(|v| v.as_u64().is_none_or(|p| p == 0 || p > i32::MAX as u64))
    {
        return Err("invalid GPU worker child identity".into());
    }
    Ok(())
}
pub fn restore_children(value: Option<&crate::json::Value>) {
    if let Some(rows) = value.and_then(crate::json::Value::as_array) {
        for (slot, value) in CHILDREN.iter().zip(rows) {
            let pid = value.as_u64().unwrap() as i32;
            // waitpid(0) establishes that this is still our live unreaped child.
            // An exited child is reaped here; ECHILD never licenses a kill.
            if unsafe { os::waitpid(pid, core::ptr::null_mut(), 1) } == 0 {
                unsafe {
                    os::kill(pid, 9);
                }
                slot.store(-pid, Ordering::Relaxed);
            }
        }
    }
    reap();
}

struct Channel(i32);
impl Drop for Channel {
    fn drop(&mut self) {
        unsafe {
            os::close(self.0);
        }
    }
}
fn transfer(fd: i32, bytes: &mut [u8], write: bool, deadline: u64) -> Result<(), &'static str> {
    let mut offset = 0;
    while offset < bytes.len() {
        if os::milliseconds() >= deadline {
            return Err(FAILURE);
        }
        let count = unsafe {
            if write {
                send(
                    fd,
                    bytes[offset..].as_ptr().cast(),
                    bytes.len() - offset,
                    0x4000,
                )
            } else {
                os::read(
                    fd,
                    bytes[offset..].as_mut_ptr().cast(),
                    bytes.len() - offset,
                )
            }
        };
        if count > 0 {
            offset += count as usize;
            continue;
        }
        if count == 0 {
            return Err(FAILURE);
        }
        match os::errno() {
            4 => continue,
            11 => {
                let mut poll = os::PollFd {
                    fd,
                    events: if write { os::POLLOUT } else { os::POLLIN },
                    revents: 0,
                };
                let left = deadline
                    .saturating_sub(os::milliseconds())
                    .min(i32::MAX as u64) as i32;
                if unsafe { os::poll(&mut poll, 1, left) } < 0 && os::errno() != 4 {
                    return Err(FAILURE);
                }
            }
            _ => return Err(FAILURE),
        }
    }
    Ok(())
}
fn packet(fd: i32, bytes: &mut [u8], deadline: u64) -> Result<(), &'static str> {
    let mut size = (bytes.len() as u32).to_le_bytes();
    transfer(fd, &mut size, true, deadline)?;
    transfer(fd, bytes, true, deadline)
}
fn receive(fd: i32, deadline: u64) -> Result<Vec<u8>, &'static str> {
    let mut size = [0; 4];
    transfer(fd, &mut size, false, deadline)?;
    let size = u32::from_le_bytes(size) as usize;
    if size > LIMIT {
        return Err(FAILURE);
    }
    let mut bytes = alloc::vec![0; size];
    transfer(fd, &mut bytes, false, deadline)?;
    Ok(bytes)
}
#[derive(Default)]
struct Encoder(Vec<u8>);
impl Encoder {
    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn float(&mut self, value: f32) {
        self.u32(value.to_bits());
    }
    fn bytes(&mut self, value: &[u8]) {
        self.u32(value.len() as u32);
        self.0.extend_from_slice(value);
    }
    fn rect(&mut self, r: Rect) {
        self.float(r.x);
        self.float(r.y);
        self.float(r.width);
        self.float(r.height);
    }
    fn style(&mut self, s: Style) {
        self.u32(u32::from(s.bold) | (u32::from(s.italic) << 1));
    }
}
struct Decoder<'a>(&'a [u8]);
impl<'a> Decoder<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        if n > self.0.len() {
            return Err(FAILURE);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u32(&mut self) -> Result<u32, &'static str> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, &'static str> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn float(&mut self) -> Result<f32, &'static str> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn bytes(&mut self) -> Result<&'a [u8], &'static str> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn text(&mut self) -> Result<&'a str, &'static str> {
        core::str::from_utf8(self.bytes()?).map_err(|_| FAILURE)
    }
    fn rect(&mut self) -> Result<Rect, &'static str> {
        Ok(Rect {
            x: self.float()?,
            y: self.float()?,
            width: self.float()?,
            height: self.float()?,
        })
    }
    fn style(&mut self) -> Result<Style, &'static str> {
        let bits = self.u32()?;
        Ok(Style {
            bold: bits & 1 != 0,
            italic: bits & 2 != 0,
        })
    }
}

pub struct Renderer {
    channel: Channel,
    slot: usize,
    commands: Encoder,
    driver: String,
    scratch: GridScratch,
    images: BTreeMap<u64, (u64, usize)>,
    image_bytes: usize,
    failed: bool,
}
impl Renderer {
    /// # Safety
    /// The native display must be live; the window must belong to that display.
    pub unsafe fn new(
        display: *mut crate::x11::Display,
        window: usize,
        visual: i32,
        width: u32,
        height: u32,
        font: &str,
    ) -> Result<Self, &'static str> {
        reap();
        let slot = CHILDREN
            .iter()
            .position(|s| s.load(Ordering::Relaxed) == 0)
            .ok_or("GPU worker limit reached")?;
        let name = unsafe { CStr::from_ptr(XDisplayString(display)) }.to_bytes();
        let mut init = Encoder::default();
        init.bytes(name);
        init.u64(window as u64);
        init.u32(visual as u32);
        init.u32(width);
        init.u32(height);
        init.bytes(font.as_bytes());
        let parent = CString::new(alloc::format!("{}", unsafe { os::getpid() })).unwrap();
        let environment = os::environment();
        let env: Vec<*const c_char> = environment
            .iter()
            .map(|s| s.as_ptr())
            .chain(core::iter::once(core::ptr::null()))
            .collect();
        let args = [
            c"ztty".as_ptr(),
            c"__gpu-worker".as_ptr(),
            parent.as_ptr(),
            core::ptr::null(),
        ];
        let mut sockets = [-1; 2];
        if unsafe { socketpair(1, 1 | 0x80000, 0, sockets.as_mut_ptr()) } != 0 {
            return Err(FAILURE);
        }
        let host = Channel(sockets[0]).above_stdio()?;
        let child = Channel(sockets[1]).above_stdio()?;
        // Submit native-window creation before the separate X client uses it.
        unsafe {
            XSync(display, 0);
        }
        if unsafe { os::fcntl(host.0, 4, 0x800) } < 0 {
            return Err(FAILURE);
        }
        // Keep the child waitable until cancellation; no PID reuse before reap.
        unsafe {
            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
        }
        let pid = unsafe { os::fork() };
        if pid < 0 {
            return Err(FAILURE);
        }
        if pid == 0 {
            unsafe {
                if dup2(child.0, 0) < 0
                    || dup2(child.0, 1) < 0
                    || syscall(436, 3u32, u32::MAX, 0u32) != 0
                {
                    os::_exit(126);
                }
                execve(c"/proc/self/exe".as_ptr(), args.as_ptr(), env.as_ptr());
                os::_exit(127);
            }
        }
        CHILDREN[slot].store(pid, Ordering::Relaxed);
        let mut result = Self {
            channel: host,
            slot,
            commands: Encoder::default(),
            driver: String::new(),
            scratch: GridScratch::default(),
            images: BTreeMap::new(),
            image_bytes: 0,
            failed: false,
        };
        let deadline = os::milliseconds() + STARTUP_MS;
        packet(result.channel.0, &mut init.0, deadline)?;
        let bytes = receive(result.channel.0, deadline)?;
        let mut reply = Decoder(&bytes);
        if reply.u32()? != 0 {
            return Err(FAILURE);
        }
        result.driver = String::from(reply.text()?);
        Ok(result)
    }
    pub fn pid(&self) -> i32 {
        CHILDREN[self.slot].load(Ordering::Relaxed)
    }
    pub fn driver(&self) -> &str {
        &self.driver
    }
    fn command(&mut self, op: u32) -> Result<(), &'static str> {
        if self.failed || self.commands.0.len() > LIMIT - 1024 {
            return Err(FAILURE);
        }
        self.commands.u32(op);
        Ok(())
    }
    fn flush(&mut self) -> Result<Vec<u8>, &'static str> {
        if self.failed || self.commands.0.len() > LIMIT {
            return Err(FAILURE);
        }
        let deadline = os::milliseconds() + TIMEOUT_MS;
        let result = (|| {
            packet(self.channel.0, &mut self.commands.0, deadline)?;
            receive(self.channel.0, deadline)
        })();
        self.commands.0.clear();
        if self.commands.0.capacity() > 1024 * 1024 {
            self.commands.0 = Vec::new();
        }
        if result.is_err() {
            self.failed = true;
        }
        let bytes = result?;
        let mut reply = Decoder(&bytes);
        if reply.u32()? != 0 {
            self.failed = true;
            return Err(FAILURE);
        }
        Ok(reply.0.to_vec())
    }
    pub fn set_font(&mut self, font: &str) -> Result<(), &'static str> {
        self.command(1)?;
        self.commands.bytes(font.as_bytes());
        self.flush().map(|_| ())
    }
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), &'static str> {
        self.command(2)?;
        self.commands.u32(width);
        self.commands.u32(height);
        self.flush().map(|_| ())
    }
    pub fn present(&mut self) -> Result<(), &'static str> {
        self.command(3)?;
        self.flush().map(|_| ())
    }
    pub fn measure_text(&mut self, text: &str, style: Style) -> Result<f32, &'static str> {
        self.command(4)?;
        self.commands.bytes(text.as_bytes());
        self.commands.style(style);
        let bytes = self.flush()?;
        Decoder(&bytes).float()
    }
    pub fn draw_text(
        &mut self,
        text: &str,
        x: f32,
        baseline: f32,
        rgb: u32,
        style: Style,
    ) -> Result<(), &'static str> {
        self.command(5)?;
        self.commands.bytes(text.as_bytes());
        self.commands.float(x);
        self.commands.float(baseline);
        self.commands.u32(rgb);
        self.commands.style(style);
        Ok(())
    }
    pub fn draw_terminal(
        &mut self,
        terminal: &zterm::Terminal,
        offset: usize,
        style: GridStyle,
        selected: impl Fn(usize, usize) -> bool,
    ) -> Result<(), &'static str> {
        let mut scratch = core::mem::take(&mut self.scratch);
        let result =
            gpu::grid::draw_terminal(self, &mut scratch, terminal, offset, style, selected);
        self.scratch = scratch;
        result
    }
}
impl Channel {
    fn above_stdio(mut self) -> Result<Self, &'static str> {
        if self.0 <= 2 {
            let next = unsafe { os::fcntl(self.0, 1030, 3) };
            if next < 0 {
                return Err(FAILURE);
            }
            unsafe {
                os::close(self.0);
            }
            self.0 = next;
        }
        Ok(self)
    }
}
impl Painter for Renderer {
    fn begin_frame(&mut self, background: u32, alpha: u8) -> Result<(), &'static str> {
        self.command(6)?;
        self.commands.u32(background);
        self.commands.u32(alpha as u32);
        Ok(())
    }
    fn clip(&mut self, r: Option<Rect>) -> Result<(), &'static str> {
        self.command(7)?;
        self.commands.u32(u32::from(r.is_some()));
        if let Some(r) = r {
            self.commands.rect(r);
        }
        Ok(())
    }
    fn draw_rect(&mut self, r: Rect, rgb: u32, alpha: u8) -> Result<(), &'static str> {
        self.command(8)?;
        self.commands.rect(r);
        self.commands.u32(rgb);
        self.commands.u32(alpha as u32);
        Ok(())
    }
    fn draw_cells(
        &mut self,
        text: &str,
        cells: &[TextCell],
        x: f32,
        baseline: f32,
        rgb: u32,
        style: Style,
    ) -> Result<(), &'static str> {
        self.command(9)?;
        self.commands.bytes(text.as_bytes());
        self.commands.u32(cells.len() as u32);
        for cell in cells {
            self.commands.u32(cell.byte);
            self.commands.float(cell.x);
            self.commands.float(cell.width);
        }
        self.commands.float(x);
        self.commands.float(baseline);
        self.commands.u32(rgb);
        self.commands.style(style);
        Ok(())
    }
    fn draw_image(
        &mut self,
        key: u64,
        revision: u64,
        size: (u32, u32),
        rgba: &[u8],
        source: Rect,
        destination: Rect,
    ) -> Result<(), &'static str> {
        if rgba.len() > IMAGE_LIMIT || size.0 as u64 * size.1 as u64 * 4 != rgba.len() as u64 {
            return Err("image exceeds isolated renderer budget");
        }
        if self
            .images
            .get(&key)
            .is_none_or(|entry| entry.0 != revision)
        {
            if self.image_bytes + rgba.len() > IMAGE_LIMIT {
                let keys: Vec<_> = self.images.keys().copied().collect();
                for key in keys {
                    self.remove_image(key)?;
                }
            }
            if self.commands.0.len() + rgba.len() + 128 > LIMIT {
                self.flush()?;
            }
            self.command(10)?;
            self.commands.u64(key);
            self.commands.u64(revision);
            self.commands.u32(size.0);
            self.commands.u32(size.1);
            self.commands.bytes(rgba);
            if let Some((_, bytes)) = self.images.insert(key, (revision, rgba.len())) {
                self.image_bytes -= bytes;
            }
            self.image_bytes += rgba.len();
        }
        self.command(11)?;
        self.commands.u64(key);
        self.commands.rect(source);
        self.commands.rect(destination);
        Ok(())
    }
    fn remove_image(&mut self, key: u64) -> Result<(), &'static str> {
        self.command(12)?;
        self.commands.u64(key);
        if let Some((_, bytes)) = self.images.remove(&key) {
            self.image_bytes -= bytes;
        }
        Ok(())
    }
    fn image_revisions(&self) -> Vec<(u64, u64)> {
        self.images
            .iter()
            .map(|(key, (revision, _))| (*key, *revision))
            .collect()
    }
}
impl Drop for Renderer {
    fn drop(&mut self) {
        let pid = CHILDREN[self.slot].load(Ordering::Relaxed);
        if pid > 0 {
            unsafe {
                os::kill(pid, 9);
            };
            CHILDREN[self.slot].store(-pid, Ordering::Relaxed);
        }
        reap();
    }
}
struct Image {
    revision: u64,
    size: (u32, u32),
    pixels: Vec<u8>,
}
fn execute(
    renderer: &mut gpu::Renderer,
    images: &mut BTreeMap<u64, Image>,
    data: &[u8],
) -> Result<Vec<u8>, &'static str> {
    let mut input = Decoder(data);
    let mut output = Encoder::default();
    output.u32(0);
    while !input.0.is_empty() {
        match input.u32()? {
            1 => renderer.set_font(input.text()?)?,
            2 => renderer.resize(input.u32()?, input.u32()?)?,
            3 => renderer.present()?,
            4 => output.float(renderer.measure_text(input.text()?, input.style()?)?),
            5 => {
                let text = input.text()?;
                let x = input.float()?;
                let baseline = input.float()?;
                renderer.draw_text(text, x, baseline, input.u32()?, input.style()?)?;
            }
            6 => renderer.begin_frame(input.u32()?, input.u32()? as u8)?,
            7 => {
                let r = if input.u32()? != 0 {
                    Some(input.rect()?)
                } else {
                    None
                };
                renderer.clip(r)?;
            }
            8 => renderer.draw_rect(input.rect()?, input.u32()?, input.u32()? as u8)?,
            9 => {
                let text = input.text()?;
                let count = input.u32()? as usize;
                if count > 8192 {
                    return Err(FAILURE);
                }
                let mut cells = Vec::with_capacity(count);
                for _ in 0..count {
                    cells.push(TextCell {
                        byte: input.u32()?,
                        x: input.float()?,
                        width: input.float()?,
                    });
                }
                renderer.draw_cells(
                    text,
                    &cells,
                    input.float()?,
                    input.float()?,
                    input.u32()?,
                    input.style()?,
                )?;
            }
            10 => {
                let key = input.u64()?;
                let revision = input.u64()?;
                let size = (input.u32()?, input.u32()?);
                let pixels = input.bytes()?;
                let used: usize = images
                    .iter()
                    .filter(|(k, _)| **k != key)
                    .map(|(_, i)| i.pixels.len())
                    .sum();
                if pixels.len() + used > IMAGE_LIMIT {
                    return Err(FAILURE);
                }
                images.insert(
                    key,
                    Image {
                        revision,
                        size,
                        pixels: pixels.to_vec(),
                    },
                );
            }
            11 => {
                let key = input.u64()?;
                let image = images.get(&key).ok_or(FAILURE)?;
                let source = input.rect()?;
                let dest = input.rect()?;
                renderer.draw_image(
                    key,
                    image.revision,
                    image.size,
                    &image.pixels,
                    source,
                    dest,
                )?;
            }
            12 => {
                let key = input.u64()?;
                images.remove(&key);
                renderer.remove_image(key)?;
            }
            _ => return Err(FAILURE),
        }
    }
    Ok(output.0)
}

pub fn worker(args: &[&CStr]) -> Result<i32, String> {
    let parent = args
        .first()
        .and_then(|a| a.to_str().ok())
        .and_then(|s| s.parse::<i32>().ok())
        .filter(|p| *p > 0)
        .ok_or("invalid GPU worker parent")?;
    if unsafe { prctl(1, 9, 0, 0, 0) } != 0 || unsafe { getppid() } != parent {
        return Err("GPU worker parent disappeared".into());
    }
    let result = (|| {
        let init = receive(0, os::milliseconds() + STARTUP_MS)?;
        let mut init = Decoder(&init);
        let name = CString::new(init.bytes()?).map_err(|_| FAILURE)?;
        let window = init.u64()? as usize;
        let visual = init.u32()? as i32;
        let width = init.u32()?;
        let height = init.u32()?;
        let font = init.text()?;
        let native = unsafe { XOpenDisplay(name.as_ptr()) };
        if native.is_null() {
            return Err(FAILURE);
        }
        let display = unsafe { gpu::Display::x11(native.cast()) }?;
        let mut renderer = gpu::Renderer::new(
            display,
            gpu::Target::X11 { window, visual },
            width,
            height,
            font,
        )?;
        let mut reply = Encoder::default();
        reply.u32(0);
        reply.bytes(renderer.driver().as_bytes());
        packet(1, &mut reply.0, os::milliseconds() + STARTUP_MS)?;
        let mut images = BTreeMap::new();
        loop {
            // Idle workers may sleep indefinitely; each actual call is bounded
            // by the broker. A dead parent also delivers SIGKILL via prctl.
            let data = receive(0, u64::MAX)?;
            let mut reply = execute(&mut renderer, &mut images, &data)?;
            packet(1, &mut reply, os::milliseconds() + TIMEOUT_MS)?;
        }
    })();
    // Never run driver destructors on the broker, nor wait for this worker.
    result.map_err(String::from)
}
