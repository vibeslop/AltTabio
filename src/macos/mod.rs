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
mod single_instance;
mod status_item;
mod window_list;

use crate::settings_io::SettingsStore;
use alttabio::app_switcher::{
    Action, AppEntry, AppSwitcher, Effect, Target, WindowEntry, WindowHistory, group_by_app,
};
use alttabio::input::WindowCommand;
use alttabio::settings::Settings;
use alttabio::switcher::ProcessIdentity;
use alttabio::theme::{ResolvedTheme, SwitcherTokens, resolve};
use block2::RcBlock;
use commands::AppRef;
use dispatch2::DispatchQueue;
use event_tap::EventTap;
use hotkey::{HotkeySettings, HotkeyState, TapEvent};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSAlert, NSAlertStyle, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    NSApplicationActivationPolicy, NSEvent, NSImage, NSRunningApplication, NSScreen, NSWorkspace,
    NSWorkspaceActiveSpaceDidChangeNotification, NSWorkspaceDidActivateApplicationNotification,
    NSWorkspaceDidHideApplicationNotification, NSWorkspaceDidLaunchApplicationNotification,
    NSWorkspaceDidTerminateApplicationNotification, NSWorkspaceDidUnhideApplicationNotification,
};
use objc2_foundation::{
    NSArray, NSNotification, NSNotificationName, NSObjectProtocol, NSOperationQueue, NSPoint,
    NSSize, NSString, NSTimer, NSURL,
};
use objc2_screen_capture_kit::SCShareableContent;
use overlay::{
    CloseButtonVisualState, FrameModel, Hit, Layout, Overlay, PreviewModel, Row, Tile, ViewEvent,
    WindowState, scroll_into_view,
};
use preview::{CaptureRequest, PreviewResult, PreviewSource};
use settings_window::{SettingsEvent, SettingsWindow};
use status_item::{MenuAction, StatusItem};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;
use window_list::{EnumerationOptions, Listing, WindowRecord, WindowlessApp, merge_order};

const PREVIEW_INTERVAL_SECONDS: f64 = 0.15;
// The panel waits this long after ⌘ Tab. A quick press and release switches before it passes,
// so flipping between two windows never flashes the panel, as with the system switcher.
const REVEAL_SECONDS: f64 = 0.12;
const BACKGROUND_REFRESH_SECONDS: f64 = 2.0;
// How often a start without Accessibility access checks whether the grant has arrived.
const TAP_RETRY_SECONDS: f64 = 2.0;
// How long the pointer rests on an app's tile before the app is selected, so a pointer that
// crosses the strip on its way to a window does not change the app underneath it.
const TILE_DWELL_SECONDS: f64 = 0.08;
// How far the pointer travels after the panel appears before hovering selects anything.
const HOVER_ARM_DISTANCE: f64 = 8.0;
// Activation history kept for ordering the strip; apps activated longer ago than this follow
// in window order, which is what they would get anyway.
const RECENT_APPS_KEPT: usize = 64;
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

/// Schedules `work` on the main run loop after `seconds`.
fn schedule(seconds: f64, work: impl Fn(&mut App) + 'static) -> Retained<NSTimer> {
    let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
        let _ = with_app(&work);
    });
    unsafe {
        // SAFETY: scheduled from the main thread onto the main run loop, where the block's
        // captures were created.
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(seconds, false, &block)
    }
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
    APP.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&app)));
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

fn show_fatal_error(mtm: MainThreadMarker, message: &str) {
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Critical);
    alert.setMessageText(&NSString::from_str("AltTabio"));
    alert.setInformativeText(&NSString::from_str(message));
    let _response = alert.runModal();
}

fn print_window_list() {
    let listing = window_list::enumerate(EnumerationOptions {
        current_pid: current_pid(),
        display_bounds: None,
    });
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
    })
    .windows;
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
                    let listing = window_list::enumerate(options);
                    post_to_app(move |app| app.refresh_completed(listing));
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
    preview_mode: bool,
    switcher: AppSwitcher,
    hotkey: HotkeyState,
    hotkey_settings: HotkeySettings,
    overlay: Option<Rc<Overlay>>,
    status_item: Option<StatusItem>,
    settings_window: Option<SettingsWindow>,
    event_tap: Option<EventTap>,
    observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    refresh: RefreshWorker,
    records: Vec<WindowRecord>,
    windowless: Vec<WindowlessApp>,
    order: Vec<u32>,
    icons: HashMap<i32, Retained<NSImage>>,
    // Process ids in the order their apps were last activated, the frontmost first.
    recent_apps: Vec<u32>,
    // Windows in the order they last had focus, which orders each app's windows.
    window_history: WindowHistory,
    // The most apps and the longest window list this session has listed. The panel is sized
    // for them, so it never shrinks under the pointer while it shows.
    extent: (usize, usize),
    // The first tile and row drawn; they move only as far as the selection needs.
    tile_start: usize,
    row_start: usize,
    // What the last frame drew, for resolving pointer events against it.
    shown: Option<Shown>,
    panel: Panel,
    preview: PreviewSource,
    preview_image: Option<Retained<NSImage>>,
    preview_message: Option<&'static str>,
    preview_window: Option<u32>,
    preview_in_flight: bool,
    preview_timer: Option<Retained<NSTimer>>,
    refresh_timer: Option<Retained<NSTimer>>,
    tap_retry_timer: Option<Retained<NSTimer>>,
    close_button: CloseButton,
    // The tile or row a click started on; the switch happens when it ends there too.
    pressed: Option<Hit>,
    // Where the pointer was when the panel appeared. Hovering selects nothing until the pointer
    // has moved away from here, so a nudge of the trackpad while ⌘ is down cannot change the
    // window the release switches to.
    pointer_origin: Option<NSPoint>,
    // The tile under the pointer, as an app index, and the timer that selects it after a rest.
    hovered_tile: Option<usize>,
    dwell_timer: Option<Retained<NSTimer>>,
    // Preview mode shows the overlay as soon as the first window list arrives.
    show_when_listed: bool,
    // The front app named in the last secure-keyboard-input report, so the log says it once per
    // app rather than on every activation.
    secure_input_holder: Option<String>,
}

/// Whether the panel is on screen. After ⌘ Tab the session is open while the panel waits for its
/// timer; nothing draws until it fires.
enum Panel {
    Hidden,
    Waiting(Retained<NSTimer>),
    Shown,
}

/// The geometry and scroll positions of the last frame drawn.
#[derive(Clone, Copy, Debug)]
struct Shown {
    layout: Layout,
    app: Option<ProcessIdentity>,
    tile_start: usize,
    tiles: usize,
    row_start: usize,
    rows: usize,
    selected_row: Option<usize>,
}

impl Shown {
    fn hit(&self, x: f64, y: f64) -> Option<Hit> {
        self.layout
            .hit(self.tiles, self.rows, self.selected_row, x, y)
    }
}

fn record_process(record: &WindowRecord) -> ProcessIdentity {
    ProcessIdentity::new(
        u32::try_from(record.pid).unwrap_or_default(),
        record.launched_at,
    )
}

fn frontmost_pid() -> Option<u32> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .and_then(|app| u32::try_from(app.processIdentifier()).ok())
}

impl App {
    fn new(
        mtm: MainThreadMarker,
        settings: Settings,
        store: SettingsStore,
        preview_mode: bool,
    ) -> Self {
        let hotkey_settings = hotkey_settings(&settings);
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
            refresh: RefreshWorker::spawn(),
            records: Vec::new(),
            windowless: Vec::new(),
            order: Vec::new(),
            icons: HashMap::new(),
            recent_apps: frontmost_pid().into_iter().collect(),
            window_history: WindowHistory::default(),
            extent: (0, 0),
            tile_start: 0,
            row_start: 0,
            shown: None,
            panel: Panel::Hidden,
            preview: PreviewSource::default(),
            preview_image: None,
            preview_message: None,
            preview_window: None,
            preview_in_flight: false,
            preview_timer: None,
            refresh_timer: None,
            tap_retry_timer: None,
            close_button: CloseButton::default(),
            pressed: None,
            pointer_origin: None,
            hovered_tile: None,
            dwell_timer: None,
            show_when_listed: false,
            secure_input_holder: None,
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
        if self.settings.appearance.preview {
            self.ask_for_screen_recording();
        }
        if !self.install_event_tap() {
            self.start_tap_retry_timer();
        }
    }

    /// Polls for the Accessibility grant so it is picked up while the app keeps running,
    /// which spares the user a relaunch.
    fn start_tap_retry_timer(&mut self) {
        if self.tap_retry_timer.is_some() {
            return;
        }
        let block = RcBlock::new(|_timer: NonNull<NSTimer>| {
            let _ = with_app(App::retry_event_tap);
        });
        self.tap_retry_timer = Some(unsafe {
            // SAFETY: scheduled from the main thread onto the main run loop.
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(TAP_RETRY_SECONDS, true, &block)
        });
    }

    /// Records the new front app for the strip's order, then puts the tap back at the head of
    /// the session taps.
    ///
    /// Remote desktop and VM clients insert a tap of their own to hand ⌘ Tab to the guest; the
    /// system asks the newest head-inserted tap first, so reinserting ours keeps the switcher
    /// working inside those apps. Secure keyboard input is the one thing no tap gets past, so
    /// it is named in the log when it is on.
    fn front_app_changed(&mut self) {
        self.note_front_app();
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

    fn install_event_tap(&mut self) -> bool {
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
        let activated = unsafe {
            // SAFETY: the notification name constant is a static string exported by AppKit.
            NSWorkspaceDidActivateApplicationNotification
        };
        let block = RcBlock::new(|_notification: NonNull<NSNotification>| {
            let _ = with_app(App::front_app_changed);
        });
        let token = unsafe {
            // SAFETY: as above, the block runs on the main thread.
            center.addObserverForName_object_queue_usingBlock(
                Some(activated),
                None,
                Some(&NSOperationQueue::mainQueue()),
                &block,
            )
        };
        self.observers.push(token);
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

    pub fn refresh_completed(&mut self, listing: Listing) {
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
        // The front app's topmost window is the one with focus; the window list refreshes on
        // every activation and every two seconds, so the history follows within that time.
        let front = frontmost_pid();
        let focused = records
            .iter()
            .find(|record| record.is_on_screen && Some(record_process(record).id) == front)
            .and_then(|record| isize::try_from(record.window_id).ok());
        let listed = self
            .order
            .iter()
            .filter_map(|id| isize::try_from(*id).ok())
            .collect::<Vec<_>>();
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
                self.widen_extent();
                self.selection_changed(before);
            } else {
                self.hide_overlay();
            }
        }
    }

    /// Records the front app as the most recently used one, for the strip's order.
    fn note_front_app(&mut self) {
        let Some(pid) = frontmost_pid() else {
            return;
        };
        self.recent_apps.retain(|known| *known != pid);
        self.recent_apps.insert(0, pid);
        self.recent_apps.truncate(RECENT_APPS_KEPT);
    }

    /// The listed windows grouped by app, the most recently used app first.
    fn app_entries(&self) -> Vec<AppEntry> {
        // Focus history first, then stacking order for windows it has not seen.
        let mut order = self.order.clone();
        order.sort_by_key(|id| {
            isize::try_from(*id).map_or(usize::MAX, |handle| self.window_history.rank(handle))
        });
        let windows = order
            .iter()
            .filter_map(|id| self.records.iter().find(|record| record.window_id == *id))
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
        group_by_app(&windows, &self.recent_apps, &windowless)
    }

    fn record(&self, window_handle: isize) -> Option<&WindowRecord> {
        let id = u32::try_from(window_handle).ok()?;
        self.records.iter().find(|record| record.window_id == id)
    }

    fn app_name(&self, process: ProcessIdentity) -> String {
        self.switcher
            .apps()
            .iter()
            .find(|app| app.process == process)
            .map(|app| app.name.clone())
            .unwrap_or_default()
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
                inside_overlay: self
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

    pub fn apply_action(&mut self, action: Action) {
        let before = self.switcher.selected_target();
        let effect = self.switcher.handle(action);
        if tracing() {
            eprintln!("action {action:?} -> {effect:?}");
        }
        self.apply_effect(effect, before);
    }

    fn apply_effect(&mut self, effect: Effect, before: Option<Target>) {
        match effect {
            Effect::None => {}
            Effect::Open { step } => self.show_overlay(Some(step)),
            Effect::Hide => self.hide_overlay(),
            Effect::Redraw => self.selection_changed(before),
            Effect::Activate(target) => self.activate_target(target),
            Effect::Execute { command, target } => self.execute_command(command, target),
        }
    }

    fn selection_changed(&mut self, before: Option<Target>) {
        self.redraw();
        if self.switcher.selected_target() != before {
            self.request_preview_capture();
        }
    }

    fn show_overlay(&mut self, step: Option<i32>) {
        self.switcher
            .open(self.app_entries(), step, frontmost_pid());
        if !self.switcher.is_active() {
            self.hide_overlay();
            return;
        }
        self.reset_pointer();
        self.extent = (0, 0);
        self.widen_extent();
        self.tile_start = 0;
        self.row_start = 0;
        self.shown = None;
        self.preview_image = None;
        self.preview_window = None;
        self.preview_message = None;
        self.hotkey.set_overlay_active(true);
        self.request_refresh();
        // Only the keyboard gesture waits; a list opened from the menu bar shows at once.
        if step.is_some() {
            self.panel = Panel::Waiting(schedule(REVEAL_SECONDS, App::reveal));
        } else {
            self.reveal();
        }
    }

    fn reveal(&mut self) {
        self.panel = Panel::Hidden;
        if !self.switcher.is_active() {
            return;
        }
        let theme = self.resolved_theme();
        if let Some(overlay) = self.overlay.clone() {
            overlay.set_theme(theme, SwitcherTokens::new(theme));
            overlay.show(self.layout(&overlay).size());
        }
        self.panel = Panel::Shown;
        self.pointer_origin = Some(NSEvent::mouseLocation());
        if self.settings.appearance.preview {
            self.preview.refresh_content();
        }
        self.start_preview_timer();
        self.redraw();
        self.request_preview_capture();
    }

    fn hide_overlay(&mut self) {
        self.switcher.hide();
        if let Panel::Waiting(timer) = std::mem::replace(&mut self.panel, Panel::Hidden) {
            timer.invalidate();
        }
        if let Some(overlay) = &self.overlay {
            overlay.hide();
        }
        self.hotkey.set_overlay_active(false);
        self.reset_pointer();
        self.shown = None;
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

    fn reset_pointer(&mut self) {
        self.close_button = CloseButton::default();
        self.pressed = None;
        self.pointer_origin = None;
        self.hovered_tile = None;
        self.cancel_dwell();
    }

    fn activate_target(&mut self, target: Target) {
        let result = match target {
            Target::Window { handle, .. } => self.record(handle).map_or_else(
                || Err("The selected window is no longer listed".to_owned()),
                commands::activate,
            ),
            Target::App(process) => commands::activate_app(&AppRef {
                process,
                name: &self.app_name(process),
            }),
        };
        if let Err(error) = result {
            eprintln!("{error}");
        }
        self.hide_overlay();
        Self::schedule_refresh_burst();
    }

    fn execute_command(&mut self, command: WindowCommand, target: Target) {
        let result = match target {
            Target::Window { handle, .. } => self.record(handle).map_or_else(
                || Err("The selected window is no longer listed".to_owned()),
                |record| commands::execute_on_window(command, record),
            ),
            Target::App(process) => commands::execute_on_app(
                command,
                &AppRef {
                    process,
                    name: &self.app_name(process),
                },
            ),
        };
        if let Err(error) = result {
            eprintln!("{error}");
            return;
        }
        // The switcher stays open and keeps its selection; the refreshes show the window or
        // app leaving the list in place.
        self.request_refresh();
        Self::schedule_refresh_burst();
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

    fn widen_extent(&mut self) {
        let apps = self.switcher.apps();
        let windows = apps.iter().map(|app| app.windows.len()).max().unwrap_or(0);
        self.extent = (self.extent.0.max(apps.len()), self.extent.1.max(windows));
    }

    fn layout(&self, overlay: &Overlay) -> Layout {
        Layout::new(
            self.extent.0,
            self.extent.1,
            self.settings.appearance.preview,
            overlay.max_size(),
        )
    }

    fn window_state(&self, window_handle: isize) -> WindowState {
        match self.record(window_handle) {
            Some(record) if record.is_minimized => WindowState::Minimized,
            Some(record) if record.is_hidden => WindowState::Hidden,
            Some(record) if !record.is_on_screen => WindowState::OtherDesktop,
            _ => WindowState::Normal,
        }
    }

    fn redraw(&mut self) {
        let Some(overlay) = self.overlay.clone() else {
            return;
        };
        if !self.switcher.is_active() || !matches!(self.panel, Panel::Shown) {
            return;
        }
        let layout = self.layout(&overlay);
        overlay.resize(layout.size());
        let selected_app = self.switcher.selected_app_index().unwrap_or_default();
        let selected_process = self.switcher.selected_app().map(|app| app.process);
        if self.shown.and_then(|shown| shown.app) != selected_process {
            self.row_start = 0;
        }

        let tiles = self.tiles(layout, selected_app);
        let (rows, selected_row, empty_note, more_note) = self.rows(layout);
        let preview = self
            .settings
            .appearance
            .preview
            .then(|| self.preview_model());

        self.shown = Some(Shown {
            layout,
            app: selected_process,
            tile_start: self.tile_start,
            tiles: tiles.len(),
            row_start: self.row_start,
            rows: rows.len(),
            selected_row,
        });
        overlay.present(FrameModel {
            layout,
            tokens: SwitcherTokens::new(self.resolved_theme()),
            tiles,
            rows,
            empty_note,
            more_note,
            close_state: self.close_button.visual_state(),
            preview,
        });
    }

    /// The strip's tiles, scrolled so the selected app shows.
    fn tiles(&mut self, layout: Layout, selected_app: usize) -> Vec<Tile> {
        let apps = self.switcher.apps();
        self.tile_start =
            scroll_into_view(self.tile_start, selected_app, apps.len(), layout.tile_slots);
        apps.iter()
            .enumerate()
            .skip(self.tile_start)
            .take(layout.tile_slots)
            .map(|(index, app)| Tile {
                name: app.name.clone(),
                icon: i32::try_from(app.process.id)
                    .ok()
                    .and_then(|pid| self.icons.get(&pid).cloned()),
                selected: index == selected_app,
            })
            .collect()
    }

    /// The selected app's rows scrolled so the selected window shows, the selected row among
    /// them, and the notes for an app without windows or a list longer than the panel.
    fn rows(
        &mut self,
        layout: Layout,
    ) -> (Vec<Row>, Option<usize>, Option<String>, Option<String>) {
        let windows = self
            .switcher
            .selected_app()
            .map_or(&[][..], |app| &app.windows[..]);
        let selected_window = self.switcher.selected_window_index();
        // A list longer than the panel gives its last slot to the count of the rest.
        let fits = if windows.len() > layout.row_slots {
            layout.row_slots.saturating_sub(1).max(1)
        } else {
            layout.row_slots
        };
        self.row_start = scroll_into_view(
            self.row_start,
            selected_window.unwrap_or_default(),
            windows.len(),
            fits,
        );
        let rows = windows
            .iter()
            .enumerate()
            .skip(self.row_start)
            .take(fits)
            .map(|(index, handle)| Row {
                number: (index < 9).then_some(index + 1),
                title: self
                    .record(*handle)
                    .map(|record| record.title.clone())
                    .unwrap_or_default(),
                state: self.window_state(*handle),
                selected: selected_window == Some(index),
            })
            .collect::<Vec<_>>();
        let hidden = windows.len() - rows.len();
        let more_note =
            (hidden > 0 && rows.len() < layout.row_slots).then(|| format!("{hidden} more"));
        let empty_note = windows.is_empty().then(|| "No open windows".to_owned());
        let selected_row = selected_window
            .and_then(|index| index.checked_sub(self.row_start))
            .filter(|row| *row < rows.len());
        (rows, selected_row, empty_note, more_note)
    }

    /// The selected window's latest capture, or why there is none.
    fn preview_model(&self) -> PreviewModel {
        let window = self
            .switcher
            .selected_window()
            .and_then(|handle| u32::try_from(handle).ok());
        let image = self
            .preview_image
            .clone()
            .filter(|_| window.is_some() && self.preview_window == window);
        let message = if window.is_none() || image.is_some() {
            None
        } else if !permissions::screen_recording_granted() {
            Some("Allow Screen Recording in System Settings to see previews".to_owned())
        } else {
            self.preview_message.map(str::to_owned)
        };
        PreviewModel { image, message }
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
        if !self.switcher.is_active() || !self.settings.appearance.preview || self.preview_in_flight
        {
            return;
        }
        let Some(overlay) = self.overlay.clone() else {
            return;
        };
        let Some(window_id) = self
            .switcher
            .selected_window()
            .and_then(|handle| u32::try_from(handle).ok())
        else {
            return;
        };
        let Some((width, height)) = self.shown.and_then(|shown| shown.layout.preview_size()) else {
            return;
        };
        if !permissions::screen_recording_granted() {
            return;
        }
        if !self.preview.has_content() {
            self.preview.refresh_content();
            return;
        }
        let scale = overlay.backing_scale();
        let request = CaptureRequest {
            window_id,
            pixel_width: pixel_length(width, scale),
            pixel_height: pixel_length(height, scale),
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
        if self.switcher.is_active() {
            self.request_preview_capture();
        }
    }

    pub fn preview_captured(&mut self, window_id: u32, result: MainThreadValue<PreviewResult>) {
        self.preview_in_flight = false;
        let selected = self
            .switcher
            .selected_window()
            .and_then(|handle| u32::try_from(handle).ok());
        if !self.switcher.is_active() || selected != Some(window_id) {
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
        if !self.switcher.is_active() {
            return;
        }
        let Some(shown) = self.shown else {
            return;
        };
        match event {
            ViewEvent::MouseMoved(x, y) => self.handle_mouse_moved(shown, shown.hit(x, y)),
            ViewEvent::MouseDown(x, y) => self.handle_mouse_down(shown, shown.hit(x, y)),
            ViewEvent::MouseUp(x, y) => self.handle_mouse_up(shown.hit(x, y)),
            ViewEvent::RightMouseDown(x, y) => {
                self.handle_right_mouse_down(shown, shown.hit(x, y), x, y);
            }
            ViewEvent::MouseExited => {
                self.hovered_tile = None;
                self.cancel_dwell();
                if self.close_button.hovered {
                    self.close_button.hovered = false;
                    self.redraw();
                }
            }
            ViewEvent::Scroll(step) => self.apply_action(Action::StepWindow(step)),
        }
    }

    /// Selects the app at `app` and its window `window`, or its last-used window.
    fn select(&mut self, app: usize, window: Option<usize>) {
        let before = self.switcher.selected_target();
        if self.switcher.select(app, window) {
            self.selection_changed(before);
        }
    }

    fn handle_mouse_moved(&mut self, shown: Shown, hit: Option<Hit>) {
        let hovered = matches!(hit, Some(Hit::CloseButton(_)));
        let close_changed = hovered != self.close_button.hovered;
        self.close_button.hovered = hovered;
        if let Some(origin) = self.pointer_origin {
            let now = NSEvent::mouseLocation();
            if (now.x - origin.x).hypot(now.y - origin.y) < HOVER_ARM_DISTANCE {
                if close_changed {
                    self.redraw();
                }
                return;
            }
            self.pointer_origin = None;
        }
        let tile = match hit {
            Some(Hit::Tile(slot)) => Some(shown.tile_start + slot),
            _ => None,
        };
        if tile != self.hovered_tile {
            self.hovered_tile = tile;
            self.cancel_dwell();
            if let Some(app) = tile
                && Some(app) != self.switcher.selected_app_index()
            {
                self.dwell_timer = Some(schedule(TILE_DWELL_SECONDS, move |state| {
                    state.finish_dwell(app);
                }));
            }
        }
        let selected_app = self.switcher.selected_app_index().unwrap_or_default();
        if let Some(Hit::Row(row)) = hit
            && !self.close_button.pressed
            && shown.selected_row != Some(row)
        {
            self.select(selected_app, Some(shown.row_start + row));
        } else if close_changed {
            self.redraw();
        }
    }

    fn finish_dwell(&mut self, app: usize) {
        self.dwell_timer = None;
        if self.switcher.is_active() && self.hovered_tile == Some(app) {
            self.select(app, None);
        }
    }

    fn cancel_dwell(&mut self) {
        if let Some(timer) = self.dwell_timer.take() {
            timer.invalidate();
        }
    }

    fn handle_mouse_down(&mut self, shown: Shown, hit: Option<Hit>) {
        match hit {
            Some(Hit::CloseButton(_)) => {
                self.close_button.pressed = true;
                self.redraw();
            }
            Some(Hit::Tile(slot)) => {
                self.pressed = hit;
                self.cancel_dwell();
                self.select(shown.tile_start + slot, None);
            }
            Some(Hit::Row(row)) => {
                self.pressed = hit;
                let app = self.switcher.selected_app_index().unwrap_or_default();
                self.select(app, Some(shown.row_start + row));
            }
            None => {}
        }
    }

    fn handle_mouse_up(&mut self, hit: Option<Hit>) {
        let pressed = self.pressed.take();
        if self.close_button.pressed {
            self.close_button.pressed = false;
            if matches!(hit, Some(Hit::CloseButton(_))) {
                self.apply_action(Action::Command(WindowCommand::Close));
            }
            self.redraw();
        } else if hit.is_some() && hit == pressed {
            self.apply_action(Action::Activate);
        }
    }

    fn handle_right_mouse_down(&mut self, shown: Shown, hit: Option<Hit>, x: f64, y: f64) {
        let on_window = match hit {
            Some(Hit::Row(row) | Hit::CloseButton(row)) => {
                let app = self.switcher.selected_app_index().unwrap_or_default();
                self.select(app, Some(shown.row_start + row));
                true
            }
            Some(Hit::Tile(slot)) => {
                self.cancel_dwell();
                self.select(shown.tile_start + slot, None);
                false
            }
            None => return,
        };
        let window = on_window && self.switcher.selected_window().is_some();
        let app_name = self
            .switcher
            .selected_app()
            .map(|app| app.name.clone())
            .unwrap_or_default();
        let Some(overlay) = self.overlay.clone() else {
            return;
        };
        if self.switcher.open_context_menu() {
            run_later(move || {
                let command = overlay.show_context_menu(x, y, window, &app_name);
                let _ = with_app(|app| app.finish_context_menu(command));
            });
        }
    }

    fn finish_context_menu(&mut self, command: Option<WindowCommand>) {
        let before = self.switcher.selected_target();
        let effect = self.switcher.finish_context_menu(command);
        self.apply_effect(effect, before);
    }

    fn show_settings(&mut self) {
        if self.settings_window.is_none() {
            let handler: Rc<dyn Fn(SettingsEvent)> = Rc::new(|event| match event {
                SettingsEvent::Changed(settings) => {
                    let _ = with_app(|app| app.apply_settings(settings));
                }
                SettingsEvent::Autostart(enabled) => {
                    let _ = with_app(|app| app.set_autostart(enabled));
                }
                SettingsEvent::OpenAccessibility => permissions::open_accessibility_settings(),
                SettingsEvent::OpenScreenRecording => {
                    permissions::open_screen_recording_settings();
                }
            });
            self.settings_window = Some(SettingsWindow::new(self.mtm, &self.settings, handler));
        }
        if let Some(window) = &self.settings_window {
            window.show(&self.settings);
        }
    }

    fn apply_settings(&mut self, settings: Settings) {
        let previews_turned_on = settings.appearance.preview && !self.settings.appearance.preview;
        self.settings = settings;
        self.hotkey_settings = hotkey_settings(&self.settings);
        self.save_settings();
        if previews_turned_on {
            self.ask_for_screen_recording();
        }
        if self.switcher.is_active() {
            if let Some(overlay) = &self.overlay {
                let theme = self.resolved_theme();
                overlay.set_theme(theme, SwitcherTokens::new(theme));
            }
            self.start_preview_timer();
            self.redraw();
            self.request_preview_capture();
        }
    }

    /// Previews are the only thing that needs Screen Recording, so the system prompt comes with
    /// them: on a start with previews on, and when they are turned on.
    fn ask_for_screen_recording(&mut self) {
        if permissions::screen_recording_granted() {
            self.preview.refresh_content();
        } else if !permissions::request_screen_recording() {
            eprintln!("Screen Recording is not granted; previews stay blank until it is allowed.");
        }
    }

    /// Registers or removes the login item. The system owns that state, so the settings file
    /// records whatever it reports afterwards rather than what was asked for.
    fn set_autostart(&mut self, enabled: bool) {
        if let Err(error) = autostart::set_enabled(enabled) {
            let mtm = self.mtm;
            run_later(move || show_fatal_error(mtm, &error));
        }
        self.settings.general.autostart = autostart::is_enabled();
        self.save_settings();
    }

    /// The defaults promise launch at login, but only a registration makes it true. Saving right
    /// away marks the first start as done, so a login item the user later removes in System
    /// Settings stays removed.
    fn complete_first_start(&mut self) {
        if self.settings.general.autostart
            && let Err(error) = autostart::set_enabled(true)
        {
            eprintln!("{error}");
        }
        self.settings.general.autostart = autostart::is_enabled();
        self.save_settings();
    }

    fn save_settings(&mut self) {
        if let Err(error) = self.store.save(&self.settings) {
            eprintln!("{error}");
        }
    }

    fn shutdown(&mut self) {
        self.event_tap = None;
        self.cancel_dwell();
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

fn hotkey_settings(settings: &Settings) -> HotkeySettings {
    HotkeySettings {
        command_tab: settings.general.replace_alt_tab,
        option_tab: settings.general.replace_win_tab,
    }
}
