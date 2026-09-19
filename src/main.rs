#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(not(panic = "unwind"))]
compile_error!("AltTabio requires panic=unwind to contain panics at native callback boundaries");

#[cfg(windows)]
mod about_dialog;
#[cfg(windows)]
mod app_icon;
#[cfg(windows)]
mod hook;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod native_drawing;
#[cfg(windows)]
mod native_theme;
#[cfg(windows)]
mod preview;
#[cfg(windows)]
mod process_info;
#[cfg(windows)]
mod renderer;
#[cfg(windows)]
mod settings_dialog;
mod settings_io;
#[cfg(windows)]
mod shell_menu;
#[cfg(windows)]
mod single_instance;
#[cfg(windows)]
mod startup;
#[cfg(windows)]
mod switch_hotkey;
#[cfg(windows)]
mod task_icon;
#[cfg(windows)]
mod task_query;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod win_events;
#[cfg(windows)]
mod window_commands;
#[cfg(windows)]
mod windows_app;

fn main() {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    run(&arguments);
}

#[cfg(windows)]
fn run(arguments: &[std::ffi::OsString]) {
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

#[cfg(target_os = "macos")]
fn run(arguments: &[std::ffi::OsString]) {
    macos::run(arguments);
}
