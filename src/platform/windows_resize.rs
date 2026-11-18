//! Coordinate native PTY sizes without committing a partially resized view.
//! Actual sizes are retained even when rollback fails, so the frontend can
//! pause parsing against a stale grid and retry without losing PTY identities.

#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub actual: zpty::Size,
    pub desired: zpty::Size,
    pub committed: zpty::Size,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    pub pane: usize,
    pub rollback_failed: bool,
}

pub fn coordinate(
    plans: &mut [Plan],
    force: bool,
    mut resize: impl FnMut(usize, zpty::Size) -> bool,
) -> Result<(), Failure> {
    for index in 0..plans.len() {
        let desired = plans[index].desired;
        if plans[index].actual == desired && !force {
            continue;
        }
        if resize(index, desired) {
            plans[index].actual = desired;
            continue;
        }
        let mut rollback_failed = false;
        for (prior, plan) in plans.iter_mut().enumerate() {
            if plan.actual != plan.committed {
                if resize(prior, plan.committed) {
                    plan.actual = plan.committed;
                } else {
                    rollback_failed = true;
                }
            }
        }
        return Err(Failure {
            pane: index,
            rollback_failed,
        });
    }
    Ok(())
}
