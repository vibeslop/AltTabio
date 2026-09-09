//! Temporary registration for an actual Tab press already owned by the switcher.

use alttabio::input::HookOutcome;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_TAB,
};
use windows::core::{Error, HRESULT};

static NEXT_ID: AtomicU32 = AtomicU32::new(0);
static CLEANUP_ERROR: AtomicI32 = AtomicI32::new(0);
static REGISTRATION_ERROR: AtomicI32 = AtomicI32::new(0);
const FIRST_ID: u32 = 0x6000;
const ID_COUNT: u32 = 0x5000;

pub fn owns_id(id: usize) -> bool {
    (0x6000..0xB000).contains(&id)
}

pub fn take_cleanup_error() -> Option<Error> {
    let code = CLEANUP_ERROR.swap(0, Ordering::AcqRel);
    (code != 0).then(|| Error::from_hresult(HRESULT(code)))
}

pub fn take_registration_error() -> Option<Error> {
    let code = REGISTRATION_ERROR.swap(0, Ordering::AcqRel);
    (code != 0).then(|| Error::from_hresult(HRESULT(code)))
}

#[derive(Default)]
pub struct ActionBuffer {
    outcomes: [Option<HookOutcome>; 16],
    len: usize,
}

impl ActionBuffer {
    pub fn push(&mut self, outcome: HookOutcome) -> bool {
        if outcome.actions().next().is_none() {
            return true;
        }
        let Some(slot) = self.outcomes.get_mut(self.len) else {
            return false;
        };
        *slot = Some(outcome);
        self.len += 1;
        true
    }

    pub fn take(&mut self) -> impl Iterator<Item = HookOutcome> + use<> {
        self.len = 0;
        std::mem::take(&mut self.outcomes).into_iter().flatten()
    }
}

pub struct PendingSwitch {
    id: i32,
    pub generation: usize,
    pub actions: ActionBuffer,
    started: Instant,
}

impl PendingSwitch {
    #[cfg(test)]
    pub fn without_registration(generation: usize) -> Self {
        Self {
            id: -1,
            generation,
            actions: ActionBuffer::default(),
            started: Instant::now(),
        }
    }

    pub fn register(generation: usize) -> Result<Self, Error> {
        let id = i32::try_from(FIRST_ID + NEXT_ID.fetch_add(1, Ordering::Relaxed) % ID_COUNT)
            .map_err(|_| Error::from_hresult(HRESULT(0x8007_0057_u32.cast_signed())))?;
        let mut modifiers = HOT_KEY_MODIFIERS::default();
        for (key, modifier) in [
            (VK_MENU, MOD_ALT),
            (VK_CONTROL, MOD_CONTROL),
            (VK_SHIFT, MOD_SHIFT),
            (VK_LWIN, MOD_WIN),
            (VK_RWIN, MOD_WIN),
        ] {
            // SAFETY: the adapter samples system modifier state without changing any held key.
            if unsafe { GetAsyncKeyState(i32::from(key.0)) } < 0 {
                modifiers |= modifier;
            }
        }
        // SAFETY: None registers on this hook-owning thread, whose message queue already exists.
        // This bounded USER call runs before forwarding the physical Tab to hotkey processing.
        unsafe { RegisterHotKey(None, id, modifiers, u32::from(VK_TAB.0)) }
            .inspect_err(|error| REGISTRATION_ERROR.store(error.code().0, Ordering::Release))?;
        Ok(Self {
            id,
            generation,
            actions: ActionBuffer::default(),
            started: Instant::now(),
        })
    }

    pub fn matches_id(&self, id: usize) -> bool {
        usize::try_from(self.id) == Ok(id)
    }

    pub fn expired(&self) -> bool {
        self.started.elapsed() >= Duration::from_millis(200)
    }
}

impl Drop for PendingSwitch {
    fn drop(&mut self) {
        #[cfg(test)]
        if self.id == -1 {
            return;
        }
        // SAFETY: the registration is owned and released once on the hook thread. Cleanup can
        // occur in a callback, so errors are published for the message loop, never logged here.
        if let Err(error) = unsafe { UnregisterHotKey(None, self.id) } {
            CLEANUP_ERROR.store(error.code().0, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alttabio::input::{HookSettings, HookState, InputAction, Key, KeyEvent, Modifiers};

    #[test]
    fn release_before_hotkey_dispatch_stays_after_the_open_and_cycle_actions() {
        let mut state = HookState::default();
        let settings = HookSettings::default();
        let modifiers = Modifiers::default();
        let _ = state.process_key(KeyEvent::pressed(Key::LeftAlt, modifiers), settings);
        let mut buffer = ActionBuffer::default();
        for event in [
            KeyEvent::pressed(Key::Tab, modifiers),
            KeyEvent::released(Key::Tab, modifiers),
            KeyEvent::pressed(Key::Tab, modifiers),
            KeyEvent::released(Key::LeftAlt, modifiers),
        ] {
            assert!(buffer.push(state.process_key(event, settings)));
        }
        let actions: Vec<_> = buffer
            .take()
            .flat_map(|outcome| outcome.actions().collect::<Vec<_>>())
            .collect();
        assert_eq!(
            actions,
            [
                InputAction::Switch(1),
                InputAction::Switch(1),
                InputAction::AltReleased
            ]
        );
        assert!(buffer.take().next().is_none());
    }

    #[test]
    fn action_buffer_has_a_fixed_capacity_and_does_not_overwrite_earlier_input() {
        let mut state = HookState::default();
        let outcome = state.process_key(
            KeyEvent::pressed(
                Key::Tab,
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
            ),
            HookSettings::default(),
        );
        let mut buffer = ActionBuffer::default();
        for _ in 0..16 {
            assert!(buffer.push(outcome));
        }
        assert!(!buffer.push(outcome));
        assert_eq!(buffer.take().count(), 16);
    }
}
