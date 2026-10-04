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
