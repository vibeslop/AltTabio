//! Commands the switcher can run on the selected window or its application.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowCommand {
    Close,
    Minimize,
    Maximize,
    Restore,
    Terminate,
    Run,
    /// Asks the window's application to quit; unlike Terminate it may prompt to save.
    Quit,
    /// Hides every window of the application (macOS app hiding).
    Hide,
}

impl WindowCommand {
    #[must_use]
    pub(crate) const fn from_function_key(number: u8) -> Option<Self> {
        match number {
            4 => Some(Self::Close),
            5 => Some(Self::Minimize),
            6 => Some(Self::Maximize),
            7 => Some(Self::Restore),
            8 => Some(Self::Terminate),
            9 => Some(Self::Run),
            _ => None,
        }
    }

    /// The F-key bound to the command; Quit and Hide only have modifier chords.
    #[must_use]
    pub const fn function_key(self) -> Option<u8> {
        match self {
            Self::Close => Some(4),
            Self::Minimize => Some(5),
            Self::Maximize => Some(6),
            Self::Restore => Some(7),
            Self::Terminate => Some(8),
            Self::Run => Some(9),
            Self::Quit | Self::Hide => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The hook thread posts a command as its F-key number, so the two tables must agree.
    #[test]
    fn every_function_key_maps_back_to_its_command() {
        for command in [
            WindowCommand::Close,
            WindowCommand::Minimize,
            WindowCommand::Maximize,
            WindowCommand::Restore,
            WindowCommand::Terminate,
            WindowCommand::Run,
        ] {
            assert_eq!(
                command
                    .function_key()
                    .and_then(WindowCommand::from_function_key),
                Some(command)
            );
        }
        assert_eq!(WindowCommand::Quit.function_key(), None);
        assert_eq!(WindowCommand::Hide.function_key(), None);
        assert_eq!(WindowCommand::from_function_key(3), None);
        assert_eq!(WindowCommand::from_function_key(10), None);
    }
}
