#[path = "../src/repaint.rs"]
mod repaint;

use repaint::{Plan, Repaint};
use zterm::Terminal;
use ztty::text_blink::TextBlink;

#[test]
fn ongoing_output_keeps_the_original_deadline() {
    let mut repaint = Repaint::default();
    repaint.painted(100);
    for now in 101..108 {
        repaint.output_received();
        assert_eq!(repaint.plan(now, true, false, false), Plan::WaitUntil(108));
    }
    assert_eq!(repaint.plan(108, true, false, false), Plan::Render);
}

#[test]
fn quiet_final_damage_has_a_deadline_but_idle_has_none() {
    let mut repaint = Repaint::default();
    repaint.painted(100);
    assert_eq!(repaint.plan(101, true, false, false), Plan::WaitUntil(108));
    assert_eq!(repaint.plan(108, true, false, false), Plan::Render);
    repaint.painted(108);
    for now in 108..1000 {
        assert_eq!(repaint.plan(now, false, false, false), Plan::Idle);
    }
}

#[test]
fn force_first_frame_and_expose_bypass_the_budget() {
    let mut repaint = Repaint::default();
    assert_eq!(repaint.plan(0, true, true, false), Plan::Render);
    repaint.painted(100);
    assert_eq!(repaint.plan(101, true, true, false), Plan::Render);
}

#[test]
fn old_screen_repaint_does_not_consume_pending_input() {
    let mut repaint = Repaint::default();
    repaint.painted(100);
    repaint.input_sent();
    assert_eq!(repaint.plan(101, true, true, false), Plan::Render);
    repaint.painted(101);
    assert_eq!(repaint.plan(102, true, false, false), Plan::WaitUntil(109));
    repaint.output_received();
    assert_eq!(repaint.plan(102, true, false, false), Plan::Render);
    repaint.painted(102);
    assert_eq!(repaint.plan(103, true, false, false), Plan::WaitUntil(110));
}

#[test]
fn synchronized_frames_do_not_return_expired_repaint_deadlines() {
    let mut repaint = Repaint::default();
    repaint.painted(100);
    repaint.input_sent();
    repaint.output_received();
    for now in [101, 108, 250] {
        assert_eq!(repaint.plan(now, true, true, true), Plan::Idle);
    }
    // A release or timeout is urgent even if it lands inside the ordinary budget.
    assert_eq!(repaint.plan(101, true, true, false), Plan::Render);
}

#[test]
fn canceling_input_and_protocol_output_cannot_create_interaction_priority() {
    let mut repaint = Repaint::default();
    repaint.painted(100);
    repaint.input_sent();
    repaint.output_received();
    repaint.cancel_input();
    repaint.output_received();
    assert_eq!(repaint.plan(101, true, false, false), Plan::WaitUntil(108));
    repaint.input_sent();
    repaint.cancel_input();
    repaint.output_received();
    assert_eq!(repaint.plan(102, true, false, false), Plan::WaitUntil(108));
}

#[test]
fn batched_final_output_starts_and_stops_blink_without_another_pty_wakeup() {
    let mut terminal = Terminal::new(10, 2, 16);
    let mut blink = TextBlink::default();
    let mut repaint = Repaint::default();
    blink.update(&terminal, 0, true, true, false, 100);
    terminal.clear_dirty();
    repaint.painted(100);

    terminal.feed(b"\x1b[5mX");
    for now in 101..108 {
        // No cell refresh until the scheduled frame, and no damage is lost.
        let tick = blink.tick(true, false, now);
        assert_eq!(tick.deadline, None);
        assert!(!tick.changed);
        assert!(terminal.dirty());
        assert_eq!(
            repaint.plan(now, terminal.dirty(), false, false),
            Plan::WaitUntil(108)
        );
    }
    assert_eq!(
        repaint.plan(108, terminal.dirty(), false, false),
        Plan::Render
    );
    let first = blink.update(&terminal, 0, false, true, false, 108);
    assert_eq!(first.visible, [true; 2]);
    assert_eq!(first.deadline, Some(608));
    terminal.clear_dirty();
    repaint.painted(108);

    let hidden = blink.tick(true, false, 608);
    assert_eq!(hidden.visible, [false, true]);
    assert!(hidden.changed);
    assert_eq!(
        repaint.plan(608, hidden.changed, hidden.changed, false),
        Plan::Render
    );
    blink.update(&terminal, 0, true, true, false, 608);
    terminal.clear_dirty();
    repaint.painted(608);

    terminal.feed(b"\x1b[0m\x1b[2Jplain");
    assert!(!blink.tick(true, false, 609).changed);
    assert_eq!(
        repaint.plan(609, terminal.dirty(), false, false),
        Plan::WaitUntil(616)
    );
    let erased = blink.update(&terminal, 0, false, true, false, 616);
    assert_eq!(erased.visible, [true; 2]);
    assert!(erased.changed);
    assert_eq!(erased.deadline, None);
    terminal.clear_dirty();
    repaint.painted(616);
    let idle = blink.tick(true, false, 10_000);
    assert!(!idle.changed);
    assert_eq!(idle.deadline, None);
    assert_eq!(
        repaint.plan(10_000, terminal.dirty(), false, false),
        Plan::Idle
    );
}

#[test]
fn deferred_blink_refresh_preserves_input_priority_and_synchronized_release() {
    let mut terminal = Terminal::new(10, 2, 16);
    let mut blink = TextBlink::default();
    let mut repaint = Repaint::default();
    blink.update(&terminal, 0, true, true, false, 100);
    terminal.clear_dirty();
    repaint.painted(100);

    repaint.input_sent();
    terminal.feed(b"\x1b[6mR");
    repaint.output_received();
    assert!(!blink.tick(true, false, 101).changed);
    assert_eq!(
        repaint.plan(101, terminal.dirty(), false, false),
        Plan::Render
    );
    assert_eq!(
        blink.update(&terminal, 0, false, true, false, 101).deadline,
        Some(351)
    );
    terminal.clear_dirty();
    repaint.painted(101);

    terminal.feed(b"\x1b[?2026h\x1b[2JS");
    let held = blink.tick(true, terminal.modes().synchronized_output, 400);
    assert_eq!(held.visible, [true; 2]);
    assert_eq!(held.deadline, None);
    assert_eq!(
        repaint.plan(
            400,
            terminal.dirty(),
            true,
            terminal.modes().synchronized_output
        ),
        Plan::Idle
    );
    assert!(terminal.dirty());

    terminal.feed(b"\x1b[?2026l");
    assert_eq!(
        repaint.plan(401, terminal.dirty(), terminal.sync_commit_pending(), false),
        Plan::Render
    );
    let committed = blink.update(&terminal, 0, false, true, false, 401);
    assert_eq!(committed.visible, [true; 2]);
    assert_eq!(committed.deadline, Some(651));
    terminal.clear_dirty();
    repaint.painted(401);
    assert_eq!(blink.tick(true, false, 651).visible, [true, false]);
}
