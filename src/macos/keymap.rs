//! The macOS virtual key codes the switcher reacts to.

use alttabio::input::WindowCommand;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacKey {
    Tab,
    Return,
    Escape,
    Left,
    Right,
    Up,
    Down,
    /// The key left of 1, which steps backwards in the system app switcher.
    Backtick,
    /// 1 to 9 on the number row or the keypad.
    Digit(u8),
    /// A letter that runs a command while the switch modifier is down, as in the system switcher.
    Command(WindowCommand),
    Other,
}

#[must_use]
pub const fn key_for_code(code: u16) -> MacKey {
    match code {
        48 => MacKey::Tab,
        36 | 76 => MacKey::Return,
        53 => MacKey::Escape,
        123 => MacKey::Left,
        124 => MacKey::Right,
        126 => MacKey::Up,
        125 => MacKey::Down,
        50 => MacKey::Backtick,
        18 | 83 => MacKey::Digit(1),
        19 | 84 => MacKey::Digit(2),
        20 | 85 => MacKey::Digit(3),
        21 | 86 => MacKey::Digit(4),
        23 | 87 => MacKey::Digit(5),
        22 | 88 => MacKey::Digit(6),
        26 | 89 => MacKey::Digit(7),
        28 | 91 => MacKey::Digit(8),
        25 | 92 => MacKey::Digit(9),
        13 => MacKey::Command(WindowCommand::Close),
        46 => MacKey::Command(WindowCommand::Minimize),
        4 => MacKey::Command(WindowCommand::Hide),
        12 => MacKey::Command(WindowCommand::Quit),
        _ => MacKey::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switcher_keys_and_command_letters_are_recognised() {
        assert_eq!(key_for_code(48), MacKey::Tab);
        assert_eq!(key_for_code(76), MacKey::Return);
        assert_eq!(key_for_code(53), MacKey::Escape);
        assert_eq!(key_for_code(126), MacKey::Up);
        assert_eq!(key_for_code(50), MacKey::Backtick);
        assert_eq!(key_for_code(18), MacKey::Digit(1));
        assert_eq!(key_for_code(92), MacKey::Digit(9));
        assert_eq!(key_for_code(29), MacKey::Other);
        assert_eq!(key_for_code(13), MacKey::Command(WindowCommand::Close));
        assert_eq!(key_for_code(12), MacKey::Command(WindowCommand::Quit));
        assert_eq!(key_for_code(0), MacKey::Other);
    }
}
