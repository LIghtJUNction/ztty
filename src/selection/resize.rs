//! Selection and viewport anchors shared by the native resize paths.
use super::*;
use zterm::{ResizeAnchor, TextPosition};

pub type Points = ((usize, usize), (usize, usize));

pub struct ViewAnchor {
    anchors: [ResizeAnchor; 3],
    following: bool,
    reversed: bool,
    selected: bool,
}

pub struct RestoredView {
    pub history: usize,
    pub selection: Option<Points>,
}

impl ViewAnchor {
    pub fn capture(terminal: &Terminal, history: usize, selection: Option<Points>) -> Self {
        let mut result = Self {
            anchors: [
                ResizeAnchor::boundary(terminal.view_position(0, 0, history)),
                ResizeAnchor::boundary(None),
                ResizeAnchor::boundary(None),
            ],
            following: history == 0,
            reversed: false,
            selected: selection.is_some(),
        };
        if let Some((a, b)) = selection {
            result.reversed = (a.1, a.0) > (b.1, b.0);
            let (start, end) = if result.reversed { (b, a) } else { (a, b) };
            let position = |point: (usize, usize), end: bool| {
                let mut point = terminal.view_position(point.0, point.1, history)?;
                if terminal
                    .position_cell(point)
                    .is_some_and(|cell| cell.width == 0)
                    && point.column != 0
                {
                    point.column -= 1;
                }
                if end {
                    point.column += terminal
                        .position_cell(point)
                        .map_or(1, |cell| usize::from(cell.width).max(1));
                    point.column = point.column.min(terminal.cols());
                }
                Some(point)
            };
            result.anchors[1].position = position(start, false);
            result.anchors[2].position = position(end, true);
        }
        result
    }

    pub fn anchors_mut(&mut self) -> &mut [ResizeAnchor] {
        &mut self.anchors
    }

    pub fn restore(&self, terminal: &Terminal) -> RestoredView {
        let mut history = if self.following || terminal.modes().alternate_screen {
            0
        } else {
            self.anchors[0]
                .position
                .map_or(terminal.history_len(), |point| {
                    terminal
                        .history_position()
                        .saturating_sub(point.line)
                        .min(terminal.history_len() as u64) as usize
                })
        };
        // Inclusive viewport coordinates cannot represent a selection above
        // row zero. Keep a retained selection visible when narrowing pushes its
        // start into history, instead of silently discarding selected text.
        if self.selected
            && let (Some(start), Some(end)) = (self.anchors[1].position, self.anchors[2].position)
            && start < end
        {
            let needed = terminal
                .history_position()
                .saturating_sub(start.line)
                .min(terminal.history_len() as u64) as usize;
            history = history.max(needed);
        }
        let selection = if self.selected {
            self.selection(terminal, history)
        } else {
            None
        };
        RestoredView { history, selection }
    }

    fn selection(&self, terminal: &Terminal, history: usize) -> Option<Points> {
        let start = self.anchors[1].position?;
        let end = self.anchors[2].position?;
        if end <= start {
            return None;
        }
        let end = if end.column != 0 {
            TextPosition {
                column: end.column - 1,
                ..end
            }
        } else {
            TextPosition {
                line: end.line.checked_sub(1)?,
                column: terminal.cols() - 1,
            }
        };
        if end < start {
            // A newline-only or completely elided padding selection cannot be
            // represented as inclusive visible cells. Never copy other text.
            return None;
        }
        let start = terminal.position_in_view(start, history)?;
        let end = terminal.position_in_view(end, history)?;
        Some(if self.reversed {
            (end, start)
        } else {
            (start, end)
        })
    }
}
