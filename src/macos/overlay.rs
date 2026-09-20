//! The switcher panel: a non-activating floating `NSPanel` over Liquid Glass with one custom view
//! that draws the numbered task list, the selected row's close button, and the live preview.

use super::hotkey::HeldModifier;
use super::shortcuts::{ACTIONS, Footer};
use alttabio::input::WindowCommand;
use alttabio::overlay_layout::OverlayLayout;
use alttabio::switcher::filter_match;
use alttabio::theme::{ResolvedTheme, Rgb8, Rgba, SwitcherTokens};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSAttributedStringNSStringDrawing, NSBackingStoreType, NSBezierPath, NSColor,
    NSCompositingOperation, NSEvent, NSEventModifierFlags, NSFont, NSFontAttributeName,
    NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold, NSForegroundColorAttributeName,
    NSGlassEffectView, NSGlassEffectViewStyle, NSImage, NSImageSymbolConfiguration,
    NSLineBreakMode, NSMenu, NSMenuItem, NSMutableParagraphStyle, NSPanel,
    NSParagraphStyleAttributeName, NSPopUpMenuWindowLevel, NSScreen, NSStringDrawing,
    NSTextAlignment, NSTrackingArea, NSTrackingAreaOptions, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindowAnimationBehavior,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{
    NSAttributedStringKey, NSDictionary, NSMutableAttributedString, NSObjectProtocol, NSPoint,
    NSRange, NSRect, NSSize, NSString,
};
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

// The overlay takes five eighths of the display like the Windows build.
const OVERLAY_FRACTION: f64 = 5.0 / 8.0;
const CORNER_RADIUS: f64 = 16.0;
// The preview and search wells sit further than 24pt from the panel edge, so their radii are
// chosen on their own rather than derived from the panel corner.
const PREVIEW_RADIUS: f64 = 8.0;
const SEARCH_RADIUS: f64 = 8.0;
const KEYCAP_RADIUS: f64 = 6.0;
const KEYCAP_MINIMUM_WIDTH: f64 = 24.0;
/// Row numbers are plain text at rest; a flat pill this wide appears under the digit only while
/// the modifier makes it a key, so the numbers never compete with the titles.
const NUMBER_PILL_WIDTH: f64 = 22.0;
const BADGE_POINT_SIZE: f64 = 11.0;
const HINT_GAP: f64 = 16.0;
/// Horizontal inset of content inside wells, panel rows, and the footer.
const INSET: f64 = 12.0;
/// Width of the footer's trailing "Actions ⌘K" hit area.
const ACTIONS_BUTTON_WIDTH: f64 = 96.0;
const ACTION_PANEL_WIDTH: f64 = 320.0;
const ACTION_PANEL_PADDING: f64 = 8.0;
const ACTION_PANEL_HEADER_HEIGHT: f64 = 32.0;
const ACTION_ROW_HEIGHT: f64 = 32.0;
// Radius 16 minus the 8pt padding gives the selected action row its 8pt radius.
const ACTION_PANEL_RADIUS: f64 = 16.0;
/// Height reserved above the list while search text shows, including the gap to the rows.
pub const SEARCH_ROW_HEIGHT: f32 = 48.0;
/// Height reserved under the list and preview for the hint bar, including its gap.
pub const FOOTER_HEIGHT: f32 = 40.0;
const SEARCH_ROW_GAP: f64 = 12.0;
const FOOTER_GAP: f64 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewEvent {
    MouseMoved(f64, f64),
    MouseDown(f64, f64),
    MouseUp(f64, f64),
    RightMouseDown(f64, f64),
    MouseExited,
    Scroll(i32),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CloseButtonVisualState {
    #[default]
    Normal,
    Hovered,
    Pressed,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "fields mirror the independent appearance switches"
)]
pub struct RenderOptions {
    pub compact_list: bool,
    pub large_icons: bool,
    pub show_numbers: bool,
    pub show_app_names: bool,
    pub visible_borders: bool,
    pub preview: bool,
}

/// Where a window is when it is not plainly on the current Space; drawn as a row badge.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WindowState {
    #[default]
    Normal,
    Minimized,
    Hidden,
    OtherSpace,
}

pub struct RowModel {
    pub position: usize,
    pub title: String,
    pub app_name: String,
    pub icon: Option<Retained<NSImage>>,
    pub selected: bool,
    pub state: WindowState,
}

pub struct FrameModel {
    pub rows: Vec<RowModel>,
    pub layout: OverlayLayout,
    pub options: RenderOptions,
    pub tokens: SwitcherTokens,
    pub close_state: CloseButtonVisualState,
    pub preview: Option<Retained<NSImage>>,
    pub preview_message: Option<String>,
    pub filter: String,
    /// The switch modifier that is down, which turns the row numbers into keycaps.
    pub held_modifier: Option<HeldModifier>,
    /// The row that a number key just picked, lit for a moment before the switch.
    pub flash_position: Option<usize>,
    pub hidden_above: usize,
    pub hidden_below: usize,
    /// The action bar; None when hints are turned off.
    pub footer: Option<Footer>,
    /// The ⌘K panel with its selected entry, when open.
    pub action_panel: Option<ActionPanelModel>,
}

pub struct ActionPanelModel {
    pub selected: usize,
    /// Title of the window the actions apply to, shown as the panel header.
    pub target: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Hit {
    Row(usize),
    CloseButton(usize),
    /// The "Actions ⌘K" button in the footer.
    ActionsButton,
    /// An entry of the open action panel.
    ActionRow(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn contains(self, x: f64, y: f64) -> bool {
        x >= self.left && x < self.left + self.width && y >= self.top && y < self.top + self.height
    }

    fn ns(self) -> NSRect {
        NSRect::new(
            NSPoint::new(self.left, self.top),
            NSSize::new(self.width, self.height),
        )
    }
}

#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    reason = "overlay sizes are small point values that f32 represents exactly enough"
)]
pub fn list_width(size: (f64, f64), layout: OverlayLayout) -> f64 {
    f64::from(layout.list_width(size.0 as f32, 1.0))
}

#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    reason = "overlay sizes are small point values that f32 represents exactly enough"
)]
pub fn visible_rows(size: (f64, f64), layout: OverlayLayout) -> usize {
    layout.visible_row_count(size.1 as f32)
}

#[must_use]
#[allow(
    clippy::cast_precision_loss,
    reason = "row indices are small on-screen counts"
)]
pub fn row_rect(size: (f64, f64), layout: OverlayLayout, row: usize) -> Rect {
    let top =
        f64::from(layout.list_top()) + row as f64 * f64::from(layout.row_height + layout.row_gap);
    Rect {
        left: f64::from(layout.outer_padding),
        top,
        width: list_width(size, layout) - f64::from(layout.outer_padding),
        height: f64::from(layout.row_height),
    }
}

#[must_use]
pub fn close_button_rect(row: Rect, layout: OverlayLayout) -> Rect {
    let size = f64::from(layout.close_button_size);
    Rect {
        left: row.left + row.width - f64::from(layout.close_button_inset) - size,
        top: row.top + (row.height - size) / 2.0,
        width: size,
        height: size,
    }
}

#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    reason = "overlay sizes are small point values that f32 represents exactly enough"
)]
pub fn list_bottom(size: (f64, f64), layout: OverlayLayout) -> f64 {
    f64::from(layout.list_bottom(size.1 as f32))
}

#[must_use]
pub fn preview_rect(size: (f64, f64), layout: OverlayLayout) -> Rect {
    let padding = f64::from(layout.outer_padding);
    let left = list_width(size, layout) + padding * 2.0;
    Rect {
        left,
        top: padding,
        width: (size.0 - padding - left).max(0.0),
        height: (list_bottom(size, layout) - padding).max(0.0),
    }
}

/// The search well above the rows; empty when the layout reserves no search row.
#[must_use]
pub fn search_row_rect(size: (f64, f64), layout: OverlayLayout) -> Rect {
    let padding = f64::from(layout.outer_padding);
    let reserved = f64::from(layout.search_row_height);
    Rect {
        left: padding,
        top: padding,
        width: (list_width(size, layout) - padding).max(0.0),
        height: (reserved - SEARCH_ROW_GAP).max(0.0),
    }
}

/// The footer's "Actions ⌘K" button, at the footer's right end.
#[must_use]
pub fn actions_button_rect(size: (f64, f64), layout: OverlayLayout) -> Rect {
    let footer = footer_rect(size, layout);
    Rect {
        left: footer.left + footer.width - ACTIONS_BUTTON_WIDTH,
        top: footer.top,
        width: ACTIONS_BUTTON_WIDTH.min(footer.width),
        height: footer.height,
    }
}

/// The ⌘K panel, anchored above the footer's right end like a launcher's action panel.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    reason = "the action count is a small constant"
)]
pub fn action_panel_rect(size: (f64, f64), layout: OverlayLayout) -> Rect {
    let padding = f64::from(layout.outer_padding);
    let footer = footer_rect(size, layout);
    let bottom = if footer.height > 0.0 {
        footer.top - FOOTER_GAP
    } else {
        size.1 - padding
    };
    let height = ACTION_PANEL_HEADER_HEIGHT
        + ACTIONS.len() as f64 * ACTION_ROW_HEIGHT
        + ACTION_PANEL_PADDING * 2.0;
    Rect {
        left: size.0 - padding - ACTION_PANEL_WIDTH,
        top: bottom - height,
        width: ACTION_PANEL_WIDTH,
        height,
    }
}

#[must_use]
#[allow(
    clippy::cast_precision_loss,
    reason = "the action index is a small constant"
)]
pub fn action_row_rect(panel: Rect, index: usize) -> Rect {
    Rect {
        left: panel.left + ACTION_PANEL_PADDING,
        top: panel.top
            + ACTION_PANEL_PADDING
            + ACTION_PANEL_HEADER_HEIGHT
            + index as f64 * ACTION_ROW_HEIGHT,
        width: panel.width - ACTION_PANEL_PADDING * 2.0,
        height: ACTION_ROW_HEIGHT,
    }
}

/// The hint bar under the rows and preview; empty when the layout reserves no footer.
#[must_use]
pub fn footer_rect(size: (f64, f64), layout: OverlayLayout) -> Rect {
    let padding = f64::from(layout.outer_padding);
    let reserved = f64::from(layout.footer_height);
    Rect {
        left: padding,
        top: size.1 - padding - reserved + FOOTER_GAP,
        width: (size.0 - padding * 2.0).max(0.0),
        height: (reserved - FOOTER_GAP).max(0.0),
    }
}

#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    reason = "overlay sizes are small point values that f32 represents exactly enough"
)]
pub fn hit_test(
    size: (f64, f64),
    layout: OverlayLayout,
    selected_row: Option<usize>,
    panel_open: bool,
    x: f64,
    y: f64,
) -> Option<Hit> {
    if panel_open {
        let panel = action_panel_rect(size, layout);
        if panel.contains(x, y) {
            return (0..ACTIONS.len())
                .find(|index| action_row_rect(panel, *index).contains(x, y))
                .map(Hit::ActionRow);
        }
    }
    if layout.footer_height > 0.0 && actions_button_rect(size, layout).contains(x, y) {
        return Some(Hit::ActionsButton);
    }
    let row = layout.visible_row_at(size.1 as f32, y as f32)?;
    let bounds = row_rect(size, layout, row);
    if !bounds.contains(x, y) {
        return None;
    }
    if selected_row == Some(row) && close_button_rect(bounds, layout).contains(x, y) {
        return Some(Hit::CloseButton(row));
    }
    Some(Hit::Row(row))
}

pub type ViewHandler = Rc<dyn Fn(ViewEvent)>;

pub struct SwitcherViewIvars {
    model: RefCell<Option<FrameModel>>,
    handler: RefCell<Option<ViewHandler>>,
    tracking_area: RefCell<Option<Retained<NSTrackingArea>>>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclassing requirements beyond calling the designated initializer.
    // - SwitcherView does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "AltTabioSwitcherView"]
    #[ivars = SwitcherViewIvars]
    pub struct SwitcherView;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for SwitcherView {}

    impl SwitcherView {
        // SAFETY: the signatures match the NSView and NSResponder declarations.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            let model = self.ivars().model.borrow();
            if let Some(model) = model.as_ref() {
                draw_frame(self.bounds(), model);
            }
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            if let Some(previous) = self.ivars().tracking_area.borrow_mut().take() {
                self.removeTrackingArea(&previous);
            }
            let area = unsafe {
                // SAFETY: the view owns the tracking area and removes it before replacing it.
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    self.bounds(),
                    NSTrackingAreaOptions::MouseMoved
                        | NSTrackingAreaOptions::MouseEnteredAndExited
                        | NSTrackingAreaOptions::ActiveAlways
                        | NSTrackingAreaOptions::InVisibleRect,
                    Some(self),
                    None,
                )
            };
            self.addTrackingArea(&area);
            *self.ivars().tracking_area.borrow_mut() = Some(area);
            unsafe {
                // SAFETY: the superclass implements updateTrackingAreas.
                let _: () = msg_send![super(self), updateTrackingAreas];
            }
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let (x, y) = self.local_point(event);
            self.emit(ViewEvent::MouseMoved(x, y));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let (x, y) = self.local_point(event);
            self.emit(ViewEvent::MouseMoved(x, y));
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let (x, y) = self.local_point(event);
            self.emit(ViewEvent::MouseDown(x, y));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let (x, y) = self.local_point(event);
            self.emit(ViewEvent::MouseUp(x, y));
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            let (x, y) = self.local_point(event);
            self.emit(ViewEvent::RightMouseDown(x, y));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.emit(ViewEvent::MouseExited);
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let delta = event.scrollingDeltaY();
            if delta.abs() < 0.5 {
                return;
            }
            self.emit(ViewEvent::Scroll(if delta > 0.0 { 1 } else { -1 }));
        }
    }
);

impl SwitcherView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SwitcherViewIvars {
            model: RefCell::new(None),
            handler: RefCell::new(None),
            tracking_area: RefCell::new(None),
        });
        unsafe {
            // SAFETY: initWithFrame: is NSView's designated initializer.
            msg_send![super(this), initWithFrame: frame]
        }
    }

    fn local_point(&self, event: &NSEvent) -> (f64, f64) {
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        (point.x, point.y)
    }

    fn emit(&self, event: ViewEvent) {
        let handler = self.ivars().handler.borrow().clone();
        if let Some(handler) = handler {
            handler(event);
        }
    }
}

pub struct ContextMenuIvars {
    chosen: Cell<Option<WindowCommand>>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - ContextMenuTarget does not implement Drop.
    #[unsafe(super(objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AltTabioContextMenuTarget"]
    #[ivars = ContextMenuIvars]
    pub struct ContextMenuTarget;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for ContextMenuTarget {}

    impl ContextMenuTarget {
        // SAFETY: the action signature matches the target/action convention.
        #[unsafe(method(chooseCommand:))]
        fn choose_command(&self, sender: Option<&AnyObject>) {
            let Some(item) = sender.and_then(|sender| sender.downcast_ref::<NSMenuItem>()) else {
                return;
            };
            let command = match item.tag() {
                1 => Some(WindowCommand::Close),
                2 => Some(WindowCommand::Minimize),
                3 => Some(WindowCommand::Maximize),
                4 => Some(WindowCommand::Restore),
                5 => Some(WindowCommand::Terminate),
                6 => Some(WindowCommand::Run),
                7 => Some(WindowCommand::Hide),
                8 => Some(WindowCommand::Quit),
                _ => None,
            };
            self.ivars().chosen.set(command);
        }
    }
);

impl ContextMenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ContextMenuIvars {
            chosen: Cell::new(None),
        });
        unsafe {
            // SAFETY: init is NSObject's designated initializer.
            msg_send![super(this), init]
        }
    }
}

/// The Unicode function-key character `AppKit` uses as a menu key equivalent for F-key `number`.
fn function_key_equivalent(number: u32) -> String {
    char::from_u32(0xF704 + number - 1)
        .map(|value| value.to_string())
        .unwrap_or_default()
}

pub struct Overlay {
    panel: Retained<NSPanel>,
    view: Retained<SwitcherView>,
    glass: Option<Retained<NSGlassEffectView>>,
    mtm: MainThreadMarker,
}

impl Overlay {
    pub fn new(mtm: MainThreadMarker, handler: ViewHandler) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(900.0, 600.0));
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        unsafe {
            // SAFETY: the panel is owned by this Overlay rather than by a window controller.
            panel.setReleasedWhenClosed(false);
        }
        panel.setLevel(NSPopUpMenuWindowLevel);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        panel.setHidesOnDeactivate(false);
        panel.setBecomesKeyOnlyIfNeeded(true);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setOpaque(false);
        panel.setHasShadow(true);
        panel.setMovable(false);
        panel.setAcceptsMouseMovedEvents(true);
        panel.setIgnoresMouseEvents(false);
        // The Windows switcher appears instantly; skipping the window animation keeps that feel.
        panel.setAnimationBehavior(NSWindowAnimationBehavior::None);

        let view = SwitcherView::new(mtm, frame);
        *view.ivars().handler.borrow_mut() = Some(handler);
        view.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        // Liquid Glass exists from macOS 26; older systems fall back to the classic blur.
        let glass = if AnyClass::get(c"NSGlassEffectView").is_some() {
            let glass = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), frame);
            glass.setCornerRadius(CORNER_RADIUS);
            glass.setStyle(NSGlassEffectViewStyle::Regular);
            glass.setContentView(Some(&view));
            panel.setContentView(Some(&glass));
            Some(glass)
        } else {
            let effect = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
            effect.setMaterial(NSVisualEffectMaterial::HUDWindow);
            effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            effect.setState(NSVisualEffectState::Active);
            effect.addSubview(&view);
            panel.setContentView(Some(&effect));
            None
        };
        Self {
            panel,
            view,
            glass,
            mtm,
        }
    }

    /// Applies the resolved theme: the panel appearance so system colors resolve to it, and the
    /// glass tint that keeps the switcher's own dark or light surface.
    pub fn set_theme(&self, theme: ResolvedTheme, tokens: SwitcherTokens) {
        let name = unsafe {
            // SAFETY: the appearance name constants are static strings exported by AppKit.
            match theme {
                ResolvedTheme::Light => NSAppearanceNameAqua,
                ResolvedTheme::Dark => NSAppearanceNameDarkAqua,
            }
        };
        self.panel
            .setAppearance(NSAppearance::appearanceNamed(name).as_deref());
        if let Some(glass) = &self.glass {
            // A translucent tint keeps AltTabio's own dark or light surface while the glass
            // still refracts whatever sits behind the switcher.
            glass.setTintColor(Some(&rgba(tokens.canvas)));
        }
    }

    pub fn show_on_cursor_screen(&self) {
        let location = NSEvent::mouseLocation();
        let screen = NSScreen::screens(self.mtm)
            .iter()
            .find(|screen| {
                let frame = screen.frame();
                location.x >= frame.origin.x
                    && location.x < frame.origin.x + frame.size.width
                    && location.y >= frame.origin.y
                    && location.y < frame.origin.y + frame.size.height
            })
            .or_else(|| NSScreen::mainScreen(self.mtm));
        if let Some(screen) = screen {
            let area = screen.visibleFrame();
            let width = (area.size.width * OVERLAY_FRACTION).round();
            let height = (area.size.height * OVERLAY_FRACTION).round();
            let frame = NSRect::new(
                NSPoint::new(
                    (area.origin.x + (area.size.width - width) / 2.0).round(),
                    (area.origin.y + (area.size.height - height) / 2.0).round(),
                ),
                NSSize::new(width, height),
            );
            self.panel.setFrame_display(frame, false);
        }
        self.panel.orderFrontRegardless();
        self.view.updateTrackingAreas();
    }

    pub fn hide(&self) {
        self.panel.orderOut(None);
    }

    #[must_use]
    pub fn content_size(&self) -> (f64, f64) {
        let bounds = self.view.bounds();
        (bounds.size.width, bounds.size.height)
    }

    #[must_use]
    pub fn backing_scale(&self) -> f64 {
        self.panel
            .screen()
            .map_or(2.0, |screen| screen.backingScaleFactor())
    }

    /// Whether the cursor is over the panel; used to tell an outside click from a row click.
    #[must_use]
    pub fn contains_mouse(&self) -> bool {
        let location = NSEvent::mouseLocation();
        let frame = self.panel.frame();
        self.panel.isVisible()
            && location.x >= frame.origin.x
            && location.x < frame.origin.x + frame.size.width
            && location.y >= frame.origin.y
            && location.y < frame.origin.y + frame.size.height
    }

    pub fn present(&self, model: FrameModel) {
        *self.view.ivars().model.borrow_mut() = Some(model);
        self.view.setNeedsDisplay(true);
    }

    /// Runs the row command menu synchronously; call it outside any app-state borrow.
    #[must_use]
    pub fn show_context_menu(&self, x: f64, y: f64) -> Option<WindowCommand> {
        let target = ContextMenuTarget::new(self.mtm);
        let menu = NSMenu::new(self.mtm);
        menu.setAutoenablesItems(false);
        for (tag, title, key, modifiers) in [
            (
                1,
                "Close",
                function_key_equivalent(4),
                NSEventModifierFlags::empty(),
            ),
            (
                2,
                "Minimize",
                function_key_equivalent(5),
                NSEventModifierFlags::empty(),
            ),
            (
                3,
                "Zoom",
                function_key_equivalent(6),
                NSEventModifierFlags::empty(),
            ),
            (
                4,
                "Restore",
                function_key_equivalent(7),
                NSEventModifierFlags::empty(),
            ),
            (7, "Hide App", "h".to_owned(), NSEventModifierFlags::Command),
            (8, "Quit App", "q".to_owned(), NSEventModifierFlags::Command),
            (
                5,
                "Force Quit",
                function_key_equivalent(8),
                NSEventModifierFlags::empty(),
            ),
            (
                6,
                "New Instance",
                function_key_equivalent(9),
                NSEventModifierFlags::empty(),
            ),
        ] {
            let item = unsafe {
                // SAFETY: the selector exists on ContextMenuTarget with a matching signature.
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm),
                    &NSString::from_str(title),
                    Some(sel!(chooseCommand:)),
                    &NSString::from_str(&key),
                )
            };
            item.setKeyEquivalentModifierMask(modifiers);
            item.setTag(tag);
            unsafe {
                // SAFETY: the target outlives the menu; both are dropped after the pop-up.
                item.setTarget(Some(&target));
            }
            menu.addItem(&item);
        }
        let _shown = menu.popUpMenuPositioningItem_atLocation_inView(
            None,
            NSPoint::new(x, y),
            Some(&self.view),
        );
        target.ivars().chosen.get()
    }
}

/// The frame's colors as `NSColor`s, straight from the shared semantic tokens.
///
/// Text is never drawn in the accent color: the accent also tints the selected row, and accent
/// text on an accent tint drops well under the readable lightness gap.
struct Colors {
    canvas_tokens: SwitcherTokens,
    label: Retained<NSColor>,
    secondary: Retained<NSColor>,
    ring: Retained<NSColor>,
    well: Retained<NSColor>,
    surface: Retained<NSColor>,
    surface_edge: Retained<NSColor>,
    selection: Retained<NSColor>,
    raised: Retained<NSColor>,
    raised_edge: Retained<NSColor>,
    raised_base: Retained<NSColor>,
}

fn colors(tokens: SwitcherTokens) -> Colors {
    Colors {
        canvas_tokens: tokens,
        label: color(tokens.text, 1.0),
        secondary: color(tokens.text_secondary, 1.0),
        ring: rgba(tokens.ring),
        well: rgba(tokens.well),
        surface: rgba(tokens.surface),
        surface_edge: rgba(tokens.surface_edge),
        selection: rgba(tokens.selection),
        raised: color(tokens.raised, 1.0),
        raised_edge: rgba(tokens.raised_edge),
        raised_base: rgba(tokens.raised_base),
    }
}

fn rgba(value: Rgba) -> Retained<NSColor> {
    color(value.color, value.alpha)
}

fn color(value: Rgb8, alpha: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(value.red) / 255.0,
        f64::from(value.green) / 255.0,
        f64::from(value.blue) / 255.0,
        alpha,
    )
}

struct Fonts {
    title: Retained<NSFont>,
    title_match: Retained<NSFont>,
    detail: Retained<NSFont>,
    detail_match: Retained<NSFont>,
    number: Retained<NSFont>,
    keycap: Retained<NSFont>,
    hint: Retained<NSFont>,
}

fn fonts(compact: bool) -> Fonts {
    let (title, detail, number) = if compact {
        (13.0, 10.5, 11.5)
    } else {
        (15.0, 11.0, 12.5)
    };
    unsafe {
        // SAFETY: the font weight constants are static values exported by AppKit.
        Fonts {
            title: NSFont::systemFontOfSize_weight(title, NSFontWeightRegular),
            title_match: NSFont::systemFontOfSize_weight(title, NSFontWeightSemibold),
            detail: NSFont::systemFontOfSize_weight(detail, NSFontWeightRegular),
            detail_match: NSFont::systemFontOfSize_weight(detail, NSFontWeightSemibold),
            number: NSFont::monospacedDigitSystemFontOfSize_weight(number, NSFontWeightMedium),
            keycap: NSFont::systemFontOfSize_weight(11.0, NSFontWeightMedium),
            hint: NSFont::systemFontOfSize_weight(11.0, NSFontWeightRegular),
        }
    }
}

fn text_attributes(
    font: &NSFont,
    color: &NSColor,
    alignment: NSTextAlignment,
) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
    let style = NSMutableParagraphStyle::new();
    style.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    style.setAlignment(alignment);
    let keys: [&NSAttributedStringKey; 3] = unsafe {
        // SAFETY: the attribute name constants are static strings exported by AppKit.
        [
            NSFontAttributeName,
            NSForegroundColorAttributeName,
            NSParagraphStyleAttributeName,
        ]
    };
    let objects: [&AnyObject; 3] = [font, color, &style];
    NSDictionary::from_slices(&keys, &objects)
}

fn measure(text: &str, font: &NSFont) -> NSSize {
    let attributes = text_attributes(font, &NSColor::labelColor(), NSTextAlignment::Left);
    unsafe {
        // SAFETY: the attributes dictionary is live for the synchronous measurement.
        NSString::from_str(text).sizeWithAttributes(Some(&attributes))
    }
}

/// Vertically centers `height` inside `bounds`.
fn centered(bounds: Rect, height: f64) -> Rect {
    let height = height.min(bounds.height);
    Rect {
        left: bounds.left,
        top: bounds.top + (bounds.height - height) / 2.0,
        width: bounds.width,
        height,
    }
}

fn draw_text(text: &str, bounds: Rect, font: &NSFont, color: &NSColor, alignment: NSTextAlignment) {
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }
    let attributes = text_attributes(font, color, alignment);
    let string = NSString::from_str(text);
    let size = unsafe {
        // SAFETY: the attributes dictionary is live for the synchronous measurement.
        string.sizeWithAttributes(Some(&attributes))
    };
    unsafe {
        // SAFETY: drawing happens inside drawRect: with a current graphics context.
        string.drawInRect_withAttributes(centered(bounds, size.height).ns(), Some(&attributes));
    }
}

/// Draws `text` with the characters in `emphasis` set in `emphasis_font` and `emphasis_color`.
fn draw_text_emphasized(
    text: &str,
    emphasis: Option<Range<usize>>,
    bounds: Rect,
    font: &NSFont,
    color: &NSColor,
    emphasis_font: &NSFont,
    emphasis_color: &NSColor,
) {
    let Some(range) = emphasis else {
        draw_text(text, bounds, font, color, NSTextAlignment::Left);
        return;
    };
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }
    let utf16_offset =
        |characters: usize| -> usize { text.chars().take(characters).map(char::len_utf16).sum() };
    let location = utf16_offset(range.start);
    let length = utf16_offset(range.end).saturating_sub(location);
    let attributes = text_attributes(font, color, NSTextAlignment::Left);
    let string = unsafe {
        // SAFETY: both arguments are live objects for the initializer.
        NSMutableAttributedString::initWithString_attributes(
            NSMutableAttributedString::alloc(),
            &NSString::from_str(text),
            Some(&attributes),
        )
    };
    let emphasis_attributes = text_attributes(emphasis_font, emphasis_color, NSTextAlignment::Left);
    unsafe {
        // SAFETY: the range was computed in UTF-16 units of the same string.
        string.addAttributes_range(&emphasis_attributes, NSRange::new(location, length));
    }
    let size = string.size();
    string.drawInRect(centered(bounds, size.height).ns());
}

fn fill_rounded(rect: Rect, radius: f64, color: &NSColor) {
    color.setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect.ns(), radius, radius).fill();
}

/// Strokes a 1pt ring just inside `rect`, the way an inset outline sits on an image.
fn ring_rounded(rect: Rect, radius: f64, color: &NSColor) {
    let inset = Rect {
        left: rect.left + 0.5,
        top: rect.top + 0.5,
        width: (rect.width - 1.0).max(0.0),
        height: (rect.height - 1.0).max(0.0),
    };
    color.setStroke();
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        inset.ns(),
        (radius - 0.5).max(0.0),
        (radius - 0.5).max(0.0),
    );
    path.setLineWidth(1.0);
    path.stroke();
}

/// Draws `image` aspect-fitted into `bounds` and returns where it landed.
fn draw_image_fit(image: &NSImage, bounds: Rect) -> Option<Rect> {
    let size = image.size();
    if size.width <= 0.0 || size.height <= 0.0 || bounds.width <= 0.0 || bounds.height <= 0.0 {
        return None;
    }
    let scale = (bounds.width / size.width).min(bounds.height / size.height);
    let width = size.width * scale;
    let height = size.height * scale;
    let rect = Rect {
        left: bounds.left + (bounds.width - width) / 2.0,
        top: bounds.top + (bounds.height - height) / 2.0,
        width,
        height,
    };
    unsafe {
        // SAFETY: drawing happens inside drawRect: with a current graphics context.
        image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
            rect.ns(),
            NSRect::ZERO,
            NSCompositingOperation::SourceOver,
            1.0,
            true,
            None,
        );
    }
    Some(rect)
}

/// An SF Symbol rendered in `color` at `point_size`, or None when the system lacks it.
fn symbol(name: &str, point_size: f64, color: &NSColor) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    let configuration = unsafe {
        // SAFETY: the font weight constant is a static value exported by AppKit.
        NSImageSymbolConfiguration::configurationWithPointSize_weight(
            point_size,
            NSFontWeightMedium,
        )
    };
    let configuration = configuration.configurationByApplyingConfiguration(
        &NSImageSymbolConfiguration::configurationWithHierarchicalColor(color),
    );
    image.imageWithSymbolConfiguration(&configuration)
}

struct KeycapStyle<'a> {
    fill: &'a NSColor,
    edge: &'a NSColor,
    /// The bottom edge, darker than the fill, that gives the key its height.
    base: &'a NSColor,
    text: &'a NSColor,
}

impl<'a> KeycapStyle<'a> {
    fn raised(colors: &'a Colors) -> Self {
        Self {
            fill: &colors.raised,
            edge: &colors.raised_edge,
            base: &colors.raised_base,
            text: &colors.label,
        }
    }
}

/// Draws a key chip of the measured width at `left`, vertically centered in `bounds`; returns
/// the chip's right edge.
fn draw_keycap(
    text: &str,
    left: f64,
    bounds: Rect,
    minimum_width: f64,
    font: &NSFont,
    style: &KeycapStyle<'_>,
) -> f64 {
    let size = measure(text, font);
    let width = (size.width + 12.0).max(minimum_width).round();
    let height = (size.height + 5.0).round();
    let chip = Rect {
        left,
        top: (bounds.top + (bounds.height - height) / 2.0).round(),
        width,
        height,
    };
    // The base is a second, slightly taller rounded rect underneath so the bottom edge reads
    // as the side of a raised key rather than as a border.
    fill_rounded(
        Rect {
            top: chip.top + 1.0,
            ..chip
        },
        KEYCAP_RADIUS,
        style.base,
    );
    fill_rounded(chip, KEYCAP_RADIUS, style.fill);
    ring_rounded(chip, KEYCAP_RADIUS, style.edge);
    draw_text(
        text,
        Rect {
            top: chip.top - 0.5,
            ..chip
        },
        font,
        style.text,
        NSTextAlignment::Center,
    );
    chip.left + chip.width
}

fn draw_panel_ring(size: (f64, f64), colors: &Colors) {
    ring_rounded(
        Rect {
            left: 0.0,
            top: 0.0,
            width: size.0,
            height: size.1,
        },
        CORNER_RADIUS,
        &colors.ring,
    );
}

fn draw_preview(model: &FrameModel, size: (f64, f64), fonts: &Fonts, colors: &Colors) {
    let preview = preview_rect(size, model.layout);
    fill_rounded(preview, PREVIEW_RADIUS, &colors.well);
    if let Some(image) = &model.preview {
        let inset = Rect {
            left: preview.left + 1.0,
            top: preview.top + 1.0,
            width: (preview.width - 2.0).max(0.0),
            height: (preview.height - 2.0).max(0.0),
        };
        if let Some(drawn) = draw_image_fit(image, inset) {
            ring_rounded(drawn, 0.0, &colors.ring);
        }
    } else if let Some(message) = &model.preview_message {
        draw_text(
            message,
            Rect {
                left: preview.left + 24.0,
                top: preview.top,
                width: (preview.width - 48.0).max(0.0),
                height: preview.height,
            },
            &fonts.title,
            &colors.secondary,
            NSTextAlignment::Center,
        );
    }
    if model.options.visible_borders {
        ring_rounded(preview, PREVIEW_RADIUS, &colors.ring);
    }
}

fn draw_search_row(model: &FrameModel, size: (f64, f64), fonts: &Fonts, colors: &Colors) {
    let row = search_row_rect(size, model.layout);
    if row.height <= 0.0 {
        return;
    }
    fill_rounded(row, SEARCH_RADIUS, &colors.well);
    let mut left = row.left + INSET;
    if let Some(glass) = symbol("magnifyingglass", 12.0, &colors.secondary) {
        let icon = glass.size();
        let _ = draw_image_fit(
            &glass,
            Rect {
                left,
                top: row.top + (row.height - icon.height) / 2.0,
                width: icon.width,
                height: icon.height,
            },
        );
        left += icon.width + 10.0;
    }
    draw_text(
        &model.filter,
        Rect {
            left,
            top: row.top,
            width: (row.left + row.width - INSET - left).max(0.0),
            height: row.height,
        },
        &fonts.title,
        &colors.label,
        NSTextAlignment::Left,
    );
}

fn draw_footer(model: &FrameModel, size: (f64, f64), fonts: &Fonts, colors: &Colors) {
    let footer = footer_rect(size, model.layout);
    let Some(content) = &model.footer else {
        return;
    };
    if footer.height <= 0.0 {
        return;
    }
    let style = KeycapStyle::raised(colors);
    // Trailing hints are laid out from the right, label then key like a launcher's action bar;
    // the last one sits inside the actions button hit area.
    let mut right = footer.left + footer.width - 10.0;
    for hint in content.trailing.iter().rev() {
        let chip_width = (measure(hint.keys, &fonts.keycap).width + 10.0).max(KEYCAP_MINIMUM_WIDTH);
        let label_width = measure(hint.label, &fonts.hint).width.ceil();
        let chip_left = right - chip_width;
        let _ = draw_keycap(
            hint.keys,
            chip_left,
            footer,
            KEYCAP_MINIMUM_WIDTH,
            &fonts.keycap,
            &style,
        );
        let label_left = chip_left - 8.0 - label_width;
        draw_text(
            hint.label,
            Rect {
                left: label_left,
                top: footer.top,
                width: label_width + 1.0,
                height: footer.height,
            },
            &fonts.hint,
            &colors.secondary,
            NSTextAlignment::Left,
        );
        right = label_left - HINT_GAP;
    }
    draw_text(
        &content.status,
        Rect {
            left: footer.left + 10.0,
            top: footer.top,
            width: (right - footer.left - 10.0).max(0.0),
            height: footer.height,
        },
        &fonts.hint,
        &colors.secondary,
        NSTextAlignment::Left,
    );
}

fn draw_action_panel(model: &FrameModel, size: (f64, f64), fonts: &Fonts, colors: &Colors) {
    let Some(panel_model) = &model.action_panel else {
        return;
    };
    let panel = action_panel_rect(size, model.layout);
    // The panel floats over the preview well, so it needs its own nearly opaque surface.
    fill_rounded(panel, ACTION_PANEL_RADIUS, &colors.surface);
    ring_rounded(panel, ACTION_PANEL_RADIUS, &colors.surface_edge);
    draw_text(
        &panel_model.target,
        Rect {
            left: panel.left + ACTION_PANEL_PADDING + INSET,
            top: panel.top + ACTION_PANEL_PADDING,
            width: panel.width - ACTION_PANEL_PADDING * 2.0 - INSET * 2.0,
            height: ACTION_PANEL_HEADER_HEIGHT,
        },
        &fonts.detail,
        &colors.secondary,
        NSTextAlignment::Left,
    );
    let style = KeycapStyle::raised(colors);
    for (index, action) in ACTIONS.iter().enumerate() {
        let row = action_row_rect(panel, index);
        if index == panel_model.selected {
            fill_rounded(
                row,
                ACTION_PANEL_RADIUS - ACTION_PANEL_PADDING,
                &colors.selection,
            );
        }
        let chip_width =
            (measure(action.keys, &fonts.keycap).width + 10.0).max(KEYCAP_MINIMUM_WIDTH);
        let chip_left = row.left + row.width - INSET + 2.0 - chip_width;
        let _ = draw_keycap(
            action.keys,
            chip_left,
            row,
            KEYCAP_MINIMUM_WIDTH,
            &fonts.keycap,
            &style,
        );
        draw_text(
            action.label,
            Rect {
                left: row.left + INSET,
                top: row.top,
                width: (chip_left - 8.0 - row.left - INSET).max(0.0),
                height: row.height,
            },
            &fonts.title,
            &colors.label,
            NSTextAlignment::Left,
        );
    }
}

fn draw_overflow(model: &FrameModel, size: (f64, f64), fonts: &Fonts, colors: &Colors) {
    let text = match (model.hidden_above, model.hidden_below) {
        (_, below) if below > 0 => format!("{below} more below"),
        (above, _) if above > 0 => format!("{above} more above"),
        _ => return,
    };
    let slot = row_rect(size, model.layout, model.rows.len());
    if slot.top + slot.height > list_bottom(size, model.layout) + 0.5 {
        return;
    }
    draw_text(
        &text,
        slot,
        &fonts.detail,
        &colors.secondary,
        NSTextAlignment::Center,
    );
}

fn draw_close_button(model: &FrameModel, button: Rect, colors: &Colors) {
    let layout = model.layout;
    let background = match model.close_state {
        CloseButtonVisualState::Normal => None,
        CloseButtonVisualState::Hovered => Some(rgba(colors.canvas_tokens.control_hover)),
        CloseButtonVisualState::Pressed => Some(rgba(colors.canvas_tokens.control_pressed)),
    };
    if let Some(background) = background {
        fill_rounded(
            button,
            f64::from(layout.selection_radius) - 1.0,
            &background,
        );
    }
    let glyph = if model.options.compact_list {
        8.0
    } else {
        10.0
    };
    let left = button.left + (button.width - glyph) / 2.0;
    let top = button.top + (button.height - glyph) / 2.0;
    colors.label.setStroke();
    let path = NSBezierPath::bezierPath();
    path.setLineWidth(1.5);
    path.moveToPoint(NSPoint::new(left, top));
    path.lineToPoint(NSPoint::new(left + glyph, top + glyph));
    path.moveToPoint(NSPoint::new(left + glyph, top));
    path.lineToPoint(NSPoint::new(left, top + glyph));
    path.stroke();
}

/// Draws a row's number: secondary text at rest so the titles lead, a flat accent-tinted pill
/// while the modifier turns 1–9 into keys, and the accent itself the instant one was pressed.
fn draw_row_number(
    position: usize,
    slot: Rect,
    flashing: bool,
    modifier_held: bool,
    fonts: &Fonts,
    colors: &Colors,
) {
    let text = position.to_string();
    if position > 9 || (!flashing && !modifier_held) {
        draw_text(
            &text,
            slot,
            &fonts.number,
            &colors.secondary,
            NSTextAlignment::Center,
        );
        return;
    }
    let tokens = colors.canvas_tokens;
    let (fill, text_color) = if flashing {
        (
            color(tokens.keycap_pressed, 1.0),
            color(tokens.keycap_pressed_text, 1.0),
        )
    } else {
        (color(tokens.keycap_active, 1.0), colors.label.clone())
    };
    let height = (measure(&text, &fonts.number).height + 2.0).round();
    let pill = Rect {
        left: (slot.left + (slot.width - NUMBER_PILL_WIDTH) / 2.0).round(),
        top: (slot.top + (slot.height - height) / 2.0).round(),
        width: NUMBER_PILL_WIDTH,
        height,
    };
    fill_rounded(pill, KEYCAP_RADIUS, &fill);
    draw_text(
        &text,
        pill,
        &fonts.number,
        &text_color,
        NSTextAlignment::Center,
    );
}

fn badge_symbol(state: WindowState) -> Option<&'static str> {
    match state {
        WindowState::Normal => None,
        WindowState::Minimized => Some("arrow.down.right.and.arrow.up.left"),
        WindowState::Hidden => Some("eye.slash"),
        WindowState::OtherSpace => Some("rectangle.on.rectangle"),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one bounded pass draws a row; splitting it would scatter the geometry"
)]
fn draw_row(
    model: &FrameModel,
    item: &RowModel,
    bounds: Rect,
    fonts: &Fonts,
    colors: &Colors,
    icon_size: f64,
) {
    let layout = model.layout;
    let flashing = model.flash_position == Some(item.position);
    if item.selected || flashing {
        fill_rounded(
            bounds,
            f64::from(layout.selection_radius),
            &colors.selection,
        );
    }
    let mut left = bounds.left;
    if model.options.show_numbers {
        let slot = Rect {
            left,
            top: bounds.top,
            width: f64::from(layout.number_width),
            height: bounds.height,
        };
        draw_row_number(
            item.position,
            slot,
            flashing,
            model.held_modifier.is_some(),
            fonts,
            colors,
        );
        left += f64::from(layout.number_width);
    }
    if let Some(icon) = &item.icon {
        let slot = f64::from(layout.icon_slot_width);
        let _ = draw_image_fit(
            icon,
            Rect {
                left: left + (slot - icon_size) / 2.0,
                top: bounds.top + (bounds.height - icon_size) / 2.0,
                width: icon_size,
                height: icon_size,
            },
        );
    }
    left += f64::from(layout.icon_slot_width + layout.icon_text_gap);
    let close = item.selected.then(|| close_button_rect(bounds, layout));
    let mut text_right = close.map_or(bounds.left + bounds.width - INSET, |button| {
        button.left - f64::from(layout.close_button_gap)
    });
    if let Some(name) = badge_symbol(item.state)
        && let Some(badge) = symbol(name, BADGE_POINT_SIZE, &colors.secondary)
    {
        let size = badge.size();
        text_right -= size.width;
        let _ = draw_image_fit(
            &badge,
            Rect {
                left: text_right,
                top: bounds.top + (bounds.height - size.height) / 2.0,
                width: size.width,
                height: size.height,
            },
        );
        text_right -= 8.0;
    }
    let text_width = (text_right - left).max(0.0);
    let title_match = filter_match(&item.title, &model.filter);
    if model.options.show_app_names {
        let (title_top, title_bottom, name_top, name_bottom) = if model.options.compact_list {
            (1.0, 26.0, 22.0, bounds.height - 1.0)
        } else {
            (3.0, 35.0, 31.0, bounds.height - 2.0)
        };
        draw_text_emphasized(
            &item.title,
            title_match,
            Rect {
                left,
                top: bounds.top + title_top,
                width: text_width,
                height: title_bottom - title_top,
            },
            &fonts.title,
            &colors.label,
            &fonts.title_match,
            &colors.label,
        );
        draw_text_emphasized(
            &item.app_name,
            filter_match(&item.app_name, &model.filter),
            Rect {
                left,
                top: bounds.top + name_top,
                width: text_width,
                height: name_bottom - name_top,
            },
            &fonts.detail,
            &colors.secondary,
            &fonts.detail_match,
            &colors.label,
        );
    } else {
        draw_text_emphasized(
            &item.title,
            title_match,
            Rect {
                left,
                top: bounds.top,
                width: text_width,
                height: bounds.height,
            },
            &fonts.title,
            &colors.label,
            &fonts.title_match,
            &colors.label,
        );
    }
    if let Some(button) = close {
        draw_close_button(model, button, colors);
    }
}

fn draw_frame(bounds: NSRect, model: &FrameModel) {
    let size = (bounds.size.width, bounds.size.height);
    let layout = model.layout;
    let fonts = fonts(model.options.compact_list);
    let colors = colors(model.tokens);

    if model.options.visible_borders {
        draw_panel_ring(size, &colors);
    }
    if model.options.preview {
        draw_preview(model, size, &fonts, &colors);
    }
    if !model.filter.is_empty() {
        draw_search_row(model, size, &fonts, &colors);
    }
    draw_footer(model, size, &fonts, &colors);

    let icon_size = f64::from(if model.options.large_icons {
        layout.large_icon_size
    } else {
        layout.small_icon_size
    });
    let list_bottom = list_bottom(size, layout);
    for (row, item) in model.rows.iter().enumerate() {
        let bounds = row_rect(size, layout, row);
        if bounds.top + bounds.height > list_bottom + 0.5 {
            break;
        }
        draw_row(model, item, bounds, &fonts, &colors, icon_size);
    }
    draw_overflow(model, size, &fonts, &colors);
    draw_action_panel(model, size, &fonts, &colors);
}

#[cfg(test)]
mod tests {
    use super::*;
    use alttabio::overlay_layout::for_compact_list;

    #[test]
    fn hit_test_prefers_the_selected_rows_close_button() {
        let layout = for_compact_list(true);
        let size = (1000.0, 600.0);
        let row = row_rect(size, layout, 1);
        let close = close_button_rect(row, layout);
        let inside_close = (
            close.left + close.width / 2.0,
            close.top + close.height / 2.0,
        );

        assert_eq!(
            hit_test(size, layout, Some(1), false, inside_close.0, inside_close.1),
            Some(Hit::CloseButton(1))
        );
        assert_eq!(
            hit_test(size, layout, Some(0), false, inside_close.0, inside_close.1),
            Some(Hit::Row(1))
        );
        assert_eq!(
            hit_test(size, layout, None, false, 5.0, row.top + 5.0),
            None
        );
        assert_eq!(
            hit_test(
                size,
                layout,
                None,
                false,
                row.left + 5.0,
                row.top + row.height + 1.0
            ),
            None
        );
    }

    #[test]
    fn preview_sits_right_of_the_list_with_one_padding_between() {
        let layout = for_compact_list(true);
        let size = (1000.0, 600.0);
        let preview = preview_rect(size, layout);

        assert!((preview.left - (list_width(size, layout) + 36.0)).abs() < f64::EPSILON);
        assert!((preview.left + preview.width - (size.0 - 18.0)).abs() < f64::EPSILON);
        assert!((preview.top - 18.0).abs() < f64::EPSILON);
        assert!((preview.top + preview.height - (size.1 - 18.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn the_actions_button_and_open_panel_take_precedence_over_rows() {
        let layout = for_compact_list(true).with_footer(FOOTER_HEIGHT);
        let size = (1000.0, 600.0);
        let button = actions_button_rect(size, layout);
        let panel = action_panel_rect(size, layout);
        let second = action_row_rect(panel, 1);

        assert_eq!(
            hit_test(
                size,
                layout,
                None,
                false,
                button.left + 1.0,
                button.top + 1.0
            ),
            Some(Hit::ActionsButton)
        );
        assert_eq!(
            hit_test(
                size,
                layout,
                None,
                true,
                second.left + 1.0,
                second.top + 1.0
            ),
            Some(Hit::ActionRow(1))
        );
        assert_eq!(
            hit_test(
                size,
                layout,
                None,
                false,
                second.left + 1.0,
                second.top + 1.0
            ),
            None
        );
        assert!(panel.top + panel.height < button.top);
    }

    #[test]
    fn search_row_and_footer_take_their_reserved_space() {
        let layout = for_compact_list(true)
            .with_search_row(SEARCH_ROW_HEIGHT)
            .with_footer(FOOTER_HEIGHT);
        let size = (1000.0, 600.0);
        let search = search_row_rect(size, layout);
        let footer = footer_rect(size, layout);
        let preview = preview_rect(size, layout);

        assert!((search.top - 18.0).abs() < f64::EPSILON);
        assert!((search.height - 36.0).abs() < f64::EPSILON);
        assert!((row_rect(size, layout, 0).top - 66.0).abs() < f64::EPSILON);
        assert!((footer.top + footer.height - (size.1 - 18.0)).abs() < f64::EPSILON);
        assert!((footer.height - 28.0).abs() < f64::EPSILON);
        assert!((preview.top + preview.height - footer.top + 12.0).abs() < f64::EPSILON);
        assert!(footer_rect(size, for_compact_list(true)).height <= 0.0);
    }
}
