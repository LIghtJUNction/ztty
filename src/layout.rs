//! Platform-independent ownership and geometry for tabs and split panes.
//! A leaf identifies a session; terminal state and PTYs never live in this tree.
use alloc::{boxed::Box, vec, vec::Vec};

pub const MAX_PANES: usize = 64;
pub const MAX_TABS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Pane(u64),
    Split {
        axis: Axis,
        // Parts per thousand. Keeping both children nonempty is a model invariant.
        ratio: u16,
        first: Box<Node>,
        second: Box<Node>,
    },
}

impl Node {
    pub fn panes(&self, out: &mut Vec<u64>) {
        match self {
            Self::Pane(id) => out.push(*id),
            Self::Split { first, second, .. } => {
                first.panes(out);
                second.panes(out);
            }
        }
    }

    fn contains(&self, id: u64) -> bool {
        match self {
            Self::Pane(pane) => *pane == id,
            Self::Split { first, second, .. } => first.contains(id) || second.contains(id),
        }
    }

    fn split(&mut self, target: u64, new: u64, axis: Axis) -> bool {
        match self {
            Self::Pane(id) if *id == target => {
                *self = Self::Split {
                    axis,
                    ratio: 500,
                    first: Box::new(Self::Pane(target)),
                    second: Box::new(Self::Pane(new)),
                };
                true
            }
            Self::Split { first, second, .. } => {
                first.split(target, new, axis) || second.split(target, new, axis)
            }
            _ => false,
        }
    }

    fn remove(self, id: u64) -> Option<Self> {
        match self {
            Self::Pane(pane) => (pane != id).then_some(Self::Pane(pane)),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => match (first.remove(id), second.remove(id)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (first, second) => first.or(second),
            },
        }
    }

    fn swap(&mut self, a: u64, b: u64) {
        match self {
            Self::Pane(id) if *id == a => *id = b,
            Self::Pane(id) if *id == b => *id = a,
            Self::Split { first, second, .. } => {
                first.swap(a, b);
                second.swap(a, b);
            }
            _ => {}
        }
    }

    fn adjust(&mut self, id: u64, delta: i32) -> bool {
        if let Self::Split {
            ratio,
            first,
            second,
            ..
        } = self
        {
            // Resize the nearest split containing the focused leaf.
            if first.adjust(id, delta) || second.adjust(id, delta) {
                return true;
            }
            let change = if first.contains(id) {
                delta
            } else if second.contains(id) {
                -delta
            } else {
                return false;
            };
            *ratio = (i32::from(*ratio) + change).clamp(100, 900) as u16;
            return true;
        }
        false
    }

    fn place(&self, rect: Rect, out: &mut Vec<(u64, Rect)>) {
        match self {
            Self::Pane(id) => out.push((*id, rect)),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let extent = match axis {
                    Axis::Horizontal => rect.height,
                    Axis::Vertical => rect.width,
                };
                // A two-pixel gutter separates panes; tiny windows never produce
                // zero/negative X11 dimensions. Their contents may be clipped.
                let gap = if extent >= 4 { 2 } else { 0 };
                let available = (extent - gap).max(2);
                let a = (i64::from(available) * i64::from(*ratio) / 1000) as i32;
                let a = a.clamp(1, available - 1);
                let b = available - a;
                let (left, right) = match axis {
                    Axis::Vertical => (
                        Rect { width: a, ..rect },
                        Rect {
                            x: rect.x + a + gap,
                            width: b,
                            ..rect
                        },
                    ),
                    Axis::Horizontal => (
                        Rect { height: a, ..rect },
                        Rect {
                            y: rect.y + a + gap,
                            height: b,
                            ..rect
                        },
                    ),
                };
                first.place(left, out);
                second.place(right, out);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub tabs: Vec<Node>,
    pub active_tab: usize,
    pub focused: u64,
    pub maximized: bool,
}

impl Layout {
    pub fn new(id: u64) -> Self {
        Self {
            tabs: vec![Node::Pane(id)],
            active_tab: 0,
            focused: id,
            maximized: false,
        }
    }

    pub fn panes(&self) -> Vec<u64> {
        let mut panes = Vec::new();
        for tab in &self.tabs {
            tab.panes(&mut panes);
        }
        panes
    }

    pub fn contains(&self, id: u64) -> bool {
        self.tabs.iter().any(|tab| tab.contains(id))
    }

    pub fn focus(&mut self, id: u64) -> bool {
        let Some(tab) = self.tabs.iter().position(|tab| tab.contains(id)) else {
            return false;
        };
        self.active_tab = tab;
        self.focused = id;
        true
    }

    pub fn add_tab(&mut self, id: u64) -> Result<(), &'static str> {
        if self.contains(id) {
            return Err("session already belongs to this workspace");
        }
        if self.tabs.len() >= MAX_TABS || self.panes().len() >= MAX_PANES {
            return Err("workspace limit reached (32 tabs / 64 panes)");
        }
        self.tabs.push(Node::Pane(id));
        self.active_tab = self.tabs.len() - 1;
        self.focused = id;
        self.maximized = false;
        Ok(())
    }

    pub fn split(&mut self, id: u64, axis: Axis) -> Result<(), &'static str> {
        if self.contains(id) {
            return Err("session already belongs to this workspace");
        }
        if self.panes().len() >= MAX_PANES {
            return Err("workspace pane limit reached (64)");
        }
        if !self
            .tabs
            .get_mut(self.active_tab)
            .is_some_and(|tab| tab.split(self.focused, id, axis))
        {
            return Err("focused pane is no longer available");
        }
        self.focused = id;
        self.maximized = false;
        Ok(())
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let Some(index) = self.tabs.iter().position(|tab| tab.contains(id)) else {
            return false;
        };
        let old = self.tabs.remove(index);
        if let Some(new) = old.remove(id) {
            self.tabs.insert(index, new);
        } else if index < self.active_tab {
            self.active_tab -= 1;
        }
        self.active_tab = self.active_tab.min(self.tabs.len().saturating_sub(1));
        if self.focused == id {
            let mut panes = Vec::new();
            if let Some(tab) = self.tabs.get(self.active_tab) {
                tab.panes(&mut panes);
            }
            self.focused = panes.first().copied().unwrap_or(0);
        }
        self.maximized = false;
        true
    }

    pub fn cycle_tab(&mut self, forward: bool) {
        if self.tabs.is_empty() {
            return;
        }
        let count = self.tabs.len();
        self.active_tab = (self.active_tab + if forward { 1 } else { count - 1 }) % count;
        let mut panes = Vec::new();
        self.tabs[self.active_tab].panes(&mut panes);
        self.focused = panes[0];
        self.maximized = false;
    }

    pub fn cycle_pane(&mut self, forward: bool, swap: bool) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let mut panes = Vec::new();
        tab.panes(&mut panes);
        let count = panes.len();
        let index = panes.iter().position(|id| *id == self.focused).unwrap_or(0);
        let other = panes[(index + if forward { 1 } else { count - 1 }) % count];
        if swap {
            tab.swap(self.focused, other);
        } else {
            self.focused = other;
        }
    }

    pub fn adjust(&mut self, delta: i32) -> bool {
        self.tabs
            .get_mut(self.active_tab)
            .is_some_and(|tab| tab.adjust(self.focused, delta))
    }

    pub fn placements(&self, width: i32, height: i32) -> Vec<(u64, Rect)> {
        let mut out = Vec::new();
        let rect = Rect {
            x: 0,
            y: 0,
            width: width.max(1),
            height: height.max(1),
        };
        if self.maximized && self.contains(self.focused) {
            out.push((self.focused, rect));
        } else if let Some(tab) = self.tabs.get(self.active_tab) {
            tab.place(rect, &mut out);
        }
        out
    }
}
