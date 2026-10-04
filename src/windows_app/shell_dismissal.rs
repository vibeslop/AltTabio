use super::App;
use crate::app_messages::SHELL_DISMISS_TIMER_ID;
use crate::shell_menu;
use alttabio::deferred_switch::{DeferredSwitch, DeferredSwitchPoll, SwitchResume};
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

    pub(super) fn resume_shell_switch_with_hotkey(&mut self, action: InputAction) {
        // The actual Tab was delivered as a registered hotkey. Taking focus now dismisses the
        // shell naturally, without injecting Escape first.
        let pending = self.pending_shell.take();
        if pending.is_some() {
            self.kill_shell_dismissal_timer();
        }
        let pending = pending.filter(|pending| {
            let current = self
                .hooks
                .as_ref()
                .is_some_and(|hooks| hooks.action_is_current(pending.origin));
            if !current {
                // A new physical gesture must not replay actions from before a desktop or modal
                // boundary, or inherit its selection.
                self.session.hide();
            }
            current
        });
        DeferredSwitch::resume_with_hotkey(pending.map(|pending| pending.input), action, |step| {
            match step {
                SwitchResume::FocusOverlay => self.focus_overlay(),
                SwitchResume::Replay(action) => self.apply_input_action_with_reset(action, false),
                SwitchResume::Input(action) => self.apply_input_action(action),
            }
        });
    }

    /// Returns whether a pending shell dismissal took `action`.
    pub(super) fn defer_to_shell_dismissal(&mut self, action: InputAction) -> bool {
        let Some(pending) = &mut self.pending_shell else {
            return false;
        };
        if pending.input.push(action) {
            self.preview_shell_switch();
        } else {
            self.hide_overlay();
        }
        true
    }

    fn preview_shell_switch(&mut self) {
        let Some(pending) = &mut self.pending_shell else {
            return;
        };
        for action in pending.input.take_preview_actions() {
            self.apply_input_action(action);
        }
    }

    pub(super) fn stop_shell_dismissal(&mut self) {
        if self.pending_shell.take().is_none() {
            return;
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
                eprintln!("Start/Search did not relinquish foreground within the switch deadline");
                self.hide_overlay();
            }
            DeferredSwitchPoll::Ready(actions) => {
                self.stop_shell_dismissal();
                if self.is_visible() {
                    self.focus_overlay();
                }
                for action in actions {
                    self.handle_input_action(action);
                }
            }
        }
    }
}
