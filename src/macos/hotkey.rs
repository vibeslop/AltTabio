//! Pure Command+Tab and Option+Tab gesture state fed by the event-tap adapter.
//!
//! macOS delivers modifiers as flag changes and never focuses menus on a bare modifier, so this
//! machine only decides which events the switcher owns and what they mean to it. The keys are the
//! ones the system app switcher already taught: Tab and the backtick step through apps, the arrows
//! move, Return or letting go switches, Escape cancels, and W, M, H, and Q act on the selection.

use super::keymap::MacKey;
use alttabio::app_switcher::Action;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HotkeySettings {
    pub command_tab: bool,
    pub option_tab: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "fields mirror the independent modifier flags of one event"
)]
pub struct ModifierState {
    pub command: bool,
    pub option: bool,
    pub shift: bool,
    pub control: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TapEvent {
    KeyDown { key: MacKey, repeated: bool },
    KeyUp,
    ModifiersChanged(ModifierState),
    LeftMouseDown { inside_overlay: bool },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TapOutcome {
    pub suppress: bool,
    pub action: Option<Action>,
}

impl TapOutcome {
    const SUPPRESSED: Self = Self {
        suppress: true,
        action: None,
    };

    const fn with(suppress: bool, action: Action) -> Self {
        Self {
            suppress,
            action: Some(action),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Gesture {
    Command,
    Option,
}

#[derive(Debug, Default)]
pub struct HotkeyState {
    modifiers: ModifierState,
    gesture: Option<Gesture>,
    overlay_active: bool,
}

impl HotkeyState {
    /// The overlay owns every key while it is visible, even after the gesture modifier is up.
    pub fn set_overlay_active(&mut self, active: bool) {
        self.overlay_active = active;
        if !active {
            self.gesture = None;
        }
    }

    /// Whether Command or Option is down as the switcher's modifier: the gesture's own, or one
    /// pressed again while a list opened from the menu bar shows.
    const fn modifier_held(&self) -> bool {
        self.gesture.is_some()
            || (self.overlay_active && (self.modifiers.command || self.modifiers.option))
    }

    #[must_use]
    pub fn process(&mut self, event: TapEvent, settings: HotkeySettings) -> TapOutcome {
        match event {
            TapEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
                let released = match self.gesture {
                    Some(Gesture::Command) => !modifiers.command,
                    Some(Gesture::Option) => !modifiers.option,
                    None => false,
                };
                if released {
                    self.gesture = None;
                    TapOutcome::with(false, Action::Activate)
                } else {
                    TapOutcome::default()
                }
            }
            TapEvent::KeyDown { key, repeated } => self.process_key_down(key, repeated, settings),
            TapEvent::KeyUp => {
                if self.gesture.is_some() || self.overlay_active {
                    TapOutcome::SUPPRESSED
                } else {
                    TapOutcome::default()
                }
            }
            TapEvent::LeftMouseDown { inside_overlay } => {
                if self.overlay_active && !inside_overlay {
                    TapOutcome::with(false, Action::Dismiss)
                } else {
                    TapOutcome::default()
                }
            }
        }
    }

    fn process_key_down(
        &mut self,
        key: MacKey,
        repeated: bool,
        settings: HotkeySettings,
    ) -> TapOutcome {
        let modifiers = self.modifiers;
        let forward = if modifiers.shift { -1 } else { 1 };
        if key == MacKey::Tab && self.gesture.is_none() {
            if modifiers.command && !modifiers.option && settings.command_tab {
                self.gesture = Some(Gesture::Command);
                return TapOutcome::with(true, Action::StepApp(forward));
            }
            if modifiers.option && !modifiers.command && settings.option_tab {
                self.gesture = Some(Gesture::Option);
                return TapOutcome::with(true, Action::StepApp(forward));
            }
        }
        if self.gesture.is_none() && !self.overlay_active {
            return TapOutcome::default();
        }

        // The switcher owns the keyboard from here until it hides.
        let action = match key {
            MacKey::Tab | MacKey::Right => Some(Action::StepApp(forward)),
            MacKey::Backtick | MacKey::Left => Some(Action::StepApp(-forward)),
            MacKey::Up => Some(Action::StepWindow(-1)),
            MacKey::Down => Some(Action::StepWindow(1)),
            MacKey::Return if !repeated => Some(Action::Activate),
            MacKey::Escape if !repeated => Some(Action::Dismiss),
            MacKey::Command(command) if !repeated && !modifiers.control && self.modifier_held() => {
                Some(Action::Command(command))
            }
            _ => None,
        };
        if matches!(action, Some(Action::Activate | Action::Dismiss)) {
            // A later Tab while the modifier stays down starts a new gesture.
            self.gesture = None;
        }
        action.map_or(TapOutcome::SUPPRESSED, |action| {
            TapOutcome::with(true, action)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alttabio::input::WindowCommand;

    fn settings() -> HotkeySettings {
        HotkeySettings {
            command_tab: true,
            option_tab: true,
        }
    }

    fn command_down() -> ModifierState {
        ModifierState {
            command: true,
            ..ModifierState::default()
        }
    }

    fn key(key: MacKey) -> TapEvent {
        TapEvent::KeyDown {
            key,
            repeated: false,
        }
    }

    fn action(outcome: TapOutcome) -> Option<Action> {
        outcome.action
    }

    /// Command down, Tab pressed, and the overlay showing.
    fn in_gesture() -> HotkeyState {
        let mut state = HotkeyState::default();
        let _ = state.process(TapEvent::ModifiersChanged(command_down()), settings());
        let _ = state.process(key(MacKey::Tab), settings());
        state.set_overlay_active(true);
        state
    }

    #[test]
    fn command_tab_opens_and_command_release_switches() {
        let mut state = HotkeyState::default();
        let _ = state.process(TapEvent::ModifiersChanged(command_down()), settings());

        let outcome = state.process(key(MacKey::Tab), settings());
        assert!(outcome.suppress);
        assert_eq!(action(outcome), Some(Action::StepApp(1)));
        state.set_overlay_active(true);
        let outcome = state.process(
            TapEvent::ModifiersChanged(ModifierState::default()),
            settings(),
        );
        assert!(!outcome.suppress);
        assert_eq!(action(outcome), Some(Action::Activate));
    }

    #[test]
    fn shift_tab_and_the_backtick_step_backwards() {
        let mut state = in_gesture();

        assert_eq!(
            action(state.process(key(MacKey::Backtick), settings())),
            Some(Action::StepApp(-1))
        );
        let _ = state.process(
            TapEvent::ModifiersChanged(ModifierState {
                command: true,
                shift: true,
                ..ModifierState::default()
            }),
            settings(),
        );
        assert_eq!(
            action(state.process(key(MacKey::Tab), settings())),
            Some(Action::StepApp(-1))
        );
        assert_eq!(
            action(state.process(key(MacKey::Backtick), settings())),
            Some(Action::StepApp(1))
        );
    }

    #[test]
    fn side_arrows_step_apps_and_vertical_arrows_step_windows() {
        let mut state = in_gesture();

        assert_eq!(
            action(state.process(key(MacKey::Left), settings())),
            Some(Action::StepApp(-1))
        );
        assert_eq!(
            action(state.process(key(MacKey::Right), settings())),
            Some(Action::StepApp(1))
        );
        assert_eq!(
            action(state.process(key(MacKey::Up), settings())),
            Some(Action::StepWindow(-1))
        );
        assert_eq!(
            action(state.process(key(MacKey::Down), settings())),
            Some(Action::StepWindow(1))
        );
    }

    #[test]
    fn disabled_command_tab_passes_through() {
        let mut state = HotkeyState::default();
        let settings = HotkeySettings {
            command_tab: false,
            option_tab: true,
        };
        let _ = state.process(TapEvent::ModifiersChanged(command_down()), settings);

        assert_eq!(
            state.process(key(MacKey::Tab), settings),
            TapOutcome::default()
        );
    }

    #[test]
    fn option_tab_is_the_second_gesture() {
        let mut state = HotkeyState::default();
        let _ = state.process(
            TapEvent::ModifiersChanged(ModifierState {
                option: true,
                ..ModifierState::default()
            }),
            settings(),
        );

        assert_eq!(
            action(state.process(key(MacKey::Tab), settings())),
            Some(Action::StepApp(1))
        );
        assert_eq!(
            action(state.process(
                TapEvent::ModifiersChanged(ModifierState::default()),
                settings()
            )),
            Some(Action::Activate)
        );
    }

    #[test]
    fn command_letters_act_once_and_other_keys_are_swallowed() {
        let mut state = in_gesture();

        assert_eq!(
            action(state.process(key(MacKey::Command(WindowCommand::Close)), settings())),
            Some(Action::Command(WindowCommand::Close))
        );
        let repeated = TapEvent::KeyDown {
            key: MacKey::Command(WindowCommand::Close),
            repeated: true,
        };
        assert_eq!(state.process(repeated, settings()), TapOutcome::SUPPRESSED);
        assert_eq!(
            state.process(key(MacKey::Other), settings()),
            TapOutcome::SUPPRESSED
        );
        assert_eq!(
            state.process(TapEvent::KeyUp, settings()),
            TapOutcome::SUPPRESSED
        );
    }

    #[test]
    fn return_switches_and_ends_the_gesture() {
        let mut state = in_gesture();

        assert_eq!(
            action(state.process(key(MacKey::Return), settings())),
            Some(Action::Activate)
        );
        // Letting go afterwards does not switch a second time; Tab opens anew.
        assert_eq!(
            action(state.process(
                TapEvent::ModifiersChanged(ModifierState::default()),
                settings()
            )),
            None
        );
        let _ = state.process(TapEvent::ModifiersChanged(command_down()), settings());
        assert_eq!(
            action(state.process(key(MacKey::Tab), settings())),
            Some(Action::StepApp(1))
        );
    }

    #[test]
    fn a_list_opened_from_the_menu_takes_letters_only_with_command() {
        let mut state = HotkeyState::default();
        state.set_overlay_active(true);
        let quit = key(MacKey::Command(WindowCommand::Quit));

        assert_eq!(state.process(quit, settings()), TapOutcome::SUPPRESSED);
        let _ = state.process(TapEvent::ModifiersChanged(command_down()), settings());
        assert_eq!(
            action(state.process(quit, settings())),
            Some(Action::Command(WindowCommand::Quit))
        );
        // Command coming up here was never a gesture, so it does not switch.
        assert_eq!(
            action(state.process(
                TapEvent::ModifiersChanged(ModifierState::default()),
                settings()
            )),
            None
        );
    }

    #[test]
    fn keys_pass_through_after_the_overlay_hides() {
        let mut state = in_gesture();
        state.set_overlay_active(false);
        let _ = state.process(
            TapEvent::ModifiersChanged(ModifierState::default()),
            settings(),
        );

        assert_eq!(
            state.process(key(MacKey::Other), settings()),
            TapOutcome::default()
        );
        assert_eq!(
            state.process(TapEvent::KeyUp, settings()),
            TapOutcome::default()
        );
    }

    #[test]
    fn clicking_outside_the_overlay_dismisses_it() {
        let mut state = HotkeyState::default();
        state.set_overlay_active(true);

        assert_eq!(
            state.process(
                TapEvent::LeftMouseDown {
                    inside_overlay: true
                },
                settings()
            ),
            TapOutcome::default()
        );
        assert_eq!(
            action(state.process(
                TapEvent::LeftMouseDown {
                    inside_overlay: false
                },
                settings()
            )),
            Some(Action::Dismiss)
        );
    }
}
