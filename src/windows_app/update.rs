//! Native worker ownership and resource operations; decisions live in `UpdateSession`.
use super::{App, AppHost};
use crate::app_messages::{UPDATE_TIMER_ID, WM_SHOW_UPDATES, WM_UPDATE_UI};
use crate::update_window::UpdateWindow;
use crate::windows_updater::{self, Progress, StagedUpdate};
use alttabio::update::Checked;
use alttabio::update_session::{Effect, UpdateSession};
use alttabio::update_status::{Command, Failure, Status};
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, TryRecvError},
};
use std::time::Instant;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{MSG, PostQuitMessage, SetTimer, WM_TIMER};

pub(super) fn is_update_timer(message: &MSG, owner: HWND) -> bool {
    message.hwnd == owner && message.message == WM_TIMER && message.wParam.0 == UPDATE_TIMER_ID
}

enum Completion {
    Checked(Result<Checked, String>),
    Staged(Result<StagedUpdate, Failure>),
    Relaunched(StagedUpdate, Result<(), String>),
}

#[derive(Default)]
pub(super) struct Updates {
    session: UpdateSession,
    worker: Option<Receiver<Completion>>,
    // One coalesced progress value bounds memory while nested native loops are busy.
    progress: Arc<Mutex<Option<Progress>>>,
    staged: Option<StagedUpdate>,
}

impl Updates {
    fn request(&mut self) {
        let effect = self
            .session
            .open(windows_updater::is_release_build(), Instant::now());
        self.apply(&effect);
    }

    fn command(&mut self, command: Command) {
        if !windows_updater::is_release_build() {
            self.request();
            return;
        }
        let effect = self.session.command(command, Instant::now());
        self.apply(&effect);
    }

    fn tick(&mut self, automatic: bool, can_restart: bool) -> bool {
        let now = Instant::now();
        self.session.configure_automatic(automatic, now);
        let progress = self
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(progress) = progress {
            self.session.progress(match progress {
                Progress::Downloading { received, total } => {
                    Status::Downloading { received, total }
                }
                Progress::Verifying => Status::Verifying,
            });
        }
        match self.worker.as_ref().map(Receiver::try_recv) {
            Some(Ok(completion)) => {
                self.worker = None;
                self.progress
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                let effect = self.complete(completion, now);
                if self.apply(&effect) {
                    return true;
                }
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.worker = None;
                self.session.worker_failed(now);
                eprintln!("The update worker stopped unexpectedly");
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
        let effect = self
            .session
            .tick(automatic, can_restart, now, windows_updater::user_is_away);
        self.apply(&effect)
    }

    fn complete(&mut self, completion: Completion, now: Instant) -> Effect {
        match completion {
            Completion::Checked(result) => {
                if let Err(error) = &result {
                    eprintln!("{error}");
                }
                self.session.checked(result, now)
            }
            Completion::Staged(result) => {
                let result = match result {
                    Ok(staged) => {
                        self.staged = Some(staged);
                        Ok(())
                    }
                    Err(error) => {
                        eprintln!("{}", error.detail);
                        Err(error)
                    }
                };
                self.session.staged(result, now)
            }
            Completion::Relaunched(staged, result) => {
                self.staged = Some(staged);
                if let Err(error) = &result {
                    eprintln!("{error}");
                }
                self.session.prepared(result, now);
                Effect::None
            }
        }
    }

    fn apply(&mut self, effect: &Effect) -> bool {
        let result = match effect {
            Effect::None => return false,
            Effect::ApproveRestart => {
                let result = self
                    .staged
                    .as_ref()
                    .ok_or_else(|| "The staged update is missing".to_owned())
                    .and_then(windows_updater::approve_relaunch);
                return match result {
                    Ok(()) => {
                        self.session.approved();
                        true
                    }
                    Err(error) => {
                        eprintln!("{error}");
                        self.session.approval_failed(error, Instant::now());
                        false
                    }
                };
            }
            Effect::Check { manual } => {
                let manual = *manual;
                spawn(move || Completion::Checked(windows_updater::check(manual)))
            }
            Effect::Stage(update) => {
                let update = update.clone();
                let progress = self.progress.clone();
                spawn(move || {
                    Completion::Staged(windows_updater::stage(&update, |event| {
                        *progress
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(event);
                    }))
                })
            }
            Effect::PrepareRestart => {
                if let Some(mut staged) = self.staged.take() {
                    spawn(move || {
                        let result = windows_updater::relaunch(&mut staged);
                        Completion::Relaunched(staged, result)
                    })
                } else {
                    Err("The staged update is missing".into())
                }
            }
        };
        match result {
            Ok(worker) => self.worker = Some(worker),
            Err(error) => {
                eprintln!("{error}");
                self.session.start_failed(effect, error, Instant::now());
            }
        }
        false
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
                let view = self
                    .state
                    .borrow()
                    .updates
                    .session
                    .view(env!("CARGO_PKG_VERSION"));
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
                && let Err(error) = tray.set_update_installed(app.updates.session.has_status())
            {
                eprintln!("Could not update the tray command: {error}");
            }
            quit
        };
        self.refresh_updates();
        quit
    }

    pub(super) fn refresh_updates(&self) {
        let (view, theme) = {
            let app = self.state.borrow();
            (
                app.updates.session.view(env!("CARGO_PKG_VERSION")),
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
