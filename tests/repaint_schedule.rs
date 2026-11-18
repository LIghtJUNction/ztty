#[path = "../src/repaint.rs"]
mod repaint;

use repaint::{Plan, Repaint};

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
