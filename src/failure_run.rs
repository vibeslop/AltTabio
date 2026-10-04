//! Runs of a failure that recurs on every message, such as a paint that keeps failing, so the
//! caller logs the first failure of each run rather than every one.

use std::cell::Cell;

/// Whether the last attempt of a repeated operation failed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FailureRun {
    failing: bool,
}

impl FailureRun {
    /// Records a failure and returns whether it starts a run, the one failure worth logging.
    #[must_use]
    pub const fn fail(&mut self) -> bool {
        let started = !self.failing;
        self.failing = true;
        started
    }

    /// Ends the current run, so the next failure is logged again.
    pub const fn succeed(&mut self) {
        self.failing = false;
    }
}

/// A `FailureRun` for state that is only reachable through a shared borrow.
#[derive(Debug, Default)]
pub struct SharedFailureRun(Cell<FailureRun>);

impl SharedFailureRun {
    /// Records a failure and returns whether it starts a run, the one failure worth logging.
    #[must_use]
    pub fn fail(&self) -> bool {
        let mut run = self.0.get();
        let started = run.fail();
        self.0.set(run);
        started
    }

    /// Ends the current run, so the next failure is logged again.
    pub fn succeed(&self) {
        self.0.set(FailureRun::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_first_failure_of_a_run_is_reported() {
        let mut run = FailureRun::default();

        assert!(run.fail());
        assert!(!run.fail());
        assert!(!run.fail());
    }

    #[test]
    fn a_success_starts_a_new_run() {
        let mut run = FailureRun::default();

        run.succeed();
        assert!(run.fail());
        run.succeed();
        run.succeed();
        assert!(run.fail());
        assert!(!run.fail());
    }

    #[test]
    fn the_shared_run_reports_like_the_owned_one() {
        let run = SharedFailureRun::default();

        assert!(run.fail());
        assert!(!run.fail());
        run.succeed();
        assert!(run.fail());
    }
}
