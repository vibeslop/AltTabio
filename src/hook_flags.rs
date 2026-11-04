//! What the input hook thread shares with the UI thread: interception flags under a generation,
//! and action messages stamped with that generation so the UI can drop actions queued before a
//! modal or input-desktop boundary.

use crate::input::InputAction;
use crate::window_command::WindowCommand;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};

pub const INTERCEPTION_SUSPENDED: usize = 1;
pub const SEARCH_ACTIVE: usize = 2;
pub const OVERLAY_ACTIVE: usize = 4;
pub const FILTER_VISIBLE: usize = 8;
pub const OVERLAY_FLAGS: usize = SEARCH_ACTIVE | OVERLAY_ACTIVE | FILTER_VISIBLE;
// Generations are multiples of 16, so the flags share their word.
const FLAG_BITS: usize = INTERCEPTION_SUSPENDED | OVERLAY_FLAGS;
const GENERATION_SHIFT: usize = 4;
// UI visibility is distinct from capture armed for an opening that has not reached the UI.
const OVERLAY_VISIBLE: u64 = 1 << 4;
const OPENING_STEP: u64 = 1 << 5;
const OPENING_MASK: u64 = (1 << 32) - OPENING_STEP;
const STATE_EPOCH_SHIFT: u32 = 32;

const ACTION_SWITCH: usize = 1;
const ACTION_MOUSE_WHEEL: usize = 4;
const ACTION_MOUSE_WHEEL_RELEASED: usize = 5;
const ACTION_RIGHT_BUTTON_PRESSED: usize = 6;
const ACTION_ACTIVATE_POSITION: usize = 2;
const ACTION_ALT_RELEASED: usize = 3;
const ACTION_APPEND_SEARCH_CHARACTER: usize = 8;
const ACTION_BACKSPACE_SEARCH: usize = 9;
const ACTION_NAVIGATE: usize = 10;
const ACTION_ACTIVATE_SELECTED: usize = 11;
const ACTION_SELECT_FIRST: usize = 12;
const ACTION_SELECT_LAST: usize = 13;
const ACTION_DISMISS_OVERLAY: usize = 14;
const ACTION_CLOSE_SELECTED: usize = 15;
const ACTION_WINDOW_COMMAND: usize = 16;
const ACTION_EDIT_SEARCH: usize = 17;
const ACTION_NUMBER_KEY: usize = 18;
const ACTION_OVERLAY_WHEEL: usize = 19;
const ACTION_CYCLE_SELECTION: usize = 20;
const ACTION_SEARCH_NAVIGATION: usize = 21;
const ACTION_CODE_MASK: usize = 0xFF;
// Keep the epoch within 32 bits so an epoch and opening count fit one atomic word.
const ACTION_EPOCH_MASK: usize = (usize::MAX >> 8) & u32::MAX as usize;
static NEXT_HOOK_EPOCH: AtomicUsize = AtomicUsize::new(1);

fn next_hook_generation() -> usize {
    (NEXT_HOOK_EPOCH.fetch_add(1, Ordering::Relaxed) & ACTION_EPOCH_MASK) << GENERATION_SHIFT
}

const fn action_wparam(code: usize, generation: usize) -> usize {
    (((generation >> GENERATION_SHIFT) & ACTION_EPOCH_MASK) << 8) | (code & ACTION_CODE_MASK)
}

/// Encodes an action as the `(wparam, lparam)` payload of a message stamped with `generation`.
#[must_use]
pub fn encode_action(action: InputAction, generation: usize) -> (usize, isize) {
    let (code, value) = match action {
        InputAction::Switch(delta) => (ACTION_SWITCH, delta as isize),
        InputAction::CycleSelection(delta) => (ACTION_CYCLE_SELECTION, delta as isize),
        InputAction::OverlayWheel(delta) => (ACTION_OVERLAY_WHEEL, delta as isize),
        InputAction::MouseWheel(delta) => (ACTION_MOUSE_WHEEL, delta as isize),
        InputAction::MouseWheelReleased => (ACTION_MOUSE_WHEEL_RELEASED, 0),
        InputAction::RightButtonPressed => (ACTION_RIGHT_BUTTON_PRESSED, 0),
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
        InputAction::NumberKey { position, text } => {
            (ACTION_NUMBER_KEY, encode_number_key(position, text))
        }
        InputAction::AltReleased => (ACTION_ALT_RELEASED, 0),
        InputAction::AppendSearchCharacter(character) => (
            ACTION_APPEND_SEARCH_CHARACTER,
            isize::try_from(u32::from(character)).unwrap_or_default(),
        ),
        InputAction::BackspaceSearch => (ACTION_BACKSPACE_SEARCH, 0),
        InputAction::EditSearch(command) => (ACTION_EDIT_SEARCH, isize::from(command as u8)),
        InputAction::SearchNavigation { command, modified } => (
            ACTION_SEARCH_NAVIGATION,
            isize::from(command as u8) | (isize::from(modified) << 8),
        ),
    };
    (action_wparam(code, generation), value)
}

/// Decodes an action message. Whether its generation is still current is
/// [`HookFlags::action_is_current`]'s question.
#[must_use]
pub fn decode_action(wparam: usize, lparam: isize) -> Option<InputAction> {
    match wparam & ACTION_CODE_MASK {
        ACTION_SWITCH => i32::try_from(lparam).ok().map(InputAction::Switch),
        ACTION_CYCLE_SELECTION => i32::try_from(lparam).ok().map(InputAction::CycleSelection),
        ACTION_OVERLAY_WHEEL => i32::try_from(lparam).ok().map(InputAction::OverlayWheel),
        ACTION_MOUSE_WHEEL => i32::try_from(lparam).ok().map(InputAction::MouseWheel),
        ACTION_MOUSE_WHEEL_RELEASED => Some(InputAction::MouseWheelReleased),
        ACTION_RIGHT_BUTTON_PRESSED => Some(InputAction::RightButtonPressed),
        ACTION_ACTIVATE_POSITION => usize::try_from(lparam)
            .ok()
            .map(InputAction::ActivateVisiblePosition),
        ACTION_NUMBER_KEY => decode_number_key(lparam),
        ACTION_ALT_RELEASED => Some(InputAction::AltReleased),
        ACTION_APPEND_SEARCH_CHARACTER => u32::try_from(lparam)
            .ok()
            .and_then(char::from_u32)
            .map(InputAction::AppendSearchCharacter),
        ACTION_BACKSPACE_SEARCH => Some(InputAction::BackspaceSearch),
        ACTION_EDIT_SEARCH => u8::try_from(lparam)
            .ok()
            .and_then(crate::text_edit::TextCommand::from_code)
            .map(InputAction::EditSearch),
        ACTION_SEARCH_NAVIGATION => {
            let value = u16::try_from(lparam).ok()?;
            if value & !0x01FF != 0 {
                return None;
            }
            Some(InputAction::SearchNavigation {
                command: crate::text_edit::TextCommand::from_code(
                    u8::try_from(value & 0xFF).ok()?,
                )?,
                modified: value & 0x0100 != 0,
            })
        }
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

// Four bits hold positions 1-9; the remaining 21 bits hold a Unicode scalar plus one, with zero
// meaning no translated text. The complete payload fits a signed 32-bit message parameter.
fn encode_number_key(position: usize, text: Option<char>) -> isize {
    let Ok(position @ 1..=9) = u32::try_from(position) else {
        return -1;
    };
    let value = (text.map_or(0, |character| u32::from(character) + 1) << 4) | position;
    isize::try_from(value).unwrap_or(-1)
}

fn decode_number_key(value: isize) -> Option<InputAction> {
    let value = u32::try_from(value).ok()?;
    let position = usize::try_from(value & 0xF).ok()?;
    if !(1..=9).contains(&position) {
        return None;
    }
    let text = match value >> 4 {
        0 => None,
        code => Some(char::from_u32(code - 1)?),
    };
    Some(InputAction::NumberKey { position, text })
}

fn pack_state(snapshot: usize) -> u64 {
    ((((snapshot >> GENERATION_SHIFT) & ACTION_EPOCH_MASK) as u64) << STATE_EPOCH_SHIFT)
        | (snapshot & FLAG_BITS) as u64
}

fn state_snapshot(value: u64) -> usize {
    (usize::try_from(value >> STATE_EPOCH_SHIFT).unwrap_or_default() << GENERATION_SHIFT)
        | usize::try_from(value & FLAG_BITS as u64).unwrap_or_default()
}

fn completed_state(value: u64) -> u64 {
    let value = value & !(OVERLAY_VISIBLE | FILTER_VISIBLE as u64);
    if value & OPENING_MASK == 0 {
        value & !(OVERLAY_FLAGS as u64)
    } else {
        value
    }
}

/// Epoch boundaries discard pending input; only actual UI visibility can retain capture.
fn visible_ui_state(value: u64) -> u64 {
    value
        & (INTERCEPTION_SUSPENDED as u64
            | if value & OVERLAY_VISIBLE != 0 {
                OVERLAY_VISIBLE | OVERLAY_FLAGS as u64
            } else {
                0
            })
}

/// Capture flags, UI visibility, pending-opening count, and action epoch share one atomic word. Publishing an opening
/// and hiding a completed session therefore cannot expose a false inactive state to input.
/// The public snapshot retains the action encoding's epoch-and-flags layout.
#[derive(Default)]
pub struct HookFlags {
    value: AtomicU64,
    recovery_pending: AtomicBool,
}

impl HookFlags {
    #[must_use]
    pub fn new() -> Self {
        Self::with_value(next_hook_generation())
    }

    fn with_value(value: usize) -> Self {
        Self {
            value: AtomicU64::new(pack_state(value)),
            recovery_pending: AtomicBool::new(false),
        }
    }

    #[must_use]
    pub fn action_is_current(&self, wparam: usize) -> bool {
        let flags = self.load();
        !self.recovery_pending()
            && flags & INTERCEPTION_SUSPENDED == 0
            && (wparam >> 8) == ((flags >> GENERATION_SHIFT) & ACTION_EPOCH_MASK)
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
        state_snapshot(self.value.load(Ordering::Acquire))
    }

    pub fn set(&self, flag: usize, active: bool) {
        let flag = (flag & FLAG_BITS) as u64
            | if flag & OVERLAY_ACTIVE != 0 {
                OVERLAY_VISIBLE
            } else {
                0
            };
        if active {
            self.value.fetch_or(flag, Ordering::AcqRel);
        } else {
            self.value.fetch_and(!flag, Ordering::AcqRel);
        }
    }

    /// Publishes UI visibility and derives capture atomically, retaining accepted openings.
    pub fn sync_overlay(&self, visible: bool, typed_search: bool, filtered: bool) {
        let _previous = self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                let capture = visible || value & OPENING_MASK != 0;
                let flags = if capture { OVERLAY_ACTIVE } else { 0 }
                    | if capture && typed_search {
                        SEARCH_ACTIVE
                    } else {
                        0
                    }
                    | if visible && typed_search && filtered {
                        FILTER_VISIBLE
                    } else {
                        0
                    };
                Some(
                    (value & !(OVERLAY_VISIBLE | OVERLAY_FLAGS as u64))
                        | flags as u64
                        | if visible { OVERLAY_VISIBLE } else { 0 },
                )
            });
    }

    /// Register and arm together before posting. Retire each opening when applied or discarded.
    #[must_use]
    pub fn begin_opening(&self, generation: usize, typed_search: bool) -> bool {
        self.update_once(|value| self.opening_state(value, generation, typed_search))
    }

    fn opening_state(&self, value: u64, generation: usize, typed_search: bool) -> Option<u64> {
        let snapshot = state_snapshot(value);
        if self.recovery_pending()
            || snapshot & !OVERLAY_FLAGS != generation
            || snapshot & INTERCEPTION_SUSPENDED != 0
            || value & OPENING_MASK == OPENING_MASK
        {
            return None;
        }
        let flags = OVERLAY_ACTIVE
            | if typed_search {
                SEARCH_ACTIVE | (snapshot & FILTER_VISIBLE)
            } else {
                0
            };
        Some(((value + OPENING_STEP) & !(OVERLAY_FLAGS as u64)) | flags as u64)
    }

    /// Callback publication makes one attempt; concurrent UI work wins without a retry loop.
    fn update_once(&self, update: impl FnOnce(u64) -> Option<u64>) -> bool {
        let value = self.value.load(Ordering::Acquire);
        let Some(updated) = update(value) else {
            return false;
        };
        self.value
            .compare_exchange(value, updated, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Withdraw failed posts on the hook message loop, outside native input callbacks.
    /// Stale cleanup cannot retire a new epoch's openings or clear its interception flags.
    pub fn withdraw_openings(&self, generation: usize, count: usize) {
        let count = u64::try_from(count).unwrap_or(u64::MAX);
        let _previous = self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                if state_snapshot(value) & !FLAG_BITS != generation & !FLAG_BITS {
                    return None;
                }
                let pending = ((value & OPENING_MASK) / OPENING_STEP).saturating_sub(count);
                let value = (value & !OPENING_MASK) | (pending * OPENING_STEP);
                Some(if pending == 0 && value & OVERLAY_VISIBLE == 0 {
                    value & !(OVERLAY_FLAGS as u64)
                } else {
                    value
                })
            });
    }

    pub fn retire_action_openings(&self, wparam: usize, count: usize) {
        self.retire_openings((wparam >> 8) << GENERATION_SHIFT, count);
    }

    pub fn retire_openings(&self, generation: usize, count: usize) {
        let count = u64::try_from(count).unwrap_or(u64::MAX);
        let _previous = self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                if state_snapshot(value) & !FLAG_BITS != generation & !FLAG_BITS {
                    return None;
                }
                let pending = (value & OPENING_MASK) / OPENING_STEP;
                Some((value & !OPENING_MASK) | (pending.saturating_sub(count) * OPENING_STEP))
            });
    }

    /// Hide the completed session without disarming a later opening waiting on the UI.
    pub fn complete_overlay(&self) {
        let _previous = self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                // UI visibility still changes during recovery. Derive from the latest word,
                // retaining its epoch and any subsequently accepted opening receipts.
                Some(completed_state(value))
            });
    }

    pub fn cancel_overlay(&self) {
        let _previous = self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(pack_state(next_hook_generation()) | (value & INTERCEPTION_SUSPENDED as u64))
            });
    }

    /// Publishes capture and query presence together without changing the action generation.
    pub fn set_search_state(&self, active: bool, filtered: bool) {
        let search = if active { SEARCH_ACTIVE } else { 0 }
            | if active && filtered {
                FILTER_VISIBLE
            } else {
                0
            };
        let _previous = self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some((value & !((SEARCH_ACTIVE | FILTER_VISIBLE) as u64)) | search as u64)
            });
    }

    pub fn suspend(&self, suspended: bool) {
        // Each modal boundary advances the generation, even if no callback ran in between.
        let _previous = self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                if (value & INTERCEPTION_SUSPENDED as u64 != 0) == suspended {
                    return None;
                }
                let state = if suspended {
                    INTERCEPTION_SUSPENDED as u64
                } else {
                    visible_ui_state(value) & !(INTERCEPTION_SUSPENDED as u64)
                };
                Some(pack_state(next_hook_generation()) | state)
            });
    }

    /// Invalidates queued openings and retains capture only for a visible UI session.
    pub fn advance_generation(&self) {
        let _previous = self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(pack_state(next_hook_generation()) | visible_ui_state(value))
            });
    }

    fn replace_modal_flags(&self, flags: u64) -> u64 {
        // The closure always supplies a value, so fetch_update retries until it swaps the
        // complete snapshot. Entry and exit advance the generation even when already suspended.
        let previous = self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |_value| {
                Some(pack_state(next_hook_generation()) | visible_ui_state(flags))
            })
            .unwrap_or_else(|value| value);
        visible_ui_state(previous)
    }

    pub fn update_overlay(&self, generation: usize, active: bool, typed_search: bool) -> bool {
        let value = self.value.load(Ordering::Acquire);
        let snapshot = state_snapshot(value);
        if self.recovery_pending()
            || snapshot & !OVERLAY_FLAGS != generation
            || snapshot & INTERCEPTION_SUSPENDED != 0
            || (!active && value & (OPENING_MASK | OVERLAY_VISIBLE) != 0)
        {
            return false;
        }
        let flags = if active {
            OVERLAY_ACTIVE
                | if typed_search {
                    SEARCH_ACTIVE | (snapshot & FILTER_VISIBLE)
                } else {
                    0
                }
        } else {
            0
        };
        // A single attempt keeps the hook callback bounded. A concurrent UI update wins.
        self.value
            .compare_exchange(
                value,
                (value & !(OVERLAY_FLAGS as u64)) | flags as u64,
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

/// Validates each queued action and retains any opening receipt throughout UI application.
#[must_use = "keep the guard alive until the action finishes applying"]
pub struct HookActionGuard {
    flags: Arc<HookFlags>,
    origin: usize,
    opening: bool,
}

impl HookActionGuard {
    pub fn new(flags: Arc<HookFlags>, origin: usize, action: InputAction) -> Option<Self> {
        let opening = matches!(action, InputAction::Switch(_) | InputAction::MouseWheel(_));
        if !flags.action_is_current(origin) {
            if opening {
                flags.retire_action_openings(origin, 1);
            }
            return None;
        }
        Some(Self {
            flags,
            origin,
            opening,
        })
    }
}

impl Drop for HookActionGuard {
    fn drop(&mut self) {
        if self.opening {
            self.flags.retire_action_openings(self.origin, 1);
        }
    }
}

/// Restores a modal scope's saved flags on drop. Nested guards must drop in reverse order.
#[must_use = "keep the guard alive for the entire modal call"]
pub struct HookInterceptionGuard {
    flags: Arc<HookFlags>,
    saved_flags: u64,
}

impl HookInterceptionGuard {
    pub fn new(flags: Arc<HookFlags>) -> Self {
        let saved_flags = flags.replace_modal_flags(INTERCEPTION_SUSPENDED as u64);
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

    #[test]
    fn modal_restoration_discards_capture_from_an_invalidated_hidden_opening() {
        use crate::input::{HookSettings, HookState, KeyEvent, Modifiers, MouseEvent};
        let flags = Arc::new(HookFlags::new());
        let generation = flags.load();
        assert!(flags.begin_opening(generation, true));
        let (origin, _) = encode_action(InputAction::MouseWheel(1), generation);
        {
            let _modal = HookInterceptionGuard::new(Arc::clone(&flags));
            assert!(!flags.action_is_current(origin));
        }
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
        let mut hook = HookState::default();
        let capture = flags.load();
        hook.set_overlay_active(capture & OVERLAY_ACTIVE != 0);
        let settings = HookSettings {
            search_active: capture & SEARCH_ACTIVE != 0,
            ..HookSettings::default()
        };
        assert!(
            !hook
                .process_key(
                    KeyEvent::pressed(Key::Other(0x41), Modifiers::default()).with_text('a'),
                    settings
                )
                .suppress
        );
        assert!(
            !hook
                .process_mouse(MouseEvent::Wheel(-120), settings)
                .suppress
        );
    }

    #[test]
    fn resumed_visible_panel_keeps_capture_through_recovery_and_failed_post_cleanup() {
        for recovery in [false, true] {
            let flags = HookFlags::new();
            flags.sync_overlay(true, true, true);
            flags.suspend(true);
            flags.sync_overlay(true, true, true);
            flags.suspend(false);
            if recovery {
                flags.advance_generation();
            }
            assert_eq!(flags.load() & OVERLAY_FLAGS, OVERLAY_FLAGS);
            let generation = flags.load() & !OVERLAY_FLAGS;
            assert!(flags.begin_opening(generation, true));
            flags.withdraw_openings(generation, 1);
            assert_eq!(flags.load() & OVERLAY_FLAGS, OVERLAY_FLAGS);
            flags.complete_overlay();
            assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
        }
    }

    #[test]
    fn completion_during_recovery_clears_visibility_but_keeps_unapplied_ownership_until_invalidated()
     {
        for pending in [false, true] {
            let flags = HookFlags::new();
            flags.sync_overlay(true, true, true);
            let generation = flags.load() & !OVERLAY_FLAGS;
            if pending {
                assert!(flags.begin_opening(generation, true));
            }
            flags.set_recovery_pending(true);
            flags.complete_overlay();
            assert_eq!(flags.value.load(Ordering::Acquire) & OVERLAY_VISIBLE, 0);
            assert_eq!(
                flags.load() & OVERLAY_FLAGS,
                if pending {
                    OVERLAY_ACTIVE | SEARCH_ACTIVE
                } else {
                    0
                }
            );
            flags.advance_generation();
            flags.set_recovery_pending(false);
            assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
        }
    }

    #[test]
    fn completion_clears_old_query_presence_for_a_queued_mouse_opening() {
        use crate::input::{HookSettings, HookState, Key, KeyEvent, Modifiers, MouseEvent};
        let flags = HookFlags::new();
        flags.sync_overlay(true, true, true);
        let generation = flags.load() & !OVERLAY_FLAGS;
        let mut hook = HookState::default();
        let settings = HookSettings {
            right_button_wheel_switching: true,
            ..HookSettings::default()
        };
        hook.set_overlay_active(true);
        let _press = hook.process_mouse(MouseEvent::RightButtonPressed, settings);
        let _wheel = hook.process_mouse(MouseEvent::Wheel(-120), settings);
        assert!(flags.begin_opening(generation, true));
        // The searched session completes before the queued fresh session reaches the UI.
        flags.complete_overlay();
        let capture = flags.load();
        assert_eq!(capture & FILTER_VISIBLE, 0);
        hook.set_overlay_active(capture & OVERLAY_ACTIVE != 0);
        let left = hook.process_key(
            KeyEvent::pressed(Key::LeftArrow, Modifiers::default()),
            HookSettings {
                search_active: capture & SEARCH_ACTIVE != 0,
                filter_visible: capture & FILTER_VISIBLE != 0,
                ..settings
            },
        );
        let action = InputAction::SearchNavigation {
            command: crate::text_edit::TextCommand::Left,
            modified: false,
        };
        assert_eq!(left.actions().next(), Some(action));
        let mut session =
            crate::switcher::SwitcherSession::new(crate::switcher::SwitcherSessionSettings {
                typed_search: true,
                release_alt_switches: true,
            });
        session.open(
            [
                crate::switcher::SwitchTask::new(1, 10, "One", "one"),
                crate::switcher::SwitchTask::new(2, 20, "Two", "two"),
            ],
            Some(1),
        );
        assert_eq!(
            session.handle_input(action),
            crate::switcher::SwitcherEffect::Redraw
        );
        assert_eq!(
            session
                .switcher()
                .selected_task()
                .map(|task| task.window_handle),
            Some(10)
        );
    }

    #[test]
    fn hidden_ui_sync_keeps_an_accepted_mouse_opening_armed() {
        use crate::hook_delivery::{Delivery, deliver};
        use crate::input::{HookSettings, HookState, MouseEvent};
        for typed_search in [false, true] {
            let flags = Arc::new(HookFlags::new());
            let generation = flags.load();
            let settings = HookSettings {
                typed_search,
                right_button_wheel_switching: true,
                ..HookSettings::default()
            };
            let mut hook = HookState::default();
            let _press = hook.process_mouse(MouseEvent::RightButtonPressed, settings);
            let opening = hook.process_mouse(MouseEvent::Wheel(-120), settings);
            assert_eq!(
                deliver(&flags, generation, typed_search, opening, |_| true),
                Delivery::Posted
            );
            // Settings closes before the accepted opening reaches the UI.
            flags.sync_overlay(false, typed_search, false);
            let expected = OVERLAY_ACTIVE | if typed_search { SEARCH_ACTIVE } else { 0 };
            assert_eq!(flags.load() & OVERLAY_FLAGS, expected);
            hook.set_overlay_active(flags.load() & OVERLAY_ACTIVE != 0);
            let release = hook.process_mouse(MouseEvent::RightButtonReleased, settings);
            assert!(release.suppress);
            assert_eq!(
                release.actions().next(),
                Some(InputAction::MouseWheelReleased)
            );
            let (origin, _) = encode_action(InputAction::MouseWheel(1), generation);
            let Some(applying) =
                HookActionGuard::new(Arc::clone(&flags), origin, InputAction::MouseWheel(1))
            else {
                panic!("accepted opening must stay current");
            };
            flags.sync_overlay(true, typed_search, false);
            drop(applying);
            flags.complete_overlay();
            assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
        }
    }

    #[test]
    fn queued_actions_stop_when_application_cancels_their_epoch() {
        use crate::switcher::{
            SwitchTask, SwitcherEffect, SwitcherSession, SwitcherSessionSettings,
        };
        let flags = Arc::new(HookFlags::new());
        let generation = flags.load();
        let tasks = [
            SwitchTask::new(1, 10, "One", "one"),
            SwitchTask::new(2, 20, "Two", "two"),
        ];
        let mut session = SwitcherSession::new(SwitcherSessionSettings {
            typed_search: true,
            release_alt_switches: true,
        });
        let _opening = session.handle_input(InputAction::MouseWheel(1));
        session.open(tasks.clone(), Some(1));
        flags.sync_overlay(true, true, false);
        assert!(flags.begin_opening(generation, true));
        assert!(flags.begin_opening(generation, true));
        let mut opened = 0;
        let mut activations = Vec::new();
        for action in [
            InputAction::MouseWheelReleased,
            InputAction::MouseWheel(1),
            InputAction::MouseWheel(1),
            InputAction::MouseWheelReleased,
        ] {
            let (origin, _) = encode_action(action, generation);
            let Some(_applying) = HookActionGuard::new(Arc::clone(&flags), origin, action) else {
                continue;
            };
            match session.handle_input(action) {
                SwitcherEffect::Open { selection_delta } => {
                    opened += 1;
                    if opened == 1 {
                        // The first enumeration failed, cancelling the originating batch.
                        session.hide();
                        flags.cancel_overlay();
                    } else {
                        session.open(tasks.clone(), selection_delta);
                        flags.sync_overlay(true, true, false);
                    }
                }
                SwitcherEffect::Activate(target) => {
                    activations.push(target);
                    flags.complete_overlay();
                }
                SwitcherEffect::None | SwitcherEffect::Redraw => {}
                effect => panic!("unexpected batch effect: {effect:?}"),
            }
        }
        assert_eq!(opened, 1, "cancelled input must not retry opening");
        assert_eq!(activations, [20]);
        assert!(!session.is_visible());
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
    }

    #[test]
    fn failed_post_cleanup_preserves_an_opening_being_applied() {
        let flags = Arc::new(HookFlags::new());
        let generation = flags.load();
        assert!(flags.begin_opening(generation, true)); // A failed to post.
        assert!(flags.begin_opening(generation, true)); // B reached the UI.
        let (origin, _) = encode_action(InputAction::MouseWheel(1), generation);
        let Some(applying) =
            HookActionGuard::new(Arc::clone(&flags), origin, InputAction::MouseWheel(1))
        else {
            panic!("opening must be current");
        };
        // B is enumerating windows and has not published visibility yet.
        flags.withdraw_openings(generation, 1);
        assert_ne!(flags.load() & OVERLAY_ACTIVE, 0);
        let mut hook = crate::input::HookState::default();
        hook.set_overlay_active(flags.load() & OVERLAY_ACTIVE != 0);
        let escape = hook.process_key(
            crate::input::KeyEvent::pressed(
                crate::input::Key::Escape,
                crate::input::Modifiers::default(),
            ),
            crate::input::HookSettings::default(),
        );
        assert!(escape.suppress);
        flags.set(OVERLAY_ACTIVE, true);
        drop(applying);
        assert_eq!(flags.value.load(Ordering::Acquire) & OPENING_MASK, 0);
        flags.complete_overlay();
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
    }

    #[test]
    fn refused_mouse_publication_does_not_capture_enter_or_number_keys() {
        use crate::input::{
            HookOutcome, HookSettings, HookState, Key, KeyEvent, Modifiers, MouseEvent,
        };
        for key in [Key::Enter, Key::Digit(2)] {
            let flags = HookFlags::new();
            flags.set(OVERLAY_ACTIVE, true);
            let generation = flags.load() & !OVERLAY_FLAGS;
            let mut hook = HookState::default();
            let settings = HookSettings {
                right_button_wheel_switching: true,
                ..HookSettings::default()
            };
            hook.set_overlay_active(true);
            let _press = hook.process_mouse(MouseEvent::RightButtonPressed, settings);
            assert_eq!(
                hook.process_mouse(MouseEvent::Wheel(-120), settings)
                    .actions()
                    .next(),
                Some(InputAction::MouseWheel(1))
            );
            // Run the same publication policy and CAS, with completion precisely between
            // the opening read and publication. No opening is posted after refusal.
            assert!(!flags.update_once(|value| {
                let opening = flags.opening_state(value, generation, true);
                flags.complete_overlay();
                opening
            }));
            assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
            hook.set_overlay_active(flags.load() & OVERLAY_ACTIVE != 0);
            for event in [
                KeyEvent::pressed(key, Modifiers::default()),
                KeyEvent::released(key, Modifiers::default()),
            ] {
                assert_eq!(hook.process_key(event, settings), HookOutcome::default());
            }
            let release = hook.process_mouse(MouseEvent::RightButtonReleased, settings);
            assert!(release.suppress);
            assert!(release.actions().next().is_none());
            assert!(!hook.take_replayed_right_click());
        }
    }

    #[test]
    fn failed_post_cleanup_preserves_a_later_visible_session() {
        let flags = HookFlags::new();
        let generation = flags.load();
        assert!(flags.begin_opening(generation, true)); // Failed post, cleanup delayed.
        assert!(flags.begin_opening(generation, true)); // Later successful opening.
        flags.retire_openings(generation, 1);
        flags.set(OVERLAY_ACTIVE, true); // The UI has applied the successful opening.
        flags.withdraw_openings(generation, 1);
        assert_ne!(flags.load() & OVERLAY_ACTIVE, 0);
        flags.complete_overlay();
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
    }

    #[test]
    fn completion_before_failed_post_cleanup_eventually_disarms_capture() {
        let flags = HookFlags::new();
        let generation = flags.load();
        assert!(flags.begin_opening(generation, true));
        flags.set(OVERLAY_ACTIVE, true);
        flags.complete_overlay();
        assert_ne!(flags.load() & OVERLAY_ACTIVE, 0);
        flags.withdraw_openings(generation, 1);
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
    }

    #[test]
    fn callback_publication_loses_contention_without_retrying_or_leaking_a_receipt() {
        let flags = HookFlags::new();
        let generation = flags.load();
        let mut attempts = 0;
        assert!(!flags.update_once(|value| {
            attempts += 1;
            // The UI changes the same word after the callback read and before its CAS.
            flags.set_search_state(true, true);
            Some(value + OPENING_STEP)
        }));
        assert_eq!(attempts, 1);
        assert_eq!(flags.value.load(Ordering::Acquire) & OPENING_MASK, 0);
        assert_eq!(flags.load() & OVERLAY_FLAGS, SEARCH_ACTIVE | FILTER_VISIBLE);
        assert!(flags.begin_opening(generation, true));
        assert_eq!(
            flags.value.load(Ordering::Acquire) & OPENING_MASK,
            OPENING_STEP
        );
        flags.retire_openings(generation, 1);
        flags.complete_overlay();
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
    }

    #[test]
    fn captured_wheel_preserves_raw_deltas_and_the_interception_epoch() {
        let flags = HookFlags::new();
        for delta in [
            i32::from(i16::MIN),
            -120,
            -30,
            0,
            30,
            120,
            i32::from(i16::MAX),
        ] {
            let action = InputAction::OverlayWheel(delta);
            let (wparam, lparam) = encode_action(action, flags.load());
            assert_eq!(decode_action(wparam, lparam), Some(action));
            assert!(flags.action_is_current(wparam));
            flags.advance_generation();
            assert!(!flags.action_is_current(wparam));
        }
    }

    #[test]
    fn a_stale_completion_read_cannot_disarm_an_opening_before_escape() {
        use crate::hook_delivery::{Delivery, deliver};
        use crate::input::{HookSettings, HookState, Key, KeyEvent, Modifiers, MouseEvent};
        let flags = HookFlags::new();
        flags.set(OVERLAY_ACTIVE, true);
        let completion_read = flags.load() & !OVERLAY_FLAGS;
        let observed = flags.value.load(Ordering::Acquire);
        let completed = completed_state(observed);
        // A's completion observed zero pending input. B is published before A can clear.
        let mut hook = HookState::default();
        let settings = HookSettings {
            right_button_wheel_switching: true,
            ..HookSettings::default()
        };
        hook.set_overlay_active(true);
        let _down = hook.process_mouse(MouseEvent::RightButtonPressed, settings);
        let opening = hook.process_mouse(MouseEvent::Wheel(-120), settings);
        assert_eq!(
            deliver(&flags, completion_read, true, opening, |_| true),
            Delivery::Posted
        );
        assert!(
            flags
                .value
                .compare_exchange(observed, completed, Ordering::AcqRel, Ordering::Acquire)
                .is_err(),
            "an opening published after the completion read must defeat its stale clear"
        );
        let _cleared = flags.update_overlay(completion_read, false, false);
        // Escape runs at the formerly vulnerable point, before any repair/rearming.
        hook.set_overlay_active(flags.load() & OVERLAY_ACTIVE != 0);
        let escape = hook.process_key(
            KeyEvent::pressed(Key::Escape, Modifiers::default()),
            settings,
        );
        assert!(
            escape.suppress,
            "completion exposed a false inactive state to Escape"
        );
        assert_eq!(escape.actions().next(), Some(InputAction::DismissOverlay));
        assert!(
            hook.process_mouse(MouseEvent::RightButtonReleased, settings)
                .actions()
                .next()
                .is_none()
        );
    }

    #[test]
    fn query_presence_tracks_search_without_invalidating_queued_actions() {
        let flags = HookFlags::new();
        let generation = flags.load();
        let (action, _) = encode_action(InputAction::Navigate(1), generation);
        for (active, filtered, expected) in [
            (true, true, SEARCH_ACTIVE | FILTER_VISIBLE),
            (true, false, SEARCH_ACTIVE),
            (false, true, 0),
            (false, false, 0),
        ] {
            flags.set_search_state(active, filtered);
            assert_eq!(flags.load() & (SEARCH_ACTIVE | FILTER_VISIBLE), expected);
            assert!(flags.action_is_current(action));
            assert!(flags.update_overlay(generation, true, true));
            assert_eq!(flags.load() & FILTER_VISIBLE, expected & FILTER_VISIBLE);
        }
    }

    #[test]
    fn number_shortcuts_preserve_translated_text_across_the_message_boundary() {
        for position in [1, 9] {
            for text in [
                None,
                Some('&'),
                Some('!'),
                Some('ą'),
                Some('\0'),
                Some('\u{10FFFF}'),
            ] {
                let action = InputAction::NumberKey { position, text };
                let (code, value) = encode_action(action, 0);
                assert_eq!(decode_action(code, value), Some(action));
            }
        }
        assert!(decode_action(ACTION_NUMBER_KEY, -1).is_none());
        assert!(decode_action(ACTION_NUMBER_KEY, 0).is_none());
        assert!(decode_action(ACTION_NUMBER_KEY, (0x0011_0001 << 4) | 1).is_none());
    }
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
        for generation in [
            0,
            16,
            ACTION_EPOCH_MASK << GENERATION_SHIFT,
            usize::MAX & !FLAG_BITS,
        ] {
            let flags = HookFlags::with_value(generation);
            let action = action_wparam(ACTION_CLOSE_SELECTED, generation);
            assert_eq!(action & ACTION_CODE_MASK, ACTION_CLOSE_SELECTED);
            assert_eq!(
                action >> 8,
                (generation >> GENERATION_SHIFT) & ACTION_EPOCH_MASK
            );
            assert!(flags.action_is_current(action));
            assert_eq!(decode_action(action, 0), Some(InputAction::CloseSelected));
        }
    }

    #[test]
    fn scoped_suspension_restores_only_visible_capture_with_new_generations() {
        for saved_flags in 0..=FLAG_BITS {
            let flags = Arc::new(HookFlags::with_value(saved_flags));
            let visible = saved_flags & OVERLAY_ACTIVE != 0;
            if visible {
                flags.value.fetch_or(OVERLAY_VISIBLE, Ordering::AcqRel);
            }
            let initial_generation = flags.load() & !FLAG_BITS;
            let guard = HookInterceptionGuard::new(Arc::clone(&flags));
            let suspended = flags.load();
            assert_eq!(suspended & FLAG_BITS, INTERCEPTION_SUSPENDED);
            assert_ne!(suspended & !FLAG_BITS, initial_generation);
            assert!(!flags.update_overlay(initial_generation, true, true));
            flags.set(SEARCH_ACTIVE, true);
            flags.set(OVERLAY_ACTIVE, true);
            drop(guard);
            assert_eq!(
                flags.load() & FLAG_BITS,
                if visible {
                    saved_flags
                } else {
                    saved_flags & INTERCEPTION_SUSPENDED
                }
            );
            assert_ne!(flags.load() & !FLAG_BITS, suspended & !FLAG_BITS);
            assert!(!flags.update_overlay(suspended & !OVERLAY_FLAGS, true, true));
        }
    }

    #[test]
    fn nested_scoped_suspension_restores_outer_state_without_resuming_it() {
        let flags = Arc::new(HookFlags::with_value(OVERLAY_FLAGS));
        flags.sync_overlay(true, true, true);
        let outer = HookInterceptionGuard::new(Arc::clone(&flags));
        flags.set(SEARCH_ACTIVE, true);
        {
            let _inner = HookInterceptionGuard::new(Arc::clone(&flags));
            assert_eq!(flags.load() & FLAG_BITS, INTERCEPTION_SUSPENDED);
            flags.set(OVERLAY_ACTIVE, true);
        }
        assert_eq!(flags.load() & FLAG_BITS, INTERCEPTION_SUSPENDED);
        drop(outer);
        assert_eq!(flags.load() & FLAG_BITS, OVERLAY_FLAGS);
    }

    #[test]
    fn scoped_suspension_restores_flags_during_unwinding() {
        let flags = Arc::new(HookFlags::with_value(OVERLAY_FLAGS));
        flags.sync_overlay(true, true, true);
        let result = std::panic::catch_unwind(|| {
            let _guard = HookInterceptionGuard::new(Arc::clone(&flags));
            panic!("modal call failed");
        });
        assert!(result.is_err());
        assert_eq!(flags.load() & FLAG_BITS, OVERLAY_FLAGS);
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
            InputAction::CycleSelection(-1),
            InputAction::CycleSelection(1),
            InputAction::SearchNavigation {
                command: crate::text_edit::TextCommand::SelectLeft,
                modified: true,
            },
            InputAction::SearchNavigation {
                command: crate::text_edit::TextCommand::Left,
                modified: false,
            },
            InputAction::MouseWheel(-1),
            InputAction::MouseWheelReleased,
            InputAction::RightButtonPressed,
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
        let flags = HookFlags::with_value(16 | OVERLAY_FLAGS);
        assert!(flags.generation_is_current(16));
        flags.set(SEARCH_ACTIVE, false);
        assert!(
            flags.generation_is_current(16),
            "UI flags are not a boundary"
        );
        assert!(!flags.generation_is_current(32));
        flags.set_recovery_pending(true);
        assert!(!flags.generation_is_current(16));
        flags.set_recovery_pending(false);
        flags.suspend(true);
        assert!(!flags.generation_is_current(16));
        assert!(!flags.generation_is_current(flags.load() & !OVERLAY_FLAGS));
    }

    #[test]
    fn advancing_the_generation_keeps_only_visible_ui_flags_and_retires_queued_actions() {
        // The last epoch before the counter wraps, so no generation handed out in tests repeats it.
        let generation = ACTION_EPOCH_MASK << GENERATION_SHIFT;
        for saved_flags in 0..=FLAG_BITS {
            let flags = HookFlags::with_value(generation | saved_flags);
            let visible = saved_flags & OVERLAY_ACTIVE != 0;
            if visible {
                flags.value.fetch_or(OVERLAY_VISIBLE, Ordering::AcqRel);
            }
            let (queued, _) = encode_action(InputAction::AltReleased, flags.load());
            assert_eq!(flags.action_is_current(queued), saved_flags & 1 == 0);
            flags.advance_generation();
            assert_eq!(
                flags.load() & FLAG_BITS,
                if visible {
                    saved_flags
                } else {
                    saved_flags & INTERCEPTION_SUSPENDED
                }
            );
            assert_ne!(flags.load() & !FLAG_BITS, generation);
            assert!(!flags.action_is_current(queued));
        }
    }
}
