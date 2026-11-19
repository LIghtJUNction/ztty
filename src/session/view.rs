//! View state, selection, search and complete-frame painting.
use super::{ClipboardMenu, Palette, PaletteItem, PaletteTarget, Search, Ui};
use crate::{
    Appearance, actions,
    display::Window,
    os, paste, queue_input,
    repaint::{Plan, Repaint},
    search,
    session_core::SessionCore,
    x11::{self, Selection},
};
use alloc::{format, string::String, vec::Vec};
use zterm::Terminal;

impl ClipboardMenu {
    pub(super) fn row(&self, x: i32, y: i32, height: i32) -> Option<usize> {
        let bounds = self.bounds;
        if x < bounds.x
            || x >= bounds.x + bounds.width
            || y < bounds.y + 1
            || y >= bounds.y + bounds.height - 1
        {
            return None;
        }
        let row = ((y - bounds.y - 1) / (height + 8).max(1)) as usize;
        (row < 2).then_some(row)
    }

    pub(super) fn action(&self) -> Option<PaletteTarget> {
        self.enabled[self.selected].then_some(PaletteTarget::Action(if self.selected == 0 {
            "copy"
        } else {
            "paste"
        }))
    }
}

impl Palette {
    pub(super) fn matching(&self) -> impl Iterator<Item = &PaletteItem> {
        self.items.iter().filter(|item| {
            let action = match item.target {
                PaletteTarget::Action(name) => name,
                PaletteTarget::Session { .. }
                | PaletteTarget::Plugin(_)
                | PaletteTarget::CancelPlugin => "",
            };
            actions::matches(&self.query, &[&item.label, &item.detail, action])
        })
    }
}

impl Ui {
    pub(super) fn new(window: Window, attachment: u64) -> Self {
        Self {
            window,
            workspace: None,
            visible: true,
            read_only: false,
            owns_child: false,
            search: None,
            palette: None,
            clipboard_menu: None,
            osc_clipboard: None,
            pending_action: None,
            plugin_running: false,
            message: None,
            command: None,
            attachment,
            selection: None,
            selection_unit: ztty::selection::Unit::Cell,
            selection_anchor: ((0, 0), (0, 0)),
            clicks: None,
            history: 0,
            new_output: false,
            scroll_prompt: None,
            native_ui: ztty::native_ui::View::default(),
            force: true,
            repaint: Repaint::default(),
            open: true,
            title: String::new(),
            held: [0; 256],
            intercepted: [false; 256],
            local_buttons: [false; 12],
            swallowed_buttons: [false; 12],
            reported_buttons: 0,
            sync_since: None,
            blink_deadline: None,
            cursor_visible: true,
            last_cursor: None,
            reset_blink: false,
            text_blink: ztty::text_blink::TextBlink::default(),
        }
    }
    pub(super) fn error(&mut self, message: String) {
        self.message = Some((
            message
                .chars()
                .filter(|c| !c.is_control())
                .take(512)
                .collect(),
            os::milliseconds() + 5000,
        ));
        self.force = true;
    }
    pub(super) fn notice(&mut self, message: &str) {
        self.message = Some((message.into(), os::milliseconds() + 1800));
        self.force = true;
    }
    pub(super) fn paste_result(
        &mut self,
        data: Option<Vec<u8>>,
        core: &mut SessionCore,
        allow_input: bool,
    ) {
        let error = self.window.take_paste_error();
        if self.osc_clipboard.is_some() {
            self.finish_osc_clipboard(data, error.is_some(), core, allow_input);
            return;
        }
        if let Some(error) = error {
            self.error(error.into());
            return;
        }
        if !allow_input {
            return;
        }
        if let Some(data) = data {
            match paste(data, &core.terminal, &mut core.input) {
                Ok(0) => self.notice("Clipboard is empty"),
                Ok(_) => {
                    self.scroll_bottom();
                    self.clicks = None;
                    self.repaint.input_sent();
                    self.notice("Pasted");
                }
                Err(error) => self.error(error.into()),
            }
        }
    }
    pub(super) fn reset_input(&mut self) {
        if self.osc_clipboard.take().is_some() {
            self.window.cancel_paste();
        }
        self.native_ui.blur();
        self.held.fill(0);
        self.intercepted.fill(false);
        self.local_buttons.fill(false);
        self.swallowed_buttons.fill(false);
        self.reported_buttons = 0;
        self.clicks = None;
        if self.clipboard_menu.take().is_some() {
            self.force = true;
        }
        if let Some(selection) = &mut self.selection {
            selection.dragging = false;
        }
    }
    pub(super) fn cancel_input(&mut self) {
        self.repaint.cancel_input();
        self.window.cancel_paste();
        self.reset_input();
    }

    pub(super) fn capture_view_anchor(&self, terminal: &Terminal) -> ztty::selection::ViewAnchor {
        ztty::selection::ViewAnchor::capture(
            terminal,
            self.history,
            self.selection
                .as_ref()
                .map(|selection| (selection.start, selection.end)),
        )
    }

    pub(super) fn restore_view_anchor(
        &mut self,
        terminal: &Terminal,
        anchor: ztty::selection::ViewAnchor,
    ) {
        let restored = anchor.restore(terminal);
        self.set_history(restored.history);
        self.selection = restored.selection.map(|(start, end)| x11::Selection {
            start,
            end,
            dragging: false,
        });
        self.selection_anchor = restored.selection.unwrap_or(((0, 0), (0, 0)));
        self.selection_unit = ztty::selection::Unit::Cell;
        self.clicks = None;
    }

    pub(super) fn set_history(&mut self, history: usize) {
        self.history = history;
        if history == 0 {
            self.new_output = false;
            self.scroll_prompt = None;
        }
    }

    pub(super) fn scroll_bottom(&mut self) {
        self.set_history(0);
        self.selection = None;
        self.force = true;
    }

    pub(super) fn extend_selection(&mut self, terminal: &Terminal, point: (usize, usize)) {
        let (start, end) =
            ztty::selection::unit(terminal, point, self.history, self.selection_unit);
        if let Some(selection) = &mut self.selection {
            let (anchor_start, anchor_end) = self.selection_anchor;
            if (start.1, start.0) < (anchor_start.1, anchor_start.0) {
                selection.start = anchor_end;
                selection.end = start;
            } else {
                selection.start = anchor_start;
                selection.end = end;
            }
            self.force = true;
        }
    }

    // A temporary menu owns its mouse gestures, including the release after
    // an item closes it. It never turns an outside click into application input.
    pub(super) fn clipboard_menu_pointer(&mut self, event: &x11::Event) -> bool {
        let Some(menu) = &mut self.clipboard_menu else {
            return false;
        };
        match unsafe { event.kind } {
            x11::MOTION_NOTIFY => {
                let e = unsafe { event.motion };
                if let Some(row) = menu.row(e.x, e.y, self.window.ch)
                    && menu.enabled[row]
                    && menu.selected != row
                {
                    menu.selected = row;
                    self.force = true;
                }
                true
            }
            x11::BUTTON_PRESS => {
                let e = unsafe { event.button };
                if !(1..=11).contains(&e.button) {
                    return true;
                }
                self.swallowed_buttons[e.button as usize] = true;
                self.local_buttons[e.button as usize] = false;
                self.reported_buttons &= !(1u16 << e.button);
                if e.button == 1
                    && let Some(row) = menu.row(e.x, e.y, self.window.ch)
                {
                    menu.selected = row;
                    self.pending_action = menu.action();
                }
                self.clipboard_menu = None;
                self.force = true;
                true
            }
            _ => false,
        }
    }

    // A drawn view control takes precedence over application mouse reports and
    // custom mouse bindings. Keep ownership of its release after it disappears.
    pub(super) fn scroll_prompt_event(&mut self, event: &x11::Event) -> bool {
        match unsafe { event.kind } {
            x11::BUTTON_PRESS | x11::BUTTON_RELEASE => {
                let e = unsafe { event.button };
                if !(1..=11).contains(&e.button) {
                    return false;
                }
                let button = e.button as usize;
                if e.kind == x11::BUTTON_RELEASE && self.swallowed_buttons[button] {
                    self.swallowed_buttons[button] = false;
                    return true;
                }
                if e.kind != x11::BUTTON_PRESS
                    || (4..=7).contains(&e.button)
                    || self.history == 0
                    || !ztty::scroll_prompt::contains(self.scroll_prompt, e.x, e.y)
                {
                    return false;
                }
                self.swallowed_buttons[button] = true;
                self.local_buttons[button] = false;
                self.reported_buttons &= !(1u16 << button);
                if button == 1 {
                    self.scroll_bottom();
                }
                true
            }
            x11::MOTION_NOTIFY => {
                let e = unsafe { event.motion };
                self.swallowed_buttons.iter().any(|held| *held)
                    || self.history != 0
                        && ztty::scroll_prompt::contains(self.scroll_prompt, e.x, e.y)
            }
            _ => false,
        }
    }
    pub(super) fn find(&mut self, terminal: &Terminal, forward: bool, restart: bool) {
        let Some(search) = &mut self.search else {
            return;
        };
        let result = search::find(
            terminal,
            &search.query,
            search.case,
            if restart { None } else { search.hit },
            forward,
        );
        search.hit = result.hit;
        search.count = result.count;
        search.ordinal = result.ordinal;
        if let Some(hit) = result.hit {
            let (history, row) = search::viewport(terminal, hit);
            self.set_history(history);
            self.selection = Some(Selection {
                start: (hit.start, row),
                end: (hit.end, row + hit.end_row - hit.row),
                dragging: false,
            });
        } else {
            self.selection = None;
        }
        self.force = true;
    }

    pub(super) fn start_search(&mut self, terminal: &Terminal) {
        self.clipboard_menu = None;
        let query = self
            .selection
            .map(|selection| selection.text(terminal, self.history))
            .filter(|text| text.len() <= 1024 && !text.contains('\n'))
            .unwrap_or_default();
        self.search = Some(Search {
            query,
            case: search::Case::Sensitive,
            hit: None,
            count: 0,
            ordinal: 0,
        });
        self.find(terminal, false, true);
    }
    pub(super) fn native_action(
        &mut self,
        core: &mut SessionCore,
        action: Option<zterm::ui::Action>,
    ) {
        if let Some(action) = action {
            let bytes = ztty::native_ui::action_bytes(action);
            self.repaint.input_sent();
            queue_input(&mut core.input, &bytes);
        }
    }

    pub(super) fn native_pointer(
        &mut self,
        event: &x11::Event,
        core: &mut SessionCore,
        allow_input: bool,
    ) -> bool {
        let geometry_current = self
            .native_ui
            .layout
            .as_ref()
            .is_none_or(|layout| layout.geometry == self.window.native_ui_geometry());
        if !geometry_current {
            self.native_ui.blur();
            self.force = true;
        }
        let interactive = geometry_current
            && allow_input
            && self.visible
            && self.open
            && !core.eof()
            && self.history == 0
            && self.search.is_none()
            && self.palette.is_none()
            && core.terminal.ui_scene().is_some_and(|scene| scene.events);
        match unsafe { event.kind } {
            x11::BUTTON_PRESS | x11::BUTTON_RELEASE => {
                let e = unsafe { event.button };
                let focused = self.native_ui.focused_node();
                let (handled, action) = self.native_ui.pointer(
                    &mut core.terminal,
                    e.x,
                    e.y,
                    e.button,
                    e.kind == x11::BUTTON_PRESS,
                    interactive,
                );
                if handled || focused != self.native_ui.focused_node() {
                    self.force = true;
                }
                self.native_action(core, action);
                handled
            }
            x11::MOTION_NOTIFY => {
                let e = unsafe { event.motion };
                self.native_ui.blocks_motion(e.x, e.y)
            }
            _ => false,
        }
    }
    pub(super) fn paint(
        &mut self,
        terminal: &mut Terminal,
        appearance: &Appearance,
        observing: bool,
        alive: bool,
    ) -> Option<u64> {
        if let Some(message) = self.window.take_renderer_notice() {
            self.error(message);
        }
        if self
            .message
            .as_ref()
            .is_some_and(|(_, until)| *until <= os::milliseconds())
        {
            self.message = None;
            self.force = true;
        }
        if self.history == 0 {
            self.new_output = false;
            self.scroll_prompt = None;
        }
        let now = os::milliseconds();
        let animation_deadline = terminal.advance_graphics(now, self.history, alive);
        if terminal.modes().synchronized_output {
            let start = self.sync_since.get_or_insert(now);
            if now.saturating_sub(*start) >= 150 {
                terminal.force_end_sync();
                self.sync_since = None;
            }
        } else {
            self.sync_since = None;
        }
        let title = if observing {
            format!(
                "{} · observe",
                if terminal.title().is_empty() {
                    "ztty"
                } else {
                    terminal.title()
                }
            )
        } else {
            terminal.title().into()
        };
        if title != self.title {
            self.title = title;
            self.window.title(&self.title);
        }
        let modes = terminal.modes();
        let cursor = (self.history == 0 && modes.cursor_visible).then(|| {
            (
                terminal.cursor(),
                modes.cursor_shape.unwrap_or(appearance.cursor_shape),
            )
        });
        let blinking = self.window.focused
            && cursor.is_some()
            && modes.cursor_blink.unwrap_or(appearance.cursor_blink);
        let synchronized = modes.synchronized_output;
        if !blinking || synchronized {
            self.blink_deadline = None;
            self.cursor_visible = true;
        } else if self.blink_deadline.is_none() || self.reset_blink || cursor != self.last_cursor {
            self.blink_deadline = Some(now + appearance.cursor_blink_interval);
            self.cursor_visible = true;
        } else if self.blink_deadline.is_some_and(|deadline| deadline <= now) {
            self.blink_deadline = Some(now + appearance.cursor_blink_interval);
            self.cursor_visible = !self.cursor_visible;
        }
        self.last_cursor = cursor;
        self.reset_blink = false;
        let cursor = cursor.filter(|_| self.cursor_visible);
        let text_blink = self.text_blink.update(
            terminal,
            self.history,
            self.force,
            self.window.focused,
            synchronized,
            now,
        );
        self.force |= text_blink.changed;
        self.window.set_text_blink_visible(text_blink.visible);
        let native_interactive =
            alive && !observing && terminal.ui_scene().is_some_and(|scene| scene.events);
        if self.native_ui.refresh(
            terminal,
            self.window.native_ui_geometry(),
            self.history == 0 && self.search.is_none() && self.palette.is_none(),
            native_interactive,
        ) {
            self.force = true;
        }
        let plan = self.repaint.plan(
            now,
            self.force
                || terminal.dirty()
                || terminal.sync_commit_pending()
                || self.window.cursor_changed(cursor)
                || self.window.graphics_changed(terminal),
            self.force || terminal.sync_commit_pending(),
            synchronized,
        );
        let repaint_deadline = match plan {
            Plan::WaitUntil(deadline) => Some(deadline),
            Plan::Idle | Plan::Render => None,
        };
        if plan == Plan::Render {
            if let Some(search) = &self.search
                && (self.force || terminal.dirty())
            {
                self.window.search_matches(
                    terminal.cols(),
                    search::highlights(terminal, &search.query, search.case, self.history),
                );
            }
            self.window
                .render(terminal, self.history, self.selection, self.force, cursor);
            self.window
                .native_ui(terminal, &self.native_ui, native_interactive);
            let mut overlay_rows = 0;
            if let Some(search) = &self.search {
                self.window.overlay(&format!(
                    "Find: {}   {}/{}   {} · Alt+C   Enter / Shift+Enter · Esc",
                    search.query,
                    search.ordinal,
                    search.count,
                    match search.case {
                        search::Case::Sensitive => "Case",
                        search::Case::Insensitive => "Ignore case",
                    }
                ));
                overlay_rows = 1;
            }
            if let Some(palette) = &self.palette {
                let mut lines = alloc::vec![format!(
                    "{}: {}",
                    if palette.sessions { "Sessions" } else { "Run" },
                    palette.query
                )];
                let start = palette.selected.saturating_sub(2);
                for (index, item) in palette.matching().enumerate().skip(start).take(5) {
                    lines.push(format!(
                        "{} {}{}{}",
                        if index == palette.selected { ">" } else { " " },
                        item.label,
                        if !palette.sessions && !item.detail.is_empty() {
                            "    "
                        } else {
                            ""
                        },
                        if palette.sessions { "" } else { &item.detail }
                    ));
                }
                if lines.len() == 1 {
                    lines.push(
                        if palette.sessions {
                            "  No matching session"
                        } else {
                            "  No matching action"
                        }
                        .into(),
                    );
                }
                let selected = palette.matching().nth(palette.selected);
                if palette.sessions
                    && let Some(item) = selected
                {
                    lines.push(String::new());
                    lines.push(item.detail.clone());
                }
                let verb = match selected.map(|item| item.target) {
                    Some(PaletteTarget::Session {
                        attach: true,
                        observe: true,
                        ..
                    }) => "attach read-only",
                    Some(PaletteTarget::Session { attach: true, .. }) => "attach",
                    Some(PaletteTarget::Session { .. }) => "focus",
                    Some(PaletteTarget::Plugin(_)) => "transform and copy",
                    _ => "run",
                };
                lines.push(format!("↑/↓ select · Enter {verb} · Esc close"));
                self.window.overlay_lines(&lines);
                overlay_rows = lines.len();
            }
            if let Some((message, _)) = &self.message {
                self.window.overlay(&format!("ztty: {message}"));
                overlay_rows = overlay_rows.max(1);
            }
            self.scroll_prompt = if self.history != 0 {
                self.window.scroll_prompt(self.new_output, overlay_rows)
            } else {
                None
            };
            if let Some(menu) = &self.clipboard_menu {
                let copy = actions::shortcut(&appearance.bindings, "copy").unwrap_or_default();
                let paste = actions::shortcut(&appearance.bindings, "paste").unwrap_or_default();
                self.window.clipboard_menu(
                    menu.bounds,
                    menu.selected,
                    menu.enabled,
                    [&copy, &paste],
                );
            }
            self.window.finish_frame();
            self.force = self.window.needs_repaint();
            self.repaint.painted(os::milliseconds());
        }

        // A synchronized frame only uses its existing 150 ms safety deadline;
        // returning an expired ordinary deadline here would busy-poll.
        let deadline = [
            (self.window.needs_repaint() && !synchronized).then_some(now),
            self.sync_since.map(|s| s + 150),
            self.blink_deadline,
            animation_deadline,
            text_blink.deadline,
            repaint_deadline,
        ]
        .into_iter()
        .flatten()
        .min();
        self.message
            .as_ref()
            .map(|(_, until)| deadline.map_or(*until, |d| d.min(*until)))
            .or(deadline)
    }
}
