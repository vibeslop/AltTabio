//! The private message and timer IDs of the app window. The hook thread, the tray icon and the
//! window-event callbacks post to that window as well as the app itself, so all its IDs live
//! here, where a test checks that none collide. Messages for the hook thread's queue and for the
//! dialog windows go elsewhere, so `hook` and `dialog_host` may reuse these numbers.

use windows::Win32::UI::WindowsAndMessaging::WM_APP;

// Posted by the hook thread.
pub(crate) const WM_HOOK_ACTION: u32 = WM_APP + 1;
pub(crate) const WM_HOOK_HOTKEY_ACTION: u32 = WM_APP + 21;

// Sent by the Shell on behalf of the tray icon.
pub(crate) const WM_TRAY_CALLBACK: u32 = WM_APP + 2;

// Posted by the window-event callbacks.
pub(crate) const WM_FOREGROUND_CHECK: u32 = WM_APP + 6;
pub(crate) const WM_LISTED_WINDOW_REFRESH: u32 = WM_APP + 7;

// Posted by the app to itself.
pub(crate) const WM_SHOW_SETTINGS: u32 = WM_APP + 3;
pub(crate) const WM_DESTROY_APP: u32 = WM_APP + 4;
pub(crate) const WM_SHOW_ABOUT: u32 = WM_APP + 5;

pub(crate) const CLOSE_REFRESH_TIMER_ID: usize = 1;
pub(crate) const LISTED_REFRESH_RETRY_TIMER_ID: usize = 2;
pub(crate) const SHELL_DISMISS_TIMER_ID: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn app_window_messages_are_distinct() {
        let messages = [
            WM_HOOK_ACTION,
            WM_HOOK_HOTKEY_ACTION,
            WM_TRAY_CALLBACK,
            WM_FOREGROUND_CHECK,
            WM_LISTED_WINDOW_REFRESH,
            WM_SHOW_SETTINGS,
            WM_DESTROY_APP,
            WM_SHOW_ABOUT,
        ];

        assert_eq!(
            messages.iter().collect::<HashSet<_>>().len(),
            messages.len()
        );
    }

    #[test]
    fn app_window_timers_are_distinct() {
        let timers = [
            CLOSE_REFRESH_TIMER_ID,
            LISTED_REFRESH_RETRY_TIMER_ID,
            SHELL_DISMISS_TIMER_ID,
        ];

        assert_eq!(timers.iter().collect::<HashSet<_>>().len(), timers.len());
    }
}
