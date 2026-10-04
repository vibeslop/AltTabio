use super::App;
use crate::process_info::ProcessInfo;
use crate::task_query::window_class_name;
use crate::win_events;
use alttabio::passthrough::{PassthroughPolicy, is_remote_desktop_client, window_fills_monitor};
use std::mem::size_of;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsZoomed,
};

impl App {
    pub(super) fn handle_foreground_check(&mut self) {
        win_events::acknowledge_foreground_check();
        let policy = foreground_passthrough_policy(self.hwnd);
        if policy.bypasses_local_switching() && (self.is_visible() || self.pending_shell.is_some())
        {
            self.hide_overlay();
        }
        let Some(hooks) = self.hooks.as_ref() else {
            return;
        };
        if let Err(error) = hooks.set_remote_desktop_passthrough(policy) {
            eprintln!("{error}");
        }
    }
}

pub(super) fn foreground_passthrough_policy(overlay: HWND) -> PassthroughPolicy {
    let hwnd = unsafe {
        // SAFETY: GetForegroundWindow has no pointer preconditions.
        GetForegroundWindow()
    };
    if hwnd.0.is_null() || hwnd == overlay {
        return PassthroughPolicy::Local;
    }
    let class_name = window_class_name(hwnd);
    let mut process_id = 0_u32;
    let thread_id = unsafe {
        // SAFETY: process_id is writable and hwnd is the live foreground window.
        GetWindowThreadProcessId(hwnd, Some(&raw mut process_id))
    };
    // A process that refuses the query or has exited leaves the executable unknown. The window
    // class still identifies remote desktop clients, so the policy proceeds without it.
    let process = if thread_id == 0 {
        ProcessInfo::unavailable(process_id)
    } else {
        ProcessInfo::query(process_id).unwrap_or_else(|_| ProcessInfo::unavailable(process_id))
    };
    PassthroughPolicy::from_foreground(
        is_remote_desktop_client(&class_name, process.executable_stem()),
        is_maximized_or_fullscreen(hwnd),
    )
}

fn is_maximized_or_fullscreen(hwnd: HWND) -> bool {
    if unsafe {
        // SAFETY: hwnd is the live foreground window.
        IsZoomed(hwnd)
    }
    .as_bool()
    {
        return true;
    }
    let mut window = RECT::default();
    if unsafe {
        // SAFETY: `window` is writable and hwnd is a live top-level window.
        GetWindowRect(hwnd, &raw mut window)
    }
    .is_err()
    {
        return false;
    }
    let monitor = unsafe {
        // SAFETY: hwnd is live and nearest-monitor fallback is requested.
        MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)
    };
    let mut monitor_info = MONITORINFO {
        cbSize: u32::try_from(size_of::<MONITORINFO>()).unwrap_or_default(),
        ..MONITORINFO::default()
    };
    let success = unsafe {
        // SAFETY: `monitor_info` is writable with a correct cbSize.
        GetMonitorInfoW(monitor, &raw mut monitor_info)
    };
    success.as_bool()
        && window_fills_monitor(
            [window.left, window.top, window.right, window.bottom],
            [
                monitor_info.rcMonitor.left,
                monitor_info.rcMonitor.top,
                monitor_info.rcMonitor.right,
                monitor_info.rcMonitor.bottom,
            ],
        )
}
