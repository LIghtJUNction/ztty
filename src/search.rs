//! Literal UTF-8 history search. Only one row and its cell mapping are copied;
//! match counts and next/previous navigation do not retain all history matches.
use alloc::{string::String, vec, vec::Vec};
use zterm::Terminal;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub row: usize,
    pub start: usize,
    pub end: usize,
    byte: usize,
}

pub struct Result {
    pub hit: Option<Hit>,
    pub count: usize,
    pub ordinal: usize,
}

struct Row {
    text: String,
    cells: Vec<(usize, usize, usize)>,
}

impl Row {
    fn new() -> Self {
        Self {
            text: String::new(),
            cells: Vec::new(),
        }
    }

    fn load(&mut self, terminal: &Terminal, absolute: usize) {
        self.text.clear();
        self.cells.clear();
        let history = terminal.history_len();
        let (y, offset) = if absolute < history {
            (0, history - absolute)
        } else {
            (absolute - history, 0)
        };
        for x in 0..terminal.cols() {
            let cell = terminal.view_cell(x, y, offset);
            if cell.width == 0 {
                continue;
            }
            self.cells
                .push((self.text.len(), x, usize::from(cell.width)));
            self.text.push(if cell.ch == '\0' { ' ' } else { cell.ch });
            for ch in cell.combining {
                if ch != '\0' {
                    self.text.push(ch);
                }
            }
        }
    }

    fn hits(&self, query: &str, row: usize, mut visit: impl FnMut(Hit)) {
        for (byte, _) in self.text.match_indices(query) {
            let a = self
                .cells
                .partition_point(|(offset, _, _)| *offset <= byte)
                .saturating_sub(1);
            let b = self
                .cells
                .partition_point(|(offset, _, _)| *offset < byte + query.len())
                .saturating_sub(1);
            if let (Some(&(_, start, _)), Some(&(_, end, width))) =
                (self.cells.get(a), self.cells.get(b))
            {
                visit(Hit {
                    row,
                    start,
                    end: end + width.saturating_sub(1),
                    byte,
                });
            }
        }
    }
}

pub fn find(terminal: &Terminal, query: &str, from: Option<Hit>, forward: bool) -> Result {
    if query.is_empty() || query.len() > 1024 {
        return Result {
            hit: None,
            count: 0,
            ordinal: 0,
        };
    }
    let mut row = Row::new();
    let mut first = None;
    let mut last = None;
    let mut candidate = None;
    let mut count = 0;
    for y in 0..terminal.history_len() + terminal.rows() {
        row.load(terminal, y);
        row.hits(query, y, |hit| {
            count += 1;
            let record = (hit, count);
            if first.is_none() {
                first = Some(record);
            }
            last = Some(record);
            if let Some(from) = from {
                let location = (hit.row, hit.byte);
                let start = (from.row, from.byte);
                if forward && location > start && candidate.is_none()
                    || !forward && location < start
                {
                    candidate = Some(record);
                }
            }
        });
    }
    let selected = candidate.or(if forward { first } else { last });
    Result {
        hit: selected.map(|v| v.0),
        count,
        ordinal: selected.map_or(0, |v| v.1),
    }
}

pub fn highlights(terminal: &Terminal, query: &str, history: usize) -> Vec<u64> {
    if query.is_empty() || query.len() > 1024 {
        return Vec::new();
    }
    let cols = terminal.cols();
    let rows = terminal.rows();
    let mut bits = vec![0u64; (cols * rows).div_ceil(64)];
    let first = terminal
        .history_len()
        .saturating_sub(history.min(terminal.history_len()));
    let mut row = Row::new();
    for y in 0..rows {
        row.load(terminal, first + y);
        row.hits(query, y, |hit| {
            for x in hit.start..=hit.end.min(cols.saturating_sub(1)) {
                let bit = y * cols + x;
                bits[bit / 64] |= 1 << (bit % 64);
            }
        });
    }
    bits
}

pub fn viewport(terminal: &Terminal, hit: Hit) -> (usize, usize) {
    let first = hit
        .row
        .saturating_sub(terminal.rows() / 2)
        .min(terminal.history_len());
    (terminal.history_len() - first, hit.row - first)
}
