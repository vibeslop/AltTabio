//! Switching to a window or an app, and the commands the switcher runs on them, on top of
//! Accessibility and `AppKit`.

use super::window_list::{WindowRecord, launch_time};
use alttabio::input::WindowCommand;
use alttabio::switcher::ProcessIdentity;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};

/// A running app as the switcher lists it.
pub struct AppRef<'a> {
    pub process: ProcessIdentity,
    pub name: &'a str,
}

pub fn activate(record: &WindowRecord) -> Result<(), String> {
    let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(record.pid)
    else {
        return Err(format!("{} is no longer running", record.app_name));
    };
    if app.isHidden() && !app.unhide() {
        eprintln!("Could not unhide {}", record.app_name);
    }
    if let Some(ax) = &record.ax {
        if ax.boolean("AXMinimized") == Some(true) && !ax.set_boolean("AXMinimized", false) {
            eprintln!("Could not restore the minimized window {}", record.title);
        }
        // Raising before activation makes the chosen window the app's key window instead of
        // whichever window the app last used.
        if !ax.perform("AXRaise") {
            eprintln!("Could not raise the window {}", record.title);
        }
    }
    bring_forward(&app, &record.app_name)?;
    if let Some(ax) = &record.ax
        && !ax.perform("AXRaise")
    {
        eprintln!("Could not bring {} to the front", record.title);
    }
    Ok(())
}

/// Brings an app forward on its own, as the system switcher does for an app with no window.
pub fn activate_app(app: &AppRef<'_>) -> Result<(), String> {
    let running = running_application(app)?;
    if running.isHidden() && !running.unhide() {
        eprintln!("Could not unhide {}", app.name);
    }
    bring_forward(&running, app.name)
}

fn bring_forward(app: &NSRunningApplication, name: &str) -> Result<(), String> {
    #[allow(
        deprecated,
        reason = "cooperative activation cannot target another app's window; the ignoring-other-apps path is the documented behaviour for switchers"
    )]
    let activated =
        app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
    if activated {
        Ok(())
    } else {
        Err(format!("macOS refused to activate {name}"))
    }
}

/// Runs a command that acts on one window: Close or Minimize.
pub fn execute_on_window(command: WindowCommand, record: &WindowRecord) -> Result<(), String> {
    let ax = record.ax.as_ref().ok_or_else(|| {
        format!(
            "{} is not reachable through Accessibility; grant AltTabio Accessibility access",
            record.title
        )
    })?;
    match command {
        WindowCommand::Close => {
            let close = ax
                .element("AXCloseButton")
                .ok_or_else(|| format!("{} has no close button", record.title))?;
            if close.perform("AXPress") {
                Ok(())
            } else {
                Err(format!("Could not close {}", record.title))
            }
        }
        WindowCommand::Minimize => {
            if ax.set_boolean("AXMinimized", true) {
                Ok(())
            } else {
                Err(format!("Could not minimize {}", record.title))
            }
        }
        other => Err(format!("{other:?} does not act on a single window")),
    }
}

/// Runs a command that acts on the whole app: Hide, Quit, or Force Quit.
pub fn execute_on_app(command: WindowCommand, app: &AppRef<'_>) -> Result<(), String> {
    match command {
        WindowCommand::Hide => {
            if running_application(app)?.hide() {
                Ok(())
            } else {
                Err(format!("Could not hide {}", app.name))
            }
        }
        WindowCommand::Quit => {
            if running_application(app)?.terminate() {
                Ok(())
            } else {
                Err(format!("{} did not accept the quit request", app.name))
            }
        }
        WindowCommand::Terminate => force_quit(app),
        other => Err(format!("{other:?} does not act on an app")),
    }
}

fn running_application(app: &AppRef<'_>) -> Result<Retained<NSRunningApplication>, String> {
    let pid = i32::try_from(app.process.id)
        .map_err(|_| format!("{} has an invalid process id", app.name))?;
    let running = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .ok_or_else(|| format!("{} is no longer running", app.name))?;
    // A reused pid after the listed app exited must not act on an unrelated process.
    if launch_time(&running) != app.process.started_at {
        return Err(format!(
            "{} was replaced by another process; leaving it alone",
            app.name
        ));
    }
    Ok(running)
}

fn force_quit(app: &AppRef<'_>) -> Result<(), String> {
    let running = running_application(app)?;
    let pid = running.processIdentifier();
    let result = unsafe {
        // SAFETY: kill has no memory preconditions; the pid was verified against its launch time.
        libc::kill(pid, libc::SIGKILL)
    };
    if result != 0 {
        return Err(format!(
            "Could not force quit {}: {}",
            app.name,
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}
