//! Opt-in right-button wheel gestures. A native click is replayed only if no wheel was used.

use super::{HookOutcome, InputAction};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseEvent {
    LeftButtonPressed,
    RightButtonPressed,
    RightButtonReleased,
    Wheel(i16),
}

#[derive(Debug, Default)]
pub(super) struct MouseGesture {
    press: MousePress,
    replay_click: bool,
}

#[derive(Debug, Default)]
enum MousePress {
    #[default]
    Idle,
    PendingClick,
    Scrolling {
        remainder: i32,
        active: bool,
    },
    Cancelled,
}

impl MouseGesture {
    pub(super) const fn is_active(&self) -> bool {
        matches!(self.press, MousePress::Scrolling { active: true, .. })
    }

    pub(super) fn cancel(&mut self) {
        // A cancelled gesture still owns the physical release of its swallowed press.
        if self.owns_button() {
            self.press = MousePress::Cancelled;
        }
        self.replay_click = false;
    }

    pub(super) const fn owns_button(&self) -> bool {
        !matches!(self.press, MousePress::Idle)
    }

    pub(super) fn take_replayed_click(&mut self) -> bool {
        core::mem::take(&mut self.replay_click)
    }

    pub(super) fn process(&mut self, event: MouseEvent, enabled: bool) -> HookOutcome {
        if !enabled {
            self.cancel();
        }
        match event {
            MouseEvent::LeftButtonPressed => {
                self.cancel();
                HookOutcome::default()
            }
            MouseEvent::RightButtonPressed if enabled => {
                *self = Self {
                    press: MousePress::PendingClick,
                    ..Self::default()
                };
                HookOutcome::suppressed()
            }
            MouseEvent::RightButtonPressed => {
                *self = Self::default();
                HookOutcome::default()
            }
            MouseEvent::RightButtonReleased => match core::mem::take(&mut self.press) {
                MousePress::Idle => HookOutcome::default(),
                MousePress::PendingClick => {
                    self.replay_click = true;
                    HookOutcome::suppressed()
                }
                MousePress::Scrolling { active: true, .. } => {
                    HookOutcome::one(true, InputAction::MouseWheelReleased)
                }
                MousePress::Scrolling { active: false, .. } | MousePress::Cancelled => {
                    HookOutcome::suppressed()
                }
            },
            MouseEvent::Wheel(delta) if delta != 0 => {
                let (remainder, active) = match self.press {
                    MousePress::PendingClick => (0, false),
                    MousePress::Scrolling { remainder, active } => (remainder, active),
                    MousePress::Idle | MousePress::Cancelled => return HookOutcome::default(),
                };
                let total = remainder + i32::from(delta);
                let steps = -(total / 120);
                self.press = MousePress::Scrolling {
                    remainder: total % 120,
                    active: active || steps != 0,
                };
                if steps == 0 {
                    HookOutcome::suppressed()
                } else {
                    HookOutcome::one(true, InputAction::MouseWheel(steps))
                }
            }
            MouseEvent::Wheel(_) => HookOutcome::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{HookSettings, HookState};

    const ENABLED: HookSettings = HookSettings {
        right_button_wheel_switching: true,
        replace_alt_tab: true,
        replace_win_tab: true,
        typed_search: true,
        search_active: false,
        filter_visible: false,
    };

    #[test]
    fn ordinary_overlay_wheel_and_rmb_gestures_keep_distinct_ownership() {
        for settings in [HookSettings::default(), ENABLED] {
            let mut state = HookState::default();
            state.set_overlay_active(true);
            let ordinary = state.process_mouse(MouseEvent::Wheel(30), settings);
            assert!(ordinary.suppress);
            assert_eq!(
                ordinary.actions().collect::<Vec<_>>(),
                [InputAction::OverlayWheel(30)]
            );
            if settings.right_button_wheel_switching {
                let _press = state.process_mouse(MouseEvent::RightButtonPressed, settings);
                let gesture = state.process_mouse(MouseEvent::Wheel(-120), settings);
                assert!(gesture.suppress);
                assert_eq!(
                    gesture.actions().collect::<Vec<_>>(),
                    [InputAction::MouseWheel(1)]
                );
                assert_eq!(
                    state
                        .process_mouse(MouseEvent::RightButtonReleased, settings)
                        .actions()
                        .collect::<Vec<_>>(),
                    [InputAction::MouseWheelReleased]
                );
            }
        }
    }

    #[test]
    fn selecting_with_a_click_or_enter_ends_mouse_ownership_immediately() {
        use crate::input::{Key, KeyEvent, Modifiers};
        for click in [false, true] {
            let mut state = HookState::default();
            let _press = state.process_mouse(MouseEvent::RightButtonPressed, ENABLED);
            let _wheel = state.process_mouse(MouseEvent::Wheel(-120), ENABLED);
            state.set_overlay_active(true);
            if click {
                let _click = state.process_mouse(MouseEvent::LeftButtonPressed, ENABLED);
            } else {
                let enter =
                    state.process_key(KeyEvent::pressed(Key::Enter, Modifiers::default()), ENABLED);
                assert!(
                    enter.suppress,
                    "Enter must not reach the previous application"
                );
                assert_eq!(enter.actions().next(), Some(InputAction::ActivateSelected));
                assert!(
                    state
                        .process_key(
                            KeyEvent::released(Key::Enter, Modifiers::default()),
                            ENABLED
                        )
                        .suppress
                );
            }
            // Until the UI acknowledges closing, capture ordinary wheel input without
            // reviving the completed right-button gesture.
            let wheel = state.process_mouse(MouseEvent::Wheel(-120), ENABLED);
            assert!(wheel.suppress);
            assert_eq!(
                wheel.actions().next(),
                Some(InputAction::OverlayWheel(-120))
            );
            assert_eq!(
                state.process_mouse(MouseEvent::RightButtonReleased, ENABLED),
                HookOutcome::suppressed()
            );
            state.set_overlay_active(false);
            assert_eq!(
                state.process_mouse(MouseEvent::Wheel(-120), ENABLED),
                HookOutcome::default()
            );
        }
    }

    #[test]
    fn escape_cancels_queued_mouse_actions_before_the_ui_resets_the_hook() {
        use crate::hook_delivery::{Delivery, deliver};
        use crate::hook_flags::{HookFlags, OVERLAY_ACTIVE, OVERLAY_FLAGS};
        use crate::input::{Key, KeyEvent, Modifiers};
        use crate::switcher::{
            SwitchTask, SwitcherEffect, SwitcherSession, SwitcherSessionSettings,
        };

        fn post(outcome: HookOutcome, flags: &HookFlags, queue: &mut Vec<InputAction>) {
            assert_eq!(
                deliver(
                    flags,
                    flags.load() & !OVERLAY_FLAGS,
                    ENABLED.typed_search,
                    outcome,
                    |action| {
                        queue.push(action);
                        true
                    }
                ),
                Delivery::Posted
            );
        }

        let mut state = HookState::default();
        let flags = HookFlags::new();
        let mut queue = Vec::new();
        let _down = state.process_mouse(MouseEvent::RightButtonPressed, ENABLED);
        post(
            state.process_mouse(MouseEvent::Wheel(-120), ENABLED),
            &flags,
            &mut queue,
        );
        state.set_overlay_active(flags.load() & OVERLAY_ACTIVE != 0);
        post(
            state.process_key(
                KeyEvent::pressed(Key::Escape, Modifiers::default()),
                ENABLED,
            ),
            &flags,
            &mut queue,
        );
        let wheel_after_escape = state.process_mouse(MouseEvent::Wheel(-120), ENABLED);
        let release_after_escape = state.process_mouse(MouseEvent::RightButtonReleased, ENABLED);
        post(wheel_after_escape, &flags, &mut queue);
        post(release_after_escape, &flags, &mut queue);

        // Drain only after every physical event. Hiding deliberately does not run the
        // asynchronous hook reset, so the hook must have cancelled the gesture itself.
        let mut session = SwitcherSession::new(SwitcherSessionSettings {
            typed_search: true,
            release_alt_switches: true,
        });
        let mut openings = 0;
        let mut activations = Vec::new();
        for action in queue {
            match session.handle_input(action) {
                SwitcherEffect::Open { selection_delta } => {
                    openings += 1;
                    session.open(
                        [
                            SwitchTask::new(1, 10, "One", "one"),
                            SwitchTask::new(2, 20, "Two", "two"),
                        ],
                        selection_delta,
                    );
                }
                SwitcherEffect::Hide => session.hide(),
                SwitcherEffect::Activate(target) => activations.push(target),
                SwitcherEffect::None | SwitcherEffect::Redraw => {}
                effect @ SwitcherEffect::Execute(_) => {
                    panic!("unexpected queued mouse effect: {effect:?}");
                }
            }
        }
        assert_eq!(
            openings, 1,
            "scrolling after Escape reopened the cancelled switcher"
        );
        assert!(
            activations.is_empty(),
            "RMB release activated a window after Escape"
        );
        assert!(!session.is_visible());
        // The still-visible overlay captures the wheel until it closes, but this action
        // cannot reopen it or start a fresh right-button gesture.
        assert!(wheel_after_escape.suppress);
        assert_eq!(
            wheel_after_escape.actions().next(),
            Some(InputAction::OverlayWheel(-120))
        );
        assert_eq!(release_after_escape, HookOutcome::suppressed());

        let _fresh_down = state.process_mouse(MouseEvent::RightButtonPressed, ENABLED);
        assert_eq!(
            state
                .process_mouse(MouseEvent::Wheel(-120), ENABLED)
                .actions()
                .next(),
            Some(InputAction::MouseWheel(1)),
            "a fresh RMB press must start a new gesture"
        );
    }

    #[test]
    fn unavailable_win_number_keeps_number_selection_until_successful_completion() {
        use crate::input::{Key, KeyEvent, Modifiers};
        use crate::switcher::{
            SwitchTask, SwitcherEffect, SwitcherSession, SwitcherSessionSettings,
        };
        for typed_search in [false, true] {
            let mut hook = HookState::default();
            let settings = HookSettings {
                search_active: typed_search,
                right_button_wheel_switching: false,
                ..HookSettings::default()
            };
            let win = Modifiers {
                left_windows: true,
                ..Modifiers::default()
            };
            for event in [
                KeyEvent::pressed(Key::LeftWindows, Modifiers::default()),
                KeyEvent::pressed(Key::Tab, win),
                KeyEvent::released(Key::Tab, win),
                KeyEvent::released(Key::LeftWindows, Modifiers::default()),
            ] {
                let _event = hook.process_key(event, settings);
            }
            let mut session = SwitcherSession::new(SwitcherSessionSettings {
                typed_search,
                release_alt_switches: true,
            });
            session.open(
                [
                    SwitchTask::new(1, 10, "One", "one"),
                    SwitchTask::new(2, 20, "Two", "two"),
                ],
                None,
            );
            hook.set_overlay_active(true);
            for (digit, expected) in [(9, SwitcherEffect::None), (2, SwitcherEffect::Activate(20))]
            {
                let outcome = hook.process_key(
                    KeyEvent::pressed(Key::Digit(digit), Modifiers::default())
                        .with_text(char::from(b'0' + digit)),
                    settings,
                );
                assert!(outcome.suppress);
                let Some(action) = outcome.actions().next() else {
                    panic!("number must reach the session");
                };
                assert_eq!(session.handle_input(action), expected);
                assert!(session.switcher().filter().is_empty());
                hook.set_overlay_active(session.is_visible());
                assert!(
                    hook.process_key(
                        KeyEvent::released(Key::Digit(digit), Modifiers::default()),
                        settings
                    )
                    .suppress
                );
            }
            assert!(!session.is_visible());
            assert!(
                !hook
                    .process_key(
                        KeyEvent::pressed(Key::Other(0x41), Modifiers::default()),
                        HookSettings::default()
                    )
                    .suppress,
                "successful selection must stop capturing ordinary typing"
            );
        }
    }

    #[test]
    fn ownership_of_sticky_win_gesture_ends_on_number_or_pointer_selection() {
        use crate::input::{Key, KeyEvent, Modifiers};
        for pointer in [false, true] {
            let mut state = HookState::default();
            let settings = HookSettings::default();
            let win = Modifiers {
                left_windows: true,
                ..Modifiers::default()
            };
            for event in [
                KeyEvent::pressed(Key::LeftWindows, Modifiers::default()),
                KeyEvent::pressed(Key::Tab, win),
                KeyEvent::released(Key::Tab, win),
                KeyEvent::released(Key::LeftWindows, Modifiers::default()),
            ] {
                let _event = state.process_key(event, settings);
            }
            state.set_overlay_active(true);
            if pointer {
                let _click = state.process_mouse(MouseEvent::LeftButtonPressed, settings);
            } else {
                let _number = state.process_key(
                    KeyEvent::pressed(Key::Digit(1), Modifiers::default()),
                    settings,
                );
                assert!(
                    state
                        .process_key(
                            KeyEvent::released(Key::Digit(1), Modifiers::default()),
                            settings
                        )
                        .suppress
                );
            }
            state.set_overlay_active(false);
            assert!(
                !state
                    .process_key(
                        KeyEvent::pressed(Key::Other(0x41), Modifiers::default()),
                        settings
                    )
                    .suppress,
                "completed Win gesture swallowed ordinary typing"
            );
        }
    }

    #[test]
    fn mixed_mouse_ownership_retires_sticky_keyboard_completion() {
        use crate::input::{Key, KeyEvent, Modifiers};
        let mut state = HookState::default();
        let win = Modifiers {
            left_windows: true,
            ..Modifiers::default()
        };
        for event in [
            KeyEvent::pressed(Key::LeftWindows, Modifiers::default()),
            KeyEvent::pressed(Key::Tab, win),
            KeyEvent::released(Key::Tab, win),
            KeyEvent::released(Key::LeftWindows, Modifiers::default()),
        ] {
            let _event = state.process_key(event, ENABLED);
        }
        state.set_overlay_active(true);
        let _down = state.process_mouse(MouseEvent::RightButtonPressed, ENABLED);
        let _wheel = state.process_mouse(MouseEvent::Wheel(-120), ENABLED);
        let release = state.process_mouse(MouseEvent::RightButtonReleased, ENABLED);
        assert_eq!(
            release.actions().next(),
            Some(InputAction::MouseWheelReleased)
        );
        state.set_overlay_active(false);
        assert!(
            !state
                .process_key(
                    KeyEvent::pressed(Key::Other(0x41), Modifiers::default()),
                    ENABLED
                )
                .suppress,
            "mouse completion left keyboard interception active"
        );
    }

    #[test]
    fn non_selection_click_keeps_alt_release_completion() {
        use crate::input::{Key, KeyEvent, Modifiers};
        use crate::switcher::{
            SwitchTask, SwitcherEffect, SwitcherSession, SwitcherSessionSettings,
        };
        use crate::window_command::WindowCommand;
        for close_button in [false, true] {
            let mut state = HookState::default();
            let settings = HookSettings::default();
            let alt = Modifiers {
                alt: true,
                ..Modifiers::default()
            };
            let _down = state.process_key(
                KeyEvent::pressed(Key::LeftAlt, Modifiers::default()),
                settings,
            );
            let _tab = state.process_key(KeyEvent::pressed(Key::Tab, alt), settings);
            let _up = state.process_key(KeyEvent::released(Key::Tab, alt), settings);
            state.set_overlay_active(true);
            let mut session = SwitcherSession::new(SwitcherSessionSettings {
                typed_search: true,
                release_alt_switches: true,
            });
            session.open(
                [
                    SwitchTask::new(1, 10, "One", "one"),
                    SwitchTask::new(2, 20, "Two", "two"),
                ],
                Some(1),
            );
            let _click = state.process_mouse(MouseEvent::LeftButtonPressed, settings);
            // Neither unused panel space nor closing one task completes the overlay.
            if close_button {
                assert!(matches!(
                    session.handle_input(InputAction::WindowCommand(WindowCommand::Close)),
                    SwitcherEffect::Execute(_)
                ));
            }
            assert!(session.is_visible());
            let release = state.process_key(
                KeyEvent::released(Key::LeftAlt, Modifiers::default()),
                settings,
            );
            assert_eq!(
                release.actions().next(),
                Some(InputAction::AltReleased),
                "Alt completion lost after non-selection click (close={close_button})"
            );
            assert!(matches!(
                session.handle_input(InputAction::AltReleased),
                SwitcherEffect::Activate(_)
            ));
        }
    }

    #[test]
    fn mouse_number_shortcuts_respect_empty_and_filtered_queries() {
        use crate::input::{Key, KeyEvent, Modifiers};
        use crate::switcher::{
            SwitchTask, SwitcherEffect, SwitcherSession, SwitcherSessionSettings,
        };
        for filtered in [false, true] {
            let mut hook = HookState::default();
            let mut session = SwitcherSession::new(SwitcherSessionSettings {
                typed_search: true,
                release_alt_switches: true,
            });
            let _down = hook.process_mouse(MouseEvent::RightButtonPressed, ENABLED);
            let _wheel = hook.process_mouse(MouseEvent::Wheel(-120), ENABLED);
            session.open(
                [
                    SwitchTask::new(1, 10, "One1", "one"),
                    SwitchTask::new(2, 20, "Two", "two"),
                ],
                Some(1),
            );
            if filtered {
                for character in "One".chars() {
                    let _edit = session.handle_input(InputAction::AppendSearchCharacter(character));
                }
            }
            hook.set_overlay_active(true);
            let settings = HookSettings {
                search_active: true,
                filter_visible: filtered,
                ..ENABLED
            };
            let mut event = KeyEvent::pressed(Key::Digit(1), Modifiers::default());
            event.text = Some('1');
            let number = hook.process_key(event, settings);
            assert!(number.suppress);
            assert_eq!(
                number.actions().next(),
                Some(InputAction::NumberKey {
                    position: 1,
                    text: Some('1')
                })
            );
            let effect = session.handle_input(InputAction::NumberKey {
                position: 1,
                text: Some('1'),
            });
            if filtered {
                assert_eq!(effect, SwitcherEffect::Redraw);
                assert_eq!(session.switcher().filter(), "One1");
            } else {
                assert!(matches!(effect, SwitcherEffect::Activate(_)));
            }
            assert!(
                hook.process_key(
                    KeyEvent::released(Key::Digit(1), Modifiers::default()),
                    settings
                )
                .suppress
            );
            assert!(
                hook.process_mouse(MouseEvent::RightButtonReleased, ENABLED)
                    .actions()
                    .next()
                    .is_none()
            );
        }
    }

    #[test]
    fn search_edit_shortcut_retires_mouse_release_completion() {
        use crate::input::{Key, KeyEvent, Modifiers};
        use crate::text_edit::TextCommand;
        let mut state = HookState::default();
        let _press = state.process_mouse(MouseEvent::RightButtonPressed, ENABLED);
        let _wheel = state.process_mouse(MouseEvent::Wheel(-120), ENABLED);
        state.set_overlay_active(true);
        let editing = state.process_key(
            KeyEvent::pressed(
                Key::Other(0x56),
                Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
            ),
            HookSettings {
                search_active: true,
                ..ENABLED
            },
        );
        assert!(editing.suppress);
        assert_eq!(
            editing.actions().next(),
            Some(InputAction::EditSearch(TextCommand::Paste))
        );
        assert!(
            state
                .process_mouse(MouseEvent::RightButtonReleased, ENABLED)
                .actions()
                .next()
                .is_none(),
            "pasting a search must not commit on RMB release"
        );
        let _fresh = state.process_mouse(MouseEvent::RightButtonPressed, ENABLED);
        assert_eq!(
            state
                .process_mouse(MouseEvent::Wheel(-120), ENABLED)
                .actions()
                .next(),
            Some(InputAction::MouseWheel(1))
        );
    }

    #[test]
    fn ownership_after_mouse_search_allows_a_fresh_context_click() {
        use crate::input::{Key, KeyEvent, Modifiers};
        use crate::switcher::{
            SwitchTask, SwitcherEffect, SwitcherSession, SwitcherSessionSettings,
        };
        let mut hook = HookState::default();
        let mut session = SwitcherSession::new(SwitcherSessionSettings {
            typed_search: true,
            release_alt_switches: true,
        });
        let _down = hook.process_mouse(MouseEvent::RightButtonPressed, ENABLED);
        let opening = hook.process_mouse(MouseEvent::Wheel(-120), ENABLED);
        let Some(action) = opening.actions().next() else {
            panic!("missing open action");
        };
        let SwitcherEffect::Open { selection_delta } = session.handle_input(action) else {
            panic!("missing open effect");
        };
        session.open([SwitchTask::new(1, 10, "One", "one")], selection_delta);
        hook.set_overlay_active(true);
        let mut event = KeyEvent::pressed(Key::Other(0x4F), Modifiers::default());
        event.text = Some('o');
        let typing = hook.process_key(
            event,
            HookSettings {
                search_active: true,
                ..ENABLED
            },
        );
        for action in typing.actions() {
            let _effect = session.handle_input(action);
        }
        assert!(
            hook.process_mouse(MouseEvent::RightButtonReleased, ENABLED)
                .actions()
                .next()
                .is_none()
        );
        assert!(
            !session.open_context_menu(),
            "the original held-button release must remain guarded"
        );
        let fresh = hook.process_mouse(MouseEvent::RightButtonPressed, ENABLED);
        for action in fresh.actions() {
            let _effect = session.handle_input(action);
        }
        assert!(
            session.open_context_menu(),
            "a fresh right click must open the task menu after searching"
        );
    }

    #[test]
    fn plain_clicks_are_replayed_only_when_enabled_and_scrolling_passes_through() {
        for settings in [HookSettings::default(), ENABLED] {
            let mut state = HookState::default();
            assert_eq!(
                state.process_mouse(MouseEvent::Wheel(120), settings),
                HookOutcome::default()
            );
            assert_eq!(
                state.process_mouse(MouseEvent::RightButtonPressed, settings),
                if settings.right_button_wheel_switching {
                    HookOutcome::suppressed()
                } else {
                    HookOutcome::default()
                }
            );
            if !settings.right_button_wheel_switching {
                assert_eq!(
                    state.process_mouse(MouseEvent::Wheel(-120), settings),
                    HookOutcome::default()
                );
            }
            assert_eq!(
                state.process_mouse(MouseEvent::RightButtonReleased, settings),
                if settings.right_button_wheel_switching {
                    HookOutcome::suppressed()
                } else {
                    HookOutcome::default()
                }
            );
            assert_eq!(
                state.take_replayed_right_click(),
                settings.right_button_wheel_switching
            );
            assert!(!state.take_replayed_right_click());
        }
    }

    #[test]
    fn wheel_direction_and_release_follow_one_physical_gesture() {
        let mut state = HookState::default();
        let down = state.process_mouse(MouseEvent::RightButtonPressed, ENABLED);
        assert!(down.suppress);
        for (delta, steps) in [(120, -1), (-240, 2)] {
            let wheel = state.process_mouse(MouseEvent::Wheel(delta), ENABLED);
            assert!(wheel.suppress);
            assert_eq!(
                wheel.actions().collect::<Vec<_>>(),
                [InputAction::MouseWheel(steps)]
            );
        }
        let release = state.process_mouse(MouseEvent::RightButtonReleased, ENABLED);
        assert!(release.suppress);
        assert_eq!(
            release.actions().collect::<Vec<_>>(),
            [InputAction::MouseWheelReleased]
        );
        assert_eq!(
            state.process_mouse(MouseEvent::RightButtonReleased, ENABLED),
            HookOutcome::default()
        );
        assert_eq!(
            state.process_mouse(MouseEvent::Wheel(120), ENABLED),
            HookOutcome::default()
        );
    }

    #[test]
    fn high_resolution_wheels_accumulate_and_clear_at_release() {
        let mut state = HookState::default();
        let _down = state.process_mouse(MouseEvent::RightButtonPressed, ENABLED);
        for _ in 0..3 {
            let partial = state.process_mouse(MouseEvent::Wheel(30), ENABLED);
            assert!(partial.suppress);
            assert!(partial.actions().next().is_none());
        }
        assert_eq!(
            state
                .process_mouse(MouseEvent::Wheel(30), ENABLED)
                .actions()
                .next(),
            Some(InputAction::MouseWheel(-1))
        );
        let _partial = state.process_mouse(MouseEvent::Wheel(30), ENABLED);
        let _release = state.process_mouse(MouseEvent::RightButtonReleased, ENABLED);
        let _down = state.process_mouse(MouseEvent::RightButtonPressed, ENABLED);
        assert!(
            state
                .process_mouse(MouseEvent::Wheel(90), ENABLED)
                .actions()
                .next()
                .is_none()
        );
        assert!(
            state
                .process_mouse(MouseEvent::RightButtonReleased, ENABLED)
                .actions()
                .next()
                .is_none()
        );
    }

    #[test]
    fn suspension_reset_and_disabling_require_a_fresh_button_press() {
        for boundary in 0..3 {
            let mut state = HookState::default();
            let _down = state.process_mouse(MouseEvent::RightButtonPressed, ENABLED);
            let _wheel = state.process_mouse(MouseEvent::Wheel(120), ENABLED);
            match boundary {
                0 => {
                    state.set_interception_suspended(true);
                    assert_eq!(
                        state.process_mouse(MouseEvent::Wheel(120), ENABLED),
                        HookOutcome::default()
                    );
                    state.set_interception_suspended(false);
                }
                1 => state.reset_gestures(),
                _ => {
                    let _disabled =
                        state.process_mouse(MouseEvent::Wheel(120), HookSettings::default());
                }
            }
            assert_eq!(
                state.process_mouse(MouseEvent::Wheel(120), ENABLED),
                HookOutcome::default()
            );
            assert_eq!(
                state.process_mouse(MouseEvent::RightButtonReleased, ENABLED),
                HookOutcome::suppressed()
            );
            assert!(!state.take_replayed_right_click());
        }
    }
}
