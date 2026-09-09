use crate::hook;
use crate::process_info::ProcessInfo;
use crate::task_query::window_class_name;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible,
};

pub fn foreground_menu() -> Option<HWND> {
    remaining_foreground_menu(HWND::default())
}

pub fn remaining_foreground_menu(known_menu: HWND) -> Option<HWND> {
    // SAFETY: this query transfers no ownership and has no preconditions.
    let window = unsafe { GetForegroundWindow() };
    foreground_menu_with(window, known_menu, is_presented, is_menu_window)
}

fn foreground_menu_with(
    window: HWND,
    known_menu: HWND,
    is_presented: impl FnOnce(HWND) -> bool,
    is_menu: impl FnOnce(HWND) -> bool,
) -> Option<HWND> {
    // Start can cloak itself without changing GetForegroundWindow. Reusing its identity must
    // never bypass the visibility check, or every new Tab waits for an already-closed menu.
    (window != HWND::default() && (window == known_menu || is_menu(window)) && is_presented(window))
        .then_some(window)
}

fn is_presented(window: HWND) -> bool {
    // SAFETY: this read-only query accepts a borrowed HWND, including one that has expired.
    if !unsafe { IsWindowVisible(window) }.as_bool() {
        return false;
    }
    let mut cloaked = 0_u32;
    // SAFETY: window is borrowed and cloaked is writable for the exact size supplied.
    if let Err(error) =
        unsafe { DwmGetWindowAttribute(window, DWMWA_CLOAKED, (&raw mut cloaked).cast(), 4) }
    {
        eprintln!("Could not query Start/Search visibility: {error}");
        return true;
    }
    cloaked == 0
}

fn is_menu_window(window: HWND) -> bool {
    if window == HWND::default() || window_class_name(window) != "Windows.UI.Core.CoreWindow" {
        return false;
    }
    let mut process_id = 0;
    // SAFETY: window is borrowed and process_id remains writable during the query.
    unsafe {
        GetWindowThreadProcessId(window, Some(&raw mut process_id));
    }
    let process = match ProcessInfo::query(process_id) {
        Ok(process) => process,
        Err(error) => {
            eprintln!("Could not identify the foreground CoreWindow process: {error}");
            return false;
        }
    };
    is_menu_host(process.executable_stem())
}

fn is_menu_host(name: &str) -> bool {
    name.eq_ignore_ascii_case("SearchHost") || name.eq_ignore_ascii_case("StartMenuExperienceHost")
}

pub fn dismiss(window: HWND) -> Result<(), String> {
    if foreground_menu() != Some(window) {
        return Ok(());
    }
    // SAFETY: the query returns a borrowed HWND; a changed foreground must receive no Escape.
    if unsafe { GetForegroundWindow() } != window {
        return Ok(());
    }
    hook::send_shell_escape()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alttabio::deferred_switch::{DeferredSwitch, DeferredSwitchPoll};
    use alttabio::input::InputAction;
    use std::time::Duration;

    #[test]
    fn closed_start_with_unchanged_foreground_handle_releases_queued_activation() {
        let start = HWND(0xD0D1C_usize as *mut core::ffi::c_void);
        let mut deferred = DeferredSwitch::new(InputAction::Switch(1));
        assert_eq!(deferred.take_preview_actions(), [InputAction::Switch(1)]);
        assert!(deferred.push(InputAction::AltReleased));
        let remaining = foreground_menu_with(
            start,
            start,
            |_| false,
            |_| panic!("The known Start window does not need another process lookup"),
        );
        assert_eq!(
            deferred.poll(remaining.is_some(), Duration::from_millis(32)),
            DeferredSwitchPoll::Ready(vec![InputAction::AltReleased]),
            "A hidden or cloaked Start must not retain the overlay until its timeout"
        );
    }

    #[test]
    fn closed_start_does_not_begin_another_dismissal_when_it_still_has_foreground() {
        let start = HWND(0xD0D1C_usize as *mut core::ffi::c_void);
        assert_eq!(
            foreground_menu_with(start, HWND::default(), |_| false, |_| true),
            None
        );
    }

    #[test]
    fn visible_start_and_replacement_search_still_block_activation() {
        let start = HWND(0x100_usize as *mut core::ffi::c_void);
        let search = HWND(0x200_usize as *mut core::ffi::c_void);
        assert_eq!(
            foreground_menu_with(start, start, |_| true, |_| false),
            Some(start)
        );
        assert_eq!(
            foreground_menu_with(search, start, |_| true, |_| true),
            Some(search)
        );
        assert_eq!(
            foreground_menu_with(search, start, |_| true, |_| false),
            None
        );
        assert_eq!(
            foreground_menu_with(HWND::default(), start, |_| true, |_| true),
            None
        );
    }

    #[test]
    fn shell_dismissal_does_not_target_arbitrary_core_windows() {
        assert!(is_menu_host("SearchHost"));
        assert!(is_menu_host("STARTMENUEXPERIENCEHOST"));
        for other in [
            "ApplicationFrameHost",
            "ShellExperienceHost",
            "SystemSettings",
            "explorer",
            "SearchIndexer",
        ] {
            assert!(!is_menu_host(other));
        }
    }
}
