use crate::app_icon;
use crate::native_theme::{DarkModeApi, PreferredAppMode, preferred_app_mode};
use alttabio::settings::IconColor;
use alttabio::theme::ResolvedTheme;
use std::mem::size_of;
use windows::Win32::Foundation::{HINSTANCE, HWND, POINT};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, HMENU, MF_SEPARATOR, MF_STRING,
    RegisterWindowMessageW, SetForegroundWindow, TPM_BOTTOMALIGN, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTALIGN, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP,
};
use windows::core::{Error, PCWSTR, Result, w};

pub const WM_TRAY_CALLBACK: u32 = WM_APP + 2;

const fn is_recreation_message(message: u32, registered_message: u32) -> bool {
    registered_message != 0 && message == registered_message
}

const SHOW_COMMAND: usize = 1;
const SETTINGS_COMMAND: usize = 2;
const ABOUT_COMMAND: usize = 3;
const EXIT_COMMAND: usize = 4;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TrayAction {
    Show,
    Settings,
    About,
    Exit,
    #[default]
    None,
}

pub struct TrayIcon {
    data: NOTIFYICONDATAW,
    menu: HMENU,
    instance: HINSTANCE,
    recreation_message: u32,
    theme: ResolvedTheme,
    dark_mode_api: Option<DarkModeApi>,
}

impl TrayIcon {
    pub fn new(
        hwnd: HWND,
        instance: HINSTANCE,
        theme: ResolvedTheme,
        icon: IconColor,
    ) -> Result<Self> {
        let recreation_message = unsafe {
            // SAFETY: the message name is a static null-terminated UTF-16 string.
            RegisterWindowMessageW(w!("TaskbarCreated"))
        };
        if recreation_message == 0 {
            return Err(Error::from_thread());
        }
        let icon = app_icon::load_tray(instance, icon)?;
        let dark_mode_api = match DarkModeApi::load(theme == ResolvedTheme::Dark) {
            Ok(api) => Some(api),
            Err(error) => {
                eprintln!("Native tray-menu themes are unavailable: {error}");
                None
            }
        };
        let menu = unsafe {
            // SAFETY: CreatePopupMenu has no pointer preconditions and returns a uniquely owned
            // menu handle on success.
            CreatePopupMenu()
        }?;
        let menu_result = unsafe {
            // SAFETY: menu is live, command ids are application-owned, and labels are static
            // null-terminated UTF-16 strings. A zero-id separator cannot emit an application
            // command.
            AppendMenuW(menu, MF_STRING, SHOW_COMMAND, w!("Show")).and_then(|()| {
                AppendMenuW(menu, MF_STRING, SETTINGS_COMMAND, w!("Settings..."))
                    .and_then(|()| {
                        AppendMenuW(menu, MF_STRING, ABOUT_COMMAND, w!("About AltTabio..."))
                    })
                    .and_then(|()| AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()))
                    .and_then(|()| AppendMenuW(menu, MF_STRING, EXIT_COMMAND, w!("Exit")))
            })
        };
        if let Err(error) = menu_result {
            destroy_menu(menu);
            return Err(error);
        }

        let mut tip = [0_u16; 128];
        copy_utf16(&mut tip, "AltTabio");
        let data = NOTIFYICONDATAW {
            cbSize: u32::try_from(size_of::<NOTIFYICONDATAW>()).unwrap_or_default(),
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY_CALLBACK,
            hIcon: icon,
            szTip: tip,
            ..NOTIFYICONDATAW::default()
        };
        if let Err(error) = add_icon(&data) {
            destroy_menu(menu);
            return Err(error);
        }
        Ok(Self {
            data,
            menu,
            instance,
            recreation_message,
            theme,
            dark_mode_api,
        })
    }

    pub fn show_menu(&self) -> TrayAction {
        let mut cursor = POINT::default();
        let position_read = unsafe {
            // SAFETY: cursor is writable for the synchronous call.
            GetCursorPos(&raw mut cursor)
        };
        if position_read.is_err() {
            return TrayAction::None;
        }
        if let Some(api) = self.dark_mode_api.as_ref() {
            apply_menu_theme(api, self.theme);
        }
        unsafe {
            // SAFETY: the hidden application HWND owns the tray menu; bringing it to the foreground
            // ensures Windows dismisses the popup correctly when focus changes.
            let _foreground = SetForegroundWindow(self.data.hWnd);
        }
        let command = unsafe {
            // SAFETY: menu and HWND are live, the coordinates came from GetCursorPos, and no RECT
            // pointer is required. TPM_RETURNCMD prevents asynchronous WM_COMMAND delivery.
            TrackPopupMenu(
                self.menu,
                TPM_RIGHTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
                cursor.x,
                cursor.y,
                None,
                self.data.hWnd,
                None,
            )
        };
        action_for_command(usize::try_from(command.0).unwrap_or_default())
    }

    pub fn set_theme(&mut self, theme: ResolvedTheme) {
        self.theme = theme;
        if let Some(api) = self.dark_mode_api.as_ref() {
            apply_menu_theme(api, theme);
        }
    }

    pub fn restore_for_message(&self, message: u32) -> Option<Result<()>> {
        is_recreation_message(message, self.recreation_message).then(|| add_icon(&self.data))
    }

    pub fn set_icon(&mut self, icon: IconColor) -> Result<()> {
        let loaded = app_icon::load_tray(self.instance, icon)?;
        let previous = self.data.hIcon;
        self.data.hIcon = loaded;
        let updated = unsafe {
            // SAFETY: data identifies the live notification icon and the shell copies the new
            // shared resource handle during this synchronous update.
            Shell_NotifyIconW(NIM_MODIFY, &raw const self.data).as_bool()
        };
        if updated {
            Ok(())
        } else {
            self.data.hIcon = previous;
            Err(Error::from_thread())
        }
    }
}

fn add_icon(data: &NOTIFYICONDATAW) -> Result<()> {
    let added = unsafe {
        // SAFETY: data is fully initialized and remains alive for the synchronous shell call.
        Shell_NotifyIconW(NIM_ADD, data).as_bool()
    };
    if added {
        Ok(())
    } else {
        Err(Error::from_thread())
    }
}

trait MenuThemeApi {
    fn set_preferred_app_mode(&self, mode: PreferredAppMode);
    fn flush_menu_themes(&self);
}

impl MenuThemeApi for DarkModeApi {
    fn set_preferred_app_mode(&self, mode: PreferredAppMode) {
        DarkModeApi::set_preferred_app_mode(self, mode);
    }

    fn flush_menu_themes(&self) {
        DarkModeApi::flush_menu_themes(self);
    }
}

fn apply_menu_theme(api: &impl MenuThemeApi, theme: ResolvedTheme) {
    api.set_preferred_app_mode(preferred_app_mode(theme == ResolvedTheme::Dark));
    api.flush_menu_themes();
}

fn action_for_command(command: usize) -> TrayAction {
    match command {
        SHOW_COMMAND => TrayAction::Show,
        SETTINGS_COMMAND => TrayAction::Settings,
        ABOUT_COMMAND => TrayAction::About,
        EXIT_COMMAND => TrayAction::Exit,
        _ => TrayAction::None,
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        let deleted = unsafe {
            // SAFETY: data identifies the icon added by this guard and the shell copies it during
            // the synchronous deletion call.
            Shell_NotifyIconW(NIM_DELETE, &raw const self.data).as_bool()
        };
        if !deleted {
            eprintln!("Could not remove the AltTabio tray icon");
        }
        destroy_menu(self.menu);
    }
}

fn copy_utf16(destination: &mut [u16], value: &str) {
    for (slot, character) in destination.iter_mut().zip(value.encode_utf16().chain([0])) {
        *slot = character;
    }
}

fn destroy_menu(menu: HMENU) {
    let result = unsafe {
        // SAFETY: the caller transfers one uniquely owned popup-menu handle for exactly one
        // destruction attempt.
        DestroyMenu(menu)
    };
    if let Err(error) = result {
        eprintln!("Could not destroy the tray menu: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum MenuThemeOperation {
        SetMode(PreferredAppMode),
        Flush,
    }

    #[derive(Default)]
    struct RecordingMenuThemeApi {
        operations: std::cell::RefCell<Vec<MenuThemeOperation>>,
    }

    impl MenuThemeApi for RecordingMenuThemeApi {
        fn set_preferred_app_mode(&self, mode: PreferredAppMode) {
            self.operations
                .borrow_mut()
                .push(MenuThemeOperation::SetMode(mode));
        }

        fn flush_menu_themes(&self) {
            self.operations.borrow_mut().push(MenuThemeOperation::Flush);
        }
    }

    #[test]
    fn tray_commands_map_to_their_actions() {
        assert_eq!(action_for_command(SHOW_COMMAND), TrayAction::Show);
        assert_eq!(action_for_command(SETTINGS_COMMAND), TrayAction::Settings);
        assert_eq!(action_for_command(ABOUT_COMMAND), TrayAction::About);
        assert_eq!(action_for_command(EXIT_COMMAND), TrayAction::Exit);
        assert_eq!(action_for_command(0), TrayAction::None);
    }

    #[test]
    fn taskbar_created_message_requests_tray_icon_restoration() {
        let taskbar_created_message = 0xC123;

        assert!(is_recreation_message(
            taskbar_created_message,
            taskbar_created_message
        ));
        assert!(!is_recreation_message(
            WM_TRAY_CALLBACK,
            taskbar_created_message
        ));
        assert!(!is_recreation_message(0, 0));
    }

    #[test]
    fn tray_popup_applies_the_resolved_theme_before_flushing_the_menu_cache() {
        for (theme, expected_mode) in [
            (ResolvedTheme::Light, PreferredAppMode::ForceLight),
            (ResolvedTheme::Dark, PreferredAppMode::ForceDark),
        ] {
            let api = RecordingMenuThemeApi::default();

            apply_menu_theme(&api, theme);

            assert_eq!(
                *api.operations.borrow(),
                [
                    MenuThemeOperation::SetMode(expected_mode),
                    MenuThemeOperation::Flush,
                ]
            );
        }
    }
}
