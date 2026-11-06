//! Provenance policy for the Windows low-level input boundary.

use super::{HookOutcome, HookSettings, HookState, KeyEvent, MouseEvent};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputProvenance {
    Physical,
    InternalReplay,
    Injected,
}

impl InputProvenance {
    /// The replay marker is a routing hint, never authority across an integrity boundary.
    #[must_use]
    pub const fn classify(injected: bool, lower_integrity: bool, replay_marker: bool) -> Self {
        if lower_integrity {
            Self::Injected
        } else if replay_marker {
            Self::InternalReplay
        } else if injected {
            Self::Injected
        } else {
            Self::Physical
        }
    }

    /// Also prevent delivery to native overlay/menu handlers while `AltTabio` owns input.
    #[must_use]
    pub const fn suppress_native_delivery(self, owns_input: bool) -> bool {
        matches!(self, Self::Injected) && owns_input
    }
}

impl HookState {
    /// Only physical observations may authorize switching or consume gesture release debt.
    /// Windows' Alt context and async modifier state can include rejected injected presses.
    #[must_use]
    pub fn process_key_from(
        &mut self,
        provenance: InputProvenance,
        mut event: KeyEvent,
        settings: HookSettings,
    ) -> Option<HookOutcome> {
        if provenance != InputProvenance::Physical {
            return None;
        }
        event.modifiers.alt = self.any_alt_key_down();
        event.modifiers.left_windows = self.owned_key_is_down(super::Key::LeftWindows);
        event.modifiers.right_windows = self.owned_key_is_down(super::Key::RightWindows);
        Some(self.process_key(event, settings))
    }

    #[must_use]
    pub fn process_mouse_from(
        &mut self,
        provenance: InputProvenance,
        event: MouseEvent,
        settings: HookSettings,
    ) -> Option<HookOutcome> {
        if provenance != InputProvenance::Physical {
            return None;
        }
        Some(self.process_mouse(event, settings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{InputAction, Key, Modifiers};
    use crate::window_command::WindowCommand;

    const PHYSICAL: InputProvenance = InputProvenance::Physical;
    const INJECTED: InputProvenance = InputProvenance::Injected;
    const REPLAY: InputProvenance = InputProvenance::InternalReplay;

    fn key(state: &mut HookState, source: InputProvenance, event: KeyEvent) -> Option<HookOutcome> {
        state.process_key_from(source, event, HookSettings::default())
    }

    #[test]
    fn lower_integrity_cannot_authenticate_with_the_replay_marker() {
        for injected in [false, true] {
            for marker in [false, true] {
                assert_eq!(InputProvenance::classify(injected, true, marker), INJECTED);
            }
            assert_eq!(InputProvenance::classify(injected, false, true), REPLAY);
        }
        assert_eq!(InputProvenance::classify(false, false, false), PHYSICAL);
        assert_eq!(InputProvenance::classify(true, false, false), INJECTED);
        assert!(INJECTED.suppress_native_delivery(true));
        assert!(!INJECTED.suppress_native_delivery(false));
        assert!(!REPLAY.suppress_native_delivery(true));
        assert!(!PHYSICAL.suppress_native_delivery(true));
    }

    #[test]
    fn injected_modifiers_and_system_context_cannot_arm_a_physical_tab() {
        for modifier in [
            Key::LeftAlt,
            Key::RightAlt,
            Key::LeftWindows,
            Key::RightWindows,
        ] {
            let mut state = HookState::default();
            assert_eq!(
                key(
                    &mut state,
                    INJECTED,
                    KeyEvent::pressed(modifier, Modifiers::default())
                ),
                None
            );
            // Even an unflagged Tab's OS modifier context can reflect a synthetic press.
            let polluted = Modifiers {
                alt: true,
                left_windows: true,
                right_windows: true,
                ..Modifiers::default()
            };
            let tab = key(&mut state, PHYSICAL, KeyEvent::pressed(Key::Tab, polluted));
            assert_eq!(tab, Some(HookOutcome::default()));
            assert_eq!(
                key(
                    &mut state,
                    PHYSICAL,
                    KeyEvent::pressed(Key::Function(8), polluted)
                ),
                Some(HookOutcome::default())
            );
            assert_eq!(state.take_replayed_key_events(), [None; 3]);
        }
    }

    #[test]
    fn injected_commands_and_releases_cannot_change_a_physical_gesture() {
        for modifier in [Key::LeftAlt, Key::LeftWindows] {
            let mut state = HookState::default();
            let plain = Modifiers::default();
            let _press = key(&mut state, PHYSICAL, KeyEvent::pressed(modifier, plain));
            assert_eq!(
                key(&mut state, PHYSICAL, KeyEvent::pressed(Key::Tab, plain))
                    .map(|o| o.actions().collect::<Vec<_>>()),
                Some(vec![InputAction::Switch(1)])
            );
            state.set_overlay_active(true);
            for source in [INJECTED, REPLAY] {
                for event in [
                    KeyEvent::pressed(Key::Function(8), plain),
                    KeyEvent::released(Key::Tab, plain),
                    KeyEvent::released(modifier, plain),
                ] {
                    assert_eq!(key(&mut state, source, event), None);
                }
            }
            assert_eq!(
                key(
                    &mut state,
                    PHYSICAL,
                    KeyEvent::pressed(Key::Function(8), plain)
                )
                .map(|o| o.actions().collect::<Vec<_>>()),
                Some(vec![InputAction::WindowCommand(WindowCommand::Terminate)])
            );
            let tab_up = key(&mut state, PHYSICAL, KeyEvent::released(Key::Tab, plain));
            assert!(tab_up.is_some_and(|o| o.suppress));
            let release = key(&mut state, PHYSICAL, KeyEvent::released(modifier, plain));
            assert!(release.is_some_and(|o| o.suppress));
            if modifier == Key::LeftAlt {
                assert_eq!(
                    release.map(|o| o.actions().collect::<Vec<_>>()),
                    Some(vec![InputAction::AltReleased])
                );
            }
        }
    }

    #[test]
    fn injected_mouse_events_cannot_arm_or_complete_a_physical_scroll() {
        let settings = HookSettings {
            right_button_wheel_switching: true,
            ..HookSettings::default()
        };
        let mut state = HookState::default();
        for source in [INJECTED, REPLAY] {
            assert_eq!(
                state.process_mouse_from(source, MouseEvent::RightButtonPressed, settings),
                None
            );
        }
        assert_eq!(
            state.process_mouse_from(PHYSICAL, MouseEvent::Wheel(-120), settings),
            Some(HookOutcome::default())
        );
        let _down = state.process_mouse_from(PHYSICAL, MouseEvent::RightButtonPressed, settings);
        for source in [INJECTED, REPLAY] {
            for event in [
                MouseEvent::LeftButtonPressed,
                MouseEvent::Wheel(-120),
                MouseEvent::RightButtonReleased,
            ] {
                assert_eq!(state.process_mouse_from(source, event, settings), None);
            }
        }
        assert!(state.owns_right_button());
        assert!(!state.take_replayed_right_click());
        assert_eq!(
            state
                .process_mouse_from(PHYSICAL, MouseEvent::Wheel(-120), settings)
                .map(|o| o.actions().collect::<Vec<_>>()),
            Some(vec![InputAction::MouseWheel(1)])
        );
        assert_eq!(
            state.process_mouse_from(INJECTED, MouseEvent::RightButtonReleased, settings),
            None
        );
        assert_eq!(
            state
                .process_mouse_from(PHYSICAL, MouseEvent::RightButtonReleased, settings)
                .map(|o| o.actions().collect::<Vec<_>>()),
            Some(vec![InputAction::MouseWheelReleased])
        );
        assert!(!state.owns_right_button());
    }

    #[test]
    fn rebased_modifiers_still_pass_until_their_physical_release() {
        let mut state = HookState::default();
        state.rebase_modifier(Key::LeftAlt, true);
        assert_eq!(
            key(
                &mut state,
                PHYSICAL,
                KeyEvent::pressed(Key::Tab, Modifiers::default())
            ),
            Some(HookOutcome::default())
        );
        assert_eq!(
            key(
                &mut state,
                INJECTED,
                KeyEvent::released(Key::LeftAlt, Modifiers::default())
            ),
            None
        );
        let _up = key(
            &mut state,
            PHYSICAL,
            KeyEvent::released(Key::LeftAlt, Modifiers::default()),
        );
        let _down = key(
            &mut state,
            PHYSICAL,
            KeyEvent::pressed(Key::LeftAlt, Modifiers::default()),
        );
        assert_eq!(
            key(
                &mut state,
                PHYSICAL,
                KeyEvent::pressed(Key::Tab, Modifiers::default())
            )
            .map(|o| o.actions().next()),
            Some(Some(InputAction::Switch(1)))
        );
    }
}
