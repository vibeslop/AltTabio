//! Replays suppressed input through `SendInput`, tagged so the hook lets it pass.

use super::key_pressed;
use alttabio::input::{HookOutcome, Key, KeyTransition, ReplayedKeyEvent, ReplayedMouseEvent};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MOUSEEVENTF_RIGHTUP, MOUSEINPUT, SendInput,
    VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN,
};

pub(super) const REPLAYED_INPUT_MARKER: usize = 0x0A17_AB10;

fn replayed_key_event_to_input(event: ReplayedKeyEvent) -> INPUT {
    let virtual_key = event.virtual_key();
    let mut flags = if is_extended_virtual_key(virtual_key) {
        KEYEVENTF_EXTENDEDKEY
    } else {
        KEYBD_EVENT_FLAGS::default()
    };
    if event.transition == KeyTransition::Released {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(virtual_key),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: REPLAYED_INPUT_MARKER,
            },
        },
    }
}

fn replayed_mouse_event_to_input(event: ReplayedMouseEvent) -> INPUT {
    let flags = match event {
        ReplayedMouseEvent::RightButtonReleased => MOUSEEVENTF_RIGHTUP,
    };
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: REPLAYED_INPUT_MARKER,
            },
        },
    }
}

pub(super) fn replay_key_events(
    events: [Option<ReplayedKeyEvent>; 3],
    outcome: &mut HookOutcome,
) -> bool {
    replay_key_events_with(events, outcome, |inputs, input_size| unsafe {
        // SAFETY: `replay_key_events_with` passes only its initialized INPUT prefix, and
        // SendInput copies the records synchronously without retaining the borrowed slice.
        SendInput(inputs, input_size)
    })
}

/// Called on the UI thread before opening the overlay, never from an input callback.
pub fn send_shell_escape() -> Result<(), String> {
    send_shell_escape_with(
        |key| key_pressed(key.0),
        |inputs, input_size| unsafe {
            // SAFETY: the replay helper supplies initialized INPUT records and SendInput copies
            // them synchronously without retaining the borrowed slice.
            SendInput(inputs, input_size)
        },
    )
}

fn send_shell_escape_with(
    mut is_down: impl FnMut(VIRTUAL_KEY) -> bool,
    sender: impl FnMut(&[INPUT], i32) -> u32,
) -> Result<(), String> {
    // Do not release a modifier forwarded to another application or send its Escape shortcut.
    if [VK_MENU, VK_CONTROL, VK_LWIN, VK_RWIN]
        .into_iter()
        .any(&mut is_down)
    {
        return Err("Cannot dismiss Start while a system modifier is forwarded".to_owned());
    }
    let events = [
        // Suppressing Alt can leave LLKHF_ALTDOWN set even while async state reports up.
        // Clear that context on both sides before Escape. Our tagged releases bypass physical
        // gesture tracking, so held-Alt cycling and the eventual real release remain intact.
        Some(ReplayedKeyEvent::released(Key::LeftAlt)),
        Some(ReplayedKeyEvent::released(Key::RightAlt)),
        Some(ReplayedKeyEvent::pressed(Key::Escape)),
        Some(ReplayedKeyEvent::released(Key::Escape)),
    ];
    if replay_key_events_with(events, &mut HookOutcome::default(), sender) {
        Ok(())
    } else {
        Err("Could not send Escape to the Start/Search menu".to_owned())
    }
}

fn replay_key_events_with<const N: usize>(
    events: [Option<ReplayedKeyEvent>; N],
    outcome: &mut HookOutcome,
    mut sender: impl FnMut(&[INPUT], i32) -> u32,
) -> bool {
    let mut inputs = [INPUT::default(); N];
    let mut input_count = 0;
    for event in events.into_iter().flatten() {
        let Some(slot) = inputs.get_mut(input_count) else {
            return false;
        };
        *slot = replayed_key_event_to_input(event);
        input_count += 1;
    }
    if input_count == 0 {
        return true;
    }

    let Ok(input_size) = i32::try_from(core::mem::size_of::<INPUT>()) else {
        return false;
    };
    let Some(inputs) = inputs.get(..input_count) else {
        return false;
    };
    let mut sent = 0;
    while sent < inputs.len() {
        let remaining = &inputs[sent..];
        let Some(inserted) = usize::try_from(sender(remaining, input_size)).ok() else {
            outcome.suppress = false;
            return false;
        };
        if inserted == 0 || inserted > remaining.len() {
            outcome.suppress = false;
            return false;
        }
        sent += inserted;
    }
    true
}

pub(super) fn replay_mouse_event(
    event: Option<ReplayedMouseEvent>,
    outcome: &mut HookOutcome,
) -> bool {
    replay_mouse_event_with(event, outcome, |inputs, input_size| unsafe {
        // SAFETY: `replay_mouse_event_with` passes only initialized INPUT records, and
        // SendInput copies the records synchronously without retaining the borrowed slice.
        SendInput(inputs, input_size)
    })
}

fn replay_mouse_event_with(
    event: Option<ReplayedMouseEvent>,
    outcome: &mut HookOutcome,
    sender: impl FnOnce(&[INPUT], i32) -> u32,
) -> bool {
    let Some(event) = event else {
        return true;
    };
    let inputs = [replayed_mouse_event_to_input(event)];
    let Ok(input_size) = i32::try_from(core::mem::size_of::<INPUT>()) else {
        outcome.suppress = false;
        return false;
    };
    if sender(&inputs, input_size) != 1 {
        outcome.suppress = false;
        return false;
    }
    true
}

pub(super) const fn is_own_replayed_input(extra_info: usize) -> bool {
    extra_info == REPLAYED_INPUT_MARKER
}

const fn is_extended_virtual_key(virtual_key: u16) -> bool {
    matches!(
        virtual_key,
        0x21..=0x28 | 0x2D..=0x2E | 0x5B..=0x5D | 0x6F | 0x90 | 0xA3 | 0xA5
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::{CONTEXT, process_keyboard_message, test_context};
    use alttabio::input::{InputAction, KeyEvent, Modifiers};
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_LMENU, VK_RMENU};
    use windows::Win32::UI::WindowsAndMessaging::{KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_KEYUP};

    #[test]
    fn shell_escape_clears_suppressed_alt_context_before_escape() {
        for held_alt in [VK_LMENU, VK_RMENU] {
            // Captured failure: GetAsyncKeyState permitted dismissal, but both injected Escape
            // events carried LLKHF_ALTDOWN and Start retained focus until the 500 ms timeout.
            let mut alt_context = true;
            let mut escapes = Vec::new();
            assert!(
                send_shell_escape_with(
                    |_| false,
                    |inputs, _| {
                        for input in inputs {
                            assert_eq!(input.r#type, INPUT_KEYBOARD);
                            // SAFETY: the replay helper initializes the keyboard union member.
                            let keyboard = unsafe { input.Anonymous.ki };
                            assert_eq!(keyboard.dwExtraInfo, REPLAYED_INPUT_MARKER);
                            if keyboard.wVk == held_alt
                                && keyboard.dwFlags.contains(KEYEVENTF_KEYUP)
                            {
                                alt_context = false;
                            }
                            if keyboard.wVk == VK_ESCAPE {
                                escapes.push((
                                    keyboard.dwFlags.contains(KEYEVENTF_KEYUP),
                                    alt_context,
                                ));
                            }
                        }
                        u32::try_from(inputs.len()).unwrap_or_default()
                    },
                )
                .is_ok()
            );
            assert_eq!(
                escapes,
                [(false, false), (true, false)],
                "Start must receive plain Escape, even when suppressed Alt is absent from async state"
            );
        }
    }

    #[test]
    fn shell_escape_preserves_forwarded_system_modifiers() {
        for held in [VK_MENU, VK_CONTROL, VK_LWIN, VK_RWIN] {
            assert!(
                send_shell_escape_with(
                    |key| key == held,
                    |_, _| panic!("A forwarded modifier must prevent all injected input"),
                )
                .is_err()
            );
        }
    }

    #[test]
    fn shell_escape_keeps_the_next_gesture_after_start_interrupts_alt_tab() {
        for alt in [Key::LeftAlt, Key::RightAlt] {
            let mut context = test_context();
            let modifiers = Modifiers::default();
            for event in [
                KeyEvent::pressed(alt, modifiers),
                KeyEvent::pressed(Key::Tab, modifiers),
                KeyEvent::released(Key::Tab, modifiers),
                KeyEvent::pressed(Key::LeftWindows, modifiers),
                KeyEvent::released(Key::LeftWindows, modifiers),
                KeyEvent::pressed(Key::Tab, modifiers),
                KeyEvent::released(Key::Tab, modifiers),
                KeyEvent::released(alt, modifiers),
                KeyEvent::pressed(alt, modifiers),
                KeyEvent::pressed(Key::Tab, modifiers),
                KeyEvent::released(Key::Tab, modifiers),
            ] {
                let _outcome = context.state.process_key(event, context.settings);
            }
            CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));
            let result = send_shell_escape_with(
                |_| false,
                |inputs, _| {
                    for input in inputs {
                        // SAFETY: send_shell_escape_with only constructs keyboard INPUT records.
                        let keyboard = unsafe { input.Anonymous.ki };
                        let data = KBDLLHOOKSTRUCT {
                            vkCode: u32::from(keyboard.wVk.0),
                            dwExtraInfo: keyboard.dwExtraInfo,
                            ..KBDLLHOOKSTRUCT::default()
                        };
                        let message = if keyboard.dwFlags.contains(KEYEVENTF_KEYUP) {
                            WM_KEYUP
                        } else {
                            WM_KEYDOWN
                        };
                        // Pass every injected release and Escape through the real adapter.
                        assert_eq!(
                            process_keyboard_message(
                                WPARAM(message as usize),
                                LPARAM((&raw const data) as isize)
                            ),
                            None
                        );
                    }
                    u32::try_from(inputs.len()).unwrap_or_default()
                },
            );
            let Some(mut context) = CONTEXT.with(|slot| slot.borrow_mut().take()) else {
                panic!("test context disappeared");
            };
            assert!(result.is_ok());
            for delta in [1, -1] {
                if delta == -1 {
                    let _shift = context.state.process_key(
                        KeyEvent::pressed(Key::LeftShift, modifiers),
                        context.settings,
                    );
                }
                let outcome = context
                    .state
                    .process_key(KeyEvent::pressed(Key::Tab, modifiers), context.settings);
                assert_eq!(outcome.actions().next(), Some(InputAction::Switch(delta)));
                assert!(outcome.suppress);
                let _release = context
                    .state
                    .process_key(KeyEvent::released(Key::Tab, modifiers), context.settings);
            }
            let release = context
                .state
                .process_key(KeyEvent::released(alt, modifiers), context.settings);
            assert!(release.suppress);
            assert_eq!(release.actions().next(), Some(InputAction::AltReleased));
        }
    }

    #[test]
    fn replayed_windows_events_are_tagged_extended_keyboard_input() {
        for (event, expected_key_up) in [
            (ReplayedKeyEvent::pressed(Key::LeftWindows), false),
            (ReplayedKeyEvent::released(Key::RightWindows), true),
        ] {
            let input = replayed_key_event_to_input(event);
            assert_eq!(input.r#type, INPUT_KEYBOARD);
            let keyboard = unsafe {
                // SAFETY: `replayed_key_event_to_input` initializes the keyboard union member and
                // marks the enclosing INPUT as INPUT_KEYBOARD.
                input.Anonymous.ki
            };
            assert_eq!(keyboard.wVk.0, event.virtual_key());
            assert!(keyboard.dwFlags.contains(KEYEVENTF_EXTENDEDKEY));
            assert_eq!(keyboard.dwFlags.contains(KEYEVENTF_KEYUP), expected_key_up);
            assert_eq!(keyboard.dwExtraInfo, REPLAYED_INPUT_MARKER);
        }
    }

    #[test]
    fn replayed_right_button_release_is_tagged_mouse_input() {
        let input = replayed_mouse_event_to_input(ReplayedMouseEvent::RightButtonReleased);
        assert_eq!(input.r#type, INPUT_MOUSE);
        let mouse = unsafe {
            // SAFETY: `replayed_mouse_event_to_input` initializes the mouse union member and marks
            // the enclosing INPUT as INPUT_MOUSE.
            input.Anonymous.mi
        };
        assert_eq!(mouse.dwFlags, MOUSEEVENTF_RIGHTUP);
        assert_eq!(mouse.dwExtraInfo, REPLAYED_INPUT_MARKER);
    }

    #[test]
    fn failed_right_button_release_replay_forwards_the_wheel_and_resets_suppression() {
        let mut outcome = HookOutcome::default();
        outcome.suppress = true;
        let replayed = replay_mouse_event_with(
            Some(ReplayedMouseEvent::RightButtonReleased),
            &mut outcome,
            |inputs, input_size| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(
                    input_size,
                    i32::try_from(core::mem::size_of::<INPUT>()).unwrap_or_default()
                );
                0
            },
        );

        assert!(!replayed);
        assert!(!outcome.suppress);
    }

    #[test]
    fn only_alttabios_exact_replay_marker_bypasses_hook_processing() {
        assert!(is_own_replayed_input(REPLAYED_INPUT_MARKER));
        assert!(!is_own_replayed_input(0));
        assert!(!is_own_replayed_input(REPLAYED_INPUT_MARKER + 1));
    }

    #[test]
    fn replay_retries_partial_send_input_and_releases_the_physical_fallback_on_failure() {
        let events = [
            Some(ReplayedKeyEvent::pressed(Key::LeftWindows)),
            Some(ReplayedKeyEvent::pressed(Key::Other(u16::from(b'R')))),
            None,
        ];
        let mut outcome = HookOutcome::default();
        outcome.suppress = true;
        let mut calls = 0;
        let replayed = replay_key_events_with(events, &mut outcome, |inputs, input_size| {
            calls += 1;
            assert_eq!(
                input_size,
                i32::try_from(core::mem::size_of::<INPUT>()).unwrap_or_default()
            );
            assert_eq!(inputs.len(), 3_usize.saturating_sub(calls));
            1
        });
        assert!(replayed);
        assert!(outcome.suppress);
        assert_eq!(calls, 2);

        let mut outcome = HookOutcome::default();
        outcome.suppress = true;
        let mut calls = 0;
        let replayed = replay_key_events_with(events, &mut outcome, |_, _| {
            calls += 1;
            u32::from(calls == 1)
        });
        assert!(!replayed);
        assert!(!outcome.suppress);
        assert_eq!(calls, 2);
    }
}
