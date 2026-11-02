//! A busy application must not hold up the switcher's task snapshot.

use super::*;
use std::panic::catch_unwind;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::LRESULT;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, IDI_APPLICATION, LoadIconW, RegisterClassExW,
    UnregisterClassW, WINDOW_EX_STYLE, WNDCLASSEXW, WS_POPUP,
};

struct BusyWindow {
    hwnd: HWND,
    stop: mpsc::SyncSender<()>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl BusyWindow {
    fn new() -> Self {
        let (ready, window) = mpsc::sync_channel(1);
        let (stop, stopped) = mpsc::sync_channel(1);
        let thread = thread::spawn(move || -> Result<()> {
            let instance = crate::win32::module_instance()?;
            // SAFETY: this shared stock icon remains valid for the class's lifetime.
            let icon = unsafe { LoadIconW(None, IDI_APPLICATION) }?;
            let class = WNDCLASSEXW {
                cbSize: u32::try_from(std::mem::size_of::<WNDCLASSEXW>()).unwrap_or_default(),
                lpfnWndProc: Some(busy_window_proc),
                hInstance: instance,
                hIcon: icon,
                lpszClassName: w!("AltTabioBusyIconTest"),
                ..Default::default()
            };
            // SAFETY: the class has a static name and a panic-contained callback.
            if unsafe { RegisterClassExW(&raw const class) } == 0 {
                return Err(Error::from_thread());
            }
            // SAFETY: this worker owns the hidden window; no user input or focus is changed.
            let created = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    class.lpszClassName,
                    w!("Busy application"),
                    WS_POPUP,
                    0,
                    0,
                    10,
                    10,
                    None,
                    None,
                    Some(instance),
                    None,
                )
            };
            if let Ok(hwnd) = created {
                assert!(ready.send(hwnd.0 as isize).is_ok());
                // Model an application's UI thread occupied by a render/load loop. It does not
                // dispatch icon queries until the test signals cleanup on this separate channel.
                assert!(stopped.recv().is_ok());
                // SAFETY: this thread owns the window and destroys it once before its class.
                unsafe { DestroyWindow(hwnd) }?;
            }
            // SAFETY: the class belongs to this worker and has no surviving windows.
            unsafe { UnregisterClassW(class.lpszClassName, Some(instance)) }?;
            created.map(|_| ())
        });
        let hwnd = window
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|error| panic!("busy icon fixture unavailable: {error}"));
        Self {
            hwnd: HWND(hwnd as *mut _),
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for BusyWindow {
    fn drop(&mut self) {
        assert!(self.stop.send(()).is_ok());
        if let Some(thread) = self.thread.take() {
            assert!(thread.join().is_ok_and(|result| result.is_ok()));
        }
    }
}

unsafe extern "system" fn busy_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    catch_unwind(|| {
        // SAFETY: Windows supplies these arguments; the fixture holds no Rust window state.
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    })
    .unwrap_or_default()
}

#[test]
fn busy_application_icon_does_not_stall_the_task_snapshot() {
    let window = BusyWindow::new();
    let started = Instant::now();
    let mut icons = TaskIcons::default();
    let icon = icons.resolve(window.hwnd, "");
    let elapsed = started.elapsed();
    assert_ne!(icon, 0, "a busy app must keep its available class icon");
    assert!(
        elapsed < Duration::from_millis(100),
        "one busy application's icon stalled the task snapshot for {elapsed:?}"
    );
}
