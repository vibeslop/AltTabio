//! Settings window: one form of a few checkboxes and the theme, with a permission row on top
//! only while macOS still withholds something the chosen settings need.

use super::{autostart, permissions};
use alttabio::settings::{Settings, Theme};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSColor, NSControlStateValue,
    NSControlStateValueOff, NSControlStateValueOn, NSFont, NSGridCell, NSGridCellPlacement,
    NSGridRow, NSGridRowAlignment, NSGridView, NSLayoutAttribute, NSPopUpButton, NSStackView,
    NSTextField, NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSEdgeInsets, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer,
};
use std::cell::RefCell;
use std::rc::Rc;

const MARGIN: f64 = 24.0;
const ROW_SPACING: f64 = 8.0;
/// Extra space above the first row of each group.
const GROUP_SPACING: f64 = 12.0;
const COLUMN_SPACING: f64 = 12.0;
/// The width a permission explanation wraps at.
const NOTE_WIDTH: f64 = 280.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingKey {
    Autostart,
    CommandTab,
    OptionTab,
    CurrentDisplayOnly,
    Preview,
}

impl SettingKey {
    /// Every key once; the index doubles as the checkbox tag.
    const ALL: [Self; 5] = [
        Self::Autostart,
        Self::CommandTab,
        Self::OptionTab,
        Self::CurrentDisplayOnly,
        Self::Preview,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Autostart => "Launch at login",
            Self::CommandTab => "Replace ⌘ Tab",
            Self::OptionTab => "Also open with ⌥ Tab",
            Self::CurrentDisplayOnly => "Only windows on the current display",
            Self::Preview => "Show window previews",
        }
    }

    fn tag(self) -> isize {
        Self::ALL
            .iter()
            .position(|key| *key == self)
            .and_then(|index| isize::try_from(index).ok())
            .unwrap_or_default()
    }

    fn from_tag(tag: isize) -> Option<Self> {
        Self::ALL.get(usize::try_from(tag).ok()?).copied()
    }

    fn get(self, settings: &Settings) -> bool {
        match self {
            Self::Autostart => settings.general.autostart,
            Self::CommandTab => settings.general.replace_alt_tab,
            Self::OptionTab => settings.general.replace_win_tab,
            Self::CurrentDisplayOnly => settings.monitor.use_current_monitor_filter,
            Self::Preview => settings.appearance.preview,
        }
    }

    fn set(self, settings: &mut Settings, value: bool) {
        match self {
            Self::Autostart => settings.general.autostart = value,
            Self::CommandTab => settings.general.replace_alt_tab = value,
            Self::OptionTab => settings.general.replace_win_tab = value,
            Self::CurrentDisplayOnly => settings.monitor.use_current_monitor_filter = value,
            Self::Preview => settings.appearance.preview = value,
        }
    }
}

/// The form's groups: a label in the first column beside the first of its checkboxes.
const GROUPS: &[(&str, &[SettingKey])] = &[
    (
        "General",
        &[
            SettingKey::Autostart,
            SettingKey::CommandTab,
            SettingKey::OptionTab,
        ],
    ),
    (
        "Windows",
        &[SettingKey::CurrentDisplayOnly, SettingKey::Preview],
    ),
];

#[derive(Clone, Debug)]
pub enum SettingsEvent {
    Changed(Settings),
    /// The launch-at-login box was clicked; carries the state the user asked for.
    Autostart(bool),
    OpenAccessibility,
    OpenScreenRecording,
}

pub struct ControllerIvars {
    handler: Rc<dyn Fn(SettingsEvent)>,
    settings: RefCell<Settings>,
    checkboxes: RefCell<Vec<(SettingKey, Retained<NSButton>)>>,
    theme: RefCell<Option<Retained<NSPopUpButton>>>,
    accessibility: RefCell<Option<Retained<NSGridRow>>>,
    screen_recording: RefCell<Option<Retained<NSGridRow>>>,
    window: RefCell<Option<Retained<NSWindow>>>,
    content: RefCell<Option<Retained<NSView>>>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - SettingsController does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AltTabioSettingsController"]
    #[ivars = ControllerIvars]
    pub struct SettingsController;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for SettingsController {}

    impl SettingsController {
        // SAFETY: the action signatures match the target/action convention.
        #[unsafe(method(toggled:))]
        fn toggled(&self, sender: Option<&AnyObject>) {
            let Some(button) = sender.and_then(|sender| sender.downcast_ref::<NSButton>()) else {
                return;
            };
            let Some(key) = SettingKey::from_tag(button.tag()) else {
                return;
            };
            let value = button.state() == NSControlStateValueOn;
            if key == SettingKey::Autostart {
                // The login item lives in the system, not in the INI; the box shows whatever
                // the system reports once the request is done.
                (self.ivars().handler)(SettingsEvent::Autostart(value));
                self.refresh_status();
                return;
            }
            let settings = {
                let mut settings = self.ivars().settings.borrow_mut();
                key.set(&mut settings, value);
                settings.clone()
            };
            (self.ivars().handler)(SettingsEvent::Changed(settings));
            // Turning previews on may bring up the Screen Recording row.
            self.refresh_status();
        }

        #[unsafe(method(themeChanged:))]
        fn theme_changed(&self, sender: Option<&AnyObject>) {
            let Some(popup) = sender.and_then(|sender| sender.downcast_ref::<NSPopUpButton>())
            else {
                return;
            };
            let theme = match popup.indexOfSelectedItem() {
                1 => Theme::Light,
                2 => Theme::Dark,
                _ => Theme::Auto,
            };
            let settings = {
                let mut settings = self.ivars().settings.borrow_mut();
                settings.appearance.theme = theme;
                settings.clone()
            };
            (self.ivars().handler)(SettingsEvent::Changed(settings));
        }

        #[unsafe(method(openAccessibility:))]
        fn open_accessibility(&self, _sender: Option<&AnyObject>) {
            (self.ivars().handler)(SettingsEvent::OpenAccessibility);
        }

        #[unsafe(method(openScreenRecording:))]
        fn open_screen_recording(&self, _sender: Option<&AnyObject>) {
            (self.ivars().handler)(SettingsEvent::OpenScreenRecording);
        }

        #[unsafe(method(refreshStatus:))]
        fn refresh_status_timer(&self, _sender: Option<&AnyObject>) {
            self.refresh_status();
        }
    }
);

impl SettingsController {
    fn new(
        mtm: MainThreadMarker,
        settings: Settings,
        handler: Rc<dyn Fn(SettingsEvent)>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ControllerIvars {
            handler,
            settings: RefCell::new(settings),
            checkboxes: RefCell::new(Vec::new()),
            theme: RefCell::new(None),
            accessibility: RefCell::new(None),
            screen_recording: RefCell::new(None),
            window: RefCell::new(None),
            content: RefCell::new(None),
        });
        unsafe {
            // SAFETY: init is NSObject's designated initializer.
            msg_send![super(this), init]
        }
    }

    /// Pushes `settings` into every control.
    fn load(&self, settings: &Settings) {
        *self.ivars().settings.borrow_mut() = settings.clone();
        for (key, button) in self.ivars().checkboxes.borrow().iter() {
            button.setState(control_state(key.get(settings)));
        }
        if let Some(theme) = self.ivars().theme.borrow().as_ref() {
            theme.selectItemAtIndex(theme_index(settings.appearance.theme));
        }
        self.refresh_status();
    }

    /// Re-reads what the system says, since permissions and login items change outside the
    /// app, and shows a permission row only while it is missing and needed.
    fn refresh_status(&self) {
        let enabled = autostart::is_enabled();
        self.ivars().settings.borrow_mut().general.autostart = enabled;
        for (key, button) in self.ivars().checkboxes.borrow().iter() {
            if *key == SettingKey::Autostart {
                button.setState(control_state(enabled));
            }
        }
        let previews = self.ivars().settings.borrow().appearance.preview;
        let status = permissions::status();
        let mut changed = false;
        for (row, needed) in [
            (&self.ivars().accessibility, !status.accessibility),
            (
                &self.ivars().screen_recording,
                previews && !status.screen_recording,
            ),
        ] {
            if let Some(row) = row.borrow().as_ref()
                && row.isHidden() == needed
            {
                row.setHidden(!needed);
                changed = true;
            }
        }
        if changed {
            self.fit_window();
        }
    }

    /// Resizes the window to its content with the title bar kept in place.
    fn fit_window(&self) {
        let (Some(window), Some(content)) = (
            self.ivars().window.borrow().clone(),
            self.ivars().content.borrow().clone(),
        ) else {
            return;
        };
        content.layoutSubtreeIfNeeded();
        let size = content.fittingSize();
        let current = window.contentRectForFrameRect(window.frame());
        let top = current.origin.y + current.size.height;
        let resized = NSRect::new(
            NSPoint::new(current.origin.x, top - size.height),
            NSSize::new(size.width, size.height),
        );
        window.setFrame_display(window.frameRectForContentRect(resized), true);
    }
}

pub struct SettingsWindow {
    window: Retained<NSWindow>,
    controller: Retained<SettingsController>,
    _timer: Retained<NSTimer>,
    mtm: MainThreadMarker,
}

impl SettingsWindow {
    pub fn new(
        mtm: MainThreadMarker,
        settings: &Settings,
        handler: Rc<dyn Fn(SettingsEvent)>,
    ) -> Self {
        let controller = SettingsController::new(mtm, settings.clone(), handler);
        let grid = form(mtm, settings, &controller);
        let root = NSStackView::new(mtm);
        root.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        root.setAlignment(NSLayoutAttribute::Leading);
        root.setEdgeInsets(NSEdgeInsets {
            top: MARGIN - GROUP_SPACING,
            left: MARGIN,
            bottom: MARGIN,
            right: MARGIN,
        });
        root.addArrangedSubview(&grid);

        let window = unsafe {
            // SAFETY: releasedWhenClosed is disabled right after creation so the Retained owns it.
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::ZERO, NSSize::new(400.0, 300.0)),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            // SAFETY: the window is owned by SettingsWindow rather than a window controller.
            window.setReleasedWhenClosed(false);
        }
        window.setTitle(&NSString::from_str("AltTabio Settings"));
        window.setContentView(Some(&root));
        *controller.ivars().window.borrow_mut() = Some(window.clone());
        *controller.ivars().content.borrow_mut() = Some(Retained::into_super(root));
        controller.load(settings);
        controller.fit_window();
        window.center();

        let timer = unsafe {
            // SAFETY: the controller stays alive through SettingsWindow; the timer retains it too.
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                2.0,
                &controller,
                sel!(refreshStatus:),
                None,
                true,
            )
        };
        Self {
            window,
            controller,
            _timer: timer,
            mtm,
        }
    }

    pub fn show(&self, settings: &Settings) {
        self.controller.load(settings);
        self.window.makeKeyAndOrderFront(None);
        #[allow(
            deprecated,
            reason = "an accessory app has no other way to bring its settings window forward"
        )]
        NSApplication::sharedApplication(self.mtm).activateIgnoringOtherApps(true);
    }
}

/// The form: permission rows, then each group's label beside its checkboxes, then the theme
/// under Appearance, the word macOS uses for the same choice.
fn form(
    mtm: MainThreadMarker,
    settings: &Settings,
    controller: &SettingsController,
) -> Retained<NSGridView> {
    let grid = NSGridView::gridViewWithNumberOfColumns_rows(2, 0, mtm);
    grid.setRowSpacing(ROW_SPACING);
    grid.setColumnSpacing(COLUMN_SPACING);
    grid.setRowAlignment(NSGridRowAlignment::FirstBaseline);
    grid.columnAtIndex(0)
        .setXPlacement(NSGridCellPlacement::Trailing);

    let accessibility = permission_row(
        mtm,
        &grid,
        controller,
        "Accessibility",
        "AltTabio needs this to see ⌘ Tab and switch windows.",
        sel!(openAccessibility:),
    );
    let screen_recording = permission_row(
        mtm,
        &grid,
        controller,
        "Screen Recording",
        "Window previews need this. Quit and reopen AltTabio after allowing it.",
        sel!(openScreenRecording:),
    );
    *controller.ivars().accessibility.borrow_mut() = Some(accessibility);
    *controller.ivars().screen_recording.borrow_mut() = Some(screen_recording);

    for (group, keys) in GROUPS {
        for (index, key) in keys.iter().enumerate() {
            let label = if index == 0 {
                label_view(mtm, group)
            } else {
                NSGridCell::emptyContentView(mtm)
            };
            let row = add_row(&grid, &label, &checkbox(mtm, *key, settings, controller));
            if index == 0 {
                row.setTopPadding(GROUP_SPACING);
            }
        }
    }
    let theme = theme_popup(mtm, settings, controller);
    add_row(&grid, &label_view(mtm, "Appearance"), &theme).setTopPadding(GROUP_SPACING);
    grid
}

fn add_row(grid: &NSGridView, label: &NSView, control: &NSView) -> Retained<NSGridRow> {
    grid.addRowWithViews(&NSArray::from_slice(&[label, control]))
}

fn label_view(mtm: MainThreadMarker, text: &str) -> Retained<NSView> {
    Retained::into_super(Retained::into_super(NSTextField::labelWithString(
        &NSString::from_str(text),
        mtm,
    )))
}

fn checkbox(
    mtm: MainThreadMarker,
    key: SettingKey,
    settings: &Settings,
    controller: &SettingsController,
) -> Retained<NSView> {
    let button = unsafe {
        // SAFETY: the selector exists on SettingsController with a matching signature.
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str(key.label()),
            Some(controller),
            Some(sel!(toggled:)),
            mtm,
        )
    };
    button.setTag(key.tag());
    button.setState(control_state(key.get(settings)));
    controller
        .ivars()
        .checkboxes
        .borrow_mut()
        .push((key, button.clone()));
    Retained::into_super(Retained::into_super(button))
}

fn theme_popup(
    mtm: MainThreadMarker,
    settings: &Settings,
    controller: &SettingsController,
) -> Retained<NSView> {
    let popup = NSPopUpButton::new(mtm);
    for name in ["System", "Light", "Dark"] {
        popup.addItemWithTitle(&NSString::from_str(name));
    }
    popup.selectItemAtIndex(theme_index(settings.appearance.theme));
    unsafe {
        // SAFETY: the selector exists on SettingsController with a matching signature.
        popup.setTarget(Some(controller));
        popup.setAction(Some(sel!(themeChanged:)));
    }
    *controller.ivars().theme.borrow_mut() = Some(popup.clone());
    Retained::into_super(Retained::into_super(Retained::into_super(popup)))
}

/// A missing permission: its name, what it is for, and the way to System Settings. Hidden
/// until the status check finds it missing.
fn permission_row(
    mtm: MainThreadMarker,
    grid: &NSGridView,
    controller: &SettingsController,
    name: &str,
    note: &str,
    action: Sel,
) -> Retained<NSGridRow> {
    let text = NSTextField::wrappingLabelWithString(&NSString::from_str(note), mtm);
    text.setSelectable(false);
    text.setFont(Some(&NSFont::systemFontOfSize(
        NSFont::smallSystemFontSize(),
    )));
    text.setTextColor(Some(&NSColor::secondaryLabelColor()));
    text.setPreferredMaxLayoutWidth(NOTE_WIDTH);
    let button = unsafe {
        // SAFETY: the selector exists on SettingsController with a matching signature.
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str("Open System Settings…"),
            Some(controller),
            Some(action),
            mtm,
        )
    };
    let column = NSStackView::new(mtm);
    column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    column.setAlignment(NSLayoutAttribute::Leading);
    column.setSpacing(6.0);
    column.addArrangedSubview(&text);
    column.addArrangedSubview(&button);
    let row = add_row(grid, &label_view(mtm, name), &column);
    row.setTopPadding(GROUP_SPACING);
    row.setHidden(true);
    row
}

fn control_state(on: bool) -> NSControlStateValue {
    if on {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    }
}

fn theme_index(theme: Theme) -> isize {
    match theme {
        Theme::Auto => 0,
        Theme::Light => 1,
        Theme::Dark => 2,
    }
}
