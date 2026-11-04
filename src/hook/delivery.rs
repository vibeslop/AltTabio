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
    fn queued_keyboard_gestures_complete_independently() {
        assert_queued_gestures_complete_independently(false, false);
    }

    #[test]
    fn queued_mouse_gestures_complete_independently() {
        assert_queued_gestures_complete_independently(true, false);
    }

    #[test]
    fn queued_enter_gestures_complete_independently() {
        assert_queued_gestures_complete_independently(false, true);
    }

    #[test]
    fn queued_mouse_enter_gestures_complete_independently() {
        assert_queued_gestures_complete_independently(true, true);
    }

    #[derive(Clone, Copy)]
    enum Event {
        Key(KeyEvent),
        Mouse(alttabio::input::MouseEvent),
    }

    #[test]
    fn stale_opening_receipt_cannot_retire_a_restarted_hooks_pending_input() {
        let old = HookFlags::new();
        let stale = encode_action(InputAction::Switch(1), old.load());
        let mut context = test_context();
        let flags = Arc::clone(&context.flags);
        let mut queue = Vec::new();
        let events = completed_gesture_events(false, false);
        for event in events.iter().chain(events[..events.len() - 1].iter()) {
            enqueue_event(&mut context, *event, &mut queue);
        }
        assert!(!flags.action_is_current(stale.0));
        retire_opening(&flags, InputAction::Switch(1), stale.0);
        retire_opening(&flags, InputAction::Switch(1), queue[0].0); // First current opening.
        flags.complete_overlay();
        assert_ne!(
            flags.load() & OVERLAY_ACTIVE,
            0,
            "a stale hook's message disarmed the pending new hook gesture"
        );
    }

    #[test]
    fn completion_keeps_escape_owned_for_a_pending_opening() {
        use alttabio::hook_flags::decode_action;
        use alttabio::switcher::{
            SwitchTask, SwitcherEffect, SwitcherSession, SwitcherSessionSettings,
        };
        for mouse in [false, true] {
            let mut context = test_context();
            context.settings.right_button_wheel_switching = mouse;
            let flags = Arc::clone(&context.flags);
            let events = completed_gesture_events(mouse, false);
            let mut queue = Vec::new();
            for event in events.iter().chain(events[..events.len() - 1].iter()) {
                enqueue_event(&mut context, *event, &mut queue);
            }
            let mut session = SwitcherSession::new(SwitcherSessionSettings {
                typed_search: true,
                release_alt_switches: true,
            });
            for (wparam, lparam) in queue.drain(..2) {
                let Some(action) = decode_action(wparam, lparam) else {
                    panic!("invalid queued action");
                };
                retire_opening(&flags, action, wparam);
                match session.handle_input(action) {
                    SwitcherEffect::Open { selection_delta } => {
                        session.open([SwitchTask::new(1, 10, "One", "one")], selection_delta);
                    }
                    SwitcherEffect::Activate(_) => {
                        session.hide();
                        flags.complete_overlay();
                    }
                    effect => panic!("unexpected first gesture effect: {effect:?}"),
                }
            }
            assert!(!session.is_visible());
            let escape = enqueue_event(
                &mut context,
                Event::Key(KeyEvent::pressed(Key::Escape, Modifiers::default())),
                &mut queue,
            );
            assert!(
                escape.suppress,
                "older completion leaked Escape from the pending gesture (mouse={mouse})"
            );
            assert_eq!(escape.actions().next(), Some(InputAction::DismissOverlay));
            let Some(&last) = events.last() else {
                panic!("missing release");
            };
            let release = enqueue_event(&mut context, last, &mut queue);
            assert!(
                release.actions().next().is_none(),
                "cancelled release activated a window"
            );
            for (wparam, lparam) in queue {
                let Some(action) = decode_action(wparam, lparam) else {
                    panic!("invalid queued action");
                };
                retire_opening(&flags, action, wparam);
                match session.handle_input(action) {
                    SwitcherEffect::Open { selection_delta } => {
                        session.open([SwitchTask::new(1, 10, "One", "one")], selection_delta);
                    }
                    SwitcherEffect::Hide => {
                        flags.cancel_overlay();
                        session.hide();
                    }
                    SwitcherEffect::None | SwitcherEffect::Redraw => {}
                    effect => panic!("cancelled second gesture: {effect:?}"),
                }
            }
            assert!(!session.is_visible());
        }
    }

    fn enqueue_event(
        context: &mut HookContext,
        event: Event,
        queue: &mut Vec<(usize, isize)>,
    ) -> HookOutcome {
        let overlay = context.sync_interception();
        context
            .state
            .set_overlay_active(overlay & OVERLAY_ACTIVE != 0);
        let outcome = match event {
            Event::Key(event) => context.state.process_key(event, context.settings),
            Event::Mouse(event) => context.state.process_mouse(event, context.settings),
        };
        let generation = context.interception_generation;
        assert!(post_context_actions(context, outcome, |_, action| {
            queue.push(encode_action(action, generation));
            true
        }));
        outcome
    }

    fn retire_opening(flags: &HookFlags, action: InputAction, origin: usize) {
        if matches!(action, InputAction::Switch(_) | InputAction::MouseWheel(_)) {
            flags.retire_action_openings(origin, 1);
        }
    }

    fn completed_gesture_events(mouse: bool, enter: bool) -> Vec<Event> {
        use alttabio::input::MouseEvent;
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        let mut events = if mouse {
            vec![
                Event::Mouse(MouseEvent::RightButtonPressed),
                Event::Mouse(MouseEvent::Wheel(-120)),
                Event::Mouse(MouseEvent::RightButtonReleased),
            ]
        } else {
            vec![
                Event::Key(KeyEvent::pressed(Key::LeftAlt, Modifiers::default())),
                Event::Key(KeyEvent::pressed(Key::Tab, alt)),
                Event::Key(KeyEvent::released(Key::Tab, alt)),
                Event::Key(KeyEvent::released(Key::LeftAlt, Modifiers::default())),
            ]
        };
        if enter {
            events.insert(
                events.len() - 1,
                Event::Key(KeyEvent::pressed(Key::Enter, alt)),
            );
            events.insert(
                events.len() - 1,
                Event::Key(KeyEvent::released(Key::Enter, alt)),
            );
        }
        events
    }

    fn assert_queued_gestures_complete_independently(mouse: bool, enter: bool) {
        use crate::hook::HookThread;
        use alttabio::hook_flags::decode_action;
        use alttabio::switcher::{
            SwitchTask, SwitcherEffect, SwitcherSession, SwitcherSessionSettings,
        };
        let mut context = test_context();
        context.settings.right_button_wheel_switching = mouse;
        let flags = Arc::clone(&context.flags);
        let hooks = HookThread {
            thread_id: u32::MAX,
            join_handle: None,
            remote_desktop_passthrough: Arc::new(AtomicBool::new(false)),
            flags: Arc::clone(&flags),
        };
        let events = completed_gesture_events(mouse, enter);
        let mut queue = Vec::new();
        for event in events.iter().cycle().take(events.len() * 2) {
            let overlay = context.sync_interception();
            context
                .state
                .set_overlay_active(overlay & OVERLAY_ACTIVE != 0);
            let outcome = match *event {
                Event::Key(event) => context.state.process_key(event, context.settings),
                Event::Mouse(event) => context.state.process_mouse(event, context.settings),
            };
            if matches!(
                *event,
                Event::Key(KeyEvent {
                    key: Key::Enter,
                    ..
                })
            ) {
                assert!(
                    outcome.suppress,
                    "Enter press and release must remain owned"
                );
            }
            let generation = context.interception_generation;
            assert!(post_context_actions(&mut context, outcome, |_, action| {
                queue.push(encode_action(action, generation));
                true
            }));
        }
        assert_eq!(
            queue.len(),
            if mouse { 5 } else { 4 },
            "two complete gestures must be queued before UI consumption"
        );
        let mut session = SwitcherSession::new(SwitcherSessionSettings {
            typed_search: true,
            release_alt_switches: true,
        });
        let mut activations = 0;
        for (wparam, lparam) in queue {
            if !flags.action_is_current(wparam) {
                continue;
            }
            let Some(action) = decode_action(wparam, lparam) else {
                panic!("invalid queued action");
            };
            retire_opening(&flags, action, wparam);
            match session.handle_input(action) {
                SwitcherEffect::Open { selection_delta } => session.open(
                    [
                        SwitchTask::new(1, 10, "One", "one"),
                        SwitchTask::new(2, 20, "Two", "two"),
                    ],
                    selection_delta,
                ),
                SwitcherEffect::Activate(_) => {
                    activations += 1;
                    // Use the same completion/cancellation policy as the activation adapter.
                    if !action.completes_selection() {
                        assert!(hooks.reset_gestures().is_err());
                    }
                    session.hide();
                    flags.complete_overlay();
                }
                SwitcherEffect::None | SwitcherEffect::Redraw => {}
                effect @ (SwitcherEffect::Hide | SwitcherEffect::Execute(_)) => {
                    panic!("unexpected effect: {effect:?}")
                }
            }
        }
        assert_eq!(
            activations, 2,
            "normal completion discarded the next independent gesture"
        );
        assert!(!session.is_visible());
    }

    #[test]
    fn outside_cancellation_retires_queued_mouse_actions_even_if_reset_post_fails() {
        use crate::hook::HookThread;
        use alttabio::hook_flags::decode_action;
        use alttabio::input::MouseEvent;
        use alttabio::switcher::{
            SwitchTask, SwitcherEffect, SwitcherSession, SwitcherSessionSettings,
        };

        let mut context = test_context();
        context.settings.right_button_wheel_switching = true;
        let flags = Arc::clone(&context.flags);
        // This fixture owns no OS thread. An invalid ID makes the wake-up post fail,
        // so invalidating queued input cannot depend on the wake-up being delivered.
        let hooks = HookThread {
            thread_id: u32::MAX,
            join_handle: None,
            remote_desktop_passthrough: Arc::new(AtomicBool::new(false)),
            flags: Arc::clone(&flags),
        };
        let mut queue = Vec::new();
        for (index, event) in [
            MouseEvent::RightButtonPressed,
            MouseEvent::Wheel(-120),
            MouseEvent::Wheel(-120),
            MouseEvent::RightButtonReleased,
        ]
        .into_iter()
        .enumerate()
        {
            if index == 2 {
                queue.push(None); // Accepted outside-pointer dismissal, in physical event order.
            }
            let outcome = context.state.process_mouse(event, context.settings);
            let generation = context.interception_generation;
            assert!(post_context_actions(&mut context, outcome, |_, action| {
                queue.push(Some(encode_action(action, generation)));
                true
            }));
        }

        let mut session = SwitcherSession::new(SwitcherSessionSettings {
            typed_search: true,
            release_alt_switches: true,
        });
        let mut openings = 0;
        let mut activations = Vec::new();
        for message in queue {
            let Some((wparam, lparam)) = message else {
                session.hide();
                flags.set(OVERLAY_FLAGS, false);
                assert!(hooks.reset_gestures().is_err());
                continue;
            };
            if !flags.action_is_current(wparam) {
                continue;
            }
            let Some(action) = decode_action(wparam, lparam) else {
                panic!("invalid queued action");
            };
            match session.handle_input(action) {
                SwitcherEffect::Open { selection_delta } => {
                    openings += 1;
                    session.open(
                        [
                            SwitchTask::new(1, 10, "One", "one"),
                            SwitchTask::new(2, 20, "Two", "two"),
                        ],
                        selection_delta,
                    );
                }
                SwitcherEffect::Activate(target) => activations.push(target),
                SwitcherEffect::None | SwitcherEffect::Redraw => {}
                effect @ (SwitcherEffect::Hide | SwitcherEffect::Execute(_)) => {
                    panic!("unexpected effect: {effect:?}")
                }
            }
        }
        assert_eq!(
            openings, 1,
            "queued wheel input reopened the cancelled switcher"
        );
        assert!(
            activations.is_empty(),
            "RMB release committed after outside cancellation"
        );
        assert!(!session.is_visible());
        context.sync_interception();
        assert_eq!(
            context
                .state
                .process_mouse(MouseEvent::Wheel(-120), context.settings),
            HookOutcome::default()
        );
        let _fresh_press = context
            .state
            .process_mouse(MouseEvent::RightButtonPressed, context.settings);
        assert_eq!(
            context
                .state
                .process_mouse(MouseEvent::Wheel(-120), context.settings)
                .actions()
                .next(),
            Some(InputAction::MouseWheel(1))
        );
    }

    #[test]
    fn delayed_cancellation_reset_keeps_a_fresh_mouse_gesture() {
        use alttabio::input::MouseEvent;
        let mut context = test_context();
        context.settings.right_button_wheel_switching = true;
        context.flags.advance_generation();
        // A fresh callback observes the cancellation before its posted wake-up runs.
        context.sync_interception();
        let _press = context
            .state
            .process_mouse(MouseEvent::RightButtonPressed, context.settings);
        let _wheel = context
            .state
            .process_mouse(MouseEvent::Wheel(-120), context.settings);
        CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));
        crate::hook::handle_gesture_reset();
        let release = process_with_context(|context| {
            context
                .state
                .process_mouse(MouseEvent::RightButtonReleased, context.settings)
        });
        CONTEXT.with(|slot| *slot.borrow_mut() = None);
        assert_eq!(
            release.and_then(|outcome| outcome.actions().next()),
            Some(InputAction::MouseWheelReleased)
        );
    }

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
        flags.set_search_state(true, true);
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
            right_button_release: Arc::new(AtomicBool::new(false)),
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
        assert_eq!(flags.load() & OVERLAY_FLAGS, OVERLAY_ACTIVE | SEARCH_ACTIVE);
    }
}
