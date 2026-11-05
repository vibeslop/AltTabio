//! Switcher actions and what they do: switching to the selection and running commands on it.

use super::App;
use crate::macos::commands::{self, AppRef};
use crate::macos::runtime::post_to_app;
use crate::macos::tracing;
use crate::macos::window_list::WindowRecord;
use alttabio::app_switcher::{Action, Effect, Target};
use alttabio::process_identity::ProcessIdentity;
use alttabio::window_command::WindowCommand;

/// An owned snapshot, so remote calls never retain an application-state borrow.
enum CommandTarget {
    Window(WindowRecord),
    App {
        process: ProcessIdentity,
        name: String,
    },
}

impl CommandTarget {
    fn run(&self, command: Option<WindowCommand>) -> Result<(), String> {
        match (self, command) {
            (Self::Window(record), None) => commands::activate(record),
            (Self::Window(record), Some(command)) => commands::execute_on_window(command, record),
            (Self::App { process, name }, command) => {
                let app = AppRef {
                    process: *process,
                    name,
                };
                match command {
                    Some(command) => commands::execute_on_app(command, &app),
                    None => commands::activate_app(&app),
                }
            }
        }
    }
}

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
        let showed_preview = self.dismiss_overlay();
        if let Err(error) = self.queue_command(target, None, showed_preview) {
            eprintln!("{error}");
            self.switch_finished(showed_preview);
        }
    }

    fn execute_command(&mut self, command: WindowCommand, target: Target) {
        if let Err(error) = self.queue_command(target, Some(command), false) {
            eprintln!("{error}");
        }
    }

    fn queue_command(
        &self,
        target: Target,
        command: Option<WindowCommand>,
        showed_preview: bool,
    ) -> Result<(), String> {
        let target = match target {
            Target::Window { handle, process } => CommandTarget::Window(
                self.record(handle)
                    .filter(|record| {
                        u32::try_from(record.pid).ok() == Some(process.id)
                            && record.launched_at == process.started_at
                    })
                    .ok_or_else(|| "The selected window is no longer listed".to_owned())?
                    .clone(),
            ),
            Target::App(process) => CommandTarget::App {
                process,
                name: self.app_name(process),
            },
        };
        // AX client calls and NSRunningApplication may run off the main thread. Serial dispatch
        // keeps user commands ordered, independently of enumeration and settings persistence.
        // https://developer.apple.com/documentation/appkit/nsrunningapplication
        let session = self.session;
        self.command_queue.exec_async(move || {
            let result = objc2::rc::autoreleasepool(|_| target.run(command));
            post_to_app(move |app| {
                match result {
                    Err(error) => eprintln!("{error}"),
                    Ok(()) if command.is_some() => {
                        // Refresh only after the remote action has completed.
                        app.request_refresh();
                        app.schedule_refresh_burst();
                    }
                    Ok(()) => {}
                }
                if command.is_none() && app.session == session {
                    app.switch_finished(showed_preview);
                }
            });
        });
        Ok(())
    }
}
