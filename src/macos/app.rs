//! The one `App`: the switcher's state on the main thread, its start, and its shutdown.

mod panel;
mod pointer;
mod previews;
mod settings;
mod switching;
mod tap;
mod update;
mod window_model;

use super::alerts::show_about;
use super::ax::AppObserver;
use super::event_tap::EventTap;
use super::hotkey::{HotkeySettings, HotkeyState};
use super::overlay::Overlay;
use super::permissions;
use super::preview::PreviewSource;
use super::refresh_worker::RefreshWorker;
use super::runtime::{run_later, with_app};
use super::settings_window::SettingsWindow;
use super::status_item::{self, MenuAction, StatusItem};
use super::window_list::{WindowRecord, WindowlessApp};
use crate::settings_io::SettingsStore;
use alttabio::app_switcher::{AppSwitcher, RecentApps, WindowHistory};
use alttabio::panel_layout::{Extent, Shown};
use alttabio::panel_pointer::Pointer;
use alttabio::settings::Settings;
use alttabio::update::Updater;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSApplication, NSImage, NSWorkspace};
use objc2_foundation::{NSObjectProtocol, NSTimer};
use panel::Panel;
use previews::Preview;
use settings::hotkey_settings;
use std::collections::HashMap;
use std::rc::Rc;
use update::FIRST_UPDATE_CHECK_SECONDS;

pub(super) struct App {
    mtm: MainThreadMarker,
    settings: Settings,
    store: SettingsStore,
    preview_mode: bool,
    switcher: AppSwitcher,
    hotkey: HotkeyState,
    hotkey_settings: HotkeySettings,
    overlay: Option<Rc<Overlay>>,
    status_item: Option<StatusItem>,
    settings_window: Option<SettingsWindow>,
    event_tap: Option<EventTap>,
    observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    // Reports focus moving between the front app's windows and the windows it opens, which no
    // workspace notification does, so the list stays current without asking every app on a
    // timer.
    front_observer: Option<AppObserver>,
    refresh: RefreshWorker,
    records: Vec<WindowRecord>,
    windowless: Vec<WindowlessApp>,
    order: Vec<u32>,
    icons: HashMap<i32, Retained<NSImage>>,
    recent_apps: RecentApps,
    // Windows in the order they last had focus, which orders each app's windows.
    window_history: WindowHistory,
    extent: Extent,
    // The first tile and row drawn; they move only as far as the selection needs.
    tile_start: usize,
    row_start: usize,
    // What the last frame drew, for resolving pointer events against it.
    shown: Option<Shown>,
    panel: Panel,
    preview: PreviewSource,
    // This session's captures by window, the newest last, so returning to a window shows it
    // without capturing it again.
    previews: Vec<(u32, Preview)>,
    // The window being captured. One capture runs at a time; when it ends, the next one takes
    // whatever is selected by then.
    preview_in_flight: Option<u32>,
    // Counts switcher sessions, so a capture that ends after its session is dropped.
    session: u64,
    tap_retry_timer: Option<Retained<NSTimer>>,
    // Whether this run asked for Screen Recording. macOS shows its prompt only once anyway;
    // this keeps every later switch from asking again.
    screen_recording_asked: bool,
    pointer: Pointer,
    // Selects the app whose tile the pointer rests on, while it waits.
    dwell_timer: Option<Retained<NSTimer>>,
    // Preview mode shows the overlay as soon as the first window list arrives.
    show_when_listed: bool,
    // The front app named in the last secure-keyboard-input report, so the log says it once per
    // app rather than on every activation.
    secure_input_holder: Option<String>,
    updater: Updater,
    // The next automatic update check, while updates are automatic.
    update_timer: Option<Retained<NSTimer>>,
    // Looks for the user to step away while an installed update waits for its relaunch.
    away_timer: Option<Retained<NSTimer>>,
}

fn frontmost_pid() -> Option<u32> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .and_then(|app| u32::try_from(app.processIdentifier()).ok())
}

impl App {
    pub(super) fn new(
        mtm: MainThreadMarker,
        settings: Settings,
        store: SettingsStore,
        preview_mode: bool,
    ) -> Self {
        let hotkey_settings = hotkey_settings(&settings);
        let mut recent_apps = RecentApps::default();
        if let Some(pid) = frontmost_pid() {
            recent_apps.note(pid);
        }
        Self {
            mtm,
            settings,
            store,
            preview_mode,
            switcher: AppSwitcher::default(),
            hotkey: HotkeyState::default(),
            hotkey_settings,
            overlay: None,
            status_item: None,
            settings_window: None,
            event_tap: None,
            observers: Vec::new(),
            front_observer: None,
            refresh: RefreshWorker::spawn(),
            records: Vec::new(),
            windowless: Vec::new(),
            order: Vec::new(),
            icons: HashMap::new(),
            recent_apps,
            window_history: WindowHistory::default(),
            extent: Extent::default(),
            tile_start: 0,
            row_start: 0,
            shown: None,
            panel: Panel::Hidden,
            preview: PreviewSource::default(),
            previews: Vec::new(),
            preview_in_flight: None,
            session: 0,
            tap_retry_timer: None,
            screen_recording_asked: false,
            pointer: Pointer::default(),
            dwell_timer: None,
            show_when_listed: false,
            secure_input_holder: None,
            updater: Updater::default(),
            update_timer: None,
            away_timer: None,
        }
    }

    pub(super) fn start(&mut self) {
        let mtm = self.mtm;
        let overlay = Rc::new(Overlay::new(
            mtm,
            Rc::new(|event| {
                let _ = with_app(|app| app.handle_view_event(event));
            }),
        ));
        self.overlay = Some(overlay);
        self.status_item = Some(status_item::install(mtm, Rc::new(handle_menu_action)));
        self.observe_workspace();
        self.request_refresh();
        self.watch_front_app();

        if self.preview_mode {
            self.show_when_listed = true;
            return;
        }
        if self.settings.general.auto_update {
            self.schedule_update_check(FIRST_UPDATE_CHECK_SECONDS);
        }
        let trusted = permissions::accessibility_trusted(true);
        if !trusted {
            eprintln!(
                "AltTabio needs Accessibility access to see Command+Tab. Allow it in System \
                 Settings > Privacy & Security > Accessibility; the switcher starts working as \
                 soon as the access is granted."
            );
            // AltTabio has no Dock icon, so a dismissed prompt would leave nothing on screen
            // while ⌘ Tab still opens the system switcher. The settings window's Accessibility
            // row stays up instead, until the grant arrives.
            self.show_settings();
        }
        // Screen Recording is asked for after the first switch instead, once the preview area
        // has shown what it is for; see `hide_overlay`.
        if self.settings.appearance.preview && permissions::screen_recording_granted() {
            self.preview.refresh_content();
        }
        if !self.install_event_tap() {
            self.start_tap_retry_timer();
        }
    }

    fn shutdown(&mut self) {
        self.event_tap = None;
        self.cancel_dwell();
        self.front_observer = None;
        for timer in [
            self.tap_retry_timer.take(),
            self.update_timer.take(),
            self.away_timer.take(),
        ]
        .into_iter()
        .flatten()
        {
            timer.invalidate();
        }
    }
}

fn handle_menu_action(action: MenuAction) {
    match action {
        MenuAction::ShowSwitcher => {
            let _ = with_app(|app| app.show_overlay(None));
        }
        MenuAction::OpenSettings => {
            let _ = with_app(App::show_settings);
        }
        MenuAction::ShowAbout => {
            if let Some(mtm) = MainThreadMarker::new() {
                run_later(mtm, move || show_about(mtm));
            }
        }
        MenuAction::Update => {
            let _ = with_app(App::update_requested);
        }
        MenuAction::Quit => {
            let mtm = with_app(|app| {
                app.shutdown();
                app.mtm
            });
            if let Some(mtm) = mtm.or_else(MainThreadMarker::new) {
                NSApplication::sharedApplication(mtm).terminate(None);
            }
        }
    }
}
