//! Bounded batching for pending terminal damage, with no idle timer.

// A scheduling budget after a completed paint, not a display refresh rate or
// an input-to-display latency guarantee. The OS and renderer may take longer.
const OUTPUT_REPAINT_BUDGET_MS: u64 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Plan {
    Idle,
    Render,
    WaitUntil(u64),
}

#[derive(Default)]
pub(crate) struct Repaint {
    last_paint: u64,
    input_pending: bool,
    input_ready: bool,
}

impl Repaint {
    pub(crate) fn input_sent(&mut self) {
        self.input_pending = true;
    }

    pub(crate) fn output_received(&mut self) {
        self.input_ready |= self.input_pending;
        self.input_pending = false;
    }

    pub(crate) fn cancel_input(&mut self) {
        self.input_pending = false;
        self.input_ready = false;
    }

    pub(crate) fn plan(&self, now: u64, damage: bool, urgent: bool, synchronized: bool) -> Plan {
        if !damage || synchronized {
            return Plan::Idle;
        }
        let deadline = self.last_paint.saturating_add(OUTPUT_REPAINT_BUDGET_MS);
        if urgent || self.input_ready || now >= deadline {
            Plan::Render
        } else {
            Plan::WaitUntil(deadline)
        }
    }

    pub(crate) fn painted(&mut self, now: u64) {
        self.last_paint = now;
        // A force/expose repaint before output arrives must not consume the
        // request to prioritize the next actual response to local or MCP input.
        self.input_ready = false;
    }
}
