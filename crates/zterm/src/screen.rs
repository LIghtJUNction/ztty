use alloc::vec;
use alloc::vec::Vec;

pub const DEFAULT_FG: u32 = 0xd9e2ef;
pub const DEFAULT_BG: u32 = 0x0b0f14;
pub const BOLD: u16 = 1;
pub const DIM: u16 = 2;
pub const ITALIC: u16 = 4;
pub const UNDERLINE: u16 = 8;
pub const INVERSE: u16 = 16;
pub const STRIKE: u16 = 32;
pub const HIDDEN: u16 = 64;
/// The background was selected by an SGR color sequence.
pub const EXPLICIT_BG: u16 = 128;
/// The foreground was selected by an SGR color sequence.
pub const EXPLICIT_FG: u16 = 256;
/// The foreground refers to the ANSI palette, indexed by `Cell::palette_indices`.
pub const PALETTE_FG: u16 = 512;
/// The background refers to the ANSI palette, indexed by `Cell::palette_indices`.
pub const PALETTE_BG: u16 = 1024;
const COLOR_FLAGS: u16 = EXPLICIT_FG | EXPLICIT_BG | PALETTE_FG | PALETTE_BG;

/// A character cell. A wide glyph's trailing cell has `width == 0`.
/// Two combining scalars are stored inline, without per-cell allocations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub combining: [char; 2],
    pub fg: u32,
    pub bg: u32,
    pub flags: u16,
    pub width: u8,
    /// ANSI color indices: foreground in bits 0..4, background in bits 4..8.
    /// Each index is meaningful only when its `PALETTE_FG`/`PALETTE_BG` flag is set.
    /// This occupies an existing padding byte; a cell remains 28 bytes.
    pub palette_indices: u8,
    pub link_id: u32,
}

impl Default for Cell {
    fn default() -> Self {
        Self::BLANK
    }
}

impl Cell {
    pub const BLANK: Self = Self {
        ch: ' ',
        combining: ['\0'; 2],
        fg: DEFAULT_FG,
        bg: DEFAULT_BG,
        flags: 0,
        width: 1,
        palette_indices: 0,
        link_id: 0,
    };
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Style {
    pub fg: u32,
    pub bg: u32,
    pub flags: u16,
    pub palette_indices: u8,
    pub link_id: u32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fg: DEFAULT_FG,
            bg: DEFAULT_BG,
            flags: 0,
            palette_indices: 0,
            link_id: 0,
        }
    }
}

impl Style {
    pub fn cell(self, ch: char, width: u8) -> Cell {
        Cell {
            ch,
            combining: ['\0'; 2],
            fg: self.fg,
            bg: self.bg,
            flags: self.flags,
            width,
            palette_indices: self.palette_indices,
            link_id: self.link_id,
        }
    }

    pub fn blank(self) -> Cell {
        // Erased cells retain the current background, never a hyperlink.
        Cell {
            fg: self.fg,
            bg: self.bg,
            flags: self.flags & COLOR_FLAGS,
            palette_indices: self.palette_indices,
            ..Cell::BLANK
        }
    }

    pub fn set_color(&mut self, foreground: bool, color: u32, palette: Option<u16>) {
        let (target, explicit, indexed, shift) = if foreground {
            (&mut self.fg, EXPLICIT_FG, PALETTE_FG, 0)
        } else {
            (&mut self.bg, EXPLICIT_BG, PALETTE_BG, 4)
        };
        *target = color;
        self.flags = (self.flags | explicit) & !indexed;
        self.palette_indices &= !(15 << shift);
        if let Some(index) = palette.filter(|index| *index < 16) {
            self.flags |= indexed;
            self.palette_indices |= (index as u8) << shift;
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct SavedCursor {
    pub x: usize,
    pub y: usize,
    pub style: Style,
    pub origin: bool,
    pub g0: bool,
    pub g1: bool,
    pub use_g1: bool,
    pub wrap: bool,
}

pub(crate) struct Screen {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    pub blank: Cell,
    pub row0: usize,
    /// Soft-wrap endpoints, indexed by the same physical rows as `cells`.
    /// Zero is a hard boundary; otherwise only these columns precede the wrap.
    /// A wide glyph may wrap before the final (padding) column.
    pub wrapped: Vec<usize>,
    pub x: usize,
    pub y: usize,
    pub wrap: bool,
    pub dirty: Vec<bool>,
    pub top: usize,
    pub bottom: usize,
    pub origin: bool,
    pub saved: SavedCursor,
    pub keyboard: u8,
    pub keyboard_stack: [u8; 32],
    pub keyboard_depth: usize,
}

impl Screen {
    pub fn new(cols: usize, rows: usize, style: Style) -> Self {
        let blank = style.blank();
        Self {
            cols,
            rows,
            cells: vec![blank; cols * rows],
            blank,
            row0: 0,
            wrapped: vec![0; rows],
            x: 0,
            y: 0,
            wrap: false,
            dirty: vec![true; rows],
            top: 0,
            bottom: rows - 1,
            origin: false,
            saved: SavedCursor {
                style,
                ..SavedCursor::default()
            },
            keyboard: 0,
            keyboard_stack: [0; 32],
            keyboard_depth: 0,
        }
    }

    #[inline]
    fn row_index(&self, y: usize) -> usize {
        debug_assert!(y < self.rows);
        // Both terms are below rows, so a single subtraction wraps the ring.
        // Avoid a dynamic integer division for every cell or row lookup.
        let row = self.row0 + y;
        if row >= self.rows {
            row - self.rows
        } else {
            row
        }
    }

    #[inline]
    pub fn index(&self, x: usize, y: usize) -> usize {
        self.row_index(y) * self.cols + x
    }

    pub fn row(&self, y: usize) -> &[Cell] {
        let start = self.index(0, y);
        &self.cells[start..start + self.cols]
    }

    pub fn row_mut(&mut self, y: usize) -> &mut [Cell] {
        let start = self.index(0, y);
        &mut self.cells[start..start + self.cols]
    }

    pub fn row_wrap(&self, y: usize) -> usize {
        self.wrapped[self.row_index(y)]
    }

    pub fn set_row_wrap(&mut self, y: usize, columns: usize) {
        let row = self.row_index(y);
        self.wrapped[row] = columns;
    }

    pub fn clear_row(&mut self, y: usize, blank: Cell) {
        self.row_mut(y).fill(blank);
        self.set_row_wrap(y, 0);
    }

    pub fn mark_all(&mut self) {
        self.dirty.fill(true);
    }

    pub fn clear(&mut self, blank: Cell) {
        self.cells.fill(blank);
        self.wrapped.fill(0);
        self.mark_all();
        self.wrap = false;
    }

    /// Remove both halves of any wide character intersecting the interval.
    pub fn clear_range(&mut self, y: usize, mut start: usize, mut end: usize, blank: Cell) {
        start = start.min(self.cols);
        end = end.min(self.cols);
        if start >= end {
            return;
        }
        let cols = self.cols;
        let row = self.row_mut(y);
        if start > 0 && row[start].width == 0 {
            start -= 1;
        }
        if end < cols && row[end].width == 0 {
            end += 1;
        }
        row[start..end].fill(blank);
        if end >= self.row_wrap(y) {
            self.set_row_wrap(y, 0);
        }
        self.dirty[y] = true;
    }

    /// Prepare a nonempty interval that the caller will overwrite completely.
    /// Only wide-character halves outside that interval need to be cleared.
    #[inline(always)]
    pub fn overwrite_span(
        &mut self,
        y: usize,
        start: usize,
        end: usize,
        blank: Cell,
    ) -> &mut [Cell] {
        debug_assert!(start < end && end <= self.cols);
        let wrapped = self.row_wrap(y);
        if wrapped != 0 && end > wrapped {
            self.set_row_wrap(y, 0);
        }
        let row = self.row_mut(y);
        if start > 0 && row[start].width == 0 {
            row[start - 1] = blank;
        }
        if end < row.len() && row[end].width == 0 {
            row[end] = blank;
        }
        &mut row[start..end]
    }

    pub fn repair_row(&mut self, y: usize, blank: Cell) {
        let row = self.row_mut(y);
        let mut x = 0;
        while x < row.len() {
            if row[x].width == 2 {
                if x + 1 == row.len() || row[x + 1].width != 0 {
                    row[x] = blank;
                } else {
                    x += 1;
                }
            } else if row[x].width == 0 {
                row[x] = blank;
            }
            x += 1;
        }
        self.dirty[y] = true;
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        if self.cols == cols && self.rows == rows {
            return;
        }
        // A height-only resize leaves the right margin unchanged. Keep a
        // deferred wrap so the next glyph cannot overwrite its final cell.
        let wrap = self.wrap && cols == self.cols;
        // Keep the cursor's row visible when shrinking vertically. The owner
        // records any discarded primary-screen rows in scrollback first.
        let dropped = self.y.saturating_add(1).saturating_sub(rows);
        let mut cells = vec![self.blank; cols * rows];
        let copied = rows.min(self.rows - dropped);
        let mut wrapped = vec![0; rows];
        for y in 0..copied {
            let count = cols.min(self.cols);
            cells[y * cols..y * cols + count].copy_from_slice(&self.row(y + dropped)[..count]);
            let endpoint = self.row_wrap(y + dropped);
            // Narrowing clips text rather than reflowing it. Do not bridge
            // omitted columns or a successor row discarded by this resize.
            if endpoint <= cols && y + 1 < copied {
                wrapped[y] = endpoint;
            }
        }
        self.cells = cells;
        self.wrapped = wrapped;
        self.cols = cols;
        self.rows = rows;
        self.row0 = 0;
        self.x = self.x.min(cols - 1);
        self.y = self.y.saturating_sub(dropped).min(rows - 1);
        self.saved.x = self.saved.x.min(cols - 1);
        self.saved.y = self.saved.y.saturating_sub(dropped).min(rows - 1);
        self.wrap = wrap;
        self.top = 0;
        self.bottom = rows - 1;
        self.dirty = vec![true; rows];
        for y in 0..rows {
            self.repair_row(y, self.blank);
        }
    }
}
