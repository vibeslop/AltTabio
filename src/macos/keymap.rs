//! The macOS keys the switcher reacts to.
//!
//! Tab, Return, Escape, the arrows, the backtick, and the digits are matched by key position.
//! The command letters are matched by the letter the keyboard layout types, as macOS matches
//! menu shortcuts, so ⌘ Q quits on the key labeled Q on AZERTY and Dvorak too.

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

/// The key for virtual key `code`, which types `typed` under the current layout with every
/// modifier but Shift ignored.
///
/// A layout that types no Latin letter on the key, such as Russian or Greek, falls back to the
/// letter at that position on a US keyboard, as macOS does for shortcuts under those layouts.
#[must_use]
pub fn key_for(code: u16, typed: Option<char>) -> MacKey {
    match typed.map(|character| character.to_ascii_lowercase()) {
        Some(letter) if letter.is_ascii_alphabetic() => {
            command_for_letter(letter).map_or(MacKey::Other, MacKey::Command)
        }
        _ => key_for_code(code),
    }
}

const fn command_for_letter(letter: char) -> Option<WindowCommand> {
    match letter {
        'w' => Some(WindowCommand::Close),
        'm' => Some(WindowCommand::Minimize),
        'h' => Some(WindowCommand::Hide),
        'q' => Some(WindowCommand::Quit),
        _ => None,
    }
}

const fn key_for_code(code: u16) -> MacKey {
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
        // The US positions of W, M, H, and Q, for layouts without Latin letters.
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
    fn switcher_keys_are_recognised_by_position() {
        assert_eq!(key_for(48, Some('\t')), MacKey::Tab);
        assert_eq!(key_for(76, Some('\r')), MacKey::Return);
        assert_eq!(key_for(53, None), MacKey::Escape);
        assert_eq!(key_for(126, None), MacKey::Up);
        assert_eq!(key_for(50, Some('`')), MacKey::Backtick);
        assert_eq!(key_for(18, Some('1')), MacKey::Digit(1));
        assert_eq!(key_for(92, Some('9')), MacKey::Digit(9));
        assert_eq!(key_for(29, Some('0')), MacKey::Other);
        // French AZERTY types & and é on the number row; the digits stay where they are.
        assert_eq!(key_for(18, Some('&')), MacKey::Digit(1));
        assert_eq!(key_for(19, Some('é')), MacKey::Digit(2));
    }

    #[test]
    fn command_letters_follow_the_layout() {
        assert_eq!(
            key_for(13, Some('w')),
            MacKey::Command(WindowCommand::Close)
        );
        assert_eq!(key_for(12, Some('Q')), MacKey::Command(WindowCommand::Quit));
        // AZERTY: Q sits where US has A, and A where US has Q.
        assert_eq!(key_for(0, Some('q')), MacKey::Command(WindowCommand::Quit));
        assert_eq!(key_for(12, Some('a')), MacKey::Other);
        // Dvorak types M on the US M key but H on the US J key.
        assert_eq!(
            key_for(46, Some('m')),
            MacKey::Command(WindowCommand::Minimize)
        );
        assert_eq!(key_for(38, Some('h')), MacKey::Command(WindowCommand::Hide));
        assert_eq!(key_for(4, Some('d')), MacKey::Other);
    }

    #[test]
    fn layouts_without_latin_letters_use_the_us_positions() {
        assert_eq!(key_for(12, Some('й')), MacKey::Command(WindowCommand::Quit));
        assert_eq!(
            key_for(13, Some('ц')),
            MacKey::Command(WindowCommand::Close)
        );
        assert_eq!(key_for(12, None), MacKey::Command(WindowCommand::Quit));
        assert_eq!(key_for(0, Some('ф')), MacKey::Other);
    }
}
