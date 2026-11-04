//! How the input hook hands an outcome's actions to the UI thread, decided apart from the
//! platform call that posts each one.

use crate::hook_flags::HookFlags;
use crate::input::{HookOutcome, InputAction};

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
    /// Posting failed. The caller must withdraw any registered opening outside the callback.
    PostFailed { pending_opening: bool },
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
        let opens_overlay = matches!(action, InputAction::Switch(_) | InputAction::MouseWheel(_));
        if opens_overlay && !flags.begin_opening(generation, typed_search) {
            return Delivery::OverlayRefused;
        }
        if !post(action) {
            return Delivery::PostFailed {
                pending_opening: opens_overlay,
            };
        }
    }
    Delivery::Posted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook_flags::{OVERLAY_ACTIVE, OVERLAY_FLAGS, SEARCH_ACTIVE};
    use crate::input::{HookSettings, HookState, Key, KeyEvent, Modifiers};

    const ALT: Modifiers = Modifiers {
        alt: true,
        control: false,
        shift: false,
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
        for (typed_search, armed) in [
            (true, OVERLAY_ACTIVE | SEARCH_ACTIVE),
            (false, OVERLAY_ACTIVE),
        ] {
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
    fn completion_preserves_pending_input_until_applied_or_cancelled() {
        let (flags, generation) = current_flags();
        for _ in 0..2 {
            assert_eq!(
                deliver(&flags, generation, true, alt_tab(), |_| true),
                Delivery::Posted
            );
        }
        flags.retire_openings(generation, 1);
        flags.complete_overlay();
        assert_eq!(flags.load() & OVERLAY_FLAGS, OVERLAY_ACTIVE | SEARCH_ACTIVE);
        flags.retire_openings(generation, 1);
        flags.complete_overlay();
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);

        assert_eq!(
            deliver(&flags, generation, true, alt_tab(), |_| true),
            Delivery::Posted
        );
        flags.cancel_overlay();
        assert_eq!(
            flags.load() & OVERLAY_FLAGS,
            0,
            "cancellation must disarm even with queued openings"
        );
        flags.retire_openings(generation, 1);
        flags.complete_overlay();
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
    }

    #[test]
    fn a_failed_post_keeps_an_earlier_pending_opening_armed() {
        let (flags, generation) = current_flags();
        assert_eq!(
            deliver(&flags, generation, true, alt_tab(), |_| true),
            Delivery::Posted
        );
        assert_eq!(
            deliver(&flags, generation, true, alt_tab(), |_| false),
            Delivery::PostFailed {
                pending_opening: true
            }
        );
        flags.withdraw_openings(generation, 1);
        assert_eq!(flags.load() & OVERLAY_FLAGS, OVERLAY_ACTIVE | SEARCH_ACTIVE);
        flags.retire_openings(generation, 1);
        flags.complete_overlay();
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
    }

    #[test]
    fn a_failed_post_withdraws_the_overlay_it_armed() {
        let (flags, generation) = current_flags();
        let delivery = deliver(&flags, generation, true, alt_tab(), |_| false);
        assert_eq!(
            delivery,
            Delivery::PostFailed {
                pending_opening: true
            }
        );
        // Callback delivery keeps the receipt until the hook message loop withdraws it.
        assert_ne!(flags.load() & OVERLAY_FLAGS, 0);
        flags.withdraw_openings(generation, 1);
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
        assert!(flags.generation_is_current(generation));
    }
}
