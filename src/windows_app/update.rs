//! Background work and modeless presentation are driven by the outer message loop.
use super::{App, AppHost};
use crate::app_messages::{UPDATE_TIMER_ID, WM_SHOW_UPDATES, WM_UPDATE_UI};
use crate::update_window::UpdateWindow;
use crate::windows_updater::{self, Progress, StageError, StagedUpdate};
use alttabio::update::{Checked, Step, Update, Updater};
use alttabio::update_status::{Command, Stage, Status};
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, TryRecvError},
};
use std::time::{Duration, Instant, SystemTime};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{MSG, PostQuitMessage, SetTimer, WM_TIMER};

const FIRST_CHECK: Duration = Duration::from_mins(1);
const RETRY: Duration = Duration::from_hours(1);
const INTERVAL: Duration = Duration::from_hours(24);

pub(super) fn is_update_timer(message: &MSG, owner: HWND) -> bool {
    message.hwnd == owner && message.message == WM_TIMER && message.wParam.0 == UPDATE_TIMER_ID
}

enum Completion {
    Checked(Result<Checked, String>),
    Staged(Result<StagedUpdate, StageError>),
    Relaunched(StagedUpdate, Result<(), String>),
}

#[derive(Default, PartialEq)]
enum Restart {
    #[default]
    None,
    HelperReady,
}

pub(super) struct Updates {
    policy: Updater,
    worker: Option<Receiver<Completion>>,
    // One coalesced progress value bounds memory even while a nested native loop is busy.
    progress: Arc<Mutex<Option<Progress>>>,
    staged: Option<StagedUpdate>,
    offer: Option<Update>,
    status: Status,
    check_at: Instant,
    restart_at: Instant,
    automatic: bool,
    check_requested: bool,
    restart_approved: bool,
    restart: Restart,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            policy: Updater::default(),
            worker: None,
            progress: Arc::new(Mutex::new(None)),
            staged: None,
            offer: None,
            status: Status::Checking,
            check_at: Instant::now() + FIRST_CHECK,
            restart_at: Instant::now(),
            automatic: false,
            check_requested: false,
            restart_approved: false,
            restart: Restart::None,
        }
    }
}

impl Updates {
    fn request(&mut self) {
        if !windows_updater::is_release_build() {
            self.status = Status::LocalBuild;
            return;
        }
        // Viewing a download or staged release does not grant immediate restart authority.
        if self.worker.is_some() {
            if self.status == Status::Checking {
                self.check_requested = true;
                let step = self.policy.check_or_observe();
                self.apply(step);
            }
            return;
        }
        if self.staged.is_some() || self.offer.is_some() {
            return;
        }
        self.check_requested = true;
        let step = self.policy.check_or_observe();
        self.apply(step);
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Check if self.worker.is_none() && self.staged.is_none() => {
                self.offer = None;
                self.request();
            }
            Command::Install if self.worker.is_none() && self.staged.is_none() => {
                if let Some(update) = self.offer.clone() {
                    let step = self.policy.offer_answered(update, true);
                    if matches!(step, Step::Install(_)) {
                        self.restart_approved = true;
                    }
                    self.apply(step);
                }
            }
            Command::Restart if self.worker.is_none() => {
                if self.staged.is_some() {
                    self.restart_approved = true;
                    self.restart_at = Instant::now();
                } else {
                    self.request();
                }
            }
            _ => {}
        }
    }

    fn tick(&mut self, automatic: bool, can_restart: bool) -> bool {
        let now = Instant::now();
        if automatic && !self.automatic {
            self.check_at = now + FIRST_CHECK;
        }
        self.automatic = automatic;
        let progress = self
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(progress) = progress {
            self.status = match progress {
                Progress::Downloading { received, total } => {
                    Status::Downloading { received, total }
                }
                Progress::Verifying => Status::Verifying,
            };
        }
        let completion = self.worker.as_ref().map(Receiver::try_recv);
        if let Some(result) = completion {
            match result {
                Ok(completion) => {
                    self.worker = None;
                    self.complete(completion, now);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    let stage = match self.status {
                        Status::Checking => None,
                        Status::Verifying => Some(Stage::Verify),
                        Status::Preparing => Some(Stage::Prepare),
                        _ => Some(Stage::Download),
                    };
                    self.worker = None;
                    self.policy = Updater::default();
                    self.restart = Restart::None;
                    self.restart_approved = false;
                    self.check_requested = false;
                    self.check_at = now + RETRY;
                    self.fail(stage, "The update worker stopped unexpectedly".into());
                }
            }
        }
        if self.worker.is_none() && automatic && now >= self.check_at {
            self.check_at = now + INTERVAL;
            let step = self.policy.check_due();
            self.apply(step);
        }
        self.restart_if_ready(can_restart, now, windows_updater::user_is_away)
    }

    fn complete(&mut self, completion: Completion, now: Instant) {
        // A final completion supersedes any coalesced progress written just before it.
        self.progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        match completion {
            Completion::Checked(result) => {
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
                // Disabling automatic updates mid-check prevents its installation.
                let result = if !self.automatic && !self.check_requested {
                    Ok(Checked::UpToDate)
                } else {
                    result
                };
                let step = self.policy.checked(result);
                self.check_requested = false;
                self.apply(step);
            }
            Completion::Staged(result) => {
                let result = match result {
                    Ok(staged) => {
                        self.staged = Some(staged);
                        self.offer = None;
                        Ok(())
                    }
                    Err(error) => {
                        self.check_at = now + INTERVAL;
                        self.restart_approved = false;
                        self.fail(Some(error.stage), error.detail.clone());
                        Err(error.detail)
                    }
                };
                let step = self.policy.installed(result);
                if let Some(version) = self.policy.installed_version() {
                    self.status = Status::Ready(version);
                }
                self.apply(step);
            }
            Completion::Relaunched(staged, result) => {
                self.staged = Some(staged);
                match result {
                    Ok(()) => {
                        self.restart = Restart::HelperReady;
                        if let Some(version) = self.policy.installed_version() {
                            self.status = Status::Ready(version);
                        }
                    }
                    Err(error) => {
                        self.restart = Restart::None;
                        self.restart_approved = false;
                        self.restart_at = now + RETRY;
                        self.fail(Some(Stage::Prepare), error);
                    }
                }
            }
        }
    }

    fn restart_requested(&self, away: impl FnOnce() -> bool) -> bool {
        self.restart_approved || (self.automatic && away())
    }

    fn restart_if_ready(
        &mut self,
        can_restart: bool,
        now: Instant,
        away: impl FnOnce() -> bool,
    ) -> bool {
        if !can_restart || now < self.restart_at || !self.restart_requested(away) {
            return false;
        }
        if self.restart == Restart::HelperReady {
            let result = self
                .staged
                .as_ref()
                .ok_or_else(|| "The staged update is missing".to_owned())
                .and_then(windows_updater::approve_relaunch);
            match result {
                Ok(()) => {
                    self.status = Status::Restarting;
                    return true;
                }
                Err(error) => {
                    self.restart_at = now + RETRY;
                    self.restart_approved = false;
                    self.fail(Some(Stage::Prepare), error);
                }
            }
        } else if self.worker.is_none()
            && let Some(mut staged) = self.staged.take()
        {
            self.status = Status::Preparing;
            match spawn(move || {
                let result = windows_updater::relaunch(&mut staged);
                Completion::Relaunched(staged, result)
            }) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    self.policy = Updater::default();
                    self.restart_approved = false;
                    self.check_at = now + RETRY;
                    self.fail(None, error);
                }
            }
        }
        false
    }

    fn apply(&mut self, step: Step) {
        let checking = matches!(step, Step::Check);
        let started = match step {
            Step::None | Step::Report(_) => return,
            Step::Offer(update) => {
                self.offer = Some(update.clone());
                self.status = Status::Available(update);
                return;
            }
            Step::Relaunch => {
                // Only explicit Install / Restart commands set restart_approved.
                self.restart_at = Instant::now();
                return;
            }
            Step::Check => {
                self.offer = None;
                self.status = Status::Checking;
                let manual = self.check_requested;
                spawn(move || Completion::Checked(windows_updater::check(manual)))
            }
            Step::Install(update) => {
                self.status = Status::Downloading {
                    received: 0,
                    total: 0,
                };
                self.offer = Some(update.clone());
                let progress = self.progress.clone();
                spawn(move || {
                    Completion::Staged(windows_updater::stage(&update, |event| {
                        *progress
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(event);
                    }))
                })
            }
        };
        match started {
            Ok(worker) => self.worker = Some(worker),
            Err(error) => {
                self.check_at = Instant::now() + RETRY;
                let step = if checking {
                    self.policy.checked(Err(error.clone()))
                } else {
                    self.policy.installed(Err(error.clone()))
                };
                self.restart_approved = false;
                self.check_requested = false;
                self.fail(
                    if checking {
                        None
                    } else {
                        Some(Stage::Download)
                    },
                    error,
                );
                self.apply(step);
            }
        }
    }

    fn fail(&mut self, stage: Option<Stage>, detail: String) {
        eprintln!("{detail}");
        self.status = Status::Failed { stage, detail };
    }
}

fn spawn(
    work: impl FnOnce() -> Completion + Send + 'static,
) -> Result<Receiver<Completion>, String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("alttabio-update".into())
        .spawn(move || {
            if sender.send(work()).is_err() {
                eprintln!("AltTabio closed before its update work finished");
            }
        })
        .map_err(|error| format!("Could not start the update worker: {error}"))?;
    Ok(receiver)
}

impl AppHost {
    pub(super) fn handle_update_message(&self, message: &MSG) -> bool {
        let owner = self.state.borrow().hwnd;
        if message.hwnd == owner && message.message == WM_SHOW_UPDATES {
            self.show_updates();
            return true;
        }
        if message.hwnd == owner
            && (message.message == WM_UPDATE_UI || is_update_timer(message, owner))
        {
            self.update_commands();
            if self.update_tick() {
                // SAFETY: queue shutdown on this UI thread, including through the Settings loop.
                unsafe { PostQuitMessage(0) };
            }
            return true;
        }
        self.update_window
            .borrow()
            .as_ref()
            .is_some_and(|window| window.translate(message))
    }

    fn show_updates(&self) {
        let (owner, theme) = {
            let mut app = self.state.borrow_mut();
            app.hide_overlay();
            (app.hwnd, app.settings.appearance.theme)
        };
        {
            let mut window = self.update_window.borrow_mut();
            if window.is_none() {
                match UpdateWindow::create(owner, theme) {
                    Ok(created) => *window = Some(created),
                    Err(error) => {
                        super::show_error_for_window(
                            owner,
                            &format!("Could not open update status: {error}"),
                        );
                        return;
                    }
                }
            }
            if let Some(window) = window.as_mut() {
                let view = {
                    let app = self.state.borrow();
                    app.updates.status.view(
                        env!("CARGO_PKG_VERSION"),
                        app.updates.automatic,
                        app.updates.restart_approved,
                    )
                };
                if let Err(error) = window.refresh(view, theme) {
                    eprintln!("Could not show update status: {error}");
                }
                window.show();
            }
        }
        self.state.borrow_mut().updates.request();
        self.refresh_updates();
    }

    fn update_commands(&self) {
        let events = self
            .update_window
            .borrow_mut()
            .as_mut()
            .map(UpdateWindow::take_events);
        match events {
            Some(Ok(events)) => {
                if events.close
                    && let Some(window) = self.update_window.borrow().as_ref()
                {
                    window.hide();
                }
                if let Some(command) = events.command {
                    if command == Command::Releases {
                        if let Err(error) = super::commands::open_releases() {
                            eprintln!("Could not open releases: {error}");
                        }
                    } else {
                        self.state.borrow_mut().updates.command(command);
                    }
                }
            }
            Some(Err(error)) => eprintln!("Could not handle update window: {error}"),
            None => {}
        }
    }

    pub(super) fn update_tick(&self) -> bool {
        let quit = {
            let mut app = self.state.borrow_mut();
            if let Some(tray) = &app.tray
                && let Err(error) = tray.retry_registration()
            {
                eprintln!("Could not register the AltTabio tray icon yet: {error}");
            }
            let quit = app.poll_updates();
            if let Some(tray) = &app.tray
                && let Err(error) = tray.set_update_installed(
                    app.updates.staged.is_some()
                        || app.updates.worker.is_some()
                        || app.updates.offer.is_some(),
                )
            {
                eprintln!("Could not update the tray command: {error}");
            }
            quit
        };
        self.refresh_updates();
        quit
    }

    fn refresh_updates(&self) {
        let (view, theme) = {
            let app = self.state.borrow();
            (
                app.updates.status.view(
                    env!("CARGO_PKG_VERSION"),
                    app.updates.automatic,
                    app.updates.restart_approved,
                ),
                app.settings.appearance.theme,
            )
        };
        if let Some(window) = self.update_window.borrow_mut().as_mut()
            && let Err(error) = window.refresh(view, theme)
        {
            eprintln!("Could not refresh update status: {error}");
        }
    }
}

impl App {
    fn poll_updates(&mut self) -> bool {
        let automatic = self.settings.general.auto_update && windows_updater::is_release_build();
        let can_restart =
            !self.dialog_state().any_open() && !self.is_visible() && self.pending_shell.is_none();
        self.updates.tick(automatic, can_restart)
    }

    pub(super) fn start_update_timer(&mut self) {
        // SAFETY: this app owns hwnd and the timer ID. No native timer callback is installed.
        if unsafe { SetTimer(Some(self.hwnd), UPDATE_TIMER_ID, 250, None) } == 0 {
            eprintln!(
                "Could not start the update timer: {}",
                windows::core::Error::from_thread()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabling_automatic_updates_during_a_check_keeps_a_valid_manual_offer() {
        let mut updates = Updates::default();
        assert_eq!(updates.policy.check_due(), Step::Check);
        let update = Update {
            version: alttabio::update::Version::parse("9.0.0")
                .unwrap_or_else(|| panic!("fixture version")),
            url: "fixture".into(),
        };
        updates.complete(
            Completion::Checked(Ok(Checked::Available(update.clone()))),
            Instant::now(),
        );
        assert!(updates.worker.is_none());
        assert_eq!(updates.offer, Some(update.clone()));
        assert_eq!(
            updates.policy.offer_answered(update.clone(), true),
            Step::Install(update)
        );
        assert!(!updates.restart_approved);
    }

    #[test]
    fn disconnected_download_keeps_a_retryable_offer() {
        let mut updates = Updates::default();
        let update = Update {
            version: alttabio::update::Version::parse("9.0.0")
                .unwrap_or_else(|| panic!("fixture version")),
            url: "fixture".into(),
        };
        updates.offer = Some(update.clone());
        updates.status = Status::Downloading {
            received: 2,
            total: 10,
        };
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        updates.worker = Some(receiver);
        assert!(!updates.tick(false, false));
        assert_eq!(updates.offer, Some(update.clone()));
        assert_eq!(
            updates.policy.offer_answered(update.clone(), true),
            Step::Install(update)
        );
        assert!(matches!(
            updates.status,
            Status::Failed {
                stage: Some(Stage::Download),
                ..
            }
        ));
    }

    #[test]
    fn manual_observation_never_authorizes_background_restart() {
        let mut updates = Updates {
            automatic: true,
            ..Updates::default()
        };
        assert!(!updates.restart_requested(|| false));
        updates.check_requested = true;
        assert!(!updates.restart_requested(|| false));
        updates.restart_approved = true;
        assert!(updates.restart_requested(|| panic!("approved restart does not need idle")));
        updates.restart_approved = false;
        updates.automatic = false;
        assert!(!updates.restart_requested(|| true));
    }

    #[test]
    fn failed_staging_preserves_retry_offer_and_defers_automatic_retry() {
        let mut updates = Updates {
            automatic: true,
            ..Updates::default()
        };
        let version =
            alttabio::update::Version::parse("9.0.0").unwrap_or_else(|| panic!("version"));
        let update = Update {
            version,
            url: "fixture".into(),
        };
        assert_eq!(updates.policy.check_due(), Step::Check);
        assert!(matches!(
            updates
                .policy
                .checked(Ok(Checked::Available(update.clone()))),
            Step::Install(_)
        ));
        updates.offer = Some(update);
        let before = Instant::now();
        updates.complete(
            Completion::Staged(Err(StageError {
                stage: Stage::Verify,
                detail: "fixture".into(),
            })),
            before,
        );
        assert!(matches!(
            updates.status,
            Status::Failed {
                stage: Some(Stage::Verify),
                ..
            }
        ));
        assert!(updates.offer.is_some());
        assert_eq!(updates.check_at, before + INTERVAL);
        assert!(!updates.restart_approved);
    }

    #[test]
    fn helper_ready_does_not_override_user_return_or_disabled_automatic_updates() {
        let mut updates = Updates {
            automatic: true,
            restart: Restart::HelperReady,
            ..Updates::default()
        };
        assert!(!updates.restart_if_ready(true, Instant::now(), || false));
        updates.automatic = false;
        assert!(!updates.restart_if_ready(true, Instant::now(), || true));
        updates.restart_approved = true;
        assert!(!updates.restart_if_ready(false, Instant::now(), || panic!("switcher gate")));
    }
}
