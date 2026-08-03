#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(panic = "unwind"))]
compile_error!("AltTabio requires panic=unwind to contain panics at Win32 callback boundaries");

mod about_dialog;
mod app_icon;
mod hook;
mod native_theme;
mod preview;
mod renderer;
mod settings_dialog;
mod settings_io;
mod single_instance;
mod startup;
mod tray;
mod window_commands;
mod windows_app;

fn main() {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    let preview_mode = arguments.iter().any(|argument| argument == "--preview");
    let dwm_preview = !arguments
        .iter()
        .any(|argument| argument == "--no-dwm-preview");
    let (settings_store, mut settings) = match settings_io::SettingsStore::load_adjacent() {
        Ok(loaded) => loaded,
        Err(error) => {
            windows_app::show_fatal_error(&error);
            return;
        }
    };
    if preview_mode
        && arguments
            .iter()
            .any(|argument| argument == "--full-desktop-preview")
    {
        settings.appearance.full_desktop_preview = true;
    }
    if let Err(error) = windows_app::run(preview_mode, dwm_preview, settings, settings_store) {
        windows_app::show_fatal_error(&error.to_string());
    }
}
