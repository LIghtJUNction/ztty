extern crate alloc;
use layout::{Axis, Layout, MAX_PANES, MAX_TABS, Node};
use ztty::layout;

#[test]
fn tabs_hide_other_sessions_without_losing_their_layout() {
    let mut layout = Layout::new(1);
    layout.split(2, Axis::Vertical).unwrap();
    layout.add_tab(3).unwrap();
    assert_eq!(
        layout
            .placements(800, 600)
            .iter()
            .map(|p| p.0)
            .collect::<Vec<_>>(),
        [3]
    );
    layout.cycle_tab(false);
    let positions = layout.placements(800, 600);
    assert_eq!(positions.len(), 2);
    assert_eq!(positions[0].1.width + positions[1].1.width + 2, 800);
    assert_eq!(positions[1].1.x, positions[0].1.width + 2);
    assert_eq!(layout.panes(), [1, 2, 3]);
}

#[test]
fn closing_pane_collapses_only_its_parent_and_preserves_focus() {
    let mut layout = Layout::new(1);
    layout.split(2, Axis::Vertical).unwrap();
    layout.split(3, Axis::Horizontal).unwrap();
    assert!(layout.remove(2));
    assert_eq!(layout.panes(), [1, 3]);
    assert_eq!(layout.focused, 3);
    assert_eq!(layout.placements(800, 600)[1].1.height, 600);
    assert!(layout.remove(3));
    assert_eq!(layout.tabs, [Node::Pane(1)]);
    assert_eq!(layout.focused, 1);
    assert!(layout.remove(1));
    assert!(layout.tabs.is_empty());
    assert!(layout.placements(800, 600).is_empty());
}

#[test]
fn closing_earlier_tab_keeps_current_tab_selected() {
    let mut layout = Layout::new(1);
    layout.add_tab(2).unwrap();
    layout.add_tab(3).unwrap();
    layout.remove(1);
    assert_eq!(layout.active_tab, 1);
    assert_eq!(layout.focused, 3);
    assert_eq!(layout.placements(900, 600)[0].0, 3);
}

#[test]
fn focus_swap_resize_and_maximize_are_reversible() {
    let mut layout = Layout::new(1);
    layout.split(2, Axis::Vertical).unwrap();
    let original = layout.placements(1000, 800);
    assert!(layout.adjust(100));
    assert!(layout.placements(1000, 800)[1].1.width > original[1].1.width);
    assert!(layout.adjust(-100));
    assert_eq!(layout.placements(1000, 800), original);
    layout.cycle_pane(true, true);
    assert_eq!(layout.panes(), [2, 1]);
    assert_eq!(layout.focused, 2);
    layout.cycle_pane(true, false);
    assert_eq!(layout.focused, 1);
    layout.maximized = true;
    assert_eq!(layout.placements(1000, 800).len(), 1);
    assert_eq!(layout.placements(1000, 800)[0].1.width, 1000);
    layout.maximized = false;
    assert_eq!(layout.placements(1000, 800).len(), 2);
}

#[test]
fn limits_and_duplicate_ids_leave_tree_unchanged() {
    let mut layout = Layout::new(1);
    let initial = layout.clone();
    assert!(layout.add_tab(1).is_err());
    assert!(layout.split(1, Axis::Horizontal).is_err());
    assert_eq!(layout, initial);
    for id in 2..=MAX_TABS as u64 {
        layout.add_tab(id).unwrap();
    }
    assert!(layout.add_tab(99).is_err());
    for id in MAX_TABS as u64 + 1..=MAX_PANES as u64 {
        layout.split(id, Axis::Horizontal).unwrap();
    }
    assert!(layout.split(999, Axis::Horizontal).is_err());
    assert_eq!(layout.panes().len(), MAX_PANES);
}

#[test]
fn tiny_window_never_emits_nonpositive_dimensions() {
    let mut layout = Layout::new(1);
    for id in 2..=16 {
        layout
            .split(
                id,
                if id % 2 == 0 {
                    Axis::Horizontal
                } else {
                    Axis::Vertical
                },
            )
            .unwrap();
    }
    for (w, h) in [(0, 0), (1, 1), (2, 3), (17, 21)] {
        for (_, rect) in layout.placements(w, h) {
            assert!(rect.width > 0 && rect.height > 0);
        }
    }
    assert!(layout.focus(1));
    assert!(!layout.focus(999));
}
