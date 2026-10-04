//! The window list: when to ask for it, and what the switcher makes of it.

use super::{App, frontmost_pid};
use crate::macos::ax::{AppObserver, WindowChange};
use crate::macos::current_pid;
use crate::macos::refresh_worker::Job;
use crate::macos::runtime::{schedule, with_app};
use crate::macos::screen::cursor_display_bounds;
use crate::macos::window_list::{self, EnumerationOptions, Listing, WindowRecord, merge_order};
use alttabio::app_switcher::{AppEntry, WindowEntry, group_by_app};
use alttabio::switcher::ProcessIdentity;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSImage, NSRunningApplication, NSWorkspace, NSWorkspaceActiveSpaceDidChangeNotification,
    NSWorkspaceDidActivateApplicationNotification, NSWorkspaceDidHideApplicationNotification,
    NSWorkspaceDidLaunchApplicationNotification, NSWorkspaceDidTerminateApplicationNotification,
    NSWorkspaceDidUnhideApplicationNotification,
};
use objc2_foundation::{NSNotification, NSNotificationName, NSOperationQueue};
use std::ptr::NonNull;
use std::time::Duration;

fn record_process(record: &WindowRecord) -> ProcessIdentity {
    ProcessIdentity::new(
        u32::try_from(record.pid).unwrap_or_default(),
        record.launched_at,
    )
}

impl App {
    pub(super) fn observe_workspace(&mut self) {
        let (activated, refreshing) = unsafe {
            // SAFETY: the notification name constants are static strings exported by AppKit.
            (
                NSWorkspaceDidActivateApplicationNotification,
                [
                    NSWorkspaceDidActivateApplicationNotification,
                    NSWorkspaceDidLaunchApplicationNotification,
                    NSWorkspaceDidTerminateApplicationNotification,
                    NSWorkspaceDidHideApplicationNotification,
                    NSWorkspaceDidUnhideApplicationNotification,
                    NSWorkspaceActiveSpaceDidChangeNotification,
                ],
            )
        };
        for name in refreshing {
            self.observe(name, Self::request_refresh);
        }
        self.observe(activated, Self::front_app_changed);
    }

    /// Runs `work` whenever the workspace posts `name`.
    fn observe(&mut self, name: &NSNotificationName, work: fn(&mut Self)) {
        let block = RcBlock::new(move |_notification: NonNull<NSNotification>| {
            let _ = with_app(work);
        });
        let token = unsafe {
            // SAFETY: the main operation queue delivers the block on the main thread, where
            // `with_app` expects to run.
            NSWorkspace::sharedWorkspace()
                .notificationCenter()
                .addObserverForName_object_queue_usingBlock(
                    Some(name),
                    None,
                    Some(&NSOperationQueue::mainQueue()),
                    &block,
                )
        };
        self.observers.push(token);
    }

    pub(super) fn request_refresh(&mut self) {
        let display_bounds = self
            .settings
            .monitor
            .use_current_monitor_filter
            .then(|| cursor_display_bounds(self.mtm))
            .flatten();
        self.refresh.request(Job::List(EnumerationOptions {
            current_pid: current_pid(),
            display_bounds,
        }));
    }

    /// Moves the observer to the front app; the window list thread registers it.
    pub(super) fn watch_front_app(&mut self) {
        match frontmost_pid().and_then(|pid| i32::try_from(pid).ok()) {
            Some(pid) if pid != current_pid() => self.refresh.request(Job::Watch(pid)),
            _ => self.front_observer = None,
        }
    }

    pub(crate) fn observer_ready(&mut self, pid: i32, observer: Option<AppObserver>) {
        // Another app may have come to the front while this one was registering.
        if frontmost_pid().and_then(|front| i32::try_from(front).ok()) != Some(pid) {
            return;
        }
        if let Some(observer) = &observer {
            observer.attach();
        }
        self.front_observer = observer;
    }

    pub(crate) fn front_window_changed(&mut self, change: WindowChange) {
        match change {
            WindowChange::Focused => self.note_focus(),
            WindowChange::Opened => self.request_refresh(),
        }
    }

    pub(super) fn schedule_refresh_burst(&self) {
        for delay_ms in [120_u64, 450, 1_200] {
            let _timer = schedule(
                self.mtm,
                Duration::from_millis(delay_ms).as_secs_f64(),
                App::request_refresh,
            );
        }
    }

    pub(crate) fn refresh_completed(&mut self, listing: Listing) {
        let records = listing.windows;
        let on_screen = records
            .iter()
            .filter(|record| record.is_on_screen)
            .map(|record| record.window_id)
            .collect::<Vec<_>>();
        let others = records
            .iter()
            .filter(|record| !record.is_on_screen)
            .map(|record| record.window_id)
            .collect::<Vec<_>>();
        self.order = merge_order(&self.order, &on_screen, &others);
        // The front app's topmost window is the one with focus. The list refreshes on every
        // activation, and the front app's observer notes focus moving between its windows.
        let front = frontmost_pid();
        let focused = records
            .iter()
            .find(|record| record.is_on_screen && Some(record_process(record).id) == front)
            .and_then(|record| isize::try_from(record.window_id).ok());
        let listed = self.listed_windows();
        self.window_history.note(focused, &listed);
        let pids = records
            .iter()
            .map(|record| record.pid)
            .chain(listing.windowless.iter().map(|app| app.pid))
            .collect::<Vec<_>>();
        for pid in &pids {
            if !self.icons.contains_key(pid)
                && let Some(icon) = application_icon(*pid)
            {
                self.icons.insert(*pid, icon);
            }
        }
        self.icons.retain(|pid, _| pids.contains(pid));
        self.records = records;
        self.windowless = listing.windowless;
        if self.show_when_listed && !self.records.is_empty() {
            self.show_when_listed = false;
            self.show_overlay(None);
            return;
        }
        if self.switcher.is_active() {
            let before = self.switcher.selected_target();
            self.switcher.refresh(self.app_entries());
            if self.switcher.is_active() {
                self.extent.widen(self.switcher.apps());
                self.selection_changed(before);
            } else {
                self.hide_overlay();
            }
        }
    }

    pub(super) fn listed_windows(&self) -> Vec<isize> {
        self.order
            .iter()
            .filter_map(|id| isize::try_from(*id).ok())
            .collect()
    }

    /// Records the front app's topmost window as the one in focus. The window server knows the
    /// order, so no app is asked.
    pub(super) fn note_focus(&mut self) {
        let Some(front) = frontmost_pid() else {
            return;
        };
        let focused = i32::try_from(front)
            .map(window_list::on_screen_windows_of)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| isize::try_from(id).ok())
            .find(|handle| self.record(*handle).is_some());
        if focused.is_some() {
            let listed = self.listed_windows();
            self.window_history.note(focused, &listed);
        }
    }

    /// Records the front app as the most recently used one, for the strip's order.
    pub(super) fn note_front_app(&mut self) {
        if let Some(pid) = frontmost_pid() {
            self.recent_apps.note(pid);
        }
    }

    /// The listed windows grouped by app, the most recently used app first.
    pub(super) fn app_entries(&self) -> Vec<AppEntry> {
        let mut order = self.listed_windows();
        self.window_history.sort(&mut order);
        let windows = order
            .iter()
            .filter_map(|handle| self.record(*handle))
            .map(|record| WindowEntry {
                handle: isize::try_from(record.window_id).unwrap_or_default(),
                process: record_process(record),
                app_name: record.app_name.clone(),
            })
            .collect::<Vec<_>>();
        let windowless = self
            .windowless
            .iter()
            .map(|app| {
                (
                    ProcessIdentity::new(
                        u32::try_from(app.pid).unwrap_or_default(),
                        app.launched_at,
                    ),
                    app.name.clone(),
                )
            })
            .collect::<Vec<_>>();
        group_by_app(&windows, self.recent_apps.as_slice(), &windowless)
    }

    pub(super) fn record(&self, window_handle: isize) -> Option<&WindowRecord> {
        let id = u32::try_from(window_handle).ok()?;
        self.records.iter().find(|record| record.window_id == id)
    }

    pub(super) fn app_name(&self, process: ProcessIdentity) -> String {
        self.switcher
            .apps()
            .iter()
            .find(|app| app.process == process)
            .map(|app| app.name.clone())
            .unwrap_or_default()
    }
}

fn application_icon(pid: i32) -> Option<Retained<NSImage>> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?.icon()
}
