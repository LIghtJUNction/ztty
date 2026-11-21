//! Bounded terminal snapshots and row-delta VT output for remote TTY views.
//!
//! Child output is parsed by the broker once. This projection emits display
//! cells and input modes, so device queries are answered only by that broker.
use crate::json::{self, Value};
use alloc::{format, string::String, vec::Vec};
use core::fmt::Write;
use zterm::{
    BLINK, BOLD, Cell, CursorShape, DIM, HIDDEN, INVERSE, ITALIC, Modes, RAPID_BLINK, STRIKE,
    Terminal, UNDERLINE,
};

/// Leave room for the broker's identity, cursor and response envelope.
pub const MAX_FRAME_BYTES: usize = 480 * 1024;
pub const MAX_HISTORY_ROWS: usize = 2000;
const STYLE_FLAGS: u16 =
    BOLD | DIM | ITALIC | UNDERLINE | INVERSE | STRIKE | HIDDEN | BLINK | RAPID_BLINK;

fn quoted_size(text: &str) -> usize {
    2 + text
        .bytes()
        .map(|byte| match byte {
            b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 8 | 12 => 2,
            0..=31 => 6,
            _ => 1,
        })
        .sum::<usize>()
}

fn safe_text(text: &str, limit: usize) -> String {
    let mut result = String::new();
    for ch in text.chars().filter(|ch| !ch.is_control()) {
        if result.len() + ch.len_utf8() > limit {
            break;
        }
        result.push(ch);
    }
    result
}

fn style(output: &mut String, cell: &Cell) {
    output.push_str("\x1b[0");
    for (flag, code) in [
        (BOLD, 1),
        (DIM, 2),
        (ITALIC, 3),
        (UNDERLINE, 4),
        (BLINK, 5),
        (RAPID_BLINK, 6),
        (INVERSE, 7),
        (HIDDEN, 8),
        (STRIKE, 9),
    ] {
        if cell.flags & flag != 0 {
            let _ = write!(output, ";{code}");
        }
    }
    let _ = write!(
        output,
        ";38;2;{};{};{};48;2;{};{};{}m",
        (cell.fg >> 16) & 255,
        (cell.fg >> 8) & 255,
        cell.fg & 255,
        (cell.bg >> 16) & 255,
        (cell.bg >> 8) & 255,
        cell.bg & 255,
    );
}

fn row(
    terminal: &Terminal,
    cols: usize,
    y: usize,
    offset: usize,
    limit: usize,
) -> Result<String, String> {
    let mut result = String::new();
    let mut previous_style = None;
    let mut previous_link = None;
    for x in 0..cols {
        let cell = terminal.view_cell(x, y, offset);
        if cell.width == 0 {
            continue;
        }
        let current_style = (cell.fg, cell.bg, cell.flags & STYLE_FLAGS);
        if previous_style != Some(current_style) {
            style(&mut result, cell);
            previous_style = Some(current_style);
        }
        let clipped_wide = cell.width == 2 && x + 1 >= cols;
        let link = terminal.cell_hyperlink(cell).filter(|link| {
            !clipped_wide && link.len() <= 4096 && !link.chars().any(char::is_control)
        });
        if link != previous_link {
            result.push_str("\x1b]8;;");
            if let Some(link) = link {
                result.push_str(link);
            }
            result.push_str("\x1b\\");
            previous_link = link;
        }
        if clipped_wide {
            // An observer's right edge must never display half a wide cluster
            // or let it spill onto the next physical terminal row.
            result.push(' ');
        } else {
            for ch in terminal.cell_chars(cell) {
                // VT controls never form ordinary terminal cells. Keep this guard
                // for restored state and graphics placeholder content as well.
                result.push(if ch.is_control() { ' ' } else { ch });
            }
        }
        if result.len() > limit {
            return Err("remote terminal frame exceeds its bounded size".into());
        }
    }
    if previous_link.is_some() {
        result.push_str("\x1b]8;;\x1b\\");
    }
    result.push_str("\x1b[0m");
    if quoted_size(&result) > limit {
        return Err("remote terminal frame exceeds its bounded size".into());
    }
    Ok(result)
}

fn modes_value(modes: &Modes) -> Value {
    let shape = match modes.cursor_shape {
        None => 0usize,
        Some(CursorShape::Block) => 1,
        Some(CursorShape::Underline) => 2,
        Some(CursorShape::Bar) => 3,
    };
    json::object([
        ("app_cursor", modes.app_cursor.into()),
        ("app_keypad", modes.app_keypad.into()),
        ("cursor_visible", modes.cursor_visible.into()),
        (
            "cursor_blink",
            modes.cursor_blink.map(Value::from).unwrap_or(Value::Null),
        ),
        ("cursor_shape", shape.into()),
        ("bracketed_paste", modes.bracketed_paste.into()),
        ("focus_events", modes.focus_events.into()),
        ("mouse_tracking", modes.mouse_tracking.into()),
        ("sgr_mouse", modes.sgr_mouse.into()),
        ("kitty_keyboard", usize::from(modes.kitty_keyboard).into()),
    ])
}

/// Capture the visible cells and newest retained primary-screen history.
/// `history_since` is the previous primary frame's history position; `None`
/// requests a reconnect snapshot. History is clipped before visible content.
pub fn capture(terminal: &Terminal, history_since: Option<u64>) -> Result<Value, String> {
    capture_view(terminal, history_since, terminal.cols(), terminal.rows())
}

/// Project an independent viewer's top-left viewport without resizing the PTY.
/// Smaller viewers clip rows/columns; larger viewers receive blank padding.
/// The cursor is clamped to the visible edge and history uses the same width.
pub fn capture_view(
    terminal: &Terminal,
    history_since: Option<u64>,
    cols: usize,
    rows: usize,
) -> Result<Value, String> {
    if !(2..=512).contains(&cols) || !(2..=256).contains(&rows) {
        return Err("remote terminal view must be 2..512 columns by 2..256 rows".into());
    }
    let mut remaining = MAX_FRAME_BYTES - 4096;
    let mut lines = Vec::with_capacity(rows);
    for y in 0..rows {
        let line = row(terminal, cols, y, 0, remaining)?;
        remaining = remaining.saturating_sub(quoted_size(&line) + 1);
        lines.push(Value::from(line));
    }
    let position = terminal.history_position();
    let available = terminal.history_len().min(MAX_HISTORY_ROWS);
    let first = position.saturating_sub(available as u64);
    let requested = history_since
        .filter(|since| *since <= position)
        .unwrap_or_else(|| position.saturating_sub(terminal.history_len() as u64));
    let start = requested.max(first);
    let wanted = position.saturating_sub(start).min(available as u64) as usize;
    let mut history = Vec::new();
    // The newest rows win when unusual styles/links fill the wire budget.
    for offset in 1..=wanted {
        let Ok(line) = row(terminal, cols, 0, offset, remaining) else {
            break;
        };
        remaining = remaining.saturating_sub(quoted_size(&line) + 1);
        history.push(Value::from(line));
    }
    history.reverse();
    let history_start = position.saturating_sub(history.len() as u64);
    let (x, y) = terminal.cursor();
    Ok(json::object([
        ("cols", cols.into()),
        ("rows", rows.into()),
        ("lines", Value::Array(lines)),
        ("cursor_x", x.min(cols - 1).into()),
        ("cursor_y", y.min(rows - 1).into()),
        ("modes", modes_value(terminal.modes())),
        ("alternate", terminal.modes().alternate_screen.into()),
        ("history_position", position.into()),
        ("history_start", history_start.into()),
        ("history", Value::Array(history)),
        ("history_truncated", (history_start > requested).into()),
        ("title", safe_text(terminal.title(), 1024).into()),
    ]))
}

fn number(value: &Value, name: &str) -> Result<u64, String> {
    value
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("invalid terminal frame {name}"))
}

fn boolean(value: &Value, name: &str) -> Result<bool, String> {
    value
        .get(name)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("invalid terminal frame {name}"))
}

fn strings<'a>(
    value: &'a Value,
    name: &str,
    count: usize,
    exact: bool,
    budget: &mut usize,
) -> Result<Vec<&'a str>, String> {
    let array = value
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("invalid terminal frame {name}"))?;
    if array.len() > count || (exact && array.len() != count) {
        return Err(format!("invalid terminal frame {name} length"));
    }
    let mut lines = Vec::with_capacity(array.len());
    for value in array {
        let text = value.as_str().ok_or("invalid terminal frame row")?;
        let size = quoted_size(text) + 1;
        *budget = budget
            .checked_sub(size)
            .ok_or("remote terminal frame exceeds its bounded size")?;
        lines.push(text);
    }
    Ok(lines)
}

fn mode_wire(value: &Value) -> Result<String, String> {
    let modes = value.get("modes").ok_or("missing terminal frame modes")?;
    let mut output = String::new();
    for (name, mode) in [
        ("app_cursor", 1),
        ("cursor_visible", 25),
        ("bracketed_paste", 2004),
        ("focus_events", 1004),
        ("sgr_mouse", 1006),
    ] {
        let end = if boolean(modes, name)? { 'h' } else { 'l' };
        let _ = write!(output, "\x1b[?{mode}{end}");
    }
    output.push_str(if boolean(modes, "app_keypad")? {
        "\x1b="
    } else {
        "\x1b>"
    });
    let tracking = number(modes, "mouse_tracking")?;
    if ![0, 9, 1000, 1002, 1003].contains(&tracking) {
        return Err("invalid terminal mouse mode".into());
    }
    output.push_str("\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l");
    if tracking != 0 {
        let _ = write!(output, "\x1b[?{tracking}h");
    }
    let keyboard = number(modes, "kitty_keyboard")?;
    if keyboard > 31 {
        return Err("invalid terminal keyboard mode".into());
    }
    let _ = write!(output, "\x1b[={keyboard}u");
    let blink = match modes.get("cursor_blink") {
        Some(Value::Null) => None,
        Some(value) => Some(value.as_bool().ok_or("invalid terminal cursor blink")?),
        None => return Err("missing terminal cursor blink".into()),
    };
    let shape = number(modes, "cursor_shape")?;
    if shape > 3 {
        return Err("invalid terminal cursor shape".into());
    }
    let shape = if shape == 0 {
        0
    } else {
        shape * 2 - u64::from(blink.unwrap_or(true))
    };
    let _ = write!(output, "\x1b[{shape} q");
    if let Some(blink) = blink {
        let _ = write!(output, "\x1b[?12{}", if blink { 'h' } else { 'l' });
    }
    Ok(output)
}

/// Retains one viewport. Repeated idle frames emit no bytes; changing one row
/// resends that row, not the entire terminal or raw terminal-query traffic.
#[derive(Default)]
pub struct Renderer {
    cols: usize,
    rows: usize,
    alternate: bool,
    lines: Vec<String>,
    cursor: Option<(usize, usize)>,
    modes: String,
    title: String,
    history_position: Option<u64>,
}

impl Renderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn render(&mut self, value: &Value) -> Result<Vec<u8>, String> {
        let cols = number(value, "cols")? as usize;
        let rows = number(value, "rows")? as usize;
        if !(1..=512).contains(&cols) || !(1..=256).contains(&rows) {
            return Err("invalid terminal frame dimensions".into());
        }
        let x = number(value, "cursor_x")? as usize;
        let y = number(value, "cursor_y")? as usize;
        if x >= cols || y >= rows {
            return Err("invalid terminal frame cursor".into());
        }
        let alternate = boolean(value, "alternate")?;
        let position = number(value, "history_position")?;
        let history_start = number(value, "history_start")?;
        let mut budget = MAX_FRAME_BYTES - 4096;
        let lines = strings(value, "lines", rows, true, &mut budget)?;
        let history = strings(value, "history", MAX_HISTORY_ROWS, false, &mut budget)?;
        if history_start.checked_add(history.len() as u64) != Some(position)
            || (alternate && !history.is_empty())
        {
            return Err("invalid terminal frame history".into());
        }
        let modes = mode_wire(value)?;
        let title = value
            .get("title")
            .and_then(Value::as_str)
            .ok_or("invalid terminal title")?;
        if title.len() > 1024 || title.chars().any(char::is_control) {
            return Err("invalid terminal title".into());
        }
        let first = self.rows == 0;
        let resized = self.cols != cols || self.rows != rows;
        let switched = self.alternate != alternate;
        let reset_history = !alternate
            && (resized
                || self.history_position.is_none()
                || self
                    .history_position
                    .is_some_and(|previous| previous > position));
        let since = if reset_history {
            history_start
        } else {
            self.history_position.unwrap_or(history_start)
        };
        let skip = since
            .saturating_sub(history_start)
            .min(history.len() as u64) as usize;
        let appended = &history[skip..];
        let changed = first
            || resized
            || switched
            || reset_history
            || !appended.is_empty()
            || self.cursor != Some((x, y))
            || self.modes != modes
            || self.title != title
            || self
                .lines
                .iter()
                .map(String::as_str)
                .ne(lines.iter().copied());
        if !changed {
            return Ok(Vec::new());
        }

        let mut output = String::new();
        output.push_str("\x1b[?2026h");
        if first || switched {
            output.push_str(if alternate {
                "\x1b[?1049h"
            } else {
                "\x1b[?1049l"
            });
        }
        // A prior interactive program can leave its own charset, margins or
        // insert mode behind. Apply these after an alternate-screen restore.
        output.push_str("\x1b[?25l\x1b[?7l\x1b[?6l\x1b[r\x1b[4l\x1b[20l\x1b(B\x1b)B\x0f\x1b[0m");
        if first || resized || switched || reset_history {
            output.push_str("\x1b[2J\x1b[H");
            self.lines.clear();
        }
        self.lines.resize_with(rows, String::new);
        if reset_history {
            output.push_str("\x1b[3J\x1b[H");
            for line in &history {
                output.push_str(line);
                output.push_str("\r\n");
            }
            for (index, line) in lines.iter().enumerate() {
                output.push_str(line);
                if index + 1 < rows {
                    output.push_str("\r\n");
                }
            }
            for (saved, line) in self.lines.iter_mut().zip(&lines) {
                saved.clear();
                saved.push_str(line);
            }
        } else if !alternate {
            for line in appended {
                if self.lines[0] != *line {
                    output.push_str("\x1b[1;1H");
                    output.push_str(line);
                }
                let _ = writeln!(output, "\x1b[{rows};1H");
                self.lines.rotate_left(1);
                self.lines[rows - 1].clear();
            }
        }
        for (index, (saved, line)) in self.lines.iter_mut().zip(&lines).enumerate() {
            if saved.as_str() != *line {
                let _ = write!(output, "\x1b[{};1H", index + 1);
                output.push_str(line);
                saved.clear();
                saved.push_str(line);
            }
        }
        output.push_str("\x1b[0m\x1b[?7h");
        if self.title != title || first {
            output.push_str("\x1b]2;");
            output.push_str(title);
            output.push_str("\x1b\\");
        }
        let _ = write!(output, "\x1b[{};{}H", y + 1, x + 1);
        // Cursor visibility is included even if the modes did not change: it
        // was temporarily hidden at the start of this synchronized repaint.
        output.push_str(&modes);
        output.push_str("\x1b[?2026l");
        self.cols = cols;
        self.rows = rows;
        self.alternate = alternate;
        self.cursor = Some((x, y));
        self.modes = modes;
        self.title.clear();
        self.title.push_str(title);
        if !alternate {
            self.history_position = Some(position);
        }
        Ok(output.into_bytes())
    }
}
