use alttabio::task_refresh::{ListedRefreshSignal, RefreshBatch, request_listed_refresh};
use std::ffi::c_void;
use std::panic::catch_unwind;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE,
    EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART,
    EVENT_SYSTEM_MOVESIZEEND, OBJID_WINDOW, PostMessageW, WINEVENT_OUTOFCONTEXT, WM_APP, WM_TIMER,
};
use windows::core::Error;

pub const WM_FOREGROUND_CHECK: u32 = WM_APP + 6;
pub const WM_LISTED_WINDOW_REFRESH: u32 = WM_APP + 7;
pub const LISTED_REFRESH_RETRY_TIMER_ID: usize = 2;
pub const LISTED_REFRESH_RETRY_DELAY_MS: u32 = 50;

static WIN_EVENT_NOTIFY_HWND: AtomicIsize = AtomicIsize::new(0);
static FOREGROUND_CHECK_QUEUED: AtomicBool = AtomicBool::new(false);
static LISTED_REFRESH_SIGNAL: ListedRefreshSignal = ListedRefreshSignal::new();

pub struct WinEventWatcher {
    hooks: Vec<HWINEVENTHOOK>,
}

impl WinEventWatcher {
    pub fn install() -> std::result::Result<Self, String> {
        const RANGES: [(u32, u32); 6] = [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZEEND),
            (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
            (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
            (EVENT_OBJECT_DESTROY, EVENT_OBJECT_DESTROY),
            (EVENT_OBJECT_HIDE, EVENT_OBJECT_HIDE),
        ];
        let mut hooks = Vec::new();
        for (min_event, max_event) in RANGES {
            let hook = unsafe {
                // SAFETY: `win_event_proc` has the required ABI, is panic-contained, and only
                // performs fixed-cost filtering plus lock-free bounded atomic updates before
                // posting at most one coalesced message. The watcher is dropped after
                // WIN_EVENT_NOTIFY_HWND is cleared during App::shutdown.
                SetWinEventHook(
                    min_event,
                    max_event,
                    None,
                    Some(win_event_proc),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                )
            };
            if hook.is_invalid() {
                for installed in hooks.drain(..) {
                    unhook_win_event(installed);
                }
                return Err(Error::from_thread().to_string());
            }
            hooks.push(hook);
        }
        Ok(Self { hooks })
    }

    #[must_use]
    pub fn install_failure_message(error: &str) -> String {
        format!(
            "Could not watch window events: {error}\n\nLive window-list updates and Remote Desktop passthrough tracking are disabled until AltTabio is restarted."
        )
    }
}

impl Drop for WinEventWatcher {
    fn drop(&mut self) {
        for hook in self.hooks.drain(..) {
            unhook_win_event(hook);
        }
    }
}

pub fn publish_notify_hwnd(hwnd: HWND) {
    WIN_EVENT_NOTIFY_HWND.store(hwnd.0 as isize, Ordering::Release);
}

pub fn clear_notify_hwnd() {
    WIN_EVENT_NOTIFY_HWND.store(0, Ordering::Release);
    FOREGROUND_CHECK_QUEUED.store(false, Ordering::Release);
    let _ = take_listed_refresh_notices();
}

pub fn take_listed_refresh_notices() -> RefreshBatch {
    LISTED_REFRESH_SIGNAL.take()
}

pub fn take_listed_refresh_retry() -> Option<RefreshBatch> {
    LISTED_REFRESH_SIGNAL.take_retry()
}

pub fn listed_refresh_message_dropped() {
    LISTED_REFRESH_SIGNAL.post_failed();
}

#[must_use]
pub const fn is_listed_refresh_wakeup(message: u32, wparam: WPARAM) -> bool {
    message == WM_LISTED_WINDOW_REFRESH
        || (message == WM_TIMER && wparam.0 == LISTED_REFRESH_RETRY_TIMER_ID)
}

pub fn acknowledge_foreground_check() {
    FOREGROUND_CHECK_QUEUED.store(false, Ordering::Release);
}

fn unhook_win_event(hook: HWINEVENTHOOK) {
    let result = unsafe {
        // SAFETY: `hook` was returned by SetWinEventHook on this UI thread and is removed once.
        UnhookWinEvent(hook)
    };
    if !result.as_bool() {
        eprintln!(
            "Could not remove the window event watcher: {}",
            Error::from_thread()
        );
    }
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    let _ = catch_unwind(|| {
        if should_request_listed_window_refresh(event, hwnd, object, child) {
            request_listed_window_refresh(hwnd);
            return;
        }
        if should_request_foreground_check(event, hwnd, object, child) {
            request_foreground_check();
        }
    });
}

fn should_request_foreground_check(event: u32, hwnd: HWND, object: i32, child: i32) -> bool {
    match event {
        EVENT_SYSTEM_FOREGROUND
        | EVENT_SYSTEM_MOVESIZEEND
        | EVENT_SYSTEM_MINIMIZESTART
        | EVENT_SYSTEM_MINIMIZEEND => true,
        EVENT_OBJECT_LOCATIONCHANGE => is_window_self_event(hwnd, object, child),
        _ => false,
    }
}

fn should_request_listed_window_refresh(event: u32, hwnd: HWND, object: i32, child: i32) -> bool {
    matches!(event, EVENT_OBJECT_HIDE | EVENT_OBJECT_DESTROY)
        && is_window_self_event(hwnd, object, child)
}

fn is_window_self_event(hwnd: HWND, object: i32, child: i32) -> bool {
    !hwnd.0.is_null()
        && object == OBJID_WINDOW.0
        && i32::try_from(CHILDID_SELF).is_ok_and(|child_self| child == child_self)
}

fn request_foreground_check() {
    let target = WIN_EVENT_NOTIFY_HWND.load(Ordering::Acquire);
    if target == 0 {
        return;
    }
    if FOREGROUND_CHECK_QUEUED.swap(true, Ordering::AcqRel) {
        return;
    }
    let hwnd = HWND(target as *mut c_void);
    unsafe {
        // SAFETY: `hwnd` is the overlay window published before the winevent hook is installed
        // and cleared before shutdown unhooks it. PostMessageW copies the integer payloads.
        if PostMessageW(Some(hwnd), WM_FOREGROUND_CHECK, WPARAM(0), LPARAM(0)).is_err() {
            FOREGROUND_CHECK_QUEUED.store(false, Ordering::Release);
        }
    }
}

fn request_listed_window_refresh(hwnd: HWND) {
    request_listed_refresh(&LISTED_REFRESH_SIGNAL, hwnd.0 as isize, post_listed_refresh);
}

fn post_listed_refresh() -> bool {
    let target = WIN_EVENT_NOTIFY_HWND.load(Ordering::Acquire);
    if target == 0 {
        return false;
    }
    let overlay = HWND(target as *mut c_void);
    unsafe {
        // SAFETY: `overlay` is the overlay window published before the winevent hook is installed
        // and cleared before shutdown unhooks it. The payload is empty; affected HWNDs live in the
        // bounded atomic slots.
        PostMessageW(
            Some(overlay),
            WM_LISTED_WINDOW_REFRESH,
            WPARAM(0),
            LPARAM(0),
        )
        .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        EVENT_OBJECT_SHOW, OBJID_CARET, OBJID_CLIENT, OBJID_CURSOR,
    };

    fn listed_window_hwnd() -> HWND {
        HWND(0x100usize as *mut c_void)
    }

    fn window_self_ids() -> (i32, i32) {
        (
            OBJID_WINDOW.0,
            i32::try_from(CHILDID_SELF).unwrap_or_default(),
        )
    }

    #[test]
    fn window_hide_and_destroy_request_listed_refresh() {
        let hwnd = listed_window_hwnd();
        let (object, child) = window_self_ids();

        assert!(should_request_listed_window_refresh(
            EVENT_OBJECT_HIDE,
            hwnd,
            object,
            child
        ));
        assert!(should_request_listed_window_refresh(
            EVENT_OBJECT_DESTROY,
            hwnd,
            object,
            child
        ));
    }

    #[test]
    fn child_object_show_and_unrelated_events_do_not_request_listed_refresh() {
        let hwnd = listed_window_hwnd();
        let (object, child) = window_self_ids();

        assert!(!should_request_listed_window_refresh(
            EVENT_OBJECT_HIDE,
            hwnd,
            OBJID_CLIENT.0,
            child
        ));
        assert!(!should_request_listed_window_refresh(
            EVENT_OBJECT_DESTROY,
            hwnd,
            object,
            1
        ));
        assert!(!should_request_listed_window_refresh(
            EVENT_OBJECT_SHOW,
            hwnd,
            object,
            child
        ));
        assert!(!should_request_listed_window_refresh(
            EVENT_SYSTEM_FOREGROUND,
            hwnd,
            object,
            child
        ));
        assert!(!should_request_listed_window_refresh(
            EVENT_OBJECT_LOCATIONCHANGE,
            hwnd,
            object,
            child
        ));
        assert!(!should_request_listed_window_refresh(
            EVENT_OBJECT_HIDE,
            HWND::default(),
            object,
            child
        ));
    }

    #[test]
    fn window_self_location_change_posts_a_coalesced_foreground_check() {
        let hwnd = listed_window_hwnd();
        let (object, child) = window_self_ids();

        assert!(should_request_foreground_check(
            EVENT_OBJECT_LOCATIONCHANGE,
            hwnd,
            object,
            child
        ));
        assert!(should_request_foreground_check(
            EVENT_SYSTEM_FOREGROUND,
            hwnd,
            object,
            child
        ));
    }

    #[test]
    fn caret_and_cursor_location_changes_are_ignored() {
        let hwnd = listed_window_hwnd();
        let child = window_self_ids().1;

        assert!(!should_request_foreground_check(
            EVENT_OBJECT_LOCATIONCHANGE,
            hwnd,
            OBJID_CARET.0,
            child
        ));
        assert!(!should_request_foreground_check(
            EVENT_OBJECT_LOCATIONCHANGE,
            hwnd,
            OBJID_CURSOR.0,
            child
        ));
        assert!(!should_request_foreground_check(
            EVENT_OBJECT_LOCATIONCHANGE,
            HWND::default(),
            OBJID_WINDOW.0,
            child
        ));
    }

    #[test]
    fn win_event_install_failure_is_a_visible_degraded_mode_error() {
        let message = WinEventWatcher::install_failure_message("access denied");
        assert!(message.contains("Could not watch window events: access denied"));
        assert!(message.contains("Live window-list updates"));
        assert!(message.contains("Remote Desktop passthrough tracking"));
        assert!(message.contains("disabled"));
    }

    #[test]
    fn listed_refresh_wakeup_covers_the_posted_message_and_retry_timer() {
        assert!(is_listed_refresh_wakeup(
            WM_LISTED_WINDOW_REFRESH,
            WPARAM(0)
        ));
        assert!(is_listed_refresh_wakeup(
            WM_TIMER,
            WPARAM(LISTED_REFRESH_RETRY_TIMER_ID)
        ));
        assert!(!is_listed_refresh_wakeup(WM_FOREGROUND_CHECK, WPARAM(0)));
    }
}
