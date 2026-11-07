//! Button delivery at the real hook boundary. Native fixtures use an unselected private desktop.

use super::*;
use std::cell::Cell;
use windows::Win32::UI::Input::KeyboardAndMouse::MOUSEEVENTF_RIGHTDOWN;

#[test]
fn ordinary_click_is_queued_until_the_physical_callback_has_returned() {
    use windows::Win32::UI::WindowsAndMessaging::PM_REMOVE;

    let mut message = MSG::default();
    // SAFETY: this test thread owns its message queue and initializes it before posting.
    let _queued = unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_NOREMOVE) };
    let mut context = test_context();
    context.settings.right_button_wheel_switching = true;
    // SAFETY: GetCurrentThreadId has no preconditions.
    context.hook_thread_id = unsafe { GetCurrentThreadId() };
    CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));
    let data = MSLLHOOKSTRUCT::default();
    for physical in [WM_RBUTTONDOWN, WM_RBUTTONUP] {
        let outcome = process_mouse_message(
            WPARAM(physical as usize),
            LPARAM((&raw const data) as isize),
        );
        assert!(outcome.is_some_and(|outcome| outcome.suppress));
    }
    // The real callback only posts a request. Sending from it would block Windows input and leave
    // no queued replay for the message loop to execute after the physical release returns.
    // SAFETY: message is writable; PeekMessageW reads only this thread's replay requests.
    assert!(unsafe {
        PeekMessageW(
            &raw mut message,
            None,
            WM_REPLAY_RIGHT_CLICK,
            WM_REPLAY_RIGHT_CLICK,
            PM_REMOVE,
        )
        .as_bool()
    });
    let sent = Cell::new(0);
    assert!(handle_mouse_replay_with(message.message, || {
        sent.set(sent.get() + 1);
        true
    }));
    assert_eq!(sent.get(), 1);
    // SAFETY: as above; a physical click must enqueue exactly one replay.
    assert!(!unsafe {
        PeekMessageW(
            &raw mut message,
            None,
            WM_REPLAY_RIGHT_CLICK,
            WM_REPLAY_RIGHT_CLICK,
            PM_REMOVE,
        )
        .as_bool()
    });
    CONTEXT.with(|slot| *slot.borrow_mut() = None);
}

#[test]
fn failed_deferred_click_reports_an_error_and_unrelated_messages_do_not_replay() {
    CONTEXT.with(|slot| *slot.borrow_mut() = Some(test_context()));
    assert!(!handle_mouse_replay_with(WM_RESET_GESTURES, || {
        panic!("unrelated message replayed a click")
    }));
    assert!(handle_mouse_replay_with(WM_REPLAY_RIGHT_CLICK, || false));
    assert_eq!(
        process_with_context(|context| context.pending_errors & HOOK_ERROR_REPLAY_MOUSE),
        Some(HOOK_ERROR_REPLAY_MOUSE)
    );
    CONTEXT.with(|slot| *slot.borrow_mut() = None);
}

#[test]
fn plain_right_click_replays_once_and_its_tag_bypasses_the_hook() {
    let mut context = test_context();
    context.settings.right_button_wheel_switching = true;
    CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));
    let replayed = Cell::new(0);
    for message in [WM_RBUTTONDOWN, WM_RBUTTONUP] {
        let data = MSLLHOOKSTRUCT::default();
        let outcome = process_mouse_message_with(
            WPARAM(message as usize),
            LPARAM((&raw const data) as isize),
            || {
                replayed.set(replayed.get() + 1);
                let tagged = MSLLHOOKSTRUCT {
                    dwExtraInfo: replay::REPLAYED_INPUT_MARKER,
                    ..Default::default()
                };
                assert_eq!(
                    process_mouse_message_with(
                        WPARAM(WM_RBUTTONUP as usize),
                        LPARAM((&raw const tagged) as isize),
                        || panic!("recursive replay")
                    ),
                    None
                );
                true
            },
        );
        assert!(outcome.is_some_and(|outcome| outcome.suppress));
    }
    assert_eq!(replayed.get(), 1);
    CONTEXT.with(|slot| *slot.borrow_mut() = None);
}

#[test]
fn restarting_the_hook_retains_the_swallowed_buttons_release_debt() {
    let mut old = test_context();
    old.settings.right_button_wheel_switching = true;
    let release = Arc::clone(&old.right_button_release);
    CONTEXT.with(|slot| *slot.borrow_mut() = Some(old));
    let data = MSLLHOOKSTRUCT::default();
    let down = process_mouse_message_with(
        WPARAM(WM_RBUTTONDOWN as usize),
        LPARAM((&raw const data) as isize),
        || panic!("unexpected click"),
    );
    assert!(down.is_some_and(|outcome| outcome.suppress));
    let mut restarted = test_context(); // Feature is now disabled in the replacement hook.
    restarted.right_button_release = release;
    CONTEXT.with(|slot| *slot.borrow_mut() = Some(restarted));
    let up = process_mouse_message_with(
        WPARAM(WM_RBUTTONUP as usize),
        LPARAM((&raw const data) as isize),
        || panic!("cancelled click replayed"),
    );
    assert!(up.is_some_and(|outcome| outcome.suppress && outcome.actions().next().is_none()));
    CONTEXT.with(|slot| *slot.borrow_mut() = None);
}

#[test]
fn failed_click_replay_reports_an_error_without_forwarding_an_unpaired_release() {
    let mut context = test_context();
    context.settings.right_button_wheel_switching = true;
    CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));
    let data = MSLLHOOKSTRUCT::default();
    let _down = process_mouse_message_with(
        WPARAM(WM_RBUTTONDOWN as usize),
        LPARAM((&raw const data) as isize),
        || panic!("early replay"),
    );
    let up = process_mouse_message_with(
        WPARAM(WM_RBUTTONUP as usize),
        LPARAM((&raw const data) as isize),
        || false,
    );
    assert!(up.is_some_and(|outcome| outcome.suppress));
    assert_eq!(
        process_with_context(|context| context.pending_errors & HOOK_ERROR_REPLAY_MOUSE),
        Some(HOOK_ERROR_REPLAY_MOUSE)
    );
    CONTEXT.with(|slot| *slot.borrow_mut() = None);
}

#[test]
fn a_failed_completion_post_cannot_forward_an_owned_button_release() {
    let mut state = HookState::default();
    let settings = HookSettings {
        right_button_wheel_switching: true,
        ..HookSettings::default()
    };
    let _down = state.process_mouse(MouseEvent::RightButtonPressed, settings);
    let _wheel = state.process_mouse(MouseEvent::Wheel(-120, (0, 0)), settings);
    let release = state.process_mouse(MouseEvent::RightButtonReleased, settings);
    assert_eq!(
        finish_mouse_callback_with(
            Some(release),
            |_| false,
            || panic!("owned release reached the original app")
        ),
        LRESULT(1)
    );
}

#[test]
#[ignore = "runs native recipients on an unselected private desktop; invoke explicitly"]
fn native_mouse_release_does_not_generate_the_original_context_menu() {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let executable =
        std::env::current_exe().unwrap_or_else(|error| panic!("test executable: {error}"));
    let mut child = Command::new(executable)
        .args([
            "--ignored",
            "--exact",
            "hook::mouse_tests::native_mouse_release_fixture",
            "--nocapture",
        ])
        .env("ALTTABIO_MOUSE_RELEASE_FIXTURE", "1")
        .creation_flags(0x0800_0000)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("start fixture: {error}"));
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => break None,
            Err(error) => panic!("fixture status: {error}"),
        }
    };
    if status.is_none() {
        child
            .kill()
            .unwrap_or_else(|error| panic!("stop fixture: {error}"));
    }
    let output = child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("fixture output: {error}"));
    assert!(
        status.is_some_and(|status| status.success()),
        "native mouse fixture failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

thread_local! { static CONTEXT_MENUS: Cell<usize> = const { Cell::new(0) }; }

#[test]
#[ignore = "child process of the isolated native mouse test"]
#[allow(
    clippy::too_many_lines,
    reason = "isolated native fixture setup and event-order assertions"
)]
fn native_mouse_release_fixture() {
    use crate::win32::wide;
    use windows::Win32::System::StationsAndDesktops::{
        CreateDesktopW, DESKTOP_CONTROL_FLAGS, SetThreadDesktop,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetActiveWindow, SetActiveWindow};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, RegisterClassW, SW_SHOWNOACTIVATE, SendMessageW, ShowWindow,
        WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    };
    use windows::core::PCWSTR;
    if std::env::var_os("ALTTABIO_MOUSE_RELEASE_FIXTURE").is_none() {
        return;
    }
    let name = wide(&format!("AltTabio-mouse-release-{}", std::process::id()));
    // SAFETY: a disposable child owns the desktop. It is never selected as the input desktop.
    let desktop = unsafe {
        CreateDesktopW(
            PCWSTR(name.as_ptr()),
            PCWSTR::null(),
            None,
            DESKTOP_CONTROL_FLAGS(0),
            0x01ff,
            None,
        )
    }
    .unwrap_or_else(|error| panic!("private desktop: {error}"));
    // SAFETY: this fresh thread has no windows or hooks yet. No user desktop is switched.
    unsafe { SetThreadDesktop(desktop) }
        .unwrap_or_else(|error| panic!("select private desktop: {error}"));
    let class = wide("AltTabioMouseReleaseFixture");
    let registration = WNDCLASSW {
        lpfnWndProc: Some(recipient_proc),
        lpszClassName: PCWSTR(class.as_ptr()),
        ..Default::default()
    };
    // SAFETY: the strings and callback remain valid for this disposable child's lifetime.
    assert_ne!(unsafe { RegisterClassW(&raw const registration) }, 0);
    let window = || {
        unsafe {
            // SAFETY: this thread owns both fixture windows and the registered class remains live.
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR(class.as_ptr()),
                PCWSTR(name.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                200,
                100,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap_or_else(|error| panic!("fixture window: {error}"))
    };
    let original = window();
    let selected = window();
    // SAFETY: both windows belong to this child's unselected desktop, never the user's desktop.
    unsafe {
        let _original = ShowWindow(original, SW_SHOWNOACTIVATE);
        let _selected = ShowWindow(selected, SW_SHOWNOACTIVATE);
    }
    for enabled in [false, true] {
        for wheel in [false, true] {
            if !enabled && wheel {
                continue;
            }
            CONTEXT_MENUS.with(|menus| menus.set(0));
            let mut context = test_context();
            context.settings.right_button_wheel_switching = enabled;
            let flags = Arc::clone(&context.flags);
            CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));
            let messages = if wheel {
                vec![WM_RBUTTONDOWN, WM_MOUSEWHEEL, WM_RBUTTONUP]
            } else {
                vec![WM_RBUTTONDOWN, WM_RBUTTONUP]
            };
            for message in messages {
                let data = MSLLHOOKSTRUCT {
                    mouseData: 0xff88_0000,
                    ..Default::default()
                };
                let outcome = process_mouse_message_with(
                    WPARAM(message as usize),
                    LPARAM((&raw const data) as isize),
                    || {
                        mouse_replay::replay_right_click_with(|inputs, _| {
                            for input in inputs {
                                // SAFETY: replay supplies only initialized INPUT_MOUSE records.
                                let mouse = unsafe { input.Anonymous.mi };
                                let message = if mouse.dwFlags == MOUSEEVENTF_RIGHTDOWN {
                                    WM_RBUTTONDOWN
                                } else {
                                    WM_RBUTTONUP
                                };
                                let tagged = MSLLHOOKSTRUCT {
                                    dwExtraInfo: mouse.dwExtraInfo,
                                    ..Default::default()
                                };
                                assert_eq!(
                                    process_mouse_message_with(
                                        WPARAM(message as usize),
                                        LPARAM((&raw const tagged) as isize),
                                        || panic!("recursive replay")
                                    ),
                                    None
                                );
                                // SAFETY: the recipient belongs to this thread on the private desktop.
                                unsafe {
                                    SendMessageW(
                                        original,
                                        message,
                                        Some(WPARAM(0)),
                                        Some(LPARAM(0)),
                                    );
                                }
                            }
                            u32::try_from(inputs.len()).unwrap_or_default()
                        })
                    },
                );
                finish_mouse_callback_with(
                    outcome,
                    |outcome| {
                        for action in outcome.actions() {
                            if matches!(action, InputAction::MouseWheel(_, _)) {
                                flags.set(OVERLAY_ACTIVE, true);
                            }
                            if action == InputAction::MouseWheelReleased {
                                // SAFETY: selected is owned by this thread on the private desktop.
                                unsafe {
                                    let _previous = SetActiveWindow(selected);
                                }
                            }
                        }
                        true
                    },
                    || {
                        // SAFETY: fixture windows are live and owned by the current thread.
                        unsafe { SendMessageW(original, message, Some(WPARAM(0)), Some(LPARAM(0))) }
                    },
                );
            }
            let expected_menu = usize::from(!wheel);
            assert_eq!(
                CONTEXT_MENUS.with(Cell::get),
                expected_menu,
                "enabled={enabled}, wheel={wheel}"
            );
            // SAFETY: GetActiveWindow reads the current fixture thread's own input queue.
            assert_eq!(
                unsafe { GetActiveWindow() },
                if wheel { selected } else { original }
            );
            CONTEXT.with(|slot| *slot.borrow_mut() = None);
        }
    }
    println!(
        "native mouse recipients: no context menu/focus return after switching; ordinary clicks preserved"
    );
    // The disposable child owns the windows and desktop. Process exit releases them together.
    std::process::exit(0);
}

unsafe extern "system" fn recipient_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        ReleaseCapture, SetActiveWindow, SetCapture,
    };
    use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, WM_CONTEXTMENU};
    std::panic::catch_unwind(|| {
        if message == WM_RBUTTONDOWN {
            // SAFETY: the live native recipient owns this thread's capture during its press.
            unsafe {
                let _previous = SetCapture(window);
            }
        } else if message == WM_RBUTTONUP {
            // SAFETY: this fixture captured the mouse on its matching press.
            if unsafe { ReleaseCapture() }.is_err() {
                return LRESULT(0);
            }
        } else if message == WM_CONTEXTMENU {
            CONTEXT_MENUS.with(|menus| menus.set(menus.get() + 1));
            // SAFETY: the recipient is a live window owned by this fixture's UI thread.
            unsafe {
                let _previous = SetActiveWindow(window);
            }
            return LRESULT(0);
        }
        // SAFETY: forward the live fixture callback's unmodified arguments to the system default.
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    })
    .unwrap_or(LRESULT(0))
}
