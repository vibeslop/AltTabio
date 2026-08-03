use std::mem::size_of;
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::core::{Error, HRESULT, PCSTR, Result, w};

const ALLOW_DARK_MODE_FOR_WINDOW_ORDINAL: usize = 133;
const SET_PREFERRED_APP_MODE_ORDINAL: usize = 135;
const FLUSH_MENU_THEMES_ORDINAL: usize = 136;
const SET_PREFERRED_APP_MODE_MINIMUM_BUILD: u32 = 18_362;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlGetVersion(version: *mut NativeOsVersionInfo) -> i32;
}

#[repr(C)]
struct NativeOsVersionInfo {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    service_pack: [u16; 128],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub(crate) enum PreferredAppMode {
    ForceDark = 2,
    ForceLight = 3,
}

pub(crate) const fn preferred_app_mode(dark: bool) -> PreferredAppMode {
    if dark {
        PreferredAppMode::ForceDark
    } else {
        PreferredAppMode::ForceLight
    }
}

type SetPreferredAppModeFn = unsafe extern "system" fn(i32) -> i32;
type AllowDarkModeForWindowFn =
    unsafe extern "system" fn(windows::Win32::Foundation::HWND, bool) -> bool;
type FlushMenuThemesFn = unsafe extern "system" fn();

pub(crate) struct DarkModeApi {
    module: HMODULE,
    set_preferred_app_mode: SetPreferredAppModeFn,
    allow_dark_mode_for_window: AllowDarkModeForWindowFn,
    flush_menu_themes: FlushMenuThemesFn,
    original_mode: i32,
}

impl DarkModeApi {
    pub(crate) fn load(initial_dark: bool) -> Result<Self> {
        if !supports_set_preferred_app_mode(current_windows_build()?) {
            return Err(Error::from_hresult(HRESULT(0x8000_4001_u32.cast_signed())));
        }
        let module = unsafe {
            // SAFETY: the system32-only search prevents DLL preloading from an application path.
            LoadLibraryExW(w!("uxtheme.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
        }?;
        let set_preferred = unsafe {
            // SAFETY: ordinal 135 is SetPreferredAppMode on supported Windows 10 1903+ systems.
            GetProcAddress(module, PCSTR(SET_PREFERRED_APP_MODE_ORDINAL as *const u8))
        };
        let allow_for_window = unsafe {
            // SAFETY: ordinal 133 is AllowDarkModeForWindow on supported Windows 10 1809+ systems.
            GetProcAddress(
                module,
                PCSTR(ALLOW_DARK_MODE_FOR_WINDOW_ORDINAL as *const u8),
            )
        };
        let flush_menus = unsafe {
            // SAFETY: ordinal 136 is FlushMenuThemes on supported Windows 10 1903+ systems.
            GetProcAddress(module, PCSTR(FLUSH_MENU_THEMES_ORDINAL as *const u8))
        };
        let (Some(set_preferred), Some(allow_for_window), Some(flush_menus)) =
            (set_preferred, allow_for_window, flush_menus)
        else {
            unsafe {
                // SAFETY: module is the unique reference acquired above and no function pointer is
                // retained when a required ordinal is unavailable.
                FreeLibrary(module)?;
            }
            return Err(Error::from_hresult(HRESULT(0x8000_4001_u32.cast_signed())));
        };
        let set_preferred_app_mode = unsafe {
            // SAFETY: ordinal 135 has the SetPreferredAppMode ABI on the supported Windows builds.
            std::mem::transmute::<unsafe extern "system" fn() -> isize, SetPreferredAppModeFn>(
                set_preferred,
            )
        };
        let allow_dark_mode_for_window = unsafe {
            // SAFETY: ordinal 133 has the AllowDarkModeForWindow ABI on supported Windows builds.
            std::mem::transmute::<unsafe extern "system" fn() -> isize, AllowDarkModeForWindowFn>(
                allow_for_window,
            )
        };
        let flush_menu_themes = unsafe {
            // SAFETY: ordinal 136 has the parameterless FlushMenuThemes ABI on supported builds.
            std::mem::transmute::<unsafe extern "system" fn() -> isize, FlushMenuThemesFn>(
                flush_menus,
            )
        };
        let original_mode = unsafe {
            // SAFETY: the resolved function pointer has the SetPreferredAppMode ABI.
            set_preferred_app_mode(preferred_app_mode(initial_dark) as i32)
        };
        Ok(Self {
            module,
            set_preferred_app_mode,
            allow_dark_mode_for_window,
            flush_menu_themes,
            original_mode,
        })
    }

    pub(crate) fn set_preferred_app_mode(&self, mode: PreferredAppMode) {
        unsafe {
            // SAFETY: the function pointer remains valid while self holds the uxtheme module.
            (self.set_preferred_app_mode)(mode as i32);
        }
    }

    pub(crate) fn set_effective_theme(&self, dark: bool) {
        self.set_preferred_app_mode(preferred_app_mode(dark));
    }

    pub(crate) fn allow_for_window(&self, hwnd: windows::Win32::Foundation::HWND, dark: bool) {
        let allowed = unsafe {
            // SAFETY: hwnd is live and the function pointer remains valid while self owns uxtheme.
            (self.allow_dark_mode_for_window)(hwnd, dark)
        };
        if !allowed {
            eprintln!("Windows declined native dark styling for a settings control");
        }
    }

    pub(crate) fn flush_menu_themes(&self) {
        unsafe {
            // SAFETY: the function pointer remains valid while self holds the uxtheme module.
            (self.flush_menu_themes)();
        }
    }
}

const fn supports_set_preferred_app_mode(build: u32) -> bool {
    build >= SET_PREFERRED_APP_MODE_MINIMUM_BUILD
}

fn current_windows_build() -> Result<u32> {
    let mut version = NativeOsVersionInfo {
        size: u32::try_from(size_of::<NativeOsVersionInfo>()).unwrap_or(u32::MAX),
        major: 0,
        minor: 0,
        build: 0,
        platform: 0,
        service_pack: [0; 128],
    };
    let status = unsafe {
        // SAFETY: version is a correctly sized writable OSVERSIONINFOW-compatible structure.
        RtlGetVersion(&raw mut version)
    };
    if status >= 0 {
        Ok(version.build)
    } else {
        Err(Error::from_hresult(HRESULT(status)))
    }
}

impl Drop for DarkModeApi {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: both function pointers remain valid until the module is released below.
            (self.set_preferred_app_mode)(self.original_mode);
            (self.flush_menu_themes)();
            // SAFETY: module is the unique LoadLibraryExW reference owned by this wrapper.
            if let Err(error) = FreeLibrary(self.module) {
                eprintln!("Could not release the native theme module: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_app_modes_follow_the_resolved_theme() {
        assert_eq!(preferred_app_mode(true), PreferredAppMode::ForceDark);
        assert_eq!(preferred_app_mode(false), PreferredAppMode::ForceLight);
        assert!(!supports_set_preferred_app_mode(18_361));
        assert!(supports_set_preferred_app_mode(18_362));
    }
}
