//! A private-desktop regression for a late close-refresh callback between Alt and Tab.

use super::*;
use crate::app_messages::CLOSE_REFRESH_TIMER_ID;
use crate::win32::ComApartment;
use alttabio::hook_flags::OVERLAY_FLAGS;
use alttabio::input::{HookSettings, HookState, InputAction, Key, KeyEvent, Modifiers};
use alttabio::switcher::{SwitchTask, WindowCommandRequest};
use alttabio::task_refresh::{RefreshDecision, RetryTimer};
use alttabio::window_command::WindowCommand;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use windows::Win32::System::StationsAndDesktops::{
    CreateDesktopW, DESKTOP_CONTROL_FLAGS, SetThreadDesktop,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SetTimer, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW,
};
use windows::core::{PCWSTR, w};

#[test]
fn closed_session_refresh_preserves_fresh_alt_tab() {
    let mut child = Command::new(std::env::current_exe().unwrap_or_else(|e| panic!("{e}")))
        .args([
            "--ignored",
            "--exact",
            "windows_app::refresh_tests::close_refresh_fixture",
            "--nocapture",
        ])
        .env("ALTTABIO_CLOSE_REFRESH_FIXTURE", "1")
        .creation_flags(0x0800_0000)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("{e}"));
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => break None,
            Err(e) => panic!("{e}"),
        }
    };
    if status.is_none() {
        child.kill().unwrap_or_else(|e| panic!("{e}"));
    }
    let output = child.wait_with_output().unwrap_or_else(|e| panic!("{e}"));
    assert!(
        status.is_some_and(|s| s.success()),
        "close-refresh regression failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "child process of the bounded private-desktop regression"]
fn close_refresh_fixture() {
    if std::env::var_os("ALTTABIO_CLOSE_REFRESH_FIXTURE").is_none() {
        return;
    }
    let name = crate::win32::wide(&format!("AltTabio-close-refresh-{}", std::process::id()));
    // SAFETY: this disposable process owns the desktop, which is never switched to input.
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
    .unwrap_or_else(|e| panic!("{e}"));
    // SAFETY: this test thread has created no GUI objects before selecting its desktop.
    unsafe { SetThreadDesktop(desktop) }.unwrap_or_else(|e| panic!("{e}"));
    let _com = ComApartment::initialize().unwrap_or_else(|e| panic!("{e}"));
    let (store, mut settings) = SettingsStore::load_from(
        std::env::temp_dir().join(format!(
            "alttabio-refresh-fixture-{}.ini",
            std::process::id()
        )),
        &Settings::default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    settings.appearance.theme = alttabio::settings::Theme::Light;
    let mut app = App::new(false, false, settings, store).unwrap_or_else(|e| panic!("{e}"));
    app.hwnd = fixture_window();
    for complete in [true, false] {
        check_fresh_gesture(&mut app, complete);
    }
    // Exit releases the private desktop, queued closes and all fixture-owned windows.
    std::process::exit(0);
}

fn fixture_window() -> HWND {
    // SAFETY: STATIC is a system class; no callback or user data is retained by the window.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            w!("Close refresh fixture"),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            320,
            200,
            None,
            None,
            None,
            None,
        )
    }
    .unwrap_or_else(|e| panic!("{e}"))
}

fn check_fresh_gesture(app: &mut App, complete: bool) {
    let (owner, flags) = crate::hook::test_ui_owner();
    app.hooks = Some(owner);
    let target = fixture_window();
    let tasks = [SwitchTask::new(1, target.0 as isize, "Fixture", "fixture")];
    app.session.open(tasks.clone(), None);
    let outcome = app.execute_window_command(WindowCommandRequest {
        command: WindowCommand::Close,
        window_handle: target.0 as isize,
        process_identity: crate::process_info::ProcessInfo::query(std::process::id())
            .unwrap_or_else(|e| panic!("{e}"))
            .identity,
    });
    app.task_refresh.apply_command_outcome(outcome);
    app.sync_hook_interception();
    // Leave the posted close undelivered, modelling a slow/refused close whose HWND survives.
    assert_eq!(
        app.task_refresh.complete_enumeration(Ok(&tasks)),
        RetryTimer::Start
    );
    // SAFETY: the live fixture overlay owns this timer, as in a production close refresh.
    assert_ne!(
        unsafe { SetTimer(Some(app.hwnd), CLOSE_REFRESH_TIMER_ID, 250, None) },
        0
    );
    if complete {
        app.finish_overlay();
    } else {
        app.hide_overlay();
    }

    let settings = HookSettings::default();
    let mut input = HookState::default();
    input.set_interception_suspended(false);
    let generation = flags.load() & !OVERLAY_FLAGS;
    assert!(
        input
            .process_key(
                KeyEvent::pressed(Key::LeftAlt, Modifiers::default()),
                settings
            )
            .suppress
    );
    app.handle_close_refresh_timer();
    if flags.load() & !OVERLAY_FLAGS != generation {
        input.set_interception_suspended(false);
    }
    let tab = input.process_key(
        KeyEvent::pressed(
            Key::Tab,
            Modifiers {
                alt: true,
                ..Modifiers::default()
            },
        ),
        settings,
    );
    assert_eq!(
        tab.actions().collect::<Vec<_>>(),
        [InputAction::Switch(1)],
        "hidden close refresh cancelled the fresh Alt+Tab and replayed it to Windows"
    );
    assert!(tab.suppress);
    assert!(input.take_replayed_key_events().iter().all(Option::is_none));
    assert!(flags.generation_is_current(generation));
    assert!(!app.task_refresh.has_pending_retries());
    assert_eq!(
        app.task_refresh.decision(false, |_| true, false),
        RefreshDecision::Ignore
    );
    app.task_refresh.request_enumeration();
    app.run_pending_task_refresh();
    assert!(flags.generation_is_current(generation));
    drop(app.hooks.take());
}
