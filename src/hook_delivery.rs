//! How the input hook hands an outcome's actions to the UI thread, decided apart from the
//! platform call that posts each one.

use crate::hook_flags::HookFlags;
use crate::input::{HookOutcome, InputAction, Key, KeyTransition};

/// What became of an outcome's actions.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    /// Every action was posted.
    Posted,
    /// A modal boundary or desktop recovery overtook the outcome, so the rest was not posted.
    Stale,
    /// The overlay could not be armed for this generation, so its opening action was withheld.
    OverlayRefused,
    /// Posting an action failed. If it would have opened the overlay, the arming was withdrawn.
    PostFailed,
}

/// Posts the outcome's actions in order. Posting can race with a modal boundary, so the
/// generation is checked again before each action. An action that opens the overlay arms overlay
/// and search interception first: keys typed before the UI acknowledges the overlay must already
/// reach it.
pub fn deliver(
    flags: &HookFlags,
    generation: usize,
    typed_search: bool,
    outcome: HookOutcome,
    mut post: impl FnMut(InputAction) -> bool,
) -> Delivery {
    for action in outcome.actions() {
        if !flags.generation_is_current(generation) {
            return Delivery::Stale;
        }
        let opens_overlay = matches!(
            action,
            InputAction::Switch(_) | InputAction::RightButtonPressed
        );
        if opens_overlay && !flags.update_overlay(generation, true, typed_search) {
            return Delivery::OverlayRefused;
        }
        if !post(action) {
            if opens_overlay {
                let _cleared = flags.update_overlay(generation, false, false);
            }
            return Delivery::PostFailed;
        }
    }
    Delivery::Posted
}

/// Whether a key event may take the registered-hotkey route: a physical Tab press, not injected
/// input, whose outcome starts or cycles a switch.
#[must_use]
pub fn routes_tab_through_hotkey(
    key: Key,
    transition: KeyTransition,
    injected: bool,
    outcome: HookOutcome,
) -> bool {
    key == Key::Tab
        && transition == KeyTransition::Pressed
        && !injected
        && outcome
            .actions()
            .any(|action| matches!(action, InputAction::Switch(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook_flags::{INTERCEPTION_SUSPENDED, OVERLAY_ACTIVE, OVERLAY_FLAGS};
    use crate::input::{HookSettings, HookState, KeyEvent, Modifiers, MouseEvent};

    const ALT: Modifiers = Modifiers {
        alt: true,
        left_windows: false,
        right_windows: false,
    };

    fn alt_tab() -> HookOutcome {
        HookState::default().process_key(KeyEvent::pressed(Key::Tab, ALT), HookSettings::default())
    }

    fn current_flags() -> (HookFlags, usize) {
        let flags = HookFlags::new();
        let generation = flags.load() & !OVERLAY_FLAGS;
        (flags, generation)
    }

    #[test]
    fn opening_the_overlay_arms_interception_before_the_action_is_posted() {
        for (typed_search, armed) in [(true, OVERLAY_FLAGS), (false, OVERLAY_ACTIVE)] {
            let (flags, generation) = current_flags();
            let mut posted = Vec::new();
            let delivery = deliver(&flags, generation, typed_search, alt_tab(), |action| {
                assert_eq!(flags.load() & OVERLAY_FLAGS, armed);
                posted.push(action);
                true
            });
            assert_eq!(delivery, Delivery::Posted);
            assert_eq!(posted, [InputAction::Switch(1)]);
            assert_eq!(flags.load() & OVERLAY_FLAGS, armed);
        }
    }

    #[test]
    fn actions_that_do_not_open_the_overlay_leave_the_flags_alone() {
        let (flags, generation) = current_flags();
        let mut state = HookState::default();
        let settings = HookSettings::default();
        let _alt = state.process_key(KeyEvent::pressed(Key::LeftAlt, ALT), settings);
        let _tab = state.process_key(KeyEvent::pressed(Key::Tab, ALT), settings);
        let release = state.process_key(KeyEvent::released(Key::LeftAlt, ALT), settings);
        let before = flags.load();
        let delivery = deliver(&flags, generation, true, release, |action| {
            action == InputAction::AltReleased
        });
        assert_eq!(delivery, Delivery::Posted);
        assert_eq!(flags.load(), before);
    }

    #[test]
    fn a_stale_generation_posts_nothing() {
        let (flags, generation) = current_flags();
        flags.suspend(true);
        let delivery = deliver(&flags, generation, true, alt_tab(), |_| {
            panic!("a stale action was posted")
        });
        assert_eq!(delivery, Delivery::Stale);
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);

        let (flags, generation) = current_flags();
        flags.set_recovery_pending(true);
        let delivery = deliver(&flags, generation, true, alt_tab(), |_| {
            panic!("an action crossed desktop recovery")
        });
        assert_eq!(delivery, Delivery::Stale);
    }

    #[test]
    fn a_failed_post_withdraws_the_overlay_it_armed() {
        let (flags, generation) = current_flags();
        let delivery = deliver(&flags, generation, true, alt_tab(), |_| false);
        assert_eq!(delivery, Delivery::PostFailed);
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
        assert!(flags.generation_is_current(generation));
    }

    #[test]
    fn a_modal_boundary_during_delivery_stops_the_remaining_actions() {
        let (flags, generation) = current_flags();
        let mut state = HookState::default();
        let settings = HookSettings::default();
        let _down = state.process_mouse(MouseEvent::RightButtonPressed, settings);
        let wheel = state.process_mouse(MouseEvent::Wheel(120), settings);
        let mut posted = Vec::new();
        let delivery = deliver(&flags, generation, true, wheel, |action| {
            posted.push(action);
            flags.suspend(true);
            true
        });
        assert_eq!(delivery, Delivery::Stale);
        assert_eq!(posted, [InputAction::RightButtonPressed]);
        assert_eq!(flags.load() & (OVERLAY_FLAGS | INTERCEPTION_SUSPENDED), 1);
    }

    #[test]
    fn only_a_physical_tab_press_that_switches_takes_the_hotkey_route() {
        let switching = alt_tab();
        assert!(routes_tab_through_hotkey(
            Key::Tab,
            KeyTransition::Pressed,
            false,
            switching
        ));
        assert!(!routes_tab_through_hotkey(
            Key::Tab,
            KeyTransition::Pressed,
            true,
            switching
        ));
        assert!(!routes_tab_through_hotkey(
            Key::Tab,
            KeyTransition::Released,
            false,
            switching
        ));
        assert!(!routes_tab_through_hotkey(
            Key::Escape,
            KeyTransition::Pressed,
            false,
            switching
        ));
        let plain_tab = HookState::default().process_key(
            KeyEvent::pressed(Key::Tab, Modifiers::default()),
            HookSettings::default(),
        );
        assert!(!routes_tab_through_hotkey(
            Key::Tab,
            KeyTransition::Pressed,
            false,
            plain_tab
        ));
    }
}
