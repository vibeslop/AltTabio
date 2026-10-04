use super::{AppHost, WM_DESTROY_APP, WM_SHOW_ABOUT, WM_SHOW_SETTINGS};
use crate::win_events::{
    self, WM_FOREGROUND_CHECK, WM_LISTED_WINDOW_REFRESH, is_listed_refresh_wakeup,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, LPARAM, LRESULT, SetLastError, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW,
    PostQuitMessage, SetWindowLongPtrW, WM_DESTROY, WM_NCACTIVATE, WM_NCCALCSIZE, WM_NCCREATE,
    WM_NCDESTROY, WM_RBUTTONUP,
};
use windows::core::{Error, Result};

pub(super) unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let handled = catch_unwind(AssertUnwindSafe(|| {
        if message == WM_NCCREATE {
            let create = unsafe {
                // SAFETY: WM_NCCREATE guarantees lParam points to CREATESTRUCTW for this callback.
                (lparam.0 as *const CREATESTRUCTW).as_ref()
            }?;
            let host = create.lpCreateParams.cast::<AppHost>();
            if host.is_null() {
                return Some(LRESULT(0));
            }
            let host_ref = unsafe {
                // SAFETY: host is the Box allocation passed to CreateWindowExW and remains live.
                &*host
            };
            let Ok(mut app) = host_ref.state.try_borrow_mut() else {
                return Some(LRESULT(0));
            };
            app.hwnd = hwnd;
            drop(app);
            // SAFETY: host remains live through the message loop.
            if let Err(error) = unsafe { set_window_user_data(hwnd, host as isize) } {
                // Failing creation lets `run` free the host instead of running a window that
                // can never reach it.
                eprintln!("Could not attach AltTabio to its window: {error}");
                return Some(LRESULT(0));
            }
            return Some(LRESULT(1));
        }
        if message == WM_NCCALCSIZE {
            return Some(LRESULT(0));
        }
        if message == WM_NCACTIVATE {
            return Some(LRESULT(1));
        }
        let host = unsafe {
            // SAFETY: user data is either zero or the live AppHost pointer installed above.
            (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppHost).as_ref()
        }?;
        if message == WM_DESTROY_APP {
            let result = unsafe {
                // SAFETY: the posted message runs on the UI thread that owns hwnd.
                DestroyWindow(hwnd)
            };
            if let Err(error) = result {
                eprintln!("Could not close AltTabio: {error}");
            }
            return Some(LRESULT(0));
        }
        if message == WM_DESTROY {
            if let Ok(mut app) = host.state.try_borrow_mut() {
                app.shutdown();
            }
            unsafe {
                // SAFETY: called on the UI thread to terminate its own message loop.
                PostQuitMessage(0);
            }
            return Some(LRESULT(0));
        }
        if message == WM_NCDESTROY {
            // SAFETY: clearing user data prevents later messages from observing host.
            if let Err(error) = unsafe { set_window_user_data(hwnd, 0) } {
                eprintln!("Could not detach AltTabio from its window: {error}");
            }
            return None;
        }
        if is_modal_dialog_message(message) {
            match message {
                WM_SHOW_SETTINGS => host.show_settings(),
                WM_SHOW_ABOUT => host.show_about(),
                _ => {}
            }
            return Some(LRESULT(0));
        }
        if message == WM_RBUTTONUP {
            host.show_task_context_menu(lparam);
            return Some(LRESULT(0));
        }
        let Ok(mut app) = host.state.try_borrow_mut() else {
            return match busy_overlay_message_action(message, wparam) {
                BusyOverlayMessage::RetryForegroundCheck => {
                    win_events::foreground_check_message_dropped();
                    Some(LRESULT(0))
                }
                BusyOverlayMessage::AcknowledgeDroppedRefresh => {
                    win_events::listed_refresh_message_dropped();
                    Some(LRESULT(0))
                }
                BusyOverlayMessage::IgnoreRetryTick => Some(LRESULT(0)),
                BusyOverlayMessage::DeferToDefault => None,
            };
        };
        app.handle_message(message, wparam, lparam)
    }))
    .ok()
    .flatten();
    handled.unwrap_or_else(|| default_window_proc(hwnd, message, wparam, lparam))
}

/// # Safety
///
/// `value` must be zero or an `AppHost` pointer that stays live until `WM_NCDESTROY` clears it,
/// because `window_proc` dereferences any nonzero user data.
unsafe fn set_window_user_data(hwnd: HWND, value: isize) -> Result<()> {
    unsafe {
        // SAFETY: SetLastError only writes this thread's last-error value.
        SetLastError(ERROR_SUCCESS);
    }
    let previous = unsafe {
        // SAFETY: hwnd is the window being handled, and the caller upholds the contract for the
        // stored value.
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, value)
    };
    // Zero is also what a successful call returns when the previous value was zero, so only a
    // last error separates failure from success.
    if previous == 0 {
        let error = Error::from_thread();
        if error.code().is_err() {
            return Err(error);
        }
    }
    Ok(())
}

const fn is_modal_dialog_message(message: u32) -> bool {
    matches!(message, WM_SHOW_SETTINGS | WM_SHOW_ABOUT)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BusyOverlayMessage {
    RetryForegroundCheck,
    AcknowledgeDroppedRefresh,
    IgnoreRetryTick,
    DeferToDefault,
}

const fn busy_overlay_message_action(message: u32, wparam: WPARAM) -> BusyOverlayMessage {
    if message == WM_FOREGROUND_CHECK {
        BusyOverlayMessage::RetryForegroundCheck
    } else if message == WM_LISTED_WINDOW_REFRESH {
        BusyOverlayMessage::AcknowledgeDroppedRefresh
    } else if is_listed_refresh_wakeup(message, wparam) {
        BusyOverlayMessage::IgnoreRetryTick
    } else {
        BusyOverlayMessage::DeferToDefault
    }
}

fn default_window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        // SAFETY: forwarding unhandled messages with the original values is the window-procedure
        // contract.
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::WM_HOOK_ACTION;
    use crate::win_events::LISTED_REFRESH_RETRY_TIMER_ID;
    use alttabio::task_refresh::{
        RefreshDecision, RetryTimer, TaskListRefresh, apply_listed_refresh_batch,
    };
    use windows::Win32::UI::WindowsAndMessaging::{WM_PAINT, WM_TIMER};

    #[test]
    fn modal_dialog_messages_run_outside_the_app_state_borrow() {
        assert!(is_modal_dialog_message(WM_SHOW_SETTINGS));
        assert!(is_modal_dialog_message(WM_SHOW_ABOUT));
        assert!(!is_modal_dialog_message(WM_HOOK_ACTION));
    }

    #[test]
    fn listed_refresh_messages_are_acknowledged_when_app_state_is_busy() {
        assert_eq!(
            busy_overlay_message_action(WM_FOREGROUND_CHECK, WPARAM(0)),
            BusyOverlayMessage::RetryForegroundCheck
        );
        assert_eq!(
            busy_overlay_message_action(WM_LISTED_WINDOW_REFRESH, WPARAM(0)),
            BusyOverlayMessage::AcknowledgeDroppedRefresh
        );
        assert_eq!(
            busy_overlay_message_action(WM_TIMER, WPARAM(LISTED_REFRESH_RETRY_TIMER_ID)),
            BusyOverlayMessage::IgnoreRetryTick
        );
        assert_eq!(
            busy_overlay_message_action(WM_PAINT, WPARAM(0)),
            BusyOverlayMessage::DeferToDefault
        );
    }

    #[test]
    fn modal_menu_reentry_cannot_lose_a_listed_refresh_notice() {
        use alttabio::task_refresh::{ListedRefreshSignal, RefreshWakeup};

        let signal = ListedRefreshSignal::new();
        let mut refresh = TaskListRefresh::default();

        assert_eq!(signal.record(10), RefreshWakeup::PostNow);
        assert_eq!(
            busy_overlay_message_action(WM_LISTED_WINDOW_REFRESH, WPARAM(0)),
            BusyOverlayMessage::AcknowledgeDroppedRefresh
        );

        signal.post_failed();
        assert!(signal.needs_retry_wakeup());
        assert_eq!(
            refresh.decision(true, |_| true, true),
            RefreshDecision::Ignore
        );

        let batch = signal.take_retry();
        assert!(batch.is_some());
        apply_listed_refresh_batch(
            &mut refresh,
            batch.unwrap_or_else(alttabio::task_refresh::RefreshBatch::empty),
        );
        assert!(!signal.is_queued());
        assert!(!signal.is_dirty());
        assert_eq!(
            refresh.decision(true, |_| true, false),
            RefreshDecision::Refresh
        );
        let stale = [alttabio::switcher::SwitchTask::new(
            1, 10, "Closing", "editor",
        )];
        assert_eq!(refresh.complete_enumeration(Ok(&stale)), RetryTimer::Start);
        assert!(refresh.has_pending_retries());
        assert_eq!(signal.record(20), RefreshWakeup::PostNow);
    }
}
