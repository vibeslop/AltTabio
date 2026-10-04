//! When to look for updates, and the relaunch into an installed one.

use super::App;
use crate::macos::alerts::{offer_update, show_update_report};
use crate::macos::runtime::{run_later, schedule, with_app};
use crate::macos::settings_window::SettingsWindow;
use crate::macos::updater;
use alttabio::update::{Checked, Step, Update};
use objc2_app_kit::NSApplication;

// The first automatic update check waits a minute after the start, so a login does not ask
// GitHub before the network is up. A failed check tries again in an hour, and any other outcome
// waits a day.
pub(super) const FIRST_UPDATE_CHECK_SECONDS: f64 = 60.0;
const UPDATE_RETRY_SECONDS: f64 = 60.0 * 60.0;
const UPDATE_INTERVAL_SECONDS: f64 = 24.0 * 60.0 * 60.0;
// An automatic update relaunches once the keyboard and pointer have rested this long, so the
// switcher is never gone at the moment it is wanted.
const AWAY_SECONDS: f64 = 5.0 * 60.0;
const AWAY_POLL_SECONDS: f64 = 60.0;
// A relaunch never takes the switcher away while it is open; it tries again this much later.
const RELAUNCH_RETRY_SECONDS: f64 = 1.0;

impl App {
    pub(super) fn schedule_update_check(&mut self, seconds: f64) {
        if let Some(timer) = self.update_timer.take() {
            timer.invalidate();
        }
        self.update_timer = Some(schedule(self.mtm, seconds, App::update_check_due));
    }

    pub(super) fn update_check_due(&mut self) {
        self.update_timer = None;
        let step = self.updater.check_due();
        // A check runs only when nothing else is under way. Whatever is keeps automatic updates
        // going by the next day: a check reschedules when it ends, and an install that fails
        // leaves this timer behind.
        if step == Step::None && self.settings.general.auto_update {
            self.schedule_update_check(UPDATE_INTERVAL_SECONDS);
        }
        self.apply_update_step(step);
    }

    pub(super) fn update_requested(&mut self) {
        let step = self.updater.requested();
        self.apply_update_step(step);
    }

    pub(crate) fn update_checked(&mut self, result: Result<Checked, String>) {
        match &result {
            Err(error) => eprintln!("{error}"),
            Ok(Checked::Blocked(version, reason)) => {
                eprintln!(
                    "AltTabio {version} is available, but this copy cannot install it. {reason}"
                );
            }
            Ok(_) => {}
        }
        if self.settings.general.auto_update {
            self.schedule_update_check(if result.is_err() {
                UPDATE_RETRY_SECONDS
            } else {
                UPDATE_INTERVAL_SECONDS
            });
        }
        let step = self.updater.checked(result);
        self.apply_update_step(step);
    }

    fn update_offer_answered(&mut self, update: Update, install: bool) {
        let step = self.updater.offer_answered(update, install);
        self.apply_update_step(step);
    }

    pub(crate) fn update_installed(&mut self, result: Result<(), String>) {
        if let Err(error) = &result {
            eprintln!("{error}");
        }
        let step = self.updater.installed(result);
        self.apply_update_step(step);
    }

    fn apply_update_step(&mut self, step: Step) {
        match step {
            Step::None => {}
            Step::Check => updater::check(updater::bundle_path()),
            Step::Install(update) => updater::install(update, updater::bundle_path()),
            Step::Offer(update) => {
                let mtm = self.mtm;
                run_later(mtm, move || {
                    let install = offer_update(mtm, update.version);
                    let _ = with_app(|app| app.update_offer_answered(update, install));
                });
            }
            Step::Report(report) => {
                let mtm = self.mtm;
                run_later(mtm, move || show_update_report(mtm, &report));
            }
            // Timers wait out an open menu or alert, so the app never quits from inside one.
            Step::Relaunch => run_later(self.mtm, || {
                let _ = with_app(App::relaunch);
            }),
        }
        let installed = self.updater.installed_version().is_some();
        if let Some(status_item) = &self.status_item {
            status_item.set_update_installed(installed);
        }
        if installed && self.away_timer.is_none() {
            self.away_timer = Some(schedule(self.mtm, AWAY_POLL_SECONDS, App::look_for_away));
        }
    }

    fn look_for_away(&mut self) {
        self.away_timer = None;
        if updater::seconds_since_input() < AWAY_SECONDS {
            self.away_timer = Some(schedule(self.mtm, AWAY_POLL_SECONDS, App::look_for_away));
            return;
        }
        let step = self.updater.user_away();
        self.apply_update_step(step);
    }

    /// Quits and opens the installed update in this copy's place, with the settings window if
    /// it is up.
    fn relaunch(&mut self) {
        if self.switcher.is_active() {
            let _timer = schedule(self.mtm, RELAUNCH_RETRY_SECONDS, App::relaunch);
            return;
        }
        let open_settings = self
            .settings_window
            .as_ref()
            .is_some_and(SettingsWindow::is_visible);
        if let Err(error) = updater::relaunch_after_exit(&updater::bundle_path(), open_settings) {
            eprintln!("{error}; the update takes effect at the next start");
            return;
        }
        self.shutdown();
        NSApplication::sharedApplication(self.mtm).terminate(None);
    }
}
