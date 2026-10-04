use super::App;
use alttabio::activation::activation_target;
use std::ffi::c_void;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetLastActivePopup, IsIconic, IsWindowVisible, SW_RESTORE,
    SetForegroundWindow, ShowWindowAsync,
};

impl App {
    pub(super) fn activate_target(&mut self, target: isize, reset_hook: bool) {
        let target = HWND(target as *mut c_void);
        if !activate_and_hide(target, || self.hide_overlay_with_reset(reset_hook)) {
            // The completed gesture has released ownership. Reopening here leaves an
            // overlay with no matching Alt release left to dismiss it.
            eprintln!("Could not activate the selected window");
        }
    }
}

pub(super) fn request_foreground(window: HWND) -> bool {
    // SAFETY: window is a borrowed overlay or selected application HWND.
    unsafe { SetForegroundWindow(window).as_bool() }
}

fn activate_and_hide(target: HWND, hide: impl FnOnce()) -> bool {
    // Keep the overlay's foreground permission until the target queue has received the
    // activation request. Hiding first can return foreground ownership to another process.
    let activated = activate_window(target);
    hide();
    activated
}

fn activate_window(owner: HWND) -> bool {
    let popup = unsafe {
        // SAFETY: owner is a borrowed HWND selected from the current EnumWindows snapshot.
        GetLastActivePopup(owner)
    };
    let popup_is_visible = popup != HWND::default()
        && popup != owner
        && unsafe {
            // SAFETY: popup is the borrowed HWND returned by GetLastActivePopup.
            IsWindowVisible(popup).as_bool()
        };
    let target = activation_target(owner, popup, popup_is_visible);

    if unsafe {
        // SAFETY: target is a borrowed top-level or owned-popup HWND.
        IsIconic(target).as_bool()
    } {
        let restore_posted = unsafe {
            // SAFETY: target is a borrowed HWND; ShowWindowAsync does not transfer ownership.
            ShowWindowAsync(target, SW_RESTORE)
        };
        if !restore_posted.as_bool() {
            eprintln!("Could not restore the minimized window before activating it");
        }
    }

    // Separate queues make target activation asynchronous even if the target is hung.
    // Ordinary switching still owns the foreground overlay here; Start/Search switching
    // retains the permission supplied by the physical registered Tab hotkey.
    let activated = request_foreground(target);
    activated
        || unsafe {
            // SAFETY: GetForegroundWindow has no preconditions and returns a borrowed window.
            GetForegroundWindow()
        } == target
}

#[cfg(test)]
#[path = "activation_tests.rs"]
mod activation_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    #[test]
    #[ignore = "requires two responsive desktop windows; changes foreground focus"]
    fn activation_switches_between_foreign_windows() {
        let handles = std::env::var("ALTTABIO_TEST_ACTIVATION_WINDOWS")
            .unwrap_or_else(|_| panic!("Set ALTTABIO_TEST_ACTIVATION_WINDOWS to two HWNDs"));
        let handles: Vec<isize> = handles
            .split(',')
            .map(|value| {
                value
                    .trim()
                    .parse()
                    .unwrap_or_else(|_| panic!("Invalid HWND"))
            })
            .collect();
        assert_eq!(handles.len(), 2);
        assert_ne!(handles[0], handles[1]);
        for handle in handles.iter().cycle().take(10) {
            let window = HWND(*handle as *mut c_void);
            let mut process = 0;
            // SAFETY: the supplied HWND is borrowed and the process output is writable.
            assert_ne!(
                unsafe { GetWindowThreadProcessId(window, Some(&raw mut process)) },
                0
            );
            assert_ne!(
                process,
                std::process::id(),
                "Use windows from other processes"
            );
            assert!(
                activate_window(window),
                "Activation was rejected for {handle}"
            );
            // SetForegroundWindow may return before a foreign input queue processes activation.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            loop {
                // SAFETY: this query has no preconditions and transfers no ownership.
                if unsafe { GetForegroundWindow() } == window {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "Window {handle} never became foreground"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}
