//! Pure state machines used by the low-level keyboard hook adapter.

mod key;
mod mouse;

use crate::text_edit::TextCommand;
use crate::window_command::WindowCommand;
use key::KeySet;
pub use key::{Key, KeyEvent, KeyTransition, Modifiers, decode_virtual_key};
pub use mouse::MouseEvent;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "fields are independent input-hook feature switches and transient overlay state"
)]
pub struct HookSettings {
    pub replace_alt_tab: bool,
    pub replace_win_tab: bool,
    pub typed_search: bool,
    pub search_active: bool,
    pub filter_visible: bool,
    pub right_button_wheel_switching: bool,
}

impl Default for HookSettings {
    fn default() -> Self {
        Self {
            replace_alt_tab: true,
            replace_win_tab: true,
            typed_search: true,
            search_active: false,
            filter_visible: false,
            right_button_wheel_switching: false,
        }
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
        self.key.virtual_key()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputAction {
    Switch(i32),
    /// Cycle an existing panel without opening a new session after a queued completion.
    CycleSelection(i32),
    /// Raw wheel delta captured while the overlay is open; positive scrolls up.
    OverlayWheel(i32),
    /// Whole wheel steps, negative for up and positive for down.
    MouseWheel(i32),
    MouseWheelReleased,
    RightButtonPressed,
    Navigate(i32),
    ActivateSelected,
    SelectFirst,
    SelectLast,
    DismissOverlay,
    CloseSelected,
    WindowCommand(WindowCommand),
    ActivateVisiblePosition(usize),
    NumberKey {
        position: usize,
        text: Option<char>,
    },
    AltReleased,
    AppendSearchCharacter(char),
    BackspaceSearch,
    EditSearch(TextCommand),
}

impl InputAction {
    /// Successful selection completes this session without cancelling later independent input.
    #[must_use]
    pub const fn completes_selection(self) -> bool {
        matches!(
            self,
            Self::AltReleased
                | Self::MouseWheelReleased
                | Self::ActivateSelected
                | Self::ActivateVisiblePosition(_)
                | Self::NumberKey { .. }
        )
    }
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HookOutcome {
    pub suppress: bool,
    actions: [Option<InputAction>; 2],
}

impl HookOutcome {
    const fn suppressed() -> Self {
        Self {
            suppress: true,
            actions: [None, None],
        }
    }

    const fn one(suppress: bool, action: InputAction) -> Self {
        Self {
            suppress,
            actions: [Some(action), None],
        }
    }

    pub fn actions(&self) -> impl Iterator<Item = InputAction> + '_ {
        self.actions.iter().flatten().copied()
    }
}

#[derive(Debug, Default)]
pub struct HookState {
    shift_keys: u8,
    pressed_owned_keys: KeySet,
    suppressed_shift_keys: u8,
    suppressed_owned_key_releases: KeySet,
    pending_alt_keys: u8,
    pending_windows_keys: u8,
    replayed_key_events: [Option<ReplayedKeyEvent>; 3],
    overlay_active: bool,
    alt_switch_gesture_active: bool,
    win_switch_gesture_active: bool,
    interception: InterceptionState,
    mouse: mouse::MouseGesture,
}

#[derive(Debug, Default)]
struct InterceptionState {
    suspended: bool,
    // Keys held across a modal boundary stay with their original recipient until release.
    passthrough_keys: KeySet,
}

impl HookState {
    /// Cancels gestures without forgetting suppressed key presses.
    /// The adapter calls this for every suspension generation, including a missed open/close.
    pub fn set_interception_suspended(&mut self, suspended: bool) {
        self.interception.suspended = suspended;
        self.cancel_held_gestures();
    }

    fn cancel_held_gestures(&mut self) {
        self.interception
            .passthrough_keys
            .insert_all(self.pressed_owned_keys);
        self.reset_gestures();
    }

    pub fn set_overlay_active(&mut self, active: bool) {
        if !active {
            // Completion or a refused opening ends sticky keyboard capture. Pending openings
            // keep interception armed, so this cannot retire a later queued gesture.
            self.alt_switch_gesture_active = false;
            self.win_switch_gesture_active = false;
            if self.mouse.is_active() {
                // Keep the swallowed button's release debt, pending clicks and sub-notch
                // accumulation. Only a selection without a visible/pending opening ends.
                self.mouse.cancel();
            }
        }
        self.overlay_active = active;
    }

    /// Rebases a modifier after an input-desktop gap, independently of release ownership.
    pub fn rebase_modifier(&mut self, key: Key, pressed: bool) {
        let pending_alt = alt_key_mask(key).is_some_and(|mask| self.pending_alt_keys & mask != 0);
        let pending_windows =
            windows_key_mask(key).is_some_and(|mask| self.pending_windows_keys & mask != 0);
        if let Some(mask) = alt_key_mask(key) {
            self.pending_alt_keys &= !mask;
        }
        if let Some(mask) = windows_key_mask(key) {
            self.pending_windows_keys &= !mask;
        }
        let event = if pressed {
            KeyEvent::pressed(key, Modifiers::default())
        } else {
            KeyEvent::released(key, Modifiers::default())
        };
        self.update_shift_state(event);
        self.update_owned_key_state(event);
        if pressed {
            // An async down means Windows has a press to balance, so its release must pass.
            self.take_suppressed_owned_key_release(key);
            if let Some(mask) = shift_mask(key) {
                self.suppressed_shift_keys &= !mask;
            }
        } else if pending_alt || pending_windows {
            self.suppress_owned_key_release(key);
        }
        self.interception.passthrough_keys.set(key, pressed);
    }

    pub fn reset_gestures(&mut self) {
        self.mouse.cancel();
        self.overlay_active = false;
        self.alt_switch_gesture_active = false;
        self.win_switch_gesture_active = false;
    }

    #[must_use]
    pub fn process_mouse(&mut self, event: MouseEvent, settings: HookSettings) -> HookOutcome {
        let outcome = self.mouse.process(
            event,
            settings.right_button_wheel_switching && !self.interception.suspended,
        );
        if !outcome.suppress
            && outcome.actions().next().is_none()
            && let MouseEvent::Wheel(delta) = event
        {
            return self.process_overlay_wheel(i32::from(delta));
        }
        if outcome
            .actions()
            .any(|action| matches!(action, InputAction::MouseWheel(_)))
        {
            self.alt_switch_gesture_active = false;
            self.win_switch_gesture_active = false;
        }
        if event == MouseEvent::RightButtonPressed
            && self.overlay_active
            && settings.right_button_wheel_switching
            && !self.interception.suspended
        {
            HookOutcome::one(outcome.suppress, InputAction::RightButtonPressed)
        } else {
            outcome
        }
    }

    pub fn take_replayed_key_events(&mut self) -> [Option<ReplayedKeyEvent>; 3] {
        core::mem::take(&mut self.replayed_key_events)
    }

    #[must_use]
    pub fn process_overlay_wheel(&self, delta: i32) -> HookOutcome {
        if delta != 0 && self.overlay_active && !self.interception.suspended {
            HookOutcome::one(true, InputAction::OverlayWheel(delta))
        } else {
            HookOutcome::default()
        }
    }

    #[must_use]
    pub fn owns_right_button(&self) -> bool {
        self.mouse.owns_button()
    }

    pub fn take_replayed_right_click(&mut self) -> bool {
        self.mouse.take_replayed_click()
    }

    #[must_use]
    pub fn process_key(&mut self, event: KeyEvent, settings: HookSettings) -> HookOutcome {
        let pressed = event.transition == KeyTransition::Pressed;
        let released = event.transition == KeyTransition::Released;
        let key_was_down = self.observe_key_transition(event);
        if let Some(outcome) = self.process_retained_key(event, settings) {
            return outcome;
        }
        let switching_allowed = ![
            Key::Alt,
            Key::LeftAlt,
            Key::RightAlt,
            Key::LeftWindows,
            Key::RightWindows,
        ]
        .into_iter()
        .any(|key| self.key_passes_until_release(key));
        if let Some(outcome) = self.process_alt_key_transition(event, settings) {
            return outcome;
        }
        let pending_alt_keys = self.pending_alt_keys;
        // Suppressed Alt presses never set Windows' LLKHF_ALTDOWN flag. Keep using
        // the observed physical press after the first Tab consumes pending_alt_keys.
        let alt_down = event.modifiers.alt || self.any_alt_key_down();
        let alt_tab = switching_allowed
            && settings.replace_alt_tab
            && pressed
            && event.key == Key::Tab
            && alt_down;
        if let Some(outcome) = self.process_pending_alt_shortcut(event, alt_tab) {
            return outcome;
        }
        if let Some(outcome) = self.process_windows_key_transition(event, settings) {
            return outcome;
        }
        // A suppressed Windows-key press never reaches the system async state, so the hook's
        // observed pending press is authoritative when the following Tab callback arrives.
        let pending_windows_keys = self.pending_windows_keys;
        let windows_tab = switching_allowed
            && settings.replace_win_tab
            && pressed
            && event.key == Key::Tab
            && (pending_windows_keys != 0
                || (self.win_switch_gesture_active && windows_modifier_mask(event.modifiers) != 0));

        if let Some(outcome) = self.process_pending_windows_shortcut(event, windows_tab) {
            return outcome;
        }

        if is_ctrl_or_print_screen(event.key) {
            return HookOutcome::default();
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
            return HookOutcome::suppressed();
        }

        if let Some(outcome) = self.process_editing_key(event, settings) {
            return outcome;
        }

        if let Some(outcome) = self.process_command_key(event) {
            return outcome;
        }

        if let Some(outcome) = self.process_boundary_key(event) {
            return outcome;
        }

        if let Some(outcome) = self.process_navigation(event, alt_down) {
            return outcome;
        }

        if self.should_suppress_shift(event) {
            return HookOutcome::suppressed();
        }

        if let Some(outcome) =
            self.process_number_shortcut(event, key_was_down, alt_down, settings.search_active)
        {
            return outcome;
        }

        if let Some(outcome) = self.process_search_key(event, settings.search_active) {
            return outcome;
        }

        if pressed && self.win_switch_gesture_active {
            self.suppress_owned_key_release(event.key);
            return HookOutcome::suppressed();
        }

        HookOutcome::default()
    }

    fn observe_key_transition(&mut self, event: KeyEvent) -> bool {
        self.update_shift_state(event);
        let key_was_down = self.owned_key_is_down(event.key);
        if event.transition == KeyTransition::Pressed && !key_was_down {
            // Reconciliation may retain an old release debt after clearing a stale down bit.
            // A fresh press begins a new pair; an old release alone is still suppressed.
            self.take_suppressed_owned_key_release(event.key);
            if let Some(mask) = shift_mask(event.key) {
                self.suppressed_shift_keys &= !mask;
            }
        }
        self.update_owned_key_state(event);
        key_was_down
    }

    fn process_retained_key(
        &mut self,
        event: KeyEvent,
        settings: HookSettings,
    ) -> Option<HookOutcome> {
        if self.interception.suspended || self.key_passes_until_release(event.key) {
            return Some(self.process_unintercepted_key(event));
        }
        if event.transition == KeyTransition::Pressed
            && !self.switch_gesture_active()
            && !settings.search_active
            && (self.owned_key_release_pending(event.key)
                || shift_mask(event.key).is_some_and(|mask| self.suppressed_shift_keys & mask != 0))
        {
            // Until a release or rebase establishes a fresh press, keep the whole old pair
            // suppressed, including repeats after search or a gesture has ended.
            return Some(HookOutcome::suppressed());
        }
        None
    }

    fn key_passes_until_release(&self, key: Key) -> bool {
        self.interception.passthrough_keys.contains(key)
    }

    fn process_unintercepted_key(&mut self, event: KeyEvent) -> HookOutcome {
        let pressed = event.transition == KeyTransition::Pressed;
        self.interception.passthrough_keys.set(event.key, pressed);
        let owned = if pressed {
            self.owned_key_release_pending(event.key)
        } else {
            self.take_suppressed_owned_key_release(event.key)
        };
        let shift_owned = shift_mask(event.key).is_some_and(|mask| {
            let owned = self.suppressed_shift_keys & mask != 0;
            if !pressed {
                self.suppressed_shift_keys &= !mask;
            }
            owned
        });
        if owned || shift_owned {
            return HookOutcome::suppressed();
        }
        let disabled = HookSettings {
            replace_alt_tab: false,
            replace_win_tab: false,
            ..HookSettings::default()
        };
        self.process_alt_key_transition(event, disabled)
            .or_else(|| self.process_windows_key_transition(event, disabled))
            .or_else(|| self.process_pending_alt_shortcut(event, false))
            .or_else(|| self.process_pending_windows_shortcut(event, false))
            .unwrap_or_default()
    }

    fn process_number_shortcut(
        &mut self,
        event: KeyEvent,
        key_was_down: bool,
        alt_down: bool,
        search_active: bool,
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
        let action = if search_active {
            if let InputAction::ActivateVisiblePosition(position) = action {
                InputAction::NumberKey {
                    position,
                    text: event.text.filter(|character| !character.is_control()),
                }
            } else {
                action
            }
        } else {
            action
        };
        if (self.alt_switch_gesture_active && alt_down)
            || self.win_switch_gesture_active
            || self.mouse.is_active()
            || self.overlay_active
        {
            self.mouse.cancel();
            self.alt_switch_gesture_active = false;
            // An unavailable position leaves the panel open. Keep sticky Win+Tab number
            // routing until the UI acknowledges completion through set_overlay_active.
            self.suppress_owned_key_release(event.key);
            return Some(HookOutcome::one(true, action));
        }
        None
    }

    fn process_windows_key_transition(
        &mut self,
        event: KeyEvent,
        settings: HookSettings,
    ) -> Option<HookOutcome> {
        let mask = windows_key_mask(event.key)?;
        if event.transition == KeyTransition::Pressed && self.alt_switch_gesture_active {
            // Start owns the Windows key, including repeats and release. Cancel the
            // switch while retaining the swallowed Alt/Tab releases until they arrive.
            self.cancel_held_gestures();
            return Some(HookOutcome::one(false, InputAction::DismissOverlay));
        }
        if event.transition == KeyTransition::Released && self.pending_windows_keys & mask != 0 {
            self.pending_windows_keys &= !mask;
            return Some(self.replay_tap(event.key));
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
        Some(HookOutcome::suppressed())
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
                return Some(self.replay_tap(event.key));
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
            return Some(HookOutcome::suppressed());
        }
        if !self.owned_key_release_pending(event.key) {
            self.pending_alt_keys |= mask;
        }
        Some(HookOutcome::suppressed())
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
        self.mouse.cancel();
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
        let held = core::mem::take(&mut self.pending_alt_keys);
        Some(self.replay_pending_shortcut(alt_keys(held), event.key))
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
        let held = core::mem::take(&mut self.pending_windows_keys);
        Some(self.replay_pending_shortcut(windows_keys(held), event.key))
    }

    /// Replays a held-back modifier's lone tap, which the system still acts on (Start, menu bar).
    fn replay_tap(&mut self, key: Key) -> HookOutcome {
        self.replayed_key_events = [
            Some(ReplayedKeyEvent::pressed(key)),
            Some(ReplayedKeyEvent::released(key)),
            None,
        ];
        HookOutcome::suppressed()
    }

    /// Replays held-back modifier presses followed by the key that turned them into a shortcut.
    fn replay_pending_shortcut(
        &mut self,
        held: impl Iterator<Item = Key>,
        key: Key,
    ) -> HookOutcome {
        let mut presses = held.chain([key]).map(ReplayedKeyEvent::pressed);
        for slot in &mut self.replayed_key_events {
            *slot = presses.next();
        }
        // Behind three held Alt keys the key no longer fits, so it passes through unreplayed.
        HookOutcome {
            suppress: presses.next().is_none(),
            ..HookOutcome::default()
        }
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

    fn process_navigation(&mut self, event: KeyEvent, alt_down: bool) -> Option<HookOutcome> {
        let mut action = overlay_key_action(OverlayKeyEvent {
            key: event.key,
            repeated: false,
            shift: self.shift_down(),
        })?;
        if let InputAction::Switch(delta) = action {
            if alt_down
                || windows_modifier_mask(event.modifiers) != 0
                || self.pending_windows_keys != 0
            {
                return None;
            }
            action = InputAction::CycleSelection(delta);
        }
        if !matches!(
            action,
            InputAction::Navigate(_) | InputAction::CycleSelection(_)
        ) {
            return None;
        }
        if event.transition == KeyTransition::Pressed && self.overlay_controls_active() {
            self.suppress_owned_key_release(event.key);
            return Some(HookOutcome::one(true, action));
        }
        None
    }

    fn process_command_key(&mut self, event: KeyEvent) -> Option<HookOutcome> {
        if !matches!(event.key, Key::Enter | Key::F4 | Key::Function(5..=9))
            || event.transition != KeyTransition::Pressed
        {
            return None;
        }
        if self.owned_key_release_pending(event.key) {
            return Some(HookOutcome::suppressed());
        }
        if !self.overlay_controls_active() {
            return None;
        }

        if event.key == Key::Enter {
            self.mouse.cancel();
            self.alt_switch_gesture_active = false;
            self.win_switch_gesture_active = false;
        }
        self.suppress_owned_key_release(event.key);
        let action = overlay_key_action(OverlayKeyEvent::pressed(event.key))?;
        Some(HookOutcome::one(true, action))
    }

    fn process_escape(&mut self, event: KeyEvent) -> Option<HookOutcome> {
        if event.key != Key::Escape {
            return None;
        }
        if event.transition == KeyTransition::Released {
            return self
                .take_suppressed_owned_key_release(event.key)
                .then_some(HookOutcome::suppressed());
        }
        if self.owned_key_release_pending(event.key) {
            return Some(HookOutcome::suppressed());
        }
        if !self.overlay_active {
            return None;
        }

        self.mouse.cancel();
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
        if event.transition == KeyTransition::Pressed && self.overlay_controls_active() {
            self.suppress_owned_key_release(event.key);
            return Some(HookOutcome::one(true, action));
        }
        None
    }

    fn process_editing_key(
        &mut self,
        event: KeyEvent,
        settings: HookSettings,
    ) -> Option<HookOutcome> {
        if !settings.search_active
            || event.transition != KeyTransition::Pressed
            || event.modifiers.left_windows
            || event.modifiers.right_windows
        {
            return None;
        }
        let command = TextCommand::for_key(
            u32::from(event.key.virtual_key()),
            event.modifiers.control,
            event.modifiers.shift,
            self.owned_key_is_down(Key::RightAlt),
        )?;
        // An empty query has no caret or selection to move. Keep modified navigation on the
        // switcher route too, including its paired release and final Alt-release activation.
        if !settings.filter_visible
            && self.overlay_controls_active()
            && matches!(
                event.key,
                Key::LeftArrow | Key::RightArrow | Key::Home | Key::End
            )
        {
            return None;
        }
        // Plain navigation also moves through windows when the query is empty. Preserve the
        // held switch gesture, as for Home/End; typing and modified edits still pin the panel.
        if !matches!(
            command,
            TextCommand::Left | TextCommand::Right | TextCommand::Start | TextCommand::End
        ) || event.modifiers.control
            || event.modifiers.shift
        {
            self.mouse.cancel();
            self.alt_switch_gesture_active = false;
            self.win_switch_gesture_active = false;
        }
        self.suppress_owned_key_release(event.key);
        let action = if command == TextCommand::Backspace {
            InputAction::BackspaceSearch
        } else {
            InputAction::EditSearch(command)
        };
        Some(HookOutcome::one(true, action))
    }
    fn process_search_key(&mut self, event: KeyEvent, search_active: bool) -> Option<HookOutcome> {
        // Typing a key whose release cannot be owned would leak that release to the foreground.
        if !KeySet::can_hold(event.key) || event.transition == KeyTransition::Released {
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
        self.mouse.cancel();
        self.alt_switch_gesture_active = false;
        self.win_switch_gesture_active = false;
        self.suppress_owned_key_release(event.key);
        Some(HookOutcome::one(true, action))
    }

    fn suppress_owned_key_release(&mut self, key: Key) {
        self.suppressed_owned_key_releases.insert(key);
    }

    fn update_owned_key_state(&mut self, event: KeyEvent) {
        self.pressed_owned_keys
            .set(event.key, event.transition == KeyTransition::Pressed);
    }

    fn owned_key_is_down(&self, key: Key) -> bool {
        self.pressed_owned_keys.contains(key)
    }

    fn any_alt_key_down(&self) -> bool {
        [Key::Alt, Key::LeftAlt, Key::RightAlt]
            .into_iter()
            .any(|key| self.owned_key_is_down(key))
    }

    fn take_suppressed_owned_key_release(&mut self, key: Key) -> bool {
        self.suppressed_owned_key_releases.remove(key)
    }

    fn owned_key_release_pending(&self, key: Key) -> bool {
        self.suppressed_owned_key_releases.contains(key)
    }

    const fn shift_down(&self) -> bool {
        self.shift_keys != 0
    }

    const fn switch_gesture_active(&self) -> bool {
        self.alt_switch_gesture_active || self.win_switch_gesture_active
    }

    const fn overlay_controls_active(&self) -> bool {
        self.switch_gesture_active() || self.overlay_active || self.mouse.is_active()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_keys_in_a_filtered_gesture_keep_translated_layout_text() {
        use crate::switcher::{SwitchTask, SwitcherSession, SwitcherSessionSettings};
        for character in ['&', '!', '1'] {
            let settings = HookSettings {
                search_active: true,
                ..HookSettings::default()
            };
            let mut hook = HookState::default();
            hook.set_overlay_active(true);
            let mut session = SwitcherSession::new(SwitcherSessionSettings {
                typed_search: true,
                release_alt_switches: true,
            });
            session.open([SwitchTask::new(1, 1, "a & ! 1", "editor")], None);
            let _effect = session.handle_input(InputAction::AppendSearchCharacter('a'));
            for event in [
                KeyEvent::pressed(Key::LeftAlt, ALT),
                KeyEvent::pressed(Key::Tab, ALT),
                KeyEvent::pressed(Key::Digit(1), ALT).with_text(character),
            ] {
                for action in hook.process_key(event, settings).actions() {
                    let _effect = session.handle_input(action);
                }
            }
            assert_eq!(session.switcher().filter(), format!("a{character}"));
        }
    }

    #[test]
    fn altgr_text_is_not_a_control_editing_shortcut() {
        let settings = HookSettings {
            search_active: true,
            ..HookSettings::default()
        };
        let modifiers = Modifiers {
            alt: true,
            control: true,
            ..Modifiers::default()
        };
        let mut state = HookState::default();
        state.rebase_modifier(Key::RightAlt, true);
        let outcome = state.process_key(
            KeyEvent::pressed(Key::Other(u16::from(b'A')), modifiers).with_text('ą'),
            settings,
        );
        assert!(outcome.suppress);
        assert_eq!(
            outcome.actions().collect::<Vec<_>>(),
            [InputAction::AppendSearchCharacter('ą')]
        );
    }

    #[test]
    fn editing_shortcuts_are_commands_and_own_their_key_releases() {
        for (key, command) in [
            (b'A', TextCommand::SelectAll),
            (b'C', TextCommand::Copy),
            (b'X', TextCommand::Cut),
            (b'V', TextCommand::Paste),
            (b'Z', TextCommand::Undo),
            (b'Y', TextCommand::Redo),
        ] {
            let mut state = HookState::default();
            let settings = HookSettings {
                search_active: true,
                ..HookSettings::default()
            };
            let modifiers = Modifiers {
                control: true,
                ..Modifiers::default()
            };
            let key = Key::Other(u16::from(key));
            let outcome = state.process_key(KeyEvent::pressed(key, modifiers), settings);
            assert!(outcome.suppress);
            assert_eq!(
                outcome.actions().collect::<Vec<_>>(),
                vec![InputAction::EditSearch(command)]
            );
            assert!(
                state
                    .process_key(KeyEvent::released(key, modifiers), settings)
                    .suppress
            );
            assert!(
                !HookState::default()
                    .process_key(KeyEvent::pressed(key, modifiers), HookSettings::default())
                    .suppress
            );
        }
    }

    #[test]
    fn owned_alt_tab_cycles_without_windows_alt_flag() {
        for alt in [Key::LeftAlt, Key::RightAlt] {
            let mut state = HookState::default();
            let settings = HookSettings::default();
            let modifiers = Modifiers::default();
            assert!(
                state
                    .process_key(KeyEvent::pressed(alt, modifiers), settings)
                    .suppress
            );
            for delta in [1, 1, -1, 1] {
                if delta == -1 {
                    let _ =
                        state.process_key(KeyEvent::pressed(Key::LeftShift, modifiers), settings);
                }
                assert_eq!(
                    state.process_key(KeyEvent::pressed(Key::Tab, modifiers), settings),
                    HookOutcome::one(true, InputAction::Switch(delta))
                );
                assert!(
                    state
                        .process_key(KeyEvent::released(Key::Tab, modifiers), settings)
                        .suppress
                );
                if delta == -1 {
                    let _ =
                        state.process_key(KeyEvent::released(Key::LeftShift, modifiers), settings);
                }
            }
            assert_eq!(
                state.process_key(KeyEvent::released(alt, modifiers), settings),
                HookOutcome::one(true, InputAction::AltReleased)
            );
        }
    }

    #[test]
    fn windows_key_cancels_alt_switch_without_losing_release_ownership() {
        for windows in [Key::LeftWindows, Key::RightWindows] {
            let mut state = HookState::default();
            let settings = HookSettings::default();
            let modifiers = Modifiers::default();
            let _ = state.process_key(KeyEvent::pressed(Key::LeftAlt, modifiers), settings);
            let _ = state.process_key(KeyEvent::pressed(Key::Tab, modifiers), settings);
            assert_eq!(
                state.process_key(KeyEvent::pressed(windows, modifiers), settings),
                HookOutcome::one(false, InputAction::DismissOverlay)
            );
            assert_eq!(
                state.process_key(KeyEvent::pressed(windows, modifiers), settings),
                HookOutcome::default()
            );
            assert!(
                state
                    .process_key(KeyEvent::released(Key::Tab, modifiers), settings)
                    .suppress
            );
            assert_eq!(
                state.process_key(KeyEvent::released(windows, modifiers), settings),
                HookOutcome::default()
            );
            assert_eq!(state.take_replayed_key_events(), [None; 3]);
            assert_eq!(
                state.process_key(KeyEvent::released(Key::LeftAlt, modifiers), settings),
                HookOutcome {
                    suppress: true,
                    ..HookOutcome::default()
                }
            );
            let _ = state.process_key(KeyEvent::pressed(Key::LeftAlt, modifiers), settings);
            assert_eq!(
                state.process_key(KeyEvent::pressed(Key::Tab, modifiers), settings),
                HookOutcome::one(true, InputAction::Switch(1))
            );
        }
    }

    #[test]
    fn rebased_shift_release_debt_does_not_swallow_a_fresh_forwarded_pair() {
        let mut state = HookState::default();
        let settings = HookSettings::default();
        let _tab = state.process_key(KeyEvent::pressed(Key::Tab, ALT), settings);
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::LeftShift, ALT), settings)
                .suppress
        );
        assert_ne!(state.suppressed_shift_keys, 0);
        state.reset_gestures();
        state.rebase_modifier(Key::LeftShift, false);
        assert_ne!(state.suppressed_shift_keys, 0);
        for event in [
            KeyEvent::pressed(Key::LeftShift, Modifiers::default()),
            KeyEvent::released(Key::LeftShift, Modifiers::default()),
        ] {
            assert_eq!(state.process_key(event, settings), HookOutcome::default());
        }
    }

    #[test]
    fn rebased_owned_keys_start_fresh_pairs_and_unrebased_keys_remain_fully_owned() {
        for key in [
            Key::LeftAlt,
            Key::RightAlt,
            Key::LeftWindows,
            Key::RightWindows,
            Key::Other(65),
        ] {
            for rebase in [false, true] {
                let mut state = HookState::default();
                let settings = HookSettings::default();
                let _win = state.process_key(
                    KeyEvent::pressed(Key::LeftWindows, Modifiers::default()),
                    settings,
                );
                let _tab =
                    state.process_key(KeyEvent::pressed(Key::Tab, Modifiers::default()), settings);
                assert!(
                    state
                        .process_key(KeyEvent::pressed(key, Modifiers::default()), settings)
                        .suppress
                );
                state.reset_gestures();
                if rebase {
                    state.rebase_modifier(key, false);
                }
                let settings = HookSettings {
                    replace_alt_tab: false,
                    replace_win_tab: false,
                    ..settings
                };
                for event in [
                    KeyEvent::pressed(key, Modifiers::default()),
                    KeyEvent::released(key, Modifiers::default()),
                ] {
                    let outcome = state.process_key(event, settings);
                    assert_eq!(outcome.suppress, !rebase);
                    assert!(outcome.actions().next().is_none());
                }
            }
        }
    }

    #[test]
    fn ordinary_commands_share_ownership_but_only_enter_ends_the_gesture() {
        for key in [
            Key::Enter,
            Key::F4,
            Key::Function(5),
            Key::Function(6),
            Key::Function(7),
            Key::Function(8),
            Key::Function(9),
        ] {
            let mut state = HookState::default();
            let settings = HookSettings::default();
            let _alt = state.process_key(KeyEvent::pressed(Key::LeftAlt, ALT), settings);
            let _tab = state.process_key(KeyEvent::pressed(Key::Tab, ALT), settings);
            let action = state.process_key(KeyEvent::pressed(key, ALT), settings);
            assert!(action.suppress);
            assert_eq!(
                action.actions().next(),
                overlay_key_action(OverlayKeyEvent::pressed(key))
            );
            for event in [KeyEvent::pressed(key, ALT), KeyEvent::released(key, ALT)] {
                let outcome = state.process_key(event, settings);
                assert!(outcome.suppress);
                assert!(outcome.actions().next().is_none());
            }
            let release = state.process_key(
                KeyEvent::released(Key::LeftAlt, Modifiers::default()),
                settings,
            );
            assert!(release.suppress);
            assert_eq!(
                release.actions().next(),
                (key != Key::Enter).then_some(InputAction::AltReleased)
            );
        }
    }

    #[test]
    fn print_screen_replays_both_sides_and_repeats_without_replaying_modifiers_twice() {
        for modifiers in [
            [Key::LeftAlt, Key::RightAlt],
            [Key::LeftWindows, Key::RightWindows],
        ] {
            let mut state = HookState::default();
            let settings = HookSettings::default();
            for key in modifiers {
                assert!(
                    state
                        .process_key(KeyEvent::pressed(key, Modifiers::default()), settings)
                        .suppress
                );
            }
            assert!(
                state
                    .process_key(
                        KeyEvent::pressed(Key::PrintScreen, Modifiers::default()),
                        settings
                    )
                    .suppress
            );
            assert_eq!(
                state.take_replayed_key_events(),
                [
                    Some(ReplayedKeyEvent::pressed(modifiers[0])),
                    Some(ReplayedKeyEvent::pressed(modifiers[1])),
                    Some(ReplayedKeyEvent::pressed(Key::PrintScreen))
                ]
            );
            assert_eq!(
                state.process_key(
                    KeyEvent::pressed(Key::PrintScreen, Modifiers::default()),
                    settings
                ),
                HookOutcome::default()
            );
            assert_eq!(state.take_replayed_key_events(), [None; 3]);
        }
    }

    #[test]
    fn suspension_passes_switching_and_typing_without_actions() {
        let mut state = HookState::default();
        let settings = HookSettings {
            search_active: true,
            ..HookSettings::default()
        };
        state.set_overlay_active(true);
        state.set_interception_suspended(true);
        for key in [
            Key::LeftAlt,
            Key::Tab,
            Key::Other(65),
            Key::Escape,
            Key::F4,
            Key::Enter,
            Key::LeftWindows,
        ] {
            assert_eq!(
                state.process_key(KeyEvent::pressed(key, ALT).with_text('a'), settings),
                HookOutcome::default()
            );
            assert_eq!(
                state.process_key(KeyEvent::released(key, Modifiers::default()), settings),
                HookOutcome::default()
            );
        }
        assert!(!state.switch_gesture_active());
    }

    #[test]
    fn suspension_keeps_owned_repeats_and_releases_suppressed() {
        let mut state = HookState::default();
        let settings = HookSettings {
            search_active: true,
            ..HookSettings::default()
        };
        for key in [Key::LeftAlt, Key::Tab, Key::LeftShift] {
            assert!(
                state
                    .process_key(KeyEvent::pressed(key, ALT), settings)
                    .suppress
            );
        }
        assert!(
            state
                .process_key(
                    KeyEvent::pressed(Key::Other(65), ALT).with_text('a'),
                    settings
                )
                .suppress
        );
        state.set_interception_suspended(true);
        for key in [Key::LeftAlt, Key::Tab, Key::LeftShift, Key::Other(65)] {
            for event in [
                KeyEvent::pressed(key, ALT),
                KeyEvent::released(key, Modifiers::default()),
            ] {
                let outcome = state.process_key(event, settings);
                assert!(outcome.suppress);
                assert!(outcome.actions().next().is_none());
            }
            assert_eq!(
                state.process_key(KeyEvent::released(key, Modifiers::default()), settings),
                HookOutcome::default()
            );
        }
    }

    #[test]
    fn suspension_replays_pending_modifiers_for_shortcuts_and_lone_releases() {
        for modifier in [
            Key::LeftAlt,
            Key::RightAlt,
            Key::LeftWindows,
            Key::RightWindows,
        ] {
            for shortcut in [None, Some(Key::PrintScreen), Some(Key::Other(82))] {
                let mut state = HookState::default();
                let settings = HookSettings::default();
                assert!(
                    state
                        .process_key(KeyEvent::pressed(modifier, Modifiers::default()), settings)
                        .suppress
                );
                state.set_interception_suspended(true);
                let event = shortcut
                    .map_or(KeyEvent::released(modifier, Modifiers::default()), |key| {
                        KeyEvent::pressed(key, Modifiers::default())
                    });
                let outcome = state.process_key(event, settings);
                assert!(outcome.suppress);
                assert!(outcome.actions().next().is_none());
                assert_eq!(
                    state.take_replayed_key_events(),
                    [
                        Some(ReplayedKeyEvent::pressed(modifier)),
                        Some(ReplayedKeyEvent {
                            key: event.key,
                            transition: event.transition
                        }),
                        None
                    ]
                );
            }
        }
    }

    #[test]
    fn resume_does_not_inherit_held_switch_or_typing_keys() {
        for owned in [false, true] {
            let mut state = HookState::default();
            let settings = HookSettings::default();
            if !owned {
                state.set_interception_suspended(true);
            }
            let _alt = state.process_key(KeyEvent::pressed(Key::LeftAlt, ALT), settings);
            let _tab = state.process_key(KeyEvent::pressed(Key::Tab, ALT), settings);
            state.set_interception_suspended(true);
            let _typing = state.process_key(
                KeyEvent::pressed(Key::Other(65), ALT).with_text('a'),
                settings,
            );
            state.set_interception_suspended(false);
            for key in [Key::Tab, Key::Other(65)] {
                assert!(
                    state
                        .process_key(
                            KeyEvent::pressed(key, ALT).with_text('a'),
                            HookSettings {
                                search_active: true,
                                ..settings
                            }
                        )
                        .actions()
                        .next()
                        .is_none()
                );
                let _release = state.process_key(KeyEvent::released(key, ALT), settings);
            }
            assert!(
                state
                    .process_key(KeyEvent::pressed(Key::Tab, ALT), settings)
                    .actions()
                    .next()
                    .is_none()
            );
            let _tab = state.process_key(KeyEvent::released(Key::Tab, ALT), settings);
            let _alt = state.process_key(
                KeyEvent::released(Key::LeftAlt, Modifiers::default()),
                settings,
            );
            let _alt = state.process_key(KeyEvent::pressed(Key::LeftAlt, ALT), settings);
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
    fn pending_modifiers_are_replayed_before_print_screen() {
        for modifier in [
            Key::LeftAlt,
            Key::RightAlt,
            Key::LeftWindows,
            Key::RightWindows,
        ] {
            let mut state = HookState::default();
            let settings = HookSettings::default();
            assert!(
                state
                    .process_key(KeyEvent::pressed(modifier, Modifiers::default()), settings)
                    .suppress
            );
            let outcome = state.process_key(
                KeyEvent::pressed(Key::PrintScreen, Modifiers::default()),
                settings,
            );
            assert!(outcome.suppress);
            assert!(outcome.actions().next().is_none());
            assert_eq!(
                state.take_replayed_key_events(),
                [
                    Some(ReplayedKeyEvent::pressed(modifier)),
                    Some(ReplayedKeyEvent::pressed(Key::PrintScreen)),
                    None
                ]
            );
            for key in [Key::PrintScreen, modifier] {
                assert_eq!(
                    state.process_key(KeyEvent::released(key, Modifiers::default()), settings),
                    HookOutcome::default()
                );
            }
        }
    }

    const ALT: Modifiers = Modifiers {
        alt: true,
        control: false,
        shift: false,
        left_windows: false,
        right_windows: false,
    };

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
    fn held_modifiers_replay_ahead_of_the_shortcut_key_while_it_fits() {
        let settings = HookSettings::default();
        let mut state = HookState::default();
        for alt in [Key::LeftAlt, Key::RightAlt, Key::Alt] {
            assert!(
                state
                    .process_key(KeyEvent::pressed(alt, ALT), settings)
                    .suppress
            );
        }
        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::F4, ALT), settings),
            HookOutcome::default()
        );
        assert_eq!(
            state.take_replayed_key_events(),
            [Key::LeftAlt, Key::RightAlt, Key::Alt].map(|key| Some(ReplayedKeyEvent::pressed(key)))
        );

        let mut state = HookState::default();
        for windows in [Key::LeftWindows, Key::RightWindows] {
            assert!(
                state
                    .process_key(KeyEvent::pressed(windows, Modifiers::default()), settings)
                    .suppress
            );
        }
        let shortcut = Key::Other(u16::from(b'R'));
        assert_eq!(
            state.process_key(KeyEvent::pressed(shortcut, Modifiers::default()), settings),
            HookOutcome::suppressed()
        );
        assert_eq!(
            state.take_replayed_key_events(),
            [Key::LeftWindows, Key::RightWindows, shortcut]
                .map(|key| Some(ReplayedKeyEvent::pressed(key)))
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
    fn remote_desktop_passthrough_forwards_alt_tab_instead_of_opening_the_switcher() {
        let mut state = HookState::default();
        let settings = crate::passthrough::PassthroughPolicy::RemoteDesktopFullscreen
            .apply(HookSettings::default());

        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::LeftAlt, ALT), settings),
            HookOutcome::default()
        );
        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::Tab, ALT), settings),
            HookOutcome::default()
        );
        assert_eq!(state.take_replayed_key_events(), [None; 3]);
    }

    #[test]
    fn windowed_remote_desktop_keeps_alt_tab_replacement() {
        let settings = crate::passthrough::PassthroughPolicy::from_foreground(true, false)
            .apply(HookSettings::default());
        let mut state = HookState::default();
        assert!(
            state
                .process_key(KeyEvent::pressed(Key::LeftAlt, ALT), settings)
                .suppress
        );
        assert_eq!(
            state.process_key(KeyEvent::pressed(Key::Tab, ALT), settings),
            HookOutcome {
                suppress: true,
                actions: [Some(InputAction::Switch(1)), None],
            }
        );
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
                actions: [
                    Some(InputAction::NumberKey {
                        position: 9,
                        text: None
                    }),
                    None
                ],
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
    fn empty_query_navigation_preserves_alt_release_activation() {
        use crate::switcher::{
            SwitchTask, SwitcherEffect, SwitcherSession, SwitcherSessionSettings,
        };

        for (control, shift) in [(false, false), (false, true), (true, false), (true, true)] {
            let modifiers = Modifiers {
                control,
                shift,
                ..Modifiers::default()
            };
            for (key, target) in [
                (Key::LeftArrow, 10),
                (Key::RightArrow, 30),
                (Key::Home, 10),
                (Key::End, 30),
            ] {
                let mut state = HookState::default();
                let settings = HookSettings {
                    search_active: true,
                    ..HookSettings::default()
                };
                let mut session = SwitcherSession::new(SwitcherSessionSettings {
                    typed_search: true,
                    release_alt_switches: true,
                });
                assert!(
                    state
                        .process_key(KeyEvent::pressed(Key::LeftAlt, modifiers), settings)
                        .suppress
                );
                let opening = state.process_key(KeyEvent::pressed(Key::Tab, modifiers), settings);
                assert_eq!(
                    opening
                        .actions()
                        .next()
                        .map(|action| session.handle_input(action)),
                    Some(SwitcherEffect::Open {
                        selection_delta: Some(1),
                    })
                );
                session.open(
                    [
                        SwitchTask::new(1, 10, "Alpha", "editor"),
                        SwitchTask::new(2, 20, "Beta", "browser"),
                        SwitchTask::new(3, 30, "Gamma", "terminal"),
                    ],
                    Some(1),
                );
                let mut effects = Vec::new();
                for event in [
                    KeyEvent::released(Key::Tab, modifiers),
                    KeyEvent::pressed(key, modifiers),
                    KeyEvent::released(key, modifiers),
                    KeyEvent::released(Key::LeftAlt, modifiers),
                ] {
                    let outcome = state.process_key(event, settings);
                    assert!(outcome.suppress);
                    effects.extend(outcome.actions().map(|action| session.handle_input(action)));
                }
                assert_eq!(effects.last(), Some(&SwitcherEffect::Activate(target)));
                assert!(!session.is_visible());
            }
        }
    }

    #[test]
    fn modified_navigation_edits_a_nonempty_query_and_pins_the_panel() {
        let settings = HookSettings {
            search_active: true,
            filter_visible: true,
            ..HookSettings::default()
        };
        for (control, shift) in [(false, true), (true, false), (true, true)] {
            let modifiers = Modifiers {
                control,
                shift,
                ..Modifiers::default()
            };
            for key in [Key::LeftArrow, Key::RightArrow, Key::Home, Key::End] {
                let mut state = HookState::default();
                for event in [
                    KeyEvent::pressed(Key::LeftAlt, ALT),
                    KeyEvent::pressed(Key::Tab, ALT),
                ] {
                    let _outcome = state.process_key(event, settings);
                }
                let command =
                    TextCommand::for_key(u32::from(key.virtual_key()), control, shift, false)
                        .unwrap_or_else(|| {
                            panic!("navigation key has no editing command: {key:?}")
                        });
                assert_eq!(
                    state.process_key(KeyEvent::pressed(key, modifiers), settings),
                    HookOutcome::one(true, InputAction::EditSearch(command))
                );
                assert!(
                    state
                        .process_key(KeyEvent::released(key, modifiers), settings)
                        .suppress
                );
                assert!(
                    state
                        .process_key(
                            KeyEvent::released(Key::LeftAlt, Modifiers::default()),
                            settings
                        )
                        .actions()
                        .next()
                        .is_none()
                );
            }
        }
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

#[cfg(test)]
mod overlay_wheel_tests {
    use super::*;

    #[test]
    fn ordinary_wheel_is_captured_only_while_the_overlay_is_open() {
        let mut state = HookState::default();
        assert_eq!(state.process_overlay_wheel(-30), HookOutcome::default());
        state.set_overlay_active(true);
        assert_eq!(state.process_overlay_wheel(0), HookOutcome::default());
        let wheel = state.process_overlay_wheel(-30);
        assert!(
            wheel.suppress,
            "wheel input leaked to the background application"
        );
        assert_eq!(
            wheel.actions().collect::<Vec<_>>(),
            [InputAction::OverlayWheel(-30)]
        );
        state.set_interception_suspended(true);
        state.set_overlay_active(true);
        assert_eq!(state.process_overlay_wheel(-30), HookOutcome::default());
    }

    #[test]
    fn ordinary_overlay_wheel_preserves_alt_release_completion() {
        let mut state = HookState::default();
        let settings = HookSettings::default();
        let _open = state.process_key(
            KeyEvent::pressed(
                Key::Tab,
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
            ),
            settings,
        );
        state.set_overlay_active(true);
        assert_eq!(
            state.process_overlay_wheel(-120).actions().next(),
            Some(InputAction::OverlayWheel(-120))
        );
        let release =
            state.process_key(KeyEvent::released(Key::Alt, Modifiers::default()), settings);
        assert_eq!(release.actions().next(), Some(InputAction::AltReleased));
    }
}
