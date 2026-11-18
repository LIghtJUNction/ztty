#[path = "../src/platform/windows_dispatch.rs"]
mod windows_dispatch;

use core::cell::Cell;
use windows_dispatch::Dispatch;

#[test]
fn nested_resize_survives_the_mutable_dispatch_guard() {
    let dispatch = Dispatch::default();
    let accesses = Cell::new(0);
    dispatch.with(|| {
        accesses.set(accesses.get() + 1);
        dispatch.resize_event(false);
        assert!(dispatch.with(|| accesses.set(99)).is_none());
        assert!(dispatch.resize_pending());
    });
    assert_eq!(accesses.get(), 1);
    assert!(dispatch.take_resize());
    assert!(!dispatch.take_resize());
    assert!(dispatch.with(|| accesses.set(2)).is_some());
    assert_eq!(accesses.get(), 2);
}

#[test]
fn rapid_resizes_commit_only_the_latest_client_dimensions() {
    let dispatch = Dispatch::default();
    let client = Cell::new((144, 190));
    let surface = Cell::new(client.get());
    dispatch.with(|| {
        for width in [180, 162, 180] {
            client.set((width, 190));
            dispatch.resize_event(false);
        }
    });
    assert!(dispatch.take_resize());
    surface.set(client.get());
    assert_eq!(surface.get(), (180, 190));
    assert!(!dispatch.resize_pending());
}

#[test]
fn resize_queued_during_commit_survives_the_current_callback() {
    let dispatch = Dispatch::default();
    let commits = Cell::new(0);
    dispatch.resize_event(false);
    let turn = || {
        dispatch.with(|| {
            if dispatch.take_resize() {
                commits.set(commits.get() + 1);
                dispatch.resize_event(false);
            }
        });
    };
    turn();
    assert_eq!(commits.get(), 1);
    assert!(dispatch.resize_pending());
    turn();
    assert_eq!(commits.get(), 2);
    assert!(dispatch.resize_pending());
}

#[test]
fn nested_paints_preserve_one_obligation_after_default_validation() {
    let dispatch = Dispatch::default();
    let paints = Cell::new(0);
    let update_region = Cell::new(false);
    dispatch.with(|| {
        for _ in 0..3 {
            update_region.set(true);
            if dispatch.with(|| paints.set(paints.get() + 1)).is_none() {
                dispatch.defer_paint();
                // DefWindowProc validates this request without drawing App.
                update_region.set(false);
            }
        }
        assert_eq!(paints.get(), 0);
        assert!(dispatch.paint_pending());
    });
    assert!(!update_region.get());
    assert_eq!(dispatch.deferred_paints(), 3);
    assert!(dispatch.take_paint());
    assert!(!dispatch.take_paint());
    update_region.set(true);
    assert!(dispatch.with(|| paints.set(paints.get() + 1)).is_some());
    assert_eq!(paints.get(), 1);
}

#[test]
fn paint_queued_during_revalidation_survives_the_current_attempt() {
    let dispatch = Dispatch::default();
    dispatch.defer_paint();
    assert!(dispatch.take_paint());
    // InvalidateRect/UpdateWindow can synchronously enter the window procedure.
    dispatch.with(|| {
        assert!(dispatch.with(|| panic!("overlapping App borrow")).is_none());
        dispatch.defer_paint();
    });
    assert!(dispatch.paint_pending());
    assert!(dispatch.take_paint());
    assert!(!dispatch.paint_pending());
    assert_eq!(dispatch.deferred_paints(), 2);
}

#[test]
fn failed_invalidation_requeues_without_counting_another_nested_paint() {
    let dispatch = Dispatch::default();
    dispatch.defer_paint();
    assert!(dispatch.take_paint());
    dispatch.retry_paint();
    assert!(dispatch.paint_pending());
    assert_eq!(dispatch.deferred_paints(), 1);
    assert!(dispatch.take_paint());
    assert!(!dispatch.take_paint());
}

#[test]
fn committing_resize_before_paint_does_not_consume_another_paint_obligation() {
    let dispatch = Dispatch::default();
    let client = Cell::new(180);
    let surface = Cell::new(144);
    dispatch.resize_event(false);
    dispatch.defer_paint();
    dispatch.with(|| {
        if dispatch.take_resize() {
            surface.set(client.get());
            // A newer size arriving during commit remains independent.
            client.set(198);
            dispatch.resize_event(false);
        }
        assert_eq!(surface.get(), 180);
        assert!(dispatch.paint_pending());
    });
    assert!(dispatch.take_resize());
    surface.set(client.get());
    assert_eq!(surface.get(), 198);
    assert!(dispatch.take_paint());
    dispatch.defer_paint();
    dispatch.resize_event(true);
    assert!(!dispatch.resize_pending());
    assert!(dispatch.paint_pending());
}

#[test]
fn minimize_cancels_resize_until_the_window_has_a_client_again() {
    let dispatch = Dispatch::default();
    dispatch.resize_event(false);
    dispatch.resize_event(true);
    assert!(!dispatch.take_resize());
    dispatch.resize_event(false);
    assert!(dispatch.take_resize());
}
