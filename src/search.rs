//! Literal UTF-8 history search across soft-wrapped rows. The streaming matcher
//! keeps at most one query's byte positions, never a copy of a logical line or
//! the history. Match counts and navigation do not retain all matches.
use alloc::{string::String, vec, vec::Vec};
use zterm::Terminal;

const MAX_QUERY: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub row: usize,
    pub start: usize,
    pub end_row: usize,
    pub end: usize,
    byte: usize,
    revision: u64,
}

pub struct Result {
    pub hit: Option<Hit>,
    pub count: usize,
    pub ordinal: usize,
}

#[derive(Clone, Copy, Default)]
struct Position {
    row: usize,
    start: usize,
    end: usize,
    byte: usize,
}

struct Matcher<'a> {
    query: &'a [u8],
    prefix: Vec<usize>,
    positions: Vec<Position>,
    next: usize,
    matched: usize,
    revision: u64,
}

impl<'a> Matcher<'a> {
    fn new(query: &'a str, revision: u64) -> Self {
        let query = query.as_bytes();
        let mut prefix = vec![0; query.len()];
        for at in 1..query.len() {
            let mut length = prefix[at - 1];
            while length != 0 && query[at] != query[length] {
                length = prefix[length - 1];
            }
            if query[at] == query[length] {
                length += 1;
            }
            prefix[at] = length;
        }
        Self {
            query,
            prefix,
            positions: vec![Position::default(); query.len()],
            next: 0,
            matched: 0,
            revision,
        }
    }

    fn push(&mut self, byte: u8, position: Position) -> Option<Hit> {
        self.positions[self.next] = position;
        self.next += 1;
        if self.next == self.positions.len() {
            self.next = 0;
        }
        while self.matched != 0 && byte != self.query[self.matched] {
            self.matched = self.prefix[self.matched - 1];
        }
        if byte == self.query[self.matched] {
            self.matched += 1;
        }
        if self.matched != self.query.len() {
            return None;
        }
        // Retain str::match_indices' non-overlapping match semantics.
        self.matched = 0;
        let start = self.positions[self.next];
        Some(Hit {
            row: start.row,
            start: start.start,
            end_row: position.row,
            end: position.end,
            byte: start.byte,
            revision: self.revision,
        })
    }
}

fn coordinates(terminal: &Terminal, absolute: usize) -> (usize, usize) {
    let history = terminal.history_len();
    if absolute < history {
        (0, history - absolute)
    } else {
        (absolute - history, 0)
    }
}

fn row_wrap(terminal: &Terminal, absolute: usize) -> Option<usize> {
    let (row, offset) = coordinates(terminal, absolute);
    terminal.view_row_wrap(row, offset)
}

/// Scan whole rows through `end`, then at most query.len()-1 more bytes so
/// matches ending below a viewport still highlight their visible beginning.
fn hits(terminal: &Terminal, query: &str, first: usize, end: usize, mut visit: impl FnMut(Hit)) {
    let mut matcher = Matcher::new(query, terminal.text_revision());
    let mut suffix = query.len() - 1;
    for absolute in first..terminal.history_len() + terminal.rows() {
        let (row, offset) = coordinates(terminal, absolute);
        let wrapped = terminal.view_row_wrap(row, offset);
        let columns = wrapped.unwrap_or(terminal.cols());
        let mut byte = 0;
        for x in 0..columns {
            let cell = terminal.view_cell(x, row, offset);
            if cell.width == 0 {
                continue;
            }
            for ch in core::iter::once(if cell.ch == '\0' { ' ' } else { cell.ch })
                .chain(cell.combining.into_iter().filter(|ch| *ch != '\0'))
            {
                let mut encoded = [0; 4];
                for &value in ch.encode_utf8(&mut encoded).as_bytes() {
                    if absolute >= end {
                        if suffix == 0 {
                            return;
                        }
                        suffix -= 1;
                    }
                    if let Some(hit) = matcher.push(
                        value,
                        Position {
                            row: absolute,
                            start: x,
                            end: x + usize::from(cell.width) - 1,
                            byte,
                        },
                    ) {
                        visit(hit);
                    }
                    byte += 1;
                }
            }
        }
        if wrapped.is_none() {
            matcher.matched = 0;
            if absolute + 1 >= end {
                return;
            }
        }
    }
}

pub fn find(terminal: &Terminal, query: &str, from: Option<Hit>, forward: bool) -> Result {
    if query.is_empty() || query.len() > MAX_QUERY {
        return Result {
            hit: None,
            count: 0,
            ordinal: 0,
        };
    }
    let from = from.filter(|hit| hit.revision == terminal.text_revision());
    let mut first = None;
    let mut last = None;
    let mut candidate = None;
    let mut count = 0;
    hits(
        terminal,
        query,
        0,
        terminal.history_len() + terminal.rows(),
        |hit| {
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
        },
    );
    let selected = candidate.or(if forward { first } else { last });
    Result {
        hit: selected.map(|v| v.0),
        count,
        ordinal: selected.map_or(0, |v| v.1),
    }
}

pub fn highlights(terminal: &Terminal, query: &str, history: usize) -> Vec<u64> {
    if query.is_empty() || query.len() > MAX_QUERY {
        return Vec::new();
    }
    let cols = terminal.cols();
    let rows = terminal.rows();
    let mut bits = vec![0u64; (cols * rows).div_ceil(64)];
    let first = terminal
        .history_len()
        .saturating_sub(history.min(terminal.history_len()));
    // Start at the logical line boundary. This keeps non-overlapping matches
    // identical to find(), including repeated patterns beginning offscreen.
    let mut begin = first;
    while begin != 0 && row_wrap(terminal, begin - 1).is_some() {
        begin -= 1;
    }
    hits(terminal, query, begin, first + rows, |hit| {
        for absolute in hit.row.max(first)..=hit.end_row.min(first + rows - 1) {
            let start = if absolute == hit.row { hit.start } else { 0 };
            let end = if absolute == hit.end_row {
                hit.end + 1
            } else {
                row_wrap(terminal, absolute).unwrap_or(cols)
            };
            for x in start..end.min(cols) {
                let bit = (absolute - first) * cols + x;
                bits[bit / 64] |= 1 << (bit % 64);
            }
        }
    });
    bits
}

pub fn viewport(terminal: &Terminal, hit: Hit) -> (usize, usize) {
    let height = hit.end_row - hit.row + 1;
    let padding = terminal.rows().saturating_sub(height) / 2;
    let first = hit.row.saturating_sub(padding).min(terminal.history_len());
    (terminal.history_len() - first, hit.row - first)
}

/// Copy the complete cells touched by a hit, including matches taller than the
/// viewport. A hit belongs to the same terminal and becomes invalid after any
/// nonempty input or actual resize, so old history coordinates are never copied.
pub fn text(terminal: &Terminal, hit: Hit) -> Option<String> {
    if hit.revision != terminal.text_revision()
        || hit.end_row >= terminal.history_len() + terminal.rows()
    {
        return None;
    }
    let mut text = String::new();
    for absolute in hit.row..=hit.end_row {
        let (row, offset) = coordinates(terminal, absolute);
        let start = if absolute == hit.row { hit.start } else { 0 };
        let end = if absolute == hit.end_row {
            hit.end + 1
        } else {
            terminal.view_row_wrap(row, offset)?
        };
        for x in start..end {
            let cell = terminal.view_cell(x, row, offset);
            if cell.width == 0 {
                continue;
            }
            for ch in core::iter::once(if cell.ch == '\0' { ' ' } else { cell.ch })
                .chain(cell.combining.into_iter().filter(|ch| *ch != '\0'))
            {
                // Only the two endpoint cells can add scalars outside the
                // query; each stores a base scalar and two combining scalars.
                if text.len() + ch.len_utf8() > MAX_QUERY + 16 {
                    return None;
                }
                text.push(ch);
            }
        }
    }
    Some(text)
}
