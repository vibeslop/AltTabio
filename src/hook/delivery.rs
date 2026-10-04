//! Posts hook actions to the UI thread. While another window owns the foreground, a switch's
//! actions wait for the physical Tab to arrive as a registered hotkey, which carries the
//! permission to take the foreground.

use super::{
    CONTEXT, HOOK_ERROR_POST_ACTION, HOOK_ERROR_REGISTERED_SWITCH, HookContext, WM_HOOK_ACTION,
    WM_HOOK_HOTKEY_ACTION, decode_virtual_key, process_with_context,
};
use alttabio::hook_delivery::{Delivery, deliver, routes_tab_through_hotkey};
use alttabio::hook_flags::encode_action;
use alttabio::input::{HookOutcome, InputAction, Key, KeyTransition};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, GetForegroundWindow, KBDLLHOOKSTRUCT, LLKHF_INJECTED, PostMessageW,
};
use windows::core::Error;

pub(super) fn post_actions(outcome: HookOutcome) -> bool {
    CONTEXT
        .try_with(|context| {
            let mut context = context.try_borrow_mut().ok()?;
            let context = context.as_mut()?;
            let generation = context.interception_generation;
            Some(post_context_actions(context, outcome, |target, action| {
                post_action(target, action, generation)
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
    if !context
        .flags
        .generation_is_current(context.interception_generation)
    {
        context.registered_switch = None;
        return true;
    }
    if outcome
        .actions()
        .any(|action| action == InputAction::DismissOverlay)
    {
        context.registered_switch = None;
    }
    if let Some(pending) = &mut context.registered_switch {
        if !pending.actions.push(outcome) {
            context.registered_switch = None;
            context.state.reset_gestures();
            let _cleared =
                context
                    .flags
                    .update_overlay(context.interception_generation, false, false);
            if !post(context.target, InputAction::DismissOverlay) {
                context.record_error(HOOK_ERROR_POST_ACTION);
            }
            context.record_error(HOOK_ERROR_REGISTERED_SWITCH);
        }
        return true;
    }
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

fn post_action(target: HWND, action: InputAction, generation: usize) -> bool {
    post_action_message(target, action, generation, WM_HOOK_ACTION)
}

pub(super) fn route_registered_switch(
    context: &mut HookContext,
    outcome: HookOutcome,
    data: &KBDLLHOOKSTRUCT,
    transition: KeyTransition,
) -> HookOutcome {
    route_registered_switch_with(
        context,
        outcome,
        data,
        transition,
        // SAFETY: this bounded metadata query neither sends window messages nor enumerates.
        || unsafe { GetForegroundWindow() },
        crate::switch_hotkey::PendingSwitch::register,
    )
}

fn route_registered_switch_with(
    context: &mut HookContext,
    mut outcome: HookOutcome,
    data: &KBDLLHOOKSTRUCT,
    transition: KeyTransition,
    foreground: impl FnOnce() -> HWND,
    register: impl FnOnce(usize) -> Result<crate::switch_hotkey::PendingSwitch, Error>,
) -> HookOutcome {
    let key = decode_virtual_key(data.vkCode);
    if key == Key::Tab && transition == KeyTransition::Released && context.registered_tab_down {
        // Windows received this physical down as a hotkey. Balance it even if the pure
        // switcher would normally own the release of an intercepted Tab.
        context.registered_tab_down = false;
        outcome.suppress = false;
    }
    if context.registered_switch.is_some()
        || !context
            .flags
            .generation_is_current(context.interception_generation)
    {
        return outcome;
    }
    if !routes_tab_through_hotkey(key, transition, data.flags.contains(LLKHF_INJECTED), outcome)
        // Intercepting Tab is not input delivered to our UI. Receive the physical hotkey
        // whenever another window owns foreground, including ordinary applications.
        || foreground() == context.target
    {
        return outcome;
    }
    if let Ok(mut pending) = register(context.interception_generation) {
        if !context.flags.update_overlay(
            context.interception_generation,
            true,
            context.settings.typed_search,
        ) {
            return outcome;
        }
        if !pending.actions.push(outcome) {
            return outcome;
        }
        context.registered_switch = Some(pending);
        context.registered_tab_down = true;
        HookOutcome::default()
    } else {
        context.record_error(HOOK_ERROR_REGISTERED_SWITCH);
        outcome
    }
}

pub(super) fn dispatch_registered_switch(hotkey_id: Option<usize>) {
    let pending = process_with_context(|context| {
        let ready = context.registered_switch.as_ref().is_some_and(|pending| {
            hotkey_id.map_or_else(|| pending.expired(), |id| pending.matches_id(id))
        });
        if ready {
            context.registered_switch.take()
        } else {
            None
        }
    })
    .flatten();
    let Some(mut pending) = pending else {
        return;
    };
    let generation = pending.generation;
    let native = hotkey_id.is_some();
    if native {
        // SAFETY: this thread received the physical registered hotkey. Explicitly share its
        // activation permission with our process before notifying the separate UI thread.
        let granted = unsafe { AllowSetForegroundWindow(GetCurrentProcessId()) };
        if let Err(error) = granted {
            eprintln!("Could not share physical hotkey activation permission: {error}");
        }
    }
    let outcomes = pending.actions.take();
    drop(pending);
    for outcome in outcomes {
        let _posted = process_with_context(|context| {
            if context.interception_generation != generation {
                return;
            }
            let message = if native {
                WM_HOOK_HOTKEY_ACTION
            } else {
                WM_HOOK_ACTION
            };
            if !post_context_actions(context, outcome, |target, action| {
                post_action_message(target, action, generation, message)
            }) {
                context.state.reset_gestures();
            }
        });
    }
}

fn post_action_message(target: HWND, action: InputAction, generation: usize, message: u32) -> bool {
    let (wparam, lparam) = encode_action(action, generation);
    unsafe {
        // SAFETY: `target` is the UI HWND supplied at hook creation; PostMessageW copies the two
        // integer payloads and retains no Rust references.
        PostMessageW(Some(target), message, WPARAM(wparam), LPARAM(lparam))
    }
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::keyboard_state::KeyboardState;
    use crate::hook::test_context;
    use alttabio::hook_flags::{HookFlags, OVERLAY_ACTIVE, OVERLAY_FLAGS, SEARCH_ACTIVE};
    use alttabio::input::{HookSettings, HookState, KeyEvent, Modifiers, MouseEvent};
    use std::sync::{Arc, atomic::AtomicBool};
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_LMENU, VK_TAB};

    #[test]
    fn ordinary_alt_tab_requests_foreground_permission_before_replaying_release() {
        let mut context = test_context();
        context.target = HWND(10_usize as *mut core::ffi::c_void);
        let other_app = HWND(20_usize as *mut core::ffi::c_void);
        let modifiers = Modifiers::default();
        let _alt = context
            .state
            .process_key(KeyEvent::pressed(Key::LeftAlt, modifiers), context.settings);
        let opening = context
            .state
            .process_key(KeyEvent::pressed(Key::Tab, modifiers), context.settings);
        let data = KBDLLHOOKSTRUCT {
            vkCode: u32::from(VK_TAB.0),
            ..KBDLLHOOKSTRUCT::default()
        };
        let routed = route_registered_switch_with(
            &mut context,
            opening,
            &data,
            KeyTransition::Pressed,
            || other_app,
            |generation| {
                Ok(crate::switch_hotkey::PendingSwitch::without_registration(
                    generation,
                ))
            },
        );
        assert!(
            context.registered_switch.is_some(),
            "Ordinary Alt+Tab must receive physical hotkey permission when another app owns foreground"
        );
        assert!(
            !routed.suppress,
            "The actual Tab must reach hotkey processing"
        );
        assert!(routed.actions().next().is_none());
        for (event, vk) in [
            (KeyEvent::released(Key::Tab, modifiers), VK_TAB),
            (KeyEvent::released(Key::LeftAlt, modifiers), VK_LMENU),
        ] {
            let outcome = context.state.process_key(event, context.settings);
            let data = KBDLLHOOKSTRUCT {
                vkCode: u32::from(vk.0),
                ..KBDLLHOOKSTRUCT::default()
            };
            let routed = route_registered_switch_with(
                &mut context,
                outcome,
                &data,
                KeyTransition::Released,
                || other_app,
                |_| panic!("A key release must not register another hotkey"),
            );
            assert_eq!(routed.suppress, vk != VK_TAB);
            assert!(post_context_actions(&mut context, routed, |_, _| {
                panic!("The release must wait for the physical hotkey permission")
            }));
        }
        let Some(mut pending) = context.registered_switch.take() else {
            panic!("The physical hotkey must still own the buffered sequence")
        };
        let actions: Vec<_> = pending
            .actions
            .take()
            .flat_map(|outcome| outcome.actions().collect::<Vec<_>>())
            .collect();
        assert_eq!(actions, [InputAction::Switch(1), InputAction::AltReleased]);
    }

    #[test]
    fn focused_cycling_and_injected_input_do_not_register_a_hotkey() {
        for (focused, injected) in [(true, false), (false, true)] {
            let mut context = test_context();
            context.target = HWND(10_usize as *mut core::ffi::c_void);
            let foreground = if focused {
                context.target
            } else {
                HWND::default()
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
            let data = KBDLLHOOKSTRUCT {
                vkCode: u32::from(VK_TAB.0),
                flags: if injected {
                    LLKHF_INJECTED
                } else {
                    windows::Win32::UI::WindowsAndMessaging::KBDLLHOOKSTRUCT_FLAGS::default()
                },
                ..KBDLLHOOKSTRUCT::default()
            };
            let routed = route_registered_switch_with(
                &mut context,
                outcome,
                &data,
                KeyTransition::Pressed,
                || foreground,
                |_| panic!("This input must remain on the existing delivery path"),
            );
            assert_eq!(routed, outcome);
            assert!(context.registered_switch.is_none());
            assert!(!context.registered_tab_down);
        }
    }

    #[test]
    fn pending_recovery_or_suspension_registers_no_hotkey() {
        for recovering in [true, false] {
            let mut context = test_context();
            context.target = HWND(10_usize as *mut core::ffi::c_void);
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
            assert!(routes_tab_through_hotkey(
                Key::Tab,
                KeyTransition::Pressed,
                false,
                outcome
            ));
            if recovering {
                context.flags.set_recovery_pending(true);
            } else {
                // Synchronized while suspended, so the generation still matches the flags.
                context.flags.suspend(true);
                assert_eq!(context.sync_interception(), 0);
            }
            let data = KBDLLHOOKSTRUCT {
                vkCode: u32::from(VK_TAB.0),
                ..KBDLLHOOKSTRUCT::default()
            };
            let routed = route_registered_switch_with(
                &mut context,
                outcome,
                &data,
                KeyTransition::Pressed,
                HWND::default,
                |_| panic!("A stale generation must not register a hotkey"),
            );
            assert_eq!(routed, outcome);
            assert!(context.registered_switch.is_none());
            assert!(!context.registered_tab_down);
        }
    }

    #[test]
    fn unavailable_hotkey_keeps_tab_owned_and_reports_the_failure() {
        let mut context = test_context();
        context.target = HWND(10_usize as *mut core::ffi::c_void);
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
        let data = KBDLLHOOKSTRUCT {
            vkCode: u32::from(VK_TAB.0),
            ..KBDLLHOOKSTRUCT::default()
        };
        let routed = route_registered_switch_with(
            &mut context,
            outcome,
            &data,
            KeyTransition::Pressed,
            HWND::default,
            |_| Err(Error::from_hresult(windows::Win32::Foundation::E_FAIL)),
        );
        assert_eq!(routed, outcome);
        assert!(
            routed.suppress,
            "Do not leak Tab to the foreground application"
        );
        assert!(context.registered_switch.is_none());
        assert!(!context.registered_tab_down);
        assert_ne!(context.pending_errors & HOOK_ERROR_REGISTERED_SWITCH, 0);
    }

    #[test]
    fn registered_switch_keeps_mouse_and_keyboard_actions_in_physical_order() {
        use alttabio::switcher::{
            SwitchTask, SwitcherEffect, SwitcherSession, SwitcherSessionSettings,
        };

        let mut context = test_context();
        let mut pending = crate::switch_hotkey::PendingSwitch::without_registration(0);
        let _alt = context.state.process_key(
            KeyEvent::pressed(Key::LeftAlt, Modifiers::default()),
            context.settings,
        );
        let opening = context.state.process_key(
            KeyEvent::pressed(Key::Tab, Modifiers::default()),
            context.settings,
        );
        assert!(pending.actions.push(opening));
        context.registered_switch = Some(pending);
        let settings = HookSettings {
            right_button_wheel_switching: true,
            ..context.settings
        };
        let _press = context
            .state
            .process_mouse(MouseEvent::RightButtonPressed, settings);
        let mouse = context
            .state
            .process_mouse(MouseEvent::Wheel(120), settings);
        let mut delivered = Vec::new();
        assert!(post_context_actions(&mut context, mouse, |_, action| {
            delivered.push(action);
            true
        }));
        let release = context.state.process_key(
            KeyEvent::released(Key::LeftAlt, Modifiers::default()),
            context.settings,
        );
        let release = route_registered_switch(
            &mut context,
            release,
            &KBDLLHOOKSTRUCT {
                vkCode: u32::from(VK_LMENU.0),
                ..KBDLLHOOKSTRUCT::default()
            },
            KeyTransition::Released,
        );
        assert!(post_context_actions(&mut context, release, |_, action| {
            delivered.push(action);
            true
        }));
        let Some(mut pending) = context.registered_switch.take() else {
            panic!("pending switch lost")
        };
        for outcome in pending.actions.take() {
            assert!(post_context_actions(&mut context, outcome, |_, action| {
                delivered.push(action);
                true
            }));
        }
        let mut session = SwitcherSession::new(SwitcherSessionSettings {
            typed_search: true,
            release_alt_switches: true,
            release_right_button_switches: true,
        });
        let mut activated = None;
        for action in delivered {
            match session.handle_input(action) {
                SwitcherEffect::Open { selection_delta } => session.open(
                    [
                        SwitchTask::new(1, 10, "First", "first"),
                        SwitchTask::new(2, 20, "Second", "second"),
                        SwitchTask::new(3, 30, "Third", "third"),
                    ],
                    selection_delta,
                ),
                SwitcherEffect::Activate(target) => activated = Some(target),
                SwitcherEffect::Redraw | SwitcherEffect::None => {}
                effect => panic!("unexpected effect: {effect:?}"),
            }
        }
        assert_eq!(activated, Some(30));
        assert!(
            !session.is_visible(),
            "Alt release must complete the pending switch"
        );
    }

    #[test]
    fn pending_registered_actions_are_discarded_at_cancellation_and_modal_boundaries() {
        for modal in [false, true] {
            let mut context = test_context();
            let _alt = context.state.process_key(
                KeyEvent::pressed(Key::LeftAlt, Modifiers::default()),
                context.settings,
            );
            let opening = context.state.process_key(
                KeyEvent::pressed(Key::Tab, Modifiers::default()),
                context.settings,
            );
            let mut pending = crate::switch_hotkey::PendingSwitch::without_registration(0);
            assert!(pending.actions.push(opening));
            context.registered_switch = Some(pending);
            let dismissal = context.state.process_key(
                KeyEvent::pressed(Key::LeftWindows, Modifiers::default()),
                context.settings,
            );
            if modal {
                context.flags.suspend(true);
            }
            let mut delivered = Vec::new();
            assert!(post_context_actions(
                &mut context,
                dismissal,
                |_, action| {
                    delivered.push(action);
                    true
                }
            ));
            assert!(context.registered_switch.is_none());
            assert_eq!(
                delivered,
                if modal {
                    vec![]
                } else {
                    vec![InputAction::DismissOverlay]
                }
            );
        }
    }

    #[test]
    fn physical_hotkey_tab_release_is_balanced_when_a_modal_cancels_the_gesture() {
        let mut context = test_context();
        let modifiers = Modifiers::default();
        let _ = context
            .state
            .process_key(KeyEvent::pressed(Key::LeftAlt, modifiers), context.settings);
        let _ = context
            .state
            .process_key(KeyEvent::pressed(Key::Tab, modifiers), context.settings);
        context.registered_tab_down = true;
        context.state.set_interception_suspended(true);
        for (key, vk, expected_suppression) in
            [(Key::Tab, VK_TAB, false), (Key::LeftAlt, VK_LMENU, true)]
        {
            let outcome = context
                .state
                .process_key(KeyEvent::released(key, modifiers), context.settings);
            let data = KBDLLHOOKSTRUCT {
                vkCode: u32::from(vk.0),
                ..KBDLLHOOKSTRUCT::default()
            };
            let routed =
                route_registered_switch(&mut context, outcome, &data, KeyTransition::Released);
            assert_eq!(routed.suppress, expected_suppression);
            assert!(routed.actions().next().is_none());
        }
        assert!(!context.registered_tab_down);
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
    fn suspension_during_mouse_action_delivery_stops_remaining_actions() {
        let mut context = test_context();
        let flags = Arc::clone(&context.flags);
        let _down = context
            .state
            .process_mouse(MouseEvent::RightButtonPressed, context.settings);
        let outcome = context
            .state
            .process_mouse(MouseEvent::Wheel(120), context.settings);
        let mut posts = 0;
        assert!(post_context_actions(&mut context, outcome, |_, action| {
            posts += 1;
            assert_eq!(action, InputAction::RightButtonPressed);
            flags.suspend(true);
            true
        }));
        assert_eq!(posts, 1);
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
        assert_eq!(context.sync_interception(), 0);
        let release = context
            .state
            .process_mouse(MouseEvent::RightButtonReleased, context.settings);
        assert!(release.suppress);
        assert!(release.actions().next().is_none());
    }

    #[test]
    fn posting_overlay_open_arms_typed_search_before_the_ui_acknowledges_visibility() {
        let flags = Arc::new(HookFlags::default());
        let mut context = HookContext {
            registered_switch: None,
            registered_tab_down: false,
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
