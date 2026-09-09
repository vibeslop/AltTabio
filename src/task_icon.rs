use std::collections::HashMap;
use windows::Win32::Foundation::{HWND, LPARAM, PROPERTYKEY, WPARAM};
use windows::Win32::UI::Shell::PropertiesSystem::{
    IPropertyStore, PSGetPropertyKeyFromName, SHGetPropertyStoreForWindow,
};
use windows::Win32::UI::Shell::{
    BHID_SFUIObject, ExtractIconExW, IExtractIconW, IShellItem, SHCreateItemFromParsingName,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GCLP_HICON, GCLP_HICONSM, GetClassLongPtrW, HICON, ICON_BIG, ICON_SMALL,
    ICON_SMALL2, SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_GETICON,
};
use windows::core::{BSTR, Error, PCWSTR, Result, w};

#[derive(Default)]
/// Owns extracted icons for one task-list snapshot. Keep it alive until that list is replaced.
pub struct TaskIcons {
    icons: HashMap<String, Option<OwnedIcon>>,
    app_icons: HashMap<String, Option<OwnedIcon>>,
}

impl TaskIcons {
    pub fn resolve(&mut self, hwnd: HWND, executable: &str) -> isize {
        let borrowed = window_icon(hwnd);
        if borrowed != 0 {
            return borrowed;
        }
        let extracted = self
            .icons
            .entry(executable.to_owned())
            .or_insert_with(|| extract_icon(executable))
            .as_ref()
            .map_or(0, |icon| icon.0.0 as isize);
        if extracted != 0 {
            return extracted;
        }
        // Windows without an explicit app identity cannot use the AppsFolder fallback.
        let Ok(app_id) = window_app_id(hwnd) else {
            return 0;
        };
        if app_id.is_empty() {
            return 0;
        }
        self.app_icons
            .entry(app_id.clone())
            .or_insert_with(|| match shell_app_icon(&app_id) {
                Ok(icon) => icon,
                Err(error) => {
                    eprintln!("Could not load a packaged task icon: {error}");
                    None
                }
            })
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
    if executable.is_empty() || executable.contains('\0') {
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

fn window_app_id(hwnd: HWND) -> Result<String> {
    // SAFETY: hwnd is borrowed; the UI thread has initialized COM. The returned store and
    // property value own their resources and release them on this same thread.
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd)?;
        let mut key = PROPERTYKEY::default();
        PSGetPropertyKeyFromName(w!("System.AppUserModel.ID"), &raw mut key)?;
        let value = store.GetValue(&raw const key)?;
        Ok(BSTR::try_from(&value)?.to_string())
    }
}

fn shell_app_icon(app_id: &str) -> Result<Option<OwnedIcon>> {
    if app_id.is_empty() || app_id.contains('\0') {
        return Ok(None);
    }
    let path: Vec<u16> = format!("shell:AppsFolder\\{app_id}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: COM is initialized on this thread and path is a live terminated string.
    let item: IShellItem = unsafe { SHCreateItemFromParsingName(PCWSTR(path.as_ptr()), None) }?;
    // SAFETY: item is a live COM interface; this handler supplies the Shell's icon extractor.
    let extractor: IExtractIconW = unsafe { item.BindToHandler(None, &BHID_SFUIObject) }?;
    let mut location = vec![0_u16; 32768];
    let mut index = 0;
    let mut flags = 0;
    // SAFETY: all outputs are writable for this synchronous COM call.
    unsafe { extractor.GetIconLocation(0, &mut location, &raw mut index, &raw mut flags) }?;
    let mut icon = HICON::default();
    // SAFETY: location is terminated, extractor is live, and icon is writable. The returned
    // icon is owned, including when the extractor reports an error after allocating it.
    let result = unsafe {
        extractor.Extract(
            PCWSTR(location.as_ptr()),
            index.cast_unsigned(),
            Some(&raw mut icon),
            None,
            48,
        )
    };
    let owned = if icon.is_invalid() {
        None
    } else {
        Some(OwnedIcon(icon))
    };
    result?;
    Ok(owned)
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
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::UI::WindowsAndMessaging::{
        CopyIcon, CreateWindowExW, DestroyWindow, IDI_APPLICATION, LoadIconW, SendMessageW,
        WINDOW_EX_STYLE, WM_SETICON, WS_POPUP,
    };
    use windows::core::{Result, w};

    struct TestApartment;

    impl TestApartment {
        fn new() -> Result<Self> {
            // SAFETY: this test thread balances each successful initialization in Drop.
            unsafe {
                CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
            }
            Ok(Self)
        }
    }

    impl Drop for TestApartment {
        fn drop(&mut self) {
            // SAFETY: the guard remains on the thread whose COM initialization it owns.
            unsafe {
                CoUninitialize();
            }
        }
    }

    struct TestWindow(HWND, TestApartment);

    impl TestWindow {
        fn new() -> Result<Self> {
            let apartment = TestApartment::new()?;
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
                .map(|hwnd| Self(hwnd, apartment))
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
    #[ignore = "requires an open Windows Security window"]
    fn running_security_has_task_icon() -> Result<()> {
        let _apartment = TestApartment::new()?;
        let tasks = crate::task_query::enumerate_switchable_windows(
            &alttabio::settings::Settings::default(),
        )?;
        let task = tasks
            .tasks
            .iter()
            .find(|task| task.title == "Windows Security")
            .ok_or_else(|| {
                Error::new(
                    windows::core::HRESULT(0x8000_4005_u32.cast_signed()),
                    "Open Windows Security first",
                )
            })?;
        assert_ne!(
            task.icon_handle, 0,
            "Windows Security must have a task icon"
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires ALTTABIO_SECURITY_EXE pointing to installed Windows Security"]
    fn installed_security_supplies_icon_for_iconless_window() -> Result<()> {
        let window = TestWindow::new()?;
        let executable = std::env::var("ALTTABIO_SECURITY_EXE").map_err(|error| {
            Error::new(
                windows::core::HRESULT(0x8000_4005_u32.cast_signed()),
                error.to_string(),
            )
        })?;
        // SAFETY: the test owns this window and COM apartment, and the store copies the value.
        unsafe {
            let store: IPropertyStore = SHGetPropertyStoreForWindow(window.0)?;
            let mut key = PROPERTYKEY::default();
            PSGetPropertyKeyFromName(w!("System.AppUserModel.ID"), &raw mut key)?;
            store.SetValue(
                &raw const key,
                &PROPVARIANT::from("Microsoft.SecHealthUI_8wekyb3d8bbwe!SecHealthUI"),
            )?;
        }
        assert!(
            extract_icon(&executable).is_none(),
            "fixture must lack an executable icon"
        );
        let mut icons = TaskIcons::default();
        let icon = icons.resolve(window.0, &executable);
        assert_ne!(icon, 0, "Windows Security must have a task icon");
        assert_eq!(
            icons.resolve(window.0, ""),
            icon,
            "an unavailable process path must still use the window's app identity"
        );
        assert_eq!(icons.app_icons.len(), 1);
        // SAFETY: the snapshot owns this live Shell icon; CopyIcon returns a new owned handle.
        drop(OwnedIcon(unsafe { CopyIcon(HICON(icon as *mut _)) }?));
        drop(icons);
        // SAFETY: CopyIcon validates the stale scalar handle, with no intervening icon allocation.
        assert!(
            unsafe { CopyIcon(HICON(icon as *mut _)) }.is_err(),
            "replacing the snapshot must release Shell icons"
        );
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
