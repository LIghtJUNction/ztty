//! Literal UTF-8 history search across soft-wrapped rows. The streaming matcher
//! keeps at most one query's byte positions, never a copy of a logical line or
//! the history. Navigation keeps only a bounded window of matching coordinates.
use alloc::{borrow::Cow, string::String, vec, vec::Vec};
use zterm::Terminal;

const MAX_QUERY: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Case {
    Sensitive,
    Insensitive,
}

enum Characters {
    Exact(core::iter::Once<char>),
    Lower(core::char::ToLowercase),
}

impl Iterator for Characters {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        match self {
            Self::Exact(chars) => chars.next(),
            Self::Lower(chars) => chars.next(),
        }
    }
}

impl Case {
    fn characters(self, ch: char) -> Characters {
        match self {
            Self::Sensitive => Characters::Exact(core::iter::once(ch)),
            Self::Insensitive => Characters::Lower(ch.to_lowercase()),
        }
    }

    // Unicode lowercase comparison preserves original cells for copy/highlight.
    // It does not normalize accents or apply full Unicode case folding.
    fn query(self, query: &str) -> Cow<'_, str> {
        match self {
            Self::Sensitive => Cow::Borrowed(query),
            Self::Insensitive => Cow::Owned(query.chars().flat_map(char::to_lowercase).collect()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub row: usize,
    pub start: usize,
    pub end_row: usize,
    pub end: usize,
    byte: usize,
    revision: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Result {
    pub hit: Option<Hit>,
    pub count: usize,
    pub ordinal: usize,
}

/// Navigation state for one terminal. A full scan computes the initial result;
/// later navigation retains at most 128 hits, without copying history text.
/// A query, case or text revision change discards those coordinates. Drop this
/// state when changing terminals or closing search.
#[derive(Default)]
pub struct Cache {
    query: String,
    key: Option<(u64, Case)>,
    result: Result,
    first: usize,
    hits: Vec<Hit>,
}

const CACHED_HITS: usize = 128;

impl Cache {
    pub fn find(
        &mut self,
        terminal: &Terminal,
        query: &str,
        case: Case,
        from: Option<Hit>,
        forward: bool,
    ) -> Result {
        if query.is_empty() || query.len() > MAX_QUERY {
            *self = Self::default();
            return self.result;
        }
        let key = (terminal.text_revision(), case);
        if self.key != Some(key) || self.query != query {
            self.key = Some(key);
            self.query.clear();
            self.query.reserve_exact(query.len());
            self.query.push_str(query);
            return self.refresh(terminal, query, case, from, forward);
        }
        if self.result.count == 0 {
            return self.result;
        }
        let from = from.filter(|hit| hit.revision == key.0);
        let current = match from {
            None => None,
            Some(hit) if Some(hit) == self.result.hit => Some(self.result.ordinal),
            Some(hit) => match self.hits.iter().position(|&cached| cached == hit) {
                Some(index) => Some(self.first + index),
                None => return self.refresh(terminal, query, case, from, forward),
            },
        };
        let ordinal = match current {
            None => {
                if forward {
                    1
                } else {
                    self.result.count
                }
            }
            Some(current) if forward => {
                if current == self.result.count {
                    1
                } else {
                    current + 1
                }
            }
            Some(current) => {
                if current == 1 {
                    self.result.count
                } else {
                    current - 1
                }
            }
        };
        if ordinal == self.result.ordinal {
            return self.result;
        }
        if ordinal < self.first || ordinal - self.first >= self.hits.len() {
            // Include the previous selection when changing direction. Small
            // result sets fit completely, including both wrap boundaries.
            let (first, end) = if self.result.count <= CACHED_HITS {
                (1, self.result.count)
            } else if forward {
                let first = ordinal.saturating_sub(1).max(1);
                (first, (first + CACHED_HITS - 1).min(self.result.count))
            } else {
                let end = (ordinal + 1).min(self.result.count);
                (end.saturating_sub(CACHED_HITS - 1).max(1), end)
            };
            self.first = first;
            self.hits.clear();
            self.hits.reserve_exact(end - first + 1);
            let normalized = case.query(query);
            let mut count = 0;
            hits(
                terminal,
                &normalized,
                case,
                0,
                terminal.history_len() + terminal.rows(),
                |hit| {
                    count += 1;
                    if count >= first {
                        self.hits.push(hit);
                    }
                    count < end
                },
            );
        }
        let Some(hit) = self.hits.get(ordinal - self.first).copied() else {
            return self.refresh(terminal, query, case, from, forward);
        };
        self.result = Result {
            hit: Some(hit),
            count: self.result.count,
            ordinal,
        };
        self.result
    }

    fn refresh(
        &mut self,
        terminal: &Terminal,
        query: &str,
        case: Case,
        from: Option<Hit>,
        forward: bool,
    ) -> Result {
        self.hits.clear();
        self.first = 0;
        self.result = find(terminal, query, case, from, forward);
        self.result
    }
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
        while self.matched != 0 && byte != self.query[self.matched] {
            self.matched = self.prefix[self.matched - 1];
        }
        if byte != self.query[self.matched] {
            return None;
        }
        // Record only bytes extending a prefix. Every complete match supplies
        // query.len() consecutive positions and overwrites the entire ring.
        self.positions[self.next] = position;
        self.next += 1;
        if self.next == self.positions.len() {
            self.next = 0;
        }
        self.matched += 1;
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
fn hits(
    terminal: &Terminal,
    query: &str,
    case: Case,
    first: usize,
    end: usize,
    mut visit: impl FnMut(Hit) -> bool,
) {
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
            for ch in terminal.cell_chars(cell).flat_map(|ch| case.characters(ch)) {
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
                    ) && !visit(hit)
                    {
                        return;
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

pub fn find(
    terminal: &Terminal,
    query: &str,
    case: Case,
    from: Option<Hit>,
    forward: bool,
) -> Result {
    if query.is_empty() || query.len() > MAX_QUERY {
        return Result {
            hit: None,
            count: 0,
            ordinal: 0,
        };
    }
    let query = case.query(query);
    let from = from.filter(|hit| hit.revision == terminal.text_revision());
    let mut first = None;
    let mut last = None;
    let mut candidate = None;
    let mut count = 0;
    hits(
        terminal,
        &query,
        case,
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
            true
        },
    );
    let selected = candidate.or(if forward { first } else { last });
    Result {
        hit: selected.map(|v| v.0),
        count,
        ordinal: selected.map_or(0, |v| v.1),
    }
}

pub fn highlights(terminal: &Terminal, query: &str, case: Case, history: usize) -> Vec<u64> {
    if query.is_empty() || query.len() > MAX_QUERY {
        return Vec::new();
    }
    let query = case.query(query);
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
    hits(terminal, &query, case, begin, first + rows, |hit| {
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
        true
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
            for ch in terminal.cell_chars(cell) {
                // Only the two endpoint clusters may contribute bytes outside
                // the query. Each retained cluster has an explicit byte cap.
                if text.len() + ch.len_utf8() > MAX_QUERY + 2 * zterm::MAX_GRAPHEME_BYTES {
                    return None;
                }
                text.push(ch);
            }
        }
    }
    Some(text)
}

#[cfg(test)]
mod cache_bounds {
    use super::*;

    #[test]
    fn navigation_storage_is_bounded_and_empty_queries_release_it() {
        let mut terminal = Terminal::new(16, 3, 512);
        terminal.feed("hit\r\n".repeat(300).as_bytes());
        let mut cache = Cache::default();
        let first = cache.find(&terminal, "hit", Case::Sensitive, None, true);
        assert_eq!(
            cache.hits.capacity(),
            0,
            "initial search does not allocate a window"
        );
        let mut result = cache.find(&terminal, "hit", Case::Sensitive, first.hit, true);
        for direction in [true, false] {
            for _ in 0..310 {
                result = cache.find(&terminal, "hit", Case::Sensitive, result.hit, direction);
                assert!(cache.hits.capacity() <= CACHED_HITS);
                assert!(cache.query.capacity() <= MAX_QUERY);
            }
        }
        cache.find(
            &terminal,
            &"x".repeat(MAX_QUERY),
            Case::Sensitive,
            None,
            true,
        );
        assert!(cache.query.capacity() <= MAX_QUERY);
        cache.find(&terminal, "", Case::Sensitive, None, true);
        assert_eq!(cache.hits.capacity(), 0);
        assert_eq!(cache.query.capacity(), 0);
        cache.find(
            &terminal,
            &"x".repeat(MAX_QUERY + 1),
            Case::Sensitive,
            None,
            true,
        );
        assert_eq!(cache.query.capacity(), 0);
    }
}
