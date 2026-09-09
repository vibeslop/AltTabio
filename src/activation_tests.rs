//! Native activation checks on a private desktop. The user's desktop is never switched.

use super::*;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows::Win32::System::StationsAndDesktops::{
    CreateDesktopW, DESKTOP_CONTROL_FLAGS, HDESK, SetThreadDesktop,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW,
};

#[test]
#[ignore = "runs isolated native desktop fixtures; invoke explicitly"]
fn native_activation_of_hung_window_does_not_block() {
    let Ok(executable) = std::env::current_exe() else {
        panic!("test executable unavailable")
    };
    let Ok(mut child) = Command::new(executable)
        .args([
            "--ignored",
            "--exact",
            "windows_app::activation_tests::native_activation_fixture",
            "--nocapture",
        ])
        .env("ALTTABIO_ACTIVATION_FIXTURE", "1")
        .creation_flags(0x0800_0000)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        panic!("could not start native activation fixture")
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => break None,
            Err(error) => panic!("could not inspect fixture: {error}"),
        }
    };
    if status.is_none() {
        assert!(child.kill().is_ok(), "could not stop hung fixture");
    }
    let Ok(output) = child.wait_with_output() else {
        panic!("could not collect fixture output")
    };
    assert!(
        status.is_some_and(|status| status.success()),
        "activation failed or blocked the caller:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "child process of the bounded native activation test"]
fn native_activation_fixture() {
    if std::env::var_os("ALTTABIO_ACTIVATION_FIXTURE").is_none() {
        return;
    }
    let name = null_terminated(&format!("AltTabio-activation-test-{}", std::process::id()));
    // SAFETY: name is terminated and borrowed for the call. The desktop belongs to this
    // disposable fixture process, which exits after validation and releases all its objects.
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
    .unwrap_or_else(|error| panic!("could not create isolated desktop: {error}"));
    // SAFETY: this test thread has not created any windows or hooks. This does not switch
    // the input desktop and cannot steal focus from the user's applications.
    unsafe { SetThreadDesktop(desktop) }
        .unwrap_or_else(|error| panic!("could not select fixture desktop: {error}"));
    let source = fixture_window("Source");
    let (sender, receiver) = mpsc::sync_channel(1);
    let desktop_value = desktop.0 as usize;
    std::thread::spawn(move || {
        // SAFETY: the shared desktop remains open for the process lifetime. This fresh
        // thread has created no GUI objects before selecting it.
        unsafe { SetThreadDesktop(HDESK(desktop_value as *mut c_void)) }
            .unwrap_or_else(|error| panic!("could not select target desktop: {error}"));
        let target = fixture_window("Target");
        sender
            .send(target.0 as usize)
            .unwrap_or_else(|error| panic!("could not report target: {error}"));
        loop {
            std::thread::park();
        }
    });
    let target = HWND(
        receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|error| panic!("fixture target unavailable: {error}"))
            as *mut c_void,
    );
    let started = Instant::now();
    println!("activation-start: target created and intentionally not processing messages");
    let _accepted = activate_and_hide(target, || {
        // SAFETY: source belongs to this thread on the isolated test desktop.
        unsafe {
            let _was_visible = ShowWindow(source, SW_HIDE);
        }
    });
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "activation blocked the caller"
    );
    // Deliberately end the fixture, including the unresponsive target thread. Windows
    // releases its desktop/window handles. No user process or desktop was modified.
    std::process::exit(0);
}

fn fixture_window(title: &str) -> HWND {
    let title = null_terminated(title);
    // SAFETY: STATIC is a system class, strings outlive the call, no callbacks or Rust
    // references are retained, and the current thread owns the resulting fixture window.
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            PCWSTR(title.as_ptr()),
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
    .unwrap_or_else(|error| panic!("could not create fixture window: {error}"));
    // SAFETY: this thread owns window. Its desktop is never presented to the user.
    unsafe {
        let _was_visible = ShowWindow(window, SW_SHOWNOACTIVATE);
    }
    window
}
