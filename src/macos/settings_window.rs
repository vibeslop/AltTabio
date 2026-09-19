//! Settings window with the Windows build's options, macOS labels, and permission shortcuts.

use super::{autostart, permissions};
use alttabio::settings::{Settings, Theme};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSColor, NSControlStateValueOff,
    NSControlStateValueOn, NSFont, NSLayoutAttribute, NSPopUpButton, NSStackView, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    NSEdgeInsets, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer,
};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingKey {
    Autostart,
    CommandTab,
    OptionTab,
    TypedSearch,
    ReleaseSwitches,
    ReleaseRightButtonSwitches,
    RightButtonWheel,
    MouseOverSelection,
    CompactList,
    LargeIcons,
    ShowNumbers,
    ShowAppNames,
    VisibleBorders,
    Preview,
    FullDesktopPreview,
    CurrentDisplayOnly,
}

impl SettingKey {
    const GENERAL: [Self; 8] = [
        Self::Autostart,
        Self::CommandTab,
        Self::OptionTab,
        Self::TypedSearch,
        Self::ReleaseSwitches,
        Self::ReleaseRightButtonSwitches,
        Self::RightButtonWheel,
        Self::MouseOverSelection,
    ];
    const APPEARANCE: [Self; 7] = [
        Self::CompactList,
        Self::LargeIcons,
        Self::ShowNumbers,
        Self::ShowAppNames,
        Self::VisibleBorders,
        Self::Preview,
        Self::FullDesktopPreview,
    ];
    const DISPLAY: [Self; 1] = [Self::CurrentDisplayOnly];

    const fn label(self) -> &'static str {
        match self {
            Self::Autostart => "Launch at login",
            Self::CommandTab => "Replace ⌘ Tab",
            Self::OptionTab => "Also open with ⌥ Tab",
            Self::TypedSearch => "Type to search",
            Self::ReleaseSwitches => "Switch when the modifier is released",
            Self::ReleaseRightButtonSwitches => "Switch when the right mouse button is released",
            Self::RightButtonWheel => "Right mouse button + wheel switching",
            Self::MouseOverSelection => "Select on mouse over",
            Self::CompactList => "Compact list",
            Self::LargeIcons => "Large icons",
            Self::ShowNumbers => "Show numbers",
            Self::ShowAppNames => "Show app names",
            Self::VisibleBorders => "Visible borders",
            Self::Preview => "Live preview",
            Self::FullDesktopPreview => "Show the preview on the full desktop",
            Self::CurrentDisplayOnly => "Only list windows on the current display",
        }
    }

    fn tag(self) -> isize {
        Self::GENERAL
            .iter()
            .chain(Self::APPEARANCE.iter())
            .chain(Self::DISPLAY.iter())
            .position(|key| *key == self)
            .and_then(|index| isize::try_from(index).ok())
            .unwrap_or_default()
    }

    fn from_tag(tag: isize) -> Option<Self> {
        Self::GENERAL
            .iter()
            .chain(Self::APPEARANCE.iter())
            .chain(Self::DISPLAY.iter())
            .nth(usize::try_from(tag).ok()?)
            .copied()
    }

    fn get(self, settings: &Settings) -> bool {
        match self {
            Self::Autostart => settings.general.autostart,
            Self::CommandTab => settings.general.replace_alt_tab,
            Self::OptionTab => settings.general.replace_win_tab,
            Self::TypedSearch => settings.general.typed_search,
            Self::ReleaseSwitches => settings.general.release_alt_switches,
            Self::ReleaseRightButtonSwitches => settings.general.release_right_button_switches,
            Self::RightButtonWheel => settings.general.right_button_wheel_switching,
            Self::MouseOverSelection => settings.general.mouse_over_selection,
            Self::CompactList => settings.appearance.compact_list,
            Self::LargeIcons => settings.appearance.large_icons,
            Self::ShowNumbers => settings.appearance.show_numbers,
            Self::ShowAppNames => settings.appearance.show_app_names,
            Self::VisibleBorders => settings.appearance.visible_borders,
            Self::Preview => settings.appearance.preview,
            Self::FullDesktopPreview => settings.appearance.full_desktop_preview,
            Self::CurrentDisplayOnly => settings.monitor.use_current_monitor_filter,
        }
    }

    fn set(self, settings: &mut Settings, value: bool) {
        match self {
            Self::Autostart => settings.general.autostart = value,
            Self::CommandTab => settings.general.replace_alt_tab = value,
            Self::OptionTab => settings.general.replace_win_tab = value,
            Self::TypedSearch => settings.general.typed_search = value,
            Self::ReleaseSwitches => settings.general.release_alt_switches = value,
            Self::ReleaseRightButtonSwitches => {
                settings.general.release_right_button_switches = value;
            }
            Self::RightButtonWheel => settings.general.right_button_wheel_switching = value,
            Self::MouseOverSelection => settings.general.mouse_over_selection = value,
            Self::CompactList => settings.appearance.compact_list = value,
            Self::LargeIcons => settings.appearance.large_icons = value,
            Self::ShowNumbers => settings.appearance.show_numbers = value,
            Self::ShowAppNames => settings.appearance.show_app_names = value,
            Self::VisibleBorders => settings.appearance.visible_borders = value,
            Self::Preview => settings.appearance.preview = value,
            Self::FullDesktopPreview => settings.appearance.full_desktop_preview = value,
            Self::CurrentDisplayOnly => settings.monitor.use_current_monitor_filter = value,
        }
    }
}

#[derive(Clone, Debug)]
pub enum SettingsEvent {
    Changed(Settings),
    OpenAccessibility,
    OpenScreenRecording,
}

pub struct ControllerIvars {
    handler: Rc<dyn Fn(SettingsEvent)>,
    settings: RefCell<Settings>,
    accessibility_label: RefCell<Option<Retained<NSTextField>>>,
    screen_recording_label: RefCell<Option<Retained<NSTextField>>>,
    autostart_button: RefCell<Option<Retained<NSButton>>>,
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
            let settings = {
                let mut settings = self.ivars().settings.borrow_mut();
                key.set(&mut settings, value);
                settings.clone()
            };
            (self.ivars().handler)(SettingsEvent::Changed(settings));
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
        fn refresh_status(&self, _sender: Option<&AnyObject>) {
            self.refresh_labels();
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
            accessibility_label: RefCell::new(None),
            screen_recording_label: RefCell::new(None),
            autostart_button: RefCell::new(None),
        });
        unsafe {
            // SAFETY: init is NSObject's designated initializer.
            msg_send![super(this), init]
        }
    }

    fn refresh_labels(&self) {
        let status = permissions::status();
        if let Some(label) = self.ivars().accessibility_label.borrow().as_ref() {
            label.setStringValue(&NSString::from_str(&permission_text(
                "Accessibility",
                status.accessibility,
            )));
        }
        if let Some(label) = self.ivars().screen_recording_label.borrow().as_ref() {
            label.setStringValue(&NSString::from_str(&permission_text(
                "Screen Recording",
                status.screen_recording,
            )));
        }
        if let Some(button) = self.ivars().autostart_button.borrow().as_ref() {
            let enabled = autostart::is_enabled();
            button.setState(if enabled {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
    }
}

fn permission_text(name: &str, granted: bool) -> String {
    if granted {
        format!("{name}: granted")
    } else {
        format!("{name}: not granted")
    }
}

pub struct SettingsWindow {
    window: Retained<NSWindow>,
    controller: Retained<SettingsController>,
    checkboxes: Vec<(SettingKey, Retained<NSButton>)>,
    theme: Retained<NSPopUpButton>,
    _timer: Retained<NSTimer>,
    mtm: MainThreadMarker,
}

impl SettingsWindow {
    #[allow(
        clippy::too_many_lines,
        reason = "the form is one linear list of controls; splitting it would hide the order"
    )]
    pub fn new(
        mtm: MainThreadMarker,
        settings: &Settings,
        settings_path: &str,
        handler: Rc<dyn Fn(SettingsEvent)>,
    ) -> Self {
        let controller = SettingsController::new(mtm, settings.clone(), handler);
        let stack = NSStackView::new(mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::Leading);
        stack.setSpacing(6.0);
        stack.setEdgeInsets(NSEdgeInsets {
            top: 20.0,
            left: 20.0,
            bottom: 20.0,
            right: 20.0,
        });

        let mut checkboxes = Vec::new();
        stack.addArrangedSubview(&heading(mtm, "General"));
        for key in SettingKey::GENERAL {
            let button = checkbox(mtm, key, settings, &controller);
            if key == SettingKey::Autostart {
                *controller.ivars().autostart_button.borrow_mut() = Some(button.clone());
            }
            stack.addArrangedSubview(&button);
            checkboxes.push((key, button));
        }

        stack.addArrangedSubview(&spacer(mtm));
        stack.addArrangedSubview(&heading(mtm, "Appearance"));
        let theme_row = NSStackView::new(mtm);
        theme_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        theme_row.setSpacing(8.0);
        theme_row.addArrangedSubview(&NSTextField::labelWithString(
            &NSString::from_str("Theme"),
            mtm,
        ));
        let theme = NSPopUpButton::new(mtm);
        for name in ["Follow macOS", "Light", "Dark"] {
            theme.addItemWithTitle(&NSString::from_str(name));
        }
        theme.selectItemAtIndex(theme_index(settings.appearance.theme));
        unsafe {
            // SAFETY: the selector exists on SettingsController with a matching signature.
            theme.setTarget(Some(&controller));
            theme.setAction(Some(sel!(themeChanged:)));
        }
        theme_row.addArrangedSubview(&theme);
        stack.addArrangedSubview(&theme_row);
        for key in SettingKey::APPEARANCE {
            let button = checkbox(mtm, key, settings, &controller);
            stack.addArrangedSubview(&button);
            checkboxes.push((key, button));
        }

        stack.addArrangedSubview(&spacer(mtm));
        stack.addArrangedSubview(&heading(mtm, "Displays"));
        for key in SettingKey::DISPLAY {
            let button = checkbox(mtm, key, settings, &controller);
            stack.addArrangedSubview(&button);
            checkboxes.push((key, button));
        }

        stack.addArrangedSubview(&spacer(mtm));
        stack.addArrangedSubview(&heading(mtm, "Permissions"));
        let accessibility = permission_row(
            mtm,
            &controller,
            sel!(openAccessibility:),
            "Accessibility lets AltTabio see ⌘ Tab and control windows.",
        );
        *controller.ivars().accessibility_label.borrow_mut() = Some(accessibility.0);
        stack.addArrangedSubview(&accessibility.1);
        let screen_recording = permission_row(
            mtm,
            &controller,
            sel!(openScreenRecording:),
            "Screen Recording lets AltTabio show live previews.",
        );
        *controller.ivars().screen_recording_label.borrow_mut() = Some(screen_recording.0);
        stack.addArrangedSubview(&screen_recording.1);

        stack.addArrangedSubview(&spacer(mtm));
        let note = NSTextField::labelWithString(
            &NSString::from_str(&format!("Settings are stored in {settings_path}")),
            mtm,
        );
        note.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        note.setTextColor(Some(&NSColor::secondaryLabelColor()));
        note.setSelectable(true);
        stack.addArrangedSubview(&note);
        controller.refresh_labels();

        let window = unsafe {
            // SAFETY: releasedWhenClosed is disabled right after creation so the Retained owns it.
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(480.0, 640.0)),
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
        stack.layoutSubtreeIfNeeded();
        let size = stack.fittingSize();
        window.setContentSize(size);
        stack.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), size));
        window.setContentView(Some(&stack));
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
            checkboxes,
            theme,
            _timer: timer,
            mtm,
        }
    }

    pub fn show(&self, settings: &Settings) {
        *self.controller.ivars().settings.borrow_mut() = settings.clone();
        for (key, button) in &self.checkboxes {
            button.setState(if key.get(settings) {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
        self.theme
            .selectItemAtIndex(theme_index(settings.appearance.theme));
        self.controller.refresh_labels();
        self.window.makeKeyAndOrderFront(None);
        #[allow(
            deprecated,
            reason = "an accessory app has no other way to bring its settings window forward"
        )]
        NSApplication::sharedApplication(self.mtm).activateIgnoringOtherApps(true);
    }
}

fn theme_index(theme: Theme) -> isize {
    match theme {
        Theme::Auto => 0,
        Theme::Light => 1,
        Theme::Dark => 2,
    }
}

fn heading(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
    label
}

fn spacer(mtm: MainThreadMarker) -> Retained<NSView> {
    let view = NSView::new(mtm);
    view.setFrameSize(NSSize::new(1.0, 6.0));
    view
}

fn checkbox(
    mtm: MainThreadMarker,
    key: SettingKey,
    settings: &Settings,
    controller: &SettingsController,
) -> Retained<NSButton> {
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
    button.setState(if key.get(settings) {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    button
}

fn permission_row(
    mtm: MainThreadMarker,
    controller: &SettingsController,
    action: objc2::runtime::Sel,
    description: &str,
) -> (Retained<NSTextField>, Retained<NSStackView>) {
    let column = NSStackView::new(mtm);
    column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    column.setAlignment(NSLayoutAttribute::Leading);
    column.setSpacing(2.0);
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setSpacing(8.0);
    let label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
    row.addArrangedSubview(&label);
    let button = unsafe {
        // SAFETY: the selector exists on SettingsController with a matching signature.
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str("Open System Settings"),
            Some(controller),
            Some(action),
            mtm,
        )
    };
    row.addArrangedSubview(&button);
    column.addArrangedSubview(&row);
    let note = NSTextField::labelWithString(&NSString::from_str(description), mtm);
    note.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    note.setTextColor(Some(&NSColor::secondaryLabelColor()));
    column.addArrangedSubview(&note);
    (label, column)
}
