//! Switcher actions and what they do: switching to the selection and running commands on it.

use super::App;
use crate::macos::commands::{self, AppRef};
use crate::macos::tracing;
use crate::macos::window_list::WindowRecord;
use alttabio::app_switcher::{Action, Effect, Target};
use alttabio::input::WindowCommand;

impl App {
    pub(super) fn apply_action(&mut self, action: Action) {
        let before = self.switcher.selected_target();
        let effect = self.switcher.handle(action);
        if tracing() {
            eprintln!("action {action:?} -> {effect:?}");
        }
        self.apply_effect(effect, before);
    }

    pub(super) fn apply_effect(&mut self, effect: Effect, before: Option<Target>) {
        match effect {
            Effect::None => {}
            Effect::Open { step } => self.show_overlay(Some(step)),
            Effect::Hide => self.hide_overlay(),
            Effect::Redraw => self.selection_changed(before),
            Effect::Activate(target) => self.activate_target(target),
            Effect::Execute { command, target } => self.execute_command(command, target),
        }
    }

    pub(super) fn selection_changed(&mut self, before: Option<Target>) {
        self.redraw();
        if self.switcher.selected_target() != before {
            self.request_preview_capture();
        }
    }

    fn activate_target(&mut self, target: Target) {
        if let Target::Window { handle, .. } = target {
            // The switch is the window's latest use; the refresh the activation brings may run
            // before the app has raised it.
            let listed = self.listed_windows();
            self.window_history.note(Some(handle), &listed);
        }
        if let Err(error) = self.act_on(target, commands::activate, commands::activate_app) {
            eprintln!("{error}");
        }
        self.hide_overlay();
    }

    fn execute_command(&mut self, command: WindowCommand, target: Target) {
        let result = self.act_on(
            target,
            |record| commands::execute_on_window(command, record),
            |app| commands::execute_on_app(command, app),
        );
        if let Err(error) = result {
            eprintln!("{error}");
            return;
        }
        // The switcher stays open and keeps its selection; the refreshes show the window or
        // app leaving the list in place.
        self.request_refresh();
        Self::schedule_refresh_burst();
    }

    /// Runs `on_window` on the target window's record, or `on_app` on the target app.
    fn act_on(
        &self,
        target: Target,
        on_window: impl FnOnce(&WindowRecord) -> Result<(), String>,
        on_app: impl FnOnce(&AppRef<'_>) -> Result<(), String>,
    ) -> Result<(), String> {
        match target {
            Target::Window { handle, .. } => self.record(handle).map_or_else(
                || Err("The selected window is no longer listed".to_owned()),
                on_window,
            ),
            Target::App(process) => on_app(&AppRef {
                process,
                name: &self.app_name(process),
            }),
        }
    }
}
