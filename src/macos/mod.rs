//! macOS adapter: `AppKit` overlay, `CGEventTap` hotkeys, and Accessibility window control.
//!
//! Every native callback (event tap, view events, timers, completion blocks) funnels into the
//! single `App` on the main thread through `with_app` or `post_to_app`; nothing native holds a
//! borrow across a nested run loop.

mod alerts;
mod app;
mod autostart;
mod ax;
mod cli;
mod commands;
mod event_tap;
mod hotkey;
mod keymap;
mod overlay;
mod permissions;
mod preview;
mod refresh_worker;
mod runtime;
mod screen;
mod settings_window;
mod single_instance;
mod status_item;
mod updater;
mod window_list;

use crate::settings_io::SettingsStore;
use alerts::show_fatal_error;
use alttabio::settings::Settings;
use app::App;
use cli::{activate_from_command_line, print_window_list};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use std::cell::RefCell;
use std::ffi::OsString;
use std::path::PathBuf;
use std::rc::Rc;

/// `ALTTABIO_TRACE=1` prints every tap event and switcher action to stderr for debugging.
fn tracing() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("ALTTABIO_TRACE").is_some_and(|value| value == "1"))
}

pub fn run(arguments: &[OsString]) {
    let preview_mode = arguments.iter().any(|argument| argument == "--preview");
    let settings_mode = arguments.iter().any(|argument| argument == "--settings");
    if arguments.iter().any(|argument| argument == "--list") {
        print_window_list();
        return;
    }
    if let Some(index) = arguments
        .iter()
        .position(|argument| argument == "--activate")
    {
        activate_from_command_line(arguments.get(index + 1));
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("AltTabio must start on the main thread");
        return;
    };
    if single_instance::another_instance_running() {
        eprintln!("AltTabio is already running");
        return;
    }
    let path = settings_path();
    let first_start = !path.exists();
    let (store, settings) = match SettingsStore::load_from(path, &Settings::macos_default()) {
        Ok(loaded) => loaded,
        Err(error) => {
            show_fatal_error(mtm, &error);
            return;
        }
    };

    let ns_app = NSApplication::sharedApplication(mtm);
    ns_app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let app = Rc::new(RefCell::new(App::new(mtm, settings, store, preview_mode)));
    runtime::install(&app);
    app.borrow_mut().start();
    if first_start && !preview_mode {
        app.borrow_mut().complete_first_start();
    }
    if settings_mode {
        app.borrow_mut().show_settings();
    }
    ns_app.run();
}

fn settings_path() -> PathBuf {
    if let Ok(executable) = std::env::current_exe() {
        let adjacent = executable.with_file_name("AltTabio.ini");
        if adjacent.exists() {
            return adjacent;
        }
    }
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    let directory = home
        .join("Library")
        .join("Application Support")
        .join("AltTabio");
    if let Err(error) = std::fs::create_dir_all(&directory) {
        eprintln!(
            "Could not create the settings directory {}: {error}",
            directory.display()
        );
    }
    directory.join("AltTabio.ini")
}

fn current_pid() -> i32 {
    i32::try_from(std::process::id()).unwrap_or_default()
}
