//! Bounded native scene layout and view-local input state.
//! Pixels stay in the terminal's existing graphics store; this module keeps only IDs.
use crate::{json, layout::Rect};
use alloc::{format, vec::Vec};
use zterm::{
    Terminal,
    ui::{Action, Kind, Scene},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub width: i32,
    pub height: i32,
    pub cell_width: i32,
    pub cell_height: i32,
    pub padding: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub surface: u64,
    pub revision: u64,
    pub change: u64,
}

impl Stamp {
    fn current(self, terminal: &Terminal) -> bool {
        terminal.ui_revision() == self.change
            && terminal.ui_scene().is_some_and(|scene| {
                scene.surface == self.surface && scene.revision == self.revision
            })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub index: usize,
    pub rect: Rect,
}

pub struct Layout {
    pub stamp: Stamp,
    pub panel: Rect,
    pub items: Vec<Item>,
    pub geometry: Geometry,
}

fn contains(rect: Rect, x: i32, y: i32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height
}

fn index(scene: &Scene, id: u32) -> Option<usize> {
    scene.nodes.binary_search_by_key(&id, |node| node.id).ok()
}

fn height(scene: &Scene, id: u32, ch: i32, depth: usize) -> i32 {
    if depth > 16 {
        return 0;
    }
    let Some(index) = index(scene, id) else {
        return 0;
    };
    match &scene.nodes[index].kind {
        Kind::Column { children } => children
            .iter()
            .fold(0i32, |sum, id| {
                sum.saturating_add(height(scene, *id, ch, depth + 1))
                    .saturating_add(10)
            })
            .saturating_sub(10)
            .max(0),
        Kind::Row { children } => children
            .iter()
            .map(|id| height(scene, *id, ch, depth + 1))
            .max()
            .unwrap_or(0),
        Kind::Text { text, .. } => (text.split('\n').count().min(32) as i32).max(1) * ch,
        Kind::Progress { .. } => ch + 14,
        Kind::Button { .. } => ch + 16,
        Kind::Image { rows, .. } => i32::from(*rows).saturating_mul(ch),
    }
}

fn place(
    scene: &Scene,
    id: u32,
    rect: Rect,
    geometry: Geometry,
    depth: usize,
    out: &mut Vec<Item>,
) {
    if depth > 16 || rect.width <= 0 || rect.height <= 0 {
        return;
    }
    let Some(index) = index(scene, id) else {
        return;
    };
    match &scene.nodes[index].kind {
        Kind::Column { children } => {
            let mut y = rect.y;
            let bottom = rect.y + rect.height;
            for id in children {
                let h = height(scene, *id, geometry.cell_height, depth + 1).min(bottom - y);
                if h <= 0 {
                    break;
                }
                place(
                    scene,
                    *id,
                    Rect {
                        x: rect.x,
                        y,
                        width: rect.width,
                        height: h,
                    },
                    geometry,
                    depth + 1,
                    out,
                );
                y = y.saturating_add(h).saturating_add(10);
            }
        }
        Kind::Row { children } => {
            if children.is_empty() {
                return;
            }
            let gap = 12;
            let count = children.len() as i32;
            let available = (rect.width - gap * (count - 1)).max(0);
            for (column, id) in children.iter().enumerate() {
                let left = available * column as i32 / count;
                let right = available * (column as i32 + 1) / count;
                place(
                    scene,
                    *id,
                    Rect {
                        x: rect.x + left + column as i32 * gap,
                        y: rect.y,
                        width: right - left,
                        height: rect.height,
                    },
                    geometry,
                    depth + 1,
                    out,
                );
            }
        }
        _ => out.push(Item { index, rect }),
    }
}

impl Layout {
    fn new(scene: &Scene, change: u64, geometry: Geometry) -> Option<Self> {
        let inset = geometry.padding.max(0) + 12;
        let width = (geometry.width - inset * 2).min(760);
        let available = geometry.height - inset * 2;
        let header = geometry.cell_height + 24;
        if width < 96 || available < header + geometry.cell_height + 16 {
            return None;
        }
        let content_height =
            height(scene, scene.root, geometry.cell_height, 0).min(available - header - 16);
        let panel = Rect {
            x: inset,
            y: inset,
            width,
            height: content_height + header + 16,
        };
        let mut items = Vec::with_capacity(scene.nodes.len());
        place(
            scene,
            scene.root,
            Rect {
                x: inset + 16,
                y: inset + header,
                width: width - 32,
                height: content_height,
            },
            geometry,
            0,
            &mut items,
        );
        Some(Self {
            stamp: Stamp {
                surface: scene.surface,
                revision: scene.revision,
                change,
            },
            panel,
            items,
            geometry,
        })
    }

    fn button(&self, terminal: &Terminal, x: i32, y: i32) -> Option<u32> {
        if !self.stamp.current(terminal) {
            return None;
        }
        let scene = terminal.ui_scene()?;
        self.items.iter().find_map(|item| {
            if !contains(item.rect, x, y) || item.rect.height < self.geometry.cell_height + 16 {
                return None;
            }
            let node = scene.nodes.get(item.index)?;
            matches!(node.kind, Kind::Button { enabled: true, .. }).then_some(node.id)
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hit {
    stamp: Stamp,
    node: u32,
}

#[derive(Default)]
pub struct View {
    pub layout: Option<Layout>,
    focused: Option<Hit>,
    pressed: Option<Hit>,
    swallowed: u16,
    interactive: bool,
    displayed: Option<(Stamp, Geometry)>,
}

impl View {
    /// Relayout only for scene/geometry/lifecycle changes, never for cursor blink.
    pub fn refresh(
        &mut self,
        terminal: &Terminal,
        geometry: Geometry,
        visible: bool,
        interactive: bool,
    ) -> bool {
        let stamp = terminal.ui_scene().filter(|_| visible).map(|scene| Stamp {
            surface: scene.surface,
            revision: scene.revision,
            change: terminal.ui_revision(),
        });
        let displayed = stamp.map(|stamp| (stamp, geometry));
        let unchanged = self.displayed == displayed;
        if unchanged && self.interactive == interactive {
            return false;
        }
        self.blur();
        self.interactive = interactive;
        self.displayed = displayed;
        self.layout = terminal
            .ui_scene()
            .filter(|_| visible)
            .and_then(|scene| Layout::new(scene, terminal.ui_revision(), geometry));
        true
    }

    /// Keep release suppression across focus loss, replacement, clear and resize.
    pub fn blur(&mut self) {
        self.focused = None;
        self.pressed = None;
    }

    pub fn invalidate(&mut self) {
        self.blur();
        self.layout = None;
        self.displayed = None;
    }

    /// A different view received the release; the host consumed it globally.
    pub fn cancel_button(&mut self, button: u32) {
        if (1..=11).contains(&button) {
            self.swallowed &= !(1u16 << button);
            if button == 1 {
                self.pressed = None;
            }
        }
    }

    pub fn focused_node(&self) -> Option<u32> {
        self.focused.map(|hit| hit.node)
    }

    pub fn blocks_motion(&self, x: i32, y: i32) -> bool {
        // Core X11 exposes held state only for buttons 1–5. Extended buttons
        // consume press/release without taking ownership of pointer motion.
        self.swallowed & 0x3e != 0
            || self
                .layout
                .as_ref()
                .is_some_and(|layout| contains(layout.panel, x, y))
    }

    pub fn pointer(
        &mut self,
        terminal: &mut Terminal,
        x: i32,
        y: i32,
        button: u32,
        press: bool,
        interactive: bool,
    ) -> (bool, Option<Action>) {
        if !(1..=11).contains(&button) {
            return (false, None);
        }
        let mask = 1u16 << button;
        if !press && self.swallowed & mask != 0 {
            self.swallowed &= !mask;
            let captured = if button == 1 {
                self.pressed.take()
            } else {
                None
            };
            let action = captured.and_then(|hit| {
                if !interactive
                    || !self.interactive
                    || !hit.stamp.current(terminal)
                    || self
                        .layout
                        .as_ref()
                        .and_then(|layout| layout.button(terminal, x, y))
                        != Some(hit.node)
                {
                    return None;
                }
                terminal.ui_action(hit.node, hit.stamp.surface, hit.stamp.revision)
            });
            return (true, action);
        }
        if !press {
            return (false, None);
        }
        let Some(layout) = &self.layout else {
            self.blur();
            return (false, None);
        };
        if !contains(layout.panel, x, y) {
            self.blur();
            return (false, None);
        }
        self.swallowed |= mask;
        if button == 1 {
            let node = (interactive && self.interactive)
                .then(|| layout.button(terminal, x, y))
                .flatten();
            let hit = node.map(|node| Hit {
                stamp: layout.stamp,
                node,
            });
            self.focused = hit;
            self.pressed = hit;
        }
        (true, None)
    }

    /// Once a button has focus, ordinary keys belong to this view until Escape
    /// or an outside click. The host consumes matching key releases as well.
    pub fn key(
        &mut self,
        terminal: &mut Terminal,
        symbol: u64,
        shift: bool,
        repeated: bool,
        interactive: bool,
    ) -> (bool, Option<Action>) {
        let Some(hit) = self.focused else {
            return (false, None);
        };
        if !interactive || !self.interactive || !hit.stamp.current(terminal) {
            self.blur();
            return (true, None);
        }
        match symbol {
            0xff1b => self.blur(),
            0xff09 | 0xfe20 => {
                let Some(layout) = &self.layout else {
                    self.blur();
                    return (true, None);
                };
                let Some(scene) = terminal.ui_scene() else {
                    self.blur();
                    return (true, None);
                };
                let buttons: Vec<u32> = layout
                    .items
                    .iter()
                    .filter_map(|item| {
                        let node = scene.nodes.get(item.index)?;
                        (item.rect.height >= layout.geometry.cell_height + 16
                            && matches!(node.kind, Kind::Button { enabled: true, .. }))
                        .then_some(node.id)
                    })
                    .collect();
                if let Some(position) = buttons.iter().position(|node| *node == hit.node) {
                    let next = (position
                        + if shift || symbol == 0xfe20 {
                            buttons.len() - 1
                        } else {
                            1
                        })
                        % buttons.len();
                    self.focused = Some(Hit {
                        node: buttons[next],
                        ..hit
                    });
                }
            }
            0xff0d | 0xff8d | 0x20 if !repeated => {
                return (
                    true,
                    terminal.ui_action(hit.node, hit.stamp.surface, hit.stamp.revision),
                );
            }
            _ => {}
        }
        (true, None)
    }
}

/// Serialized intent only. The receiving application decides what the action means.
pub fn action_bytes(action: Action) -> Vec<u8> {
    let value = json::object([
        ("v", 1u32.into()),
        ("type", "action".into()),
        ("surface", action.surface.into()),
        ("rev", action.revision.into()),
        ("node", action.node.into()),
        ("action", action.action.into()),
        ("event_id", action.event_id.into()),
    ]);
    format!("\x1b_ZUI;1;{}\x1b\\", json::stringify(&value)).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    fn scene(terminal: &mut Terminal, rev: u64) {
        terminal.set_ui_host_enabled(true);
        terminal.feed(format!("\x1b_ZUI;1;{{\"v\":1,\"op\":\"replace\",\"surface\":7,\"rev\":{rev},\"root\":1,\"events\":true,\"nodes\":[{{\"id\":1,\"type\":\"column\",\"children\":[2,3]}},{{\"id\":2,\"type\":\"button\",\"label\":\"Run\",\"action\":\"run\",\"enabled\":true}},{{\"id\":3,\"type\":\"button\",\"label\":\"Next\",\"action\":\"next\",\"enabled\":true}}]}}\x1b\\").as_bytes());
    }
    fn geometry() -> Geometry {
        Geometry {
            width: 800,
            height: 480,
            cell_width: 8,
            cell_height: 18,
            padding: 0,
        }
    }
    fn first(view: &View) -> (i32, i32) {
        let rect = view.layout.as_ref().unwrap().items[0].rect;
        (rect.x + 4, rect.y + 4)
    }

    #[test]
    fn current_click_emits_bound_intent_and_never_runs_a_command() {
        let mut terminal = Terminal::new(100, 24, 0);
        scene(&mut terminal, 1);
        let mut view = View::default();
        assert!(view.refresh(&terminal, geometry(), true, true));
        assert!(!view.refresh(&terminal, geometry(), true, true));
        let (x, y) = first(&view);
        assert!(view.pointer(&mut terminal, x, y, 1, true, true).0);
        let action = view.pointer(&mut terminal, x, y, 1, false, true).1.unwrap();
        assert_eq!((action.surface, action.revision, action.node), (7, 1, 2));
        let bytes = action_bytes(action);
        assert!(bytes.starts_with(b"\x1b_ZUI;1;"));
        assert!(bytes.ends_with(b"\x1b\\"));
        let value = json::parse(&bytes[8..bytes.len() - 2]).unwrap();
        assert_eq!(
            value.get("action").and_then(json::Value::as_str),
            Some("run")
        );
    }

    #[test]
    fn stale_press_is_consumed_after_update_clear_and_resize() {
        for reason in 0..3 {
            let mut terminal = Terminal::new(100, 24, 0);
            scene(&mut terminal, 1);
            let mut view = View::default();
            view.refresh(&terminal, geometry(), true, true);
            let (x, y) = first(&view);
            view.pointer(&mut terminal, x, y, 1, true, true);
            match reason {
                0 => scene(&mut terminal, 2),
                1 => terminal.feed(b"\x1bc"),
                _ => view.invalidate(),
            }
            assert!(view.blocks_motion(x, y));
            let (handled, action) = view.pointer(&mut terminal, x, y, 1, false, true);
            assert!(handled);
            assert!(action.is_none());
        }
    }

    #[test]
    fn observing_eof_and_focus_loss_cannot_activate() {
        let mut terminal = Terminal::new(100, 24, 0);
        scene(&mut terminal, 1);
        let mut view = View::default();
        view.refresh(&terminal, geometry(), true, false);
        let (x, y) = first(&view);
        view.pointer(&mut terminal, x, y, 1, true, false);
        assert!(
            view.pointer(&mut terminal, x, y, 1, false, false)
                .1
                .is_none()
        );
        view.refresh(&terminal, geometry(), true, true);
        view.pointer(&mut terminal, x, y, 1, true, true);
        view.blur();
        assert!(
            view.pointer(&mut terminal, x, y, 1, false, true)
                .1
                .is_none()
        );
        view.pointer(&mut terminal, x, y, 1, true, true);
        assert!(
            view.pointer(&mut terminal, x, y, 1, false, false)
                .1
                .is_none()
        );
    }

    #[test]
    fn focus_consumes_text_navigates_and_escape_returns_control() {
        let mut terminal = Terminal::new(100, 24, 0);
        scene(&mut terminal, 1);
        let mut view = View::default();
        view.refresh(&terminal, geometry(), true, true);
        let (x, y) = first(&view);
        view.pointer(&mut terminal, x, y, 1, true, true);
        view.pointer(&mut terminal, x, y, 1, false, true);
        assert!(view.key(&mut terminal, b'x' as u64, false, false, true).0);
        assert!(view.key(&mut terminal, 0xff09, false, false, true).0);
        let action = view
            .key(&mut terminal, 0xff0d, false, false, true)
            .1
            .unwrap();
        assert_eq!(action.node, 3);
        assert!(
            view.key(&mut terminal, 0xff0d, false, true, true)
                .1
                .is_none()
        );
        assert!(view.key(&mut terminal, 0xff1b, false, false, true).0);
        assert!(!view.key(&mut terminal, b'x' as u64, false, false, true).0);
    }
    #[test]
    fn undersized_view_stays_idle_until_geometry_changes() {
        let mut terminal = Terminal::new(10, 2, 0);
        scene(&mut terminal, 1);
        let mut view = View::default();
        let tiny = Geometry {
            width: 80,
            height: 36,
            ..geometry()
        };
        assert!(view.refresh(&terminal, tiny, true, true));
        assert!(view.layout.is_none());
        assert!(!view.refresh(&terminal, tiny, true, true));
        assert!(!view.pointer(&mut terminal, 20, 20, 1, true, true).0);
        assert!(view.refresh(&terminal, geometry(), true, true));
        assert!(view.layout.is_some());
    }
    #[test]
    fn extended_button_release_is_consumed_without_latching_motion() {
        let mut terminal = Terminal::new(100, 24, 0);
        scene(&mut terminal, 1);
        let mut view = View::default();
        view.refresh(&terminal, geometry(), true, true);
        let (x, y) = first(&view);
        assert!(view.pointer(&mut terminal, x, y, 8, true, true).0);
        view.invalidate();
        assert!(!view.blocks_motion(790, 470));
        let (handled, action) = view.pointer(&mut terminal, 790, 470, 8, false, false);
        assert!(handled);
        assert!(action.is_none());
        assert!(!view.pointer(&mut terminal, 790, 470, 8, false, false).0);
    }
}
