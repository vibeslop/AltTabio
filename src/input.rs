//! Pure state machines used by the low-level keyboard and mouse hook adapter.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "fields are independent input-hook feature switches and transient overlay state"
)]
pub struct HookSettings {
    pub replace_alt_tab: bool,
    pub replace_win_tab: bool,
    pub right_button_wheel_switching: bool,
    pub typed_search: bool,
    pub search_active: bool,
}

impl Default for HookSettings {
    fn default() -> Self {
        Self {
            replace_alt_tab: true,
            replace_win_tab: true,
            right_button_wheel_switching: true,
            typed_search: true,
            search_active: false,
        }
    }
}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplayedKeyEvent {
    pub key: Key,
    pub transition: KeyTransition,
}

impl ReplayedKeyEvent {
    #[must_use]
    pub const fn pressed(key: Key) -> Self {
        Self {
            key,
            transition: KeyTransition::Pressed,
        }
    }

    #[must_use]
    pub const fn released(key: Key) -> Self {
        Self {
            key,
            transition: KeyTransition::Released,
        }
    }

    #[must_use]
    pub fn virtual_key(self) -> u16 {
        virtual_key(self.key)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseEvent {
    RightButtonPressed,
    RightButtonReleased,
    Wheel(i16),
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputAction {
    Switch(i32),
    Navigate(i32),
    ActivateSelected,
    SelectFirst,
    SelectLast,
    DismissOverlay,
    CloseSelected,
    WindowCommand(WindowCommand),
    ActivateVisiblePosition(usize),
    AltReleased,
    RightButtonPressed,
    RightButtonReleased,
    MouseWheel(i32),
    AppendSearchCharacter(char),
    BackspaceSearch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OverlayKeyEvent {
    pub key: Key,
    pub repeated: bool,
    pub shift: bool,
}

impl OverlayKeyEvent {
    #[must_use]
    pub const fn pressed(key: Key) -> Self {
        Self {
            key,
            repeated: false,
            shift: false,
        }
    }
}

#[must_use]
pub const fn overlay_key_action(event: OverlayKeyEvent) -> Option<InputAction> {
    match event.key {
        Key::Tab => Some(InputAction::Switch(if event.shift { -1 } else { 1 })),
        Key::LeftArrow | Key::UpArrow => Some(InputAction::Navigate(-1)),
        Key::RightArrow | Key::DownArrow => Some(InputAction::Navigate(1)),
        Key::Home => Some(InputAction::SelectFirst),
        Key::End => Some(InputAction::SelectLast),
        Key::Enter if !event.repeated => Some(InputAction::ActivateSelected),
        Key::Escape if !event.repeated => Some(InputAction::DismissOverlay),
        Key::F4 if !event.repeated => Some(InputAction::CloseSelected),
        Key::Function(number) if !event.repeated => {
            match WindowCommand::from_function_key(number) {
                Some(command) => Some(InputAction::WindowCommand(command)),
                None => None,
            }
        }
        Key::Digit(position) | Key::NumpadDigit(position)
            if !event.repeated && position >= 1 && position <= 9 =>
        {
            Some(InputAction::ActivateVisiblePosition(position as usize))
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowCommand {
    Close,
    Minimize,
    Maximize,
    Restore,
    Terminate,
    Run,
}

impl WindowCommand {
    #[must_use]
    pub const fn from_function_key(number: u8) -> Option<Self> {
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

    #[must_use]
    pub const fn function_key(self) -> u8 {
        match self {
            Self::Close => 4,
            Self::Minimize => 5,
            Self::Maximize => 6,
            Self::Restore => 7,
            Self::Terminate => 8,
            Self::Run => 9,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HookOutcome {
    pub suppress: bool,
    actions: [Option<InputAction>; 2],
}

impl HookOutcome {
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

#[derive(Debug, Default)]
pub struct HookState {
    shift_keys: u8,
    pressed_owned_keys: [u64; 4],
    suppressed_shift_keys: u8,
    suppressed_owned_key_releases: [u64; 4],
    pending_alt_keys: u8,
    pending_windows_keys: u8,
    replayed_key_events: [Option<ReplayedKeyEvent>; 3],
    overlay_active: bool,
    alt_switch_gesture_active: bool,
    win_switch_gesture_active: bool,
    right_button: RightButtonState,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum RightButtonState {
    #[default]
    Released,
    Pressed,
    WheelGesture,
}

impl HookState {
    pub fn set_overlay_active(&mut self, active: bool) {
        self.overlay_active = active;
    }

    pub fn reset_gestures(&mut self) {
        self.overlay_active = false;
        self.alt_switch_gesture_active = false;
        self.win_switch_gesture_active = false;
        self.right_button = RightButtonState::Released;
    }

    pub fn take_replayed_key_events(&mut self) -> [Option<ReplayedKeyEvent>; 3] {
        core::mem::take(&mut self.replayed_key_events)
    }

    #[must_use]
    pub fn process_key(&mut self, event: KeyEvent, settings: HookSettings) -> HookOutcome {
        if is_ctrl_or_print_screen(event.key) {
            return HookOutcome::default();
        }
        self.update_shift_state(event);

        let pressed = event.transition == KeyTransition::Pressed;
        let released = event.transition == KeyTransition::Released;
        let key_was_down = self.owned_key_is_down(event.key);
        self.update_owned_key_state(event);
        if let Some(outcome) = self.process_alt_key_transition(event, settings) {
            return outcome;
        }
        let pending_alt_keys = self.pending_alt_keys;
        let alt_down = event.modifiers.alt || pending_alt_keys != 0 || is_alt(event.key);
        let alt_tab = settings.replace_alt_tab && pressed && event.key == Key::Tab && alt_down;
        if let Some(outcome) = self.process_pending_alt_shortcut(event, alt_tab) {
            return outcome;
        }
        if let Some(outcome) = self.process_windows_key_transition(event, settings) {
            return outcome;
        }
        // A suppressed Windows-key press never reaches the system async state, so the hook's
        // observed pending press is authoritative when the following Tab callback arrives.
        let pending_windows_keys = self.pending_windows_keys;
        let windows_tab = settings.replace_win_tab
            && pressed
            && event.key == Key::Tab
            && (pending_windows_keys != 0
                || (self.win_switch_gesture_active && windows_modifier_mask(event.modifiers) != 0));

        if let Some(outcome) = self.process_pending_windows_shortcut(event, windows_tab) {
            return outcome;
        }

        if let Some(outcome) = self.process_escape(event) {
            return outcome;
        }

        if let Some(outcome) = self.process_switch_start(
            event.key,
            alt_tab,
            windows_tab,
            key_was_down,
            pending_alt_keys,
            pending_windows_keys,
        ) {
            return outcome;
        }

        if let Some(outcome) = self.process_alt_gesture_release(event) {
            return outcome;
        }

        if released && self.take_suppressed_owned_key_release(event.key) {
            return HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            };
        }

        if let Some(outcome) = self.process_f4(event) {
            return outcome;
        }

        if let Some(outcome) = self.process_function_key(event) {
            return outcome;
        }

        if let Some(outcome) = self.process_enter(event) {
            return outcome;
        }

        if let Some(outcome) = self.process_boundary_key(event) {
            return outcome;
        }

        if let Some(outcome) = self.process_arrow(event) {
            return outcome;
        }

        if self.should_suppress_shift(event) {
            return HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            };
        }

        if let Some(outcome) = self.process_number_shortcut(event, key_was_down, alt_down) {
            return outcome;
        }

        if let Some(outcome) = self.process_search_key(event, settings.search_active) {
            return outcome;
        }

        if pressed && self.win_switch_gesture_active {
            self.suppress_owned_key_release(event.key);
            return HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            };
        }

        HookOutcome::default()
    }

    fn process_number_shortcut(
        &mut self,
        event: KeyEvent,
        key_was_down: bool,
        alt_down: bool,
    ) -> Option<HookOutcome> {
        if event.transition != KeyTransition::Pressed {
            return None;
        }
        let action @ InputAction::ActivateVisiblePosition(_) =
            overlay_key_action(OverlayKeyEvent {
                key: event.key,
                repeated: key_was_down,
                shift: self.shift_down(),
            })?
        else {
            return None;
        };
        if self.alt_switch_gesture_active && alt_down {
            self.alt_switch_gesture_active = false;
            self.win_switch_gesture_active = false;
            self.suppress_owned_key_release(event.key);
            return Some(HookOutcome::one(true, action));
        }
        if self.win_switch_gesture_active {
            self.suppress_owned_key_release(event.key);
            return Some(HookOutcome::one(true, action));
        }
        None
    }

    #[must_use]
    pub fn process_mouse(&mut self, event: MouseEvent, settings: HookSettings) -> HookOutcome {
        match event {
            MouseEvent::RightButtonPressed if settings.right_button_wheel_switching => {
                self.right_button = RightButtonState::Pressed;
                HookOutcome::default()
            }
            MouseEvent::RightButtonReleased if self.right_button != RightButtonState::Released => {
                let suppress = self.right_button == RightButtonState::WheelGesture;
                self.right_button = RightButtonState::Released;
                if suppress {
                    HookOutcome::one(true, InputAction::RightButtonReleased)
                } else {
                    HookOutcome::default()
                }
            }
            MouseEvent::Wheel(delta)
                if self.right_button != RightButtonState::Released
                    && settings.right_button_wheel_switching =>
            {
                let wheel = InputAction::MouseWheel(i32::from(delta.signum()));
                if self.right_button == RightButtonState::WheelGesture {
                    HookOutcome::one(true, wheel)
                } else {
                    self.right_button = RightButtonState::WheelGesture;
                    HookOutcome::two(true, InputAction::RightButtonPressed, wheel)
                }
            }
            _ => HookOutcome::default(),
        }
    }

    fn process_windows_key_transition(
        &mut self,
        event: KeyEvent,
        settings: HookSettings,
    ) -> Option<HookOutcome> {
        let mask = windows_key_mask(event.key)?;
        if event.transition == KeyTransition::Released && self.pending_windows_keys & mask != 0 {
            self.pending_windows_keys &= !mask;
            self.replayed_key_events = [
                Some(ReplayedKeyEvent::pressed(event.key)),
                Some(ReplayedKeyEvent::released(event.key)),
                None,
            ];
            return Some(HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            });
        }
        if event.transition != KeyTransition::Pressed
            || !settings.replace_win_tab
            || self.alt_switch_gesture_active
        {
            return None;
        }
        if self.win_switch_gesture_active {
            self.suppress_owned_key_release(event.key);
        } else if !self.owned_key_release_pending(event.key) {
            self.pending_windows_keys |= mask;
        }
        Some(HookOutcome {
            suppress: true,
            ..HookOutcome::default()
        })
    }

    fn process_alt_key_transition(
        &mut self,
        event: KeyEvent,
        settings: HookSettings,
    ) -> Option<HookOutcome> {
        let mask = alt_key_mask(event.key)?;
        if event.transition == KeyTransition::Released {
            if self.alt_switch_gesture_active {
                return None;
            }
            if self.pending_alt_keys & mask != 0 {
                self.pending_alt_keys &= !mask;
                self.replayed_key_events = [
                    Some(ReplayedKeyEvent::pressed(event.key)),
                    Some(ReplayedKeyEvent::released(event.key)),
                    None,
                ];
                return Some(HookOutcome {
                    suppress: true,
                    ..HookOutcome::default()
                });
            }
            return None;
        }
        if !settings.replace_alt_tab
            || self.win_switch_gesture_active
            || self.pending_windows_keys != 0
        {
            return None;
        }
        if self.alt_switch_gesture_active {
            self.suppress_owned_key_release(event.key);
            return Some(HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            });
        }
        if !self.owned_key_release_pending(event.key) {
            self.pending_alt_keys |= mask;
        }
        Some(HookOutcome {
            suppress: true,
            ..HookOutcome::default()
        })
    }

    fn process_switch_start(
        &mut self,
        key: Key,
        alt_tab: bool,
        windows_tab: bool,
        key_was_down: bool,
        pending_alt_keys: u8,
        pending_windows_keys: u8,
    ) -> Option<HookOutcome> {
        if !alt_tab && !windows_tab {
            return None;
        }
        self.alt_switch_gesture_active = alt_tab;
        self.win_switch_gesture_active = windows_tab;
        if alt_tab {
            for alt_key in alt_keys(pending_alt_keys) {
                self.suppress_owned_key_release(alt_key);
            }
            self.pending_alt_keys = 0;
            if !key_was_down {
                self.suppress_owned_key_release(key);
            }
        }
        if windows_tab {
            for windows_key in windows_keys(pending_windows_keys) {
                self.suppress_owned_key_release(windows_key);
            }
            self.pending_windows_keys &= !pending_windows_keys;
            self.suppress_owned_key_release(key);
        }
        let action = overlay_key_action(OverlayKeyEvent {
            key,
            repeated: key_was_down,
            shift: self.shift_down(),
        })?;
        Some(HookOutcome::one(true, action))
    }

    fn process_alt_gesture_release(&mut self, event: KeyEvent) -> Option<HookOutcome> {
        if event.transition != KeyTransition::Released
            || !is_alt(event.key)
            || !self.alt_switch_gesture_active
        {
            return None;
        }
        let owned_release = self.take_suppressed_owned_key_release(event.key);
        if self.any_alt_key_down() {
            return Some(HookOutcome {
                suppress: owned_release,
                ..HookOutcome::default()
            });
        }
        self.alt_switch_gesture_active = false;
        Some(HookOutcome::one(owned_release, InputAction::AltReleased))
    }

    fn process_pending_alt_shortcut(
        &mut self,
        event: KeyEvent,
        alt_tab: bool,
    ) -> Option<HookOutcome> {
        if event.transition != KeyTransition::Pressed || self.pending_alt_keys == 0 || alt_tab {
            return None;
        }

        let mut replayed_key_events = [None; 3];
        let mut next_event = 0;
        for alt_key in alt_keys(self.pending_alt_keys) {
            if let Some(slot) = replayed_key_events.get_mut(next_event) {
                *slot = Some(ReplayedKeyEvent::pressed(alt_key));
                next_event += 1;
            }
        }
        let suppress = if let Some(slot) = replayed_key_events.get_mut(next_event) {
            *slot = Some(ReplayedKeyEvent::pressed(event.key));
            true
        } else {
            false
        };
        self.pending_alt_keys = 0;
        self.replayed_key_events = replayed_key_events;
        Some(HookOutcome {
            suppress,
            ..HookOutcome::default()
        })
    }

    fn process_pending_windows_shortcut(
        &mut self,
        event: KeyEvent,
        windows_tab: bool,
    ) -> Option<HookOutcome> {
        if event.transition != KeyTransition::Pressed
            || self.pending_windows_keys == 0
            || windows_tab
        {
            return None;
        }

        let mut replayed_key_events = [None; 3];
        let mut next_event = 0;
        for windows_key in windows_keys(self.pending_windows_keys) {
            if let Some(slot) = replayed_key_events.get_mut(next_event) {
                *slot = Some(ReplayedKeyEvent::pressed(windows_key));
                next_event += 1;
            }
        }
        if let Some(slot) = replayed_key_events.get_mut(next_event) {
            *slot = Some(ReplayedKeyEvent::pressed(event.key));
        }
        self.pending_windows_keys = 0;
        self.replayed_key_events = replayed_key_events;
        Some(HookOutcome {
            suppress: true,
            ..HookOutcome::default()
        })
    }

    fn update_shift_state(&mut self, event: KeyEvent) {
        let Some(mask) = shift_mask(event.key) else {
            return;
        };
        if event.transition == KeyTransition::Pressed {
            self.shift_keys |= mask;
        } else {
            self.shift_keys &= !mask;
        }
    }

    fn should_suppress_shift(&mut self, event: KeyEvent) -> bool {
        let Some(mask) = shift_mask(event.key) else {
            return false;
        };
        if event.transition == KeyTransition::Pressed {
            if self.alt_switch_gesture_active {
                self.suppressed_shift_keys |= mask;
                return true;
            }
            return false;
        }

        let suppressed = self.suppressed_shift_keys & mask != 0;
        self.suppressed_shift_keys &= !mask;
        suppressed
    }

    fn process_arrow(&mut self, event: KeyEvent) -> Option<HookOutcome> {
        let action = overlay_key_action(OverlayKeyEvent {
            key: event.key,
            repeated: false,
            shift: self.shift_down(),
        })?;
        if !matches!(action, InputAction::Navigate(_)) {
            return None;
        }
        if event.transition == KeyTransition::Pressed && self.switch_gesture_active() {
            self.suppress_owned_key_release(event.key);
            return Some(HookOutcome::one(true, action));
        }
        None
    }

    fn process_enter(&mut self, event: KeyEvent) -> Option<HookOutcome> {
        if event.key != Key::Enter || event.transition != KeyTransition::Pressed {
            return None;
        }
        if self.owned_key_release_pending(event.key) {
            return Some(HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            });
        }
        if !self.switch_gesture_active() {
            return None;
        }

        self.alt_switch_gesture_active = false;
        self.win_switch_gesture_active = false;
        self.suppress_owned_key_release(event.key);
        let action = overlay_key_action(OverlayKeyEvent::pressed(event.key))?;
        Some(HookOutcome::one(true, action))
    }

    fn process_f4(&mut self, event: KeyEvent) -> Option<HookOutcome> {
        if event.key != Key::F4 || event.transition != KeyTransition::Pressed {
            return None;
        }
        if self.owned_key_release_pending(event.key) {
            return Some(HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            });
        }
        if !self.switch_gesture_active() {
            return None;
        }

        self.suppress_owned_key_release(event.key);
        let action = overlay_key_action(OverlayKeyEvent::pressed(event.key))?;
        Some(HookOutcome::one(true, action))
    }

    fn process_function_key(&mut self, event: KeyEvent) -> Option<HookOutcome> {
        let Key::Function(number @ 5..=9) = event.key else {
            return None;
        };
        if event.transition != KeyTransition::Pressed {
            return None;
        }
        if self.owned_key_release_pending(event.key) {
            return Some(HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            });
        }
        if !self.switch_gesture_active() {
            return None;
        }
        self.suppress_owned_key_release(event.key);
        let action = overlay_key_action(OverlayKeyEvent::pressed(Key::Function(number)))?;
        Some(HookOutcome::one(true, action))
    }

    fn process_escape(&mut self, event: KeyEvent) -> Option<HookOutcome> {
        if event.key != Key::Escape {
            return None;
        }
        if event.transition == KeyTransition::Released {
            return self
                .take_suppressed_owned_key_release(event.key)
                .then_some(HookOutcome {
                    suppress: true,
                    ..HookOutcome::default()
                });
        }
        if self.owned_key_release_pending(event.key) {
            return Some(HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            });
        }
        if !self.overlay_active {
            return None;
        }

        self.alt_switch_gesture_active = false;
        self.win_switch_gesture_active = false;
        self.suppress_owned_key_release(event.key);
        let action = overlay_key_action(OverlayKeyEvent::pressed(event.key))?;
        Some(HookOutcome::one(true, action))
    }

    fn process_boundary_key(&mut self, event: KeyEvent) -> Option<HookOutcome> {
        let action = overlay_key_action(OverlayKeyEvent::pressed(event.key))?;
        if !matches!(action, InputAction::SelectFirst | InputAction::SelectLast) {
            return None;
        }
        if event.transition == KeyTransition::Pressed && self.switch_gesture_active() {
            self.suppress_owned_key_release(event.key);
            return Some(HookOutcome::one(true, action));
        }
        None
    }

    fn process_search_key(&mut self, event: KeyEvent, search_active: bool) -> Option<HookOutcome> {
        let _slot = owned_key_slot(event.key)?;
        if event.transition == KeyTransition::Released {
            return None;
        }
        if !search_active || event.modifiers.left_windows || event.modifiers.right_windows {
            return None;
        }

        let action = if event.key == Key::Backspace {
            InputAction::BackspaceSearch
        } else {
            InputAction::AppendSearchCharacter(event.text.filter(|value| !value.is_control())?)
        };
        self.alt_switch_gesture_active = false;
        self.win_switch_gesture_active = false;
        self.suppress_owned_key_release(event.key);
        Some(HookOutcome::one(true, action))
    }

    fn suppress_owned_key_release(&mut self, key: Key) {
        let Some((word_index, mask)) = owned_key_slot(key) else {
            return;
        };
        if let Some(word) = self.suppressed_owned_key_releases.get_mut(word_index) {
            *word |= mask;
        }
    }

    fn update_owned_key_state(&mut self, event: KeyEvent) {
        let Some((word_index, mask)) = owned_key_slot(event.key) else {
            return;
        };
        let Some(word) = self.pressed_owned_keys.get_mut(word_index) else {
            return;
        };
        if event.transition == KeyTransition::Pressed {
            *word |= mask;
        } else {
            *word &= !mask;
        }
    }

    fn owned_key_is_down(&self, key: Key) -> bool {
        let Some((word_index, mask)) = owned_key_slot(key) else {
            return false;
        };
        self.pressed_owned_keys
            .get(word_index)
            .is_some_and(|word| word & mask != 0)
    }

    fn any_alt_key_down(&self) -> bool {
        [Key::Alt, Key::LeftAlt, Key::RightAlt]
            .into_iter()
            .any(|key| self.owned_key_is_down(key))
    }

    fn take_suppressed_owned_key_release(&mut self, key: Key) -> bool {
        let Some((word_index, mask)) = owned_key_slot(key) else {
            return false;
        };
        let Some(word) = self.suppressed_owned_key_releases.get_mut(word_index) else {
            return false;
        };
        let suppressed = *word & mask != 0;
        *word &= !mask;
        suppressed
    }

    fn owned_key_release_pending(&self, key: Key) -> bool {
        let Some((word_index, mask)) = owned_key_slot(key) else {
            return false;
        };
        self.suppressed_owned_key_releases
            .get(word_index)
            .is_some_and(|word| word & mask != 0)
    }

    const fn shift_down(&self) -> bool {
        self.shift_keys != 0
    }

    const fn switch_gesture_active(&self) -> bool {
        self.alt_switch_gesture_active || self.win_switch_gesture_active
    }
}

const fn is_alt(key: Key) -> bool {
    matches!(key, Key::Alt | Key::LeftAlt | Key::RightAlt)
}

const fn alt_key_mask(key: Key) -> Option<u8> {
    match key {
        Key::LeftAlt => Some(1),
        Key::RightAlt => Some(2),
        Key::Alt => Some(4),
        _ => None,
    }
}

fn alt_keys(mask: u8) -> impl Iterator<Item = Key> {
    [(1, Key::LeftAlt), (2, Key::RightAlt), (4, Key::Alt)]
        .into_iter()
        .filter_map(move |(bit, key)| (mask & bit != 0).then_some(key))
}

const fn is_ctrl_or_print_screen(key: Key) -> bool {
    matches!(
        key,
        Key::Control | Key::LeftControl | Key::RightControl | Key::PrintScreen
    )
}

const fn shift_mask(key: Key) -> Option<u8> {
    match key {
        Key::LeftShift => Some(1),
        Key::RightShift => Some(2),
        _ => None,
    }
}

fn virtual_key(key: Key) -> u16 {
    match key {
        Key::Tab => 0x09,
        Key::Enter => 0x0D,
        Key::Home => 0x24,
        Key::End => 0x23,
        Key::Escape => 0x1B,
        Key::F4 => 0x73,
        Key::Function(number) => 0x6F + u16::from(number),
        Key::Alt => 0x12,
        Key::LeftAlt => 0xA4,
        Key::RightAlt => 0xA5,
        Key::LeftWindows => 0x5B,
        Key::RightWindows => 0x5C,
        Key::Control => 0x11,
        Key::LeftControl => 0xA2,
        Key::RightControl => 0xA3,
        Key::LeftShift => 0xA0,
        Key::RightShift => 0xA1,
        Key::PrintScreen => 0x2C,
        Key::Backspace => 0x08,
        Key::LeftArrow => 0x25,
        Key::UpArrow => 0x26,
        Key::RightArrow => 0x27,
        Key::DownArrow => 0x28,
        Key::Digit(value) => 0x30 + u16::from(value),
        Key::NumpadDigit(value) => 0x60 + u16::from(value),
        Key::Other(value) => value,
    }
}

const fn windows_key_mask(key: Key) -> Option<u8> {
    match key {
        Key::LeftWindows => Some(1),
        Key::RightWindows => Some(2),
        _ => None,
    }
}

fn windows_modifier_mask(modifiers: Modifiers) -> u8 {
    u8::from(modifiers.left_windows) | (u8::from(modifiers.right_windows) << 1)
}

fn windows_keys(mask: u8) -> impl Iterator<Item = Key> {
    [(1, Key::LeftWindows), (2, Key::RightWindows)]
        .into_iter()
        .filter_map(move |(key_mask, key)| (mask & key_mask != 0).then_some(key))
}

fn owned_key_slot(key: Key) -> Option<(usize, u64)> {
    let virtual_key = usize::from(virtual_key(key));
    if virtual_key >= 256 {
        return None;
    }
    Some((virtual_key / 64, 1_u64 << (virtual_key % 64)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALT: Modifiers = Modifiers {
        alt: true,
        left_windows: false,
        right_windows: false,
    };

    #[test]
    fn plain_right_click_passes_through() {
        let mut state = HookState::default();
        let settings = HookSettings::default();

        assert_eq!(
            state.process_mouse(MouseEvent::RightButtonPressed, settings),
            HookOutcome::default()
        );
        assert_eq!(
            state.process_mouse(MouseEvent::RightButtonReleased, settings),
            HookOutcome::default()
        );
    }

    #[test]
    fn right_button_wheel_starts_then_advances_switcher() {
        let mut state = HookState::default();
        let settings = HookSettings::default();
        assert_eq!(
            state.process_mouse(MouseEvent::RightButtonPressed, settings),
            HookOutcome::default()
        );

        assert_eq!(
            state.process_mouse(MouseEvent::Wheel(120), settings),
            HookOutcome {
                suppress: true,
                actions: [
                    Some(InputAction::RightButtonPressed),
                    Some(InputAction::MouseWheel(1)),
                ],
            }
        );
        assert_eq!(
            state.process_mouse(MouseEvent::Wheel(-120), settings),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::MouseWheel(-1)), None],
            }
        );
        assert_eq!(
            state.process_mouse(MouseEvent::RightButtonReleased, settings),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::RightButtonReleased), None],
            }
        );
    }

    #[test]
    fn disabled_right_button_wheel_switching_never_arms_release_activation() {
        let mut state = HookState::default();
        let settings = HookSettings {
            right_button_wheel_switching: false,
            ..HookSettings::default()
        };

        for event in [
            MouseEvent::RightButtonPressed,
            MouseEvent::Wheel(120),
            MouseEvent::RightButtonReleased,
        ] {
            assert_eq!(state.process_mouse(event, settings), HookOutcome::default());
        }
    }

    #[test]
    fn plain_alt_release_passes_through_without_action() {
        let mut state = HookState::default();
        assert_eq!(
            state.process_key(
                KeyEvent::released(Key::Alt, Modifiers::default()),
                HookSettings::default()
            ),
            HookOutcome::default()
        );
    }

    #[test]
    fn plain_alt_press_is_replayed_as_a_complete_sequence() {
        for alt_key in [Key::LeftAlt, Key::RightAlt] {
            let mut state = HookState::default();
            let settings = HookSettings::default();

            assert!(
                state
                    .process_key(KeyEvent::pressed(alt_key, ALT), settings)
                    .suppress
            );
            assert!(
                state
                    .process_key(KeyEvent::released(alt_key, Modifiers::default()), settings)
                    .suppress
            );
            assert_eq!(
                state.take_replayed_key_events(),
                [
                    Some(ReplayedKeyEvent::pressed(alt_key)),
                    Some(ReplayedKeyEvent::released(alt_key)),
                    None,
                ]
            );
        }
    }

    #[test]
    fn non_switch_alt_shortcut_is_replayed_without_changing_its_key_order() {
        let mut state = HookState::default();
        let settings = HookSettings::default();

        assert!(
            state
                .process_key(KeyEvent::pressed(Key::LeftAlt, ALT), settings)
                .suppress
        );
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::F4, ALT), settings)
                .suppress
        );
        assert_eq!(
            state.take_replayed_key_events(),
            [
                Some(ReplayedKeyEvent::pressed(Key::LeftAlt)),
                Some(ReplayedKeyEvent::pressed(Key::F4)),
                None,
            ]
        );
        assert_eq!(
            state.process_key(KeyEvent::released(Key::F4, ALT), settings),
            HookOutcome::default()
        );
        assert_eq!(
            state.process_key(
                KeyEvent::released(Key::LeftAlt, Modifiers::default()),
                settings
            ),
            HookOutcome::default()
        );
    }

    #[test]
    fn disabled_alt_tab_replacement_leaves_alt_input_untouched() {
        let mut state = HookState::default();
        let settings = HookSettings {
            replace_alt_tab: false,
            ..HookSettings::default()
        };

        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::LeftAlt, ALT), settings),
            HookOutcome::default()
        );
        assert_eq!(
            state.process_key(
                KeyEvent::released(Key::LeftAlt, Modifiers::default()),
                settings
            ),
            HookOutcome::default()
        );
        assert_eq!(state.take_replayed_key_events(), [None; 3]);
    }

    #[test]
    fn alt_release_after_alt_tab_requests_activation_and_passes_through_when_not_owned() {
        let mut state = HookState::default();
        let settings = HookSettings::default();

        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::Tab, ALT), settings),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::Switch(1)), None],
            }
        );
        assert_eq!(
            state.process_key(KeyEvent::released(Key::Tab, ALT), settings),
            HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            }
        );
        assert_eq!(
            state.process_key(KeyEvent::released(Key::Alt, Modifiers::default()), settings),
            HookOutcome {
                suppress: false,
                actions: [Some(InputAction::AltReleased), None],
            }
        );
    }

    #[test]
    fn alt_tab_owns_the_complete_physical_alt_sequence() {
        for alt_key in [Key::LeftAlt, Key::RightAlt] {
            let mut state = HookState::default();
            let settings = HookSettings::default();

            assert_eq!(
                state.process_key(KeyEvent::pressed(alt_key, ALT), settings),
                HookOutcome {
                    suppress: true,
                    ..HookOutcome::default()
                }
            );
            assert_eq!(
                state.process_key(KeyEvent::pressed(Key::Tab, ALT), settings),
                HookOutcome {
                    suppress: true,
                    actions: [Some(InputAction::Switch(1)), None],
                }
            );
            assert_eq!(
                state.process_key(KeyEvent::released(Key::Tab, ALT), settings),
                HookOutcome {
                    suppress: true,
                    ..HookOutcome::default()
                }
            );
            assert_eq!(
                state.process_key(KeyEvent::released(alt_key, Modifiers::default()), settings),
                HookOutcome {
                    suppress: true,
                    actions: [Some(InputAction::AltReleased), None],
                }
            );
            assert_eq!(state.take_replayed_key_events(), [None; 3]);
        }
    }

    #[test]
    fn preheld_tab_release_is_not_owned_when_alt_tab_starts_from_repeat() {
        let mut state = HookState::default();
        let settings = HookSettings::default();

        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::Tab, Modifiers::default()), settings),
            HookOutcome::default()
        );
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                .suppress
        );
        assert_eq!(
            state.process_key(KeyEvent::released(Key::Tab, ALT), settings),
            HookOutcome::default()
        );
    }

    #[test]
    fn win_tab_does_not_arm_alt_release() {
        let mut state = HookState::default();
        let settings = HookSettings::default();
        let windows = Modifiers {
            left_windows: true,
            ..Modifiers::default()
        };

        assert!(
            state
                .process_key(KeyEvent::pressed(Key::LeftWindows, windows), settings)
                .suppress
        );
        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::Tab, windows), settings),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::Switch(1)), None],
            }
        );
        assert!(
            state
                .process_key(
                    KeyEvent::released(Key::LeftWindows, Modifiers::default()),
                    settings
                )
                .suppress
        );
        assert_eq!(
            state.process_key(KeyEvent::released(Key::Alt, Modifiers::default()), settings),
            HookOutcome::default()
        );
    }

    #[test]
    fn win_tab_starts_before_the_async_windows_modifier_state_updates() {
        for windows_key in [Key::LeftWindows, Key::RightWindows] {
            let mut state = HookState::default();
            let settings = HookSettings::default();

            assert!(
                state
                    .process_key(
                        KeyEvent::pressed(windows_key, Modifiers::default()),
                        settings
                    )
                    .suppress
            );
            assert_eq!(
                state.process_key(KeyEvent::pressed(Key::Tab, Modifiers::default()), settings),
                HookOutcome {
                    suppress: true,
                    actions: [Some(InputAction::Switch(1)), None],
                }
            );
            assert_eq!(state.take_replayed_key_events(), [None; 3]);
        }
    }

    #[test]
    fn win_tab_owns_the_complete_physical_windows_key_sequence() {
        for (windows_key, windows) in [
            (
                Key::LeftWindows,
                Modifiers {
                    left_windows: true,
                    ..Modifiers::default()
                },
            ),
            (
                Key::RightWindows,
                Modifiers {
                    right_windows: true,
                    ..Modifiers::default()
                },
            ),
        ] {
            let mut state = HookState::default();
            let settings = HookSettings::default();

            assert!(
                state
                    .process_key(KeyEvent::pressed(windows_key, windows), settings)
                    .suppress
            );
            assert_eq!(
                state.process_key(KeyEvent::pressed(Key::Tab, windows), settings),
                HookOutcome {
                    suppress: true,
                    actions: [Some(InputAction::Switch(1)), None],
                }
            );
            assert!(
                state
                    .process_key(KeyEvent::released(Key::Tab, windows), settings)
                    .suppress
            );
            assert!(
                state
                    .process_key(
                        KeyEvent::released(windows_key, Modifiers::default()),
                        settings
                    )
                    .suppress
            );
        }
    }

    #[test]
    fn win_tab_repeats_remain_owned_without_replaying_windows_input() {
        for (windows_key, windows) in [
            (
                Key::LeftWindows,
                Modifiers {
                    left_windows: true,
                    ..Modifiers::default()
                },
            ),
            (
                Key::RightWindows,
                Modifiers {
                    right_windows: true,
                    ..Modifiers::default()
                },
            ),
        ] {
            let mut state = HookState::default();
            let settings = HookSettings::default();

            for _ in 0..2 {
                assert!(
                    state
                        .process_key(KeyEvent::pressed(windows_key, windows), settings)
                        .suppress
                );
            }
            for _ in 0..2 {
                assert_eq!(
                    state.process_key(KeyEvent::pressed(Key::Tab, windows), settings),
                    HookOutcome {
                        suppress: true,
                        actions: [Some(InputAction::Switch(1)), None],
                    }
                );
            }
            assert!(
                state
                    .process_key(KeyEvent::pressed(windows_key, windows), settings)
                    .suppress
            );
            assert!(
                state
                    .process_key(KeyEvent::released(Key::Tab, windows), settings)
                    .suppress
            );
            assert!(
                state
                    .process_key(
                        KeyEvent::released(windows_key, Modifiers::default()),
                        settings
                    )
                    .suppress
            );
            assert_eq!(state.take_replayed_key_events(), [None; 3]);
        }
    }

    #[test]
    fn a_windows_key_repress_during_the_owned_gesture_does_not_open_start() {
        for (windows_key, windows) in [
            (
                Key::LeftWindows,
                Modifiers {
                    left_windows: true,
                    ..Modifiers::default()
                },
            ),
            (
                Key::RightWindows,
                Modifiers {
                    right_windows: true,
                    ..Modifiers::default()
                },
            ),
        ] {
            let mut state = HookState::default();
            let settings = HookSettings::default();
            assert!(
                state
                    .process_key(KeyEvent::pressed(windows_key, windows), settings)
                    .suppress
            );
            assert!(
                state
                    .process_key(KeyEvent::pressed(Key::Tab, windows), settings)
                    .suppress
            );
            assert!(
                state
                    .process_key(KeyEvent::released(Key::Tab, windows), settings)
                    .suppress
            );
            assert!(
                state
                    .process_key(
                        KeyEvent::released(windows_key, Modifiers::default()),
                        settings
                    )
                    .suppress
            );

            assert!(
                state
                    .process_key(KeyEvent::pressed(windows_key, windows), settings)
                    .suppress
            );
            assert!(
                state
                    .process_key(
                        KeyEvent::released(windows_key, Modifiers::default()),
                        settings
                    )
                    .suppress
            );
            assert_eq!(state.take_replayed_key_events(), [None; 3]);
        }
    }

    #[test]
    fn win_tab_release_pairs_remain_owned_in_either_physical_order() {
        for (windows_key, windows) in [
            (
                Key::LeftWindows,
                Modifiers {
                    left_windows: true,
                    ..Modifiers::default()
                },
            ),
            (
                Key::RightWindows,
                Modifiers {
                    right_windows: true,
                    ..Modifiers::default()
                },
            ),
        ] {
            for windows_released_first in [false, true] {
                let mut state = HookState::default();
                let settings = HookSettings::default();
                assert!(
                    state
                        .process_key(KeyEvent::pressed(windows_key, windows), settings)
                        .suppress
                );
                assert!(
                    state
                        .process_key(KeyEvent::pressed(Key::Tab, windows), settings)
                        .suppress
                );

                let windows_released = KeyEvent::released(windows_key, Modifiers::default());
                let tab_released = KeyEvent::released(Key::Tab, Modifiers::default());
                let releases = if windows_released_first {
                    [windows_released, tab_released]
                } else {
                    [tab_released, windows_released]
                };
                for release in releases {
                    assert!(state.process_key(release, settings).suppress);
                }
                assert_eq!(state.take_replayed_key_events(), [None; 3]);
            }
        }
    }

    #[test]
    fn a_preheld_windows_key_is_not_claimed_as_a_new_win_tab_gesture() {
        for (windows_key, windows) in [
            (
                Key::LeftWindows,
                Modifiers {
                    left_windows: true,
                    ..Modifiers::default()
                },
            ),
            (
                Key::RightWindows,
                Modifiers {
                    right_windows: true,
                    ..Modifiers::default()
                },
            ),
        ] {
            let mut state = HookState::default();
            let disabled = HookSettings {
                replace_win_tab: false,
                ..HookSettings::default()
            };
            assert_eq!(
                state.process_key(KeyEvent::pressed(windows_key, windows), disabled),
                HookOutcome::default()
            );

            let settings = HookSettings::default();
            assert_eq!(
                state.process_key(KeyEvent::pressed(Key::Tab, windows), settings),
                HookOutcome::default()
            );
            assert_eq!(
                state.process_key(KeyEvent::released(Key::Tab, windows), settings),
                HookOutcome::default()
            );
            assert_eq!(
                state.process_key(
                    KeyEvent::released(windows_key, Modifiers::default()),
                    settings
                ),
                HookOutcome::default()
            );
            assert_eq!(state.take_replayed_key_events(), [None; 3]);
        }
    }

    #[test]
    fn windows_key_alone_is_replayed_as_a_complete_pair() {
        for windows_key in [Key::LeftWindows, Key::RightWindows] {
            let mut state = HookState::default();
            let settings = HookSettings::default();

            assert!(
                state
                    .process_key(
                        KeyEvent::pressed(windows_key, Modifiers::default()),
                        settings
                    )
                    .suppress
            );
            assert!(
                state
                    .process_key(
                        KeyEvent::released(windows_key, Modifiers::default()),
                        settings
                    )
                    .suppress
            );
            assert_eq!(
                state.take_replayed_key_events(),
                [
                    Some(ReplayedKeyEvent::pressed(windows_key)),
                    Some(ReplayedKeyEvent::released(windows_key)),
                    None,
                ]
            );
        }
    }

    #[test]
    fn unrelated_windows_shortcuts_are_replayed_and_released_to_windows() {
        for (windows_key, windows) in [
            (
                Key::LeftWindows,
                Modifiers {
                    left_windows: true,
                    ..Modifiers::default()
                },
            ),
            (
                Key::RightWindows,
                Modifiers {
                    right_windows: true,
                    ..Modifiers::default()
                },
            ),
        ] {
            for shortcut_key in [Key::Digit(1), Key::Other(u16::from(b'R'))] {
                let mut state = HookState::default();
                let settings = HookSettings {
                    search_active: true,
                    ..HookSettings::default()
                };

                assert!(
                    state
                        .process_key(KeyEvent::pressed(windows_key, windows), settings)
                        .suppress
                );
                let shortcut = state.process_key(
                    KeyEvent::pressed(shortcut_key, windows).with_text('r'),
                    settings,
                );
                assert!(shortcut.suppress);
                assert_eq!(shortcut.actions().next(), None);
                assert_eq!(
                    state.take_replayed_key_events(),
                    [
                        Some(ReplayedKeyEvent::pressed(windows_key)),
                        Some(ReplayedKeyEvent::pressed(shortcut_key)),
                        None,
                    ]
                );
                assert_eq!(
                    state.process_key(KeyEvent::released(shortcut_key, windows), settings),
                    HookOutcome::default()
                );
                assert_eq!(
                    state.process_key(
                        KeyEvent::released(windows_key, Modifiers::default()),
                        settings
                    ),
                    HookOutcome::default()
                );
            }
        }
    }

    #[test]
    fn win_tab_owns_armed_gesture_input_instead_of_the_background_window() {
        let mut state = HookState::default();
        let settings = HookSettings::default();
        let windows = Modifiers {
            left_windows: true,
            ..Modifiers::default()
        };

        assert!(
            state
                .process_key(KeyEvent::pressed(Key::LeftWindows, windows), settings)
                .suppress
        );
        let trigger = state.process_key(KeyEvent::pressed(Key::Tab, windows), settings);
        let tab_released = state.process_key(KeyEvent::released(Key::Tab, windows), settings);
        let arrow_pressed = state.process_key(
            KeyEvent::pressed(Key::LeftArrow, Modifiers::default()),
            settings,
        );
        state.reset_gestures();
        let windows_released = state.process_key(
            KeyEvent::released(Key::LeftWindows, Modifiers::default()),
            settings,
        );
        let arrow_released = state.process_key(
            KeyEvent::released(Key::LeftArrow, Modifiers::default()),
            settings,
        );
        let background_key = state.process_key(
            KeyEvent::pressed(Key::Other(0x42), Modifiers::default()),
            settings,
        );

        assert_eq!(
            trigger,
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::Switch(1)), None],
            }
        );
        assert_eq!(
            arrow_pressed,
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::Navigate(-1)), None],
            }
        );
        assert_eq!(
            [
                tab_released.suppress,
                windows_released.suppress,
                arrow_pressed.suppress,
                arrow_released.suppress,
            ],
            [true; 4]
        );
        assert_eq!(tab_released.actions().next(), None);
        assert_eq!(arrow_released.actions().next(), None);
        assert_eq!(background_key, HookOutcome::default());
    }

    #[test]
    fn alt_number_activates_visible_position_and_cancels_alt_release() {
        let mut state = HookState::default();
        let settings = HookSettings {
            search_active: true,
            ..HookSettings::default()
        };
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                .suppress
        );

        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::NumpadDigit(9), ALT), settings),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::ActivateVisiblePosition(9)), None],
            }
        );
        assert_eq!(
            state.process_key(KeyEvent::released(Key::Alt, Modifiers::default()), settings),
            HookOutcome::default()
        );
        assert!(
            state
                .process_key(
                    KeyEvent::released(Key::NumpadDigit(9), Modifiers::default()),
                    settings
                )
                .suppress
        );
    }

    #[test]
    fn alt_number_without_switcher_passes_through() {
        let mut state = HookState::default();
        assert_eq!(
            state.process_key(
                KeyEvent::pressed(Key::Digit(1), ALT),
                HookSettings::default()
            ),
            HookOutcome::default()
        );
    }

    #[test]
    fn either_shift_key_reverses_alt_tab_until_released() {
        for shift in [Key::LeftShift, Key::RightShift] {
            let mut state = HookState::default();
            let settings = HookSettings::default();
            assert_eq!(
                state.process_key(KeyEvent::pressed(shift, Modifiers::default()), settings),
                HookOutcome::default()
            );
            assert_eq!(
                state
                    .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                    .actions()
                    .next(),
                Some(InputAction::Switch(-1))
            );

            assert_eq!(
                state.process_key(KeyEvent::released(shift, Modifiers::default()), settings),
                HookOutcome::default()
            );
            assert_eq!(
                state
                    .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                    .actions()
                    .next(),
                Some(InputAction::Switch(1))
            );
        }
    }

    #[test]
    fn shift_is_suppressed_during_alt_tab_gesture_to_prevent_language_change() {
        for shift in [Key::LeftShift, Key::RightShift] {
            let mut state = HookState::default();
            let settings = HookSettings::default();
            assert!(
                state
                    .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                    .suppress
            );

            assert!(
                state
                    .process_key(KeyEvent::pressed(shift, ALT), settings)
                    .suppress
            );
            assert!(
                state
                    .process_key(KeyEvent::released(shift, ALT), settings)
                    .suppress
            );
        }
    }

    #[test]
    fn search_keys_pass_through_when_search_is_inactive() {
        let mut state = HookState::default();
        let settings = HookSettings::default();

        assert_eq!(
            state.process_key(
                KeyEvent::pressed(Key::Other(u16::from(b'A')), Modifiers::default()).with_text('a'),
                settings
            ),
            HookOutcome::default()
        );
        assert_eq!(
            state.process_key(
                KeyEvent::pressed(Key::Backspace, Modifiers::default()),
                settings
            ),
            HookOutcome::default()
        );
    }

    #[test]
    fn active_search_posts_text_and_suppresses_paired_transitions() {
        let mut state = HookState::default();
        let settings = HookSettings {
            search_active: true,
            ..HookSettings::default()
        };
        let key = Key::Other(u16::from(b'A'));

        assert_eq!(
            state.process_key(
                KeyEvent::pressed(key, Modifiers::default()).with_text('a'),
                settings
            ),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::AppendSearchCharacter('a')), None],
            }
        );
        assert_eq!(
            state.process_key(KeyEvent::released(key, Modifiers::default()), settings),
            HookOutcome {
                suppress: true,
                actions: [None, None],
            }
        );
    }

    #[test]
    fn active_search_backspace_is_handled_without_leaking() {
        let mut state = HookState::default();
        let settings = HookSettings {
            search_active: true,
            ..HookSettings::default()
        };

        assert_eq!(
            state.process_key(
                KeyEvent::pressed(Key::Backspace, Modifiers::default()),
                settings
            ),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::BackspaceSearch), None],
            }
        );
        assert!(
            state
                .process_key(
                    KeyEvent::released(Key::Backspace, Modifiers::default()),
                    settings
                )
                .suppress
        );
    }

    #[test]
    fn active_search_preserves_non_text_shortcuts() {
        let mut state = HookState::default();
        let settings = HookSettings {
            search_active: true,
            ..HookSettings::default()
        };
        let print_screen = Key::PrintScreen;

        assert_eq!(
            state.process_key(
                KeyEvent::pressed(print_screen, Modifiers::default()),
                settings
            ),
            HookOutcome::default()
        );
        assert_eq!(
            state.process_key(
                KeyEvent::released(print_screen, Modifiers::default()),
                settings
            ),
            HookOutcome::default()
        );
    }

    #[test]
    fn active_search_preserves_windows_modified_text() {
        let mut state = HookState::default();
        let settings = HookSettings {
            search_active: true,
            ..HookSettings::default()
        };
        let windows = Modifiers {
            left_windows: true,
            ..Modifiers::default()
        };

        assert_eq!(
            state.process_key(
                KeyEvent::pressed(Key::Other(u16::from(b'R')), windows).with_text('r'),
                settings
            ),
            HookOutcome::default()
        );
    }

    #[test]
    fn ctrl_print_screen_passes_through_while_resident() {
        for control in [Key::Control, Key::LeftControl, Key::RightControl] {
            let mut state = HookState::default();
            let settings = HookSettings::default();

            for event in [
                KeyEvent::pressed(control, Modifiers::default()),
                KeyEvent::pressed(Key::PrintScreen, Modifiers::default()),
                KeyEvent::released(Key::PrintScreen, Modifiers::default()),
                KeyEvent::released(control, Modifiers::default()),
            ] {
                assert_eq!(state.process_key(event, settings), HookOutcome::default());
            }
        }
    }

    #[test]
    fn win_tab_then_ctrl_print_screen_passes_through_without_mutating_owned_releases() {
        for control in [Key::Control, Key::LeftControl, Key::RightControl] {
            for print_screen_released_first in [true, false] {
                let mut state = HookState::default();
                let settings = HookSettings {
                    search_active: true,
                    ..HookSettings::default()
                };
                let windows = Modifiers {
                    left_windows: true,
                    ..Modifiers::default()
                };
                assert!(
                    state
                        .process_key(KeyEvent::pressed(Key::LeftWindows, windows), settings)
                        .suppress
                );
                assert!(
                    state
                        .process_key(KeyEvent::pressed(Key::Tab, windows), settings)
                        .suppress
                );
                state.set_overlay_active(true);

                assert_eq!(
                    state.process_key(KeyEvent::pressed(control, Modifiers::default()), settings),
                    HookOutcome::default()
                );
                assert_eq!(
                    state.process_key(
                        KeyEvent::pressed(Key::PrintScreen, Modifiers::default()),
                        settings
                    ),
                    HookOutcome::default()
                );

                let releases = if print_screen_released_first {
                    [
                        KeyEvent::released(Key::PrintScreen, Modifiers::default()),
                        KeyEvent::released(control, Modifiers::default()),
                    ]
                } else {
                    [
                        KeyEvent::released(control, Modifiers::default()),
                        KeyEvent::released(Key::PrintScreen, Modifiers::default()),
                    ]
                };
                for event in releases {
                    assert_eq!(state.process_key(event, settings), HookOutcome::default());
                }
                assert!(
                    state
                        .process_key(KeyEvent::released(Key::Tab, windows), settings)
                        .suppress
                );
                assert!(
                    state
                        .process_key(
                            KeyEvent::released(Key::LeftWindows, Modifiers::default()),
                            settings
                        )
                        .suppress
                );
                assert_eq!(state.take_replayed_key_events(), [None; 3]);
                assert_eq!(
                    state.process_key(
                        KeyEvent::pressed(Key::Other(u16::from(b'Q')), Modifiers::default())
                            .with_text('q'),
                        settings
                    ),
                    HookOutcome {
                        suppress: true,
                        actions: [Some(InputAction::AppendSearchCharacter('q')), None],
                    }
                );
            }
        }
    }

    #[test]
    fn typing_search_text_cancels_release_to_activate() {
        let mut state = HookState::default();
        let settings = HookSettings {
            search_active: true,
            ..HookSettings::default()
        };
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                .suppress
        );
        assert!(
            state
                .process_key(
                    KeyEvent::pressed(Key::Other(u16::from(b'K')), ALT).with_text('k'),
                    settings
                )
                .suppress
        );

        assert_eq!(
            state.process_key(KeyEvent::released(Key::Alt, Modifiers::default()), settings),
            HookOutcome::default()
        );
    }

    #[test]
    fn arrows_navigate_directionally_and_suppress_paired_transitions() {
        for (key, delta) in [
            (Key::LeftArrow, -1),
            (Key::UpArrow, -1),
            (Key::RightArrow, 1),
            (Key::DownArrow, 1),
        ] {
            let mut state = HookState::default();
            let settings = HookSettings::default();
            assert!(
                state
                    .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                    .suppress
            );

            assert_eq!(
                state.process_key(KeyEvent::pressed(key, ALT), settings),
                HookOutcome {
                    suppress: true,
                    actions: [Some(InputAction::Navigate(delta)), None],
                }
            );
            assert_eq!(
                state.process_key(KeyEvent::released(Key::Alt, Modifiers::default()), settings),
                HookOutcome {
                    suppress: false,
                    actions: [Some(InputAction::AltReleased), None],
                }
            );
            assert_eq!(
                state.process_key(KeyEvent::released(key, Modifiers::default()), settings),
                HookOutcome {
                    suppress: true,
                    ..HookOutcome::default()
                }
            );
        }
    }

    #[test]
    fn enter_outside_a_switch_gesture_passes_through() {
        let mut state = HookState::default();
        let settings = HookSettings::default();

        assert_eq!(
            state.process_key(
                KeyEvent::pressed(Key::Enter, Modifiers::default()),
                settings
            ),
            HookOutcome::default()
        );
        assert_eq!(
            state.process_key(
                KeyEvent::released(Key::Enter, Modifiers::default()),
                settings
            ),
            HookOutcome::default()
        );
    }

    #[test]
    fn enter_activates_selected_once_and_suppresses_the_complete_key_sequence() {
        let mut state = HookState::default();
        let settings = HookSettings::default();
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                .suppress
        );

        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::Enter, ALT), settings),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::ActivateSelected), None],
            }
        );
        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::Enter, ALT), settings),
            HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            }
        );
        assert_eq!(
            state.process_key(KeyEvent::released(Key::Enter, ALT), settings),
            HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            }
        );
        assert_eq!(
            state.process_key(KeyEvent::released(Key::Alt, Modifiers::default()), settings),
            HookOutcome::default()
        );
    }

    #[test]
    fn arrows_pass_through_without_an_active_switch_gesture() {
        let mut state = HookState::default();
        let settings = HookSettings::default();

        for key in [
            Key::LeftArrow,
            Key::UpArrow,
            Key::RightArrow,
            Key::DownArrow,
        ] {
            assert_eq!(
                state.process_key(KeyEvent::pressed(key, Modifiers::default()), settings),
                HookOutcome::default()
            );
            assert_eq!(
                state.process_key(KeyEvent::released(key, Modifiers::default()), settings),
                HookOutcome::default()
            );
        }
    }

    #[test]
    fn home_and_end_are_suppressed_and_dispatched_during_switch_gesture() {
        for (key, action) in [
            (Key::Home, InputAction::SelectFirst),
            (Key::End, InputAction::SelectLast),
        ] {
            let mut state = HookState::default();
            let settings = HookSettings::default();
            assert!(
                state
                    .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                    .suppress
            );

            assert_eq!(
                state.process_key(KeyEvent::pressed(key, ALT), settings),
                HookOutcome {
                    suppress: true,
                    actions: [Some(action), None],
                }
            );
            assert_eq!(
                state.process_key(KeyEvent::released(Key::Alt, Modifiers::default()), settings),
                HookOutcome {
                    suppress: false,
                    actions: [Some(InputAction::AltReleased), None],
                }
            );
            assert_eq!(
                state.process_key(KeyEvent::released(key, Modifiers::default()), settings),
                HookOutcome {
                    suppress: true,
                    ..HookOutcome::default()
                }
            );
        }
    }

    #[test]
    fn home_and_end_pass_through_without_switch_gesture() {
        for key in [Key::Home, Key::End] {
            let mut state = HookState::default();
            let settings = HookSettings::default();

            assert_eq!(
                state.process_key(KeyEvent::pressed(key, ALT), settings),
                HookOutcome::default()
            );
            assert_eq!(
                state.process_key(KeyEvent::released(key, Modifiers::default()), settings),
                HookOutcome::default()
            );
        }
    }

    #[test]
    fn escape_dismisses_active_switcher_without_activation_or_delayed_leak() {
        let mut state = HookState::default();
        let settings = HookSettings::default();
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                .suppress
        );
        state.set_overlay_active(true);

        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::Escape, ALT), settings),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::DismissOverlay), None],
            }
        );
        state.set_overlay_active(false);
        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::Escape, ALT), settings),
            HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            }
        );
        assert_eq!(
            state.process_key(KeyEvent::released(Key::Escape, ALT), settings),
            HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            }
        );
        assert_eq!(
            state.process_key(KeyEvent::released(Key::Alt, Modifiers::default()), settings),
            HookOutcome::default()
        );
    }

    #[test]
    fn escape_passes_through_when_overlay_is_inactive() {
        let mut state = HookState::default();
        let settings = HookSettings::default();

        assert_eq!(
            state.process_key(
                KeyEvent::pressed(Key::Escape, Modifiers::default()),
                settings
            ),
            HookOutcome::default()
        );
        assert_eq!(
            state.process_key(
                KeyEvent::released(Key::Escape, Modifiers::default()),
                settings
            ),
            HookOutcome::default()
        );
    }

    #[test]
    fn escape_dismisses_overlay_without_an_alt_tab_gesture() {
        let mut state = HookState::default();
        let settings = HookSettings::default();
        state.set_overlay_active(true);

        assert_eq!(
            state.process_key(
                KeyEvent::pressed(Key::Escape, Modifiers::default()),
                settings
            ),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::DismissOverlay), None],
            }
        );
        state.set_overlay_active(false);
        assert!(
            state
                .process_key(
                    KeyEvent::released(Key::Escape, Modifiers::default()),
                    settings
                )
                .suppress
        );
    }

    #[test]
    fn f4_closes_once_and_suppresses_repeat_and_key_up_during_switch_gesture() {
        let mut state = HookState::default();
        let settings = HookSettings::default();
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                .suppress
        );

        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::F4, ALT), settings),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::CloseSelected), None],
            }
        );
        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::F4, ALT), settings),
            HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            }
        );
        assert!(
            state
                .process_key(KeyEvent::released(Key::F4, ALT), settings)
                .suppress
        );
    }

    #[test]
    fn alt_f4_outside_switch_gesture_passes_through() {
        let mut state = HookState::default();
        let settings = HookSettings::default();

        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::F4, ALT), settings),
            HookOutcome::default()
        );
        assert_eq!(
            state.process_key(KeyEvent::released(Key::F4, ALT), settings),
            HookOutcome::default()
        );
    }

    #[test]
    fn f4_key_up_remains_suppressed_after_alt_is_released() {
        let mut state = HookState::default();
        let settings = HookSettings::default();
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                .suppress
        );
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::F4, ALT), settings)
                .suppress
        );
        assert_eq!(
            state.process_key(KeyEvent::released(Key::Alt, Modifiers::default()), settings),
            HookOutcome {
                suppress: false,
                actions: [Some(InputAction::AltReleased), None],
            }
        );
        assert!(
            state
                .process_key(KeyEvent::released(Key::F4, Modifiers::default()), settings)
                .suppress
        );
    }

    #[test]
    fn f5_through_f9_execute_exact_commands_and_suppress_paired_transitions() {
        let shortcuts = [
            (Key::Function(5), WindowCommand::Minimize),
            (Key::Function(6), WindowCommand::Maximize),
            (Key::Function(7), WindowCommand::Restore),
            (Key::Function(8), WindowCommand::Terminate),
            (Key::Function(9), WindowCommand::Run),
        ];

        for (key, command) in shortcuts {
            let mut state = HookState::default();
            let settings = HookSettings::default();
            assert!(
                state
                    .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                    .suppress
            );

            assert_eq!(
                state.process_key(KeyEvent::pressed(key, ALT), settings),
                HookOutcome {
                    suppress: true,
                    actions: [Some(InputAction::WindowCommand(command)), None],
                }
            );
            assert_eq!(
                state.process_key(KeyEvent::pressed(key, ALT), settings),
                HookOutcome {
                    suppress: true,
                    ..HookOutcome::default()
                }
            );
            assert_eq!(
                state.process_key(KeyEvent::released(Key::Alt, Modifiers::default()), settings),
                HookOutcome {
                    suppress: false,
                    actions: [Some(InputAction::AltReleased), None],
                }
            );
            assert!(
                state
                    .process_key(KeyEvent::released(key, Modifiers::default()), settings)
                    .suppress
            );
        }
    }

    #[test]
    fn function_key_pressed_before_switch_gesture_keeps_its_release_pair() {
        let mut state = HookState::default();
        let settings = HookSettings::default();
        assert_eq!(
            state.process_key(
                KeyEvent::pressed(Key::Function(5), Modifiers::default()),
                settings
            ),
            HookOutcome::default()
        );
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                .suppress
        );
        assert_eq!(
            state.process_key(KeyEvent::released(Key::Function(5), ALT), settings),
            HookOutcome::default()
        );
    }

    #[test]
    fn win_tab_uses_semantic_navigation_commands_then_allows_typed_search() {
        let mut state = HookState::default();
        let settings = HookSettings {
            search_active: true,
            ..HookSettings::default()
        };
        let windows = Modifiers {
            left_windows: true,
            ..Modifiers::default()
        };

        assert!(
            state
                .process_key(KeyEvent::pressed(Key::LeftWindows, windows), settings)
                .suppress
        );
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::Tab, windows), settings)
                .suppress
        );
        assert!(
            state
                .process_key(KeyEvent::released(Key::Tab, windows), settings)
                .suppress
        );
        assert!(
            state
                .process_key(
                    KeyEvent::released(Key::LeftWindows, Modifiers::default()),
                    settings
                )
                .suppress
        );
        assert_eq!(
            state.process_key(
                KeyEvent::pressed(Key::RightArrow, Modifiers::default()),
                settings
            ),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::Navigate(1)), None],
            }
        );
        assert!(
            state
                .process_key(
                    KeyEvent::released(Key::RightArrow, Modifiers::default()),
                    settings
                )
                .suppress
        );
        assert_eq!(
            state.process_key(
                KeyEvent::pressed(Key::Other(u16::from(b'A')), Modifiers::default()).with_text('a'),
                settings
            ),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::AppendSearchCharacter('a')), None],
            }
        );
        state.reset_gestures();
        assert!(
            state
                .process_key(
                    KeyEvent::released(Key::Other(u16::from(b'A')), Modifiers::default()),
                    settings
                )
                .suppress
        );
    }

    #[test]
    fn resetting_gestures_preserves_owned_release_pairs() {
        let mut state = HookState::default();
        let settings = HookSettings::default();
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                .suppress
        );
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::Function(9), ALT), settings)
                .suppress
        );

        state.reset_gestures();

        assert!(
            state
                .process_key(
                    KeyEvent::released(Key::Function(9), Modifiers::default()),
                    settings
                )
                .suppress
        );
    }

    #[test]
    fn rejected_open_reset_allows_a_later_alt_tab_gesture() {
        let mut state = HookState::default();
        let settings = HookSettings::default();

        assert!(
            state
                .process_key(KeyEvent::pressed(Key::Alt, ALT), settings)
                .suppress
        );
        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::Tab, ALT), settings),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::Switch(1)), None],
            }
        );

        state.set_overlay_active(true);
        state.set_overlay_active(false);
        state.reset_gestures();

        assert_eq!(
            state.process_key(KeyEvent::released(Key::Tab, ALT), settings),
            HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            }
        );
        assert_eq!(
            state.process_key(KeyEvent::released(Key::Alt, Modifiers::default()), settings),
            HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            }
        );

        assert!(
            state
                .process_key(KeyEvent::pressed(Key::Alt, ALT), settings)
                .suppress
        );
        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::Tab, ALT), settings),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::Switch(1)), None],
            }
        );
        state.set_overlay_active(true);
        assert_eq!(
            state.process_key(KeyEvent::released(Key::Tab, ALT), settings),
            HookOutcome {
                suppress: true,
                ..HookOutcome::default()
            }
        );
        assert_eq!(
            state.process_key(KeyEvent::released(Key::Alt, Modifiers::default()), settings),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::AltReleased), None],
            }
        );
    }

    #[test]
    fn overlay_numbers_produce_semantic_activation_actions() {
        assert_eq!(
            overlay_key_action(OverlayKeyEvent::pressed(Key::Digit(3))),
            Some(InputAction::ActivateVisiblePosition(3))
        );
        assert_eq!(
            overlay_key_action(OverlayKeyEvent::pressed(Key::NumpadDigit(9))),
            Some(InputAction::ActivateVisiblePosition(9))
        );
    }

    #[test]
    fn overlay_navigation_keys_produce_semantic_actions() {
        assert_eq!(
            overlay_key_action(OverlayKeyEvent::pressed(Key::Tab)),
            Some(InputAction::Switch(1))
        );
        assert_eq!(
            overlay_key_action(OverlayKeyEvent {
                key: Key::Tab,
                repeated: true,
                shift: true,
            }),
            Some(InputAction::Switch(-1))
        );
        assert_eq!(
            overlay_key_action(OverlayKeyEvent::pressed(Key::LeftArrow)),
            Some(InputAction::Navigate(-1))
        );
        assert_eq!(
            overlay_key_action(OverlayKeyEvent::pressed(Key::RightArrow)),
            Some(InputAction::Navigate(1))
        );
        assert_eq!(
            overlay_key_action(OverlayKeyEvent::pressed(Key::Home)),
            Some(InputAction::SelectFirst)
        );
        assert_eq!(
            overlay_key_action(OverlayKeyEvent::pressed(Key::End)),
            Some(InputAction::SelectLast)
        );
    }

    #[test]
    fn overlay_one_shot_keys_ignore_repeated_messages() {
        let cases = [
            (Key::Enter, InputAction::ActivateSelected),
            (Key::Escape, InputAction::DismissOverlay),
            (Key::F4, InputAction::CloseSelected),
            (
                Key::Function(5),
                InputAction::WindowCommand(WindowCommand::Minimize),
            ),
            (
                Key::Function(9),
                InputAction::WindowCommand(WindowCommand::Run),
            ),
        ];

        for (key, action) in cases {
            assert_eq!(
                overlay_key_action(OverlayKeyEvent::pressed(key)),
                Some(action)
            );
            assert_eq!(
                overlay_key_action(OverlayKeyEvent {
                    key,
                    repeated: true,
                    shift: false,
                }),
                None
            );
        }
    }
}
