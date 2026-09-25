//! Settings window: a tabbed, System Settings-style form over the shared `Settings`.
//!
//! Every tab is an `NSStackView` built from the same section helpers so the spacing is uniform;
//! the window sizes itself once to the tallest tab and never resizes.

use super::{autostart, permissions};
use alttabio::settings::{Settings, Theme};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSBackingStoreType, NSBorderType, NSButton, NSColor,
    NSControlStateValue, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSFontWeightMedium,
    NSImage, NSImageSymbolConfiguration, NSImageView, NSLayoutAttribute,
    NSLayoutConstraintOrientation, NSLayoutPriorityRequired, NSLineBreakMode, NSPopUpButton,
    NSScrollView, NSStackView, NSStackViewDistribution, NSTabView, NSTabViewItem, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    NSEdgeInsets, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer,
};
use std::cell::RefCell;
use std::rc::Rc;

const WINDOW_WIDTH: f64 = 520.0;
/// Inset between the window edge and the tab view, and between a tab's edge and its content.
const MARGIN: f64 = 20.0;
const SECTION_SPACING: f64 = 24.0;
const CONTROL_SPACING: f64 = 10.0;
/// A tab taller than this scrolls instead of growing the window.
const MAX_TAB_HEIGHT: f64 = 560.0;
/// Where a checkbox title starts relative to the control's leading edge (14pt box plus gap), so
/// a description underneath lines up with the title rather than the box.
const CHECKBOX_TITLE_INDENT: f64 = 18.0;
const STATUS_DOT_SIZE: f64 = 10.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingKey {
    Autostart,
    CommandTab,
    OptionTab,
    Preview,
    CurrentDisplayOnly,
}

impl SettingKey {
    /// Every key once; the index doubles as the checkbox tag.
    const ALL: [Self; 5] = [
        Self::Autostart,
        Self::CommandTab,
        Self::OptionTab,
        Self::Preview,
        Self::CurrentDisplayOnly,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Autostart => "Launch at login",
            Self::CommandTab => "Replace ⌘ Tab",
            Self::OptionTab => "Also open with ⌥ Tab",
            Self::Preview => "Show window previews",
            Self::CurrentDisplayOnly => "Only windows on the current display",
        }
    }

    /// A secondary line under the checkbox, only where the title alone leaves the effect unclear.
    const fn description(self) -> Option<&'static str> {
        match self {
            Self::CommandTab => Some("AltTabio opens instead of the system app switcher"),
            _ => None,
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
            Self::Preview => settings.appearance.preview,
            Self::CurrentDisplayOnly => settings.monitor.use_current_monitor_filter,
        }
    }

    fn set(self, settings: &mut Settings, value: bool) {
        match self {
            Self::Autostart => settings.general.autostart = value,
            Self::CommandTab => settings.general.replace_alt_tab = value,
            Self::OptionTab => settings.general.replace_win_tab = value,
            Self::Preview => settings.appearance.preview = value,
            Self::CurrentDisplayOnly => settings.monitor.use_current_monitor_filter = value,
        }
    }
}

/// A titled group of checkboxes on a tab.
struct CheckboxSection {
    title: &'static str,
    keys: &'static [SettingKey],
}

const GENERAL_SECTIONS: &[CheckboxSection] = &[
    CheckboxSection {
        title: "Startup",
        keys: &[SettingKey::Autostart],
    },
    CheckboxSection {
        title: "Hotkeys",
        keys: &[SettingKey::CommandTab, SettingKey::OptionTab],
    },
];

/// The Appearance tab after its theme row.
const APPEARANCE_SECTIONS: &[CheckboxSection] = &[
    CheckboxSection {
        title: "Preview",
        keys: &[SettingKey::Preview],
    },
    CheckboxSection {
        title: "Displays",
        keys: &[SettingKey::CurrentDisplayOnly],
    },
];

#[derive(Clone, Debug)]
pub enum SettingsEvent {
    Changed(Settings),
    /// The launch-at-login box was clicked; carries the state the user asked for.
    Autostart(bool),
    OpenAccessibility,
    OpenScreenRecording,
}

/// The live parts of one Permissions row: the colored dot and the "name: state" label.
struct PermissionRow {
    name: &'static str,
    dot: Retained<NSImageView>,
    status: Retained<NSTextField>,
}

impl PermissionRow {
    fn update(&self, granted: bool) {
        let state = if granted { "granted" } else { "not granted" };
        self.status
            .setStringValue(&NSString::from_str(&format!("{}: {state}", self.name)));
        self.dot.setImage(status_dot(granted).as_deref());
    }
}

pub struct ControllerIvars {
    handler: Rc<dyn Fn(SettingsEvent)>,
    settings: RefCell<Settings>,
    checkboxes: RefCell<Vec<(SettingKey, Retained<NSButton>)>>,
    theme: RefCell<Option<Retained<NSPopUpButton>>>,
    accessibility: RefCell<Option<PermissionRow>>,
    screen_recording: RefCell<Option<PermissionRow>>,
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
        });
        unsafe {
            // SAFETY: init is NSObject's designated initializer.
            msg_send![super(this), init]
        }
    }

    /// Pushes `settings` into every control, including the dependent enabled states.
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

    /// Re-reads what the system says, since permissions and login items change outside the app.
    fn refresh_status(&self) {
        let status = permissions::status();
        if let Some(row) = self.ivars().accessibility.borrow().as_ref() {
            row.update(status.accessibility);
        }
        if let Some(row) = self.ivars().screen_recording.borrow().as_ref() {
            row.update(status.screen_recording);
        }
        let enabled = autostart::is_enabled();
        self.ivars().settings.borrow_mut().general.autostart = enabled;
        for (key, button) in self.ivars().checkboxes.borrow().iter() {
            if *key == SettingKey::Autostart {
                button.setState(control_state(enabled));
            }
        }
    }
}

define_class!(
    // SAFETY:
    // - NSView has no subclassing requirements beyond calling the designated initializer.
    // - FlippedView does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "AltTabioFlippedView"]
    pub struct FlippedView;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for FlippedView {}

    impl FlippedView {
        // SAFETY: the signature matches the NSView declaration.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedView {
    /// A scroll view's document view that starts at the top instead of the bottom.
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe {
            // SAFETY: initWithFrame: is NSView's designated initializer.
            msg_send![super(this), initWithFrame: frame]
        }
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
        settings_path: &str,
        handler: Rc<dyn Fn(SettingsEvent)>,
    ) -> Self {
        let controller = SettingsController::new(mtm, settings.clone(), handler);
        let tab_width = WINDOW_WIDTH - 2.0 * MARGIN;

        let tab_view = NSTabView::new(mtm);
        // The tab strip and bezel take a fixed amount around the content; measure it once with
        // a probe frame so the tallest tab's height can be turned into a frame height, and so
        // the tabs know how wide their wrapped descriptions may be before they are measured.
        let probe = NSSize::new(tab_width, 100.0);
        tab_view.setFrameSize(probe);
        let content = tab_view.contentRect();
        let chrome_height = probe.height - content.size.height;
        let description_width = content.size.width - 2.0 * MARGIN - CHECKBOX_TITLE_INDENT;
        let tabs: [(&str, Retained<NSView>); 3] = [
            (
                "General",
                general_tab(mtm, settings, &controller, description_width),
            ),
            (
                "Appearance",
                appearance_tab(mtm, settings, &controller, description_width),
            ),
            ("Permissions", permissions_tab(mtm, &controller)),
        ];
        let mut content_height: f64 = 0.0;
        for (label, view) in tabs {
            view.layoutSubtreeIfNeeded();
            let height = view.fittingSize().height;
            content_height = content_height.max(height.min(MAX_TAB_HEIGHT));
            // The item's view is final before it joins the tab view; the selected item does
            // not pick up a view swapped in afterwards.
            let wrapped = if height > MAX_TAB_HEIGHT {
                scrollable(mtm, &view, NSSize::new(content.size.width, height))
            } else {
                top_pinned(mtm, &view)
            };
            let item = NSTabViewItem::new();
            item.setLabel(&NSString::from_str(label));
            item.setView(Some(&wrapped));
            tab_view.addTabViewItem(&item);
        }
        tab_view.setTranslatesAutoresizingMaskIntoConstraints(false);
        tab_view
            .widthAnchor()
            .constraintEqualToConstant(tab_width)
            .setActive(true);
        tab_view
            .heightAnchor()
            .constraintEqualToConstant(content_height + chrome_height)
            .setActive(true);

        let root = vertical_stack(mtm, 10.0);
        root.setEdgeInsets(NSEdgeInsets {
            top: MARGIN,
            left: MARGIN,
            bottom: 14.0,
            right: MARGIN,
        });
        root.addArrangedSubview(&tab_view);
        let note = secondary_label(mtm, &format!("Settings are stored in {settings_path}"));
        note.setSelectable(true);
        note.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        note.widthAnchor()
            .constraintEqualToConstant(tab_width)
            .setActive(true);
        root.addArrangedSubview(&note);
        controller.load(settings);

        let window = unsafe {
            // SAFETY: releasedWhenClosed is disabled right after creation so the Retained owns it.
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::ZERO, NSSize::new(WINDOW_WIDTH, 400.0)),
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
        root.layoutSubtreeIfNeeded();
        let size = root.fittingSize();
        window.setContentSize(size);
        root.setFrame(NSRect::new(NSPoint::ZERO, size));
        window.setContentView(Some(&root));
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

fn general_tab(
    mtm: MainThreadMarker,
    settings: &Settings,
    controller: &SettingsController,
    description_width: f64,
) -> Retained<NSView> {
    let tab = tab_stack(mtm);
    for section in GENERAL_SECTIONS {
        tab.addArrangedSubview(&checkbox_section(
            mtm,
            section,
            settings,
            controller,
            description_width,
        ));
    }
    Retained::into_super(tab)
}

fn appearance_tab(
    mtm: MainThreadMarker,
    settings: &Settings,
    controller: &SettingsController,
    description_width: f64,
) -> Retained<NSView> {
    let tab = tab_stack(mtm);
    let theme = section(mtm, "Theme");
    theme.addArrangedSubview(&theme_row(mtm, settings, controller));
    tab.addArrangedSubview(&theme);
    for section in APPEARANCE_SECTIONS {
        tab.addArrangedSubview(&checkbox_section(
            mtm,
            section,
            settings,
            controller,
            description_width,
        ));
    }
    Retained::into_super(tab)
}

fn permissions_tab(mtm: MainThreadMarker, controller: &SettingsController) -> Retained<NSView> {
    let tab = tab_stack(mtm);
    let accessibility = permission_block(
        mtm,
        &tab,
        controller,
        "Accessibility",
        "Lets AltTabio see ⌘ Tab and control windows.",
        None,
        sel!(openAccessibility:),
    );
    *controller.ivars().accessibility.borrow_mut() = Some(accessibility);
    let screen_recording = permission_block(
        mtm,
        &tab,
        controller,
        "Screen Recording",
        "Lets AltTabio show live window previews.",
        Some("After granting, quit and reopen AltTabio."),
        sel!(openScreenRecording:),
    );
    *controller.ivars().screen_recording.borrow_mut() = Some(screen_recording);
    Retained::into_super(tab)
}

/// The content stack of one tab: sections stacked top-down with the window margin all around.
fn tab_stack(mtm: MainThreadMarker) -> Retained<NSStackView> {
    let stack = vertical_stack(mtm, SECTION_SPACING);
    stack.setEdgeInsets(NSEdgeInsets {
        top: MARGIN,
        left: MARGIN,
        bottom: MARGIN,
        right: MARGIN,
    });
    stack
}

fn vertical_stack(mtm: MainThreadMarker, spacing: f64) -> Retained<NSStackView> {
    let stack = NSStackView::new(mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(NSLayoutAttribute::Leading);
    stack.setSpacing(spacing);
    stack
}

/// A bold title with its controls underneath; callers append the controls.
fn section(mtm: MainThreadMarker, title: &str) -> Retained<NSStackView> {
    let stack = vertical_stack(mtm, CONTROL_SPACING);
    let label = NSTextField::labelWithString(&NSString::from_str(title), mtm);
    label.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
    stack.addArrangedSubview(&label);
    stack
}

fn checkbox_section(
    mtm: MainThreadMarker,
    section_spec: &CheckboxSection,
    settings: &Settings,
    controller: &SettingsController,
    description_width: f64,
) -> Retained<NSStackView> {
    let stack = section(mtm, section_spec.title);
    for key in section_spec.keys {
        stack.addArrangedSubview(&checkbox_group(
            mtm,
            *key,
            settings,
            controller,
            description_width,
        ));
    }
    stack
}

/// A checkbox, with its description underneath when the key has one. The description wraps
/// at `description_width`: a single-line label longer than the tab pushes its whole section
/// out of the stack's insets and gets clipped at the edge.
fn checkbox_group(
    mtm: MainThreadMarker,
    key: SettingKey,
    settings: &Settings,
    controller: &SettingsController,
    description_width: f64,
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
    let Some(description) = key.description() else {
        return Retained::into_super(Retained::into_super(button));
    };
    let group = vertical_stack(mtm, 3.0);
    group.addArrangedSubview(&button);
    group.addArrangedSubview(&indented(
        mtm,
        &wrapping_secondary_label(mtm, description, description_width),
        CHECKBOX_TITLE_INDENT,
    ));
    Retained::into_super(group)
}

fn theme_row(
    mtm: MainThreadMarker,
    settings: &Settings,
    controller: &SettingsController,
) -> Retained<NSStackView> {
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setAlignment(NSLayoutAttribute::CenterY);
    row.setSpacing(CONTROL_SPACING);
    row.addArrangedSubview(&NSTextField::labelWithString(
        &NSString::from_str("Theme"),
        mtm,
    ));
    let popup = NSPopUpButton::new(mtm);
    for name in ["Follow macOS", "Light", "Dark"] {
        popup.addItemWithTitle(&NSString::from_str(name));
    }
    popup.selectItemAtIndex(theme_index(settings.appearance.theme));
    unsafe {
        // SAFETY: the selector exists on SettingsController with a matching signature.
        popup.setTarget(Some(controller));
        popup.setAction(Some(sel!(themeChanged:)));
    }
    row.addArrangedSubview(&popup);
    *controller.ivars().theme.borrow_mut() = Some(popup);
    row
}

/// One permission: a status dot, "name: state", the System Settings button at the trailing
/// edge, then the description (and note) indented under the label.
fn permission_block(
    mtm: MainThreadMarker,
    tab: &NSStackView,
    controller: &SettingsController,
    name: &'static str,
    description: &str,
    note: Option<&str>,
    action: Sel,
) -> PermissionRow {
    let header = NSStackView::new(mtm);
    header.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    header.setAlignment(NSLayoutAttribute::CenterY);
    header.setSpacing(CONTROL_SPACING);
    header.setDistribution(NSStackViewDistribution::Fill);
    let dot = NSImageView::new(mtm);
    dot.widthAnchor()
        .constraintEqualToConstant(STATUS_DOT_SIZE)
        .setActive(true);
    dot.setContentHuggingPriority_forOrientation(
        NSLayoutPriorityRequired,
        NSLayoutConstraintOrientation::Horizontal,
    );
    let status = NSTextField::labelWithString(&NSString::from_str(name), mtm);
    // The label gives way so the button lands at the trailing edge.
    status.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    let button = unsafe {
        // SAFETY: the selector exists on SettingsController with a matching signature.
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str("Open System Settings…"),
            Some(controller),
            Some(action),
            mtm,
        )
    };
    button.setContentHuggingPriority_forOrientation(
        NSLayoutPriorityRequired,
        NSLayoutConstraintOrientation::Horizontal,
    );
    header.addArrangedSubview(&dot);
    header.addArrangedSubview(&status);
    header.addArrangedSubview(&button);

    let column = vertical_stack(mtm, 4.0);
    column.addArrangedSubview(&header);
    let text_indent = STATUS_DOT_SIZE + CONTROL_SPACING;
    column.addArrangedSubview(&indented(
        mtm,
        &secondary_label(mtm, description),
        text_indent,
    ));
    if let Some(note) = note {
        column.addArrangedSubview(&indented(mtm, &secondary_label(mtm, note), text_indent));
    }
    tab.addArrangedSubview(&column);
    // Both views are in the tab's subtree now, so cross-view constraints can be installed.
    header
        .widthAnchor()
        .constraintEqualToAnchor(&column.widthAnchor())
        .setActive(true);
    column
        .widthAnchor()
        .constraintEqualToAnchor_constant(&tab.widthAnchor(), -2.0 * MARGIN)
        .setActive(true);
    PermissionRow { name, dot, status }
}

/// Wraps `content` in a vertical scroll view whose document starts at the top.
/// Wraps a tab's content so it keeps its own height at the top of the tab. The tab view sizes
/// every item's view to the tallest tab, and a stack that fills that frame hands the spare
/// height to whichever nested stacks hug least, which stretched the description rows.
fn top_pinned(mtm: MainThreadMarker, content: &NSView) -> Retained<NSView> {
    let container = NSView::new(mtm);
    content.setTranslatesAutoresizingMaskIntoConstraints(false);
    container.addSubview(content);
    content
        .topAnchor()
        .constraintEqualToAnchor(&container.topAnchor())
        .setActive(true);
    content
        .leadingAnchor()
        .constraintEqualToAnchor(&container.leadingAnchor())
        .setActive(true);
    content
        .trailingAnchor()
        .constraintEqualToAnchor(&container.trailingAnchor())
        .setActive(true);
    container
}

fn scrollable(mtm: MainThreadMarker, content: &NSView, size: NSSize) -> Retained<NSView> {
    let frame = NSRect::new(NSPoint::ZERO, size);
    content.setFrame(frame);
    content.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    let document = FlippedView::new(mtm, frame);
    document.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    document.addSubview(content);
    let scroll = NSScrollView::new(mtm);
    scroll.setDrawsBackground(false);
    scroll.setBorderType(NSBorderType::NoBorder);
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    scroll.setDocumentView(Some(&document));
    Retained::into_super(scroll)
}

/// Shifts `view` right by `indent` so it lines up with text rather than a control's edge.
fn indented(mtm: MainThreadMarker, view: &NSView, indent: f64) -> Retained<NSStackView> {
    let stack = NSStackView::new(mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    stack.setEdgeInsets(NSEdgeInsets {
        top: 0.0,
        left: indent,
        bottom: 0.0,
        right: 0.0,
    });
    stack.addArrangedSubview(view);
    stack
}

fn secondary_label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    label
}

/// A secondary label that wraps at `width`. The preferred width also fixes the label's
/// intrinsic height, so the tab measures tall enough for the wrapped lines before it is shown.
fn wrapping_secondary_label(
    mtm: MainThreadMarker,
    text: &str,
    width: f64,
) -> Retained<NSTextField> {
    let label = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    label.setSelectable(false);
    label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    label.setPreferredMaxLayoutWidth(width);
    label
}

/// A filled circle in the system green or red, or None when SF Symbols are unavailable.
fn status_dot(granted: bool) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str("circle.fill"),
        Some(&NSString::from_str(if granted {
            "granted"
        } else {
            "not granted"
        })),
    )?;
    let color = if granted {
        NSColor::systemGreenColor()
    } else {
        NSColor::systemRedColor()
    };
    let configuration = unsafe {
        // SAFETY: the font weight constant is a static value exported by AppKit.
        NSImageSymbolConfiguration::configurationWithPointSize_weight(
            STATUS_DOT_SIZE,
            NSFontWeightMedium,
        )
    };
    let configuration = configuration.configurationByApplyingConfiguration(
        &NSImageSymbolConfiguration::configurationWithHierarchicalColor(&color),
    );
    image.imageWithSymbolConfiguration(&configuration)
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
