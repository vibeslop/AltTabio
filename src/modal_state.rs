//! The modal surfaces that can be open over the switcher.

/// Settings, About, and the task context menu each run a nested message loop on the UI thread.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModalState {
    pub settings_dialog: bool,
    pub about_dialog: bool,
    pub context_menu: bool,
}

impl ModalState {
    /// Gates opening another dialog. Unlike `any_open`, an open task context menu does not count.
    #[must_use]
    pub const fn dialog_open(self) -> bool {
        self.settings_dialog || self.about_dialog
    }

    /// While any modal surface is open, hook gestures belong to it instead of the switcher.
    #[must_use]
    pub const fn any_open(self) -> bool {
        self.dialog_open() || self.context_menu
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_dialogs_gate_hook_actions() {
        assert!(!ModalState::default().any_open());
        assert!(
            ModalState {
                settings_dialog: true,
                ..ModalState::default()
            }
            .any_open()
        );
        assert!(
            ModalState {
                about_dialog: true,
                ..ModalState::default()
            }
            .any_open()
        );
    }

    #[test]
    fn a_context_menu_gates_hook_actions_but_not_dialogs() {
        let menu = ModalState {
            context_menu: true,
            ..ModalState::default()
        };

        assert!(menu.any_open());
        assert!(!menu.dialog_open());
    }

    #[test]
    fn either_dialog_blocks_the_other() {
        for state in [
            ModalState {
                settings_dialog: true,
                ..ModalState::default()
            },
            ModalState {
                about_dialog: true,
                ..ModalState::default()
            },
        ] {
            assert!(state.dialog_open());
        }
        assert!(!ModalState::default().dialog_open());
    }
}
