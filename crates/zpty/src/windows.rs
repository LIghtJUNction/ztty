//! Native Windows pseudoconsole ownership. The public I/O calls never block on
//! the console: two native workers service synchronous ConPTY pipes through
//! bounded queues. No Rust standard library or external runtime is required.

use alloc::{boxed::Box, collections::VecDeque, sync::Arc, vec, vec::Vec};
use core::{
    cell::UnsafeCell,
    ffi::c_void,
    fmt, ptr,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering},
};

type Handle = *mut c_void;
const QUEUE_LIMIT: usize = 1024 * 1024;
const AGAIN: i32 = 232;
const BROKEN_PIPE: i32 = 109;
const INFINITE: u32 = u32::MAX;

type CreateConsole = unsafe extern "system" fn(ffi::Coord, Handle, Handle, u32, *mut Handle) -> i32;
type ResizeConsole = unsafe extern "system" fn(Handle, ffi::Coord) -> i32;
type CloseConsole = unsafe extern "system" fn(Handle);

struct ConptyApi {
    module: Handle,
    create: CreateConsole,
    resize: ResizeConsole,
    close: CloseConsole,
}
impl ConptyApi {
    fn load() -> Self {
        let inbox = Self {
            module: ptr::null_mut(),
            create: ffi::CreatePseudoConsole,
            resize: ffi::ResizePseudoConsole,
            close: ffi::ClosePseudoConsole,
        };
        let mut path = vec![0u16; 32768];
        let length = unsafe {
            ffi::GetModuleFileNameW(ptr::null_mut(), path.as_mut_ptr(), path.len() as u32)
        } as usize;
        if length == 0 || length >= path.len() {
            return inbox;
        }
        path.truncate(length);
        let Some(separator) = path
            .iter()
            .rposition(|&c| c == b'\\' as u16 || c == b'/' as u16)
        else {
            return inbox;
        };
        path.truncate(separator + 1);
        let mut host = path.clone();
        host.extend("OpenConsole.exe".encode_utf16());
        host.push(0);
        let attributes = unsafe { ffi::GetFileAttributesW(host.as_ptr()) };
        // Use the application-local pair only. Never search PATH, the working
        // directory, or arbitrary DLL locations, and never mix mismatched hosts.
        if attributes == u32::MAX || attributes & 0x10 != 0 {
            return inbox;
        }
        path.extend("conpty.dll".encode_utf16());
        path.push(0);
        let module =
            unsafe { ffi::LoadLibraryExW(path.as_ptr(), ptr::null_mut(), 0x00000100 | 0x00000800) };
        if module.is_null() {
            return inbox;
        }
        let create = unsafe { ffi::GetProcAddress(module, c"ConptyCreatePseudoConsole".as_ptr()) };
        let resize = unsafe { ffi::GetProcAddress(module, c"ConptyResizePseudoConsole".as_ptr()) };
        let close = unsafe { ffi::GetProcAddress(module, c"ConptyClosePseudoConsole".as_ptr()) };
        if create.is_null() || resize.is_null() || close.is_null() {
            unsafe {
                ffi::FreeLibrary(module);
            }
            return inbox;
        }
        Self {
            module,
            // The official ConPTY header declares the exact signatures above.
            create: unsafe { core::mem::transmute::<*mut c_void, CreateConsole>(create) },
            resize: unsafe { core::mem::transmute::<*mut c_void, ResizeConsole>(resize) },
            close: unsafe { core::mem::transmute::<*mut c_void, CloseConsole>(close) },
        }
    }
}
impl Drop for ConptyApi {
    fn drop(&mut self) {
        if !self.module.is_null() {
            unsafe {
                ffi::FreeLibrary(self.module);
            }
        }
    }
}

/// Operation that failed at the native boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Spawn,
    Nonblocking,
    CloseOnExec,
    Read,
    Write,
    Resize,
    Wait,
    Signal,
}

/// A Win32 error (or HRESULT for pseudoconsole operations), without allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Error {
    pub operation: Operation,
    pub errno: i32,
}
impl Error {
    fn last(operation: Operation) -> Self {
        Self {
            operation,
            errno: unsafe { ffi::GetLastError() } as i32,
        }
    }
    fn invalid(operation: Operation) -> Self {
        Self {
            operation,
            errno: 87,
        }
    }
    pub const fn is_would_block(self) -> bool {
        self.errno == AGAIN
    }
    pub const fn is_interrupted(self) -> bool {
        false
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: Windows error {}", self.operation, self.errno)
    }
}
impl core::error::Error for Error {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Size {
    pub rows: u16,
    pub cols: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}
impl Size {
    pub const fn new(cols: u16, rows: u16) -> Self {
        Self {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
    fn coord(self) -> Result<ffi::Coord, Error> {
        if self.cols == 0
            || self.rows == 0
            || self.cols > i16::MAX as u16
            || self.rows > i16::MAX as u16
        {
            return Err(Error::invalid(Operation::Resize));
        }
        Ok(ffi::Coord {
            x: self.cols as i16,
            y: self.rows as i16,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExitStatus(u32);
impl ExitStatus {
    pub const fn code(self) -> Option<i32> {
        Some(self.0 as i32)
    }
    pub const fn signal(self) -> Option<i32> {
        None
    }
    pub const fn success(self) -> bool {
        self.0 == 0
    }
    pub const fn raw(self) -> i32 {
        self.0 as i32
    }
}

struct Channel {
    lock: UnsafeCell<usize>,
    bytes: UnsafeCell<VecDeque<u8>>,
    ready: Handle,
    space: Handle,
    stopping: AtomicBool,
    closed: AtomicBool,
    error: AtomicU32,
}
// SRWLOCK serializes every queue access. Events and atomics are native
// thread-safe objects and remain alive through each worker's Arc.
unsafe impl Send for Channel {}
unsafe impl Sync for Channel {}
impl Channel {
    fn new() -> Result<Arc<Self>, Error> {
        let ready = unsafe { ffi::CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if ready.is_null() {
            return Err(Error::last(Operation::Spawn));
        }
        let space = unsafe { ffi::CreateEventW(ptr::null(), 0, 0, ptr::null()) };
        if space.is_null() {
            let error = Error::last(Operation::Spawn);
            unsafe {
                ffi::CloseHandle(ready);
            }
            return Err(error);
        }
        Ok(Arc::new(Self {
            lock: UnsafeCell::new(0),
            bytes: UnsafeCell::new(VecDeque::new()),
            ready,
            space,
            stopping: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            error: AtomicU32::new(0),
        }))
    }
    fn with_queue<R>(&self, f: impl FnOnce(&mut VecDeque<u8>) -> R) -> R {
        unsafe {
            ffi::AcquireSRWLockExclusive(self.lock.get());
        }
        let result = f(unsafe { &mut *self.bytes.get() });
        unsafe {
            ffi::ReleaseSRWLockExclusive(self.lock.get());
        }
        result
    }
    fn finish(&self, error: u32) {
        self.error.store(error, Ordering::Release);
        self.closed.store(true, Ordering::Release);
        unsafe {
            ffi::SetEvent(self.ready);
            ffi::SetEvent(self.space);
        }
    }
}
impl Drop for Channel {
    fn drop(&mut self) {
        unsafe {
            ffi::CloseHandle(self.ready);
            ffi::CloseHandle(self.space);
        }
    }
}

struct Worker {
    channel: Arc<Channel>,
    pipe: Handle,
}

unsafe extern "system" fn reader(context: *mut c_void) -> u32 {
    let worker = unsafe { Box::from_raw(context.cast::<Worker>()) };
    let mut buffer = [0u8; 16384];
    let error = loop {
        let mut count = 0;
        if unsafe {
            ffi::ReadFile(
                worker.pipe,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut count,
                ptr::null_mut(),
            )
        } == 0
        {
            break unsafe { ffi::GetLastError() };
        }
        if count == 0 {
            break 0;
        }
        let mut offset = 0;
        while offset < count as usize {
            // Teardown must continue draining ConPTY, even when the frontend
            // has stopped consuming output, to avoid ClosePseudoConsole deadlock.
            if worker.channel.stopping.load(Ordering::Acquire) {
                break;
            }
            let copied = worker.channel.with_queue(|queue| {
                let n = (QUEUE_LIMIT - queue.len()).min(count as usize - offset);
                queue.extend(&buffer[offset..offset + n]);
                if n != 0 {
                    unsafe {
                        ffi::SetEvent(worker.channel.ready);
                    }
                }
                n
            });
            offset += copied;
            if copied == 0 {
                unsafe {
                    ffi::WaitForSingleObject(worker.channel.space, INFINITE);
                }
            }
        }
    };
    worker.channel.finish(error);
    unsafe {
        ffi::CloseHandle(worker.pipe);
    }
    0
}

unsafe extern "system" fn writer(context: *mut c_void) -> u32 {
    let worker = unsafe { Box::from_raw(context.cast::<Worker>()) };
    let mut buffer = [0u8; 16384];
    let error = 'outer: loop {
        if worker.channel.stopping.load(Ordering::Acquire) {
            break 0;
        }
        let count = worker.channel.with_queue(|queue| {
            let count = queue.len().min(buffer.len());
            for byte in &mut buffer[..count] {
                *byte = queue.pop_front().unwrap_or(0);
            }
            if queue.is_empty() {
                unsafe {
                    ffi::ResetEvent(worker.channel.ready);
                }
            }
            count
        });
        if count == 0 {
            // hangup may have set ready immediately before the empty-queue
            // branch reset it. Recheck the stop flag after that reset so a
            // worker cannot sleep forever during teardown.
            if !worker.channel.stopping.load(Ordering::Acquire) {
                unsafe {
                    ffi::WaitForSingleObject(worker.channel.ready, INFINITE);
                }
            }
            continue;
        }
        unsafe {
            ffi::SetEvent(worker.channel.space);
        }
        let mut offset = 0;
        while offset < count {
            if worker.channel.stopping.load(Ordering::Acquire) {
                break 'outer 0;
            }
            let mut written = 0;
            if unsafe {
                ffi::WriteFile(
                    worker.pipe,
                    buffer[offset..count].as_ptr().cast(),
                    (count - offset) as u32,
                    &mut written,
                    ptr::null_mut(),
                )
            } == 0
            {
                break 'outer unsafe { ffi::GetLastError() };
            }
            if written == 0 {
                break 'outer BROKEN_PIPE as u32;
            }
            offset += written as usize;
        }
    };
    worker.channel.finish(error);
    unsafe {
        ffi::CloseHandle(worker.pipe);
    }
    0
}

fn start_worker(
    channel: Arc<Channel>,
    pipe: Handle,
    entry: unsafe extern "system" fn(*mut c_void) -> u32,
) -> Result<Handle, Error> {
    let context = Box::into_raw(Box::new(Worker { channel, pipe }));
    let thread = unsafe {
        ffi::CreateThread(
            ptr::null(),
            0,
            Some(entry),
            context.cast(),
            0,
            ptr::null_mut(),
        )
    };
    if thread.is_null() {
        let error = Error::last(Operation::Spawn);
        unsafe {
            drop(Box::from_raw(context));
            ffi::CloseHandle(pipe);
        }
        return Err(error);
    }
    Ok(thread)
}

/// An owned ConPTY and child process. A pair of worker threads bridge the
/// synchronous Windows API to nonblocking, bounded public read/write calls.
/// `hangup` closes the pseudoconsole and its attached process tree while the
/// output worker drains its final frame. No unrelated process is reaped.
pub struct Pty {
    api: ConptyApi,
    console: AtomicPtr<c_void>,
    process: Handle,
    pid: u32,
    input: Arc<Channel>,
    output: Arc<Channel>,
    reader: Handle,
    writer: Handle,
    status: Option<ExitStatus>,
}

impl Pty {
    /// Start a native UTF-16 command. `arguments` includes argv[0]; every string
    /// excludes its terminating NUL. `application` is a resolved executable
    /// path. The optional environment is a sorted double-NUL-terminated Windows
    /// block; `None` inherits the process environment. Arguments are quoted
    /// using the Windows CRT escaping rules, with a maximum 32767 UTF-16 units.
    pub fn spawn_utf16(
        application: &[u16],
        arguments: &[&[u16]],
        environment: Option<&[u16]>,
        size: Size,
    ) -> Result<Self, Error> {
        if application.is_empty()
            || application.contains(&0)
            || arguments.is_empty()
            || arguments.iter().any(|s| s.contains(&0))
        {
            return Err(Error::invalid(Operation::Spawn));
        }
        if environment.is_some_and(|e| e.len() < 2 || e[e.len() - 2..] != [0, 0]) {
            return Err(Error::invalid(Operation::Spawn));
        }
        let coord = size.coord().map_err(|error| Error {
            operation: Operation::Spawn,
            ..error
        })?;
        let mut command = command_line(arguments)?;
        let mut app = application.to_vec();
        app.push(0);
        let input = Channel::new()?;
        let output = Channel::new()?;
        let mut input_read = ptr::null_mut();
        let mut input_write = ptr::null_mut();
        let mut output_read = ptr::null_mut();
        let mut output_write = ptr::null_mut();
        if unsafe { ffi::CreatePipe(&mut input_read, &mut input_write, ptr::null(), 65536) } == 0 {
            return Err(Error::last(Operation::Spawn));
        }
        if unsafe { ffi::CreatePipe(&mut output_read, &mut output_write, ptr::null(), 65536) } == 0
        {
            let e = Error::last(Operation::Spawn);
            unsafe {
                ffi::CloseHandle(input_read);
                ffi::CloseHandle(input_write);
            }
            return Err(e);
        }
        let api = ConptyApi::load();
        let mut console = ptr::null_mut();
        let hr = unsafe { (api.create)(coord, input_read, output_write, 0, &mut console) };
        if hr < 0 {
            unsafe {
                ffi::CloseHandle(input_read);
                ffi::CloseHandle(input_write);
                ffi::CloseHandle(output_read);
                ffi::CloseHandle(output_write);
            }
            return Err(Error {
                operation: Operation::Spawn,
                errno: hr,
            });
        }
        // The pseudoconsole duplicates the handles; close the host's extra
        // endpoints before the child starts so EOF propagates correctly.
        unsafe {
            ffi::CloseHandle(input_read);
            ffi::CloseHandle(output_write);
        }
        let reader = match start_worker(output.clone(), output_read, reader) {
            Ok(thread) => thread,
            Err(e) => {
                unsafe {
                    ffi::CloseHandle(input_write);
                    (api.close)(console);
                }
                return Err(e);
            }
        };
        let mut pty = Self {
            api,
            console: AtomicPtr::new(console),
            process: ptr::null_mut(),
            pid: 0,
            input,
            output,
            reader,
            writer: ptr::null_mut(),
            status: None,
        };
        pty.writer = start_worker(pty.input.clone(), input_write, writer)?;
        let mut bytes = 0;
        unsafe {
            ffi::InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(Error::last(Operation::Spawn));
        }
        let mut attributes = vec![0usize; bytes.div_ceil(core::mem::size_of::<usize>())];
        let list = attributes.as_mut_ptr().cast();
        if unsafe { ffi::InitializeProcThreadAttributeList(list, 1, 0, &mut bytes) } == 0 {
            return Err(Error::last(Operation::Spawn));
        }
        let success = unsafe {
            ffi::UpdateProcThreadAttribute(
                list,
                0,
                0x00020016,
                console,
                core::mem::size_of::<Handle>(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if success == 0 {
            let e = Error::last(Operation::Spawn);
            unsafe {
                ffi::DeleteProcThreadAttributeList(list);
            }
            return Err(e);
        }
        let mut startup: ffi::StartupInfoEx = unsafe { core::mem::zeroed() };
        startup.startup.cb = core::mem::size_of::<ffi::StartupInfoEx>() as u32;
        // Explicit null stdio lets ConPTY supply its console handles instead of
        // duplicating redirected host handles, even with inheritance disabled.
        // Mirrors Microsoft's _LaunchAttachedClient:
        // https://github.com/microsoft/terminal/blob/66cfdd5ecf159cde82f8cb27c2756062da7296a3/src/cascadia/TerminalConnection/ConptyConnection.cpp#L34-L36
        startup.startup.flags = 0x00000100; // STARTF_USESTDHANDLES
        startup.attributes = list;
        let mut process: ffi::ProcessInformation = unsafe { core::mem::zeroed() };
        let success = unsafe {
            ffi::CreateProcessW(
                app.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                0x00080000 | 0x00000400,
                environment.map_or(ptr::null(), |e| e.as_ptr().cast()),
                ptr::null(),
                &startup.startup,
                &mut process,
            )
        };
        let error = Error::last(Operation::Spawn);
        unsafe {
            ffi::DeleteProcThreadAttributeList(list);
        }
        if success == 0 {
            return Err(error);
        }
        pty.process = process.process;
        pty.pid = process.process_id;
        unsafe {
            ffi::CloseHandle(process.thread);
        }
        Ok(pty)
    }

    /// Borrow the manual-reset output readiness event. Wait with
    /// `MsgWaitForMultipleObjectsEx` in a UI, then drain `read` until would-block.
    /// The handle remains owned by this object and must not be closed by callers.
    pub fn output_event(&self) -> *mut c_void {
        self.output.ready
    }
    /// Borrow the child process handle for native wait sets. It is signaled
    /// when the directly owned child exits and remains owned by this object.
    pub fn process_event(&self) -> *mut c_void {
        self.process
    }
    /// Whether this session uses the official application-local ConPTY DLL
    /// and OpenConsole.exe pair instead of the operating system's inbox copy.
    pub fn uses_bundled_conpty(&self) -> bool {
        !self.api.module.is_null()
    }
    pub const fn pid(&self) -> u32 {
        self.pid
    }
    pub const fn status(&self) -> Option<ExitStatus> {
        self.status
    }

    pub fn read(&self, bytes: &mut [u8]) -> Result<usize, Error> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let count = self.output.with_queue(|queue| {
            let n = queue.len().min(bytes.len());
            for byte in &mut bytes[..n] {
                *byte = queue.pop_front().unwrap_or(0);
            }
            if queue.is_empty() && !self.output.closed.load(Ordering::Acquire) {
                unsafe {
                    ffi::ResetEvent(self.output.ready);
                }
            }
            n
        });
        if count != 0 {
            unsafe {
                ffi::SetEvent(self.output.space);
            }
            return Ok(count);
        }
        if self.output.closed.load(Ordering::Acquire) {
            let error = self.output.error.load(Ordering::Acquire) as i32;
            return if error == 0 || error == BROKEN_PIPE || error == 995 {
                Ok(0)
            } else {
                Err(Error {
                    operation: Operation::Read,
                    errno: error,
                })
            };
        }
        Err(Error {
            operation: Operation::Read,
            errno: AGAIN,
        })
    }

    /// Queue as much input as fits in the 1 MiB bounded queue. The return value
    /// may be shorter than the argument; preserve the suffix and retry later.
    pub fn write(&self, bytes: &[u8]) -> Result<usize, Error> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.input.closed.load(Ordering::Acquire) || self.input.stopping.load(Ordering::Acquire)
        {
            return Err(Error {
                operation: Operation::Write,
                errno: BROKEN_PIPE,
            });
        }
        let count = self.input.with_queue(|queue| {
            let n = (QUEUE_LIMIT - queue.len()).min(bytes.len());
            queue.extend(&bytes[..n]);
            if n != 0 {
                unsafe {
                    ffi::SetEvent(self.input.ready);
                }
            }
            n
        });
        if count == 0 {
            Err(Error {
                operation: Operation::Write,
                errno: AGAIN,
            })
        } else {
            Ok(count)
        }
    }
    pub fn resize(&self, size: Size) -> Result<(), Error> {
        let console = self.console.load(Ordering::Acquire);
        if console.is_null() {
            return Err(Error::invalid(Operation::Resize));
        }
        let hr = unsafe { (self.api.resize)(console, size.coord()?) };
        if hr < 0 {
            Err(Error {
                operation: Operation::Resize,
                errno: hr,
            })
        } else {
            Ok(())
        }
    }
    pub fn reap(&mut self) -> Result<Option<ExitStatus>, Error> {
        self.wait_timeout(0)
    }
    pub fn wait_timeout(&mut self, timeout_ms: u32) -> Result<Option<ExitStatus>, Error> {
        if self.status.is_some() {
            return Ok(self.status);
        }
        match unsafe { ffi::WaitForSingleObject(self.process, timeout_ms) } {
            0 => {
                let mut code = 0;
                if unsafe { ffi::GetExitCodeProcess(self.process, &mut code) } == 0 {
                    return Err(Error::last(Operation::Wait));
                }
                self.status = Some(ExitStatus(code));
                Ok(self.status)
            }
            258 => Ok(None),
            _ => Err(Error::last(Operation::Wait)),
        }
    }
    pub fn wait(&mut self) -> Result<ExitStatus, Error> {
        self.wait_timeout(INFINITE)?
            .ok_or(Error::invalid(Operation::Wait))
    }

    pub fn hangup(&self) -> Result<(), Error> {
        let console = self.console.swap(ptr::null_mut(), Ordering::AcqRel);
        if console.is_null() {
            return Ok(());
        }
        self.input.stopping.store(true, Ordering::Release);
        self.output.stopping.store(true, Ordering::Release);
        unsafe {
            ffi::SetEvent(self.input.ready);
            ffi::SetEvent(self.output.space);
            if !self.writer.is_null() {
                ffi::CancelSynchronousIo(self.writer);
            }
            // A different worker continues draining output throughout this call.
            (self.api.close)(console);
        }
        Ok(())
    }
}
impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.hangup();
        unsafe {
            for thread in [self.writer, self.reader] {
                if !thread.is_null() {
                    ffi::CancelSynchronousIo(thread);
                    ffi::WaitForSingleObject(thread, INFINITE);
                    ffi::CloseHandle(thread);
                }
            }
            if !self.process.is_null() {
                ffi::CloseHandle(self.process);
            }
        }
    }
}

fn command_line(arguments: &[&[u16]]) -> Result<Vec<u16>, Error> {
    if arguments
        .iter()
        .try_fold(0usize, |total, argument| {
            total
                .checked_add(argument.len())
                .and_then(|n| n.checked_add(3))
        })
        .is_none_or(|n| n > 32767)
    {
        return Err(Error::invalid(Operation::Spawn));
    }
    let mut result = Vec::new();
    for (index, argument) in arguments.iter().enumerate() {
        if index != 0 {
            result.push(b' ' as u16);
        }
        if !argument.is_empty()
            && !argument
                .iter()
                .any(|&c| c == b' ' as u16 || c == b'\t' as u16 || c == b'"' as u16)
        {
            result.extend_from_slice(argument);
            if result.len() > 32766 {
                return Err(Error::invalid(Operation::Spawn));
            }
            continue;
        }
        result.push(b'"' as u16);
        let mut slashes = 0;
        for &unit in *argument {
            if unit == b'\\' as u16 {
                slashes += 1;
                continue;
            }
            result.extend(core::iter::repeat_n(
                b'\\' as u16,
                if unit == b'"' as u16 {
                    slashes * 2 + 1
                } else {
                    slashes
                },
            ));
            slashes = 0;
            result.push(unit);
            if result.len() > 32765 {
                return Err(Error::invalid(Operation::Spawn));
            }
        }
        result.extend(core::iter::repeat_n(b'\\' as u16, slashes * 2));
        result.push(b'"' as u16);
        if result.len() > 32766 {
            return Err(Error::invalid(Operation::Spawn));
        }
    }
    result.push(0);
    Ok(result)
}

mod ffi {
    use super::*;
    #[repr(C)]
    pub struct Coord {
        pub x: i16,
        pub y: i16,
    }
    #[repr(C)]
    pub struct StartupInfo {
        pub cb: u32,
        pub reserved: *mut u16,
        pub desktop: *mut u16,
        pub title: *mut u16,
        pub x: u32,
        pub y: u32,
        pub x_size: u32,
        pub y_size: u32,
        pub x_count: u32,
        pub y_count: u32,
        pub fill: u32,
        pub flags: u32,
        pub show: u16,
        pub reserved2: u16,
        pub reserved_ptr: *mut u8,
        pub stdin: Handle,
        pub stdout: Handle,
        pub stderr: Handle,
    }
    #[repr(C)]
    pub struct StartupInfoEx {
        pub startup: StartupInfo,
        pub attributes: *mut c_void,
    }
    #[repr(C)]
    pub struct ProcessInformation {
        pub process: Handle,
        pub thread: Handle,
        pub process_id: u32,
        pub thread_id: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetLastError() -> u32;
        pub fn GetModuleFileNameW(module: Handle, filename: *mut u16, size: u32) -> u32;
        pub fn GetFileAttributesW(filename: *const u16) -> u32;
        pub fn LoadLibraryExW(filename: *const u16, file: Handle, flags: u32) -> Handle;
        pub fn GetProcAddress(module: Handle, name: *const core::ffi::c_char) -> *mut c_void;
        pub fn FreeLibrary(module: Handle) -> i32;
        pub fn CreatePipe(
            read: *mut Handle,
            write: *mut Handle,
            attributes: *const c_void,
            size: u32,
        ) -> i32;
        pub fn ReadFile(
            file: Handle,
            buffer: *mut c_void,
            count: u32,
            read: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        pub fn WriteFile(
            file: Handle,
            buffer: *const c_void,
            count: u32,
            written: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        pub fn CreatePseudoConsole(
            size: Coord,
            input: Handle,
            output: Handle,
            flags: u32,
            console: *mut Handle,
        ) -> i32;
        pub fn ResizePseudoConsole(console: Handle, size: Coord) -> i32;
        pub fn ClosePseudoConsole(console: Handle);
        pub fn CreateEventW(
            attributes: *const c_void,
            manual: i32,
            initial: i32,
            name: *const u16,
        ) -> Handle;
        pub fn SetEvent(event: Handle) -> i32;
        pub fn ResetEvent(event: Handle) -> i32;
        pub fn AcquireSRWLockExclusive(lock: *mut usize);
        pub fn ReleaseSRWLockExclusive(lock: *mut usize);
        pub fn WaitForSingleObject(handle: Handle, timeout: u32) -> u32;
        pub fn CloseHandle(handle: Handle) -> i32;
        pub fn CreateThread(
            attributes: *const c_void,
            stack: usize,
            entry: Option<unsafe extern "system" fn(*mut c_void) -> u32>,
            parameter: *mut c_void,
            flags: u32,
            id: *mut u32,
        ) -> Handle;
        pub fn CancelSynchronousIo(thread: Handle) -> i32;
        pub fn InitializeProcThreadAttributeList(
            list: *mut c_void,
            count: u32,
            flags: u32,
            size: *mut usize,
        ) -> i32;
        pub fn UpdateProcThreadAttribute(
            list: *mut c_void,
            flags: u32,
            attribute: usize,
            value: *mut c_void,
            size: usize,
            previous: *mut c_void,
            returned: *mut usize,
        ) -> i32;
        pub fn DeleteProcThreadAttributeList(list: *mut c_void);
        pub fn CreateProcessW(
            application: *const u16,
            command: *mut u16,
            process_attributes: *const c_void,
            thread_attributes: *const c_void,
            inherit: i32,
            flags: u32,
            environment: *const c_void,
            directory: *const u16,
            startup: *const StartupInfo,
            process: *mut ProcessInformation,
        ) -> i32;
        pub fn GetExitCodeProcess(process: Handle, code: *mut u32) -> i32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quotes_empty_spaces_quotes_and_trailing_backslashes() {
        let args: Vec<Vec<u16>> = ["tool.exe", "", "a b", "a\\\"b", "C:\\folder name\\"]
            .iter()
            .map(|s| s.encode_utf16().collect())
            .collect();
        let refs: Vec<&[u16]> = args.iter().map(Vec::as_slice).collect();
        let command = command_line(&refs).unwrap();
        assert_eq!(
            alloc::string::String::from_utf16(&command[..command.len() - 1]).unwrap(),
            "tool.exe \"\" \"a b\" \"a\\\\\\\"b\" \"C:\\folder name\\\\\""
        );
    }
    #[test]
    fn command_line_limit_is_checked_after_escaping() {
        let mut slashes = vec![b' ' as u16];
        slashes.extend(core::iter::repeat_n(b'\\' as u16, 20000));
        assert!(command_line(&[&slashes]).is_err());
    }
    #[test]
    fn leaves_command_interpreter_switches_unquoted() {
        let args: Vec<Vec<u16>> = ["cmd.exe", "/D", "/C", "exit 0"]
            .iter()
            .map(|s| s.encode_utf16().collect())
            .collect();
        let refs: Vec<&[u16]> = args.iter().map(Vec::as_slice).collect();
        let command = command_line(&refs).unwrap();
        assert_eq!(
            alloc::string::String::from_utf16(&command[..command.len() - 1]).unwrap(),
            "cmd.exe /D /C \"exit 0\""
        );
    }
}
