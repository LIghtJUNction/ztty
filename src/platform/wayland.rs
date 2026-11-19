//! Explicit experimental single-window Wayland host.
//! The X11 broker remains the default and keeps shared-session/MCP ownership.
//! This host binds xdg-shell and uses the common PTY/parser/input/GPU components.
use crate::{
    Config, appearance::RendererKind, key, os, queue_input, working_directory::WorkingDirectory,
};
use alloc::{ffi::CString, format, string::String, vec::Vec};
use core::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr::NonNull,
};
use zterm::{
    Terminal,
    input::{self, Key, KeyEvent, Modifiers, MouseEvent},
};
use ztty::{
    gpu::{Display, GridStyle, Renderer, Target},
    session_core::{ReadStep, SessionCore},
    text_blink::TextBlink,
};

#[repr(C)]
struct Event {
    kind: u32,
    symbol: u32,
    modifiers: u32,
    state: u32,
    x: i32,
    y: i32,
    text: [u8; 64],
}
impl Default for Event {
    fn default() -> Self {
        Self {
            kind: 0,
            symbol: 0,
            modifiers: 0,
            state: 0,
            x: 0,
            y: 0,
            text: [0; 64],
        }
    }
}
unsafe extern "C" {
    fn ztty_wl_create(width: i32, height: i32) -> *mut c_void;
    fn ztty_wl_destroy(window: *mut c_void);
    fn ztty_wl_display(window: *mut c_void) -> *mut c_void;
    fn ztty_wl_window(window: *mut c_void) -> usize;
    fn ztty_wl_next(window: *mut c_void, event: *mut Event) -> c_int;
    fn ztty_wl_resize(window: *mut c_void, width: i32, height: i32) -> c_int;
    fn ztty_wl_title(window: *mut c_void, title: *const c_char);
    fn ztty_wl_ready(window: *mut c_void) -> c_int;
    fn ztty_wl_request_frame(window: *mut c_void) -> c_int;
    fn ztty_wl_pump(window: *mut c_void, fd: c_int, write: c_int, timeout: c_int) -> c_int;
}
struct Native(NonNull<c_void>);
impl Native {
    fn ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            ztty_wl_destroy(self.ptr());
        }
    }
}
struct OwnedSession(SessionCore);
impl Drop for OwnedSession {
    fn drop(&mut self) {
        // The only process group signalled is the PTY session we created.
        // A lost compositor must not leave a live interactive shell behind.
        if self.0.pty.status().is_none() {
            let _ = self.0.pty.hangup();
            if self.0.pty.wait_timeout(200).ok().flatten().is_none() {
                unsafe {
                    os::kill(-self.0.pty.pid(), 9);
                }
                let _ = self.0.pty.wait_timeout(200);
            }
        }
    }
}
fn size(width: u32, height: u32, padding: u32, cw: u32, ch: u32) -> zpty::Size {
    zpty::Size {
        cols: ((width.saturating_sub(2 * padding)) / cw.max(1)).clamp(2, 512) as u16,
        rows: ((height.saturating_sub(2 * padding)) / ch.max(1)).clamp(2, 256) as u16,
        pixel_width: width as u16,
        pixel_height: height as u16,
    }
}
fn timeout(now: u64, deadline: Option<u64>, fallback: i32) -> i32 {
    deadline.map_or(fallback, |t| {
        t.saturating_sub(now).min(fallback as u64) as i32
    })
}
fn pointer_cell(
    event: &Event,
    core: &SessionCore,
    padding: u32,
    cw: u32,
    ch: u32,
) -> (usize, usize) {
    let col = (event.x.max(0) as u32).saturating_sub(padding) / cw.max(1);
    let row = (event.y.max(0) as u32).saturating_sub(padding) / ch.max(1);
    (
        (col as usize).min(core.terminal.cols() - 1),
        (row as usize).min(core.terminal.rows() - 1),
    )
}
fn button(raw: u32) -> Option<u8> {
    match raw {
        272 => Some(0),
        273 => Some(2),
        274 => Some(1),
        275..=278 => Some((raw - 268) as u8),
        _ => None,
    }
}
pub fn run(cfg: Config) -> Result<i32, String> {
    if cfg.observe || cfg.target.is_some() || cfg.name.is_some() {
        return Err("the experimental Wayland backend owns one standalone PTY; shared-session options require --backend x11".into());
    }
    if !cfg.appearance.text_plugins.is_empty() || !cfg.appearance.bindings.is_empty() {
        return Err("text plugins and custom bindings are not yet supported by the experimental Wayland backend".into());
    }
    if cfg.appearance.renderer == RendererKind::Software {
        return Err("the Wayland backend requires renderer=auto or renderer=gpu (EGL/GLES)".into());
    }
    unsafe {
        os::setlocale(0, c"".as_ptr());
    }
    let padding = cfg.appearance.padding.max(0) as u32;
    let initial_width = cfg.cols as u32 * 9 + padding * 2;
    let initial_height = cfg.rows as u32 * 18 + padding * 2;
    let native = Native(
        NonNull::new(unsafe { ztty_wl_create(initial_width as i32, initial_height as i32) })
            .ok_or("cannot connect to a Wayland compositor with xdg-shell")?,
    );
    // Prepare and fork before EGL/font drivers can start worker threads.
    let directory = cfg
        .cwd
        .as_deref()
        .map(|path| CString::new(path).map_err(|_| String::from("cwd contains NUL")))
        .transpose()?;
    let saved_directory = directory
        .as_deref()
        .map(WorkingDirectory::enter)
        .transpose()?;
    let mut environment = os::environment();
    environment.retain(|entry| {
        !entry.to_bytes().starts_with(b"PWD=")
            && !entry.to_bytes().starts_with(b"ZTTY_SOCKET=")
            && !entry.to_bytes().starts_with(b"ZTTY_SESSION=")
    });
    let mut cwd = [0u8; 4096];
    if unsafe { os::getcwd(cwd.as_mut_ptr().cast(), cwd.len()) }.is_null() {
        return Err("cannot resolve command working directory".into());
    }
    let actual = CStr::from_bytes_until_nul(&cwd).map_err(|_| "working directory is too long")?;
    let mut pwd = b"PWD=".to_vec();
    pwd.extend_from_slice(actual.to_bytes());
    environment.push(CString::new(pwd).map_err(|_| "invalid working directory")?);
    let (program, environment) = os::child_setup_with_env(&cfg.command[0], environment)?;
    let argv: Vec<&CStr> = cfg.command.iter().map(CString::as_c_str).collect();
    let child_env: Vec<&CStr> = environment.iter().map(CString::as_c_str).collect();
    // Wayland's display fd is CLOEXEC; no display objects or GPU threads exist in
    // the exec'd child. The parent is still single-threaded at this point.
    let pty = unsafe {
        zpty::Pty::spawn(
            &program,
            &argv,
            &child_env,
            zpty::Size::new(cfg.cols as u16, cfg.rows as u16),
            &[],
        )
    }
    .map_err(|error| format!("cannot create Wayland PTY: {error}"))?;
    drop(saved_directory);
    let mut owned = OwnedSession(SessionCore::new(
        pty,
        Terminal::with_theme(cfg.cols, cfg.rows, cfg.history, cfg.appearance.theme),
        String::from("wayland-local"),
        1024 * 1024,
    ));
    let core = &mut owned.0;
    let display = unsafe { Display::wayland(ztty_wl_display(native.ptr())) }?;
    let mut renderer = Renderer::new(
        display,
        Target::Wayland {
            window: unsafe { ztty_wl_window(native.ptr()) },
        },
        initial_width,
        initial_height,
        &cfg.appearance.font,
    )?;
    let metrics = renderer.metrics();
    let cw = metrics.cell_width.max(1.0) as u32;
    let ch = metrics.cell_height.max(1.0) as u32;
    core.terminal.set_cell_size(cw, ch);
    let mut offset = 0usize;
    let mut focused = false;
    let mut configured = false;
    let mut repaint = true;
    let mut pointer_button = None;
    let mut last_title = String::new();
    let mut last_frame = 0u64;
    let mut cursor_epoch = os::milliseconds();
    let mut last_cursor_visible = true;
    let mut sync_since = None;
    let mut blink = TextBlink::default();
    let mut output = [0u8; 16 * 1024];
    let mut event = Event::default();
    let started = os::milliseconds();
    loop {
        while unsafe { ztty_wl_next(native.ptr(), &mut event) } != 0 {
            match event.kind {
                1 => {
                    if unsafe { ztty_wl_resize(native.ptr(), event.x, event.y) } < 0 {
                        return Err(
                            "Wayland configure exceeds the 16-megapixel frame budget".into()
                        );
                    }
                    let dimensions = size(event.x as u32, event.y as u32, padding, cw, ch);
                    // Preserve the history viewport's text anchor across reflow.
                    let position = core.terminal.view_position(0, 0, offset);
                    let mut anchors = [zterm::ResizeAnchor::cell(position)];
                    core.resize_with_cell_size_anchored(dimensions, (cw, ch), &mut anchors)
                        .map_err(|error| format!("cannot resize Wayland PTY: {error}"))?;
                    if offset > 0 {
                        offset = anchors[0]
                            .position
                            .map_or(0, |p| {
                                core.terminal.history_position().saturating_sub(p.line) as usize
                            })
                            .min(core.terminal.history_len());
                    }
                    renderer.resize(event.x as u32, event.y as u32)?;
                    configured = true;
                    repaint = true;
                }
                2 => {
                    let length = event
                        .text
                        .iter()
                        .position(|b| *b == 0)
                        .unwrap_or(event.text.len());
                    let text = core::str::from_utf8(&event.text[..length]).unwrap_or("");
                    let mods = Modifiers(event.modifiers as u8);
                    let key = key(u64::from(event.symbol));
                    let transition = match event.state {
                        0 => KeyEvent::Release,
                        2 => KeyEvent::Repeat,
                        _ => KeyEvent::Press,
                    };
                    let scroll_key = mods.contains(Modifiers::SHIFT)
                        && matches!(key, Key::PageUp | Key::PageDown);
                    if scroll_key {
                        if transition != KeyEvent::Release {
                            let step = core.terminal.rows().saturating_sub(1).max(1);
                            offset = if key == Key::PageUp {
                                offset.saturating_add(step).min(core.terminal.history_len())
                            } else {
                                offset.saturating_sub(step)
                            };
                            repaint = true;
                        }
                    } else {
                        let bytes = input::encode_key(
                            key,
                            mods,
                            transition,
                            text,
                            &core.terminal.modes().input(),
                        );
                        if !bytes.is_empty() {
                            queue_input(&mut core.input, &bytes);
                            offset = 0;
                            repaint = true;
                            cursor_epoch = os::milliseconds();
                        }
                    }
                }
                3 => {
                    focused = event.state != 0;
                    let bytes = input::encode_focus(focused, &core.terminal.modes().input());
                    queue_input(&mut core.input, &bytes);
                    pointer_button = None;
                    cursor_epoch = os::milliseconds();
                    repaint = true;
                }
                4 | 7 => {
                    let (col, row) = pointer_cell(&event, core, padding, cw, ch);
                    let mouse = if event.kind == 7 {
                        Some(MouseEvent::Motion {
                            button: pointer_button,
                            col,
                            row,
                        })
                    } else {
                        button(event.symbol).map(|button| {
                            if event.state != 0 {
                                pointer_button = Some(button);
                                MouseEvent::Press { button, col, row }
                            } else {
                                pointer_button = None;
                                MouseEvent::Release { button, col, row }
                            }
                        })
                    };
                    if !Modifiers(event.modifiers as u8).contains(Modifiers::SHIFT)
                        && let Some(mouse) = mouse
                    {
                        let bytes = input::encode_mouse(
                            mouse,
                            Modifiers(event.modifiers as u8),
                            &core.terminal.modes().input(),
                        );
                        queue_input(&mut core.input, &bytes);
                    }
                }
                5 => {
                    let steps = event.state as i32;
                    if core.terminal.modes().mouse_tracking == 0
                        || Modifiers(event.modifiers as u8).contains(Modifiers::SHIFT)
                    {
                        let amount = steps.unsigned_abs() as usize * 3;
                        offset = if steps < 0 {
                            offset
                                .saturating_add(amount)
                                .min(core.terminal.history_len())
                        } else {
                            offset.saturating_sub(amount)
                        };
                        repaint = true;
                    } else {
                        let (col, row) = pointer_cell(&event, core, padding, cw, ch);
                        for _ in 0..steps.unsigned_abs().min(64) {
                            let bytes = input::encode_mouse(
                                MouseEvent::Scroll {
                                    up: steps < 0,
                                    col,
                                    row,
                                },
                                Modifiers(event.modifiers as u8),
                                &core.terminal.modes().input(),
                            );
                            queue_input(&mut core.input, &bytes);
                        }
                    }
                }
                6 => return Ok(0),
                _ => {}
            }
        }
        // Bound each drain so sustained PTY output cannot starve configure/input.
        for _ in 0..4 {
            match core
                .read_once(&mut output, false)
                .map_err(|error| format!("Wayland PTY read: {error}"))?
            {
                ReadStep::Data {
                    history_after,
                    history_scrolled,
                    ..
                } => {
                    if offset > 0 {
                        offset = offset.saturating_add(history_scrolled).min(history_after);
                    }
                    repaint = true;
                }
                ReadStep::Pending | ReadStep::Eof => break,
            }
        }
        core.queue_responses();
        core.flush_input()
            .map_err(|error| format!("Wayland PTY write: {error}"))?;
        let now = os::milliseconds();
        let synchronized = core.terminal.modes().synchronized_output;
        if synchronized {
            let since = sync_since.get_or_insert(now);
            if now.saturating_sub(*since) >= 250 {
                core.terminal.force_end_sync();
                sync_since = None;
            }
        } else {
            sync_since = None;
        }
        let synchronized = core.terminal.modes().synchronized_output;
        let tick = blink.update(&core.terminal, offset, repaint, focused, synchronized, now);
        repaint |= tick.changed;
        let modes = core.terminal.modes();
        let cursor_blinks = focused
            && modes.cursor_visible
            && offset == 0
            && modes.cursor_blink.unwrap_or(cfg.appearance.cursor_blink)
            && !synchronized;
        let interval = cfg.appearance.cursor_blink_interval.max(100);
        let cursor_visible =
            !cursor_blinks || (now.saturating_sub(cursor_epoch) / interval).is_multiple_of(2);
        if cursor_visible != last_cursor_visible {
            repaint = true;
            last_cursor_visible = cursor_visible;
        }
        if core.terminal.title() != last_title {
            last_title = String::from(core.terminal.title());
            if let Ok(title) = CString::new(if last_title.is_empty() {
                "ztty"
            } else {
                &last_title
            }) {
                unsafe {
                    ztty_wl_title(native.ptr(), title.as_ptr());
                }
            }
        }
        let can_draw = !synchronized
            && configured
            && now.saturating_sub(last_frame) >= 8
            && unsafe { ztty_wl_ready(native.ptr()) } != 0;
        if repaint && can_draw {
            let cursor = (modes.cursor_visible && cursor_visible && offset == 0).then_some((
                core.terminal.cursor(),
                modes.cursor_shape.unwrap_or(cfg.appearance.cursor_shape),
            ));
            renderer.draw_terminal(
                &core.terminal,
                offset,
                GridStyle {
                    padding: padding as f32,
                    cell_width: cw as f32,
                    cell_height: ch as f32,
                    ascent: metrics.ascent,
                    background: cfg.appearance.theme.background,
                    background_alpha: (u16::from(cfg.appearance.opacity) * 255 / 100) as u8,
                    selection_foreground: cfg.appearance.selection_foreground,
                    selection_background: cfg.appearance.selection_background,
                    cursor,
                    cursor_color: cfg.appearance.cursor_color,
                    slow_blink_visible: tick.visible[0],
                    rapid_blink_visible: tick.visible[1],
                },
                |_, _| false,
            )?;
            if unsafe { ztty_wl_request_frame(native.ptr()) } < 0 {
                return Err("cannot request a Wayland frame callback".into());
            }
            renderer.present()?;
            core.terminal.clear_dirty();
            repaint = false;
            last_frame = now;
        }
        // Continue to drain buffered output after the process is reaped.
        let status = core
            .pty
            .reap()
            .map_err(|error| format!("Wayland child wait: {error}"))?;
        if core.eof()
            && let Some(status) = status
        {
            return Ok(status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)));
        }
        if !configured && now.saturating_sub(started) >= 5000 {
            return Err("Wayland compositor did not configure the window within 5 seconds".into());
        }
        let mut wait = timeout(
            now,
            tick.deadline,
            if core.eof() || !configured {
                100
            } else {
                60_000
            },
        );
        if cursor_blinks {
            let next = cursor_epoch.saturating_add(
                (now.saturating_sub(cursor_epoch) / interval + 1).saturating_mul(interval),
            );
            wait = timeout(now, Some(next), wait);
        }
        if let Some(since) = sync_since {
            wait = timeout(now, Some(since + 250), wait);
        }
        if repaint && configured && !synchronized && unsafe { ztty_wl_ready(native.ptr()) } != 0 {
            wait = timeout(now, Some(last_frame + 8), wait);
        }
        if unsafe {
            ztty_wl_pump(
                native.ptr(),
                if core.eof() { -1 } else { core.pty.fd() },
                core.input.pending() as c_int,
                wait,
            )
        } < 0
        {
            return Err("Wayland connection closed or its bounded input queue overflowed".into());
        }
    }
}
