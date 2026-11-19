//! Host policy for OSC 52. This is separate from user-triggered paste.
use super::*;

pub(super) struct Pending {
    request: zterm::ClipboardRequest,
    selector: u8,
    window: u64,
    attachment: u64,
    control: u64,
    terminal: u64,
    deadline: u64,
}
impl Pending {
    fn valid(&self, ui: &Ui, core: &SessionCore) -> bool {
        ui.open
            && ui.visible
            && !ui.read_only
            && !core.eof()
            && core.owns("local")
            && self.window == ui.window.id
            && self.attachment == ui.attachment
            && self.control == core.control_epoch()
            && self.terminal == core.terminal.clipboard_epoch()
    }
}
impl Ui {
    pub(super) fn manual_paste(&mut self, primary: bool) {
        self.osc_clipboard = None;
        self.window.paste(primary);
    }
    pub(super) fn finish_osc_clipboard(
        &mut self,
        data: Option<Vec<u8>>,
        failed: bool,
        core: &mut SessionCore,
        allow_input: bool,
    ) {
        if data.is_none() && !failed {
            return;
        }
        let Some(pending) = self.osc_clipboard.take() else {
            return;
        };
        self.window.cancel_paste();
        if allow_input && pending.valid(self, core) && os::milliseconds() < pending.deadline {
            let reply = pending.request.reply(
                pending.selector,
                if failed { None } else { data.as_deref() },
            );
            let _ = core.input.push(&reply);
        }
    }
    pub(super) fn clipboard_tick(
        &mut self,
        core: &mut SessionCore,
        allow_input: bool,
    ) -> Option<u64> {
        let now = os::milliseconds();
        let allowed = allow_input
            && self.open
            && self.visible
            && !self.read_only
            && self.window.id != 0
            && !core.eof()
            && core.owns("local");
        if self
            .osc_clipboard
            .as_ref()
            .is_some_and(|p| !allowed || !p.valid(self, core) || now >= p.deadline)
        {
            let pending = self.osc_clipboard.take().unwrap();
            self.window.cancel_paste();
            if allowed && pending.valid(self, core) {
                let _ = core
                    .input
                    .push(&pending.request.reply(pending.selector, None));
            }
        }
        if !allowed {
            core.deny_clipboard_requests();
            return None;
        }
        if let Some(pending) = &self.osc_clipboard {
            return Some(pending.deadline);
        }
        while let Some(mut request) = core.terminal.next_clipboard_request() {
            if let Some(text) = request.text.take() {
                let clipboard = request.selectors().contains(&b'c');
                let primary = request.selectors().iter().any(|s| matches!(s, b'p' | b's'));
                if clipboard {
                    self.window.copy(text.clone(), false);
                }
                if primary {
                    self.window.copy(text, true);
                }
                continue;
            }
            // A user-triggered INCR paste keeps its requestor and byte stream.
            // Answer a concurrent program query empty instead of waiting on an
            // unresponsive selection owner without an OSC deadline.
            if self.window.paste_pending() {
                let _ = core
                    .input
                    .push(&request.reply(request.selectors()[0], None));
                continue;
            }
            let selection = request.selectors().iter().copied().find_map(|s| {
                let primary = match s {
                    b'c' => false,
                    b'p' | b's' => true,
                    _ => return None,
                };
                self.window
                    .clipboard_available(primary)
                    .then_some((s, primary))
            });
            if let Some((selector, primary)) = selection
                && self.window.read_osc_clipboard(primary)
            {
                let deadline = now.saturating_add(2000);
                self.osc_clipboard = Some(Pending {
                    request,
                    selector,
                    window: self.window.id,
                    attachment: self.attachment,
                    control: core.control_epoch(),
                    terminal: core.terminal.clipboard_epoch(),
                    deadline,
                });
                return Some(deadline);
            }
            let _ = core
                .input
                .push(&request.reply(request.selectors()[0], None));
        }
        None
    }
}
