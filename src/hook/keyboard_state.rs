//! The keyboard as the hook observed it, including presses it suppressed, so typed search text
//! translates with the modifiers the user actually holds.

use super::key_pressed;
use alttabio::input::{HookState, Key, KeyTransition};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, GetKeyboardState, ToUnicodeEx, VIRTUAL_KEY, VK_0, VK_1, VK_9, VK_BACK,
    VK_CAPITAL, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9,
    VK_HOME, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_NUMLOCK, VK_NUMPAD0,
    VK_NUMPAD1, VK_NUMPAD9, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN,
    VK_SCROLL, VK_SHIFT, VK_SNAPSHOT, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::KBDLLHOOKSTRUCT;

pub(super) struct KeyboardState {
    pub(super) keys: [u8; 256],
    forwarded_toggle_keys: u8,
    pub(super) modifier_observations: [ModifierObservation; 8],
}

pub(super) const MODIFIER_KEYS: [VIRTUAL_KEY; 8] = [
    VK_LSHIFT,
    VK_RSHIFT,
    VK_LCONTROL,
    VK_RCONTROL,
    VK_LMENU,
    VK_RMENU,
    VK_LWIN,
    VK_RWIN,
];

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub(super) struct ModifierObservation {
    time: Option<u32>,
    sequence: u64,
}

pub(super) const fn timestamp_at_or_after(time: u32, boundary: u32) -> bool {
    time.wrapping_sub(boundary) < (1 << 31)
}

impl Default for KeyboardState {
    fn default() -> Self {
        Self {
            keys: [0; 256],
            forwarded_toggle_keys: 0,
            modifier_observations: [ModifierObservation::default(); 8],
        }
    }
}

impl KeyboardState {
    pub(super) fn snapshot() -> Result<Self, String> {
        let mut keys = [0; 256];
        unsafe {
            // SAFETY: the complete fixed-size buffer is writable. This runs on the owning UI
            // thread before hooks start, so toggle bits are seeded from its input queue.
            GetKeyboardState(&mut keys)
        }
        .map_err(|error| format!("Could not read initial keyboard state: {error}"))?;
        for key in 0_u16..=255 {
            keys[usize::from(key)] =
                (keys[usize::from(key)] & 1) | if key_pressed(key) { 0x80 } else { 0 };
        }
        let forwarded_toggle_keys = [VK_CAPITAL, VK_NUMLOCK, VK_SCROLL]
            .into_iter()
            .enumerate()
            .fold(0, |mask, (index, key)| {
                mask | if keys[usize::from(key.0)] & 0x80 != 0 {
                    1 << index
                } else {
                    0
                }
            });
        Ok(Self {
            keys,
            forwarded_toggle_keys,
            ..Self::default()
        })
    }

    pub(super) fn observe_at(&mut self, virtual_key: u32, transition: KeyTransition, time: u32) {
        self.observe(virtual_key, transition);
        if let Some(index) = MODIFIER_KEYS
            .into_iter()
            .position(|key| u32::from(key.0) == virtual_key)
        {
            let observation = &mut self.modifier_observations[index];
            observation.time = Some(time);
            observation.sequence = observation.sequence.wrapping_add(1);
        }
    }

    pub(super) fn rebase_modifiers(
        &mut self,
        boundary: u32,
        before: [ModifierObservation; 8],
        down: [bool; 8],
        state: &mut HookState,
    ) {
        state.reset_gestures();
        for (index, key) in MODIFIER_KEYS.into_iter().enumerate() {
            let observed = self.modifier_observations[index];
            if observed.sequence != before[index].sequence
                || observed
                    .time
                    .is_some_and(|time| timestamp_at_or_after(time, boundary))
            {
                continue;
            }
            self.keys[usize::from(key.0)] = if down[index] { 0x80 } else { 0 };
            state.rebase_modifier(decode_virtual_key(u32::from(key.0)), down[index]);
        }
        for (aggregate, left, right) in [
            (VK_SHIFT, VK_LSHIFT, VK_RSHIFT),
            (VK_CONTROL, VK_LCONTROL, VK_RCONTROL),
            (VK_MENU, VK_LMENU, VK_RMENU),
        ] {
            self.keys[usize::from(aggregate.0)] =
                (self.keys[usize::from(left.0)] | self.keys[usize::from(right.0)]) & 0x80;
        }
    }

    fn observe(&mut self, virtual_key: u32, transition: KeyTransition) {
        let Some(state) = usize::try_from(virtual_key)
            .ok()
            .and_then(|key| self.keys.get_mut(key))
        else {
            return;
        };
        let pressed = transition == KeyTransition::Pressed;
        *state = (*state & 1) | if pressed { 0x80 } else { 0 };
        for (aggregate, left, right) in [
            (VK_SHIFT, VK_LSHIFT, VK_RSHIFT),
            (VK_CONTROL, VK_LCONTROL, VK_RCONTROL),
            (VK_MENU, VK_LMENU, VK_RMENU),
        ] {
            if virtual_key == u32::from(left.0) || virtual_key == u32::from(right.0) {
                self.keys[usize::from(aggregate.0)] =
                    (self.keys[usize::from(left.0)] | self.keys[usize::from(right.0)]) & 0x80;
            }
        }
    }

    pub(super) fn commit_forwarded(&mut self, virtual_key: u32, transition: KeyTransition) {
        let Some(index) = [VK_CAPITAL, VK_NUMLOCK, VK_SCROLL]
            .into_iter()
            .position(|key| u32::from(key.0) == virtual_key)
        else {
            return;
        };
        let mask = 1 << index;
        if transition == KeyTransition::Pressed {
            if self.forwarded_toggle_keys & mask == 0 {
                self.keys[virtual_key as usize] ^= 1;
            }
            self.forwarded_toggle_keys |= mask;
        } else {
            self.forwarded_toggle_keys &= !mask;
        }
    }

    fn search_state(&self) -> [u8; 256] {
        let mut keys = self.keys;
        // Alt belongs to the switch gesture, not the user's search text.
        for key in [VK_MENU, VK_LMENU, VK_RMENU] {
            keys[usize::from(key.0)] = 0;
        }
        keys
    }
}

pub(super) fn translate_search_character(
    data: &KBDLLHOOKSTRUCT,
    target_thread_id: u32,
    observed: &KeyboardState,
) -> Option<char> {
    let keyboard_state = observed.search_state();
    let mut text = [0_u16; 4];
    let count = unsafe {
        // SAFETY: both buffers are initialized for their full lengths; the keyboard layout is
        // read from the overlay UI thread and flag 4 prevents mutation of the dead-key state.
        ToUnicodeEx(
            data.vkCode,
            data.scanCode,
            &keyboard_state,
            &mut text,
            4,
            Some(GetKeyboardLayout(target_thread_id)),
        )
    };
    let count = usize::try_from(count).ok()?;
    let mut characters = char::decode_utf16(text.get(..count)?.iter().copied());
    let character = characters.next()?.ok()?;
    characters.next().is_none().then_some(character)
}

pub(crate) fn decode_virtual_key(virtual_key: u32) -> Key {
    match virtual_key {
        value if value == u32::from(VK_TAB.0) => Key::Tab,
        value if value == u32::from(VK_RETURN.0) => Key::Enter,
        value if value == u32::from(VK_HOME.0) => Key::Home,
        value if value == u32::from(VK_END.0) => Key::End,
        value if value == u32::from(VK_ESCAPE.0) => Key::Escape,
        value if value == u32::from(VK_F4.0) => Key::F4,
        value if value == u32::from(VK_F5.0) => Key::Function(5),
        value if value == u32::from(VK_F6.0) => Key::Function(6),
        value if value == u32::from(VK_F7.0) => Key::Function(7),
        value if value == u32::from(VK_F8.0) => Key::Function(8),
        value if value == u32::from(VK_F9.0) => Key::Function(9),
        value if value == u32::from(VK_MENU.0) => Key::Alt,
        value if value == u32::from(VK_LMENU.0) => Key::LeftAlt,
        value if value == u32::from(VK_RMENU.0) => Key::RightAlt,
        value if value == u32::from(VK_LWIN.0) => Key::LeftWindows,
        value if value == u32::from(VK_RWIN.0) => Key::RightWindows,
        value if value == u32::from(VK_CONTROL.0) => Key::Control,
        value if value == u32::from(VK_LCONTROL.0) => Key::LeftControl,
        value if value == u32::from(VK_RCONTROL.0) => Key::RightControl,
        value if value == u32::from(VK_LSHIFT.0) => Key::LeftShift,
        value if value == u32::from(VK_RSHIFT.0) => Key::RightShift,
        value if value == u32::from(VK_SNAPSHOT.0) => Key::PrintScreen,
        value if value == u32::from(VK_BACK.0) => Key::Backspace,
        value if value == u32::from(VK_LEFT.0) => Key::LeftArrow,
        value if value == u32::from(VK_UP.0) => Key::UpArrow,
        value if value == u32::from(VK_RIGHT.0) => Key::RightArrow,
        value if value == u32::from(VK_DOWN.0) => Key::DownArrow,
        value if value >= u32::from(VK_1.0) && value <= u32::from(VK_9.0) => {
            Key::Digit(u8::try_from(value - u32::from(VK_0.0)).unwrap_or_default())
        }
        value if value >= u32::from(VK_NUMPAD1.0) && value <= u32::from(VK_NUMPAD9.0) => {
            Key::NumpadDigit(u8::try_from(value - u32::from(VK_NUMPAD0.0)).unwrap_or_default())
        }
        value => Key::Other(u16::try_from(value).unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::test_context;
    use alttabio::input::{InputAction, KeyEvent, Modifiers};

    #[test]
    fn desktop_rebase_clears_old_modifiers_but_preserves_fresh_suppressed_shift() {
        for fresh in [false, true] {
            let mut context = test_context();
            let alt = Modifiers {
                alt: true,
                ..Modifiers::default()
            };
            let _tab = context
                .state
                .process_key(KeyEvent::pressed(Key::Tab, alt), context.settings);
            let _tab_up = context
                .state
                .process_key(KeyEvent::released(Key::Tab, alt), context.settings);
            let shift = context
                .state
                .process_key(KeyEvent::pressed(Key::LeftShift, alt), context.settings);
            assert!(shift.suppress);
            context.keyboard_state.observe_at(
                u32::from(VK_LSHIFT.0),
                KeyTransition::Pressed,
                if fresh { 310 } else { 100 },
            );
            let before = context.keyboard_state.modifier_observations;
            context
                .keyboard_state
                .rebase_modifiers(300, before, [false; 8], &mut context.state);
            assert_eq!(
                context.keyboard_state.keys[usize::from(VK_SHIFT.0)] != 0,
                fresh
            );
            let direction = context
                .state
                .process_key(KeyEvent::pressed(Key::Tab, alt), context.settings);
            assert_eq!(
                direction.actions().next(),
                Some(InputAction::Switch(if fresh { -1 } else { 1 }))
            );
            let release = context
                .state
                .process_key(KeyEvent::released(Key::LeftShift, alt), context.settings);
            assert!(release.suppress);
        }
    }

    #[test]
    fn desktop_rebase_preserves_observations_during_snapshot_and_timestamp_wrap() {
        let mut context = test_context();
        context
            .keyboard_state
            .observe_at(u32::from(VK_LSHIFT.0), KeyTransition::Pressed, 100);
        let before = context.keyboard_state.modifier_observations;
        context
            .keyboard_state
            .observe_at(u32::from(VK_LSHIFT.0), KeyTransition::Released, 400);
        context
            .keyboard_state
            .observe_at(u32::from(VK_RSHIFT.0), KeyTransition::Pressed, 20);
        context.keyboard_state.rebase_modifiers(
            u32::MAX - 10,
            before,
            [true; 8],
            &mut context.state,
        );
        assert_eq!(context.keyboard_state.keys[usize::from(VK_LSHIFT.0)], 0);
        assert_eq!(context.keyboard_state.keys[usize::from(VK_RSHIFT.0)], 0x80);
        assert!(timestamp_at_or_after(20, u32::MAX - 10));
        assert!(!timestamp_at_or_after(u32::MAX - 10, 20));
    }

    #[test]
    fn observed_modifiers_combine_both_sides_and_toggle_only_on_first_press() {
        let mut state = KeyboardState::default();
        for (aggregate, left, right) in [
            (VK_SHIFT, VK_LSHIFT, VK_RSHIFT),
            (VK_CONTROL, VK_LCONTROL, VK_RCONTROL),
            (VK_MENU, VK_LMENU, VK_RMENU),
        ] {
            state.observe(u32::from(left.0), KeyTransition::Pressed);
            state.observe(u32::from(right.0), KeyTransition::Pressed);
            state.observe(u32::from(left.0), KeyTransition::Released);
            assert_eq!(state.keys[usize::from(aggregate.0)], 0x80);
            state.observe(u32::from(right.0), KeyTransition::Released);
            assert_eq!(state.keys[usize::from(aggregate.0)], 0);
        }
        for key in [VK_CAPITAL, VK_NUMLOCK, VK_SCROLL] {
            state.observe(u32::from(key.0), KeyTransition::Pressed);
            state.commit_forwarded(u32::from(key.0), KeyTransition::Pressed);
            state.observe(u32::from(key.0), KeyTransition::Pressed);
            state.commit_forwarded(u32::from(key.0), KeyTransition::Pressed);
            assert_eq!(state.keys[usize::from(key.0)], 0x81);
            state.observe(u32::from(key.0), KeyTransition::Released);
            state.commit_forwarded(u32::from(key.0), KeyTransition::Released);
            assert_eq!(state.keys[usize::from(key.0)], 1);
            state.observe(u32::from(key.0), KeyTransition::Pressed);
            state.commit_forwarded(u32::from(key.0), KeyTransition::Pressed);
            assert_eq!(state.keys[usize::from(key.0)], 0x80);
        }
    }

    #[test]
    fn translated_punctuation_uses_observed_shift_including_suppressed_presses() {
        let data = KBDLLHOOKSTRUCT {
            vkCode: u32::from(VK_1.0),
            ..KBDLLHOOKSTRUCT::default()
        };
        let mut state = KeyboardState::default();
        state.observe(data.vkCode, KeyTransition::Pressed);
        let plain = translate_search_character(&data, 0, &state);
        let mut expected = KeyboardState::default();
        expected.keys[usize::from(VK_SHIFT.0)] = 0x80;
        expected.keys[usize::from(VK_LSHIFT.0)] = 0x80;
        expected.keys[usize::from(VK_1.0)] = 0x80;
        let shifted = translate_search_character(&data, 0, &expected);
        assert!(shifted.is_some());
        assert_ne!(plain, shifted);
        state.observe(u32::from(VK_LSHIFT.0), KeyTransition::Pressed);
        state.observe(u32::from(VK_LMENU.0), KeyTransition::Pressed);
        assert_eq!(translate_search_character(&data, 0, &state), shifted);
        state.observe(data.vkCode, KeyTransition::Pressed);
        assert_eq!(translate_search_character(&data, 0, &state), shifted);
        state.observe(u32::from(VK_LSHIFT.0), KeyTransition::Released);
        assert_eq!(translate_search_character(&data, 0, &state), plain);
    }

    #[test]
    fn arrow_virtual_keys_map_to_directional_hook_keys() {
        assert_eq!(decode_virtual_key(u32::from(VK_LEFT.0)), Key::LeftArrow);
        assert_eq!(decode_virtual_key(u32::from(VK_UP.0)), Key::UpArrow);
        assert_eq!(decode_virtual_key(u32::from(VK_RIGHT.0)), Key::RightArrow);
        assert_eq!(decode_virtual_key(u32::from(VK_DOWN.0)), Key::DownArrow);
    }

    #[test]
    fn windows_virtual_keys_preserve_their_physical_side() {
        assert_eq!(decode_virtual_key(u32::from(VK_LWIN.0)), Key::LeftWindows);
        assert_eq!(decode_virtual_key(u32::from(VK_RWIN.0)), Key::RightWindows);
    }

    #[test]
    fn control_and_print_screen_virtual_keys_map_to_passthrough_hook_keys() {
        assert_eq!(decode_virtual_key(u32::from(VK_CONTROL.0)), Key::Control);
        assert_eq!(
            decode_virtual_key(u32::from(VK_LCONTROL.0)),
            Key::LeftControl
        );
        assert_eq!(
            decode_virtual_key(u32::from(VK_RCONTROL.0)),
            Key::RightControl
        );
        assert_eq!(
            decode_virtual_key(u32::from(VK_SNAPSHOT.0)),
            Key::PrintScreen
        );
    }

    #[test]
    fn home_and_end_virtual_keys_map_to_boundary_keys() {
        assert_eq!(decode_virtual_key(u32::from(VK_HOME.0)), Key::Home);
        assert_eq!(decode_virtual_key(u32::from(VK_END.0)), Key::End);
    }

    #[test]
    fn escape_virtual_key_maps_to_escape_input() {
        assert_eq!(decode_virtual_key(u32::from(VK_ESCAPE.0)), Key::Escape);
    }

    #[test]
    fn f5_through_f9_map_to_semantic_function_keys() {
        for (virtual_key, function_key) in
            [(VK_F5, 5), (VK_F6, 6), (VK_F7, 7), (VK_F8, 8), (VK_F9, 9)]
        {
            assert_eq!(
                decode_virtual_key(u32::from(virtual_key.0)),
                Key::Function(function_key)
            );
        }
    }
}
