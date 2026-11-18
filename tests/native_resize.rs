//! Failure injection for the production native multi-PTY resize transaction.
use ztty::native_resize as resize;

fn size(cols: u16) -> zpty::Size {
    zpty::Size::new(cols, 24)
}

fn plans() -> Vec<resize::Plan> {
    (0..3)
        .map(|_| resize::Plan {
            actual: size(80),
            committed: size(80),
            desired: size(40),
        })
        .collect()
}

#[test]
fn all_ptys_accept_before_the_caller_can_commit_any_grid() {
    let mut plans = plans();
    let mut accepted = Vec::new();
    resize::coordinate(&mut plans, false, |index, next| {
        accepted.push((index, next));
        true
    })
    .unwrap();
    assert_eq!(accepted, vec![(0, size(40)), (1, size(40)), (2, size(40))]);
    assert!(
        plans
            .iter()
            .all(|plan| plan.actual == size(40) && plan.committed == size(80))
    );
}

#[test]
fn a_middle_failure_rolls_back_only_changed_ptys_and_keeps_old_grids() {
    let mut plans = plans();
    let mut calls = Vec::new();
    let error = resize::coordinate(&mut plans, false, |index, next| {
        calls.push((index, next));
        index != 1
    })
    .unwrap_err();
    assert_eq!(
        error,
        resize::Failure {
            pane: 1,
            rollback_failed: false
        }
    );
    assert_eq!(calls, vec![(0, size(40)), (1, size(40)), (0, size(80))]);
    assert!(
        plans
            .iter()
            .all(|plan| plan.actual == size(80) && plan.committed == size(80))
    );
}

#[test]
fn failed_rollback_retains_actual_size_and_a_later_retry_recovers() {
    let mut plans = plans();
    let error = resize::coordinate(&mut plans, false, |index, next| {
        index == 0 && next == size(40)
    })
    .unwrap_err();
    assert_eq!(
        error,
        resize::Failure {
            pane: 1,
            rollback_failed: true
        }
    );
    assert_eq!(plans[0].actual, size(40));
    assert_eq!(plans[0].committed, size(80));
    assert_eq!(plans[1].actual, size(80));
    let mut calls = Vec::new();
    resize::coordinate(&mut plans, false, |index, next| {
        calls.push((index, next));
        true
    })
    .unwrap();
    assert_eq!(calls, vec![(1, size(40)), (2, size(40))]);
    assert!(plans.iter().all(|plan| plan.actual == size(40)));
}

#[test]
fn unchanged_grid_font_changes_still_require_every_pty_to_accept() {
    let mut plans = plans();
    for plan in &mut plans {
        plan.desired = size(80);
    }
    let mut calls = Vec::new();
    let error = resize::coordinate(&mut plans, true, |index, next| {
        calls.push((index, next));
        index != 2
    })
    .unwrap_err();
    assert_eq!(error.pane, 2);
    assert_eq!(calls, vec![(0, size(80)), (1, size(80)), (2, size(80))]);
    assert!(plans.iter().all(|plan| plan.actual == plan.committed));
}

#[test]
fn a_hidden_or_cancelled_pane_with_uncommitted_size_is_restored() {
    let mut plans = plans();
    plans[2].actual = size(100);
    plans[2].desired = plans[2].committed;
    let error = resize::coordinate(&mut plans, false, |index, next| {
        index != 0 || next == size(80)
    })
    .unwrap_err();
    assert_eq!(error.pane, 0);
    assert_eq!(plans[2].actual, size(80));
}
