//! Bounded text extraction shared by the three native clipboard frontends.
use alloc::string::String;
use zterm::Terminal;

const MAX_BYTES: usize = 4 * 1024 * 1024;

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
