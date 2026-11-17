use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::{self, Write};
use core::mem;
use unicode_width::UnicodeWidthChar;
use zvte::{Params, Parser, Perform};

use crate::screen::*;

const MAX_HISTORY_BYTES: usize = 8 * 1024 * 1024;
const MAX_LINKS: usize = 1024;
const MAX_RESPONSES: usize = 65536;

pub const ANSI_PALETTE: [u32; 16] = [
    0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5, 0x7f7f7f,
    0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
];

/// Configured foreground, background, and ANSI colors (24-bit RGB).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub foreground: u32,
    pub background: u32,
    pub palette: [u32; 16],
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            foreground: DEFAULT_FG,
            background: DEFAULT_BG,
            palette: ANSI_PALETTE,
        }
    }
}

impl Theme {
    fn style(self) -> Style {
        Style {
            fg: self.foreground,
            bg: self.background,
            ..Style::default()
        }
    }

    fn color(self, index: u16) -> u32 {
        self.palette
            .get(index as usize)
            .copied()
            .unwrap_or_else(|| extended_color(index))
    }

    fn recolor(&self, fg: &mut u32, bg: &mut u32, flags: u16, indices: u8) {
        if flags & EXPLICIT_FG == 0 {
            *fg = self.foreground;
        } else if flags & PALETTE_FG != 0 {
            *fg = self.palette[usize::from(indices & 15)];
        }
        if flags & EXPLICIT_BG == 0 {
            *bg = self.background;
        } else if flags & PALETTE_BG != 0 {
            *bg = self.palette[usize::from(indices >> 4)];
        }
    }

    fn recolor_cell(&self, cell: &mut Cell) {
        self.recolor(&mut cell.fg, &mut cell.bg, cell.flags, cell.palette_indices);
    }

    fn recolor_style(&self, style: &mut Style) {
        self.recolor(
            &mut style.fg,
            &mut style.bg,
            style.flags,
            style.palette_indices,
        );
    }

    fn recolor_screen(&self, screen: &mut Screen) {
        for cell in &mut screen.cells {
            self.recolor_cell(cell);
        }
        self.recolor_cell(&mut screen.blank);
        self.recolor_style(&mut screen.saved.style);
        screen.mark_all();
    }
}

/// Shape selected by DECSCUSR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

/// Input and rendering modes negotiated by the child process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modes {
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub cursor_visible: bool,
    /// None follows the frontend preference.
    pub cursor_blink: Option<bool>,
    pub cursor_shape: Option<CursorShape>,
    pub bracketed_paste: bool,
    pub focus_events: bool,
    /// Zero, or DECSET 1000 (click), 1002 (drag), or 1003 (all motion).
    pub mouse_tracking: u16,
    pub sgr_mouse: bool,
    pub synchronized_output: bool,
    pub kitty_keyboard: u8,
    pub alternate_screen: bool,
}

impl Default for Modes {
    fn default() -> Self {
        Self {
            app_cursor: false,
            app_keypad: false,
            cursor_visible: true,
            cursor_blink: None,
            cursor_shape: None,
            bracketed_paste: false,
            focus_events: false,
            mouse_tracking: 0,
            sgr_mouse: false,
            synchronized_output: false,
            kitty_keyboard: 0,
            alternate_screen: false,
        }
    }
}

impl Modes {
    /// Snapshot only the modes needed by a platform-independent input encoder.
    ///
    /// Rendering state remains owned by this terminal model. The returned value
    /// can also be used by clients which depend on `zinput` directly.
    pub const fn input(&self) -> zinput::Modes {
        zinput::Modes {
            app_cursor: self.app_cursor,
            app_keypad: self.app_keypad,
            bracketed_paste: self.bracketed_paste,
            focus_events: self.focus_events,
            sgr_mouse: self.sgr_mouse,
            mouse_tracking: self.mouse_tracking,
            kitty_keyboard: self.kitty_keyboard,
        }
    }
}

/// Streaming terminal parser and screen state. No operating system calls.
pub struct Terminal {
    parser: Parser<4096>,
    state: State,
    // Large image bodies bypass fixed string buffers. zvte parses VT text and
    // DCS headers; framing distinguishes ST from cancellation for image data.
    framing: u8,
    apc: zgraphics::CommandStream,
}

fn dimensions(cols: usize, rows: usize) -> (usize, usize) {
    (cols.clamp(1, 1024), rows.clamp(1, 1024))
}

impl Terminal {
    pub fn new(cols: usize, rows: usize, history_limit: usize) -> Self {
        Self::with_theme(cols, rows, history_limit, Theme::default())
    }

    pub fn with_theme(cols: usize, rows: usize, history_limit: usize, theme: Theme) -> Self {
        let (cols, rows) = dimensions(cols, rows);
        Self {
            parser: Parser::new_with_size(),
            state: State::new(cols, rows, history_limit, theme),
            framing: 0,
            apc: zgraphics::CommandStream::new(),
        }
    }

    /// Return the configured default colors and ANSI palette.
    pub fn theme(&self) -> Theme {
        self.state.theme
    }

    /// Apply a theme to retained text without changing terminal content or modes.
    ///
    /// Default colors and ANSI palette references follow the new theme on the
    /// live screen, in scrollback, and in both active and cached alternate-screen
    /// state. Explicit RGB colors and indexed colors 16..=255 keep their values.
    /// Current and saved SGR styles are updated by the same source information;
    /// matching RGB values alone never identify a default or a palette reference.
    ///
    /// An actual change marks every screen row dirty. This scans retained cells
    /// in place and does not allocate. Reapplying the same theme does nothing.
    pub fn set_theme(&mut self, theme: Theme) {
        if self.state.theme == theme {
            return;
        }
        self.state.theme = theme;
        theme.recolor_style(&mut self.state.style);
        theme.recolor_screen(&mut self.state.screen);
        if let Some(primary) = self.state.primary.as_mut() {
            theme.recolor_screen(primary);
        }
        if let Some(alternate) = self.state.inactive_alternate.as_mut() {
            theme.recolor_screen(alternate);
        }
        for row in &mut self.state.history {
            for cell in row {
                theme.recolor_cell(cell);
            }
        }
    }

    /// Consume arbitrary PTY chunks, preserving partial UTF-8 and sequences.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut offset = 0;
        while offset < bytes.len() {
            match self.framing {
                0 => {
                    // Keep ordinary VT chunks intact, including every CSI/SGR
                    // in them. Only image-capable string introducers split it.
                    // `contains` uses core's byte-search specialization, so
                    // the common no-underscore path scans words, not callbacks.
                    let remaining = &bytes[offset..];
                    let string = if remaining.contains(&b'_') || remaining.contains(&b'P') {
                        remaining
                            .windows(2)
                            .position(|pair| pair[0] == 0x1b && matches!(pair[1], b'_' | b'P'))
                    } else {
                        None
                    };
                    if let Some(start) = string {
                        // First deliver ESC: zvte must cancel/dispatch an older
                        // OSC or DCS before the APC dispatcher takes ownership.
                        if remaining[start + 1] == b'_' {
                            self.parser
                                .advance(&remaining[..start + 1], &mut self.state);
                            self.begin_apc();
                        } else {
                            self.parser
                                .advance(&remaining[..start + 2], &mut self.state);
                            self.framing = 4;
                        }
                        offset += start + 2;
                    } else {
                        self.parser.advance(remaining, &mut self.state);
                        self.framing = u8::from(remaining.last() == Some(&0x1b));
                        offset = bytes.len();
                    }
                }
                1 => {
                    let byte = bytes[offset];
                    offset += 1;
                    if byte == b'_' {
                        self.begin_apc();
                    } else if byte == b'P' {
                        self.parser.advance(&[byte], &mut self.state);
                        self.framing = 4;
                    } else {
                        self.parser.advance(&[byte], &mut self.state);
                        self.framing = u8::from(byte == 0x1b);
                    }
                }
                2 => {
                    let end = bytes[offset..]
                        .iter()
                        .position(|b| matches!(*b, 0x1b | 0x18 | 0x1a))
                        .map(|i| offset + i)
                        .unwrap_or(bytes.len());
                    if end != offset {
                        let context = self.state.graphics_context();
                        self.apc.push(
                            &mut self.state.graphics,
                            &bytes[offset..end],
                            context,
                            &mut self.state.responses,
                        );
                        offset = end;
                    }
                    if offset == bytes.len() {
                        continue;
                    }
                    let byte = bytes[offset];
                    offset += 1;
                    match byte {
                        0x1b => self.framing = 3,
                        0x18 | 0x1a => {
                            self.apc.abort(&mut self.state.graphics);
                            self.framing = 0;
                        }
                        _ => unreachable!(),
                    }
                }
                3 => {
                    let byte = bytes[offset];
                    if byte == b'\\' {
                        offset += 1;
                        let previous = self.state.graphics.revision();
                        let context = self.state.graphics_context();
                        let outcome = self.apc.finish(
                            &mut self.state.graphics,
                            context,
                            &mut self.state.responses,
                        );
                        self.state.graphics_result(previous, outcome);
                        self.framing = 0;
                    } else {
                        // A new ESC sequence cancels an unterminated APC. The
                        // current byte is processed again as an ESC continuation.
                        self.apc.abort(&mut self.state.graphics);
                        self.parser.advance(b"\x1b", &mut self.state);
                        self.framing = 1;
                    }
                }
                4 => {
                    // DCS headers are short, bounded by zvte's parameter array.
                    let byte = bytes[offset];
                    offset += 1;
                    self.parser.advance(&[byte], &mut self.state);
                    self.framing = match byte {
                        0x1b => 1,
                        0x18 | 0x1a | 0x9c => 0,
                        0x40..=0x7e => {
                            if self.state.sixel_active {
                                5
                            } else {
                                0
                            }
                        }
                        _ => 4,
                    };
                }
                5 => {
                    let end = bytes[offset..]
                        .iter()
                        .position(|b| matches!(*b, 0x1b | 0x18 | 0x1a | 0x9c))
                        .map(|i| offset + i)
                        .unwrap_or(bytes.len());
                    for &byte in &bytes[offset..end] {
                        self.state.graphics.sixel_put(byte);
                    }
                    offset = end;
                    if offset == bytes.len() {
                        continue;
                    }
                    let byte = bytes[offset];
                    offset += 1;
                    if byte == 0x1b {
                        self.framing = 6;
                    } else {
                        self.state.sixel_complete = byte == 0x9c;
                        self.parser.advance(&[byte], &mut self.state);
                        self.state.sixel_complete = false;
                        self.framing = 0;
                    }
                }
                6 => {
                    if bytes[offset] == b'\\' {
                        offset += 1;
                        self.state.sixel_complete = true;
                        self.parser.advance(b"\x1b\\", &mut self.state);
                        self.state.sixel_complete = false;
                        self.framing = 0;
                    } else {
                        self.parser.advance(b"\x1b", &mut self.state);
                        self.framing = 1;
                    }
                }
                _ => unreachable!(),
            }
        }
    }

    fn begin_apc(&mut self) {
        self.parser.reset();
        self.apc.begin();
        self.framing = 2;
    }

    /// Set physical text-cell dimensions for graphics and CSI pixel-size replies.
    pub fn set_cell_size(&mut self, width: u32, height: u32) {
        let width = width.clamp(1, 4096);
        let height = height.clamp(1, 4096);
        if (self.state.cell_width, self.state.cell_height) != (width, height) {
            self.state.cell_width = width;
            self.state.cell_height = height;
            self.state.graphics.set_viewport(width, height, self.rows());
            self.state
                .inactive_graphics
                .set_viewport(width, height, self.rows());
            self.state.screen.mark_all();
        }
    }

    /// The current screen's bounded image store and placement scene.
    pub fn graphics(&self) -> &zgraphics::Graphics {
        &self.state.graphics
    }

    /// Borrow visible image placements in z-order; this iterator allocates nothing.
    pub fn graphics_draws(
        &self,
        history_offset: usize,
    ) -> impl Iterator<Item = zgraphics::Draw<'_>> {
        self.state
            .graphics
            .draws(if self.state.modes.alternate_screen {
                0
            } else {
                history_offset.min(self.history_len())
            })
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = dimensions(cols, rows);
        if self.cols() == cols && self.rows() == rows {
            return;
        }
        if cols != self.cols() {
            self.state.shell.invalidate_screen(self.state.line_base);
        }
        if !self.state.modes.alternate_screen {
            let dropped = self.state.screen.y.saturating_add(1).saturating_sub(rows);
            for row in 0..dropped {
                self.state.record_history_row(row);
            }
            self.state.graphics.scroll(
                0,
                self.state.screen.rows - 1,
                -(dropped as i32),
                self.state.history.len(),
                true,
            );
        } else {
            let dropped = self.state.screen.y.saturating_add(1).saturating_sub(rows);
            self.state
                .graphics
                .scroll(0, self.state.screen.rows - 1, -(dropped as i32), 0, false);
        }
        self.state.screen.resize(cols, rows);
        if let Some(primary) = self.state.primary.as_mut() {
            let dropped = primary.y.saturating_add(1).saturating_sub(rows);
            for row in 0..dropped {
                self.state.line_base = self.state.line_base.saturating_add(1);
                push_history(
                    &mut self.state.history,
                    self.state.history_limit,
                    &mut self.state.history_bytes,
                    primary.row(row),
                );
            }
            self.state.inactive_graphics.scroll(
                0,
                primary.rows - 1,
                -(dropped as i32),
                self.state.history.len(),
                true,
            );
            primary.resize(cols, rows);
        }
        if let Some(alternate) = self.state.inactive_alternate.as_mut() {
            let dropped = alternate.y.saturating_add(1).saturating_sub(rows);
            self.state
                .inactive_graphics
                .scroll(0, alternate.rows - 1, -(dropped as i32), 0, false);
            alternate.resize(cols, rows);
        }
        self.state
            .graphics
            .set_viewport(self.state.cell_width, self.state.cell_height, rows);
        self.state.inactive_graphics.set_viewport(
            self.state.cell_width,
            self.state.cell_height,
            rows,
        );
        self.state.graphics.trim_history(self.state.history.len());
        let previous_cols = self.state.tabs.len();
        self.state.tabs.resize(cols, false);
        for (x, tab) in self.state.tabs.iter_mut().enumerate().skip(previous_cols) {
            if x % 8 == 0 {
                *tab = true;
            }
        }
    }

    #[inline]
    pub fn cols(&self) -> usize {
        self.state.screen.cols
    }
    #[inline]
    pub fn rows(&self) -> usize {
        self.state.screen.rows
    }
    #[inline]
    pub fn cell(&self, col: usize, row: usize) -> &Cell {
        if col >= self.cols() || row >= self.rows() {
            return &self.state.screen.blank;
        }
        &self.state.screen.cells[self.state.screen.index(col, row)]
    }

    /// View the visible screen, moved `offset` lines into primary-screen history.
    pub fn view_cell(&self, col: usize, row: usize, offset: usize) -> &Cell {
        if col >= self.cols() || row >= self.rows() {
            return &self.state.screen.blank;
        }
        if self.state.modes.alternate_screen {
            return self.cell(col, row);
        }
        let history = &self.state.history;
        let absolute = history.len().saturating_sub(offset.min(history.len())) + row;
        if absolute >= history.len() {
            return self.cell(col, absolute - history.len());
        }
        let line = &history[absolute];
        match line.get(col) {
            Some(cell) if !(cell.width == 2 && col + 1 >= self.cols()) => cell,
            _ => &self.state.screen.blank,
        }
    }

    #[inline]
    pub fn cursor(&self) -> (usize, usize) {
        (self.state.screen.x, self.state.screen.y)
    }
    #[inline]
    pub fn modes(&self) -> &Modes {
        &self.state.modes
    }
    pub fn title(&self) -> &str {
        &self.state.title
    }
    pub fn working_directory(&self) -> &str {
        &self.state.cwd
    }
    /// OSC 133 records, oldest first. No storage is allocated before the first mark.
    pub fn commands(
        &self,
    ) -> impl DoubleEndedIterator<Item = &crate::CommandRecord> + ExactSizeIterator {
        self.state.shell.records.iter()
    }

    pub fn shell_phase(&self) -> crate::ShellPhase {
        self.state.shell.phase
    }

    /// Scrollback offset that makes a retained command's prompt visible.
    pub fn command_scroll_offset(&self, id: u64) -> Option<usize> {
        if self.state.modes.alternate_screen {
            return None;
        }
        let command = self
            .state
            .shell
            .records
            .iter()
            .find(|record| record.id == id)?;
        let oldest = self
            .state
            .line_base
            .saturating_sub(self.state.history.len() as u64);
        if !command.available || command.prompt.line < oldest {
            return None;
        }
        Some(self.state.line_base.saturating_sub(command.prompt.line) as usize)
    }

    /// Copy rendered command output, excluding its input and following prompt.
    ///
    /// Physical screen lines become newlines and trailing blank cells are omitted.
    /// This reads existing primary-screen/history cells; it does not save a second
    /// output transcript. Evicted or destructively edited ranges set `truncated`.
    /// `max_bytes` is capped at `MAX_COMMAND_TEXT` and never splits UTF-8.
    pub fn command_output(&self, id: u64, max_bytes: usize) -> Option<crate::CommandText> {
        let record = self
            .state
            .shell
            .records
            .iter()
            .find(|record| record.id == id)?;
        let start = record.output?;
        let end = record.end.unwrap_or_else(|| self.state.primary_position());
        Some(self.state.command_text(
            start,
            end,
            max_bytes,
            record.available,
            record.end.is_some(),
        ))
    }

    /// Copy the rendered command line when the shell supplied an OSC 133 B mark.
    pub fn command_input(&self, id: u64, max_bytes: usize) -> Option<crate::CommandText> {
        let record = self
            .state
            .shell
            .records
            .iter()
            .find(|record| record.id == id)?;
        let start = record.input?;
        let end = record
            .output
            .unwrap_or_else(|| self.state.primary_position());
        Some(self.state.command_text(
            start,
            end,
            max_bytes,
            record.available,
            record.output.is_some(),
        ))
    }

    pub fn responses(&self) -> &[u8] {
        &self.state.responses
    }
    pub fn clear_responses(&mut self) {
        self.state.responses.clear();
    }
    pub fn dirty(&self) -> bool {
        self.state.screen.dirty.iter().any(|v| *v)
    }
    pub fn row_dirty(&self, row: usize) -> bool {
        self.state.screen.dirty.get(row).copied().unwrap_or(false)
    }
    pub fn clear_dirty(&mut self) {
        self.state.screen.dirty.fill(false);
    }
    pub fn mark_all_dirty(&mut self) {
        self.state.screen.mark_all();
    }
    pub fn history_len(&self) -> usize {
        if self.state.modes.alternate_screen {
            0
        } else {
            self.state.history.len()
        }
    }
    pub fn hyperlink(&self, id: u32) -> Option<&str> {
        if id == 0 {
            return None;
        }
        self.state
            .links
            .get(id as usize - 1)
            .map(|link| link.uri.as_str())
    }
    /// A frontend may end a stalled synchronized frame after a documented timeout.
    pub fn force_end_sync(&mut self) {
        self.state.modes.synchronized_output = false;
    }
}

struct Link {
    id: String,
    uri: String,
}

// Common numeric protocol replies fit in this stack buffer. Formatting fails
// atomically if the whole reply cannot fit; no temporary String is allocated.
struct ReplyBuffer {
    bytes: [u8; 64],
    len: usize,
}

impl fmt::Write for ReplyBuffer {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if text.len() > self.bytes.len() - self.len {
            return Err(fmt::Error);
        }
        self.bytes[self.len..self.len + text.len()].copy_from_slice(text.as_bytes());
        self.len += text.len();
        Ok(())
    }
}

struct State {
    theme: Theme,
    screen: Screen,
    primary: Option<Screen>,
    inactive_alternate: Option<Screen>,
    history: VecDeque<Vec<Cell>>,
    history_limit: usize,
    history_bytes: usize,
    line_base: u64,
    shell: crate::shell::ShellIntegration,
    style: Style,
    modes: Modes,
    autowrap: bool,
    insert: bool,
    newline: bool,
    tabs: Vec<bool>,
    g0: bool,
    g1: bool,
    use_g1: bool,
    title: String,
    cwd: String,
    links: Vec<Link>,
    responses: Vec<u8>,
    osc_overflow: bool,
    dcs_query: bool,
    dcs: Vec<u8>,
    dcs_overflow: bool,
    sixel_active: bool,
    sixel_complete: bool,
    sixel_display: bool,
    sixel_right: bool,
    sixel_private: bool,
    sixel_palette: Option<Box<zgraphics::SixelPalette>>,
    last_char: char,
    graphics: zgraphics::Graphics,
    inactive_graphics: zgraphics::Graphics,
    cell_width: u32,
    cell_height: u32,
}

impl State {
    fn new(cols: usize, rows: usize, history_limit: usize, theme: Theme) -> Self {
        let tabs = (0..cols).map(|x| x % 8 == 0).collect();
        Self {
            theme,
            screen: Screen::new(cols, rows, theme.style()),
            primary: None,
            inactive_alternate: None,
            history: VecDeque::new(),
            history_limit: history_limit.min(65536),
            history_bytes: 0,
            line_base: 0,
            shell: crate::shell::ShellIntegration::default(),
            style: theme.style(),
            modes: Modes::default(),
            autowrap: true,
            insert: false,
            newline: false,
            tabs,
            g0: false,
            g1: false,
            use_g1: false,
            title: "ztty".to_string(),
            cwd: String::new(),
            links: Vec::new(),
            responses: Vec::new(),
            osc_overflow: false,
            dcs_query: false,
            dcs: Vec::new(),
            dcs_overflow: false,
            sixel_active: false,
            sixel_complete: false,
            sixel_display: false,
            sixel_right: false,
            sixel_private: true,
            sixel_palette: None,
            last_char: ' ',
            graphics: zgraphics::Graphics::default(),
            inactive_graphics: zgraphics::Graphics::default(),
            cell_width: 8,
            cell_height: 16,
        }
    }

    fn primary_screen(&self) -> &Screen {
        self.primary.as_ref().unwrap_or(&self.screen)
    }

    fn primary_position(&self) -> crate::TextPosition {
        let screen = self.primary_screen();
        crate::TextPosition {
            line: self.line_base.saturating_add(screen.y as u64),
            column: screen.x + usize::from(screen.wrap),
        }
    }

    fn command_text(
        &self,
        start: crate::TextPosition,
        end: crate::TextPosition,
        max_bytes: usize,
        available: bool,
        complete: bool,
    ) -> crate::CommandText {
        let mut result = crate::CommandText {
            text: String::new(),
            truncated: !available,
            complete,
        };
        if !available || end < start {
            result.truncated = true;
            return result;
        }
        let limit = max_bytes.min(crate::MAX_COMMAND_TEXT);
        let screen = self.primary_screen();
        let oldest = self.line_base.saturating_sub(self.history.len() as u64);
        let last = self.line_base.saturating_add(screen.rows as u64 - 1);
        let first = start.line.max(oldest);
        result.truncated |= start.line < oldest || end.line > last;
        if first > end.line || first > last {
            result.truncated |= start != end;
            return result;
        }
        for line in first..=end.line.min(last) {
            let cells = if line < self.line_base {
                &self.history[(line - oldest) as usize][..]
            } else {
                screen.row((line - self.line_base) as usize)
            };
            let from = if line == start.line { start.column } else { 0 };
            let until = if line == end.line {
                end.column
            } else {
                cells.len()
            };
            result.truncated |= from > cells.len() || until > cells.len();
            let from = from.min(cells.len());
            let until = until.min(cells.len()).max(from);
            let selected = &cells[from..until];
            let length = selected
                .iter()
                .rposition(|cell| cell.ch != ' ' || cell.combining != ['\0'; 2])
                .map_or(0, |index| index + 1);
            for cell in &selected[..length] {
                if cell.width == 0 {
                    continue;
                }
                for ch in core::iter::once(cell.ch)
                    .chain(cell.combining.into_iter().filter(|ch| *ch != '\0'))
                {
                    if result.text.len() + ch.len_utf8() > limit {
                        result.truncated = true;
                        return result;
                    }
                    result.text.push(ch);
                }
            }
            if line < end.line {
                if result.text.len() == limit {
                    result.truncated = true;
                    return result;
                }
                result.text.push('\n');
            }
        }
        result
    }

    fn graphics_command(&mut self, body: &[u8]) {
        let previous = self.graphics.revision();
        let context = self.graphics_context();
        let outcome = self.graphics.command(body, context, &mut self.responses);
        self.graphics_result(previous, outcome);
    }

    fn graphics_context(&self) -> zgraphics::Context {
        zgraphics::Context {
            col: self.screen.x,
            row: self.screen.y,
            cols: self.screen.cols,
            rows: self.screen.rows,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        }
    }

    fn graphics_result(&mut self, previous: u64, outcome: zgraphics::Outcome) {
        if let Some((cols, rows)) = outcome.cursor_move {
            let col = self.screen.x.saturating_add(cols).min(self.screen.cols - 1);
            for _ in 0..rows.min(self.screen.rows) {
                self.linefeed();
            }
            self.screen.x = col;
            self.screen.wrap = false;
        }
        if self.graphics.revision() != previous {
            self.screen.mark_all();
        }
    }

    fn finish_sixel(&mut self) {
        let mut context = self.graphics_context();
        if self.sixel_display {
            context.col = 0;
            context.row = 0;
        }
        let Some(result) = self.graphics.finish_sixel(context) else {
            return;
        };
        if !self.sixel_private {
            self.sixel_palette = Some(Box::new(result.palette));
        }
        if result.height == 0 {
            return;
        }
        if !self.sixel_display {
            let right = context
                .col
                .saturating_add(result.width.div_ceil(self.cell_width) as usize);
            let rows = result.height.div_ceil(self.cell_height) as usize;
            // With 8452 the cursor remains beside the last image row. The
            // default xterm behavior leaves it at the beginning of the next.
            let advance = rows.saturating_sub(usize::from(self.sixel_right));
            if self.screen.y <= self.screen.bottom {
                let end = self.screen.y.saturating_add(advance);
                let mut overflow = end.saturating_sub(self.screen.bottom);
                let region = self.screen.bottom - self.screen.top + 1;
                if self.screen.top != 0 || self.screen.bottom + 1 != self.screen.rows {
                    overflow = overflow.min(region);
                }
                while overflow != 0 {
                    let count = overflow.min(region);
                    self.scroll_up(self.screen.top, self.screen.bottom, count, true);
                    overflow -= count;
                }
                self.screen.y = end.min(self.screen.bottom);
            } else {
                self.screen.y = self
                    .screen
                    .y
                    .saturating_add(advance)
                    .min(self.screen.rows - 1);
            }
            self.screen.x = if self.sixel_right {
                right.min(self.screen.cols - 1)
            } else {
                0
            };
            self.screen.wrap = false;
        }
        self.screen.mark_all();
    }

    fn reply(&mut self, value: &str) {
        if self.responses.len() + value.len() <= MAX_RESPONSES {
            self.responses.extend_from_slice(value.as_bytes());
        }
    }

    fn reply_args(&mut self, arguments: fmt::Arguments<'_>) {
        let mut buffer = ReplyBuffer {
            bytes: [0; 64],
            len: 0,
        };
        if buffer.write_fmt(arguments).is_ok() && self.responses.len() + buffer.len <= MAX_RESPONSES
        {
            self.responses
                .extend_from_slice(&buffer.bytes[..buffer.len]);
        }
    }

    fn reply_color(&mut self, command: u16, index: Option<u16>, color: u32, bell: bool) {
        let red = ((color >> 16) & 255) * 257;
        let green = ((color >> 8) & 255) * 257;
        let blue = (color & 255) * 257;
        let end = if bell { "\x07" } else { "\x1b\\" };
        if let Some(index) = index {
            self.reply(&format!(
                "\x1b]{command};{index};rgb:{red:04x}/{green:04x}/{blue:04x}{end}"
            ));
        } else {
            self.reply(&format!(
                "\x1b]{command};rgb:{red:04x}/{green:04x}/{blue:04x}{end}"
            ));
        }
    }

    fn cursor_to(&mut self, x: usize, y: usize) {
        self.screen.dirty[self.screen.y] = true;
        self.screen.x = x.min(self.screen.cols - 1);
        self.screen.y = y.min(self.screen.rows - 1);
        self.screen.dirty[self.screen.y] = true;
        self.screen.wrap = false;
    }

    fn save_cursor(&mut self) {
        self.screen.saved = SavedCursor {
            x: self.screen.x,
            y: self.screen.y,
            style: self.style,
            origin: self.screen.origin,
            charset: if self.use_g1 { self.g1 } else { self.g0 },
            wrap: self.screen.wrap,
        };
    }

    fn restore_cursor(&mut self) {
        let saved = self.screen.saved;
        self.cursor_to(saved.x, saved.y);
        self.style = saved.style;
        self.screen.origin = saved.origin;
        self.screen.wrap = saved.wrap;
        if self.use_g1 {
            self.g1 = saved.charset;
        } else {
            self.g0 = saved.charset;
        }
    }

    fn record_history_row(&mut self, row: usize) {
        if self.modes.alternate_screen {
            return;
        }
        self.line_base = self.line_base.saturating_add(1);
        push_history(
            &mut self.history,
            self.history_limit,
            &mut self.history_bytes,
            self.screen.row(row),
        );
    }

    fn scroll_up(&mut self, top: usize, bottom: usize, count: usize, save_history: bool) {
        let count = count.min(bottom - top + 1);
        if !self.modes.alternate_screen
            && (top != 0 || bottom + 1 != self.screen.rows || !save_history)
        {
            self.shell.invalidate_screen(self.line_base);
        }
        let blank = self.style.blank();
        if count == bottom - top + 1 && (top != 0 || bottom + 1 != self.screen.rows) {
            for row in top..=bottom {
                self.screen.row_mut(row).fill(blank);
            }
        } else {
            for _ in 0..count {
                if top == 0 && bottom + 1 == self.screen.rows {
                    if save_history {
                        self.record_history_row(0);
                    }
                    self.screen.row0 = (self.screen.row0 + 1) % self.screen.rows;
                    self.screen.row_mut(bottom).fill(blank);
                } else {
                    for y in top..bottom {
                        let source = self.screen.index(0, y + 1);
                        let target = self.screen.index(0, y);
                        self.screen
                            .cells
                            .copy_within(source..source + self.screen.cols, target);
                    }
                    self.screen.row_mut(bottom).fill(blank);
                }
            }
        }
        self.graphics.scroll(
            top,
            bottom,
            -(count as i32),
            self.history.len(),
            save_history && !self.modes.alternate_screen,
        );
        self.screen.dirty[top..=bottom].fill(true);
    }

    fn scroll_down(&mut self, top: usize, bottom: usize, count: usize) {
        if !self.modes.alternate_screen {
            self.shell.invalidate_screen(self.line_base);
        }
        let blank = self.style.blank();
        for _ in 0..count.min(bottom - top + 1) {
            if top == 0 && bottom + 1 == self.screen.rows {
                self.screen.row0 = (self.screen.row0 + self.screen.rows - 1) % self.screen.rows;
                self.screen.row_mut(top).fill(blank);
            } else {
                for y in (top + 1..=bottom).rev() {
                    let source = self.screen.index(0, y - 1);
                    let target = self.screen.index(0, y);
                    self.screen
                        .cells
                        .copy_within(source..source + self.screen.cols, target);
                }
                self.screen.row_mut(top).fill(blank);
            }
        }
        self.graphics.scroll(
            top,
            bottom,
            count.min(bottom - top + 1) as i32,
            self.history.len(),
            false,
        );
        self.screen.dirty[top..=bottom].fill(true);
    }

    fn linefeed(&mut self) {
        self.screen.dirty[self.screen.y] = true;
        if self.screen.y == self.screen.bottom {
            self.scroll_up(self.screen.top, self.screen.bottom, 1, true);
        } else if self.screen.y + 1 < self.screen.rows {
            self.screen.y += 1;
        }
        self.screen.dirty[self.screen.y] = true;
        self.screen.wrap = false;
        if self.newline {
            self.screen.x = 0;
        }
    }

    fn reverse_index(&mut self) {
        if self.screen.y == self.screen.top {
            self.scroll_down(self.screen.top, self.screen.bottom, 1);
        } else {
            self.cursor_to(self.screen.x, self.screen.y.saturating_sub(1));
        }
        self.screen.wrap = false;
    }

    fn put_char(&mut self, mut ch: char) {
        if if self.use_g1 { self.g1 } else { self.g0 } {
            ch = dec_graphics(ch);
        }
        let width = UnicodeWidthChar::width(ch).unwrap_or(0).min(2);
        if width == 0 {
            let mut x = if self.screen.wrap {
                self.screen.x
            } else {
                self.screen.x.saturating_sub(1)
            };
            if self.screen.x == 0 && !self.screen.wrap {
                return;
            }
            let y = self.screen.y;
            if self.screen.cells[self.screen.index(x, y)].width == 0 && x > 0 {
                x -= 1;
            }
            let index = self.screen.index(x, y);
            if let Some(slot) = self.screen.cells[index]
                .combining
                .iter_mut()
                .find(|v| **v == '\0')
            {
                *slot = ch;
                self.screen.dirty[y] = true;
            }
            return;
        }
        if self.screen.wrap && self.autowrap {
            self.linefeed();
            self.screen.x = 0;
        }
        let mut width = width;
        if width == 2 && self.screen.x + 1 >= self.screen.cols {
            if self.autowrap && self.screen.cols > 1 {
                self.screen.clear_range(
                    self.screen.y,
                    self.screen.x,
                    self.screen.cols,
                    self.style.blank(),
                );
                self.linefeed();
                self.screen.x = 0;
            } else {
                ch = '\u{fffd}';
                width = 1;
            }
        }
        let x = self.screen.x;
        let y = self.screen.y;
        if self.insert {
            self.insert_chars(width);
        }
        self.screen.clear_range(y, x, x + width, self.style.blank());
        let index = self.screen.index(x, y);
        self.screen.cells[index] = self.style.cell(ch, width as u8);
        if width == 2 {
            self.screen.cells[index + 1] = self.style.cell('\0', 0);
        }
        self.screen.dirty[y] = true;
        self.last_char = ch;
        if x + width >= self.screen.cols {
            self.screen.x = self.screen.cols - 1;
            self.screen.wrap = self.autowrap;
        } else {
            self.screen.x += width;
            self.screen.wrap = false;
        }
    }

    fn ascii(&mut self, mut bytes: &[u8]) {
        if self.insert || self.g0 || self.g1 || !self.autowrap {
            for &byte in bytes {
                self.put_char(byte as char);
            }
            return;
        }
        while !bytes.is_empty() {
            if self.screen.wrap {
                self.linefeed();
                self.screen.x = 0;
            }
            let count = bytes.len().min(self.screen.cols - self.screen.x);
            let x = self.screen.x;
            let y = self.screen.y;
            let style = self.style;
            for (cell, &byte) in self
                .screen
                .overwrite_span(y, x, x + count, style.blank())
                .iter_mut()
                .zip(bytes)
            {
                *cell = style.cell(byte as char, 1);
            }
            self.screen.dirty[y] = true;
            self.last_char = bytes[count - 1] as char;
            if x + count == self.screen.cols {
                self.screen.x = self.screen.cols - 1;
                self.screen.wrap = true;
            } else {
                self.screen.x += count;
            }
            bytes = &bytes[count..];
        }
    }

    fn insert_chars(&mut self, count: usize) {
        let x = self.screen.x;
        let y = self.screen.y;
        let cols = self.screen.cols;
        let count = count.min(cols - x);
        let blank = self.style.blank();
        let row = self.screen.row_mut(y);
        if x > 0 && row[x].width == 0 {
            row[x - 1] = blank;
            row[x] = blank;
        }
        row.copy_within(x..cols - count, x + count);
        row[x..x + count].fill(blank);
        self.screen.repair_row(y, blank);
        self.screen.wrap = false;
    }

    fn delete_chars(&mut self, count: usize) {
        let x = self.screen.x;
        let y = self.screen.y;
        let cols = self.screen.cols;
        let count = count.min(cols - x);
        let blank = self.style.blank();
        self.screen.clear_range(y, x, x + count, blank);
        let row = self.screen.row_mut(y);
        row.copy_within(x + count..cols, x);
        row[cols - count..].fill(blank);
        self.screen.repair_row(y, blank);
        self.screen.wrap = false;
    }

    fn alternate(&mut self, enabled: bool, save: bool, clear: bool) {
        if enabled == self.modes.alternate_screen {
            return;
        }
        if enabled {
            if save {
                self.save_cursor();
            }
            let mut alternate = self.inactive_alternate.take().unwrap_or_else(|| {
                Screen::new(self.screen.cols, self.screen.rows, self.theme.style())
            });
            if clear {
                alternate.x = 0;
                alternate.y = 0;
                alternate.wrap = false;
            }
            self.primary = Some(mem::replace(&mut self.screen, alternate));
        } else if let Some(primary) = self.primary.take() {
            self.inactive_alternate = Some(mem::replace(&mut self.screen, primary));
            if save {
                self.restore_cursor();
            }
        }
        self.modes.alternate_screen = enabled;
        self.graphics.abort_transfer();
        mem::swap(&mut self.graphics, &mut self.inactive_graphics);
        self.graphics.abort_transfer();
        self.graphics
            .set_viewport(self.cell_width, self.cell_height, self.screen.rows);
        self.graphics.invalidate();
        self.modes.kitty_keyboard = self.screen.keyboard;
        if enabled && clear {
            self.screen.clear(self.style.blank());
            self.graphics.clear();
        }
        self.screen.mark_all();
    }

    fn set_mode(&mut self, private: bool, mode: u16, enabled: bool) {
        if !private {
            match mode {
                4 => self.insert = enabled,
                20 => self.newline = enabled,
                _ => {}
            }
            return;
        }
        match mode {
            1 => self.modes.app_cursor = enabled,
            6 => {
                self.screen.origin = enabled;
                self.cursor_to(0, if enabled { self.screen.top } else { 0 });
            }
            7 => {
                self.autowrap = enabled;
                self.screen.wrap = false;
            }
            12 => self.modes.cursor_blink = Some(enabled),
            25 => {
                self.modes.cursor_visible = enabled;
                self.screen.dirty[self.screen.y] = true;
            }
            47 | 1047 => self.alternate(enabled, false, mode == 1047),
            80 => self.sixel_display = enabled,
            1070 => self.sixel_private = enabled,
            8452 => self.sixel_right = enabled,
            1048 => {
                if enabled {
                    self.save_cursor();
                } else {
                    self.restore_cursor();
                }
            }
            1049 => self.alternate(enabled, true, true),
            9 | 1000 | 1002 | 1003 => {
                if enabled {
                    self.modes.mouse_tracking = mode;
                } else if self.modes.mouse_tracking == mode {
                    self.modes.mouse_tracking = 0;
                }
            }
            1004 => self.modes.focus_events = enabled,
            1006 => self.modes.sgr_mouse = enabled,
            2004 => self.modes.bracketed_paste = enabled,
            2026 => self.modes.synchronized_output = enabled,
            _ => {}
        }
    }

    fn query_mode(&mut self, private: bool, mode: u16) {
        let state = if private {
            match mode {
                1 => Some(self.modes.app_cursor),
                6 => Some(self.screen.origin),
                7 => Some(self.autowrap),
                12 => self.modes.cursor_blink,
                25 => Some(self.modes.cursor_visible),
                47 | 1047 | 1049 => Some(self.modes.alternate_screen),
                80 => Some(self.sixel_display),
                1070 => Some(self.sixel_private),
                8452 => Some(self.sixel_right),
                9 | 1000 | 1002 | 1003 => Some(self.modes.mouse_tracking == mode),
                1004 => Some(self.modes.focus_events),
                1006 => Some(self.modes.sgr_mouse),
                2004 => Some(self.modes.bracketed_paste),
                2026 => Some(self.modes.synchronized_output),
                _ => None,
            }
        } else {
            match mode {
                4 => Some(self.insert),
                20 => Some(self.newline),
                _ => None,
            }
        };
        self.reply_args(format_args!(
            "\x1b[{}{};{}$y",
            if private { "?" } else { "" },
            mode,
            match state {
                Some(true) => 1,
                Some(false) => 2,
                None => 0,
            }
        ));
    }

    fn kitty(&mut self, marker: u8, groups: &[&[u16]]) {
        let value = param(groups, 0, 0) as u8 & 0b11111;
        match marker {
            b'?' => {
                let flags = self.screen.keyboard;
                self.reply_args(format_args!("\x1b[?{flags}u"));
            }
            b'>' => {
                if self.screen.keyboard_depth == self.screen.keyboard_stack.len() {
                    self.screen.keyboard_stack.copy_within(1.., 0);
                    self.screen.keyboard_depth -= 1;
                }
                self.screen.keyboard_stack[self.screen.keyboard_depth] = self.screen.keyboard;
                self.screen.keyboard_depth += 1;
                self.screen.keyboard = value;
            }
            b'<' => {
                let count = param(groups, 0, 1) as usize;
                if count > self.screen.keyboard_depth {
                    self.screen.keyboard = 0;
                    self.screen.keyboard_depth = 0;
                } else {
                    self.screen.keyboard_depth -= count;
                    self.screen.keyboard = self.screen.keyboard_stack[self.screen.keyboard_depth];
                }
            }
            b'=' => match param(groups, 1, 1) {
                1 => self.screen.keyboard = value,
                2 => self.screen.keyboard |= value,
                3 => self.screen.keyboard &= !value,
                _ => {}
            },
            _ => {}
        }
        self.modes.kitty_keyboard = self.screen.keyboard;
    }

    fn sgr(&mut self, groups: &[&[u16]]) {
        let mut i = 0;
        while i < groups.len() {
            let group = groups[i];
            let value = group.first().copied().unwrap_or(0);
            match value {
                0 => {
                    let link_id = self.style.link_id;
                    self.style = Style {
                        link_id,
                        ..self.theme.style()
                    };
                }
                1 => self.style.flags |= BOLD,
                2 => self.style.flags |= DIM,
                3 => self.style.flags |= ITALIC,
                4 => {
                    if group.get(1) == Some(&0) {
                        self.style.flags &= !UNDERLINE;
                    } else {
                        self.style.flags |= UNDERLINE;
                    }
                }
                21 => self.style.flags |= UNDERLINE,
                7 => self.style.flags |= INVERSE,
                8 => self.style.flags |= HIDDEN,
                9 => self.style.flags |= STRIKE,
                22 => self.style.flags &= !(BOLD | DIM),
                23 => self.style.flags &= !ITALIC,
                24 => self.style.flags &= !UNDERLINE,
                27 => self.style.flags &= !INVERSE,
                28 => self.style.flags &= !HIDDEN,
                29 => self.style.flags &= !STRIKE,
                30..=37 => {
                    self.style
                        .set_color(true, self.theme.color(value - 30), Some(value - 30))
                }
                40..=47 => {
                    self.style
                        .set_color(false, self.theme.color(value - 40), Some(value - 40));
                }
                90..=97 => self.style.set_color(
                    true,
                    self.theme.color(value - 90 + 8),
                    Some(value - 90 + 8),
                ),
                100..=107 => {
                    self.style.set_color(
                        false,
                        self.theme.color(value - 100 + 8),
                        Some(value - 100 + 8),
                    );
                }
                39 => {
                    self.style.fg = self.theme.foreground;
                    self.style.flags &= !(EXPLICIT_FG | PALETTE_FG);
                    self.style.palette_indices &= 0xf0;
                }
                49 => {
                    self.style.bg = self.theme.background;
                    self.style.flags &= !(EXPLICIT_BG | PALETTE_BG);
                    self.style.palette_indices &= 0x0f;
                }
                38 | 48 => {
                    let mut palette = None;
                    let color = if group.len() > 1 {
                        match group.get(1) {
                            Some(5) => group.get(2).filter(|v| **v <= 255).map(|v| {
                                palette = Some(*v);
                                self.theme.color(*v)
                            }),
                            Some(2) => {
                                let base = if group.len() >= 6 { 3 } else { 2 };
                                rgb(
                                    group.get(base).copied(),
                                    group.get(base + 1).copied(),
                                    group.get(base + 2).copied(),
                                )
                            }
                            _ => None,
                        }
                    } else {
                        match raw_param(groups, i + 1) {
                            Some(5) => {
                                let value =
                                    raw_param(groups, i + 2).filter(|v| *v <= 255).map(|v| {
                                        palette = Some(v);
                                        self.theme.color(v)
                                    });
                                i += 2;
                                value
                            }
                            Some(2) => {
                                let value = rgb(
                                    raw_param(groups, i + 2),
                                    raw_param(groups, i + 3),
                                    raw_param(groups, i + 4),
                                );
                                i += 4;
                                value
                            }
                            _ => None,
                        }
                    };
                    if let Some(color) = color {
                        self.style.set_color(value == 38, color, palette);
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn erase_display(&mut self, mode: u16) {
        let blank = self.style.blank();
        let x = self.screen.x;
        let y = self.screen.y;
        match mode {
            0 => {
                self.screen.clear_range(y, x, self.screen.cols, blank);
                for row in y + 1..self.screen.rows {
                    self.screen.row_mut(row).fill(blank);
                    self.screen.dirty[row] = true;
                }
            }
            1 => {
                for row in 0..y {
                    self.screen.row_mut(row).fill(blank);
                    self.screen.dirty[row] = true;
                }
                self.screen.clear_range(y, 0, x + 1, blank);
            }
            2 => {
                if !self.modes.alternate_screen {
                    self.shell.invalidate_screen(self.line_base);
                }
                self.screen.clear(blank);
                self.graphics_command(b"Ga=d");
            }
            3 => {
                self.history.clear();
                self.history_bytes = 0;
                if self.modes.alternate_screen {
                    self.inactive_graphics.trim_history(0);
                } else {
                    self.graphics.trim_history(0);
                }
                self.screen.mark_all();
            }
            _ => {}
        }
        self.screen.wrap = false;
    }
}

impl Perform for State {
    fn print(&mut self, ch: char) {
        self.put_char(ch);
    }
    fn text(&mut self, ascii: &[u8]) {
        self.ascii(ascii);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x08 => self.cursor_to(self.screen.x.saturating_sub(1), self.screen.y),
            0x09 => {
                let x = (self.screen.x + 1..self.screen.cols)
                    .find(|x| self.tabs[*x])
                    .unwrap_or(self.screen.cols - 1);
                self.cursor_to(x, self.screen.y);
            }
            0x0a..=0x0c | 0x84 => self.linefeed(),
            0x0d => self.cursor_to(0, self.screen.y),
            0x0e => self.use_g1 = true,
            0x0f => self.use_g1 = false,
            0x85 => {
                self.linefeed();
                self.screen.x = 0;
            }
            0x88 => self.tabs[self.screen.x] = true,
            0x8d => self.reverse_index(),
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if ignore {
            return;
        }
        match (intermediates, byte) {
            ([], b'7') => self.save_cursor(),
            ([], b'8') => self.restore_cursor(),
            ([], b'D') => self.linefeed(),
            ([], b'E') => {
                self.linefeed();
                self.screen.x = 0;
            }
            ([], b'M') => self.reverse_index(),
            ([], b'H') => self.tabs[self.screen.x] = true,
            ([], b'=') => self.modes.app_keypad = true,
            ([], b'>') => self.modes.app_keypad = false,
            ([], b'c') => {
                let (cols, rows, history) =
                    (self.screen.cols, self.screen.rows, self.history_limit);
                let (width, height) = (self.cell_width, self.cell_height);
                let mut graphics = mem::take(&mut self.graphics);
                let mut inactive_graphics = mem::take(&mut self.inactive_graphics);
                graphics.clear();
                inactive_graphics.clear();
                let mut shell = mem::take(&mut self.shell);
                shell.clear();
                *self = Self::new(cols, rows, history, self.theme);
                self.shell = shell;
                self.graphics = graphics;
                self.inactive_graphics = inactive_graphics;
                self.cell_width = width;
                self.cell_height = height;
                self.graphics.set_viewport(width, height, rows);
            }
            ([b'('], b'0') => self.g0 = true,
            ([b'('], b'B' | b'A') => self.g0 = false,
            ([b')'], b'0') => self.g1 = true,
            ([b')'], b'B' | b'A') => self.g1 = false,
            ([b'#'], b'8') => {
                self.screen.cells.fill(self.theme.style().cell('E', 1));
                self.screen.mark_all();
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if ignore {
            return;
        }
        let mut storage: [&[u16]; 32] = [&[]; 32];
        let mut count = 0;
        for group in params.iter() {
            storage[count] = group;
            count += 1;
        }
        let groups = &storage[..count];
        let private = intermediates.first() == Some(&b'?');
        if action == 'u'
            && intermediates.len() == 1
            && matches!(intermediates[0], b'?' | b'>' | b'<' | b'=')
        {
            self.kitty(intermediates[0], groups);
            return;
        }
        if action == 'q' && intermediates == b" " {
            let style = param(groups, 0, 0);
            let shape = match style {
                0..=2 => CursorShape::Block,
                3 | 4 => CursorShape::Underline,
                5 | 6 => CursorShape::Bar,
                _ => return,
            };
            self.modes.cursor_shape = Some(shape);
            self.modes.cursor_blink = Some(style == 0 || style % 2 == 1);
            return;
        }
        if action == 'p' && (intermediates == b"?$" || intermediates == b"$") {
            for group in groups {
                self.query_mode(private, group.first().copied().unwrap_or(0));
            }
            return;
        }
        if (action == 'h' || action == 'l') && (intermediates.is_empty() || intermediates == b"?") {
            for group in groups {
                self.set_mode(private, group.first().copied().unwrap_or(0), action == 'h');
            }
            return;
        }
        if action == 'c' && param(groups, 0, 0) == 0 {
            match intermediates {
                // VT220-style DA1 advertises the implemented sixel capability.
                b"" => self.reply("\x1b[?62;4c"),
                b">" => self.reply("\x1b[>0;1;0c"),
                _ => {}
            }
            return;
        }
        if action == 'n' && (intermediates.is_empty() || private) {
            match param(groups, 0, 0) {
                5 if !private => self.reply("\x1b[0n"),
                6 => {
                    let row = if self.screen.origin {
                        self.screen.y.saturating_sub(self.screen.top)
                    } else {
                        self.screen.y
                    };
                    let column = self.screen.x + 1;
                    self.reply_args(format_args!(
                        "\x1b[{}{};{}R",
                        if private { "?" } else { "" },
                        row + 1,
                        column
                    ));
                }
                _ => {}
            }
            return;
        }
        if !intermediates.is_empty() {
            return;
        }
        let p = param(groups, 0, 1) as usize;
        let raw = param(groups, 0, 0);
        let x = self.screen.x;
        let y = self.screen.y;
        let upper = if self.screen.origin {
            self.screen.top
        } else {
            0
        };
        let lower = if self.screen.origin {
            self.screen.bottom
        } else {
            self.screen.rows - 1
        };
        let move_upper = if y >= self.screen.top {
            self.screen.top
        } else {
            0
        };
        let move_lower = if y <= self.screen.bottom {
            self.screen.bottom
        } else {
            self.screen.rows - 1
        };
        match action {
            'A' => self.cursor_to(x, y.saturating_sub(p).max(move_upper)),
            'B' | 'e' => self.cursor_to(x, y.saturating_add(p).min(move_lower)),
            'C' | 'a' => self.cursor_to(x.saturating_add(p), y),
            'D' => self.cursor_to(x.saturating_sub(p), y),
            'E' => self.cursor_to(0, y.saturating_add(p).min(move_lower)),
            'F' => self.cursor_to(0, y.saturating_sub(p).max(move_upper)),
            'G' | '`' => self.cursor_to(p - 1, y),
            'H' | 'f' => {
                self.cursor_to(param(groups, 1, 1) as usize - 1, (upper + p - 1).min(lower))
            }
            'd' => self.cursor_to(x, (upper + p - 1).min(lower)),
            'J' => self.erase_display(raw),
            'K' => {
                match raw {
                    0 => self
                        .screen
                        .clear_range(y, x, self.screen.cols, self.style.blank()),
                    1 => self.screen.clear_range(y, 0, x + 1, self.style.blank()),
                    2 => self
                        .screen
                        .clear_range(y, 0, self.screen.cols, self.style.blank()),
                    _ => {}
                }
                self.screen.wrap = false;
            }
            'X' => {
                self.screen
                    .clear_range(y, x, x.saturating_add(p), self.style.blank());
                self.screen.wrap = false;
            }
            '@' => self.insert_chars(p),
            'P' => self.delete_chars(p),
            'L' if y >= self.screen.top && y <= self.screen.bottom => {
                self.scroll_down(y, self.screen.bottom, p)
            }
            'M' if y >= self.screen.top && y <= self.screen.bottom => {
                self.scroll_up(y, self.screen.bottom, p, false)
            }
            'S' => self.scroll_up(self.screen.top, self.screen.bottom, p, false),
            'T' => self.scroll_down(self.screen.top, self.screen.bottom, p),
            'm' => self.sgr(groups),
            's' => self.save_cursor(),
            'u' => self.restore_cursor(),
            'r' => {
                let top = p.saturating_sub(1).min(self.screen.rows - 1);
                let bottom = (param(groups, 1, self.screen.rows as u16) as usize - 1)
                    .min(self.screen.rows - 1);
                if top < bottom {
                    self.screen.top = top;
                    self.screen.bottom = bottom;
                    self.cursor_to(0, if self.screen.origin { top } else { 0 });
                }
            }
            'g' => match raw {
                0 => self.tabs[x] = false,
                3 => self.tabs.fill(false),
                _ => {}
            },
            'I' => {
                let mut x = x;
                for _ in 0..p.min(self.screen.cols) {
                    x = (x + 1..self.screen.cols)
                        .find(|x| self.tabs[*x])
                        .unwrap_or(self.screen.cols - 1);
                }
                self.cursor_to(x, y);
            }
            'Z' => {
                let mut x = x;
                for _ in 0..p.min(self.screen.cols) {
                    x = (0..x).rev().find(|x| self.tabs[*x]).unwrap_or(0);
                }
                self.cursor_to(x, y);
            }
            'b' => {
                for _ in 0..p.min(65535) {
                    self.put_char(self.last_char);
                }
            }
            't' if raw == 18 => {
                let (rows, cols) = (self.screen.rows, self.screen.cols);
                self.reply_args(format_args!("\x1b[8;{rows};{cols}t"));
            }
            't' if raw == 14 => {
                let height = self.screen.rows as u32 * self.cell_height;
                let width = self.screen.cols as u32 * self.cell_width;
                self.reply_args(format_args!("\x1b[4;{height};{width}t"));
            }
            't' if raw == 16 => {
                let (height, width) = (self.cell_height, self.cell_width);
                self.reply_args(format_args!("\x1b[6;{height};{width}t"));
            }
            _ => {}
        }
    }

    fn osc_overflow(&mut self) {
        self.osc_overflow = true;
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        if mem::take(&mut self.osc_overflow) {
            return;
        }
        let Some(command) = params.first() else {
            return;
        };
        match *command {
            b"0" | b"2" if params.len() > 1 => self.title = join_text_fields(&params[1..], 256),
            b"7" if params.len() > 1 => self.cwd = join_text_fields(&params[1..], 2048),
            b"133" if !self.modes.alternate_screen => {
                self.shell.mark(&params[1..], self.primary_position());
            }
            b"8" => {
                if params.len() < 3 {
                    return;
                }
                let mut uri = String::new();
                for (i, part) in params[2..].iter().enumerate() {
                    if i > 0 {
                        uri.push(';');
                    }
                    if let Ok(text) = core::str::from_utf8(part) {
                        uri.push_str(text);
                    } else {
                        return;
                    }
                }
                if uri.is_empty() {
                    self.style.link_id = 0;
                    return;
                }
                if uri.len() > 2048 || uri.chars().any(char::is_control) {
                    self.style.link_id = 0;
                    return;
                }
                let id = clean_text(params[1], 256);
                if let Some(index) = self
                    .links
                    .iter()
                    .position(|link| link.id == id && link.uri == uri)
                {
                    self.style.link_id = index as u32 + 1;
                } else if self.links.len() < MAX_LINKS {
                    self.links.push(Link { id, uri });
                    self.style.link_id = self.links.len() as u32;
                } else {
                    self.style.link_id = 0;
                }
            }
            b"4" => {
                for pair in params[1..].as_chunks::<2>().0 {
                    if pair[1] != b"?" {
                        continue;
                    }
                    if let Some(index) = core::str::from_utf8(pair[0])
                        .ok()
                        .and_then(|value| value.parse::<u16>().ok())
                        .filter(|index| *index <= 255)
                    {
                        self.reply_color(4, Some(index), self.theme.color(index), bell_terminated);
                    }
                }
            }
            b"10" | b"11" => {
                let first = if *command == b"10" { 10 } else { 11 };
                for (offset, value) in params[1..].iter().enumerate() {
                    let command = first + offset;
                    if command > 11 {
                        break;
                    }
                    if *value == b"?" {
                        self.reply_color(
                            command as u16,
                            None,
                            if command == 10 {
                                self.theme.foreground
                            } else {
                                self.theme.background
                            },
                            bell_terminated,
                        );
                    }
                }
            }
            // Clipboard requests are deliberately handled by neither parser nor frontend.
            // No child process can read or replace the desktop clipboard through OSC 52.
            _ => {}
        }
    }

    fn hook(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        self.dcs.clear();
        self.dcs_overflow = false;
        self.dcs_query = !ignore && intermediates == b"+" && action == 'q';
        self.sixel_active = !ignore
            && intermediates.is_empty()
            && action == 'q'
            && params.iter().count() <= 3
            && params.iter().all(|group| group.len() == 1);
        if self.sixel_active {
            let aspect = params
                .iter()
                .next()
                .and_then(|p| p.first())
                .copied()
                .unwrap_or(0);
            let background = params
                .iter()
                .nth(1)
                .and_then(|p| p.first())
                .copied()
                .unwrap_or(0);
            let rgb = self.style.bg;
            self.graphics.begin_sixel(
                aspect,
                background == 1,
                [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8],
                if self.sixel_private {
                    None
                } else {
                    self.sixel_palette.as_deref()
                },
            );
        }
    }

    fn put(&mut self, byte: u8) {
        if self.sixel_active {
            self.graphics.sixel_put(byte);
        }
        if self.dcs_query {
            if self.dcs.len() < 512 {
                self.dcs.push(byte);
            } else {
                self.dcs_overflow = true;
            }
        }
    }

    fn unhook(&mut self) {
        if self.sixel_active {
            self.sixel_active = false;
            if self.sixel_complete {
                self.finish_sixel();
            } else {
                self.graphics.abort_sixel();
            }
        }
        if !self.dcs_query || self.dcs_overflow {
            self.dcs_query = false;
            return;
        }
        let mut start = 0;
        while start < self.dcs.len() {
            let end = self.dcs[start..]
                .iter()
                .position(|b| *b == b';')
                .map(|v| start + v)
                .unwrap_or(self.dcs.len());
            let name = &self.dcs[start..end];
            let result = match name {
                b"544e" | b"544E" => Some("787465726d2d323536636f6c6f72"), // TN=xterm-256color
                b"436f" | b"436F" => Some("323536"),                       // Co=256
                b"5463" => Some(""),                                       // Tc boolean
                _ => None,
            };
            if name.iter().all(u8::is_ascii_hexdigit) && name.len().is_multiple_of(2) {
                let name = core::str::from_utf8(name).unwrap_or("").to_string();
                if let Some(value) = result {
                    self.reply(&format!("\x1bP1+r{name}={value}\x1b\\"));
                } else {
                    self.reply(&format!("\x1bP0+r{name}\x1b\\"));
                }
            }
            start = end + 1;
        }
        self.dcs_query = false;
    }
}

fn raw_param(groups: &[&[u16]], index: usize) -> Option<u16> {
    groups.get(index).and_then(|group| group.first()).copied()
}

fn push_history(history: &mut VecDeque<Vec<Cell>>, limit: usize, used: &mut usize, row: &[Cell]) {
    if limit == 0 {
        return;
    }
    let bytes = mem::size_of_val(row);
    if bytes > MAX_HISTORY_BYTES {
        return;
    }
    let mut reusable = None;
    while !history.is_empty() && (history.len() >= limit || *used + bytes > MAX_HISTORY_BYTES) {
        let line = history.pop_front().unwrap();
        *used -= line.capacity() * mem::size_of::<Cell>();
        reusable = Some(line);
    }
    let mut line = reusable.unwrap_or_default();
    if line.capacity() > row.len() * 2 {
        line = Vec::new();
    }
    line.resize(row.len(), Cell::BLANK);
    line.copy_from_slice(row);
    while !history.is_empty()
        && *used + line.capacity() * mem::size_of::<Cell>() > MAX_HISTORY_BYTES
    {
        let old = history.pop_front().unwrap();
        *used -= old.capacity() * mem::size_of::<Cell>();
    }
    *used += line.capacity() * mem::size_of::<Cell>();
    history.push_back(line);
}

fn join_text_fields(fields: &[&[u8]], max: usize) -> String {
    let mut text = String::new();
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            if text.len() == max {
                break;
            }
            text.push(';');
        }
        let Ok(value) = core::str::from_utf8(field) else {
            return String::new();
        };
        for ch in value.chars().filter(|ch| !ch.is_control()) {
            if text.len() + ch.len_utf8() > max {
                return text;
            }
            text.push(ch);
        }
    }
    text
}
fn param(groups: &[&[u16]], index: usize, default: u16) -> u16 {
    match raw_param(groups, index) {
        Some(0) | None => default,
        Some(value) => value,
    }
}
fn rgb(r: Option<u16>, g: Option<u16>, b: Option<u16>) -> Option<u32> {
    match (r, g, b) {
        (Some(r), Some(g), Some(b)) if r <= 255 && g <= 255 && b <= 255 => {
            Some((r as u32) << 16 | (g as u32) << 8 | b as u32)
        }
        _ => None,
    }
}
fn clean_text(bytes: &[u8], max: usize) -> String {
    let mut text = String::new();
    if let Ok(value) = core::str::from_utf8(bytes) {
        for ch in value.chars().filter(|ch| !ch.is_control()) {
            if text.len() + ch.len_utf8() > max {
                break;
            }
            text.push(ch);
        }
    }
    text
}
fn extended_color(index: u16) -> u32 {
    if index >= 232 {
        let level = 8 + (index.min(255) - 232) as u32 * 10;
        return level * 0x010101;
    }
    let n = index - 16;
    let component = |v: u16| if v == 0 { 0 } else { 55 + v as u32 * 40 };
    component(n / 36) << 16 | component(n / 6 % 6) << 8 | component(n % 6)
}
fn dec_graphics(ch: char) -> char {
    match ch {
        '`' => '◆',
        'a' => '▒',
        'b' => '␉',
        'c' => '␌',
        'd' => '␍',
        'e' => '␊',
        'f' => '°',
        'g' => '±',
        'h' => '␤',
        'i' => '␋',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        _ => ch,
    }
}
