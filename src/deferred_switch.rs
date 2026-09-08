//! Retain a switch gesture while the shell dismisses its foreground menu.

use crate::input::InputAction;
use std::time::Duration;

const MAX_ACTIONS: usize = 32;
const RETRY_AFTER: Duration = Duration::from_millis(150);
const TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, PartialEq)]
pub enum DeferredSwitchPoll {
    Wait,
    RetryDismissal,
    Ready(Vec<InputAction>),
    Cancel,
}

pub struct DeferredSwitch {
    actions: Vec<InputAction>,
    retried: bool,
    cancelled: bool,
}

impl DeferredSwitch {
    #[must_use]
    pub fn new(first: InputAction) -> Self {
        Self {
            actions: vec![first],
            retried: false,
            cancelled: false,
        }
    }

    /// False cancels the pending opening, including a Windows-key or Escape dismissal.
    #[must_use]
    pub fn push(&mut self, action: InputAction) -> bool {
        if self.cancelled
            || action == InputAction::DismissOverlay
            || self.actions.len() == MAX_ACTIONS
        {
            self.actions.clear();
            self.cancelled = true;
            return false;
        }
        self.actions.push(action);
        true
    }

    /// Display and navigate immediately, but preserve ordering once an action needs focus.
    pub fn take_preview_actions(&mut self) -> Vec<InputAction> {
        let count = self
            .actions
            .iter()
            .take_while(|action| {
                matches!(
                    action,
                    InputAction::Switch(_)
                        | InputAction::Navigate(_)
                        | InputAction::SelectFirst
                        | InputAction::SelectLast
                        | InputAction::MouseWheel(_)
                        | InputAction::AppendSearchCharacter(_)
                        | InputAction::BackspaceSearch
                )
            })
            .count();
        self.actions.drain(..count).collect()
    }

    #[must_use]
    pub fn poll(&mut self, shell_has_focus: bool, elapsed: Duration) -> DeferredSwitchPoll {
        if self.cancelled || elapsed >= TIMEOUT {
            self.actions.clear();
            self.cancelled = true;
            DeferredSwitchPoll::Cancel
        } else if !shell_has_focus {
            DeferredSwitchPoll::Ready(core::mem::take(&mut self.actions))
        } else if !self.retried && elapsed >= RETRY_AFTER {
            self.retried = true;
            DeferredSwitchPoll::RetryDismissal
        } else {
            DeferredSwitchPoll::Wait
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::switcher::{SwitchTask, SwitcherEffect, SwitcherSession, SwitcherSessionSettings};

    #[test]
    fn preview_cycles_before_shell_releases_focus_but_activation_waits() {
        let mut deferred = DeferredSwitch::new(InputAction::Switch(1));
        let mut session = SwitcherSession::new(SwitcherSessionSettings {
            typed_search: true,
            release_alt_switches: true,
            release_right_button_switches: true,
        });
        for action in deferred.take_preview_actions() {
            let SwitcherEffect::Open { selection_delta } = session.handle_input(action) else {
                panic!("the first Tab must open before shell dismissal finishes");
            };
            session.open(
                [
                    SwitchTask::new(1, 10, "First", "first"),
                    SwitchTask::new(2, 20, "Second", "second"),
                    SwitchTask::new(3, 30, "Third", "third"),
                ],
                selection_delta,
            );
        }
        assert!(session.is_visible());
        assert!(deferred.push(InputAction::Switch(-1)));
        for action in deferred.take_preview_actions() {
            assert_eq!(session.handle_input(action), SwitcherEffect::Redraw);
        }
        assert_eq!(
            session
                .switcher()
                .selected_task()
                .map(|task| task.window_handle),
            Some(10)
        );
        assert_eq!(
            deferred.poll(true, Duration::ZERO),
            DeferredSwitchPoll::Wait
        );
        assert!(deferred.push(InputAction::AltReleased));
        assert!(deferred.take_preview_actions().is_empty());
        assert!(session.is_visible());
        let DeferredSwitchPoll::Ready(actions) = deferred.poll(false, Duration::from_millis(20))
        else {
            panic!("activation must resume when shell focus is released");
        };
        assert_eq!(actions, [InputAction::AltReleased]);
        assert_eq!(
            session.handle_input(actions[0]),
            SwitcherEffect::Activate(10)
        );
    }

    #[test]
    fn preview_preserves_order_after_release_and_cancels_without_replay() {
        let mut deferred = DeferredSwitch::new(InputAction::Switch(1));
        assert!(deferred.push(InputAction::AltReleased));
        assert!(deferred.push(InputAction::Switch(-1)));
        assert_eq!(deferred.take_preview_actions(), [InputAction::Switch(1)]);
        assert!(deferred.take_preview_actions().is_empty());
        assert!(!deferred.push(InputAction::DismissOverlay));
        assert!(deferred.take_preview_actions().is_empty());
        assert_eq!(
            deferred.poll(false, Duration::ZERO),
            DeferredSwitchPoll::Cancel
        );
    }

    #[test]
    fn quick_release_and_reverse_cycles_survive_shell_dismissal() {
        let mut deferred = DeferredSwitch::new(InputAction::Switch(1));
        assert!(deferred.push(InputAction::Switch(1)));
        assert!(deferred.push(InputAction::Switch(-1)));
        assert!(deferred.push(InputAction::AltReleased));
        assert_eq!(
            deferred.poll(true, Duration::ZERO),
            DeferredSwitchPoll::Wait
        );
        let DeferredSwitchPoll::Ready(actions) = deferred.poll(false, Duration::from_millis(20))
        else {
            panic!("the shell gave up focus");
        };
        let mut session = SwitcherSession::new(SwitcherSessionSettings {
            typed_search: true,
            release_alt_switches: true,
            release_right_button_switches: true,
        });
        let mut activation = None;
        for action in actions {
            match session.handle_input(action) {
                SwitcherEffect::Open { selection_delta } => session.open(
                    [
                        SwitchTask::new(1, 10, "First", "first"),
                        SwitchTask::new(2, 20, "Second", "second"),
                        SwitchTask::new(3, 30, "Third", "third"),
                    ],
                    selection_delta,
                ),
                SwitcherEffect::Activate(target) => activation = Some(target),
                SwitcherEffect::Redraw => {}
                effect => panic!("unexpected effect: {effect:?}"),
            }
        }
        assert_eq!(activation, Some(20));
        assert!(!session.is_visible());
    }

    #[test]
    fn cancellation_never_reopens_a_queued_gesture() {
        let mut deferred = DeferredSwitch::new(InputAction::Switch(1));
        assert!(!deferred.push(InputAction::DismissOverlay));
        assert_eq!(
            deferred.poll(false, Duration::ZERO),
            DeferredSwitchPoll::Cancel
        );
    }

    #[test]
    fn shell_timeout_and_input_backlog_are_bounded() {
        let mut deferred = DeferredSwitch::new(InputAction::Switch(1));
        assert_eq!(
            deferred.poll(true, RETRY_AFTER),
            DeferredSwitchPoll::RetryDismissal
        );
        assert_eq!(deferred.poll(true, RETRY_AFTER), DeferredSwitchPoll::Wait);
        assert_eq!(deferred.poll(true, TIMEOUT), DeferredSwitchPoll::Cancel);
        assert_eq!(deferred.poll(false, TIMEOUT), DeferredSwitchPoll::Cancel);
        let mut deferred = DeferredSwitch::new(InputAction::Switch(1));
        for _ in 1..MAX_ACTIONS {
            assert!(deferred.push(InputAction::Switch(1)));
        }
        assert!(!deferred.push(InputAction::AltReleased));
        assert_eq!(
            deferred.poll(false, Duration::ZERO),
            DeferredSwitchPoll::Cancel
        );
    }
}
