use crate::app_icon;
use crate::app_messages::WM_TRAY_CALLBACK;
use crate::native_theme::{DarkModeApi, PreferredAppMode, preferred_app_mode};
use alttabio::settings::IconColor;
use alttabio::theme::ResolvedTheme;
use std::cell::Cell;
use std::mem::size_of;
use windows::Win32::Foundation::{HINSTANCE, HWND, POINT};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, HMENU, MF_BYCOMMAND, MF_SEPARATOR,
    MF_STRING, ModifyMenuW, RegisterWindowMessageW, SetForegroundWindow, TPM_BOTTOMALIGN,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTALIGN, TPM_RIGHTBUTTON, TrackPopupMenu,
};
use windows::core::{Error, PCWSTR, Result, w};

const fn is_recreation_message(message: u32, registered_message: u32) -> bool {
    registered_message != 0 && message == registered_message
}

const OPEN_SWITCHER_COMMAND: usize = 1;
const SETTINGS_COMMAND: usize = 2;
const ABOUT_COMMAND: usize = 3;
const EXIT_COMMAND: usize = 4;
const UPDATE_COMMAND: usize = 5;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TrayAction {
    OpenSwitcher,
    Settings,
    About,
    Update,
    Exit,
    #[default]
    None,
}

pub struct TrayIcon {
    data: NOTIFYICONDATAW,
    /// Owns the icon in `data.hIcon`. Fields drop after `Drop::drop` deletes the notification
    /// icon, so the shell has let go of it by then.
    icon: app_icon::OwnedIcon,
    menu: HMENU,
    instance: HINSTANCE,
    recreation_message: u32,
    theme: ResolvedTheme,
    dark_mode_api: Option<DarkModeApi>,
    update_installed: Cell<bool>,
    registered: Cell<bool>,
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
            AppendMenuW(menu, MF_STRING, OPEN_SWITCHER_COMMAND, w!("Open Switcher")).and_then(
                |()| {
                    AppendMenuW(menu, MF_STRING, SETTINGS_COMMAND, w!("Settings"))
                        .and_then(|()| {
                            AppendMenuW(menu, MF_STRING, ABOUT_COMMAND, w!("About AltTabio"))
                        })
                        .and_then(|()| {
                            AppendMenuW(menu, MF_STRING, UPDATE_COMMAND, w!("Check for Updates..."))
                        })
                        .and_then(|()| AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()))
                        .and_then(|()| AppendMenuW(menu, MF_STRING, EXIT_COMMAND, w!("Exit")))
                },
            )
        };
        if let Err(error) = menu_result {
            destroy_menu(menu);
            return Err(error);
        }

        let mut tip = [0_u16; 128];
        copy_utf16(&mut tip, concat!("AltTabio ", env!("CARGO_PKG_VERSION")));
        let data = NOTIFYICONDATAW {
            cbSize: u32::try_from(size_of::<NOTIFYICONDATAW>()).unwrap_or_default(),
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY_CALLBACK,
            hIcon: icon.handle(),
            szTip: tip,
            ..NOTIFYICONDATAW::default()
        };
        // A logon task can run before Explorer's notification area is ready. Keep the
        // owned resources alive for TaskbarCreated and the outer loop's registration retry.
        let registered = match register_icon(&data) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("The notification area is unavailable; waiting for Explorer: {error}");
                false
            }
        };
        Ok(Self {
            data,
            icon,
            menu,
            instance,
            recreation_message,
            theme,
            dark_mode_api,
            update_installed: Cell::new(false),
            registered: Cell::new(registered),
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

    pub fn set_update_installed(&self, installed: bool) -> Result<()> {
        if self.update_installed.get() == installed {
            return Ok(());
        }
        let label = if installed {
            w!("Update status...")
        } else {
            w!("Check for Updates...")
        };
        unsafe {
            // SAFETY: menu is owned by this guard, the command exists, and label is static UTF-16.
            ModifyMenuW(
                self.menu,
                u32::try_from(UPDATE_COMMAND).unwrap_or_default(),
                MF_BYCOMMAND | MF_STRING,
                UPDATE_COMMAND,
                label,
            )
        }?;
        self.update_installed.set(installed);
        Ok(())
    }

    pub fn restore_for_message(&self, message: u32) -> Option<Result<()>> {
        is_recreation_message(message, self.recreation_message).then(|| {
            self.registered.set(false);
            self.retry_registration()
        })
    }

    pub fn retry_registration(&self) -> Result<()> {
        if !self.registered.get() {
            register_icon(&self.data)?;
            self.registered.set(true);
        }
        Ok(())
    }

    pub fn set_icon(&mut self, icon: IconColor) -> Result<()> {
        let loaded = app_icon::load_tray(self.instance, icon)?;
        let previous = self.data.hIcon;
        self.data.hIcon = loaded.handle();
        let updated = unsafe {
            // SAFETY: data identifies the live notification icon and the shell copies the new
            // icon during this synchronous update.
            Shell_NotifyIconW(NIM_MODIFY, &raw const self.data).as_bool()
        };
        if updated {
            // The shell holds its own copy now, so replacing the guard releases the previous icon.
            self.icon = loaded;
            Ok(())
        } else {
            self.data.hIcon = previous;
            Err(Error::from_thread())
        }
    }
}

fn register_icon(data: &NOTIFYICONDATAW) -> Result<()> {
    let added = unsafe {
        // SAFETY: data is fully initialized and remains alive for both synchronous shell calls.
        // A timer retry can race TaskbarCreated; modifying an existing icon makes registration
        // idempotent when the shell already accepted its HWND/id.
        Shell_NotifyIconW(NIM_ADD, data).as_bool() || Shell_NotifyIconW(NIM_MODIFY, data).as_bool()
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
        OPEN_SWITCHER_COMMAND => TrayAction::OpenSwitcher,
        SETTINGS_COMMAND => TrayAction::Settings,
        ABOUT_COMMAND => TrayAction::About,
        UPDATE_COMMAND => TrayAction::Update,
        EXIT_COMMAND => TrayAction::Exit,
        _ => TrayAction::None,
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        if self.registered.get() {
            let deleted = unsafe {
                // SAFETY: data identifies the icon added by this guard and the shell copies it
                // during the synchronous deletion call.
                Shell_NotifyIconW(NIM_DELETE, &raw const self.data).as_bool()
            };
            if !deleted {
                eprintln!("Could not remove the AltTabio tray icon");
            }
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
        assert_eq!(
            action_for_command(OPEN_SWITCHER_COMMAND),
            TrayAction::OpenSwitcher
        );
        assert_eq!(action_for_command(SETTINGS_COMMAND), TrayAction::Settings);
        assert_eq!(action_for_command(ABOUT_COMMAND), TrayAction::About);
        assert_eq!(action_for_command(UPDATE_COMMAND), TrayAction::Update);
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
