use super::App;
use crate::app_messages::SHELL_DISMISS_TIMER_ID;
use crate::shell_menu;
use alttabio::deferred_switch::{DeferredSwitch, DeferredSwitchPoll};
use alttabio::input::InputAction;
use windows::Win32::Foundation::{HWND, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
use windows::core::Error;

#[cfg(test)]
#[path = "wheel_tests.rs"]
mod wheel_tests;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_io::SettingsStore;
    use crate::win32::{ComApartment, module_instance};
    use alttabio::settings::Settings;
    use alttabio::switcher::{SwitchTask, SwitcherEffect};
    use std::time::{Duration, Instant};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WS_EX_TOOLWINDOW, WS_POPUP,
    };
    use windows::core::{Result, w};

    struct Owner(HWND);

    impl Drop for Owner {
        fn drop(&mut self) {
            // SAFETY: the test uniquely owns this hidden window on its creating thread.
            assert!(unsafe { DestroyWindow(self.0) }.is_ok());
        }
    }

    #[test]
    fn two_mouse_gestures_complete_in_order_after_shell_dismissal() -> Result<()> {
        assert_deferred_mouse_completion(
            &[
                InputAction::MouseWheelReleased,
                InputAction::RightButtonPressed,
                InputAction::MouseWheel(2, (0, 0)),
                InputAction::MouseWheelReleased,
            ],
            1,
            &[20, 30],
        )
    }

    #[test]
    fn ordinary_wheel_cannot_overtake_a_deferred_mouse_release() -> Result<()> {
        assert_deferred_mouse_completion(
            &[
                InputAction::MouseWheelReleased,
                InputAction::OverlayWheel(-120, (0, 0)),
            ],
            0,
            &[20],
        )
    }

    #[test]
    fn queued_tab_after_mouse_completion_does_not_reopen_the_panel() -> Result<()> {
        assert_deferred_mouse_completion(
            &[
                InputAction::MouseWheelReleased,
                InputAction::CycleSelection(1),
            ],
            0,
            &[20],
        )
    }

    fn assert_deferred_mouse_completion(
        later: &[InputAction],
        pending_openings: usize,
        expected_activations: &[isize],
    ) -> Result<()> {
        let _apartment = ComApartment::initialize()?;
        let owner = Owner(unsafe {
            // SAFETY: STATIC is a system class; the test owns this hidden, off-screen popup.
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("STATIC"),
                w!("Deferred mouse test"),
                WS_POPUP,
                -32000,
                -32000,
                900,
                600,
                None,
                None,
                Some(module_instance()?),
                None,
            )
        }?);
        let settings = Settings::default();
        let (store, settings) = SettingsStore::load_from(
            std::env::temp_dir().join(format!(
                "alttabio-deferred-mouse-{}.ini",
                std::process::id()
            )),
            &settings,
        )
        .map_err(|error| {
            Error::new(windows::core::HRESULT(0x8000_4005_u32.cast_signed()), error)
        })?;
        let mut app = App::new(false, false, settings, store)?;
        app.hwnd = owner.0;
        let tasks = [
            SwitchTask::new(1, 10, "One", "one"),
            SwitchTask::new(2, 20, "Two", "two"),
            SwitchTask::new(3, 30, "Three", "three"),
        ];
        // A's opening has already been previewed, while shell focus remains pending.
        let _opening = app.session.handle_input(InputAction::MouseWheel(1, (0, 0)));
        app.session.open(tasks.clone(), Some(1));
        let mut input = DeferredSwitch::new(InputAction::MouseWheel(1, (0, 0)));
        assert_eq!(
            input.take_preview_actions(),
            [InputAction::MouseWheel(1, (0, 0))]
        );
        app.pending_shell = Some(PendingShellDismissal {
            window: owner.0,
            origin: WPARAM(0),
            started: Instant::now(),
            input,
        });
        for &action in later {
            app.handle_hook_input(action, WPARAM(0));
        }
        assert_eq!(
            app.session
                .switcher()
                .selected_task()
                .map(|task| task.window_handle),
            Some(20)
        );
        let Some(mut pending) = app.pending_shell.take() else {
            panic!("shell is still dismissing");
        };
        assert_eq!(pending.input.pending_openings(), pending_openings);
        let DeferredSwitchPoll::Ready(actions) = pending.input.poll(false, Duration::ZERO) else {
            panic!("input must resume when the shell releases focus");
        };
        assert_eq!(actions, later);
        let mut activations = Vec::new();
        for action in actions {
            match app.session.handle_input(action) {
                SwitcherEffect::Open { selection_delta } => {
                    app.session.open(tasks.clone(), selection_delta);
                }
                SwitcherEffect::Activate(target) => activations.push(target),
                SwitcherEffect::None | SwitcherEffect::Redraw => {}
                effect => panic!("unexpected queued effect: {effect:?}"),
            }
        }
        assert_eq!(activations, expected_activations);
        assert!(!app.session.is_visible());
        Ok(())
    }
}
