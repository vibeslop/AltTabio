//! Keys as the input hook names them.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers {
    pub alt: bool,
    pub left_windows: bool,
    pub right_windows: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Key {
    Tab,
    Enter,
    Home,
    End,
    Escape,
    F4,
    Function(u8),
    Alt,
    LeftAlt,
    RightAlt,
    LeftWindows,
    RightWindows,
    Control,
    LeftControl,
    RightControl,
    LeftShift,
    RightShift,
    PrintScreen,
    Backspace,
    LeftArrow,
    UpArrow,
    RightArrow,
    DownArrow,
    Digit(u8),
    NumpadDigit(u8),
    Other(u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyTransition {
    Pressed,
    Released,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyEvent {
    pub key: Key,
    pub transition: KeyTransition,
    pub modifiers: Modifiers,
    pub text: Option<char>,
}

impl KeyEvent {
    #[must_use]
    pub const fn pressed(key: Key, modifiers: Modifiers) -> Self {
        Self {
            key,
            transition: KeyTransition::Pressed,
            modifiers,
            text: None,
        }
    }

    #[must_use]
    pub const fn released(key: Key, modifiers: Modifiers) -> Self {
        Self {
            key,
            transition: KeyTransition::Released,
            modifiers,
            text: None,
        }
    }

    #[must_use]
    pub const fn with_text(mut self, text: char) -> Self {
        self.text = Some(text);
        self
    }
}

/// Windows virtual-key codes of the keys with their own name. Decoding and replay both read this
/// table, so a replayed key carries the code it was decoded from.
const NAMED_KEYS: [(Key, u16); 22] = [
    (Key::Tab, 0x09),
    (Key::Enter, 0x0D),
    (Key::Home, 0x24),
    (Key::End, 0x23),
    (Key::Escape, 0x1B),
    (Key::F4, 0x73),
    (Key::Alt, 0x12),
    (Key::LeftAlt, 0xA4),
    (Key::RightAlt, 0xA5),
    (Key::LeftWindows, 0x5B),
    (Key::RightWindows, 0x5C),
    (Key::Control, 0x11),
    (Key::LeftControl, 0xA2),
    (Key::RightControl, 0xA3),
    (Key::LeftShift, 0xA0),
    (Key::RightShift, 0xA1),
    (Key::PrintScreen, 0x2C),
    (Key::Backspace, 0x08),
    (Key::LeftArrow, 0x25),
    (Key::UpArrow, 0x26),
    (Key::RightArrow, 0x27),
    (Key::DownArrow, 0x28),
];

// A numbered key sits at its base plus its number. Only the numbers the switcher acts on decode
// to it; VK_0, VK_NUMPAD0, F1 to F3 and F10 onward stay `Other`.
const DIGIT_BASE: u16 = 0x30;
const NUMPAD_DIGIT_BASE: u16 = 0x60;
const FUNCTION_BASE: u16 = 0x6F;

impl Key {
    /// The Windows virtual-key code that delivers this key.
    #[must_use]
    pub fn virtual_key(self) -> u16 {
        match self {
            Self::Function(number) => FUNCTION_BASE + u16::from(number),
            Self::Digit(digit) => DIGIT_BASE + u16::from(digit),
            Self::NumpadDigit(digit) => NUMPAD_DIGIT_BASE + u16::from(digit),
            Self::Other(code) => code,
            named => NAMED_KEYS
                .iter()
                .find(|&&(key, _)| key == named)
                .map_or(0, |&(_, code)| code),
        }
    }
}

/// Names a Windows virtual-key code; codes without a name stay `Other`.
#[must_use]
pub fn decode_virtual_key(virtual_key: u32) -> Key {
    let Ok(code) = u16::try_from(virtual_key) else {
        return Key::Other(0);
    };
    if let Some(&(key, _)) = NAMED_KEYS.iter().find(|&&(_, named)| named == code) {
        return key;
    }
    let numbered = |base: u16, numbers: core::ops::RangeInclusive<u8>| {
        code.checked_sub(base)
            .and_then(|offset| u8::try_from(offset).ok())
            .filter(|number| numbers.contains(number))
    };
    if let Some(digit) = numbered(DIGIT_BASE, 1..=9) {
        Key::Digit(digit)
    } else if let Some(digit) = numbered(NUMPAD_DIGIT_BASE, 1..=9) {
        Key::NumpadDigit(digit)
    } else if let Some(number) = numbered(FUNCTION_BASE, 5..=9) {
        Key::Function(number)
    } else {
        Key::Other(code)
    }
}

/// Keys by virtual-key code. A key whose code is 256 or above is never a member.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct KeySet([u64; 4]);

impl KeySet {
    fn bit(key: Key) -> Option<(usize, u64)> {
        let code = usize::from(key.virtual_key());
        (code < 256).then(|| (code / 64, 1 << (code % 64)))
    }

    pub(super) fn can_hold(key: Key) -> bool {
        Self::bit(key).is_some()
    }

    pub(super) fn contains(&self, key: Key) -> bool {
        Self::bit(key)
            .is_some_and(|(word, mask)| self.0.get(word).is_some_and(|bits| bits & mask != 0))
    }

    pub(super) fn insert(&mut self, key: Key) {
        self.set(key, true);
    }

    /// Removes the key and reports whether it was a member.
    pub(super) fn remove(&mut self, key: Key) -> bool {
        let member = self.contains(key);
        self.set(key, false);
        member
    }

    pub(super) fn set(&mut self, key: Key, member: bool) {
        let Some((word, mask)) = Self::bit(key) else {
            return;
        };
        if let Some(bits) = self.0.get_mut(word) {
            if member {
                *bits |= mask;
            } else {
                *bits &= !mask;
            }
        }
    }

    pub(super) fn insert_all(&mut self, other: Self) {
        for (bits, other) in self.0.iter_mut().zip(other.0) {
            *bits |= other;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_virtual_key_replays_under_the_code_it_was_decoded_from() {
        for code in 0..=u16::MAX {
            assert_eq!(decode_virtual_key(u32::from(code)).virtual_key(), code);
        }
    }

    #[test]
    fn named_keys_decode_from_their_own_code() {
        for (key, code) in NAMED_KEYS {
            assert_eq!(decode_virtual_key(u32::from(code)), key);
            assert_eq!(key.virtual_key(), code);
        }
    }

    #[test]
    fn numbered_keys_decode_only_for_the_numbers_the_switcher_uses() {
        for number in 1..=9 {
            for key in [Key::Digit(number), Key::NumpadDigit(number)] {
                assert_eq!(decode_virtual_key(u32::from(key.virtual_key())), key);
            }
        }
        for number in 5..=9 {
            let key = Key::Function(number);
            assert_eq!(decode_virtual_key(u32::from(key.virtual_key())), key);
        }
        // VK_0, VK_NUMPAD0, F1 and F10.
        for code in [0x30_u16, 0x60, 0x70, 0x79] {
            assert_eq!(decode_virtual_key(u32::from(code)), Key::Other(code));
        }
    }

    #[test]
    fn key_set_tracks_each_key_apart_across_every_word() {
        let keys = [
            Key::Backspace,
            Key::Tab,
            Key::Digit(9),
            Key::LeftWindows,
            Key::Other(0x7F),
            Key::LeftShift,
            Key::Other(0xFF),
        ];
        let mut set = KeySet::default();
        for (index, key) in keys.into_iter().enumerate() {
            set.insert(key);
            for (checked_index, checked) in keys.into_iter().enumerate() {
                assert_eq!(set.contains(checked), checked_index <= index, "{checked:?}");
            }
        }
        assert!(set.remove(Key::Tab));
        assert!(!set.remove(Key::Tab));
        assert!(!set.contains(Key::Tab));
        assert!(set.contains(Key::Backspace));
        set.set(Key::Tab, true);
        assert!(set.contains(Key::Tab));
        set.set(Key::Tab, false);
        assert!(!set.contains(Key::Tab));
    }

    #[test]
    fn key_set_never_holds_codes_past_the_keyboard_state_table() {
        let mut set = KeySet::default();
        for key in [Key::Other(0x100), Key::Other(u16::MAX), Key::Function(0xFF)] {
            assert!(!KeySet::can_hold(key));
            set.insert(key);
            assert!(!set.contains(key));
            assert!(!set.remove(key));
        }
        assert_eq!(set, KeySet::default());
        assert!(KeySet::can_hold(Key::Other(0xFF)));
    }

    #[test]
    fn key_set_union_keeps_members_of_both_sets() {
        let mut held = KeySet::default();
        held.insert(Key::Tab);
        let mut pressed = KeySet::default();
        pressed.insert(Key::LeftAlt);
        pressed.insert(Key::Other(0xFF));
        held.insert_all(pressed);
        for key in [Key::Tab, Key::LeftAlt, Key::Other(0xFF)] {
            assert!(held.contains(key), "{key:?}");
        }
        assert!(!held.contains(Key::Escape));
    }

    #[cfg(windows)]
    #[test]
    fn table_matches_the_windows_virtual_key_constants() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            VK_0, VK_1, VK_9, VK_BACK, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F4, VK_F5,
            VK_F9, VK_F10, VK_HOME, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU,
            VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD9, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU,
            VK_RSHIFT, VK_RWIN, VK_SNAPSHOT, VK_TAB, VK_UP,
        };

        let expected = [
            (Key::Tab, VK_TAB),
            (Key::Enter, VK_RETURN),
            (Key::Home, VK_HOME),
            (Key::End, VK_END),
            (Key::Escape, VK_ESCAPE),
            (Key::F4, VK_F4),
            (Key::Alt, VK_MENU),
            (Key::LeftAlt, VK_LMENU),
            (Key::RightAlt, VK_RMENU),
            (Key::LeftWindows, VK_LWIN),
            (Key::RightWindows, VK_RWIN),
            (Key::Control, VK_CONTROL),
            (Key::LeftControl, VK_LCONTROL),
            (Key::RightControl, VK_RCONTROL),
            (Key::LeftShift, VK_LSHIFT),
            (Key::RightShift, VK_RSHIFT),
            (Key::PrintScreen, VK_SNAPSHOT),
            (Key::Backspace, VK_BACK),
            (Key::LeftArrow, VK_LEFT),
            (Key::UpArrow, VK_UP),
            (Key::RightArrow, VK_RIGHT),
            (Key::DownArrow, VK_DOWN),
            (Key::Function(5), VK_F5),
            (Key::Function(9), VK_F9),
            (Key::Digit(1), VK_1),
            (Key::Digit(9), VK_9),
            (Key::NumpadDigit(1), VK_NUMPAD1),
            (Key::NumpadDigit(9), VK_NUMPAD9),
        ];
        for (key, virtual_key) in expected {
            assert_eq!(key.virtual_key(), virtual_key.0, "{key:?}");
            assert_eq!(decode_virtual_key(u32::from(virtual_key.0)), key);
        }
        for (key, _) in NAMED_KEYS {
            assert!(
                expected.iter().any(|&(checked, _)| checked == key),
                "{key:?}"
            );
        }
        for unnamed in [VK_0, VK_NUMPAD0, VK_F1, VK_F10] {
            assert_eq!(
                decode_virtual_key(u32::from(unnamed.0)),
                Key::Other(unnamed.0)
            );
        }
    }
}
