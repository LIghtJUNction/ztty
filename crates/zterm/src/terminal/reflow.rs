//! Width reflow of retained primary text, with a sparse set of requested anchors.
//! Two bounded passes avoid a per-cell coordinate map or an unbounded logical line.
use super::*;
use alloc::vec;
use unicode_width::UnicodeWidthStr;

/// A retained text location to remap during a resize. A cell anchor refers to
/// the complete displayed cell; a boundary may also point just after a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResizeAnchor {
    pub position: Option<crate::TextPosition>,
    pub(super) cell: bool,
    preserve_columns: bool,
}

impl ResizeAnchor {
    pub fn cell(position: Option<crate::TextPosition>) -> Self {
        Self {
            position,
            cell: true,
            preserve_columns: false,
        }
    }

    pub fn boundary(position: Option<crate::TextPosition>) -> Self {
        Self {
            position,
            cell: false,
            preserve_columns: false,
        }
    }

    fn preserve(mut self) -> Self {
        self.preserve_columns = true;
        self
    }
}

#[derive(Clone, Copy)]
struct Mark {
    old: crate::TextPosition,
    index: usize,
    cell: bool,
    preserve_columns: bool,
}

struct Input<'a> {
    history: &'a VecDeque<HistoryRow>,
    screen: &'a Screen,
    graphemes: &'a crate::grapheme::Graphemes,
    oldest: u64,
    limits: Vec<usize>,
    marks: Vec<Mark>,
}

impl<'a> Input<'a> {
    fn new(
        history: &'a VecDeque<HistoryRow>,
        screen: &'a Screen,
        graphemes: &'a crate::grapheme::Graphemes,
        line_base: u64,
        anchors: &[ResizeAnchor],
    ) -> Self {
        let oldest = line_base.saturating_sub(history.len() as u64);
        let last = screen.rows - 1;
        let mut end = screen.y.max(screen.saved.y).min(last);
        for row in 0..screen.rows {
            if screen.row(row).iter().any(|cell| *cell != screen.blank) {
                end = end.max(row);
            }
        }
        for anchor in anchors.iter().filter_map(|anchor| anchor.position) {
            if anchor.line >= line_base && anchor.line - line_base <= last as u64 {
                end = end.max((anchor.line - line_base) as usize);
            }
        }
        let mut input = Self {
            history,
            screen,
            graphemes,
            oldest,
            limits: Vec::with_capacity(history.len() + end + 1),
            marks: Vec::with_capacity(anchors.len()),
        };
        for (index, anchor) in anchors.iter().enumerate() {
            let Some(old) = anchor.position else { continue };
            let Some(relative) = old
                .line
                .checked_sub(oldest)
                .and_then(|v| usize::try_from(v).ok())
            else {
                continue;
            };
            if relative > history.len() + end {
                continue;
            }
            let (cells, _) = input.row(relative);
            if old.column > cells.len() || anchor.cell && old.column == cells.len() {
                continue;
            }
            input.marks.push(Mark {
                old,
                index,
                cell: anchor.cell,
                preserve_columns: anchor.preserve_columns,
            });
        }
        input.marks.sort_by_key(|mark| mark.old);
        let mut next = 0;
        for row in 0..history.len() + end + 1 {
            let (cells, wrapped) = input.row(row);
            let mut length = if wrapped != 0 {
                wrapped.min(cells.len())
            } else {
                cells
                    .iter()
                    .rposition(|cell| *cell != screen.blank)
                    .map_or(0, |x| x + 1)
            };
            let line = oldest.saturating_add(row as u64);
            while next < input.marks.len() && input.marks[next].old.line == line {
                let mark = input.marks[next];
                if wrapped == 0 && mark.preserve_columns {
                    length = length
                        .max(mark.old.column + usize::from(mark.cell))
                        .min(cells.len());
                }
                next += 1;
            }
            input.limits.push(length);
        }
        input
    }

    fn row(&self, row: usize) -> (&[Cell], usize) {
        if row < self.history.len() {
            let row = &self.history[row];
            (&row.cells, row.wrapped)
        } else {
            let row = row - self.history.len();
            (self.screen.row(row), self.screen.row_wrap(row))
        }
    }

    fn width(&self, cell: &Cell, columns: usize) -> usize {
        let width = self.graphemes.get(cell.grapheme_id).map_or_else(
            || UnicodeWidthChar::width(cell.ch).unwrap_or(1),
            UnicodeWidthStr::width,
        );
        width.clamp(1, 2).min(columns)
    }

    /// The output callback sees one bounded row, never a joined logical line.
    fn walk(
        &self,
        columns: usize,
        mapped: &mut [Option<crate::TextPosition>],
        mut emit: impl FnMut(usize, &[Cell], usize),
    ) -> usize {
        let mut row = vec![self.screen.blank; columns];
        let mut y = 0usize;
        let mut x = 0usize;
        let mut mark_index = 0usize;
        for old_row in 0..self.limits.len() {
            let (cells, wrapped) = self.row(old_row);
            let line = self.oldest.saturating_add(old_row as u64);
            let limit = self.limits[old_row];
            let mut old_x = 0;
            while old_x < limit {
                let mut cell = cells[old_x];
                let consumed = usize::from(cell.width).max(1).min(limit - old_x);
                if cell.width == 0 {
                    cell = self.screen.blank;
                }
                let width = self.width(&cell, columns);
                if x + width > columns {
                    emit(y, &row, x);
                    y += 1;
                    x = 0;
                    row.fill(self.screen.blank);
                }
                while mark_index < self.marks.len() {
                    let mark = self.marks[mark_index];
                    if mark.old.line != line || mark.old.column >= old_x + consumed {
                        break;
                    }
                    let inside = mark.old.column.saturating_sub(old_x);
                    let delta = inside.min(if mark.cell { width - 1 } else { width });
                    if let Some(target) = mapped.get_mut(mark.index) {
                        *target = Some(crate::TextPosition {
                            line: y as u64,
                            column: x + delta,
                        });
                    }
                    mark_index += 1;
                }
                cell.width = width as u8;
                row[x] = cell;
                if width == 2 {
                    row[x + 1] = Cell {
                        ch: '\0',
                        combining: ['\0'; 2],
                        width: 0,
                        grapheme_id: 0,
                        ..cell
                    };
                }
                x += width;
                old_x += consumed;
            }
            while mark_index < self.marks.len() && self.marks[mark_index].old.line == line {
                let mark = self.marks[mark_index];
                if (!mark.cell || wrapped == 0 && x < columns)
                    && let Some(target) = mapped.get_mut(mark.index)
                {
                    *target = Some(crate::TextPosition {
                        line: y as u64,
                        column: x,
                    });
                }
                mark_index += 1;
            }
            if wrapped == 0 || old_row + 1 == self.limits.len() {
                emit(y, &row, 0);
                y += 1;
                x = 0;
                row.fill(self.screen.blank);
            }
        }
        y
    }
}

impl State {
    fn reflow_screen(
        &mut self,
        mut screen: Screen,
        graphics: &mut zgraphics::Graphics,
        columns: usize,
        rows: usize,
        external: &mut [ResizeAnchor],
        last_grapheme: Option<(usize, usize)>,
    ) -> Screen {
        let old_base = self.line_base;
        let position = |x: usize, y: usize| {
            Some(crate::TextPosition {
                line: old_base.saturating_add(y as u64),
                column: x,
            })
        };
        let mut anchors = vec![
            ResizeAnchor::boundary(position(screen.x + usize::from(screen.wrap), screen.y))
                .preserve(),
            ResizeAnchor::boundary(position(
                screen.saved.x + usize::from(screen.saved.wrap),
                screen.saved.y,
            ))
            .preserve(),
            ResizeAnchor::cell(last_grapheme.and_then(|(x, y)| position(x, y))),
        ];
        let external_start = anchors.len();
        anchors.extend_from_slice(external);
        let shell_start = anchors.len();
        for record in &self.shell.records {
            anchors.extend([
                ResizeAnchor::boundary(Some(record.prompt)).preserve(),
                ResizeAnchor::boundary(record.input).preserve(),
                ResizeAnchor::boundary(record.output).preserve(),
                ResizeAnchor::boundary(record.end).preserve(),
            ]);
        }
        let graphics_start = anchors.len();
        for (col, row) in graphics.anchors() {
            anchors.push(
                ResizeAnchor::cell(
                    old_base
                        .checked_add_signed(i64::from(row))
                        .zip(usize::try_from(col).ok())
                        .map(|(line, column)| crate::TextPosition { line, column }),
                )
                .preserve(),
            );
        }
        let input = Input::new(&self.history, &screen, &self.graphemes, old_base, &anchors);
        let mut mapped = vec![None; anchors.len()];
        let total = input.walk(columns, &mut mapped, |_, _, _| {});
        let cursor = mapped[0].expect("the retained primary cursor has an anchor");
        // Preserve the logical cursor. If nonblank rows below an addressed
        // cursor cannot fit, they follow the grid-resize clipping policy.
        let first_screen = total.saturating_sub(rows).min(cursor.line as usize);
        let end_screen = first_screen.saturating_add(rows);
        let mut cells = vec![screen.blank; columns * rows];
        let mut wrapped = vec![0; rows];
        let mut history = VecDeque::new();
        let mut used = 0;
        input.walk(columns, &mut [], |row, content, wrap| {
            if row < first_screen {
                push_history(&mut history, self.history_limit, &mut used, content, wrap);
            } else if row < end_screen {
                let y = row - first_screen;
                cells[y * columns..(y + 1) * columns].copy_from_slice(content);
                wrapped[y] = if row + 1 < end_screen { wrap } else { 0 };
            }
        });
        let oldest = input.oldest;
        drop(input);
        let first_retained = first_screen.saturating_sub(history.len());
        for point in &mut mapped {
            *point = point
                .filter(|point| {
                    point.line >= first_retained as u64 && point.line < end_screen as u64
                })
                .map(|point| crate::TextPosition {
                    line: oldest.saturating_add(point.line),
                    column: point.column,
                });
        }
        self.line_base = oldest.saturating_add(first_screen as u64);
        self.history = history;
        self.history_bytes = used;
        for (target, point) in external.iter_mut().zip(&mapped[external_start..]) {
            target.position = *point;
        }
        for (index, record) in self.shell.records.iter_mut().enumerate() {
            let points = &mapped[shell_start + index * 4..shell_start + index * 4 + 4];
            if let Some(prompt) = points[0] {
                record.prompt = prompt;
            } else {
                record.available = false;
            }
            for (target, point) in [&mut record.input, &mut record.output, &mut record.end]
                .into_iter()
                .zip(&points[1..])
            {
                if target.is_some() && point.is_none() {
                    record.available = false;
                }
                *target = *point;
            }
        }
        let mut index = graphics_start;
        graphics.remap_anchors(|_, _| {
            let point = mapped[index];
            index += 1;
            point.and_then(|point| {
                let row = i128::from(point.line) - i128::from(self.line_base);
                Some((i32::try_from(point.column).ok()?, i32::try_from(row).ok()?))
            })
        });
        let place_cursor = |point: crate::TextPosition| {
            let row = point
                .line
                .saturating_sub(self.line_base)
                .min(rows as u64 - 1) as usize;
            (point.column.min(columns - 1), row, point.column >= columns)
        };
        let (x, y, wrap) = place_cursor(mapped[0].expect("visible cursor is retained"));
        let saved = mapped[1].map(place_cursor);
        self.last_grapheme = mapped[2]
            .filter(|point| point.line >= self.line_base)
            .map(|point| {
                (
                    point.column.min(columns - 1),
                    (point.line - self.line_base) as usize,
                )
            });
        screen.cells = cells;
        screen.wrapped = wrapped;
        screen.cols = columns;
        screen.rows = rows;
        screen.row0 = 0;
        screen.x = x;
        screen.y = y;
        screen.wrap = wrap && self.autowrap;
        if let Some((x, y, wrap)) = saved {
            screen.saved.x = x;
            screen.saved.y = y;
            screen.saved.wrap = wrap && self.autowrap;
        } else {
            screen.saved.x = screen.saved.x.min(columns - 1);
            screen.saved.y = 0;
            screen.saved.wrap = false;
        }
        screen.top = 0;
        screen.bottom = rows - 1;
        screen.dirty = vec![true; rows];
        screen
    }

    pub(super) fn reflow_primary(
        &mut self,
        columns: usize,
        rows: usize,
        anchors: &mut [ResizeAnchor],
    ) {
        let alternate = self.modes.alternate_screen;
        let last_grapheme = self.last_grapheme;
        let last_cell = last_grapheme.and_then(|(x, y)| {
            (x < self.screen.cols && y < self.screen.rows).then(|| self.screen.row(y)[x])
        });
        let primary = if alternate {
            self.primary
                .take()
                .expect("alternate screen owns an inactive primary")
        } else {
            mem::replace(&mut self.screen, Screen::new(1, 1, self.style))
        };
        let mut graphics = if alternate {
            mem::take(&mut self.inactive_graphics)
        } else {
            mem::take(&mut self.graphics)
        };
        let primary = self.reflow_screen(
            primary,
            &mut graphics,
            columns,
            rows,
            if alternate { &mut [] } else { anchors },
            if alternate { None } else { last_grapheme },
        );
        if alternate {
            self.primary = Some(primary);
            self.inactive_graphics = graphics;
            self.last_grapheme = last_grapheme;
            let dropped = self.screen.y.saturating_add(1).saturating_sub(rows);
            self.graphics
                .scroll(0, self.screen.rows - 1, -(dropped as i32), 0, false);
            for anchor in anchors {
                anchor.position = anchor.position.and_then(|point| {
                    let row = usize::try_from(point.line).ok()?;
                    if row < dropped
                        || row >= self.screen.rows
                        || row - dropped >= rows
                        || point.column > columns
                    {
                        return None;
                    }
                    if anchor.cell {
                        let cell = self.screen.row(row).get(point.column)?;
                        if point.column >= columns
                            || point.column + usize::from(cell.width) > columns
                        {
                            return None;
                        }
                    }
                    Some(crate::TextPosition {
                        line: (row - dropped) as u64,
                        column: point.column,
                    })
                });
            }
            self.screen.resize(columns, rows);
            self.last_grapheme = last_grapheme
                .and_then(|(x, y)| {
                    (x < columns && y >= dropped && y - dropped < rows).then(|| (x, y - dropped))
                })
                .filter(|(x, y)| last_cell == Some(self.screen.row(*y)[*x]));
        } else {
            self.screen = primary;
            self.graphics = graphics;
            if let Some(alternate) = self.inactive_alternate.as_mut() {
                let dropped = alternate.y.saturating_add(1).saturating_sub(rows);
                self.inactive_graphics
                    .scroll(0, alternate.rows - 1, -(dropped as i32), 0, false);
                alternate.resize(columns, rows);
            }
        }
        self.graphics
            .set_viewport(self.cell_width, self.cell_height, rows);
        self.inactive_graphics
            .set_viewport(self.cell_width, self.cell_height, rows);
        self.graphics
            .trim_history(if alternate { 0 } else { self.history.len() });
        self.inactive_graphics
            .trim_history(if alternate { self.history.len() } else { 0 });
    }
}
