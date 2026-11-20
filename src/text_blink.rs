//! View-local SGR 5/6 timing. Only visible blinking text creates a deadline.
use alloc::vec::Vec;
use zterm::{BLINK, HIDDEN, RAPID_BLINK, STRIKE, Terminal, UNDERLINE};

const FLAGS: u16 = BLINK | RAPID_BLINK;
const INTERVALS: [u64; 2] = [500, 250];

pub struct TextBlink {
    row_flags: Vec<u16>,
    active: u16,
    columns: usize,
    offset: usize,
    alternate: bool,
    visible: [bool; 2],
    next: [Option<u64>; 2],
}

impl Default for TextBlink {
    fn default() -> Self {
        Self {
            row_flags: Vec::new(),
            active: 0,
            columns: 0,
            offset: 0,
            alternate: false,
            visible: [true; 2],
            next: [None; 2],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Tick {
    pub visible: [bool; 2],
    pub changed: bool,
    pub deadline: Option<u64>,
}

impl TextBlink {
    /// Refresh only dirty rows unless the view geometry or scroll position
    /// changed. Cursor movement and unrelated broker wakeups do not rescan text.
    pub fn update(
        &mut self,
        terminal: &Terminal,
        offset: usize,
        force: bool,
        focused: bool,
        synchronized: bool,
        now: u64,
    ) -> Tick {
        let all = force
            || self.row_flags.len() != terminal.rows()
            || self.columns != terminal.cols()
            || self.offset != offset
            || self.alternate != terminal.modes().alternate_screen;
        self.row_flags.resize(terminal.rows(), 0);
        self.columns = terminal.cols();
        self.offset = offset;
        self.alternate = terminal.modes().alternate_screen;
        for (row, flags) in self.row_flags.iter_mut().enumerate() {
            if !all && !terminal.row_dirty(row) {
                continue;
            }
            *flags = 0;
            for column in 0..terminal.cols() {
                let cell = terminal.view_cell(column, row, offset);
                let ink = (cell.ch != ' ' && cell.ch != '\0')
                    || cell.combining != ['\0'; 2]
                    || cell.grapheme_id != 0
                    || cell.flags & (UNDERLINE | STRIKE) != 0
                    || cell.link_id != 0;
                if cell.flags & HIDDEN == 0 && cell.width != 0 && ink {
                    *flags |= cell.flags & FLAGS;
                    if *flags == FLAGS {
                        break;
                    }
                }
            }
        }
        self.active = self.row_flags.iter().fold(0, |all, flags| all | flags);
        self.tick(focused, synchronized, now)
    }

    /// Advance the last rendered view's blink clocks without inspecting cells.
    /// A host that batches output calls this on every wakeup, then `update`
    /// immediately before drawing. Pending damage stays owned by the terminal
    /// until that refresh, including the final output before the PTY goes quiet.
    pub fn tick(&mut self, focused: bool, synchronized: bool, now: u64) -> Tick {
        let previous = self.visible;
        for (index, (flag, interval)) in [BLINK, RAPID_BLINK].into_iter().zip(INTERVALS).enumerate()
        {
            if self.active & flag == 0 || !focused || synchronized {
                self.visible[index] = true;
                self.next[index] = None;
                continue;
            }
            match self.next[index] {
                None => {
                    self.visible[index] = true;
                    self.next[index] = Some(now.saturating_add(interval));
                }
                Some(next) if next <= now => {
                    // A suspended event loop may miss several periods. Catch up
                    // arithmetically instead of queuing a burst of old frames.
                    let periods = now.saturating_sub(next) / interval + 1;
                    if periods % 2 == 1 {
                        self.visible[index] = !self.visible[index];
                    }
                    self.next[index] = Some(next.saturating_add(periods.saturating_mul(interval)));
                }
                Some(_) => {}
            }
        }
        Tick {
            visible: self.visible,
            changed: previous != self.visible,
            deadline: self.next.into_iter().flatten().min(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_and_hidden_text_do_not_schedule_polling() {
        let mut terminal = Terminal::new(10, 2, 16);
        let mut blink = TextBlink::default();
        terminal.feed(b"plain\x1b[5;8mhidden");
        let tick = blink.update(&terminal, 0, false, true, false, 100);
        assert_eq!(tick.deadline, None);
        assert!(!tick.changed);
        terminal.clear_dirty();
        assert_eq!(
            blink
                .update(&terminal, 0, false, true, false, 9999)
                .deadline,
            None
        );
    }

    #[test]
    fn slow_and_rapid_text_have_independent_deadlines_and_catch_up() {
        let mut terminal = Terminal::new(10, 2, 16);
        terminal.feed(b"\x1b[5mS\x1b[6mR");
        let mut blink = TextBlink::default();
        assert_eq!(
            blink.update(&terminal, 0, false, true, false, 100).deadline,
            Some(350)
        );
        terminal.clear_dirty();
        let tick = blink.update(&terminal, 0, false, true, false, 350);
        assert_eq!(tick.visible, [true, false]);
        assert!(tick.changed);
        assert_eq!(tick.deadline, Some(600));
        let tick = blink.update(&terminal, 0, false, true, false, 600);
        assert_eq!(tick.visible, [false, true]);
        let tick = blink.update(&terminal, 0, false, true, false, 1850);
        assert_eq!(tick.visible, [false, false]);
        assert_eq!(tick.deadline, Some(2100));
    }

    #[test]
    fn erasing_or_unfocusing_stops_deadlines_and_restores_text() {
        let mut terminal = Terminal::new(10, 2, 16);
        terminal.feed(b"\x1b[5mX");
        let mut blink = TextBlink::default();
        blink.update(&terminal, 0, false, true, false, 0);
        terminal.clear_dirty();
        assert!(!blink.update(&terminal, 0, false, true, false, 500).visible[0]);
        let stopped = blink.update(&terminal, 0, false, false, false, 501);
        assert_eq!(stopped.visible, [true; 2]);
        assert_eq!(stopped.deadline, None);
        assert!(stopped.changed);
        assert_eq!(
            blink.update(&terminal, 0, false, true, false, 600).deadline,
            Some(1100)
        );
        terminal.feed(b"\x1b[2J");
        assert_eq!(
            blink.update(&terminal, 0, false, true, false, 601).deadline,
            None
        );
    }

    #[test]
    fn synchronized_output_and_alternate_views_reset_visibility() {
        let mut terminal = Terminal::new(10, 2, 16);
        terminal.feed(b"\x1b[6mR");
        let mut blink = TextBlink::default();
        blink.update(&terminal, 0, false, true, false, 0);
        terminal.clear_dirty();
        assert_eq!(
            blink.update(&terminal, 0, false, true, false, 250).visible,
            [true, false]
        );
        let sync = blink.update(&terminal, 0, false, true, true, 251);
        assert_eq!(sync.visible, [true; 2]);
        assert_eq!(sync.deadline, None);
        terminal.feed(b"\x1b[?1049h");
        assert_eq!(
            blink.update(&terminal, 0, false, true, false, 252).deadline,
            None
        );
        terminal.feed(b"\x1b[?1049l");
        assert_eq!(
            blink.update(&terminal, 0, false, true, false, 253).deadline,
            Some(503)
        );
    }
}
