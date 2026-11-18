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
fn resize_queued_during_commit_waits_for_the_next_outer_turn() {
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
fn minimize_cancels_resize_until_the_window_has_a_client_again() {
    let dispatch = Dispatch::default();
    dispatch.resize_event(false);
    dispatch.resize_event(true);
    assert!(!dispatch.take_resize());
    dispatch.resize_event(false);
    assert!(dispatch.take_resize());
}
