//! Interception flags shared with the UI thread, and the action messages stamped with their
//! generation.

use alttabio::input::InputAction;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

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
pub(super) const INTERCEPTION_SUSPENDED: usize = 1;
pub(super) const SEARCH_ACTIVE: usize = 2;
pub(super) const OVERLAY_ACTIVE: usize = 4;
pub(super) const OVERLAY_FLAGS: usize = SEARCH_ACTIVE | OVERLAY_ACTIVE;
const ACTION_CODE_MASK: usize = 0xFF;
const ACTION_EPOCH_MASK: usize = usize::MAX >> 8;
static NEXT_HOOK_EPOCH: AtomicUsize = AtomicUsize::new(1);

pub(super) fn next_hook_generation() -> usize {
    (NEXT_HOOK_EPOCH.fetch_add(1, Ordering::Relaxed) & ACTION_EPOCH_MASK) << 3
}

const fn action_wparam(code: usize, generation: usize) -> WPARAM {
    WPARAM((((generation >> 3) & ACTION_EPOCH_MASK) << 8) | (code & ACTION_CODE_MASK))
}

#[derive(Default)]
pub(super) struct HookFlags {
    pub(super) value: AtomicUsize,
    pub(super) recovery_pending: AtomicBool,
}

impl HookFlags {
    pub(super) fn new() -> Self {
        Self::with_value(next_hook_generation())
    }

    fn with_value(value: usize) -> Self {
        Self {
            value: AtomicUsize::new(value),
            recovery_pending: AtomicBool::new(false),
        }
    }

    pub(super) fn action_is_current(&self, wparam: WPARAM) -> bool {
        let flags = self.load();
        !self.recovery_pending.load(Ordering::Acquire)
            && flags & INTERCEPTION_SUSPENDED == 0
            && (wparam.0 >> 8) == ((flags >> 3) & ACTION_EPOCH_MASK)
    }

    pub(super) fn load(&self) -> usize {
        self.value.load(Ordering::Acquire)
    }

    pub(super) fn set(&self, flag: usize, active: bool) {
        if active {
            self.value.fetch_or(flag, Ordering::AcqRel);
        } else {
            self.value.fetch_and(!flag, Ordering::AcqRel);
        }
    }

    pub(super) fn suspend(&self, suspended: bool) {
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

    fn replace_modal_flags(&self, flags: usize) -> usize {
        // The closure always supplies a value, so fetch_update retries until it swaps the
        // complete snapshot. Entry and exit advance the generation even when already suspended.
        self.value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |_value| {
                Some(next_hook_generation() | (flags & 7))
            })
            .unwrap_or_else(|value| value)
            & 7
    }

    pub(super) fn update_overlay(
        &self,
        generation: usize,
        active: bool,
        typed_search: bool,
    ) -> bool {
        let value = self.load();
        if self.recovery_pending.load(Ordering::Acquire)
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
}

/// Restores a modal scope's saved flags on drop. Nested guards must drop in reverse order.
#[must_use = "keep the guard alive for the entire modal call"]
pub struct HookInterceptionGuard {
    flags: Arc<HookFlags>,
    saved_flags: usize,
}

impl HookInterceptionGuard {
    pub(super) fn new(flags: Arc<HookFlags>) -> Self {
        let saved_flags = flags.replace_modal_flags(INTERCEPTION_SUSPENDED);
        Self { flags, saved_flags }
    }
}

impl Drop for HookInterceptionGuard {
    fn drop(&mut self) {
        let _previous = self.flags.replace_modal_flags(self.saved_flags);
    }
}

pub fn decode_action(wparam: WPARAM, lparam: LPARAM) -> Option<InputAction> {
    match wparam.0 & ACTION_CODE_MASK {
        ACTION_SWITCH => i32::try_from(lparam.0).ok().map(InputAction::Switch),
        ACTION_ACTIVATE_POSITION => usize::try_from(lparam.0)
            .ok()
            .map(InputAction::ActivateVisiblePosition),
        ACTION_ALT_RELEASED => Some(InputAction::AltReleased),
        ACTION_RIGHT_BUTTON_PRESSED => Some(InputAction::RightButtonPressed),
        ACTION_RIGHT_BUTTON_RELEASED => Some(InputAction::RightButtonReleased),
        ACTION_MOUSE_WHEEL => i32::try_from(lparam.0).ok().map(InputAction::MouseWheel),
        ACTION_APPEND_SEARCH_CHARACTER => u32::try_from(lparam.0)
            .ok()
            .and_then(char::from_u32)
            .map(InputAction::AppendSearchCharacter),
        ACTION_BACKSPACE_SEARCH => Some(InputAction::BackspaceSearch),
        ACTION_NAVIGATE => i32::try_from(lparam.0).ok().map(InputAction::Navigate),
        ACTION_ACTIVATE_SELECTED => Some(InputAction::ActivateSelected),
        ACTION_SELECT_FIRST => Some(InputAction::SelectFirst),
        ACTION_SELECT_LAST => Some(InputAction::SelectLast),
        ACTION_DISMISS_OVERLAY => Some(InputAction::DismissOverlay),
        ACTION_CLOSE_SELECTED => Some(InputAction::CloseSelected),
        ACTION_WINDOW_COMMAND => u8::try_from(lparam.0)
            .ok()
            .and_then(alttabio::input::WindowCommand::from_function_key)
            .map(InputAction::WindowCommand),
        _ => None,
    }
}

pub(super) fn post_action_message(
    target: HWND,
    action: InputAction,
    generation: usize,
    message: u32,
) -> bool {
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
    unsafe {
        // SAFETY: `target` is the UI HWND supplied at hook creation; PostMessageW copies the two
        // integer payloads and retains no Rust references.
        PostMessageW(
            Some(target),
            message,
            action_wparam(code, generation),
            LPARAM(value),
        )
    }
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::decode_virtual_key;
    use alttabio::input::Key;
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_F4, VK_RETURN};

    #[test]
    fn queued_destructive_action_cannot_cross_a_menu_or_hook_restart() {
        let flags = Arc::new(HookFlags::new());
        let stale = action_wparam(ACTION_WINDOW_COMMAND, flags.load());
        assert_eq!(
            decode_action(stale, LPARAM(8)),
            Some(InputAction::WindowCommand(
                alttabio::input::WindowCommand::Terminate
            ))
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
            assert_eq!(action.0 & ACTION_CODE_MASK, ACTION_CLOSE_SELECTED);
            assert_eq!(action.0 >> 8, (generation >> 3) & ACTION_EPOCH_MASK);
            assert!(flags.action_is_current(action));
            assert_eq!(
                decode_action(action, LPARAM(0)),
                Some(InputAction::CloseSelected)
            );
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
            decode_action(WPARAM(ACTION_NAVIGATE), LPARAM(-1)),
            Some(InputAction::Navigate(-1))
        );
        assert_eq!(
            decode_action(WPARAM(ACTION_NAVIGATE), LPARAM(1)),
            Some(InputAction::Navigate(1))
        );
    }

    #[test]
    fn enter_virtual_key_and_activation_message_map_to_the_new_input_event() {
        assert_eq!(decode_virtual_key(u32::from(VK_RETURN.0)), Key::Enter);
        assert_eq!(
            decode_action(WPARAM(ACTION_ACTIVATE_SELECTED), LPARAM(0)),
            Some(InputAction::ActivateSelected)
        );
    }

    #[test]
    fn boundary_actions_round_trip_through_hook_message_payloads() {
        assert_eq!(
            decode_action(WPARAM(ACTION_SELECT_FIRST), LPARAM(0)),
            Some(InputAction::SelectFirst)
        );
        assert_eq!(
            decode_action(WPARAM(ACTION_SELECT_LAST), LPARAM(0)),
            Some(InputAction::SelectLast)
        );
    }

    #[test]
    fn dismiss_overlay_action_decodes_for_the_ui_thread() {
        assert_eq!(
            decode_action(WPARAM(ACTION_DISMISS_OVERLAY), LPARAM(0)),
            Some(InputAction::DismissOverlay)
        );
    }

    #[test]
    fn f4_maps_to_close_selected_across_the_hook_message_boundary() {
        assert_eq!(decode_virtual_key(u32::from(VK_F4.0)), Key::F4);
        assert_eq!(
            decode_action(WPARAM(ACTION_CLOSE_SELECTED), LPARAM(0)),
            Some(InputAction::CloseSelected)
        );
    }

    #[test]
    fn window_command_actions_round_trip_through_hook_message_payloads() {
        for command in [
            alttabio::input::WindowCommand::Minimize,
            alttabio::input::WindowCommand::Maximize,
            alttabio::input::WindowCommand::Restore,
            alttabio::input::WindowCommand::Terminate,
            alttabio::input::WindowCommand::Run,
        ] {
            assert_eq!(
                decode_action(
                    WPARAM(ACTION_WINDOW_COMMAND),
                    LPARAM(command.function_key().map_or(0, isize::from))
                ),
                Some(InputAction::WindowCommand(command))
            );
        }
    }
}
