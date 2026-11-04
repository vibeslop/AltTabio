//! Update decisions and retained status, independent of native workers and windows.
use crate::update::{Checked, Step, Update, Updater};
use crate::update_status::{Command, Failure, Stage, Status, View};
use std::time::{Duration, Instant, SystemTime};

const FIRST_CHECK: Duration = Duration::from_mins(1);
const RETRY: Duration = Duration::from_hours(1);
const INTERVAL: Duration = Duration::from_hours(24);

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    None,
    Check { manual: bool },
    Stage(Update),
    PrepareRestart,
    ApproveRestart,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Job {
    Check,
    Stage,
    Prepare,
}

#[derive(Default)]
enum Restart {
    #[default]
    Waiting,
    Approved,
    Prepared {
        approved: bool,
    },
}
impl Restart {
    fn approve(&mut self) {
        *self = if matches!(self, Self::Prepared { .. }) {
            Self::Prepared { approved: true }
        } else {
            Self::Approved
        };
    }
    fn approved(&self) -> bool {
        matches!(self, Self::Approved | Self::Prepared { approved: true })
    }
    fn revoke(&mut self) {
        *self = if matches!(self, Self::Prepared { .. }) {
            Self::Prepared { approved: false }
        } else {
            Self::Waiting
        };
    }
}

pub struct UpdateSession {
    policy: Updater,
    status: Status,
    offer: Option<Update>,
    job: Option<Job>,
    started: bool,
    check_at: Instant,
    restart_at: Instant,
    automatic: bool,
    check_requested: bool,
    restart: Restart,
}

impl Default for UpdateSession {
    fn default() -> Self {
        Self::new(Instant::now())
    }
}

impl UpdateSession {
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            policy: Updater::default(),
            status: Status::Checking,
            offer: None,
            job: None,
            started: false,
            check_at: now + FIRST_CHECK,
            restart_at: now,
            automatic: false,
            check_requested: false,
            restart: Restart::Waiting,
        }
    }
}

impl UpdateSession {
    /// Open or focus status. Only a first open starts a new check; completed results are retained.
    pub fn open(&mut self, release: bool, now: Instant) -> Effect {
        if !release {
            self.started = true;
            self.status = Status::LocalBuild;
            return Effect::None;
        }
        if self.job == Some(Job::Check) {
            self.check_requested = true;
            let step = self.policy.check_or_observe();
            return self.apply(step, now);
        }
        if self.started {
            return Effect::None;
        }
        self.check(now)
    }

    pub fn command(&mut self, command: Command, now: Instant) -> Effect {
        if self.job.is_some() {
            return Effect::None;
        }
        match command {
            Command::Check if self.policy.installed_version().is_none() => self.check(now),
            Command::Install if self.policy.installed_version().is_none() => {
                let Some(update) = self.offer.clone() else {
                    return Effect::None;
                };
                let step = self.policy.offer_answered(update, true);
                if matches!(step, Step::Install(_)) {
                    self.restart.approve();
                }
                self.apply(step, now)
            }
            Command::Restart => {
                if self.policy.installed_version().is_some() {
                    self.restart.approve();
                    self.restart_at = now;
                    Effect::None
                } else {
                    self.check(now)
                }
            }
            _ => Effect::None,
        }
    }

    fn check(&mut self, now: Instant) -> Effect {
        self.check_requested = true;
        let step = self.policy.check_or_observe();
        self.apply(step, now)
    }

    pub fn tick(
        &mut self,
        automatic: bool,
        can_restart: bool,
        now: Instant,
        away: impl FnOnce() -> bool,
    ) -> Effect {
        self.configure_automatic(automatic, now);
        if self.job.is_none() && automatic && now >= self.check_at {
            self.check_at = now + INTERVAL;
            let step = self.policy.check_due();
            let effect = self.apply(step, now);
            if effect != Effect::None {
                return effect;
            }
        }
        if self.job.is_some()
            || self.policy.installed_version().is_none()
            || !can_restart
            || now < self.restart_at
            || !(self.restart.approved() || (automatic && away()))
        {
            return Effect::None;
        }
        if matches!(self.restart, Restart::Prepared { .. }) {
            Effect::ApproveRestart
        } else {
            self.job = Some(Job::Prepare);
            self.status = Status::Preparing;
            Effect::PrepareRestart
        }
    }

    pub fn checked(&mut self, result: Result<Checked, String>, now: Instant) -> Effect {
        self.job = None;
        self.check_at = now + if result.is_err() { RETRY } else { INTERVAL };
        match &result {
            Ok(Checked::UpToDate) => self.status = Status::Current(SystemTime::now()),
            Ok(Checked::Available(update)) => {
                self.offer = Some(update.clone());
                self.status = Status::Available(update.clone());
            }
            Ok(Checked::Blocked(version, detail)) => {
                self.status = Status::Blocked {
                    version: *version,
                    detail: detail.clone(),
                }
            }
            Err(error) => self.fail(None, error.clone()),
        }
        // A check already requested by the user still offers a release after auto-update is disabled.
        let result = if !self.automatic && !self.check_requested {
            Ok(Checked::UpToDate)
        } else {
            result
        };
        let step = self.policy.checked(result);
        self.check_requested = false;
        self.apply(step, now)
    }

    pub fn configure_automatic(&mut self, automatic: bool, now: Instant) {
        if automatic && !self.automatic {
            self.check_at = now + FIRST_CHECK;
        }
        self.automatic = automatic;
    }

    pub fn staged(&mut self, result: Result<(), Failure>, now: Instant) -> Effect {
        self.job = None;
        let result = match result {
            Ok(()) => {
                self.offer = None;
                Ok(())
            }
            Err(error) => {
                self.check_at = now + INTERVAL;
                self.restart.revoke();
                self.fail(Some(error.stage), error.detail.clone());
                Err(error.detail)
            }
        };
        let step = self.policy.installed(result);
        if let Some(version) = self.policy.installed_version() {
            self.status = Status::Ready(version);
        }
        self.apply(step, now)
    }

    pub fn prepared(&mut self, result: Result<(), String>, now: Instant) {
        self.job = None;
        match result {
            Ok(()) => {
                self.restart = Restart::Prepared {
                    approved: self.restart.approved(),
                };
                if let Some(version) = self.policy.installed_version() {
                    self.status = Status::Ready(version);
                }
            }
            Err(error) => self.approval_failed(error, now),
        }
    }

    pub fn approval_failed(&mut self, error: String, now: Instant) {
        self.restart.revoke();
        self.restart_at = now + RETRY;
        self.fail(Some(Stage::Prepare), error);
    }

    pub fn approved(&mut self) {
        self.status = Status::Restarting;
    }

    pub fn progress(&mut self, status: Status) {
        if self.job == Some(Job::Stage) {
            self.status = status;
        }
    }

    pub fn worker_failed(&mut self, now: Instant) {
        let stage = match self.status {
            Status::Checking => None,
            Status::Verifying => Some(Stage::Verify),
            Status::Preparing => Some(Stage::Prepare),
            _ => Some(Stage::Download),
        };
        self.job = None;
        self.policy = Updater::default();
        self.restart = Restart::Waiting;
        self.restart.revoke();
        self.check_requested = false;
        self.check_at = now + RETRY;
        self.fail(stage, "The update worker stopped unexpectedly".into());
    }

    pub fn start_failed(&mut self, effect: &Effect, error: String, now: Instant) {
        self.job = None;
        self.check_at = now + RETRY;
        match effect {
            Effect::Check { .. } => {
                self.policy.checked(Err(error.clone()));
            }
            Effect::Stage(_) => {
                self.policy.installed(Err(error.clone()));
            }
            Effect::PrepareRestart => {
                self.policy = Updater::default();
                self.restart = Restart::Waiting;
            }
            Effect::None | Effect::ApproveRestart => {}
        }
        self.restart.revoke();
        self.check_requested = false;
        let stage = match effect {
            Effect::Stage(_) => Some(Stage::Download),
            Effect::PrepareRestart => Some(Stage::Prepare),
            _ => None,
        };
        self.fail(stage, error);
    }

    #[must_use]
    pub fn view(&self, version: &str) -> View {
        self.status
            .view(version, self.automatic, self.restart.approved())
    }

    #[must_use]
    pub fn has_status(&self) -> bool {
        self.started
    }

    fn fail(&mut self, stage: Option<Stage>, detail: String) {
        self.status = Status::Failed { stage, detail };
    }

    fn apply(&mut self, step: Step, now: Instant) -> Effect {
        match step {
            Step::None | Step::Report(_) => Effect::None,
            Step::Offer(update) => {
                self.offer = Some(update.clone());
                self.status = Status::Available(update);
                Effect::None
            }
            Step::Relaunch => {
                self.restart_at = now;
                Effect::None
            }
            Step::Check => {
                self.started = true;
                self.offer = None;
                self.status = Status::Checking;
                self.job = Some(Job::Check);
                Effect::Check {
                    manual: self.check_requested,
                }
            }
            Step::Install(update) => {
                self.job = Some(Job::Stage);
                self.status = Status::Downloading {
                    received: 0,
                    total: 0,
                };
                self.offer = Some(update.clone());
                Effect::Stage(update)
            }
        }
    }
}

#[cfg(test)]
mod tests;
