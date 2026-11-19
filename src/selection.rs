//! Bounded text extraction shared by the three native clipboard frontends.
use alloc::string::String;
use zterm::Terminal;

const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unit {
    Cell,
    Word,
    Line,
}

/// Expand a pointer position to a complete visible selection unit. Only
/// explicit soft wraps join rows; hard newlines always end a word or line.
pub fn unit(
    terminal: &Terminal,
    point: (usize, usize),
    history_offset: usize,
    unit: Unit,
) -> ((usize, usize), (usize, usize)) {
    let cols = terminal.cols();
    let rows = terminal.rows();
    if cols == 0 || rows == 0 {
        return ((0, 0), (0, 0));
    }
    let offset = history_offset.min(terminal.history_len());
    let leading = |(mut col, row): (usize, usize)| {
        if col != 0 && terminal.view_cell(col, row, offset).width == 0 {
            col -= 1;
        }
        (col, row)
    };
    let tail = |(col, row): (usize, usize)| {
        let width = usize::from(terminal.view_cell(col, row, offset).width).max(1);
        ((col + width - 1).min(cols - 1), row)
    };
    let point = leading((point.0.min(cols - 1), point.1.min(rows - 1)));
    if unit == Unit::Cell {
        return (point, tail(point));
    }
    if unit == Unit::Line {
        let mut first = point.1;
        let mut last = point.1;
        while first != 0 && terminal.view_row_wrap(first - 1, offset).is_some() {
            first -= 1;
        }
        while last + 1 < rows && terminal.view_row_wrap(last, offset).is_some() {
            last += 1;
        }
        return ((0, first), (cols - 1, last));
    }
    let class = |(col, row)| {
        let ch = terminal.view_cell(col, row, offset).ch;
        if ch == '\0' || ch.is_whitespace() {
            0
        } else if "()[]{}<>,;\"'`".contains(ch) {
            1
        } else {
            // Keep paths, URLs, identifiers and Unicode text together.
            2
        }
    };
    let previous = |(col, row)| {
        if col != 0 {
            Some(leading((col - 1, row)))
        } else if row != 0 {
            terminal
                .view_row_wrap(row - 1, offset)
                .filter(|limit| *limit != 0)
                .map(|limit| leading((limit.min(cols) - 1, row - 1)))
        } else {
            None
        }
    };
    let next = |point: (usize, usize)| {
        let (col, row) = tail(point);
        let wrap = terminal.view_row_wrap(row, offset);
        if col + 1 < wrap.unwrap_or(cols).min(cols) {
            Some((col + 1, row))
        } else if wrap.is_some() && row + 1 < rows {
            Some((0, row + 1))
        } else {
            None
        }
    };
    let kind = class(point);
    let mut first = point;
    let mut last = point;
    while let Some(point) = previous(first).filter(|point| class(*point) == kind) {
        first = point;
    }
    while let Some(point) = next(last).filter(|point| class(*point) == kind) {
        last = point;
    }
    (first, tail(last))
}

/// Read an inclusive selection in viewport coordinates. Physical rows joined
/// by terminal autowrap stay one logical line; explicit line breaks remain.
pub fn text(
    terminal: &Terminal,
    start: (usize, usize),
    end: (usize, usize),
    history_offset: usize,
) -> String {
    let cols = terminal.cols();
    if cols == 0 {
        return String::new();
    }
    let start = (start.1, start.0.min(cols - 1));
    let end = (end.1, end.0.min(cols - 1));
    let (start, end) = (start.min(end), start.max(end));
    let history = terminal.history_len();
    let first = history.saturating_sub(history_offset.min(history));
    let available = history + terminal.rows() - first;
    if start.0 >= available {
        return String::new();
    }
    let last = end.0.min(available - 1);
    let mut output = String::new();
    for row in start.0..=last {
        // view_cell accepts a visible row, so rebase every absolute history
        // row. This also copies search hits extending past the viewport.
        let absolute = first + row;
        let (view_row, offset) = if absolute < history {
            (0, history - absolute)
        } else {
            (absolute - history, 0)
        };
        let wrap = terminal.view_row_wrap(view_row, offset);
        let mut begin = if row == start.0 { start.1 } else { 0 };
        let limit = if row == end.0 { end.1 + 1 } else { cols };
        let limit = limit.min(wrap.unwrap_or(cols));
        if begin != 0 && terminal.view_cell(begin, view_row, offset).width == 0 {
            begin -= 1;
        }
        let line_start = output.len();
        for col in begin..limit {
            let cell = terminal.view_cell(col, view_row, offset);
            if cell.width == 0 {
                continue;
            }
            let ch = if cell.ch == '\0' { ' ' } else { cell.ch };
            let bytes = ch.len_utf8()
                + cell
                    .combining
                    .iter()
                    .filter(|ch| **ch != '\0')
                    .map(|ch| ch.len_utf8())
                    .sum::<usize>();
            if output.len() + bytes > MAX_BYTES {
                return output;
            }
            output.push(ch);
            for ch in cell.combining {
                if ch != '\0' {
                    output.push(ch);
                }
            }
        }
        // Soft-wrap metadata distinguishes meaningful line-end spaces from
        // untouched padding. Preserve them when joining the following row.
        if wrap.is_none() {
            while output.len() > line_start && output.ends_with(' ') {
                output.pop();
            }
            if row != last {
                if output.len() == MAX_BYTES {
                    return output;
                }
                output.push('\n');
            }
        }
    }
    output
}
