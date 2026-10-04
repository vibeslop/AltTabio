//! `--list` and `--activate`, which work on the window list without starting the switcher.

use super::window_list::{self, EnumerationOptions, Listing, Unresponsive};
use super::{commands, current_pid};
use std::ffi::OsString;

/// Every window on every display, asked for once from the command line.
fn list_all_windows() -> Listing {
    window_list::enumerate(
        EnumerationOptions {
            current_pid: current_pid(),
            display_bounds: None,
        },
        &mut Unresponsive::default(),
    )
}

pub(super) fn print_window_list() {
    let listing = list_all_windows();
    println!(
        "{:>8}  {:>6}  {:<5} {:<24} TITLE",
        "ID", "PID", "STATE", "APP"
    );
    for record in listing.windows {
        let state = match (record.is_on_screen, record.is_minimized, record.is_hidden) {
            (true, _, _) => "shown",
            (false, true, _) => "min",
            (false, false, true) => "hide",
            (false, false, false) => "off",
        };
        println!(
            "{:>8}  {:>6}  {:<5} {:<24} {}",
            record.window_id,
            record.pid,
            state,
            truncate(&record.app_name, 24),
            record.title
        );
    }
    for app in listing.windowless {
        println!(
            "{:>8}  {:>6}  {:<5} {}",
            "-",
            app.pid,
            "none",
            truncate(&app.name, 24)
        );
    }
}

pub(super) fn activate_from_command_line(argument: Option<&OsString>) {
    let Some(window_id) = argument
        .and_then(|value| value.to_str())
        .and_then(|value| value.parse::<u32>().ok())
    else {
        eprintln!("Usage: AltTabio --activate <window id from --list>");
        return;
    };
    let records = list_all_windows().windows;
    let Some(record) = records.iter().find(|record| record.window_id == window_id) else {
        eprintln!("Window {window_id} was not found");
        return;
    };
    match commands::activate(record) {
        Ok(()) => println!("Activated {} ({})", record.title, record.app_name),
        Err(error) => eprintln!("{error}"),
    }
}

fn truncate(value: &str, width: usize) -> String {
    value.chars().take(width).collect()
}
