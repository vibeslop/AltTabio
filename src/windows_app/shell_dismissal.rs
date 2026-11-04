use super::App;
use crate::app_messages::SHELL_DISMISS_TIMER_ID;
use crate::shell_menu;
use alttabio::deferred_switch::{DeferredSwitch, DeferredSwitchPoll};
use alttabio::input::InputAction;
use windows::Win32::Foundation::{HWND, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
use windows::core::Error;

pub(super) struct PendingShellDismissal {
    window: HWND,
    origin: WPARAM,
    started: std::time::Instant,
    input: DeferredSwitch,
}

impl App {
    pub(super) fn begin_shell_dismissal(
        &mut self,
        window: HWND,
        first: InputAction,
        origin: WPARAM,
    ) {
        // SAFETY: this timer belongs to the live overlay; no callback pointer is retained.
        if unsafe { SetTimer(Some(self.hwnd), SHELL_DISMISS_TIMER_ID, 16, None) } == 0 {
            self.retire_hook_opening(first, origin);
            eprintln!(
                "Could not start the shell dismissal timer: {}",
                Error::from_thread()
            );
            self.hide_overlay();
            return;
        }
        self.pending_shell = Some(PendingShellDismissal {
            window,
            origin,
            started: std::time::Instant::now(),
            input: DeferredSwitch::new(first),
        });
        if let Err(error) = shell_menu::dismiss(window) {
            eprintln!("{error}");
            self.hide_overlay();
            return;
        }
        self.preview_shell_switch();
    }

    /// Returns whether a pending shell dismissal took `action`.
    pub(super) fn defer_to_shell_dismissal(&mut self, action: InputAction) -> bool {
        let Some(pending) = &mut self.pending_shell else {
            return false;
        };
        let openings = pending.input.pending_openings();
        let origin = pending.origin;
        if pending.input.push(action) {
            self.preview_shell_switch();
        } else {
            if let Some(hooks) = &self.hooks {
                hooks.retire_openings(origin, openings);
            }
            self.retire_hook_opening(action, origin);
            self.hide_overlay();
        }
        true
    }

    fn preview_shell_switch(&mut self) {
        let Some(pending) = &mut self.pending_shell else {
            return;
        };
        let origin = pending.origin;
        for action in pending.input.take_preview_actions() {
            self.apply_hook_action(action, origin);
        }
    }

    pub(super) fn stop_shell_dismissal(&mut self) {
        let Some(pending) = self.pending_shell.take() else {
            return;
        };
        if let Some(hooks) = &self.hooks {
            hooks.retire_openings(pending.origin, pending.input.pending_openings());
        }
        self.kill_shell_dismissal_timer();
    }

    fn kill_shell_dismissal_timer(&self) {
        // SAFETY: this balances the timer started for the live overlay's pending opening.
        if let Err(error) = unsafe { KillTimer(Some(self.hwnd), SHELL_DISMISS_TIMER_ID) } {
            eprintln!("Could not stop the shell dismissal timer: {error}");
        }
    }

    pub(super) fn handle_shell_dismissal(&mut self) {
        let Some(pending) = &mut self.pending_shell else {
            return;
        };
        if !self
            .hooks
            .as_ref()
            .is_some_and(|hooks| hooks.action_is_current(pending.origin))
        {
            self.hide_overlay();
            return;
        }
        let shell_has_focus =
            if let Some(window) = shell_menu::remaining_foreground_menu(pending.window) {
                pending.window = window;
                true
            } else {
                false
            };
        let openings = pending.input.pending_openings();
        let origin = pending.origin;
        let poll = pending
            .input
            .poll(shell_has_focus, pending.started.elapsed());
        match poll {
            DeferredSwitchPoll::Wait => {}
            DeferredSwitchPoll::RetryDismissal => {
                if let Err(error) = shell_menu::dismiss(pending.window) {
                    eprintln!("{error}");
                    self.hide_overlay();
                }
            }
            DeferredSwitchPoll::Cancel => {
                if let Some(hooks) = &self.hooks {
                    hooks.retire_openings(origin, openings);
                }
                eprintln!("Start/Search did not relinquish foreground within the switch deadline");
                self.hide_overlay();
            }
            DeferredSwitchPoll::Ready(actions) => {
                self.stop_shell_dismissal();
                if self.is_visible() {
                    self.focus_overlay();
                }
                for action in actions {
                    self.apply_hook_action(action, origin);
                }
            }
        }
    }
}
