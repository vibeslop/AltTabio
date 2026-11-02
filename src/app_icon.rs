use alttabio::settings::IconColor;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetSystemMetrics, HICON, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR,
    LoadIconW, LoadImageW, SM_CXSMICON, SendMessageW, WM_SETICON,
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

/// An icon loaded at a requested pixel size, uniquely owned rather than shared by the module.
pub struct OwnedIcon(HICON);

impl OwnedIcon {
    pub const fn handle(&self) -> HICON {
        self.0
    }
}

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns an icon LoadImageW created without LR_SHARED.
        if let Err(error) = unsafe { DestroyIcon(self.0) } {
            eprintln!("Could not release an owned icon: {error}");
        }
    }
}

/// `LoadIconW` would load the large icon size, which the notification area then shrinks; loading
/// at the small icon size picks the image drawn for the tray at the system's scale.
pub fn load_tray(instance: HINSTANCE, icon: IconColor) -> Result<OwnedIcon> {
    let size = unsafe {
        // SAFETY: GetSystemMetrics reads a system value and has no pointer preconditions.
        GetSystemMetrics(SM_CXSMICON)
    };
    load_sized(instance, tray_resource_id(icon), size)
}

pub fn load_app_sized(instance: HINSTANCE, icon: IconColor, size: i32) -> Result<OwnedIcon> {
    load_sized(instance, app_resource_id(icon), size)
}

pub fn load_github(instance: HINSTANCE, dark: bool, size: i32) -> Result<OwnedIcon> {
    load_sized(instance, if dark { 202 } else { 201 }, size)
}

fn load_sized(instance: HINSTANCE, resource_id: usize, size: i32) -> Result<OwnedIcon> {
    let image = unsafe {
        // SAFETY: app.rc embeds every mapped integer resource, and without LR_SHARED the returned
        // icon belongs to the caller, which hands it to OwnedIcon.
        LoadImageW(
            Some(instance),
            PCWSTR(resource_id as *const u16),
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        )
    }?;
    Ok(OwnedIcon(HICON(image.0)))
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
