//! macOS virtual key codes mapped onto the shared switcher key model.
//!
//! The shared model names the primary switch modifier `Alt` (Windows Alt) and the secondary one
//! `Windows`. On macOS those roles belong to Command and Option respectively, so the key codes for
//! Command map onto `Alt` and Option onto `Windows` to keep the shared overlay key handling intact.

use alttabio::input::Key;

#[must_use]
pub const fn key_for_code(code: u16) -> Key {
    match code {
        48 => Key::Tab,
        36 | 76 => Key::Enter,
        115 => Key::Home,
        119 => Key::End,
        53 => Key::Escape,
        118 => Key::F4,
        122 => Key::Function(1),
        120 => Key::Function(2),
        99 => Key::Function(3),
        96 => Key::Function(5),
        97 => Key::Function(6),
        98 => Key::Function(7),
        100 => Key::Function(8),
        101 => Key::Function(9),
        109 => Key::Function(10),
        103 => Key::Function(11),
        111 => Key::Function(12),
        51 => Key::Backspace,
        123 => Key::LeftArrow,
        126 => Key::UpArrow,
        124 => Key::RightArrow,
        125 => Key::DownArrow,
        29 => Key::Digit(0),
        18 => Key::Digit(1),
        19 => Key::Digit(2),
        20 => Key::Digit(3),
        21 => Key::Digit(4),
        23 => Key::Digit(5),
        22 => Key::Digit(6),
        26 => Key::Digit(7),
        28 => Key::Digit(8),
        25 => Key::Digit(9),
        82 => Key::NumpadDigit(0),
        83 => Key::NumpadDigit(1),
        84 => Key::NumpadDigit(2),
        85 => Key::NumpadDigit(3),
        86 => Key::NumpadDigit(4),
        87 => Key::NumpadDigit(5),
        88 => Key::NumpadDigit(6),
        89 => Key::NumpadDigit(7),
        91 => Key::NumpadDigit(8),
        92 => Key::NumpadDigit(9),
        55 => Key::LeftAlt,
        54 => Key::RightAlt,
        58 => Key::LeftWindows,
        61 => Key::RightWindows,
        59 => Key::LeftControl,
        62 => Key::RightControl,
        56 => Key::LeftShift,
        60 => Key::RightShift,
        other => Key::Other(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switcher_keys_map_to_the_shared_model() {
        assert_eq!(key_for_code(48), Key::Tab);
        assert_eq!(key_for_code(76), Key::Enter);
        assert_eq!(key_for_code(53), Key::Escape);
        assert_eq!(key_for_code(118), Key::F4);
        assert_eq!(key_for_code(101), Key::Function(9));
        assert_eq!(key_for_code(18), Key::Digit(1));
        assert_eq!(key_for_code(92), Key::NumpadDigit(9));
        assert_eq!(key_for_code(55), Key::LeftAlt);
        assert_eq!(key_for_code(58), Key::LeftWindows);
        assert_eq!(key_for_code(0), Key::Other(0));
    }
}
