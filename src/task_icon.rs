use std::collections::HashMap;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GCLP_HICON, GCLP_HICONSM, GetClassLongPtrW, HICON, ICON_BIG, ICON_SMALL,
    ICON_SMALL2, SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_GETICON,
};
use windows::core::{Error, PCWSTR};

#[derive(Default)]
/// Owns executable icons for one task-list snapshot. Keep it alive until that list is replaced.
pub struct TaskIcons {
    icons: HashMap<String, Option<OwnedIcon>>,
}

impl TaskIcons {
    pub fn resolve(&mut self, hwnd: HWND, executable: &str) -> isize {
        let borrowed = window_icon(hwnd);
        if borrowed != 0 || executable.is_empty() {
            return borrowed;
        }
        self.icons
            .entry(executable.to_owned())
            .or_insert_with(|| extract_icon(executable))
            .as_ref()
            .map_or(0, |icon| icon.0.0 as isize)
    }
}

struct OwnedIcon(HICON);

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns an extracted icon, never a window/class icon.
        if let Err(error) = unsafe { DestroyIcon(self.0) } {
            eprintln!("Could not release a task icon: {error}");
        }
    }
}

fn extract_icon(executable: &str) -> Option<OwnedIcon> {
    if executable.contains('\0') {
        return None;
    }
    let path: Vec<u16> = executable.encode_utf16().chain(Some(0)).collect();
    let mut icon = HICON::default();
    // SAFETY: path is terminated and live; icon is writable for the one requested large icon.
    let count = unsafe { ExtractIconExW(PCWSTR(path.as_ptr()), 0, Some(&raw mut icon), None, 1) };
    let error = (count == u32::MAX).then(Error::from_thread);
    let owned = if icon.is_invalid() {
        None
    } else {
        Some(OwnedIcon(icon))
    };
    if let Some(error) = error {
        eprintln!("Could not extract a task icon: {error}");
        return None;
    }
    if count == 0 {
        return None;
    }
    owned
}

fn window_icon(hwnd: HWND) -> isize {
    for size in [ICON_BIG, ICON_SMALL2, ICON_SMALL] {
        let mut icon = 0_usize;
        let sent = unsafe {
            // SAFETY: hwnd is borrowed and icon is writable for this bounded query.
            SendMessageTimeoutW(
                hwnd,
                WM_GETICON,
                WPARAM(size as usize),
                LPARAM(0),
                SMTO_BLOCK | SMTO_ABORTIFHUNG,
                75,
                Some(&raw mut icon),
            )
        };
        if sent.0 != 0 && icon != 0 {
            return isize::try_from(icon).unwrap_or_default();
        }
    }
    for class_index in [GCLP_HICON, GCLP_HICONSM] {
        let icon = unsafe {
            // SAFETY: hwnd is borrowed; class icons remain owned by the registered class.
            GetClassLongPtrW(hwnd, class_index)
        };
        if icon != 0 {
            return isize::try_from(icon).unwrap_or_default();
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        CopyIcon, CreateWindowExW, DestroyWindow, IDI_APPLICATION, LoadIconW, SendMessageW,
        WINDOW_EX_STYLE, WM_SETICON, WS_POPUP,
    };
    use windows::core::{Result, w};

    struct TestWindow(HWND);

    impl TestWindow {
        fn new() -> Result<Self> {
            // SAFETY: STATIC is a built-in class, buffers are static, and this thread owns the
            // hidden test window until TestWindow::drop. No custom callback is installed.
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("Icon test"),
                    WS_POPUP,
                    0,
                    0,
                    10,
                    10,
                    None,
                    None,
                    None,
                    None,
                )
                .map(Self)
            }
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            // SAFETY: this thread uniquely owns the live hidden test window.
            if let Err(error) = unsafe { DestroyWindow(self.0) } {
                eprintln!("Could not destroy the icon test window: {error}");
            }
        }
    }

    #[test]
    fn iconless_window_uses_executable_icon() -> Result<()> {
        let window = TestWindow::new()?;
        assert_eq!(
            window_icon(window.0),
            0,
            "fixture must have no window or class icon"
        );
        let executable = std::env::current_exe().map_err(|error| {
            windows::core::Error::new(
                windows::core::HRESULT(0x8000_4005_u32.cast_signed()),
                error.to_string(),
            )
        })?;
        let mut icons = TaskIcons::default();
        let icon = icons.resolve(window.0, &executable.to_string_lossy());
        assert_ne!(
            icon, 0,
            "an iconless window must display the icon embedded in its executable"
        );
        assert_eq!(icons.resolve(window.0, &executable.to_string_lossy()), icon);
        assert_eq!(
            icons.icons.len(),
            1,
            "rows for the same executable share one owned icon"
        );
        // SAFETY: the cache owns this live icon. CopyIcon creates an independently owned copy.
        let copy = unsafe { CopyIcon(HICON(icon as *mut _)) }?;
        drop(OwnedIcon(copy));
        drop(icons);
        // SAFETY: CopyIcon validates this stale scalar handle; nothing has reused it in between.
        assert!(
            unsafe { CopyIcon(HICON(icon as *mut _)) }.is_err(),
            "the replaced list must release its icons"
        );
        Ok(())
    }

    #[test]
    fn existing_window_icon_stays_borrowed() -> Result<()> {
        let window = TestWindow::new()?;
        // SAFETY: IDI_APPLICATION is a shared system icon; it must not be destroyed by this cache.
        let icon = unsafe { LoadIconW(None, IDI_APPLICATION) }?;
        unsafe {
            // SAFETY: this thread owns window, and icon is a shared resource valid for its lifetime.
            SendMessageW(
                window.0,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
        }
        let mut icons = TaskIcons::default();
        assert_eq!(icons.resolve(window.0, "missing.exe"), icon.0 as isize);
        assert!(
            icons.icons.is_empty(),
            "borrowed window icons never enter the owned cache"
        );
        drop(icons);
        // SAFETY: the shared icon must still be live after dropping the cache.
        let copy = unsafe { CopyIcon(icon) }?;
        drop(OwnedIcon(copy));
        Ok(())
    }

    #[test]
    fn unavailable_executable_leaves_no_owned_icon() -> Result<()> {
        let window = TestWindow::new()?;
        let mut icons = TaskIcons::default();
        assert_eq!(icons.resolve(window.0, ""), 0);
        let missing =
            std::env::temp_dir().join(format!("alttabio-missing-{}.exe", std::process::id()));
        assert_eq!(icons.resolve(window.0, &missing.to_string_lossy()), 0);
        assert!(icons.icons.values().all(Option::is_none));
        Ok(())
    }

    #[test]
    #[ignore = "requires the installed Windows Settings executable; run explicitly on Windows"]
    fn installed_settings_supplies_icon_for_iconless_window() -> Result<()> {
        let window = TestWindow::new()?;
        let windows_dir = std::env::var_os("WINDIR").ok_or_else(Error::from_thread)?;
        let executable =
            std::path::PathBuf::from(windows_dir).join("ImmersiveControlPanel/SystemSettings.exe");
        let mut icons = TaskIcons::default();
        assert_ne!(icons.resolve(window.0, &executable.to_string_lossy()), 0);
        Ok(())
    }
}
