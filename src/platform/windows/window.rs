//! Win32 window lifetime and guarded event dispatch.

use super::*;

/// Win32 may synchronously dispatch nested messages. The guard ensures the
/// same App never acquires overlapping mutable references on those callbacks.
struct WindowState {
    app: UnsafeCell<App>,
    dispatch: session_dispatch::Dispatch,
    native_trace: NativeTrace,
    native_events: NativeEvents,
}
impl WindowState {
    fn with<R>(&self, operation: impl FnOnce(&mut App) -> R) -> Option<R> {
        self.dispatch
            .with(|| operation(unsafe { &mut *self.app.get() }))
    }
}

unsafe fn default_window_proc(
    state: &WindowState,
    hwnd: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let previous = state.native_events.enter_default(message);
    let result = unsafe { ffi::DefWindowProcW(hwnd, message, wparam, lparam) };
    state.native_events.exit_default(message, previous);
    result
}

unsafe extern "system" fn window_proc(
    hwnd: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if message == 0x0081 {
        let state = unsafe { *(lparam as *const *mut WindowState) };
        unsafe {
            ffi::SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
            (*(*state).app.get()).hwnd = hwnd;
        }
        return 1;
    }
    let pointer = unsafe { ffi::GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
    if pointer.is_null() {
        return unsafe { ffi::DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    let state = unsafe { &*pointer };
    let event_sequence = state.native_events.received(message);
    if message == WM_NCCALCSIZE {
        // Copy only scalars while the message parameters are valid. No borrow
        // or pointer into them survives the reentrant default procedure.
        let input = if state.native_events.enabled && wparam != 0 {
            unsafe { (lparam as *const ffi::NcCalcSizeParams).as_ref() }.map(NcCalcStage::capture)
        } else {
            None
        };
        let result = unsafe { default_window_proc(state, hwnd, message, wparam, lparam) };
        if wparam != 0
            && let Some(params) = unsafe { (lparam as *mut ffi::NcCalcSizeParams).as_mut() }
        {
            let after_default = input.map(|_| NcCalcStage::capture(params));
            // Keep default frame/client geometry, but preserve no old client
            // pixels: the retained DIB supplies the resized frame.
            params.rects[1] = ffi::Rect::default();
            params.rects[2] = ffi::Rect::default();
            if let (Some(input), Some(after_default)) = (input, after_default) {
                let submitted = NcCalcStage::capture(params);
                state.native_events.update(|record| {
                    record.nccalc_valid_rects = Some(NcCalcEvent {
                        sequence: event_sequence,
                        wparam,
                        input,
                        default_return: result,
                        after_default,
                        submitted_return: WVR_VALIDRECTS,
                        submitted,
                    });
                });
            }
            return WVR_VALIDRECTS;
        }
        return result;
    }
    if state.native_events.enabled
        && message == 0x0047
        && let Some(position) = unsafe { (lparam as *const ffi::WindowPos).as_ref() }
    {
        state.native_events.update(|record| {
            record.changed = PositionEvent {
                sequence: event_sequence,
                flags_in: position.flags,
                flags_submitted: position.flags,
                position: [position.x, position.y, position.width, position.height],
                current_outer: None,
            }
        });
    }
    if message == WM_WINDOWPOSCHANGING {
        // The retained DIB is the source of every resized frame. Do not also
        // preserve and copy back old window pixels across a size change.
        // Keep default frame/minimum-size processing and every other flag.
        let mut current = ffi::Rect::default();
        if let Some(position) = unsafe { (lparam as *mut ffi::WindowPos).as_mut() } {
            let flags_in = position.flags;
            let mut current_outer = None;
            if position.flags & (SWP_NOSIZE | SWP_NOREDRAW) == 0
                && unsafe { ffi::GetWindowRect(hwnd, &mut current) } != 0
            {
                current_outer = Some([current.left, current.top, current.right, current.bottom]);
                if current.right.saturating_sub(current.left) != position.width
                    || current.bottom.saturating_sub(current.top) != position.height
                {
                    position.flags |= SWP_NOCOPYBITS;
                }
            }
            state.native_events.update(|record| {
                if event_sequence >= record.changing.sequence {
                    record.changing = PositionEvent {
                        sequence: event_sequence,
                        flags_in,
                        flags_submitted: position.flags,
                        position: [position.x, position.y, position.width, position.height],
                        current_outer,
                    };
                }
            });
        }
        return unsafe { default_window_proc(state, hwnd, message, wparam, lparam) };
    }
    if message == WM_NATIVE_PAINT_SAMPLE {
        if state.native_events.enabled
            && state.native_events.paint_diagnostics
            && wparam == NATIVE_SNAPSHOT_MAGIC
        {
            state
                .native_events
                .paint_sample
                .set(Some([lparam as u16 as i32, (lparam >> 16) as u16 as i32]));
            return 1;
        }
        return 0;
    }
    if message == WM_NATIVE_SNAPSHOT && wparam == NATIVE_SNAPSHOT_MAGIC {
        let resize_pending = state.dispatch.resize_pending();
        let paint_pending = state.dispatch.paint_pending();
        let deferred_paints = state.dispatch.deferred_paints();
        return state
            .with(|app| {
                app.failure_snapshot(
                    lparam as u16 as i32,
                    (lparam >> 16) as u16 as i32,
                    resize_pending,
                    paint_pending,
                    deferred_paints,
                    &state.native_events,
                )
            })
            .unwrap_or(0);
    }
    if message == WM_SIZE {
        // Record sent WM_SIZE before trying App so a reentrant request remains
        // pending for WM_PAINT or the outer loop when App is already borrowed.
        state.dispatch.resize_event(wparam == 1);
        state.native_events.update(|record| {
            record.size = [
                wparam as u32,
                lparam as u16 as u32,
                (lparam >> 16) as u16 as u32,
            ];
        });
        if state.native_trace.enabled() {
            let mut client = ffi::Rect::default();
            unsafe {
                ffi::GetClientRect(hwnd, &mut client);
            }
            state.native_trace.record(format_args!(
                "{{\"event\":\"wm-size\",\"ms\":{},\"kind\":{},\"message_size\":[{},{}],\"client\":[{},{}]}}",
                os::milliseconds(), wparam, lparam as u16, (lparam >> 16) as u16,
                client.right, client.bottom));
        }
        if wparam != 1 {
            state.with(|app| {
                if !app.panes.is_empty() && state.dispatch.take_resize() {
                    app.resize();
                }
            });
        }
        return 0;
    }
    let handled = state.with(|app| {
        if app.panes.is_empty() && !matches!(message, WM_PAINT | WM_CLOSE | WM_DESTROY | 0x0082) {
            return Some(0);
        }
        match message {
            WM_PAINT => {
                // A sent resize can precede this paint while the outer loop is
                // still draining messages. Commit its latest client size first.
                if state.dispatch.take_resize() && !app.panes.is_empty() {
                    app.resize();
                }
                app.paint(&state.native_events);
            }
            0x0014 => return Some(1),
            WM_CLOSE => {
                app.running = false;
                return Some(0);
            }
            WM_DESTROY => {
                app.running = false;
                unsafe {
                    ffi::PostQuitMessage(0);
                }
            }
            0x0007 | 0x0008 => {
                app.focused = message == 0x0007;
                app.encoded = input::encode_focus(
                    app.focused,
                    &app.panes[app.active].core.terminal.modes().input(),
                );
                app.enqueue_encoded();
                app.reset_cursor();
                if app.focused {
                    app.ime_position();
                } else {
                    app.held.fill(None);
                    app.intercepted.fill(false);
                    app.cancel_mouse_capture(true);
                }
            }
            WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
                if !app.key(message, wparam, lparam) {
                    return None;
                }
            }
            WM_CHAR | WM_SYSCHAR => app.character(wparam as u16),
            0x0109 => {
                if wparam == 0xffff {
                    return Some(1);
                }
                if let Some(ch) = char::from_u32(wparam as u32) {
                    let mut units = [0u16; 2];
                    for &unit in ch.encode_utf16(&mut units).iter() {
                        app.character(unit);
                    }
                }
            }
            0x0200..=0x020a => app.mouse(message, wparam, lparam),
            0x001f => app.cancel_mouse_capture(true),
            0x0215 => app.cancel_mouse_capture(false),
            0x02e0 => {
                let dpi = wparam as u16 as u32;
                let current = app.pending_fonts.as_ref().unwrap_or(&app.fonts);
                let candidate = Fonts::new(app.surface.dc, &current.description, dpi).ok();
                let rect = unsafe { &*(lparam as *const ffi::Rect) };
                unsafe {
                    ffi::SetWindowPos(
                        hwnd,
                        NULL,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        0x0014,
                    );
                }
                app.resize_with_fonts(candidate);
            }
            0x0082 => {
                unsafe {
                    ffi::SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                return None;
            }
            _ => return None,
        }
        Some(0)
    });
    if message == WM_PAINT && handled.is_none() {
        // Default processing validates the region even though App could not
        // paint. Retain the obligation and re-invalidate outside the guard.
        state.dispatch.defer_paint();
    }
    handled
        .flatten()
        .unwrap_or_else(|| unsafe { default_window_proc(state, hwnd, message, wparam, lparam) })
}

pub fn run(cfg: Config) -> Result<i32, &'static str> {
    unsafe {
        ffi::SetProcessDpiAwarenessContext(-4isize as Handle);
    }
    let native_trace =
        os::environment_value("ZTTY_NATIVE_TRACE").is_some_and(|value| value == [u16::from(b'1')]);
    let paint_diagnostics = os::environment_value("ZTTY_NATIVE_PAINT_DIAGNOSTICS")
        .is_some_and(|value| value == [u16::from(b'1')]);
    let native_snapshot_path = os::environment_value("ZTTY_NATIVE_SNAPSHOT").map(|mut path| {
        path.push(0);
        path
    });
    let native_events = NativeEvents::new(native_snapshot_path.is_some(), paint_diagnostics);
    let program = os::resolve(&cfg.command[0])?;
    let arguments: Vec<Vec<u16>> = cfg
        .command
        .iter()
        .map(|s| s.to_string_lossy().encode_utf16().collect())
        .collect();
    let arguments: Vec<&[u16]> = arguments.iter().map(Vec::as_slice).collect();
    let environment = os::child_environment();
    let pty = zpty::Pty::spawn_utf16(
        &program,
        &arguments,
        Some(&environment),
        zpty::Size::new(cfg.cols as u16, cfg.rows as u16),
    )
    .map_err(|_| "cannot start the Windows pseudoconsole (Windows 10 1809 or newer required)")?;
    let surface = Surface::new()?;
    let fonts = Fonts::new(surface.dc, &cfg.appearance.font, unsafe {
        ffi::GetDpiForSystem()
    })?;
    let width = cfg.cols as i32 * fonts.width + cfg.appearance.padding * 2;
    let height = cfg.rows as i32 * fonts.height + cfg.appearance.padding * 2;
    let mut terminal = Terminal::with_theme(cfg.cols, cfg.rows, cfg.history, cfg.appearance.theme);
    terminal.set_cell_size(fonts.width as u32, fonts.height as u32);
    let wake = session_wait::Wake::new()?;
    let state = Box::new(WindowState {
        app: UnsafeCell::new(App {
            hwnd: NULL,
            panes: vec![Pane::new(
                1,
                pty,
                terminal,
                zpty::Size::new(cfg.cols as u16, cfg.rows as u16),
                Rect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
                &wake,
            )?],
            active: 0,
            layout: Layout::new(1),
            next_id: 2,
            launch: Launch {
                shell: os::default_shell(),
                environment,
                history: cfg.history,
            },
            wake: Arc::clone(&wake),
            pump_cursor: 0,
            pump_again: false,
            pending_fonts: None,
            resize_pending: false,
            resize_retry_at: 0,
            full_redraw: true,
            caption_dirty: true,
            notice: None,
            last_status: 0,
            appearance: cfg.appearance,
            surface,
            fonts,
            encoded: Vec::with_capacity(128),
            running: true,
            held: [None; 256],
            intercepted: [false; 256],
            pending_key: None,
            suppress_char: false,
            surrogate: 0,
            mouse_capture: None,
            cancelled_mouse_buttons: 0,
            title: String::new(),
            focused: false,
            native_trace: NativeTrace::new(native_trace),
            native_snapshot_path,
            surface_resize: None,
            last_paint: PaintRecord::default(),
        }),
        dispatch: session_dispatch::Dispatch::default(),
        native_trace: NativeTrace::new(native_trace),
        native_events,
    });
    let class_name = wide("ztty.NativeWindow");
    let instance = unsafe { ffi::GetModuleHandleW(ptr::null()) };
    let class = ffi::WindowClass {
        size: core::mem::size_of::<ffi::WindowClass>() as u32,
        style: 3,
        procedure: Some(window_proc),
        class_extra: 0,
        window_extra: 0,
        instance,
        icon: NULL,
        cursor: unsafe { ffi::LoadCursorW(NULL, 32513usize as *const u16) },
        background: NULL,
        menu: ptr::null(),
        name: class_name.as_ptr(),
        small_icon: NULL,
    };
    if unsafe { ffi::RegisterClassExW(&class) } == 0 {
        return Err("cannot register the native window class");
    }
    let mut rect = ffi::Rect {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    unsafe {
        ffi::AdjustWindowRectEx(&mut rect, WINDOW_STYLE, 0, 0);
    }
    let hwnd = unsafe {
        ffi::CreateWindowExW(
            0,
            class_name.as_ptr(),
            wide("ztty").as_ptr(),
            WINDOW_STYLE,
            i32::MIN,
            i32::MIN,
            rect.right - rect.left,
            rect.bottom - rect.top,
            NULL,
            NULL,
            instance,
            (&*state as *const WindowState).cast_mut().cast(),
        )
    };
    if hwnd.is_null() {
        unsafe {
            ffi::UnregisterClassW(class_name.as_ptr(), instance);
        }
        return Err("cannot create a native Windows window");
    }
    let dark = 1i32;
    // Keep standard Win32 decorations while isolating the retained GDI client
    // from the separate DWM nonclient rendering path.
    let nonclient_policy = 1i32; // DWMNCRP_DISABLED
    unsafe {
        ffi::DwmSetWindowAttribute(hwnd, 20, (&dark as *const i32).cast(), 4);
        if ffi::DwmSetWindowAttribute(hwnd, 2, (&nonclient_policy as *const i32).cast(), 4) < 0 {
            os::print(2, b"ztty: native window frame policy unavailable\n");
        }
        ffi::ShowWindow(hwnd, 5);
    }
    state.with(App::resize);
    loop {
        let mut message: ffi::Message = unsafe { core::mem::zeroed() };
        let mut quit = false;
        for _ in 0..512 {
            if unsafe { ffi::PeekMessageW(&mut message, NULL, 0, 0, 1) } == 0 {
                break;
            }
            if message.message == 0x0012 {
                quit = true;
                break;
            }
            unsafe {
                ffi::TranslateMessage(&message);
                ffi::DispatchMessageW(&message);
            }
        }
        if quit {
            break;
        }
        let running = state
            .with(|app| {
                if state.dispatch.take_resize() && !app.panes.is_empty() {
                    app.resize();
                }
                app.tick();
                app.running
            })
            .unwrap_or(false);
        if !running {
            break;
        }
        // Consume before entering Win32; a reentrant paint creates a fresh
        // obligation for the next turn. Never call paint through an App borrow.
        if state.dispatch.take_paint() && unsafe { ffi::InvalidateRect(hwnd, ptr::null(), 0) } == 0
        {
            state.dispatch.retry_paint();
        }
        // The wake mask includes existing paint work; PeekMessage dispatches it.
        let timeout = state.with(|app| app.wait_timeout()).unwrap_or(8);
        let timeout = if state.dispatch.resize_pending() || state.dispatch.paint_pending() {
            0
        } else {
            timeout
        };
        if let Err(error) = wake.wait(timeout) {
            state.with(|app| app.message(error));
            break;
        }
    }
    let status = state
        .with(|app| {
            if let Some(pane) = app.panes.get_mut(app.active) {
                let _ = pane.core.pty.wait_timeout(20);
                pane.core
                    .pty
                    .status()
                    .and_then(|status| status.code())
                    .unwrap_or(app.last_status)
            } else {
                app.last_status
            }
        })
        .unwrap_or(0);
    unsafe {
        ffi::DestroyWindow(hwnd);
        ffi::UnregisterClassW(class_name.as_ptr(), instance);
    }
    Ok(status)
}
