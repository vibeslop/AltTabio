//! What the input hook thread shares with the UI thread: interception flags under a generation,
//! and action messages stamped with that generation so the UI can drop actions queued before a
//! modal or input-desktop boundary.

use crate::input::{InputAction, WindowCommand};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

pub const INTERCEPTION_SUSPENDED: usize = 1;
pub const SEARCH_ACTIVE: usize = 2;
pub const OVERLAY_ACTIVE: usize = 4;
pub const OVERLAY_FLAGS: usize = SEARCH_ACTIVE | OVERLAY_ACTIVE;
// Generations are multiples of 8, so the flags share their word.
const FLAG_BITS: usize = INTERCEPTION_SUSPENDED | OVERLAY_FLAGS;

const ACTION_SWITCH: usize = 1;
const ACTION_ACTIVATE_POSITION: usize = 2;
const ACTION_ALT_RELEASED: usize = 3;
const ACTION_RIGHT_BUTTON_PRESSED: usize = 4;
const ACTION_RIGHT_BUTTON_RELEASED: usize = 5;
const ACTION_MOUSE_WHEEL: usize = 6;
const ACTION_APPEND_SEARCH_CHARACTER: usize = 8;
const ACTION_BACKSPACE_SEARCH: usize = 9;
const ACTION_NAVIGATE: usize = 10;
const ACTION_ACTIVATE_SELECTED: usize = 11;
const ACTION_SELECT_FIRST: usize = 12;
const ACTION_SELECT_LAST: usize = 13;
const ACTION_DISMISS_OVERLAY: usize = 14;
const ACTION_CLOSE_SELECTED: usize = 15;
const ACTION_WINDOW_COMMAND: usize = 16;
const ACTION_CODE_MASK: usize = 0xFF;
const ACTION_EPOCH_MASK: usize = usize::MAX >> 8;
static NEXT_HOOK_EPOCH: AtomicUsize = AtomicUsize::new(1);

fn next_hook_generation() -> usize {
    (NEXT_HOOK_EPOCH.fetch_add(1, Ordering::Relaxed) & ACTION_EPOCH_MASK) << 3
}

const fn action_wparam(code: usize, generation: usize) -> usize {
    (((generation >> 3) & ACTION_EPOCH_MASK) << 8) | (code & ACTION_CODE_MASK)
}

/// Encodes an action as the `(wparam, lparam)` payload of a message stamped with `generation`.
#[must_use]
pub fn encode_action(action: InputAction, generation: usize) -> (usize, isize) {
    let (code, value) = match action {
        InputAction::Switch(delta) => (ACTION_SWITCH, delta as isize),
        InputAction::Navigate(delta) => (ACTION_NAVIGATE, delta as isize),
        InputAction::ActivateSelected => (ACTION_ACTIVATE_SELECTED, 0),
        InputAction::SelectFirst => (ACTION_SELECT_FIRST, 0),
        InputAction::SelectLast => (ACTION_SELECT_LAST, 0),
        InputAction::DismissOverlay => (ACTION_DISMISS_OVERLAY, 0),
        InputAction::CloseSelected => (ACTION_CLOSE_SELECTED, 0),
        InputAction::WindowCommand(command) => (
            ACTION_WINDOW_COMMAND,
            command.function_key().map_or(0, isize::from),
        ),
        InputAction::ActivateVisiblePosition(position) => (
            ACTION_ACTIVATE_POSITION,
            isize::try_from(position).unwrap_or_default(),
        ),
        InputAction::AltReleased => (ACTION_ALT_RELEASED, 0),
        InputAction::RightButtonPressed => (ACTION_RIGHT_BUTTON_PRESSED, 0),
        InputAction::RightButtonReleased => (ACTION_RIGHT_BUTTON_RELEASED, 0),
        InputAction::MouseWheel(delta) => (ACTION_MOUSE_WHEEL, delta as isize),
        InputAction::AppendSearchCharacter(character) => (
            ACTION_APPEND_SEARCH_CHARACTER,
            isize::try_from(u32::from(character)).unwrap_or_default(),
        ),
        InputAction::BackspaceSearch => (ACTION_BACKSPACE_SEARCH, 0),
    };
    (action_wparam(code, generation), value)
}

/// Decodes an action message. Whether its generation is still current is
/// [`HookFlags::action_is_current`]'s question.
#[must_use]
pub fn decode_action(wparam: usize, lparam: isize) -> Option<InputAction> {
    match wparam & ACTION_CODE_MASK {
        ACTION_SWITCH => i32::try_from(lparam).ok().map(InputAction::Switch),
        ACTION_ACTIVATE_POSITION => usize::try_from(lparam)
            .ok()
            .map(InputAction::ActivateVisiblePosition),
        ACTION_ALT_RELEASED => Some(InputAction::AltReleased),
        ACTION_RIGHT_BUTTON_PRESSED => Some(InputAction::RightButtonPressed),
        ACTION_RIGHT_BUTTON_RELEASED => Some(InputAction::RightButtonReleased),
        ACTION_MOUSE_WHEEL => i32::try_from(lparam).ok().map(InputAction::MouseWheel),
        ACTION_APPEND_SEARCH_CHARACTER => u32::try_from(lparam)
            .ok()
            .and_then(char::from_u32)
            .map(InputAction::AppendSearchCharacter),
        ACTION_BACKSPACE_SEARCH => Some(InputAction::BackspaceSearch),
        ACTION_NAVIGATE => i32::try_from(lparam).ok().map(InputAction::Navigate),
        ACTION_ACTIVATE_SELECTED => Some(InputAction::ActivateSelected),
        ACTION_SELECT_FIRST => Some(InputAction::SelectFirst),
        ACTION_SELECT_LAST => Some(InputAction::SelectLast),
        ACTION_DISMISS_OVERLAY => Some(InputAction::DismissOverlay),
        ACTION_CLOSE_SELECTED => Some(InputAction::CloseSelected),
        ACTION_WINDOW_COMMAND => u8::try_from(lparam)
            .ok()
            .and_then(WindowCommand::from_function_key)
            .map(InputAction::WindowCommand),
        _ => None,
    }
}

/// The interception flags and their generation in one word, so either thread reads or swaps
/// both at once. Every modal boundary and desktop recovery advances the generation.
#[derive(Default)]
pub struct HookFlags {
    value: AtomicUsize,
    recovery_pending: AtomicBool,
}

impl HookFlags {
    #[must_use]
    pub fn new() -> Self {
        Self::with_value(next_hook_generation())
    }

    fn with_value(value: usize) -> Self {
        Self {
            value: AtomicUsize::new(value),
            recovery_pending: AtomicBool::new(false),
        }
    }

    #[must_use]
    pub fn action_is_current(&self, wparam: usize) -> bool {
        let flags = self.load();
        !self.recovery_pending()
            && flags & INTERCEPTION_SUSPENDED == 0
            && (wparam >> 8) == ((flags >> 3) & ACTION_EPOCH_MASK)
    }

    /// Whether work begun under `generation`, as the hook last synchronized it, may still reach
    /// the UI: no desktop recovery is pending, no modal boundary has passed since, and
    /// interception was not suspended.
    #[must_use]
    pub fn generation_is_current(&self, generation: usize) -> bool {
        !self.recovery_pending()
            && self.load() & !OVERLAY_FLAGS == generation
            && generation & INTERCEPTION_SUSPENDED == 0
    }

    #[must_use]
    pub fn load(&self) -> usize {
        self.value.load(Ordering::Acquire)
    }

    pub fn set(&self, flag: usize, active: bool) {
        if active {
            self.value.fetch_or(flag, Ordering::AcqRel);
        } else {
            self.value.fetch_and(!flag, Ordering::AcqRel);
        }
    }

    pub fn suspend(&self, suspended: bool) {
        // Each modal boundary advances the generation, even if no callback ran in between.
        let _previous = self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                if (value & INTERCEPTION_SUSPENDED != 0) == suspended {
                    return None;
                }
                let flags = if suspended {
                    INTERCEPTION_SUSPENDED
                } else {
                    value & OVERLAY_FLAGS
                };
                Some(next_hook_generation() | flags)
            });
    }

    /// Invalidates queued actions without overwriting concurrent UI flag changes.
    pub fn advance_generation(&self) {
        let _previous = self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(next_hook_generation() | (value & FLAG_BITS))
            });
    }

    fn replace_modal_flags(&self, flags: usize) -> usize {
        // The closure always supplies a value, so fetch_update retries until it swaps the
        // complete snapshot. Entry and exit advance the generation even when already suspended.
        self.value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |_value| {
                Some(next_hook_generation() | (flags & FLAG_BITS))
            })
            .unwrap_or_else(|value| value)
            & FLAG_BITS
    }

    pub fn update_overlay(&self, generation: usize, active: bool, typed_search: bool) -> bool {
        let value = self.load();
        if self.recovery_pending()
            || value & !OVERLAY_FLAGS != generation
            || value & INTERCEPTION_SUSPENDED != 0
        {
            return false;
        }
        let flags = if active {
            OVERLAY_ACTIVE | if typed_search { SEARCH_ACTIVE } else { 0 }
        } else {
            0
        };
        // A single attempt keeps the hook callback bounded. A concurrent UI update wins.
        self.value
            .compare_exchange(
                value,
                generation | flags,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    #[must_use]
    pub fn recovery_pending(&self) -> bool {
        self.recovery_pending.load(Ordering::Acquire)
    }

    pub fn set_recovery_pending(&self, pending: bool) {
        self.recovery_pending.store(pending, Ordering::Release);
    }
}

/// Restores a modal scope's saved flags on drop. Nested guards must drop in reverse order.
#[must_use = "keep the guard alive for the entire modal call"]
pub struct HookInterceptionGuard {
    flags: Arc<HookFlags>,
    saved_flags: usize,
}

impl HookInterceptionGuard {
    pub fn new(flags: Arc<HookFlags>) -> Self {
        let saved_flags = flags.replace_modal_flags(INTERCEPTION_SUSPENDED);
        Self { flags, saved_flags }
    }
}

impl Drop for HookInterceptionGuard {
    fn drop(&mut self) {
        let _previous = self.flags.replace_modal_flags(self.saved_flags);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Key, decode_virtual_key};

    #[test]
    fn queued_destructive_action_cannot_cross_a_menu_or_hook_restart() {
        let flags = Arc::new(HookFlags::new());
        let stale = action_wparam(ACTION_WINDOW_COMMAND, flags.load());
        assert_eq!(
            decode_action(stale, 8),
            Some(InputAction::WindowCommand(WindowCommand::Terminate))
        );
        assert!(flags.action_is_current(stale));
        {
            let _menu = HookInterceptionGuard::new(Arc::clone(&flags));
            assert!(!flags.action_is_current(stale));
        }
        assert!(!flags.action_is_current(stale));
        let current = action_wparam(ACTION_WINDOW_COMMAND, flags.load());
        assert!(flags.action_is_current(current));
        assert!(!HookFlags::new().action_is_current(current));
    }

    #[test]
    fn action_epoch_encoding_and_validation_use_the_same_wrap_mask() {
        for generation in [0, 8, ACTION_EPOCH_MASK << 3, usize::MAX & !7] {
            let flags = HookFlags::with_value(generation);
            let action = action_wparam(ACTION_CLOSE_SELECTED, generation);
            assert_eq!(action & ACTION_CODE_MASK, ACTION_CLOSE_SELECTED);
            assert_eq!(action >> 8, (generation >> 3) & ACTION_EPOCH_MASK);
            assert!(flags.action_is_current(action));
            assert_eq!(decode_action(action, 0), Some(InputAction::CloseSelected));
        }
    }

    #[test]
    fn scoped_suspension_restores_every_flag_combination_with_new_generations() {
        for saved_flags in 0..=7 {
            let flags = Arc::new(HookFlags::with_value(saved_flags));
            let initial_generation = flags.load() & !7;
            let guard = HookInterceptionGuard::new(Arc::clone(&flags));
            let suspended = flags.load();
            assert_eq!(suspended & 7, INTERCEPTION_SUSPENDED);
            assert_ne!(suspended & !7, initial_generation);
            assert!(!flags.update_overlay(initial_generation, true, true));
            flags.set(SEARCH_ACTIVE, true);
            flags.set(OVERLAY_ACTIVE, true);
            drop(guard);
            assert_eq!(flags.load() & 7, saved_flags);
            assert_ne!(flags.load() & !7, suspended & !7);
            assert!(!flags.update_overlay(suspended & !OVERLAY_FLAGS, true, true));
        }
    }

    #[test]
    fn nested_scoped_suspension_restores_outer_state_without_resuming_it() {
        let flags = Arc::new(HookFlags::with_value(OVERLAY_FLAGS));
        let outer = HookInterceptionGuard::new(Arc::clone(&flags));
        flags.set(SEARCH_ACTIVE, true);
        {
            let _inner = HookInterceptionGuard::new(Arc::clone(&flags));
            assert_eq!(flags.load() & 7, INTERCEPTION_SUSPENDED);
            flags.set(OVERLAY_ACTIVE, true);
        }
        assert_eq!(flags.load() & 7, INTERCEPTION_SUSPENDED | SEARCH_ACTIVE);
        drop(outer);
        assert_eq!(flags.load() & 7, OVERLAY_FLAGS);
    }

    #[test]
    fn scoped_suspension_restores_flags_during_unwinding() {
        let flags = Arc::new(HookFlags::with_value(OVERLAY_FLAGS));
        let result = std::panic::catch_unwind(|| {
            let _guard = HookInterceptionGuard::new(Arc::clone(&flags));
            panic!("modal call failed");
        });
        assert!(result.is_err());
        assert_eq!(flags.load() & 7, OVERLAY_FLAGS);
    }

    #[test]
    fn navigation_actions_decode_in_both_directions() {
        assert_eq!(
            decode_action(ACTION_NAVIGATE, -1),
            Some(InputAction::Navigate(-1))
        );
        assert_eq!(
            decode_action(ACTION_NAVIGATE, 1),
            Some(InputAction::Navigate(1))
        );
    }

    #[test]
    fn enter_virtual_key_and_activation_message_map_to_the_new_input_event() {
        assert_eq!(decode_virtual_key(0x0D), Key::Enter);
        assert_eq!(
            decode_action(ACTION_ACTIVATE_SELECTED, 0),
            Some(InputAction::ActivateSelected)
        );
    }

    #[test]
    fn boundary_actions_round_trip_through_hook_message_payloads() {
        assert_eq!(
            decode_action(ACTION_SELECT_FIRST, 0),
            Some(InputAction::SelectFirst)
        );
        assert_eq!(
            decode_action(ACTION_SELECT_LAST, 0),
            Some(InputAction::SelectLast)
        );
    }

    #[test]
    fn dismiss_overlay_action_decodes_for_the_ui_thread() {
        assert_eq!(
            decode_action(ACTION_DISMISS_OVERLAY, 0),
            Some(InputAction::DismissOverlay)
        );
    }

    #[test]
    fn f4_maps_to_close_selected_across_the_hook_message_boundary() {
        assert_eq!(decode_virtual_key(0x73), Key::F4);
        assert_eq!(
            decode_action(ACTION_CLOSE_SELECTED, 0),
            Some(InputAction::CloseSelected)
        );
    }

    #[test]
    fn window_command_actions_round_trip_through_hook_message_payloads() {
        for command in [
            WindowCommand::Minimize,
            WindowCommand::Maximize,
            WindowCommand::Restore,
            WindowCommand::Terminate,
            WindowCommand::Run,
        ] {
            assert_eq!(
                decode_action(
                    ACTION_WINDOW_COMMAND,
                    command.function_key().map_or(0, isize::from)
                ),
                Some(InputAction::WindowCommand(command))
            );
        }
    }

    #[test]
    fn every_posted_action_decodes_back_with_its_generation() {
        let flags = HookFlags::new();
        for action in [
            InputAction::Switch(-1),
            InputAction::Navigate(1),
            InputAction::ActivateSelected,
            InputAction::SelectFirst,
            InputAction::SelectLast,
            InputAction::DismissOverlay,
            InputAction::CloseSelected,
            InputAction::WindowCommand(WindowCommand::Close),
            InputAction::WindowCommand(WindowCommand::Run),
            InputAction::ActivateVisiblePosition(9),
            InputAction::AltReleased,
            InputAction::RightButtonPressed,
            InputAction::RightButtonReleased,
            InputAction::MouseWheel(-1),
            InputAction::AppendSearchCharacter('é'),
            InputAction::BackspaceSearch,
        ] {
            let (wparam, lparam) = encode_action(action, flags.load());
            assert_eq!(decode_action(wparam, lparam), Some(action));
            assert!(flags.action_is_current(wparam));
        }
    }

    #[test]
    fn generation_stays_current_until_recovery_a_boundary_or_suspension() {
        let flags = HookFlags::with_value(8 | OVERLAY_FLAGS);
        assert!(flags.generation_is_current(8));
        flags.set(SEARCH_ACTIVE, false);
        assert!(
            flags.generation_is_current(8),
            "UI flags are not a boundary"
        );
        assert!(!flags.generation_is_current(16));
        flags.set_recovery_pending(true);
        assert!(!flags.generation_is_current(8));
        flags.set_recovery_pending(false);
        flags.suspend(true);
        assert!(!flags.generation_is_current(8));
        assert!(!flags.generation_is_current(flags.load() & !OVERLAY_FLAGS));
    }

    #[test]
    fn advancing_the_generation_keeps_the_ui_flags_and_retires_queued_actions() {
        // The last epoch before the counter wraps, so no generation handed out in tests repeats it.
        let generation = ACTION_EPOCH_MASK << 3;
        for saved_flags in 0..=7 {
            let flags = HookFlags::with_value(generation | saved_flags);
            let (queued, _) = encode_action(InputAction::AltReleased, flags.load());
            assert_eq!(flags.action_is_current(queued), saved_flags & 1 == 0);
            flags.advance_generation();
            assert_eq!(flags.load() & 7, saved_flags);
            assert_ne!(flags.load() & !7, generation);
            assert!(!flags.action_is_current(queued));
        }
    }
}
