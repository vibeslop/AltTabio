use alttabio::settings::IconColor;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    HICON, ICON_BIG, ICON_SMALL, LoadIconW, SendMessageW, WM_SETICON,
};
use windows::core::{PCWSTR, Result};

#[must_use]
pub const fn app_resource_id(icon: IconColor) -> usize {
    match icon {
        IconColor::Azure => 1,
        IconColor::Copper => 2,
        IconColor::Ember => 3,
        IconColor::Indigo => 4,
        IconColor::Orchid => 5,
        IconColor::Rosewood => 6,
        IconColor::Vermilion => 7,
        IconColor::Violet => 8,
    }
}

#[must_use]
pub const fn tray_resource_id(icon: IconColor) -> usize {
    match icon {
        IconColor::Azure => 101,
        IconColor::Copper => 102,
        IconColor::Ember => 103,
        IconColor::Indigo => 104,
        IconColor::Orchid => 105,
        IconColor::Rosewood => 106,
        IconColor::Vermilion => 107,
        IconColor::Violet => 108,
    }
}

pub fn load_app(instance: HINSTANCE, icon: IconColor) -> Result<HICON> {
    load(instance, app_resource_id(icon))
}

pub fn load_tray(instance: HINSTANCE, icon: IconColor) -> Result<HICON> {
    load(instance, tray_resource_id(icon))
}

pub fn apply_to_window(hwnd: HWND, instance: HINSTANCE, icon: IconColor) -> Result<()> {
    let loaded = load_app(instance, icon)?;
    unsafe {
        // SAFETY: hwnd is live, the icon is a shared executable resource, and WM_SETICON copies
        // only the handle value for the lifetime of the loaded module.
        SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_BIG as usize)),
            Some(LPARAM(loaded.0 as isize)),
        );
        SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_SMALL as usize)),
            Some(LPARAM(loaded.0 as isize)),
        );
    }
    Ok(())
}

fn load(instance: HINSTANCE, resource_id: usize) -> Result<HICON> {
    unsafe {
        // SAFETY: app.rc embeds every mapped integer resource and LoadIconW returns a shared icon
        // owned by the executable module.
        LoadIconW(Some(instance), PCWSTR(resource_id as *const u16))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn azure_is_the_default_executable_and_tray_resource() {
        assert_eq!(IconColor::default(), IconColor::Azure);
        assert_eq!(app_resource_id(IconColor::default()), 1);
        assert_eq!(tray_resource_id(IconColor::default()), 101);
    }

    #[test]
    fn every_icon_color_maps_to_distinct_app_and_tray_resources() {
        let app_ids = IconColor::ALL.map(app_resource_id);
        let tray_ids = IconColor::ALL.map(tray_resource_id);

        for index in 0..IconColor::ALL.len() {
            assert_eq!(
                app_ids.iter().filter(|id| **id == app_ids[index]).count(),
                1
            );
            assert_eq!(
                tray_ids.iter().filter(|id| **id == tray_ids[index]).count(),
                1
            );
            assert!(app_ids[index] < tray_ids[index]);
        }
    }
}
