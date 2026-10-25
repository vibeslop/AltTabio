//! Update work is driven by the outer message loop, outside window callbacks and state borrows
//! held across modal dialogs. Workers return owned results through a channel, never HWND data.

use super::{App, AppHost};
use crate::app_messages::UPDATE_TIMER_ID;
use crate::hook::HookThread;
use crate::win32::wide;
use crate::windows_updater::{self, StagedUpdate};
use alttabio::update::{Checked, Report, Step, Update, Updater};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};
use windows::Win32::UI::WindowsAndMessaging::{
    IDYES, MB_ICONINFORMATION, MB_OK, MB_YESNO, MessageBoxW, SetTimer,
};
use windows::core::{PCWSTR, w};

const FIRST_CHECK: Duration = Duration::from_mins(1);
const RETRY: Duration = Duration::from_hours(1);
const INTERVAL: Duration = Duration::from_hours(24);

enum Completion {
    Checked(Result<Checked, String>),
    Staged(Result<StagedUpdate, String>),
    Relaunched(StagedUpdate, Result<(), String>),
}

enum Action {
    None,
    Offer(Update),
    Report(Report),
    Quit,
}

#[derive(Default, PartialEq)]
enum Request {
    #[default]
    None,
    Pending,
    Active,
}

#[derive(Default, PartialEq)]
enum Restart {
    #[default]
    None,
    Requested,
    HelperReady,
}

pub(super) struct Updates {
    policy: Updater,
    worker: Option<Receiver<Completion>>,
    staged: Option<StagedUpdate>,
    check_at: Instant,
    restart_at: Instant,
    automatic: bool,
    request: Request,
    restart: Restart,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            policy: Updater::default(),
            worker: None,
            staged: None,
            check_at: Instant::now() + FIRST_CHECK,
            restart_at: Instant::now(),
            automatic: false,
            request: Request::None,
            restart: Restart::None,
        }
    }
}

impl Updates {
    pub(super) fn request(&mut self) {
        self.request = Request::Pending;
    }

    fn tick(&mut self, automatic: bool, can_restart: bool) -> Action {
        let now = Instant::now();
        if automatic && !self.automatic {
            self.check_at = now + FIRST_CHECK;
        }
        self.automatic = automatic;
        let mut step = Step::None;
        if self.request == Request::Pending {
            self.request = Request::Active;
            step = self.policy.requested();
        }
        if let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(completion) => {
                    self.worker = None;
                    step = match completion {
                        Completion::Checked(result) => {
                            self.check_at = now + if result.is_err() { RETRY } else { INTERVAL };
                            log_failure(&result);
                            // Disabling automatic updates during a check cancels its install.
                            // A check the user explicitly requested still reports and offers it.
                            self.policy
                                .checked(if !automatic && self.request == Request::None {
                                    Ok(Checked::UpToDate)
                                } else {
                                    result
                                })
                        }
                        Completion::Staged(result) => {
                            log_failure(&result);
                            if result.is_err() {
                                self.check_at = now + RETRY;
                            }
                            let result = result.map(|staged| self.staged = Some(staged));
                            self.policy.installed(result)
                        }
                        Completion::Relaunched(staged, result) => {
                            self.staged = Some(staged);
                            match result {
                                Ok(()) => {
                                    self.restart = Restart::HelperReady;
                                    return if can_restart {
                                        Action::Quit
                                    } else {
                                        Action::None
                                    };
                                }
                                Err(error) => {
                                    eprintln!("{error}");
                                    self.restart = Restart::None;
                                    self.restart_at = now + RETRY;
                                    return if self.request == Request::Active {
                                        self.request = Request::None;
                                        Action::Report(Report::Failed(error))
                                    } else {
                                        Action::None
                                    };
                                }
                            }
                        }
                    };
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.worker = None;
                    let error = "The update worker stopped unexpectedly".to_owned();
                    eprintln!("{error}");
                    // A failed check or download is retryable; no worker can have changed the EXE.
                    step = self.policy.checked(Err(error.clone()));
                    if step == Step::None {
                        step = self.policy.installed(Err(error));
                    }
                    self.check_at = now + RETRY;
                    if self.policy.installed_version().is_some() && self.staged.is_none() {
                        self.policy = Updater::default();
                        self.restart = Restart::None;
                    }
                }
            }
        }
        if self.worker.is_none() && step == Step::None && automatic && now >= self.check_at {
            self.check_at = now + INTERVAL;
            step = self.policy.check_due();
        }
        let action = self.apply(step);
        self.restart_if_ready(action, can_restart, now)
    }

    fn restart_if_ready(&mut self, action: Action, can_restart: bool, now: Instant) -> Action {
        if self.restart == Restart::HelperReady {
            return if can_restart { Action::Quit } else { action };
        }
        if matches!(action, Action::None)
            && can_restart
            && self.worker.is_none()
            && self.staged.is_some()
            && now >= self.restart_at
            && (self.restart == Restart::Requested
                || (self.automatic && windows_updater::user_is_away()))
        {
            let Some(mut staged) = self.staged.take() else {
                return action;
            };
            match spawn(move || {
                let result = windows_updater::relaunch(&mut staged);
                Completion::Relaunched(staged, result)
            }) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    // A failed spawn drops the captured staged files; start a fresh check later.
                    self.policy = Updater::default();
                    self.restart = Restart::None;
                    self.check_at = now + RETRY;
                    eprintln!("{error}");
                    if self.request == Request::Active {
                        self.request = Request::None;
                        return Action::Report(Report::Failed(error));
                    }
                }
            }
        }
        action
    }

    fn apply(&mut self, step: Step) -> Action {
        let checking = matches!(step, Step::Check);
        let started = match step {
            Step::None => return Action::None,
            Step::Offer(update) => return Action::Offer(update),
            Step::Report(report) => {
                self.request = Request::None;
                return Action::Report(report);
            }
            Step::Relaunch => {
                if self.restart != Restart::HelperReady {
                    self.restart = Restart::Requested;
                }
                self.restart_at = Instant::now();
                return Action::None;
            }
            Step::Check => spawn(|| Completion::Checked(windows_updater::check())),
            Step::Install(update) => {
                spawn(move || Completion::Staged(windows_updater::stage(&update)))
            }
        };
        match started {
            Ok(worker) => {
                self.worker = Some(worker);
                Action::None
            }
            Err(error) => {
                eprintln!("{error}");
                self.check_at = Instant::now() + RETRY;
                let step = if checking {
                    self.policy.checked(Err(error))
                } else {
                    self.policy.installed(Err(error))
                };
                self.apply(step)
            }
        }
    }
}

fn log_failure<T>(result: &Result<T, String>) {
    if let Err(error) = result {
        eprintln!("{error}");
    }
}

fn spawn(
    work: impl FnOnce() -> Completion + Send + 'static,
) -> Result<Receiver<Completion>, String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("alttabio-update".to_owned())
        .spawn(move || {
            if sender.send(work()).is_err() {
                eprintln!("AltTabio closed before its update work finished");
            }
        })
        .map_err(|error| format!("Could not start the update worker: {error}"))?;
    Ok(receiver)
}

impl AppHost {
    /// True means a helper is ready and the outer loop can shut down normally.
    pub(super) fn update_tick(&self) -> bool {
        let action = {
            let Ok(mut app) = self.state.try_borrow_mut() else {
                return false;
            };
            if app.modal_state().any_open() {
                return false;
            }
            let automatic = app.settings.general.auto_update && windows_updater::is_release_build();
            let can_restart = !app.is_visible() && app.pending_shell.is_none();
            let action = app.updates.tick(automatic, can_restart);
            let ready = app.updates.policy.installed_version().is_some();
            if let Some(tray) = &app.tray
                && let Err(error) = tray.set_update_installed(ready)
            {
                eprintln!("Could not update the tray command: {error}");
            }
            action
        };
        match action {
            Action::None => false,
            Action::Quit => true,
            Action::Offer(update) => {
                let install = self.update_dialog(
                    &format!(
                        "AltTabio {} is available. Install it and restart AltTabio?",
                        update.version
                    ),
                    true,
                );
                if let Ok(mut app) = self.state.try_borrow_mut() {
                    if !install {
                        app.updates.request = Request::None;
                    }
                    let step = app.updates.policy.offer_answered(update, install);
                    let action = app.updates.apply(step);
                    drop(app);
                    if let Action::Report(report) = action {
                        self.report_update(report);
                    }
                }
                false
            }
            Action::Report(report) => {
                self.report_update(report);
                false
            }
        }
    }

    fn report_update(&self, report: Report) {
        let text = match report {
            Report::UpToDate => "AltTabio is up to date.".to_owned(),
            Report::Blocked(version, reason) => {
                format!("AltTabio {version} is available. {reason}")
            }
            Report::Failed(error) => format!("Could not update AltTabio. {error}"),
        };
        self.update_dialog(&text, false);
    }

    fn update_dialog(&self, text: &str, offer: bool) -> bool {
        let (hwnd, suspension) = {
            let Ok(mut app) = self.state.try_borrow_mut() else {
                return false;
            };
            app.hide_overlay();
            app.update_dialog_open = true;
            (
                app.hwnd,
                app.hooks.as_ref().map(HookThread::suspend_interception),
            )
        };
        let text = wide(text);
        let result = unsafe {
            // SAFETY: hwnd is owned by this UI thread and text is terminated and lives through
            // the modal call. No app-state borrow is held across its nested message loop.
            MessageBoxW(
                Some(hwnd),
                PCWSTR(text.as_ptr()),
                w!("AltTabio"),
                MB_ICONINFORMATION | if offer { MB_YESNO } else { MB_OK },
            )
        };
        if result.0 == 0 {
            eprintln!(
                "Could not show the update dialog: {}",
                windows::core::Error::from_thread()
            );
        }
        if let Ok(mut app) = self.state.try_borrow_mut() {
            app.update_dialog_open = false;
        }
        drop(suspension);
        result == IDYES
    }
}

impl App {
    pub(super) fn start_update_timer(&mut self) {
        let timer = unsafe {
            // SAFETY: hwnd is live, the ID belongs to this app, and no timer callback is installed.
            SetTimer(Some(self.hwnd), UPDATE_TIMER_ID, 1000, None)
        };
        if timer == 0 {
            eprintln!(
                "Could not start the update timer: {}",
                windows::core::Error::from_thread()
            );
        }
    }
}
