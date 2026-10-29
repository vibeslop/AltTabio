//! Which open dialogs and menus block the switcher.

/// Settings stays independent; update prompts and context menus own switcher input.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DialogState {
    pub settings_dialog: bool,
    pub blocking_dialog: bool,
    pub context_menu: bool,
}

impl DialogState {
    /// Gates opening another dialog. Unlike `any_open`, an open task context menu does not count.
    #[must_use]
    pub const fn dialog_open(self) -> bool {
        self.settings_dialog || self.blocking_dialog
    }

    /// Gates background updates and restarts while any dialog or menu is open.
    #[must_use]
    pub const fn any_open(self) -> bool {
        self.dialog_open() || self.context_menu
    }

    /// Settings allows switching; blocking dialogs and menus retain their input.
    #[must_use]
    pub const fn blocks_switcher(self) -> bool {
        self.blocking_dialog || self.context_menu
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_allows_switching_while_deferring_updates_and_other_dialogs() {
        let settings = DialogState {
            settings_dialog: true,
            ..DialogState::default()
        };
        assert!(!settings.blocks_switcher());
        assert!(settings.any_open());
        assert!(settings.dialog_open());
    }

    #[test]
    fn a_context_menu_gates_hook_actions_but_not_dialogs() {
        let menu = DialogState {
            context_menu: true,
            ..DialogState::default()
        };

        assert!(menu.any_open());
        assert!(menu.blocks_switcher());
        assert!(!menu.dialog_open());
    }

    #[test]
    fn blocking_dialogs_suspend_switching_with_or_without_settings() {
        assert!(!DialogState::default().blocks_switcher());
        for state in [
            DialogState {
                blocking_dialog: true,
                ..DialogState::default()
            },
            DialogState {
                settings_dialog: true,
                blocking_dialog: true,
                ..DialogState::default()
            },
        ] {
            assert!(state.dialog_open());
            assert!(state.blocks_switcher());
            assert!(state.any_open());
        }
        assert!(!DialogState::default().any_open());
    }
}
