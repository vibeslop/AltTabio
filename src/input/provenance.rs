//! Provenance policy for the Windows low-level input boundary.

use super::{
    HookOutcome, HookSettings, HookState, InputAction, Key, KeyEvent, KeyTransition, MouseEvent,
    ReplayedKeyEvent, alt_key_mask, windows_key_mask,
};

const MENU_MASK_KEY: Key = Key::Other(0xE8);

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

/// Isolated modifier and switch-gesture tracking for injected keyboard macros (`Win+Tab` and
/// `Alt+Tab`), kept strictly separate from physical modifier state and privileged overlay commands.
#[derive(Debug, Default)]
pub(super) struct InjectedShortcutState {
    win_keys: u8,
    alt_keys: u8,
    shift_keys: u8,
    suppressed_win_keys: u8,
    suppressed_alt_keys: u8,
    suppressed_shift_keys: u8,
    win_gesture_active: bool,
    alt_gesture_active: bool,
    suppressed_tab_release: bool,
}

impl InjectedShortcutState {
    pub(super) const fn reset_gestures(&mut self) {
        self.win_gesture_active = false;
        self.alt_gesture_active = false;
    }
}

const fn injected_shift_mask(key: Key) -> Option<u8> {
    match key {
        Key::LeftShift => Some(1),
        Key::RightShift => Some(2),
        Key::Other(0x10) => Some(4),
        _ => None,
    }
}

impl HookState {
    /// Physical observations retain full hook authority. Injected input is isolated to
    /// `Win+Tab`/`Alt+Tab` switcher triggers and never mutates physical state or triggers
    /// privileged window commands.
    #[must_use]
    pub fn process_key_from(
        &mut self,
        provenance: InputProvenance,
        mut event: KeyEvent,
        settings: HookSettings,
    ) -> Option<HookOutcome> {
        match provenance {
            InputProvenance::Physical => {
                event.modifiers.alt = self.any_alt_key_down();
                event.modifiers.left_windows = self.owned_key_is_down(Key::LeftWindows);
                event.modifiers.right_windows = self.owned_key_is_down(Key::RightWindows);
                Some(self.process_key(event, settings))
            }
            InputProvenance::Injected => self.process_injected_key(event, settings),
            InputProvenance::InternalReplay => None,
        }
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

    fn physical_switch_busy(&self) -> bool {
        self.switch_gesture_active()
            || self.mouse.is_active()
            || self.pending_alt_keys != 0
            || self.pending_windows_keys != 0
            || self.any_alt_key_down()
            || self.owned_key_is_down(Key::LeftWindows)
            || self.owned_key_is_down(Key::RightWindows)
    }

    fn replay_masked_modifier_release(&mut self, key: Key) {
        self.replayed_key_events = [
            Some(ReplayedKeyEvent::pressed(MENU_MASK_KEY)),
            Some(ReplayedKeyEvent::released(MENU_MASK_KEY)),
            Some(ReplayedKeyEvent::released(key)),
        ];
    }

    fn process_injected_key(
        &mut self,
        event: KeyEvent,
        settings: HookSettings,
    ) -> Option<HookOutcome> {
        let pressed = event.transition == KeyTransition::Pressed;
        if !pressed && event.key == Key::Tab && self.injected.suppressed_tab_release {
            self.injected.suppressed_tab_release = false;
            return Some(HookOutcome::suppressed());
        }
        if let Some(mask) = injected_shift_mask(event.key) {
            return self.process_injected_shift(mask, pressed);
        }
        if let Some(mask) = windows_key_mask(event.key) {
            return self.process_injected_windows(event.key, mask, pressed, settings);
        }
        if let Some(mask) = alt_key_mask(event.key) {
            return self.process_injected_alt(event.key, mask, pressed, settings);
        }
        if pressed && event.key == Key::Tab {
            return self.process_injected_tab(settings);
        }
        None
    }

    fn process_injected_shift(&mut self, mask: u8, pressed: bool) -> Option<HookOutcome> {
        if pressed {
            if self.interception.suspended || self.physical_switch_busy() {
                return None;
            }
            self.injected.shift_keys |= mask;
            if self.overlay_active {
                self.injected.suppressed_shift_keys |= mask;
                return Some(HookOutcome::suppressed());
            }
            return Some(HookOutcome::default());
        }
        let was_down = self.injected.shift_keys & mask != 0;
        self.injected.shift_keys &= !mask;
        if self.injected.suppressed_shift_keys & mask != 0 {
            self.injected.suppressed_shift_keys &= !mask;
            return Some(HookOutcome::suppressed());
        }
        was_down.then(HookOutcome::default)
    }

    fn process_injected_windows(
        &mut self,
        key: Key,
        mask: u8,
        pressed: bool,
        settings: HookSettings,
    ) -> Option<HookOutcome> {
        if pressed {
            if !settings.replace_win_tab
                || self.interception.suspended
                || self.physical_switch_busy()
            {
                return None;
            }
            self.injected.win_keys |= mask;
            if self.overlay_active {
                self.injected.suppressed_win_keys |= mask;
                return Some(HookOutcome::suppressed());
            }
            return Some(HookOutcome::default());
        }
        let was_down = self.injected.win_keys & mask != 0;
        self.injected.win_keys &= !mask;
        let suppressed_down = self.injected.suppressed_win_keys & mask != 0;
        self.injected.suppressed_win_keys &= !mask;
        let had_gesture = self.injected.win_gesture_active;
        if self.injected.win_keys == 0 {
            self.injected.win_gesture_active = false;
        }
        if suppressed_down {
            return Some(HookOutcome::suppressed());
        }
        if had_gesture && was_down {
            self.replay_masked_modifier_release(key);
            return Some(HookOutcome::suppressed());
        }
        was_down.then(HookOutcome::default)
    }

    fn process_injected_alt(
        &mut self,
        key: Key,
        mask: u8,
        pressed: bool,
        settings: HookSettings,
    ) -> Option<HookOutcome> {
        if pressed {
            if !settings.replace_alt_tab
                || self.interception.suspended
                || self.physical_switch_busy()
            {
                return None;
            }
            self.injected.alt_keys |= mask;
            if self.overlay_active {
                self.injected.suppressed_alt_keys |= mask;
                return Some(HookOutcome::suppressed());
            }
            return Some(HookOutcome::default());
        }
        let was_down = self.injected.alt_keys & mask != 0;
        self.injected.alt_keys &= !mask;
        let suppressed_down = self.injected.suppressed_alt_keys & mask != 0;
        self.injected.suppressed_alt_keys &= !mask;
        let had_gesture = self.injected.alt_gesture_active;
        let completed = had_gesture && self.injected.alt_keys == 0;
        if completed {
            self.injected.alt_gesture_active = false;
        }
        let finish = |suppress: bool| {
            if completed {
                HookOutcome::one(suppress, InputAction::AltReleased)
            } else {
                HookOutcome {
                    suppress,
                    ..HookOutcome::default()
                }
            }
        };
        if suppressed_down {
            return Some(finish(true));
        }
        if had_gesture && was_down {
            self.replay_masked_modifier_release(key);
            return Some(finish(true));
        }
        was_down.then(HookOutcome::default)
    }

    fn process_injected_tab(&mut self, settings: HookSettings) -> Option<HookOutcome> {
        if self.interception.suspended || self.physical_switch_busy() {
            return None;
        }
        let win_tab = settings.replace_win_tab && self.injected.win_keys != 0;
        let alt_tab = settings.replace_alt_tab && self.injected.alt_keys != 0 && !win_tab;
        if !win_tab && !alt_tab {
            return None;
        }
        if win_tab {
            self.injected.win_gesture_active = true;
        } else {
            self.injected.alt_gesture_active = true;
        }
        self.injected.suppressed_tab_release = true;
        let delta = if self.injected.shift_keys != 0 { -1 } else { 1 };
        Some(HookOutcome::one(true, InputAction::Switch(delta)))
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
                Some(HookOutcome::default())
            );
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
    fn injected_win_tab_macro_opens_and_cycles_without_replaying_arbitrary_keys() {
        let mut state = HookState::default();
        let plain = Modifiers::default();

        assert_eq!(
            key(
                &mut state,
                INJECTED,
                KeyEvent::pressed(Key::LeftWindows, plain)
            ),
            Some(HookOutcome::default())
        );
        let tab = key(&mut state, INJECTED, KeyEvent::pressed(Key::Tab, plain));
        assert!(tab.is_some_and(|o| o.suppress));
        assert_eq!(
            tab.map(|o| o.actions().collect::<Vec<_>>()),
            Some(vec![InputAction::Switch(1)])
        );
        state.set_overlay_active(true);

        assert_eq!(
            key(&mut state, INJECTED, KeyEvent::released(Key::Tab, plain)),
            Some(HookOutcome::suppressed())
        );
        let win_up = key(
            &mut state,
            INJECTED,
            KeyEvent::released(Key::LeftWindows, plain),
        );
        assert_eq!(win_up, Some(HookOutcome::suppressed()));
        assert_eq!(
            state.take_replayed_key_events(),
            [
                Some(ReplayedKeyEvent::pressed(MENU_MASK_KEY)),
                Some(ReplayedKeyEvent::released(MENU_MASK_KEY)),
                Some(ReplayedKeyEvent::released(Key::LeftWindows)),
            ]
        );

        assert_eq!(
            key(
                &mut state,
                INJECTED,
                KeyEvent::pressed(Key::Function(8), plain)
            ),
            None
        );
        assert_eq!(
            key(&mut state, INJECTED, KeyEvent::pressed(Key::F4, plain)),
            None
        );
        assert_eq!(
            key(&mut state, INJECTED, KeyEvent::pressed(Key::Enter, plain)),
            None
        );

        assert_eq!(
            key(
                &mut state,
                INJECTED,
                KeyEvent::pressed(Key::LeftWindows, plain)
            ),
            Some(HookOutcome::suppressed())
        );
        assert_eq!(
            key(
                &mut state,
                INJECTED,
                KeyEvent::pressed(Key::LeftShift, plain)
            ),
            Some(HookOutcome::suppressed())
        );
        let reverse = key(&mut state, INJECTED, KeyEvent::pressed(Key::Tab, plain));
        assert_eq!(
            reverse.map(|o| o.actions().collect::<Vec<_>>()),
            Some(vec![InputAction::Switch(-1)])
        );
        assert_eq!(
            key(&mut state, INJECTED, KeyEvent::released(Key::Tab, plain)),
            Some(HookOutcome::suppressed())
        );
        assert_eq!(
            key(
                &mut state,
                INJECTED,
                KeyEvent::released(Key::LeftShift, plain)
            ),
            Some(HookOutcome::suppressed())
        );
        assert_eq!(
            key(
                &mut state,
                INJECTED,
                KeyEvent::released(Key::LeftWindows, plain)
            ),
            Some(HookOutcome::suppressed())
        );
        assert_eq!(state.take_replayed_key_events(), [None; 3]);
    }

    #[test]
    fn injected_alt_tab_macro_opens_masks_release_and_commits_on_alt_up() {
        let mut state = HookState::default();
        let plain = Modifiers::default();

        assert_eq!(
            key(&mut state, INJECTED, KeyEvent::pressed(Key::LeftAlt, plain)),
            Some(HookOutcome::default())
        );
        let tab = key(&mut state, INJECTED, KeyEvent::pressed(Key::Tab, plain));
        assert_eq!(
            tab.map(|o| o.actions().collect::<Vec<_>>()),
            Some(vec![InputAction::Switch(1)])
        );
        state.set_overlay_active(true);
        assert_eq!(
            key(&mut state, INJECTED, KeyEvent::released(Key::Tab, plain)),
            Some(HookOutcome::suppressed())
        );
        let alt_up = key(
            &mut state,
            INJECTED,
            KeyEvent::released(Key::LeftAlt, plain),
        );
        assert!(alt_up.is_some_and(|o| o.suppress));
        assert_eq!(
            alt_up.map(|o| o.actions().collect::<Vec<_>>()),
            Some(vec![InputAction::AltReleased])
        );
        assert_eq!(
            state.take_replayed_key_events(),
            [
                Some(ReplayedKeyEvent::pressed(MENU_MASK_KEY)),
                Some(ReplayedKeyEvent::released(MENU_MASK_KEY)),
                Some(ReplayedKeyEvent::released(Key::LeftAlt)),
            ]
        );
    }

    #[test]
    fn non_switcher_injected_win_macros_pass_through_without_replay() {
        let mut state = HookState::default();
        let plain = Modifiers::default();

        assert_eq!(
            key(
                &mut state,
                INJECTED,
                KeyEvent::pressed(Key::LeftWindows, plain)
            ),
            Some(HookOutcome::default())
        );
        assert_eq!(
            key(
                &mut state,
                INJECTED,
                KeyEvent::pressed(Key::Other(u16::from(b'R')), plain)
            ),
            None
        );
        assert_eq!(
            key(
                &mut state,
                INJECTED,
                KeyEvent::released(Key::Other(u16::from(b'R')), plain)
            ),
            None
        );
        assert_eq!(
            key(
                &mut state,
                INJECTED,
                KeyEvent::released(Key::LeftWindows, plain)
            ),
            Some(HookOutcome::default())
        );
        assert_eq!(state.take_replayed_key_events(), [None; 3]);
    }

    #[test]
    fn injected_win_shift_tab_forwards_its_forwarded_shift_release_after_overlay_opens() {
        let mut state = HookState::default();
        let plain = Modifiers::default();

        assert_eq!(
            key(
                &mut state,
                INJECTED,
                KeyEvent::pressed(Key::LeftWindows, plain)
            ),
            Some(HookOutcome::default())
        );
        assert_eq!(
            key(
                &mut state,
                INJECTED,
                KeyEvent::pressed(Key::LeftShift, plain)
            ),
            Some(HookOutcome::default())
        );
        let tab = key(&mut state, INJECTED, KeyEvent::pressed(Key::Tab, plain));
        assert_eq!(
            tab.map(|o| o.actions().collect::<Vec<_>>()),
            Some(vec![InputAction::Switch(-1)])
        );
        state.set_overlay_active(true);
        assert_eq!(
            key(&mut state, INJECTED, KeyEvent::released(Key::Tab, plain)),
            Some(HookOutcome::suppressed())
        );
        assert_eq!(
            key(
                &mut state,
                INJECTED,
                KeyEvent::released(Key::LeftShift, plain)
            ),
            Some(HookOutcome::default())
        );
        assert_eq!(
            key(
                &mut state,
                INJECTED,
                KeyEvent::released(Key::LeftWindows, plain)
            ),
            Some(HookOutcome::suppressed())
        );
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
            state.process_mouse_from(PHYSICAL, MouseEvent::Wheel(-120, (0, 0)), settings),
            Some(HookOutcome::default())
        );
        let _down = state.process_mouse_from(PHYSICAL, MouseEvent::RightButtonPressed, settings);
        for source in [INJECTED, REPLAY] {
            for event in [
                MouseEvent::LeftButtonPressed,
                MouseEvent::Wheel(-120, (0, 0)),
                MouseEvent::RightButtonReleased,
            ] {
                assert_eq!(state.process_mouse_from(source, event, settings), None);
            }
        }
        assert!(state.owns_right_button());
        assert!(!state.take_replayed_right_click());
        assert_eq!(
            state
                .process_mouse_from(PHYSICAL, MouseEvent::Wheel(-120, (0, 0)), settings)
                .map(|o| o.actions().collect::<Vec<_>>()),
            Some(vec![InputAction::MouseWheel(1, (0, 0))])
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
