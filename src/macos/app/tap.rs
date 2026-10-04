//! The event tap that hands ⌘ Tab to the switcher, and keeping it first in line.

use super::App;
use crate::macos::event_tap::EventTap;
use crate::macos::hotkey::TapEvent;
use crate::macos::permissions;
use crate::macos::runtime::{post_to_app, schedule_repeating, with_app};
use crate::macos::tracing;
use objc2_app_kit::NSWorkspace;

// How often a start without Accessibility access checks whether the grant has arrived.
const TAP_RETRY_SECONDS: f64 = 2.0;

impl App {
    /// Polls for the Accessibility grant so it is picked up while the app keeps running,
    /// which spares the user a relaunch.
    pub(super) fn start_tap_retry_timer(&mut self) {
        if self.tap_retry_timer.is_some() {
            return;
        }
        self.tap_retry_timer = Some(schedule_repeating(
            self.mtm,
            TAP_RETRY_SECONDS,
            App::retry_event_tap,
        ));
    }

    /// Records the new front app for the strip's order and follows its windows, then puts the
    /// tap back at the head of the session taps.
    ///
    /// Remote desktop and VM clients insert a tap of their own to hand ⌘ Tab to the guest; the
    /// system asks the newest head-inserted tap first, so reinserting ours keeps the switcher
    /// working inside those apps. Secure keyboard input is the one thing no tap gets past, so
    /// it is named in the log when it is on.
    pub(super) fn front_app_changed(&mut self) {
        self.note_front_app();
        self.watch_front_app();
        if self.preview_mode || self.event_tap.is_none() {
            return;
        }
        self.event_tap = None;
        if !self.install_event_tap() {
            self.start_tap_retry_timer();
        }
        let front = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .and_then(|app| app.localizedName())
            .map_or_else(|| "another app".to_owned(), |name| name.to_string());
        if tracing() {
            eprintln!("event tap reinserted at the head; front app: {front}");
        }
        if !permissions::secure_input_enabled() {
            self.secure_input_holder = None;
            return;
        }
        if self.secure_input_holder.as_deref() != Some(&front) {
            eprintln!(
                "Secure keyboard input is on while {front} is in front; macOS hides every key, \
                 including ⌘ Tab, from AltTabio until it ends."
            );
            self.secure_input_holder = Some(front);
        }
    }

    pub(super) fn install_event_tap(&mut self) -> bool {
        let handler =
            Box::new(|event: TapEvent| with_app(|app| app.handle_tap(event)).unwrap_or(false));
        match EventTap::install(handler) {
            Ok(tap) => {
                if tracing() {
                    eprintln!("event tap installed");
                }
                self.event_tap = Some(tap);
                true
            }
            Err(error) => {
                eprintln!("{error}");
                false
            }
        }
    }

    fn retry_event_tap(&mut self) {
        if self.event_tap.is_some() || !permissions::accessibility_trusted(false) {
            return;
        }
        if !self.install_event_tap() {
            return;
        }
        if let Some(timer) = self.tap_retry_timer.take() {
            timer.invalidate();
        }
        // The Accessibility row leaves the settings window as the grant arrives, which tells
        // the user it worked without bringing the window forward again.
        if let Some(window) = &self.settings_window {
            window.refresh();
        }
    }

    fn handle_tap(&mut self, event: TapEvent) -> bool {
        // The right-click menu tracks the keyboard on its own: the arrows, Return, Escape, and
        // its ⌘ letters. Keys the switcher swallowed here would never reach it.
        if self.switcher.context_menu_open()
            && matches!(event, TapEvent::KeyDown { .. } | TapEvent::KeyUp)
        {
            return false;
        }
        let event = match event {
            TapEvent::LeftMouseDown { .. } => TapEvent::LeftMouseDown {
                inside_overlay: self.switcher.is_active()
                    && self
                        .overlay
                        .as_ref()
                        .is_some_and(|overlay| overlay.contains_mouse()),
            },
            other => other,
        };
        let outcome = self.hotkey.process(event, self.hotkey_settings);
        if tracing() {
            eprintln!(
                "tap {event:?} -> suppress={} action={:?}",
                outcome.suppress, outcome.action
            );
        }
        if let Some(action) = outcome.action {
            post_to_app(move |app| app.apply_action(action));
        }
        outcome.suppress
    }
}
