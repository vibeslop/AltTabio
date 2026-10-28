//! Posts fully intercepted switcher gestures to the UI thread in input order.

use super::{CONTEXT, HOOK_ERROR_POST_ACTION, HookContext};
use crate::app_messages::WM_HOOK_ACTION;
use alttabio::hook_delivery::{Delivery, deliver};
use alttabio::hook_flags::encode_action;
use alttabio::input::{HookOutcome, InputAction};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub(super) fn post_actions(outcome: HookOutcome) -> bool {
    CONTEXT
        .try_with(|context| {
            let mut context = context.try_borrow_mut().ok()?;
            let context = context.as_mut()?;
            let generation = context.interception_generation;
            Some(post_context_actions(context, outcome, |target, action| {
                let (wparam, lparam) = encode_action(action, generation);
                // SAFETY: target is the UI HWND supplied at hook creation. PostMessageW copies
                // the scalar payloads and retains no Rust references.
                unsafe {
                    PostMessageW(Some(target), WM_HOOK_ACTION, WPARAM(wparam), LPARAM(lparam))
                }
                .is_ok()
            }))
        })
        .ok()
        .flatten()
        .unwrap_or(false)
}

fn post_context_actions(
    context: &mut HookContext,
    outcome: HookOutcome,
    mut post: impl FnMut(HWND, InputAction) -> bool,
) -> bool {
    let target = context.target;
    match deliver(
        &context.flags,
        context.interception_generation,
        context.settings.typed_search,
        outcome,
        |action| post(target, action),
    ) {
        Delivery::Posted => true,
        // Keep release ownership even when a modal boundary races with this callback.
        #[allow(
            clippy::match_same_arms,
            reason = "the comment above applies only to a stale outcome"
        )]
        Delivery::Stale => true,
        Delivery::OverlayRefused => {
            context.state.reset_gestures();
            true
        }
        Delivery::PostFailed => {
            context.record_error(HOOK_ERROR_POST_ACTION);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::keyboard_state::KeyboardState;
    use crate::hook::{
        finish_callback_with, process_keyboard_message, process_with_context, reset_context,
        test_context,
    };
    use alttabio::hook_flags::{HookFlags, OVERLAY_ACTIVE, OVERLAY_FLAGS, SEARCH_ACTIVE};
    use alttabio::input::{HookSettings, HookState, Key, KeyEvent, Modifiers};
    use std::sync::{Arc, atomic::AtomicBool};
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_LMENU, VK_RMENU, VK_TAB};
    use windows::Win32::UI::WindowsAndMessaging::{KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_KEYUP};

    #[test]
    fn opening_switcher_never_forwards_tab_to_the_previous_application() {
        for alt in [VK_LMENU, VK_RMENU] {
            use windows::Win32::System::Threading::GetCurrentThreadId;
            use windows::Win32::UI::WindowsAndMessaging::{MSG, PM_NOREMOVE, PeekMessageW};
            let mut message = MSG::default();
            // SAFETY: the initialized output belongs to this test. Create the hook thread's
            // real queue so a reintroduced hotkey route cannot fail open and mask this bug.
            let _queue_ready = unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_NOREMOVE) };
            let mut context = test_context();
            // SAFETY: this scalar query has no preconditions or ownership transfer.
            context.hook_thread_id = unsafe { GetCurrentThreadId() };
            // An opaque, never-dereferenced target for the mocked PostMessage boundary below.
            context.target = HWND(std::ptr::dangling_mut());
            CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));
            let mut forwarded = Vec::new();
            let mut actions = Vec::new();
            for (key, message) in [
                (alt, WM_KEYDOWN),
                (VK_TAB, WM_KEYDOWN),
                (VK_TAB, WM_KEYDOWN),
                (VK_TAB, WM_KEYUP),
                (VK_TAB, WM_KEYDOWN),
                (VK_TAB, WM_KEYUP),
                (alt, WM_KEYUP),
            ] {
                let data = KBDLLHOOKSTRUCT {
                    vkCode: u32::from(key.0),
                    ..Default::default()
                };
                let outcome = process_keyboard_message(
                    WPARAM(message as usize),
                    LPARAM((&raw const data) as isize),
                );
                let result = finish_callback_with(
                    outcome,
                    |outcome| {
                        process_with_context(|context| {
                            post_context_actions(context, outcome, |_, action| {
                                actions.push(action);
                                true
                            })
                        })
                        .unwrap_or(false)
                    },
                    reset_context,
                    || {
                        forwarded.push((key, message));
                        LRESULT(0)
                    },
                );
                assert_eq!(
                    result,
                    LRESULT(1),
                    "the previous application received {key:?}, message {message}"
                );
            }
            assert!(
                forwarded.is_empty(),
                "the background game received switcher keys: {forwarded:?}"
            );
            assert_eq!(
                actions,
                [
                    InputAction::Switch(1),
                    InputAction::Switch(1),
                    InputAction::Switch(1),
                    InputAction::AltReleased
                ]
            );
            CONTEXT.with(|slot| *slot.borrow_mut() = None);
        }
    }

    #[test]
    fn modal_boundaries_prevent_stale_flag_publication_and_preserve_restored_flags() {
        let mut context = test_context();
        let flags = Arc::clone(&context.flags);
        let outcome = context.state.process_key(
            KeyEvent::pressed(
                Key::Tab,
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
            ),
            context.settings,
        );
        flags.suspend(true);
        assert!(!flags.update_overlay(0, true, true));
        assert!(post_context_actions(&mut context, outcome, |_, _| panic!(
            "suspended action was posted"
        )));
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
        flags.set(OVERLAY_ACTIVE, true);
        flags.set(SEARCH_ACTIVE, true);
        flags.suspend(false);
        assert_eq!(flags.load() & OVERLAY_FLAGS, OVERLAY_FLAGS);
        // The entire modal lifetime occurred between callbacks; its generation still cancels Tab.
        assert_eq!(context.sync_interception(), OVERLAY_FLAGS);
        let release = context.state.process_key(
            KeyEvent::released(Key::Alt, Modifiers::default()),
            context.settings,
        );
        assert!(release.actions().next().is_none());
        assert!(!flags.update_overlay(0, false, false));
        assert_eq!(flags.load() & OVERLAY_FLAGS, OVERLAY_FLAGS);
    }

    #[test]
    fn posting_overlay_open_arms_typed_search_before_the_ui_acknowledges_visibility() {
        let flags = Arc::new(HookFlags::default());
        let mut context = HookContext {
            target: HWND::default(),
            state: HookState::default(),
            settings: HookSettings::default(),
            remote_desktop_passthrough: Arc::new(AtomicBool::new(false)),
            target_thread_id: 0,
            hook_thread_id: 0,
            pending_errors: 0,
            flags: Arc::clone(&flags),
            interception_generation: 0,
            keyboard_state: KeyboardState::default(),
            recovering: false,
        };
        let outcome = context.state.process_key(
            KeyEvent::pressed(
                Key::Tab,
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
            ),
            context.settings,
        );

        assert!(post_context_actions(&mut context, outcome, |_, _| true));
        assert_eq!(flags.load() & OVERLAY_FLAGS, OVERLAY_FLAGS);
    }
}
