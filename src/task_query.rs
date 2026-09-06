use crate::process_info::ProcessInfo;
use crate::task_icon::TaskIcons;
use crate::{about_dialog, settings_dialog};
use alttabio::settings::Settings;
use alttabio::switcher::{SwitchTask, WindowEligibility, is_switchable_window};
use std::ffi::c_void;
use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use windows::Win32::Foundation::{HWND, LPARAM, POINT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    HMONITOR, MONITOR_DEFAULTTONEAREST, MonitorFromPoint, MonitorFromWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetCursorPos,
    GetShellWindow, GetWindow, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, Result};

pub struct EnumeratedTasks {
    pub tasks: Vec<SwitchTask>,
    pub icons: TaskIcons,
}

pub fn enumerate_switchable_windows(settings: &Settings) -> Result<EnumeratedTasks> {
    let current_monitor = if settings.monitor.use_current_monitor_filter {
        let mut cursor = POINT::default();
        unsafe {
            // SAFETY: cursor is writable for the synchronous call.
            GetCursorPos(&raw mut cursor)?;
        }
        Some(unsafe {
            // SAFETY: cursor was initialized above and nearest-monitor fallback guarantees a result.
            MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST)
        })
    } else {
        None
    };
    let mut handles = Vec::<HWND>::new();
    unsafe {
        // SAFETY: EnumWindows is synchronous, so `handles` remains exclusively borrowed and live
        // for every invocation of enum_window.
        EnumWindows(
            Some(enum_window),
            LPARAM((&raw mut handles).cast::<c_void>() as isize),
        )?;
    }
    let mut result = EnumeratedTasks {
        tasks: Vec::new(),
        icons: TaskIcons::default(),
    };
    for hwnd in handles {
        if let Some(task) = create_switch_task(
            hwnd,
            std::process::id(),
            current_monitor,
            result.tasks.len(),
            &mut result.icons,
        ) {
            result.tasks.push(task);
        }
    }
    Ok(result)
}

unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let context = unsafe {
        // SAFETY: the caller passes an exclusive live Vec<HWND> for synchronous enumeration.
        (lparam.0 as *mut Vec<HWND>).as_mut()
    };
    let Some(context) = context else {
        return false.into();
    };

    let result = catch_unwind(AssertUnwindSafe(|| {
        context.push(hwnd);
    }));
    result.is_ok().into()
}

fn create_switch_task(
    hwnd: HWND,
    current_process_id: u32,
    current_monitor: Option<HMONITOR>,
    index: usize,
    icons: &mut TaskIcons,
) -> Option<SwitchTask> {
    let title = window_title(hwnd);
    let class_name = window_class_name(hwnd);
    let mut process_id = 0;
    unsafe {
        // SAFETY: `process_id` is writable and HWND is supplied by EnumWindows.
        GetWindowThreadProcessId(hwnd, Some(&raw mut process_id));
    }
    let extended_style = unsafe {
        // SAFETY: HWND is supplied by EnumWindows and GWL_EXSTYLE requests a scalar style value.
        GetWindowLongPtrW(hwnd, GWL_EXSTYLE)
    };
    let shell = unsafe {
        // SAFETY: GetShellWindow has no preconditions and returns a borrowed HWND.
        GetShellWindow()
    };
    let has_owner = unsafe {
        // SAFETY: HWND is supplied by EnumWindows; a missing owner is represented as an error/null.
        GetWindow(hwnd, GW_OWNER).is_ok()
    };
    let matches_monitor_filter = current_monitor.is_none_or(|current_monitor| {
        (unsafe {
            // SAFETY: HWND is supplied by EnumWindows and nearest-monitor fallback is requested.
            MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)
        }) == current_monitor
    });
    let eligibility = WindowEligibility {
        title: &title,
        class_name: &class_name,
        is_visible: unsafe {
            // SAFETY: HWND is supplied by EnumWindows.
            IsWindowVisible(hwnd).as_bool()
        },
        is_current_process: excludes_current_process_window(
            process_id,
            current_process_id,
            &class_name,
        ) || hwnd == shell,
        is_cloaked: is_cloaked(hwnd),
        is_tool_window: (extended_style & isize::try_from(WS_EX_TOOLWINDOW.0).unwrap_or_default())
            != 0,
        has_owner,
        is_app_window: (extended_style & isize::try_from(WS_EX_APPWINDOW.0).unwrap_or_default())
            != 0,
        matches_monitor_filter,
    };
    if !is_switchable_window(&eligibility) {
        return None;
    }

    let process =
        ProcessInfo::query(process_id).unwrap_or_else(|_| ProcessInfo::unavailable(process_id));
    let icon_executable =
        hosted_app_executable(hwnd, &class_name).unwrap_or_else(|| process.executable.clone());
    Some(
        SwitchTask::new(
            index + 1,
            hwnd.0 as isize,
            &title,
            process.executable_stem(),
        )
        .with_process_identity(process.identity)
        .with_icon_handle(icons.resolve(hwnd, &icon_executable.to_string_lossy())),
    )
}

fn excludes_current_process_window(
    process_id: u32,
    current_process_id: u32,
    class_name: &str,
) -> bool {
    process_id == current_process_id
        && !matches!(
            class_name,
            about_dialog::WINDOW_CLASS_NAME | settings_dialog::WINDOW_CLASS_NAME
        )
}

fn hosted_app_executable(hwnd: HWND, class_name: &str) -> Option<PathBuf> {
    if class_name != "ApplicationFrameWindow" {
        return None;
    }
    let mut children = Vec::<HWND>::new();
    unsafe {
        // SAFETY: children is exclusively borrowed for synchronous callbacks. EnumChildWindows'
        // return value is documented as unused; the callback only collects borrowed handles.
        let _unused = EnumChildWindows(
            Some(hwnd),
            Some(enum_window),
            LPARAM((&raw mut children).cast::<c_void>() as isize),
        );
    }
    for child in children {
        if window_class_name(child) != "Windows.UI.Core.CoreWindow" {
            continue;
        }
        let mut process_id = 0;
        unsafe {
            // SAFETY: child is borrowed; process_id is writable for this synchronous query.
            GetWindowThreadProcessId(child, Some(&raw mut process_id));
        }
        if let Ok(process) = ProcessInfo::query(process_id) {
            return Some(process.executable);
        }
    }
    None
}

fn window_title(hwnd: HWND) -> String {
    let length = unsafe {
        // SAFETY: HWND is supplied by EnumWindows.
        GetWindowTextLengthW(hwnd)
    };
    if length <= 0 {
        return String::new();
    }
    let capacity = usize::try_from(length)
        .unwrap_or_default()
        .saturating_add(1);
    let mut buffer = vec![0_u16; capacity];
    let written = unsafe {
        // SAFETY: `buffer` is writable and HWND is supplied by EnumWindows.
        GetWindowTextW(hwnd, &mut buffer)
    };
    utf16_prefix(&buffer, written)
}

pub fn window_class_name(hwnd: HWND) -> String {
    let mut buffer = [0_u16; 256];
    let written = unsafe {
        // SAFETY: `buffer` is writable and HWND is supplied by EnumWindows.
        GetClassNameW(hwnd, &mut buffer)
    };
    utf16_prefix(&buffer, written)
}

fn utf16_prefix(buffer: &[u16], written: i32) -> String {
    let length = usize::try_from(written).unwrap_or_default();
    String::from_utf16_lossy(buffer.get(..length).unwrap_or_default())
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0_u32;
    let result = unsafe {
        // SAFETY: `cloaked` is writable for its exact byte size and HWND is supplied by EnumWindows.
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            u32::try_from(size_of::<u32>()).unwrap_or_default(),
        )
    };
    result.is_ok() && cloaked != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{HINSTANCE, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, WNDCLASSEXW, WS_POPUP,
    };
    use windows::core::{Error, w};
    #[test]
    fn hosted_frame_uses_core_window_executable() -> Result<()> {
        use windows::Win32::UI::WindowsAndMessaging::{
            UnregisterClassW, WINDOW_EX_STYLE, WS_CHILD,
        };

        struct TestClass(HINSTANCE);
        impl Drop for TestClass {
            fn drop(&mut self) {
                // SAFETY: all windows of this test-owned class have been dropped first.
                if let Err(error) =
                    unsafe { UnregisterClassW(w!("Windows.UI.Core.CoreWindow"), Some(self.0)) }
                {
                    eprintln!("Could not unregister the test class: {error}");
                }
            }
        }
        struct TestWindow(HWND);
        impl Drop for TestWindow {
            fn drop(&mut self) {
                // SAFETY: the test owns this window on the current thread.
                if let Err(error) = unsafe { DestroyWindow(self.0) } {
                    eprintln!("Could not destroy the test window: {error}");
                }
            }
        }

        unsafe extern "system" fn test_proc(
            hwnd: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            catch_unwind(|| {
                // SAFETY: Windows supplies this procedure's arguments; no Rust window state exists.
                unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            })
            .unwrap_or_default()
        }
        // SAFETY: querying this process module transfers no ownership.
        let instance = HINSTANCE(unsafe { GetModuleHandleW(None) }?.0);
        let class = WNDCLASSEXW {
            cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or_default(),
            lpfnWndProc: Some(test_proc),
            hInstance: instance,
            lpszClassName: w!("Windows.UI.Core.CoreWindow"),
            ..Default::default()
        };
        // SAFETY: the callback contains panics and forwards to Windows; the class name is static.
        if unsafe { RegisterClassExW(&raw const class) } == 0 {
            return Err(Error::from_thread());
        }
        let _class = TestClass(instance);
        // SAFETY: this thread owns both hidden windows; their guards destroy child before parent.
        let parent = TestWindow(unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("Frame test"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                Some(instance),
                None,
            )?
        });
        assert!(hosted_app_executable(parent.0, "ApplicationFrameWindow").is_none());
        // SAFETY: parent is live; the child class is registered above and remains live until drop.
        let _child = TestWindow(unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class.lpszClassName,
                w!("App test"),
                WS_CHILD,
                0,
                0,
                10,
                10,
                Some(parent.0),
                None,
                Some(instance),
                None,
            )?
        });
        let executable = ProcessInfo::query(std::process::id())?.executable;
        assert!(!executable.as_os_str().is_empty());
        assert_eq!(
            hosted_app_executable(parent.0, "ApplicationFrameWindow"),
            Some(executable)
        );
        assert!(hosted_app_executable(parent.0, "OtherWindowClass").is_none());
        Ok(())
    }

    #[test]
    fn own_visible_dialogs_are_not_excluded_from_switching() {
        let current_process_id = 42;

        assert!(!excludes_current_process_window(
            current_process_id,
            current_process_id,
            about_dialog::WINDOW_CLASS_NAME,
        ));
        assert!(!excludes_current_process_window(
            current_process_id,
            current_process_id,
            settings_dialog::WINDOW_CLASS_NAME,
        ));
        assert!(excludes_current_process_window(
            current_process_id,
            current_process_id,
            "AltTabioRustOverlay",
        ));
        assert!(!excludes_current_process_window(
            current_process_id + 1,
            current_process_id,
            "EditorWindow",
        ));
    }
}
