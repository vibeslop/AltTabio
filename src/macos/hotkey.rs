//! Pure Command+Tab and Option+Tab gesture state fed by the event-tap adapter.
//!
//! The Windows hook state machine owns Alt and Tab presses and replays them to keep Windows'
//! menu focus quirks at bay. macOS delivers modifiers as flag changes and never focuses menus on
//! a bare modifier, so this smaller machine only decides which events the switcher owns.

use super::keymap::{Chord, chord_for_code};
use alttabio::input::{InputAction, Key, OverlayKeyEvent, WindowCommand, overlay_key_action};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "fields are independent hotkey feature switches"
)]
pub struct HotkeySettings {
    pub command_tab: bool,
    pub option_tab: bool,
    pub typed_search: bool,
    pub right_button_wheel_switching: bool,
}

impl Default for HotkeySettings {
    fn default() -> Self {
        Self {
            command_tab: true,
            option_tab: true,
            typed_search: true,
            right_button_wheel_switching: false,
        }
    }
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
    KeyDown {
        key: Key,
        text: Option<char>,
        repeated: bool,
    },
    KeyUp,
    ModifiersChanged(ModifierState),
    LeftMouseDown {
        inside_overlay: bool,
    },
    RightMouseDown,
    RightMouseUp,
    ScrollWheel(i32),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TapOutcome {
    pub suppress: bool,
    actions: [Option<InputAction>; 2],
}

impl TapOutcome {
    const SUPPRESSED: Self = Self {
        suppress: true,
        actions: [None, None],
    };

    const fn one(suppress: bool, action: InputAction) -> Self {
        Self {
            suppress,
            actions: [Some(action), None],
        }
    }

    const fn two(suppress: bool, first: InputAction, second: InputAction) -> Self {
        Self {
            suppress,
            actions: [Some(first), Some(second)],
        }
    }

    pub fn actions(&self) -> impl Iterator<Item = InputAction> + '_ {
        self.actions.iter().flatten().copied()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Gesture {
    Command,
    Option,
}

/// Which switch modifier is down while the overlay shows; drives keycaps and hints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeldModifier {
    Command,
    Option,
}

impl HeldModifier {
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Command => "⌘",
            Self::Option => "⌥",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum RightButton {
    #[default]
    Released,
    Pressed,
    WheelGesture,
}

#[derive(Debug, Default)]
pub struct HotkeyState {
    modifiers: ModifierState,
    gesture: Option<Gesture>,
    overlay_active: bool,
    right_button: RightButton,
    synthetic_right_release: bool,
}

impl HotkeyState {
    /// The overlay owns every key while it is visible, even after the gesture modifier is up.
    pub fn set_overlay_active(&mut self, active: bool) {
        self.overlay_active = active;
        if !active {
            self.gesture = None;
        }
    }

    /// The modifier the switcher currently treats as held: the gesture's own modifier, or
    /// Command or Option pressed again after the gesture ended while the list stayed open.
    #[must_use]
    pub const fn held_modifier(&self) -> Option<HeldModifier> {
        match self.gesture {
            Some(Gesture::Command) => Some(HeldModifier::Command),
            Some(Gesture::Option) => Some(HeldModifier::Option),
            None if self.overlay_active && self.modifiers.command => Some(HeldModifier::Command),
            None if self.overlay_active && self.modifiers.option => Some(HeldModifier::Option),
            None => None,
        }
    }

    /// The right button press passed through to the app under the cursor. A wheel gesture then
    /// needs a synthetic release so that app does not see a stuck button while the switcher
    /// consumes the real release later.
    pub fn take_synthetic_right_release(&mut self) -> bool {
        core::mem::take(&mut self.synthetic_right_release)
    }

    #[must_use]
    pub fn process(&mut self, event: TapEvent, settings: HotkeySettings) -> TapOutcome {
        match event {
            TapEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
                match self.gesture {
                    Some(Gesture::Command) if !modifiers.command => {
                        self.gesture = None;
                        TapOutcome::one(false, InputAction::AltReleased)
                    }
                    Some(Gesture::Option) if !modifiers.option => {
                        self.gesture = None;
                        TapOutcome::one(false, InputAction::AltReleased)
                    }
                    _ => TapOutcome::default(),
                }
            }
            TapEvent::KeyDown {
                key,
                text,
                repeated,
            } => self.process_key_down(key, text, repeated, settings),
            TapEvent::KeyUp => {
                if self.gesture.is_some() || self.overlay_active {
                    TapOutcome::SUPPRESSED
                } else {
                    TapOutcome::default()
                }
            }
            TapEvent::LeftMouseDown { inside_overlay } => {
                if self.overlay_active && !inside_overlay {
                    TapOutcome::one(false, InputAction::DismissOverlay)
                } else {
                    TapOutcome::default()
                }
            }
            TapEvent::RightMouseDown => {
                if settings.right_button_wheel_switching
                    && self.right_button == RightButton::Released
                {
                    self.right_button = RightButton::Pressed;
                }
                TapOutcome::default()
            }
            TapEvent::RightMouseUp => {
                let outcome = if self.right_button == RightButton::WheelGesture {
                    TapOutcome::one(true, InputAction::RightButtonReleased)
                } else {
                    TapOutcome::default()
                };
                self.right_button = RightButton::Released;
                outcome
            }
            TapEvent::ScrollWheel(delta) => match self.right_button {
                RightButton::Pressed if settings.right_button_wheel_switching => {
                    self.right_button = RightButton::WheelGesture;
                    self.synthetic_right_release = true;
                    TapOutcome::two(
                        true,
                        InputAction::RightButtonPressed,
                        InputAction::MouseWheel(delta),
                    )
                }
                RightButton::WheelGesture => TapOutcome::one(true, InputAction::MouseWheel(delta)),
                RightButton::Pressed | RightButton::Released => TapOutcome::default(),
            },
        }
    }

    fn process_key_down(
        &mut self,
        key: Key,
        text: Option<char>,
        repeated: bool,
        settings: HotkeySettings,
    ) -> TapOutcome {
        let modifiers = self.modifiers;
        let switch_delta = if modifiers.shift { -1 } else { 1 };
        if key == Key::Tab && self.gesture.is_none() {
            if modifiers.command && !modifiers.option && settings.command_tab {
                self.gesture = Some(Gesture::Command);
                return TapOutcome::one(true, InputAction::Switch(switch_delta));
            }
            if modifiers.option && !modifiers.command && settings.option_tab {
                self.gesture = Some(Gesture::Option);
                return TapOutcome::one(true, InputAction::Switch(switch_delta));
            }
        }
        if self.gesture.is_none() && !self.overlay_active {
            return TapOutcome::default();
        }

        // The switcher owns the keyboard from here until it hides.
        if key == Key::Tab {
            return TapOutcome::one(true, InputAction::Switch(switch_delta));
        }
        let modifier_held = self.held_modifier().is_some();
        if modifier_held
            && !modifiers.control
            && let Key::Other(code) = key
            && let Some(chord) = chord_for_code(code)
        {
            let action = match chord {
                Chord::Close => InputAction::WindowCommand(WindowCommand::Close),
                Chord::Minimize => InputAction::WindowCommand(WindowCommand::Minimize),
                Chord::Quit => InputAction::WindowCommand(WindowCommand::Quit),
                Chord::Hide => InputAction::WindowCommand(WindowCommand::Hide),
                Chord::NextWindowOfApp => InputAction::SwitchWithinProcess(switch_delta),
                Chord::Actions => InputAction::ToggleActionPanel,
            };
            if repeated && chord != Chord::NextWindowOfApp {
                return TapOutcome::SUPPRESSED;
            }
            return TapOutcome::one(true, action);
        }
        let overlay_action = overlay_key_action(OverlayKeyEvent {
            key,
            repeated,
            shift: modifiers.shift,
        });
        match overlay_action {
            // Digits filter the list once the modifier is up, like the Windows build; with the
            // modifier down they jump, in the gesture and after the list was left open.
            Some(InputAction::ActivateVisiblePosition(_))
                if !modifier_held && settings.typed_search => {}
            Some(
                action @ (InputAction::ActivateSelected
                | InputAction::DismissOverlay
                | InputAction::ActivateVisiblePosition(_)),
            ) => {
                self.gesture = None;
                return TapOutcome::one(true, action);
            }
            Some(action) => return TapOutcome::one(true, action),
            None => {}
        }
        // Letters under the modifier are chords on macOS, never search text; typing starts once
        // the modifier is up so a search for "w" cannot close a window.
        if settings.typed_search && !modifiers.control && !modifier_held {
            if key == Key::Backspace {
                return TapOutcome::one(true, InputAction::BackspaceSearch);
            }
            if let Some(character) = text.filter(|value| !value.is_control()) {
                return TapOutcome::one(true, InputAction::AppendSearchCharacter(character));
            }
        }
        TapOutcome::SUPPRESSED
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_down() -> ModifierState {
        ModifierState {
            command: true,
            ..ModifierState::default()
        }
    }

    fn tab() -> TapEvent {
        TapEvent::KeyDown {
            key: Key::Tab,
            text: Some('\t'),
            repeated: false,
        }
    }

    fn actions(outcome: TapOutcome) -> Vec<InputAction> {
        outcome.actions().collect()
    }

    #[test]
    fn command_tab_opens_and_command_release_activates() {
        let mut state = HotkeyState::default();
        let settings = HotkeySettings::default();

        assert_eq!(
            state.process(TapEvent::ModifiersChanged(command_down()), settings),
            TapOutcome::default()
        );
        let outcome = state.process(tab(), settings);
        assert!(outcome.suppress);
        assert_eq!(actions(outcome), vec![InputAction::Switch(1)]);
        state.set_overlay_active(true);
        let outcome = state.process(
            TapEvent::ModifiersChanged(ModifierState::default()),
            settings,
        );
        assert!(!outcome.suppress);
        assert_eq!(actions(outcome), vec![InputAction::AltReleased]);
    }

    #[test]
    fn shift_tab_switches_backwards() {
        let mut state = HotkeyState::default();
        let settings = HotkeySettings::default();
        let _ = state.process(
            TapEvent::ModifiersChanged(ModifierState {
                command: true,
                shift: true,
                ..ModifierState::default()
            }),
            settings,
        );

        assert_eq!(
            actions(state.process(tab(), settings)),
            vec![InputAction::Switch(-1)]
        );
    }

    #[test]
    fn disabled_command_tab_passes_through() {
        let mut state = HotkeyState::default();
        let settings = HotkeySettings {
            command_tab: false,
            ..HotkeySettings::default()
        };
        let _ = state.process(TapEvent::ModifiersChanged(command_down()), settings);

        assert_eq!(state.process(tab(), settings), TapOutcome::default());
    }

    #[test]
    fn option_tab_is_the_secondary_gesture() {
        let mut state = HotkeyState::default();
        let settings = HotkeySettings::default();
        let _ = state.process(
            TapEvent::ModifiersChanged(ModifierState {
                option: true,
                ..ModifierState::default()
            }),
            settings,
        );

        assert_eq!(
            actions(state.process(tab(), settings)),
            vec![InputAction::Switch(1)]
        );
        let outcome = state.process(
            TapEvent::ModifiersChanged(ModifierState::default()),
            settings,
        );
        assert_eq!(actions(outcome), vec![InputAction::AltReleased]);
    }

    #[test]
    fn overlay_keys_are_owned_while_the_gesture_is_active() {
        let mut state = HotkeyState::default();
        let settings = HotkeySettings::default();
        let _ = state.process(TapEvent::ModifiersChanged(command_down()), settings);
        let _ = state.process(tab(), settings);

        let down = TapEvent::KeyDown {
            key: Key::DownArrow,
            text: None,
            repeated: false,
        };
        assert_eq!(
            actions(state.process(down, settings)),
            vec![InputAction::Navigate(1)]
        );
        let digit = TapEvent::KeyDown {
            key: Key::Digit(3),
            text: Some('3'),
            repeated: false,
        };
        assert_eq!(
            actions(state.process(digit, settings)),
            vec![InputAction::ActivateVisiblePosition(3)]
        );
        // Activation ends the gesture; a later Tab while Command stays down opens again.
        assert_eq!(
            actions(state.process(tab(), settings)),
            vec![InputAction::Switch(1)]
        );
    }

    #[test]
    fn typed_characters_filter_and_unknown_keys_are_swallowed() {
        let mut state = HotkeyState::default();
        let settings = HotkeySettings::default();
        let _ = state.process(TapEvent::ModifiersChanged(command_down()), settings);
        let _ = state.process(tab(), settings);
        state.set_overlay_active(true);
        let _ = state.process(
            TapEvent::ModifiersChanged(ModifierState::default()),
            settings,
        );

        let letter = TapEvent::KeyDown {
            key: Key::Other(0),
            text: Some('a'),
            repeated: false,
        };
        assert_eq!(
            actions(state.process(letter, settings)),
            vec![InputAction::AppendSearchCharacter('a')]
        );
        let backspace = TapEvent::KeyDown {
            key: Key::Backspace,
            text: Some('\u{8}'),
            repeated: false,
        };
        assert_eq!(
            actions(state.process(backspace, settings)),
            vec![InputAction::BackspaceSearch]
        );
        let function = TapEvent::KeyDown {
            key: Key::Function(12),
            text: None,
            repeated: false,
        };
        assert_eq!(state.process(function, settings), TapOutcome::SUPPRESSED);
        assert_eq!(
            state.process(TapEvent::KeyUp, settings),
            TapOutcome::SUPPRESSED
        );
    }

    #[test]
    fn digits_filter_once_the_modifier_is_released_but_the_overlay_stays() {
        let mut state = HotkeyState::default();
        let settings = HotkeySettings::default();
        let _ = state.process(TapEvent::ModifiersChanged(command_down()), settings);
        let _ = state.process(tab(), settings);
        state.set_overlay_active(true);
        let _ = state.process(
            TapEvent::ModifiersChanged(ModifierState::default()),
            settings,
        );

        let digit = TapEvent::KeyDown {
            key: Key::Digit(2),
            text: Some('2'),
            repeated: false,
        };
        assert_eq!(
            actions(state.process(digit, settings)),
            vec![InputAction::AppendSearchCharacter('2')]
        );
        let escape = TapEvent::KeyDown {
            key: Key::Escape,
            text: None,
            repeated: false,
        };
        assert_eq!(
            actions(state.process(escape, settings)),
            vec![InputAction::DismissOverlay]
        );
    }

    #[test]
    fn letters_under_the_held_modifier_are_chords_not_search_text() {
        let mut state = HotkeyState::default();
        let settings = HotkeySettings::default();
        let _ = state.process(TapEvent::ModifiersChanged(command_down()), settings);
        let _ = state.process(tab(), settings);
        state.set_overlay_active(true);
        assert_eq!(state.held_modifier(), Some(HeldModifier::Command));

        let w = TapEvent::KeyDown {
            key: Key::Other(13),
            text: Some('w'),
            repeated: false,
        };
        assert_eq!(
            actions(state.process(w, settings)),
            vec![InputAction::WindowCommand(WindowCommand::Close)]
        );
        let repeated_w = TapEvent::KeyDown {
            key: Key::Other(13),
            text: Some('w'),
            repeated: true,
        };
        assert_eq!(state.process(repeated_w, settings), TapOutcome::SUPPRESSED);
        let a = TapEvent::KeyDown {
            key: Key::Other(0),
            text: Some('a'),
            repeated: false,
        };
        assert_eq!(state.process(a, settings), TapOutcome::SUPPRESSED);
        let backtick = TapEvent::KeyDown {
            key: Key::Other(50),
            text: Some('`'),
            repeated: true,
        };
        assert_eq!(
            actions(state.process(backtick, settings)),
            vec![InputAction::SwitchWithinProcess(1)]
        );
    }

    #[test]
    fn command_pressed_again_after_release_restores_jumps_and_chords() {
        let mut state = HotkeyState::default();
        let settings = HotkeySettings::default();
        let _ = state.process(TapEvent::ModifiersChanged(command_down()), settings);
        let _ = state.process(tab(), settings);
        state.set_overlay_active(true);
        let _ = state.process(
            TapEvent::ModifiersChanged(ModifierState::default()),
            settings,
        );
        assert_eq!(state.held_modifier(), None);

        let _ = state.process(TapEvent::ModifiersChanged(command_down()), settings);
        assert_eq!(state.held_modifier(), Some(HeldModifier::Command));
        let q = TapEvent::KeyDown {
            key: Key::Other(12),
            text: Some('q'),
            repeated: false,
        };
        assert_eq!(
            actions(state.process(q, settings)),
            vec![InputAction::WindowCommand(WindowCommand::Quit)]
        );
        let digit = TapEvent::KeyDown {
            key: Key::Digit(2),
            text: Some('2'),
            repeated: false,
        };
        assert_eq!(
            actions(state.process(digit, settings)),
            vec![InputAction::ActivateVisiblePosition(2)]
        );
    }

    #[test]
    fn keys_pass_through_after_the_overlay_hides() {
        let mut state = HotkeyState::default();
        let settings = HotkeySettings::default();
        let _ = state.process(TapEvent::ModifiersChanged(command_down()), settings);
        let _ = state.process(tab(), settings);
        state.set_overlay_active(true);
        state.set_overlay_active(false);
        let _ = state.process(
            TapEvent::ModifiersChanged(ModifierState::default()),
            settings,
        );

        let letter = TapEvent::KeyDown {
            key: Key::Other(0),
            text: Some('a'),
            repeated: false,
        };
        assert_eq!(state.process(letter, settings), TapOutcome::default());
        assert_eq!(
            state.process(TapEvent::KeyUp, settings),
            TapOutcome::default()
        );
    }

    #[test]
    fn clicking_outside_the_overlay_dismisses_it() {
        let mut state = HotkeyState::default();
        let settings = HotkeySettings::default();
        state.set_overlay_active(true);

        let inside = TapEvent::LeftMouseDown {
            inside_overlay: true,
        };
        assert_eq!(state.process(inside, settings), TapOutcome::default());
        let outside = TapEvent::LeftMouseDown {
            inside_overlay: false,
        };
        assert_eq!(
            actions(state.process(outside, settings)),
            vec![InputAction::DismissOverlay]
        );
    }

    #[test]
    fn right_button_wheel_gesture_balances_the_passed_through_press() {
        let mut state = HotkeyState::default();
        let settings = HotkeySettings {
            right_button_wheel_switching: true,
            ..HotkeySettings::default()
        };

        assert_eq!(
            state.process(TapEvent::RightMouseDown, settings),
            TapOutcome::default()
        );
        let outcome = state.process(TapEvent::ScrollWheel(-1), settings);
        assert!(outcome.suppress);
        assert_eq!(
            actions(outcome),
            vec![InputAction::RightButtonPressed, InputAction::MouseWheel(-1)]
        );
        assert!(state.take_synthetic_right_release());
        assert!(!state.take_synthetic_right_release());
        assert_eq!(
            actions(state.process(TapEvent::ScrollWheel(1), settings)),
            vec![InputAction::MouseWheel(1)]
        );
        let release = state.process(TapEvent::RightMouseUp, settings);
        assert!(release.suppress);
        assert_eq!(actions(release), vec![InputAction::RightButtonReleased]);
    }

    #[test]
    fn plain_right_clicks_and_wheels_pass_through() {
        let mut state = HotkeyState::default();
        let settings = HotkeySettings::default();

        assert_eq!(
            state.process(TapEvent::RightMouseDown, settings),
            TapOutcome::default()
        );
        assert_eq!(
            state.process(TapEvent::ScrollWheel(1), settings),
            TapOutcome::default()
        );
        assert_eq!(
            state.process(TapEvent::RightMouseUp, settings),
            TapOutcome::default()
        );
    }
}
