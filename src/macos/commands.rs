//! Window activation and the window commands (F4-F9 and the ⌘ chords) on top of Accessibility
//! and `AppKit`.

use super::window_list::{WindowRecord, launch_time};
use alttabio::input::WindowCommand;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplicationActivationOptions, NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration,
};

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
    #[allow(
        deprecated,
        reason = "cooperative activation cannot target another app's window; the ignoring-other-apps path is the documented behaviour for switchers"
    )]
    let activated =
        app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
    if !activated {
        return Err(format!("macOS refused to activate {}", record.app_name));
    }
    if let Some(ax) = &record.ax
        && !ax.perform("AXRaise")
    {
        eprintln!("Could not bring {} to the front", record.title);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandOutcome {
    /// The window list changes soon; the caller should refresh a few times.
    ListChanges,
    Unchanged,
}

pub fn execute(command: WindowCommand, record: &WindowRecord) -> Result<CommandOutcome, String> {
    match command {
        WindowCommand::Close => {
            let ax = accessible(record)?;
            let close = ax
                .element("AXCloseButton")
                .ok_or_else(|| format!("{} has no close button", record.title))?;
            if !close.perform("AXPress") {
                return Err(format!("Could not close {}", record.title));
            }
            Ok(CommandOutcome::ListChanges)
        }
        WindowCommand::Minimize => {
            let ax = accessible(record)?;
            if !ax.set_boolean("AXMinimized", true) {
                return Err(format!("Could not minimize {}", record.title));
            }
            Ok(CommandOutcome::ListChanges)
        }
        WindowCommand::Maximize => {
            let ax = accessible(record)?;
            let zoom = ax
                .element("AXZoomButton")
                .ok_or_else(|| format!("{} has no zoom button", record.title))?;
            if !zoom.perform("AXPress") {
                return Err(format!("Could not zoom {}", record.title));
            }
            Ok(CommandOutcome::Unchanged)
        }
        WindowCommand::Restore => {
            let ax = accessible(record)?;
            if ax.boolean("AXMinimized") == Some(true) && !ax.set_boolean("AXMinimized", false) {
                return Err(format!("Could not restore {}", record.title));
            }
            if ax.boolean("AXFullScreen") == Some(true) && !ax.set_boolean("AXFullScreen", false) {
                return Err(format!("Could not leave full screen for {}", record.title));
            }
            Ok(CommandOutcome::ListChanges)
        }
        WindowCommand::Terminate => terminate(record),
        WindowCommand::Run => run_another_instance(record),
        WindowCommand::Quit => {
            let app = running_application(record)?;
            if !app.terminate() {
                return Err(format!(
                    "{} did not accept the quit request",
                    record.app_name
                ));
            }
            Ok(CommandOutcome::ListChanges)
        }
        WindowCommand::Hide => {
            let app = running_application(record)?;
            if !app.hide() {
                return Err(format!("Could not hide {}", record.app_name));
            }
            Ok(CommandOutcome::ListChanges)
        }
    }
}

fn running_application(record: &WindowRecord) -> Result<Retained<NSRunningApplication>, String> {
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(record.pid)
        .ok_or_else(|| format!("{} is no longer running", record.app_name))?;
    // A reused pid after the listed app exited must not act on an unrelated process.
    if launch_time(&app) != record.launched_at {
        return Err(format!(
            "{} was replaced by another process; leaving it alone",
            record.app_name
        ));
    }
    Ok(app)
}

fn accessible(record: &WindowRecord) -> Result<&super::ax::AxElement, String> {
    record.ax.as_ref().ok_or_else(|| {
        format!(
            "{} is not reachable through Accessibility; grant AltTabio Accessibility access",
            record.title
        )
    })
}

fn terminate(record: &WindowRecord) -> Result<CommandOutcome, String> {
    let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(record.pid)
    else {
        return Ok(CommandOutcome::ListChanges);
    };
    // A reused pid after the listed app exited must not kill an unrelated process.
    if launch_time(&app) != record.launched_at {
        return Err(format!(
            "{} was replaced by another process; not terminating it",
            record.app_name
        ));
    }
    let result = unsafe {
        // SAFETY: kill has no memory preconditions; the pid was verified against its launch time.
        libc::kill(record.pid, libc::SIGKILL)
    };
    if result != 0 {
        return Err(format!(
            "Could not terminate {}: {}",
            record.app_name,
            std::io::Error::last_os_error()
        ));
    }
    Ok(CommandOutcome::ListChanges)
}

fn run_another_instance(record: &WindowRecord) -> Result<CommandOutcome, String> {
    let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(record.pid)
    else {
        return Err(format!("{} is no longer running", record.app_name));
    };
    let Some(url) = app.bundleURL() else {
        return Err(format!(
            "{} has no application bundle to launch",
            record.app_name
        ));
    };
    let configuration = NSWorkspaceOpenConfiguration::configuration();
    configuration.setCreatesNewApplicationInstance(true);
    NSWorkspace::sharedWorkspace().openApplicationAtURL_configuration_completionHandler(
        &url,
        &configuration,
        None,
    );
    Ok(CommandOutcome::ListChanges)
}
