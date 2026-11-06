//! Reject synthetic authorization before observing state, replaying, or publishing actions.

use super::{process_with_context, replay::is_own_replayed_input};
use alttabio::hook_flags::OVERLAY_ACTIVE;
use alttabio::input::InputProvenance;
use windows::Win32::Foundation::{LPARAM, LRESULT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, KBDLLHOOKSTRUCT, LLKHF_INJECTED,
    LLKHF_LOWER_IL_INJECTED, LLMHF_INJECTED, LLMHF_LOWER_IL_INJECTED, MSLLHOOKSTRUCT,
};

pub(super) fn keyboard_provenance(data: &KBDLLHOOKSTRUCT) -> InputProvenance {
    InputProvenance::classify(
        data.flags.contains(LLKHF_INJECTED),
        data.flags.contains(LLKHF_LOWER_IL_INJECTED),
        is_own_replayed_input(data.dwExtraInfo),
    )
}

pub(super) fn mouse_provenance(data: &MSLLHOOKSTRUCT) -> InputProvenance {
    InputProvenance::classify(
        data.flags & LLMHF_INJECTED != 0,
        data.flags & LLMHF_LOWER_IL_INJECTED != 0,
        is_own_replayed_input(data.dwExtraInfo),
    )
}

pub(super) fn filter_keyboard_input(
    lparam: LPARAM,
    forward: impl FnOnce() -> LRESULT,
) -> Option<LRESULT> {
    let data = unsafe {
        // SAFETY: called only during a nonnegative low-level keyboard callback; the original
        // KBDLLHOOKSTRUCT is live until the callback returns.
        (lparam.0 as *const KBDLLHOOKSTRUCT).as_ref()
    }?;
    filter_input(keyboard_provenance(data), owns_input, forward)
}

pub(super) fn filter_mouse_input(
    lparam: LPARAM,
    forward: impl FnOnce() -> LRESULT,
) -> Option<LRESULT> {
    let data = unsafe {
        // SAFETY: called only during a nonnegative low-level mouse callback; the original
        // MSLLHOOKSTRUCT is live until the callback returns.
        (lparam.0 as *const MSLLHOOKSTRUCT).as_ref()
    }?;
    filter_input(mouse_provenance(data), owns_input, forward)
}

fn filter_input(
    provenance: InputProvenance,
    owns_input: impl FnOnce() -> bool,
    forward: impl FnOnce() -> LRESULT,
) -> Option<LRESULT> {
    if provenance != InputProvenance::Injected {
        return None;
    }
    // Bypass action publication entirely. Refused/deferred openings and post failures must
    // never turn a security rejection into delivery to the focused native window procedure.
    Some(if provenance.suppress_native_delivery(owns_input()) {
        LRESULT(1)
    } else {
        forward()
    })
}

fn owns_input() -> bool {
    if process_with_context(|context| context.flags.load() & OVERLAY_ACTIVE != 0).unwrap_or(true) {
        // Includes pending openings, even before the UI has taken focus.
        return true;
    }
    let mut process_id = 0;
    unsafe {
        // SAFETY: the foreground HWND is borrowed, and process_id is writable. These bounded
        // queries also cover native menus and modal windows while hook interception is suspended.
        let foreground = GetForegroundWindow();
        if !foreground.is_invalid()
            && GetWindowThreadProcessId(foreground, Some(&raw mut process_id)) == 0
        {
            // A failed ownership query cannot authorize synthetic delivery.
            return true;
        }
    }
    process_id == std::process::id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::replay::REPLAYED_INPUT_MARKER;
    use crate::hook::{
        CONTEXT, commit_keyboard_delivery, keyboard_proc, mouse_proc, process_keyboard_message,
        process_mouse_message_with, test_context,
    };
    use std::sync::atomic::Ordering;
    use windows::Win32::Foundation::WPARAM;
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_CAPITAL, VK_LMENU};
    use windows::Win32::UI::WindowsAndMessaging::{
        LLKHF_ALTDOWN, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
        WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP,
    };

    #[test]
    fn all_injected_keyboard_flags_are_rejected_before_state_and_native_dispatch() {
        for flags in [
            LLKHF_INJECTED,
            LLKHF_LOWER_IL_INJECTED,
            LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED,
        ] {
            for marker in [0, REPLAYED_INPUT_MARKER] {
                if marker != 0 && flags == LLKHF_INJECTED {
                    continue; // Same-integrity marked replay retains its existing routing.
                }
                let context = test_context();
                context.flags.sync_overlay(true, false, false);
                let before = context.flags.load();
                CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));
                for vk in [u32::from(VK_LMENU.0), 0x09, 0x77, u32::from(VK_CAPITAL.0)] {
                    let data = KBDLLHOOKSTRUCT {
                        vkCode: vk,
                        flags,
                        dwExtraInfo: marker,
                        ..Default::default()
                    };
                    let param = LPARAM((&raw const data) as isize);
                    for message in [WM_KEYDOWN, WM_KEYUP] {
                        assert_eq!(
                            process_keyboard_message(WPARAM(message as usize), param),
                            None
                        );
                        // SAFETY: the callback receives a live, correctly typed stack record.
                        assert_eq!(
                            unsafe { keyboard_proc(0, WPARAM(message as usize), param) },
                            LRESULT(1)
                        );
                        commit_keyboard_delivery(WPARAM(message as usize), param);
                    }
                }
                CONTEXT.with(|slot| {
                    let mut slot = slot.borrow_mut();
                    let Some(context) = slot.take() else {
                        panic!("missing context")
                    };
                    assert_eq!(context.flags.load(), before);
                    assert_eq!(context.keyboard_state.key_state(VK_LMENU), 0);
                    assert_eq!(context.keyboard_state.key_state(VK_CAPITAL), 0);
                    assert_eq!(context.pending_errors, 0);
                });
            }
        }
    }

    #[test]
    fn all_injected_mouse_messages_leave_physical_ownership_intact() {
        for flags in [
            LLMHF_INJECTED,
            LLMHF_LOWER_IL_INJECTED,
            LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED,
        ] {
            let mut context = test_context();
            context.settings.right_button_wheel_switching = true;
            let _down = context.state.process_mouse(
                alttabio::input::MouseEvent::RightButtonPressed,
                context.settings,
            );
            context.right_button_release.store(true, Ordering::Release);
            context.flags.sync_overlay(true, false, false);
            CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));
            for message in [
                WM_RBUTTONDOWN,
                WM_RBUTTONUP,
                WM_LBUTTONDOWN,
                WM_LBUTTONUP,
                WM_MOUSEWHEEL,
                WM_MOUSEMOVE,
            ] {
                let data = MSLLHOOKSTRUCT {
                    flags,
                    mouseData: 120 << 16,
                    ..Default::default()
                };
                let param = LPARAM((&raw const data) as isize);
                assert_eq!(
                    process_mouse_message_with(WPARAM(message as usize), param, || panic!(
                        "injected replay"
                    )),
                    None
                );
                // SAFETY: the callback receives a live, correctly typed stack record.
                assert_eq!(
                    unsafe { mouse_proc(0, WPARAM(message as usize), param) },
                    LRESULT(1)
                );
            }
            CONTEXT.with(|slot| {
                let mut slot = slot.borrow_mut();
                let Some(mut context) = slot.take() else {
                    panic!("missing context")
                };
                assert!(context.state.owns_right_button());
                assert!(context.right_button_release.load(Ordering::Acquire));
                assert!(!context.state.take_replayed_right_click());
                assert_eq!(context.pending_errors, 0);
            });
        }
    }

    #[test]
    fn physical_tab_cannot_use_injected_alt_context() {
        CONTEXT.with(|slot| *slot.borrow_mut() = Some(test_context()));
        let data = KBDLLHOOKSTRUCT {
            vkCode: 0x09,
            flags: LLKHF_ALTDOWN,
            ..Default::default()
        };
        assert_eq!(
            process_keyboard_message(
                WPARAM(WM_KEYDOWN as usize),
                LPARAM((&raw const data) as isize)
            ),
            Some(alttabio::input::HookOutcome::default())
        );
        CONTEXT.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn physical_shortcuts_still_open_and_request_window_commands() {
        CONTEXT.with(|slot| *slot.borrow_mut() = Some(test_context()));
        let press = |vk_code| {
            let data = KBDLLHOOKSTRUCT {
                vkCode: vk_code,
                ..Default::default()
            };
            process_keyboard_message(
                WPARAM(WM_KEYDOWN as usize),
                LPARAM((&raw const data) as isize),
            )
        };
        assert!(press(u32::from(VK_LMENU.0)).is_some_and(|outcome| outcome.suppress));
        assert_eq!(
            press(0x09).map(|outcome| outcome.actions().collect::<Vec<_>>()),
            Some(vec![alttabio::input::InputAction::Switch(1)])
        );
        let _visible =
            process_with_context(|context| context.flags.sync_overlay(true, false, false));
        assert_eq!(
            press(0x77).map(|outcome| outcome.actions().collect::<Vec<_>>()),
            Some(vec![alttabio::input::InputAction::WindowCommand(
                alttabio::window_command::WindowCommand::Terminate
            )])
        );
        CONTEXT.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn mouse_replay_marker_does_not_override_lower_integrity_flags() {
        for flags in [
            LLMHF_INJECTED,
            LLMHF_LOWER_IL_INJECTED,
            LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED,
        ] {
            let data = MSLLHOOKSTRUCT {
                flags,
                dwExtraInfo: REPLAYED_INPUT_MARKER,
                ..Default::default()
            };
            assert_eq!(
                mouse_provenance(&data),
                if flags == LLMHF_INJECTED {
                    InputProvenance::InternalReplay
                } else {
                    InputProvenance::Injected
                }
            );
        }
    }

    #[test]
    fn synthetic_background_input_passes_without_hook_authority() {
        assert_eq!(
            filter_input(InputProvenance::Injected, || false, || LRESULT(42)),
            Some(LRESULT(42))
        );
        assert_eq!(
            filter_input(
                InputProvenance::Injected,
                || true,
                || panic!("native dispatch")
            ),
            Some(LRESULT(1))
        );
        for source in [InputProvenance::Physical, InputProvenance::InternalReplay] {
            assert_eq!(
                filter_input(
                    source,
                    || panic!("unneeded query"),
                    || panic!("unneeded forward")
                ),
                None
            );
        }
    }
}
