//! Delayed native hook delivery must route by the captured point, not the later cursor.

use super::super::*;
use super::PendingShellDismissal;
use alttabio::deferred_switch::{DeferredSwitch, DeferredSwitchPoll};
use alttabio::hook_flags::OVERLAY_ACTIVE;
use alttabio::input::InputAction;
use alttabio::switcher::{SwitchTask, SwitcherEffect};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, MSLLHOOKSTRUCT, PM_REMOVE, PeekMessageW, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WS_POPUP,
};

#[test]
fn queued_wheel_list_to_outside_preserves_activation() -> Result<()> {
    delayed_wheel(true, false)
}

#[test]
fn queued_wheel_preview_to_list_preserves_activation() -> Result<()> {
    delayed_wheel(false, false)
}

#[test]
fn queued_gesture_list_to_outside_preserves_activation() -> Result<()> {
    delayed_wheel(true, true)
}

#[test]
fn queued_gesture_preview_to_list_preserves_activation() -> Result<()> {
    delayed_wheel(false, true)
}

struct FixtureWindow(HWND);

impl Drop for FixtureWindow {
    fn drop(&mut self) {
        crate::hook::test_clear_mouse();
        // SAFETY: the fixture uniquely owns this hidden window on its creating thread.
        assert!(unsafe { DestroyWindow(self.0) }.is_ok());
    }
}

fn delayed_wheel(captured_in_list: bool, gesture: bool) -> Result<()> {
    for (deferred, parts) in [(false, 1), (true, 1), (true, 4)] {
        delayed_wheel_case(captured_in_list, gesture, deferred, parts)?;
    }
    Ok(())
}

fn fixture_window(captured_in_list: bool) -> Result<FixtureWindow> {
    let mut cursor = POINT::default();
    // SAFETY: the output is writable. This test observes but never moves the user's pointer.
    unsafe { GetCursorPos(&raw mut cursor) }?;
    // Put the hidden window around the current pointer, or far away from it. This models
    // movement between capture and dispatch without injecting desktop input.
    let (x, y) = if captured_in_list {
        (-32000, -32000)
    } else {
        (cursor.x - 100, cursor.y - 100)
    };
    Ok(FixtureWindow(unsafe {
        // SAFETY: STATIC is a system class; the test owns this hidden popup.
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("STATIC"),
            w!("Queued wheel fixture"),
            WS_POPUP,
            x,
            y,
            900,
            600,
            None,
            None,
            Some(module_instance()?),
            None,
        )
    }?))
}

fn fixture_app(hwnd: HWND) -> Result<(App, Arc<alttabio::hook_flags::HookFlags>)> {
    let (store, settings) = SettingsStore::load_from(
        std::env::temp_dir().join(format!("alttabio-wheel-{}.ini", std::process::id())),
        &Settings::default(),
    )
    .map_err(|error| Error::new(windows::core::HRESULT(0x8000_4005_u32.cast_signed()), error))?;
    let mut app = App::new(false, false, settings, store)?;
    app.hwnd = hwnd;
    app.renderer.prepare(hwnd)?;
    let (hooks, flags) = crate::hook::test_ui_owner();
    app.hooks = Some(hooks);
    app.session.open(
        (0..40).map(|i| SwitchTask::new(i + 1, (i + 1).cast_signed(), "Task", "task")),
        Some(1),
    );
    flags.set(OVERLAY_ACTIVE, true);
    Ok((app, flags))
}

fn delayed_wheel_case(
    captured_in_list: bool,
    gesture: bool,
    deferred: bool,
    parts: i16,
) -> Result<()> {
    let _com = ComApartment::initialize()?;
    let window = fixture_window(captured_in_list)?;
    let (mut app, flags) = fixture_app(window.0)?;
    let mut captured = POINT {
        x: if captured_in_list { 100 } else { 700 },
        y: 100,
    };
    // SAFETY: the hidden fixture HWND is live and the output is writable.
    assert!(unsafe { ClientToScreen(window.0, &raw mut captured) }.as_bool());
    if gesture {
        crate::hook::test_post_mouse(
            window.0,
            Arc::clone(&flags),
            WM_RBUTTONDOWN,
            &MSLLHOOKSTRUCT::default(),
            true,
        );
    }
    for _ in 0..parts {
        crate::hook::test_post_mouse(
            window.0,
            Arc::clone(&flags),
            WM_MOUSEWHEEL,
            &MSLLHOOKSTRUCT {
                pt: captured,
                mouseData: u32::from((-120_i16 / parts).cast_unsigned()) << 16,
                ..Default::default()
            },
            gesture,
        );
    }
    if deferred {
        // The opening preview is already visible, but shell focus has not yet been released.
        // Keep a wheel-bearing origin to exercise its epoch when later preview actions retire.
        let (origin, _) = alttabio::hook_flags::encode_action(
            InputAction::MouseWheel(1, (captured.x, captured.y)),
            flags.load(),
        );
        let mut input = DeferredSwitch::new(InputAction::Switch(1));
        assert_eq!(input.take_preview_actions(), [InputAction::Switch(1)]);
        app.pending_shell = Some(PendingShellDismissal {
            window: window.0,
            origin: WPARAM(origin),
            started: Instant::now(),
            input,
        });
    }
    // No message has reached the UI yet. Deliver the exact posted scalar payloads through
    // App's message dispatcher, including epoch validation and opening retirement.
    let dispatched = dispatch_wheel_messages(&mut app, window.0);
    assert_eq!(dispatched, if gesture { 2 } else { i32::from(parts) });
    let release = if gesture {
        InputAction::MouseWheelReleased
    } else {
        InputAction::AltReleased
    };
    if deferred {
        assert!(app.defer_to_shell_dismissal(release));
        assert!(
            app.session.is_visible(),
            "shell must retain focus-dependent activation"
        );
        let Some(mut pending) = app.pending_shell.take() else {
            panic!("shell dismissal must still be pending");
        };
        assert!(flags.action_is_current(pending.origin.0));
        // RightButtonPressed holds the following gesture wheel behind the focus barrier;
        // ordinary wheel actions have already been previewed through apply_hook_action.
        assert_eq!(pending.input.pending_openings(), usize::from(gesture));
        assert_eq!(
            pending.input.poll(true, Duration::ZERO),
            DeferredSwitchPoll::Wait
        );
        let DeferredSwitchPoll::Ready(mut actions) = pending.input.poll(false, Duration::ZERO)
        else {
            panic!("queued input must resume after shell dismissal");
        };
        assert_eq!(actions.pop(), Some(release));
        for action in actions {
            app.apply_hook_action(action, pending.origin);
        }
    }
    assert_wheel_result(&mut app, captured_in_list);
    // Exercise the real release resolver, without activating a window on the user's desktop.
    assert_eq!(
        app.session.handle_input(release),
        SwitcherEffect::Activate(if captured_in_list { 2 } else { 3 })
    );
    flags.complete_overlay();
    assert_eq!(
        flags.load() & OVERLAY_ACTIVE,
        0,
        "wheel opening receipt leaked"
    );
    Ok(())
}

fn assert_wheel_result(app: &mut App, captured_in_list: bool) {
    assert_eq!(
        app.session
            .switcher()
            .selected_task()
            .map(|task| task.window_handle),
        Some(if captured_in_list { 2 } else { 3 }),
        "queued wheel used the later cursor pane"
    );
    if captured_in_list {
        assert!(
            app.session.switcher_mut().visible_range(5).start > 0,
            "list wheel must move the viewport"
        );
    }
}

fn dispatch_wheel_messages(app: &mut App, hwnd: HWND) -> i32 {
    let mut message = MSG::default();
    let mut dispatched = 0;
    // SAFETY: only this test thread reads this live fixture's message queue.
    while unsafe {
        PeekMessageW(
            &raw mut message,
            Some(hwnd),
            crate::app_messages::WM_HOOK_ACTION,
            crate::app_messages::WM_HOOK_ACTION,
            PM_REMOVE,
        )
    }
    .as_bool()
    {
        assert!(
            app.handle_message(message.message, message.wParam, message.lParam)
                .is_some()
        );
        dispatched += 1;
    }
    dispatched
}
