//! macOS adapter: `AppKit` overlay, `CGEventTap` hotkeys, and Accessibility window control.
//!
//! Every native callback (event tap, view events, timers, completion blocks) funnels into the
//! single `App` on the main thread through `with_app` or `post_to_app`; nothing native holds a
//! borrow across a nested run loop.

mod autostart;
mod ax;
mod commands;
mod event_tap;
mod hotkey;
mod keymap;
mod overlay;
mod permissions;
mod preview;
mod settings_window;
mod shortcuts;
mod single_instance;
mod status_item;
mod window_list;

use crate::settings_io::SettingsStore;
use alttabio::input::{InputAction, WindowCommand};
use alttabio::overlay_layout::{OverlayLayout, for_macos};
use alttabio::settings::Settings;
use alttabio::switcher::{
    ProcessIdentity, SwitchTask, SwitcherEffect, SwitcherSession, SwitcherSessionSettings,
    WindowCommandRequest,
};
use alttabio::theme::{ResolvedTheme, Rgb8, SwitcherTokens, resolve};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use event_tap::EventTap;
use hotkey::{HotkeySettings, HotkeyState, TapEvent};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSAlert, NSAlertStyle, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    NSApplicationActivationPolicy, NSColor, NSColorSpace, NSImage, NSRunningApplication, NSScreen,
    NSWorkspace, NSWorkspaceActiveSpaceDidChangeNotification,
    NSWorkspaceDidActivateApplicationNotification, NSWorkspaceDidHideApplicationNotification,
    NSWorkspaceDidLaunchApplicationNotification, NSWorkspaceDidTerminateApplicationNotification,
    NSWorkspaceDidUnhideApplicationNotification,
};
use objc2_core_foundation::CGPoint;
use objc2_foundation::{
    NSArray, NSNotification, NSNotificationName, NSObjectProtocol, NSOperationQueue, NSSize,
    NSString, NSTimer, NSURL,
};
use objc2_screen_capture_kit::SCShareableContent;
use overlay::{
    ActionPanelModel, CloseButtonVisualState, FrameModel, Hit, Overlay, RenderOptions, RowModel,
    ViewEvent, WindowState,
};
use preview::{CaptureRequest, PreviewResult, PreviewSource};
use settings_window::{SettingsEvent, SettingsWindow};
use shortcuts::{ACTIONS, ActionKind, FooterContext};
use status_item::{MenuAction, StatusItem};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;
use window_list::{EnumerationOptions, WindowRecord, merge_order};

const PREVIEW_INTERVAL_SECONDS: f64 = 0.15;
// A number key lights its row for about a menu blink before the switch happens.
const FLASH_SECONDS: f64 = 0.08;
const BACKGROUND_REFRESH_SECONDS: f64 = 2.0;
// How often a start without Accessibility access checks whether the grant has arrived.
const TAP_RETRY_SECONDS: f64 = 2.0;
const GITHUB_URL: &str = "https://github.com/vibeslop/AltTabio";

/// `ALTTABIO_TRACE=1` prints every tap event and switcher action to stderr for debugging.
fn tracing() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("ALTTABIO_TRACE").is_some_and(|value| value == "1"))
}

thread_local! {
    static APP: RefCell<Option<Rc<RefCell<App>>>> = const { RefCell::new(None) };
}

/// A value that only the main thread unwraps after it crossed a completion queue.
pub struct MainThreadValue<T>(pub T);

// SAFETY: every MainThreadValue is created on a background queue and unwrapped by
// `post_to_app` on the main thread; the wrapped object is never touched in between.
unsafe impl<T> Send for MainThreadValue<T> {}

fn with_app<R>(work: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|slot| {
        let slot = slot.borrow();
        let app = slot.as_ref()?;
        if let Ok(mut app) = app.try_borrow_mut() {
            Some(work(&mut app))
        } else {
            eprintln!("Dropped a callback because the app state is busy");
            None
        }
    })
}

pub fn post_to_app(work: impl FnOnce(&mut App) + Send + 'static) {
    DispatchQueue::main().exec_async(move || {
        let _ = with_app(work);
    });
}

/// Runs `work` on the next main run loop pass without holding any app-state borrow.
fn run_later(work: impl FnOnce() + 'static) {
    let slot = RefCell::new(Some(Box::new(work) as Box<dyn FnOnce()>));
    let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
        if let Some(work) = slot.borrow_mut().take() {
            work();
        }
    });
    let _timer = unsafe {
        // SAFETY: the timer is scheduled from the main thread onto the main run loop, so the
        // block runs on the same thread that created its non-Send captures.
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.0, false, &block)
    };
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
    let (store, mut settings) = match SettingsStore::load_from(path.clone()) {
        Ok(loaded) => loaded,
        Err(error) => {
            show_fatal_error(mtm, &error);
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

    let ns_app = NSApplication::sharedApplication(mtm);
    ns_app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let app = Rc::new(RefCell::new(App::new(
        mtm,
        settings,
        store,
        path,
        preview_mode,
    )));
    APP.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&app)));
    app.borrow_mut().start();
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

fn show_fatal_error(mtm: MainThreadMarker, message: &str) {
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Critical);
    alert.setMessageText(&NSString::from_str("AltTabio"));
    alert.setInformativeText(&NSString::from_str(message));
    let _response = alert.runModal();
}

fn print_window_list() {
    let records = window_list::enumerate(EnumerationOptions {
        current_pid: current_pid(),
        display_bounds: None,
    });
    println!(
        "{:>8}  {:>6}  {:<5} {:<24} TITLE",
        "ID", "PID", "STATE", "APP"
    );
    for record in records {
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
}

fn activate_from_command_line(argument: Option<&OsString>) {
    let Some(window_id) = argument
        .and_then(|value| value.to_str())
        .and_then(|value| value.parse::<u32>().ok())
    else {
        eprintln!("Usage: AltTabio --activate <window id from --list>");
        return;
    };
    let records = window_list::enumerate(EnumerationOptions {
        current_pid: current_pid(),
        display_bounds: None,
    });
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

fn current_pid() -> i32 {
    i32::try_from(std::process::id()).unwrap_or_default()
}

struct RefreshWorker {
    sender: mpsc::Sender<EnumerationOptions>,
}

impl RefreshWorker {
    fn spawn() -> Self {
        let (sender, receiver) = mpsc::channel::<EnumerationOptions>();
        std::thread::Builder::new()
            .name("alttabio-window-list".to_owned())
            .spawn(move || {
                while let Ok(mut options) = receiver.recv() {
                    // Coalesce bursts of notifications into one enumeration.
                    while let Ok(latest) = receiver.try_recv() {
                        options = latest;
                    }
                    let records = window_list::enumerate(options);
                    post_to_app(move |app| app.refresh_completed(records));
                }
            })
            .map_or_else(
                |error| {
                    eprintln!("Could not start the window list thread: {error}");
                    Self {
                        sender: mpsc::channel().0,
                    }
                },
                |_handle| Self { sender },
            )
    }

    fn request(&self, options: EnumerationOptions) {
        if self.sender.send(options).is_err() {
            eprintln!("The window list thread is gone; the switcher keeps its last list");
        }
    }
}

#[derive(Default)]
struct CloseButton {
    hovered: bool,
    pressed: bool,
}

impl CloseButton {
    fn visual_state(&self) -> CloseButtonVisualState {
        if self.pressed {
            CloseButtonVisualState::Pressed
        } else if self.hovered {
            CloseButtonVisualState::Hovered
        } else {
            CloseButtonVisualState::Normal
        }
    }
}

pub struct App {
    mtm: MainThreadMarker,
    settings: Settings,
    store: SettingsStore,
    settings_path: PathBuf,
    preview_mode: bool,
    session: SwitcherSession,
    hotkey: HotkeyState,
    hotkey_settings: HotkeySettings,
    overlay: Option<Rc<Overlay>>,
    status_item: Option<StatusItem>,
    settings_window: Option<SettingsWindow>,
    event_tap: Option<EventTap>,
    observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    refresh: RefreshWorker,
    records: Vec<WindowRecord>,
    order: Vec<u32>,
    icons: HashMap<i32, Retained<NSImage>>,
    preview: PreviewSource,
    preview_image: Option<Retained<NSImage>>,
    preview_message: Option<&'static str>,
    preview_window: Option<u32>,
    preview_in_flight: bool,
    preview_timer: Option<Retained<NSTimer>>,
    refresh_timer: Option<Retained<NSTimer>>,
    tap_retry_timer: Option<Retained<NSTimer>>,
    close_button: CloseButton,
    pressed_row: Option<usize>,
    // The row a number key picked; its switch waits for the flash timer.
    flash_position: Option<usize>,
    flash_timer: Option<Retained<NSTimer>>,
    // The selected entry of the ⌘K action panel while it is open.
    action_panel: Option<usize>,
    // Preview mode shows the overlay as soon as the first window list arrives.
    show_when_listed: bool,
}

/// Which rows the list shows: the first visible index and how many rows fit, minus one row for
/// the overflow note when the list does not fit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RowWindow {
    start: usize,
    rows: usize,
    hidden_above: usize,
    hidden_below: usize,
}

fn row_window(
    total: usize,
    fits: usize,
    range_for: impl Fn(usize) -> std::ops::Range<usize>,
) -> RowWindow {
    let rows = if total > fits {
        fits.saturating_sub(1).max(1)
    } else {
        fits
    };
    let range = range_for(rows);
    RowWindow {
        start: range.start,
        rows,
        hidden_above: range.start,
        hidden_below: total.saturating_sub(range.end),
    }
}

impl App {
    fn new(
        mtm: MainThreadMarker,
        settings: Settings,
        store: SettingsStore,
        settings_path: PathBuf,
        preview_mode: bool,
    ) -> Self {
        let session = SwitcherSession::new(session_settings(&settings));
        let hotkey_settings = hotkey_settings(&settings);
        Self {
            mtm,
            settings,
            store,
            settings_path,
            preview_mode,
            session,
            hotkey: HotkeyState::default(),
            hotkey_settings,
            overlay: None,
            status_item: None,
            settings_window: None,
            event_tap: None,
            observers: Vec::new(),
            refresh: RefreshWorker::spawn(),
            records: Vec::new(),
            order: Vec::new(),
            icons: HashMap::new(),
            preview: PreviewSource::default(),
            preview_image: None,
            preview_message: None,
            preview_window: None,
            preview_in_flight: false,
            preview_timer: None,
            refresh_timer: None,
            tap_retry_timer: None,
            close_button: CloseButton::default(),
            pressed_row: None,
            flash_position: None,
            flash_timer: None,
            action_panel: None,
            show_when_listed: false,
        }
    }

    fn start(&mut self) {
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
        self.preview.refresh_content();
        let refresh_block = RcBlock::new(|_timer: NonNull<NSTimer>| {
            let _ = with_app(App::request_refresh);
        });
        self.refresh_timer = Some(unsafe {
            // SAFETY: scheduled from the main thread onto the main run loop.
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(
                BACKGROUND_REFRESH_SECONDS,
                true,
                &refresh_block,
            )
        });

        if self.preview_mode {
            self.show_when_listed = true;
            return;
        }
        let trusted = permissions::accessibility_trusted(true);
        if !trusted {
            eprintln!(
                "AltTabio needs Accessibility access to see Command+Tab. Allow it in System \
                 Settings > Privacy & Security > Accessibility; the switcher starts working as \
                 soon as the access is granted."
            );
        }
        if !permissions::screen_recording_granted() && !permissions::request_screen_recording() {
            eprintln!(
                "Screen Recording is not granted; live previews stay blank until it is allowed."
            );
        }
        if !self.install_event_tap() {
            // The grant arrives while the app keeps running; polling spares the user a relaunch.
            let block = RcBlock::new(|_timer: NonNull<NSTimer>| {
                let _ = with_app(App::retry_event_tap);
            });
            self.tap_retry_timer = Some(unsafe {
                // SAFETY: scheduled from the main thread onto the main run loop.
                NSTimer::scheduledTimerWithTimeInterval_repeats_block(
                    TAP_RETRY_SECONDS,
                    true,
                    &block,
                )
            });
        }
    }

    fn install_event_tap(&mut self) -> bool {
        let handler = Box::new(|event: TapEvent, location: CGPoint| {
            with_app(|app| app.handle_tap(event, location)).unwrap_or(false)
        });
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
        if self.install_event_tap()
            && let Some(timer) = self.tap_retry_timer.take()
        {
            timer.invalidate();
        }
    }

    fn observe_workspace(&mut self) {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let names: [&NSNotificationName; 6] = unsafe {
            // SAFETY: the notification name constants are static strings exported by AppKit.
            [
                NSWorkspaceDidActivateApplicationNotification,
                NSWorkspaceDidLaunchApplicationNotification,
                NSWorkspaceDidTerminateApplicationNotification,
                NSWorkspaceDidHideApplicationNotification,
                NSWorkspaceDidUnhideApplicationNotification,
                NSWorkspaceActiveSpaceDidChangeNotification,
            ]
        };
        for name in names {
            let block = RcBlock::new(|_notification: NonNull<NSNotification>| {
                let _ = with_app(App::request_refresh);
            });
            let token = unsafe {
                // SAFETY: the main operation queue delivers the block on the main thread, where
                // `with_app` expects to run.
                center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    None,
                    Some(&NSOperationQueue::mainQueue()),
                    &block,
                )
            };
            self.observers.push(token);
        }
    }

    fn request_refresh(&mut self) {
        let display_bounds = self
            .settings
            .monitor
            .use_current_monitor_filter
            .then(|| cursor_display_bounds(self.mtm))
            .flatten();
        self.refresh.request(EnumerationOptions {
            current_pid: current_pid(),
            display_bounds,
        });
    }

    fn schedule_refresh_burst() {
        for delay_ms in [120_u64, 450, 1_200] {
            let block = RcBlock::new(|_timer: NonNull<NSTimer>| {
                let _ = with_app(App::request_refresh);
            });
            let _timer = unsafe {
                // SAFETY: scheduled from the main thread onto the main run loop.
                NSTimer::scheduledTimerWithTimeInterval_repeats_block(
                    Duration::from_millis(delay_ms).as_secs_f64(),
                    false,
                    &block,
                )
            };
        }
    }

    pub fn refresh_completed(&mut self, records: Vec<WindowRecord>) {
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
        for record in &records {
            if !self.icons.contains_key(&record.pid)
                && let Some(icon) = application_icon(record.pid)
            {
                self.icons.insert(record.pid, icon);
            }
        }
        self.icons
            .retain(|pid, _| records.iter().any(|record| record.pid == *pid));
        self.records = records;
        if self.show_when_listed && !self.records.is_empty() {
            self.show_when_listed = false;
            self.show_overlay(None);
            return;
        }
        if self.session.is_visible() {
            self.session.refresh_tasks(self.tasks());
            if self.session.is_visible() {
                self.redraw();
                self.request_preview_capture();
            } else {
                self.hide_overlay();
            }
        }
    }

    fn tasks(&self) -> Vec<SwitchTask> {
        self.order
            .iter()
            .filter_map(|id| self.records.iter().find(|record| record.window_id == *id))
            .enumerate()
            .map(|(index, record)| {
                SwitchTask::new(
                    index + 1,
                    isize::try_from(record.window_id).unwrap_or_default(),
                    &record.title,
                    &record.app_name,
                )
                .with_process_identity(ProcessIdentity::new(
                    u32::try_from(record.pid).unwrap_or_default(),
                    record.launched_at,
                ))
                .with_icon_handle(isize::try_from(record.pid).unwrap_or_default())
            })
            .collect()
    }

    fn record(&self, window_handle: isize) -> Option<&WindowRecord> {
        let id = u32::try_from(window_handle).ok()?;
        self.records.iter().find(|record| record.window_id == id)
    }

    fn handle_tap(&mut self, event: TapEvent, location: CGPoint) -> bool {
        let event = match event {
            TapEvent::LeftMouseDown { .. } => TapEvent::LeftMouseDown {
                inside_overlay: self
                    .overlay
                    .as_ref()
                    .is_some_and(|overlay| overlay.contains_mouse()),
            },
            other => other,
        };
        let held_before = self.hotkey.held_modifier();
        let outcome = self.hotkey.process(event, self.hotkey_settings);
        if self.hotkey.held_modifier() != held_before && self.session.is_visible() {
            // The keycaps and hint bar follow the modifier; nothing else changes on a bare
            // modifier transition, so the switcher session is not involved.
            post_to_app(App::redraw);
        }
        if tracing() {
            eprintln!(
                "tap {event:?} -> suppress={} actions={:?}",
                outcome.suppress,
                outcome.actions().collect::<Vec<_>>()
            );
        }
        if self.hotkey.take_synthetic_right_release() {
            EventTap::post_right_button_release(location);
        }
        for action in outcome.actions() {
            post_to_app(move |app| app.apply_action(action));
        }
        outcome.suppress
    }

    pub fn apply_action(&mut self, action: InputAction) {
        if self.session.is_visible() && self.route_to_action_panel(action) {
            return;
        }
        if let InputAction::ActivateVisiblePosition(position) = action
            && self.session.is_visible()
            && self.flash_position.is_none()
            && self
                .session
                .switcher_mut()
                .select_visible_position(position)
        {
            self.flash_position = Some(position);
            self.redraw();
            let block = RcBlock::new(|_timer: NonNull<NSTimer>| {
                let _ = with_app(App::finish_flash);
            });
            self.flash_timer = Some(unsafe {
                // SAFETY: scheduled from the main thread onto the main run loop.
                NSTimer::scheduledTimerWithTimeInterval_repeats_block(FLASH_SECONDS, false, &block)
            });
            return;
        }
        let selected_before = self.selected_window();
        let effect = self.session.handle_input(action);
        if tracing() {
            eprintln!("action {action:?} -> {effect:?}");
        }
        match effect {
            SwitcherEffect::None => {}
            SwitcherEffect::Open { selection_delta } => self.show_overlay(selection_delta),
            SwitcherEffect::Hide => self.hide_overlay(),
            SwitcherEffect::Redraw => {
                self.redraw();
                if self.selected_window() != selected_before {
                    self.request_preview_capture();
                }
            }
            SwitcherEffect::Activate(target) => self.activate_target(target),
            SwitcherEffect::Execute(request) => self.execute_command(request),
        }
    }

    /// Handles `action` for the ⌘K panel; true when the panel consumed it.
    fn route_to_action_panel(&mut self, action: InputAction) -> bool {
        if action == InputAction::ToggleActionPanel {
            self.action_panel = if self.action_panel.is_some() {
                None
            } else {
                self.session.switcher().selected_task().map(|_| 0)
            };
            self.redraw();
            return true;
        }
        let Some(selected) = self.action_panel else {
            return false;
        };
        let last = ACTIONS.len().saturating_sub(1);
        match action {
            InputAction::Navigate(delta) | InputAction::Switch(delta) => {
                self.action_panel = Some(
                    usize::try_from(i64::try_from(selected).unwrap_or_default() + i64::from(delta))
                        .unwrap_or_default()
                        .min(last),
                );
                self.redraw();
            }
            InputAction::MouseWheel(delta) => {
                self.action_panel = Some(if delta > 0 {
                    selected.saturating_sub(1)
                } else {
                    (selected + 1).min(last)
                });
                self.redraw();
            }
            InputAction::SelectFirst => {
                self.action_panel = Some(0);
                self.redraw();
            }
            InputAction::SelectLast => {
                self.action_panel = Some(last);
                self.redraw();
            }
            InputAction::ActivateSelected => self.run_action(selected),
            InputAction::DismissOverlay => {
                self.action_panel = None;
                self.redraw();
            }
            // Letting go of the modifier while choosing an action must not switch windows; the
            // list simply stays open, which is where the panel leads anyway.
            InputAction::AltReleased
            | InputAction::RightButtonReleased
            | InputAction::AppendSearchCharacter(_)
            | InputAction::BackspaceSearch
            | InputAction::RightButtonPressed
            | InputAction::ToggleActionPanel => {}
            // Shortcuts and number keys work as they do without the panel; it just closes.
            InputAction::CloseSelected
            | InputAction::WindowCommand(_)
            | InputAction::SwitchWithinProcess(_)
            | InputAction::ActivateVisiblePosition(_) => {
                self.action_panel = None;
                return false;
            }
        }
        true
    }

    fn run_action(&mut self, index: usize) {
        self.action_panel = None;
        let Some(action) = ACTIONS.get(index) else {
            self.redraw();
            return;
        };
        let input = match action.kind {
            ActionKind::Activate => InputAction::ActivateSelected,
            ActionKind::Command(command) => InputAction::WindowCommand(command),
            ActionKind::NextWindowOfApp => InputAction::SwitchWithinProcess(1),
        };
        self.apply_action(input);
        if self.session.is_visible() {
            self.redraw();
        }
    }

    fn finish_flash(&mut self) {
        self.flash_timer = None;
        if self.flash_position.take().is_none() {
            return;
        }
        // The row was selected when the flash started; activating the selection rather than the
        // position keeps the choice even if the list changed underneath in the meantime.
        if self.session.is_visible() {
            self.apply_action(InputAction::ActivateSelected);
        }
    }

    fn clear_flash(&mut self) {
        self.flash_position = None;
        if let Some(timer) = self.flash_timer.take() {
            timer.invalidate();
        }
    }

    fn selected_window(&self) -> Option<isize> {
        self.session
            .switcher()
            .selected_task()
            .map(|task| task.window_handle)
    }

    fn show_overlay(&mut self, selection_delta: Option<i32>) {
        let tasks = self.tasks();
        self.session.open(tasks, selection_delta);
        if !self.session.is_visible() {
            self.hide_overlay();
            return;
        }
        self.close_button = CloseButton::default();
        self.pressed_row = None;
        self.clear_flash();
        self.action_panel = None;
        let theme = self.resolved_theme();
        if let Some(overlay) = &self.overlay {
            overlay.set_theme(theme, Self::tokens(theme));
            overlay.show_on_cursor_screen();
        }
        self.hotkey.set_overlay_active(true);
        self.preview_image = None;
        self.preview_window = None;
        self.preview_message = None;
        self.preview.refresh_content();
        self.request_refresh();
        self.start_preview_timer();
        self.redraw();
        self.request_preview_capture();
    }

    fn hide_overlay(&mut self) {
        self.session.hide();
        if let Some(overlay) = &self.overlay {
            overlay.hide();
        }
        self.hotkey.set_overlay_active(false);
        self.close_button = CloseButton::default();
        self.pressed_row = None;
        self.clear_flash();
        self.action_panel = None;
        self.preview_image = None;
        self.preview_window = None;
        if let Some(timer) = self.preview_timer.take() {
            timer.invalidate();
        }
        if self.preview_mode {
            self.shutdown();
            NSApplication::sharedApplication(self.mtm).terminate(None);
        }
    }

    fn activate_target(&mut self, target: isize) {
        if let Some(record) = self.record(target).cloned() {
            if let Err(error) = commands::activate(&record) {
                eprintln!("{error}");
            }
        } else {
            eprintln!("The selected window is no longer listed");
        }
        self.hide_overlay();
        Self::schedule_refresh_burst();
    }

    fn execute_command(&mut self, request: WindowCommandRequest) {
        let Some(record) = self.record(request.window_handle).cloned() else {
            eprintln!("The selected window is no longer listed");
            return;
        };
        if let Err(error) = commands::execute(request.command, &record) {
            eprintln!("{error}");
            return;
        }
        // `refresh_tasks` keeps the selection, so redrawing after each refresh shows the closed
        // window leaving the list in place.
        self.request_refresh();
        Self::schedule_refresh_burst();
        if matches!(
            request.command,
            WindowCommand::Maximize | WindowCommand::Restore
        ) {
            self.request_preview_capture();
        }
    }

    fn resolved_theme(&self) -> ResolvedTheme {
        let appearance = NSApplication::sharedApplication(self.mtm).effectiveAppearance();
        let (aqua, dark) = unsafe {
            // SAFETY: the appearance name constants are static strings exported by AppKit.
            (NSAppearanceNameAqua, NSAppearanceNameDarkAqua)
        };
        let names = NSArray::from_slice(&[aqua, dark]);
        let system = if appearance
            .bestMatchFromAppearancesWithNames(&names)
            .is_some_and(|name| &*name == dark)
        {
            ResolvedTheme::Dark
        } else {
            ResolvedTheme::Light
        };
        resolve(self.settings.appearance.theme, system)
    }

    fn tokens(theme: ResolvedTheme) -> SwitcherTokens {
        SwitcherTokens::new(theme, accent_color())
    }

    fn layout(&self) -> OverlayLayout {
        let search_row = if self.session.switcher().filter().is_empty() {
            0.0
        } else {
            overlay::SEARCH_ROW_HEIGHT
        };
        let footer = if self.settings.appearance.show_hints {
            overlay::FOOTER_HEIGHT
        } else {
            0.0
        };
        for_macos(self.settings.appearance.compact_list)
            .with_search_row(search_row)
            .with_footer(footer)
    }

    fn row_window(&self, size: (f64, f64), layout: OverlayLayout) -> RowWindow {
        let switcher = self.session.switcher();
        row_window(
            switcher.visible_task_count(),
            overlay::visible_rows(size, layout),
            |rows| switcher.visible_range(rows),
        )
    }

    fn footer_context(&self) -> FooterContext {
        let switcher = self.session.switcher();
        if !switcher.filter().is_empty() {
            FooterContext::Searching {
                matches: switcher.visible_task_count(),
            }
        } else if let Some(modifier) = self.hotkey.held_modifier() {
            FooterContext::Held(modifier)
        } else {
            FooterContext::Released
        }
    }

    fn window_state(&self, window_handle: isize) -> WindowState {
        match self.record(window_handle) {
            Some(record) if record.is_minimized => WindowState::Minimized,
            Some(record) if record.is_hidden => WindowState::Hidden,
            Some(record) if !record.is_on_screen => WindowState::OtherSpace,
            _ => WindowState::Normal,
        }
    }

    fn render_options(&self) -> RenderOptions {
        let appearance = &self.settings.appearance;
        RenderOptions {
            compact_list: appearance.compact_list,
            large_icons: appearance.large_icons,
            show_numbers: appearance.show_numbers,
            show_app_names: appearance.show_app_names,
            visible_borders: appearance.visible_borders,
            preview: appearance.preview,
        }
    }

    fn redraw(&mut self) {
        let Some(overlay) = self.overlay.clone() else {
            return;
        };
        let size = overlay.content_size();
        let layout = self.layout();
        let window = self.row_window(size, layout);
        let switcher = self.session.switcher();
        let selected = switcher.selected_task().map(|task| task.window_handle);
        let rows = switcher
            .positioned_visible_tasks()
            .skip(window.start)
            .take(window.rows)
            .map(|(position, task)| RowModel {
                position,
                title: task.title.clone(),
                app_name: task.process_name.clone(),
                icon: i32::try_from(task.icon_handle)
                    .ok()
                    .and_then(|pid| self.icons.get(&pid).cloned()),
                selected: selected == Some(task.window_handle),
                state: self.window_state(task.window_handle),
            })
            .collect();
        let no_selection = switcher.selected_task().is_none();
        let preview_message = if no_selection {
            Some("No windows match".to_owned())
        } else if self.preview_image.is_some() {
            None
        } else if !permissions::screen_recording_granted() {
            Some("Allow Screen Recording in System Settings to see live previews".to_owned())
        } else {
            self.preview_message.map(str::to_owned)
        };
        let footer = self
            .settings
            .appearance
            .show_hints
            .then(|| shortcuts::footer(self.footer_context(), self.action_panel.is_some()));
        let action_panel = self.action_panel.map(|selected| ActionPanelModel {
            selected,
            target: switcher
                .selected_task()
                .map(|task| task.title.clone())
                .unwrap_or_default(),
        });
        overlay.present(FrameModel {
            rows,
            layout,
            options: self.render_options(),
            tokens: Self::tokens(self.resolved_theme()),
            close_state: self.close_button.visual_state(),
            preview: if no_selection {
                None
            } else {
                self.preview_image.clone()
            },
            preview_message,
            filter: switcher.filter().to_owned(),
            held_modifier: self.hotkey.held_modifier(),
            flash_position: self.flash_position,
            hidden_above: window.hidden_above,
            hidden_below: window.hidden_below,
            footer,
            action_panel,
        });
    }

    fn start_preview_timer(&mut self) {
        if self.preview_timer.is_some() || !self.settings.appearance.preview {
            return;
        }
        let block = RcBlock::new(|_timer: NonNull<NSTimer>| {
            let _ = with_app(App::request_preview_capture);
        });
        self.preview_timer = Some(unsafe {
            // SAFETY: scheduled from the main thread onto the main run loop.
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(
                PREVIEW_INTERVAL_SECONDS,
                true,
                &block,
            )
        });
    }

    fn request_preview_capture(&mut self) {
        if !self.session.is_visible() || !self.settings.appearance.preview || self.preview_in_flight
        {
            return;
        }
        let Some(overlay) = &self.overlay else {
            return;
        };
        let Some(window_id) = self
            .selected_window()
            .and_then(|handle| u32::try_from(handle).ok())
        else {
            return;
        };
        if !permissions::screen_recording_granted() {
            return;
        }
        if !self.preview.has_content() {
            self.preview.refresh_content();
            return;
        }
        let area = overlay::preview_rect(overlay.content_size(), self.layout());
        let scale = overlay.backing_scale();
        let request = CaptureRequest {
            window_id,
            full_desktop: self.settings.appearance.full_desktop_preview,
            pixel_width: pixel_length(area.width, scale),
            pixel_height: pixel_length(area.height, scale),
        };
        match self.preview.capture(&request) {
            Ok(()) => self.preview_in_flight = true,
            Err(message) => {
                if self.preview_window != Some(window_id) {
                    self.preview_image = None;
                    self.preview_window = Some(window_id);
                }
                self.preview_message = Some(message);
                self.redraw();
            }
        }
    }

    pub fn preview_content_ready(
        &mut self,
        content: MainThreadValue<Option<Retained<SCShareableContent>>>,
    ) {
        self.preview.set_content(content.0);
        if self.session.is_visible() {
            self.request_preview_capture();
        }
    }

    pub fn preview_captured(&mut self, window_id: u32, result: MainThreadValue<PreviewResult>) {
        self.preview_in_flight = false;
        let selected = self
            .selected_window()
            .and_then(|handle| u32::try_from(handle).ok());
        if !self.session.is_visible() || selected != Some(window_id) {
            return;
        }
        match result.0 {
            PreviewResult::Image(image) => {
                let ns_image =
                    NSImage::initWithCGImage_size(NSImage::alloc(), &image, NSSize::ZERO);
                self.preview_image = Some(ns_image);
                self.preview_message = None;
            }
            PreviewResult::Unavailable(message) => {
                if self.preview_window != Some(window_id) {
                    self.preview_image = None;
                }
                self.preview_message = Some(message);
            }
        }
        self.preview_window = Some(window_id);
        self.redraw();
    }

    fn handle_view_event(&mut self, event: ViewEvent) {
        if !self.session.is_visible() {
            return;
        }
        let Some(overlay) = self.overlay.clone() else {
            return;
        };
        let size = overlay.content_size();
        let layout = self.layout();
        let window = self.row_window(size, layout);
        let (start, visible_rows) = (window.start, window.rows);
        let selected_row = self
            .session
            .switcher()
            .selected_visible_index()
            .and_then(|index| index.checked_sub(start))
            .filter(|row| *row < visible_rows);
        let panel_open = self.action_panel.is_some();
        // The overflow note occupies the last fitted slot; it is not a row.
        let hit_test = |x: f64, y: f64| {
            overlay::hit_test(size, layout, selected_row, panel_open, x, y).filter(
                |hit| match hit {
                    Hit::Row(row) | Hit::CloseButton(row) => *row < visible_rows,
                    Hit::ActionsButton | Hit::ActionRow(_) => true,
                },
            )
        };
        match event {
            ViewEvent::MouseMoved(x, y) => {
                let hit = hit_test(x, y);
                let hovered = matches!(hit, Some(Hit::CloseButton(_)));
                let mut changed = hovered != self.close_button.hovered;
                self.close_button.hovered = hovered;
                if let Some(Hit::ActionRow(index)) = hit
                    && self.action_panel.is_some_and(|selected| selected != index)
                {
                    self.action_panel = Some(index);
                    changed = true;
                }
                if let Some(Hit::Row(row)) = hit
                    && self.settings.general.mouse_over_selection
                    && !self.close_button.pressed
                    && selected_row != Some(row)
                {
                    let switcher = self.session.switcher_mut();
                    switcher.pin_visible_range(visible_rows);
                    if switcher.select_visible_position(start + row + 1) {
                        changed = true;
                        self.request_preview_capture();
                    }
                }
                if changed {
                    self.redraw();
                }
            }
            ViewEvent::MouseDown(x, y) => {
                self.handle_mouse_down(hit_test(x, y), start, visible_rows);
            }
            ViewEvent::MouseUp(x, y) => self.handle_mouse_up(hit_test(x, y)),
            ViewEvent::RightMouseDown(x, y) => {
                if let Some(Hit::Row(row)) = hit_test(x, y) {
                    let switcher = self.session.switcher_mut();
                    switcher.pin_visible_range(visible_rows);
                    if switcher.select_visible_position(start + row + 1) {
                        self.redraw();
                    }
                    if self.session.open_context_menu() {
                        run_later(move || {
                            let command = overlay.show_context_menu(x, y);
                            let _ = with_app(|app| app.finish_context_menu(command));
                        });
                    }
                }
            }
            ViewEvent::MouseExited => {
                if self.close_button.hovered {
                    self.close_button.hovered = false;
                    self.redraw();
                }
            }
            ViewEvent::Scroll(delta) => self.apply_action(InputAction::MouseWheel(delta)),
        }
    }

    fn handle_mouse_down(&mut self, hit: Option<Hit>, start: usize, visible_rows: usize) {
        match hit {
            Some(Hit::ActionsButton) => self.apply_action(InputAction::ToggleActionPanel),
            Some(Hit::ActionRow(index)) => self.run_action(index),
            Some(Hit::CloseButton(_)) => {
                self.close_button.pressed = true;
                self.redraw();
            }
            Some(Hit::Row(row)) => {
                self.pressed_row = Some(row);
                self.action_panel = None;
                let switcher = self.session.switcher_mut();
                switcher.pin_visible_range(visible_rows);
                if switcher.select_visible_position(start + row + 1) {
                    self.redraw();
                    self.request_preview_capture();
                }
            }
            None => {}
        }
    }

    fn handle_mouse_up(&mut self, hit: Option<Hit>) {
        if self.close_button.pressed {
            self.close_button.pressed = false;
            if matches!(hit, Some(Hit::CloseButton(_))) {
                self.apply_action(InputAction::CloseSelected);
            }
            self.redraw();
        } else if let (Some(Hit::Row(row)), Some(pressed)) = (hit, self.pressed_row)
            && row == pressed
        {
            self.apply_action(InputAction::ActivateSelected);
        }
        self.pressed_row = None;
    }

    fn finish_context_menu(&mut self, command: Option<WindowCommand>) {
        if let SwitcherEffect::Execute(request) = self.session.finish_context_menu(command) {
            self.execute_command(request);
        }
    }

    fn show_settings(&mut self) {
        if self.settings_window.is_none() {
            let handler: Rc<dyn Fn(SettingsEvent)> = Rc::new(|event| match event {
                SettingsEvent::Changed(settings) => {
                    let _ = with_app(|app| app.apply_settings(settings));
                }
                SettingsEvent::OpenAccessibility => permissions::open_accessibility_settings(),
                SettingsEvent::OpenScreenRecording => {
                    permissions::open_screen_recording_settings();
                }
            });
            self.settings_window = Some(SettingsWindow::new(
                self.mtm,
                &self.settings,
                &self.settings_path.display().to_string(),
                handler,
            ));
        }
        if let Some(window) = &self.settings_window {
            window.show(&self.settings);
        }
    }

    fn apply_settings(&mut self, settings: Settings) {
        let autostart_changed = settings.general.autostart != self.settings.general.autostart;
        self.settings = settings;
        self.hotkey_settings = hotkey_settings(&self.settings);
        self.session
            .update_settings(session_settings(&self.settings));
        if let Err(error) = self.store.save(&self.settings) {
            eprintln!("{error}");
        }
        if autostart_changed
            && let Err(error) = autostart::set_enabled(self.settings.general.autostart)
        {
            let mtm = self.mtm;
            run_later(move || show_fatal_error(mtm, &error));
        }
        if self.session.is_visible() {
            if let Some(overlay) = &self.overlay {
                let theme = self.resolved_theme();
                overlay.set_theme(theme, Self::tokens(theme));
            }
            self.redraw();
        }
    }

    fn shutdown(&mut self) {
        self.event_tap = None;
        self.clear_flash();
        if let Some(timer) = self.preview_timer.take() {
            timer.invalidate();
        }
        if let Some(timer) = self.refresh_timer.take() {
            timer.invalidate();
        }
        if let Some(timer) = self.tap_retry_timer.take() {
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
                run_later(move || show_about(mtm));
            }
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

fn show_about(mtm: MainThreadMarker) {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str("AltTabio"));
    alert.setInformativeText(&NSString::from_str(concat!(
        "Version ",
        env!("CARGO_PKG_VERSION"),
        "\nOpen-source window switcher for macOS and Windows.\nCopyright (c) 2026 VibeSlop"
    )));
    let _ok = alert.addButtonWithTitle(&NSString::from_str("OK"));
    let _github = alert.addButtonWithTitle(&NSString::from_str("GitHub"));
    #[allow(
        deprecated,
        reason = "an accessory app has no other way to bring its alert forward"
    )]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    // The second button returns NSAlertSecondButtonReturn (1001).
    if alert.runModal() == 1001
        && let Some(url) = NSURL::URLWithString(&NSString::from_str(GITHUB_URL))
        && !NSWorkspace::sharedWorkspace().openURL(&url)
    {
        eprintln!("Could not open {GITHUB_URL}");
    }
}

/// The user's accent color in sRGB, falling back to the system blue.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "components are clamped to 0..=1 before scaling to a byte"
)]
fn accent_color() -> Rgb8 {
    let fallback = Rgb8::new(0, 122, 255);
    let Some(color) =
        NSColor::controlAccentColor().colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())
    else {
        return fallback;
    };
    let byte = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb8::new(
        byte(color.redComponent()),
        byte(color.greenComponent()),
        byte(color.blueComponent()),
    )
}

fn application_icon(pid: i32) -> Option<Retained<NSImage>> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?.icon()
}

/// Bounds of the display under the cursor in top-left window-list coordinates.
fn cursor_display_bounds(mtm: MainThreadMarker) -> Option<[f64; 4]> {
    let location = objc2_app_kit::NSEvent::mouseLocation();
    let screens = NSScreen::screens(mtm);
    let primary_height = screens.iter().next()?.frame().size.height;
    let screen = screens.iter().find(|screen| {
        let frame = screen.frame();
        location.x >= frame.origin.x
            && location.x < frame.origin.x + frame.size.width
            && location.y >= frame.origin.y
            && location.y < frame.origin.y + frame.size.height
    })?;
    let frame = screen.frame();
    Some([
        frame.origin.x,
        primary_height - (frame.origin.y + frame.size.height),
        frame.size.width,
        frame.size.height,
    ])
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "preview areas are small positive point sizes"
)]
fn pixel_length(points: f64, scale: f64) -> usize {
    (points * scale).round().max(1.0) as usize
}

fn session_settings(settings: &Settings) -> SwitcherSessionSettings {
    SwitcherSessionSettings {
        typed_search: settings.general.typed_search,
        release_alt_switches: settings.general.release_alt_switches,
        release_right_button_switches: settings.general.release_right_button_switches,
    }
}

fn hotkey_settings(settings: &Settings) -> HotkeySettings {
    HotkeySettings {
        command_tab: settings.general.replace_alt_tab,
        option_tab: settings.general.replace_win_tab,
        typed_search: settings.general.typed_search,
        right_button_wheel_switching: settings.general.right_button_wheel_switching,
    }
}

#[cfg(test)]
mod tests {
    use super::{RowWindow, row_window};

    #[test]
    fn lists_that_fit_show_every_row_without_an_overflow_slot() {
        assert_eq!(
            row_window(5, 8, |rows| 0..5.min(rows)),
            RowWindow {
                start: 0,
                rows: 8,
                hidden_above: 0,
                hidden_below: 0,
            }
        );
    }

    #[test]
    fn overflowing_lists_give_up_one_row_for_the_note_and_count_the_rest() {
        assert_eq!(
            row_window(20, 8, |rows| 3..3 + rows),
            RowWindow {
                start: 3,
                rows: 7,
                hidden_above: 3,
                hidden_below: 10,
            }
        );
        assert_eq!(row_window(3, 1, |rows| 1..1 + rows).rows, 1);
    }
}
