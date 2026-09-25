//! The switcher panel: a non-activating floating `NSPanel` over Liquid Glass with one custom view
//! that draws a strip of app icons, the selected app's windows under it, and, when previews are
//! on, the selected window beside them.

use alttabio::input::WindowCommand;
use alttabio::theme::{ResolvedTheme, Rgb8, Rgba, SwitcherTokens};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSBackingStoreType, NSBezierPath, NSColor, NSCompositingOperation, NSEvent,
    NSEventModifierFlags, NSFont, NSFontAttributeName, NSFontWeightMedium, NSFontWeightRegular,
    NSForegroundColorAttributeName, NSGlassEffectView, NSGlassEffectViewStyle, NSGraphicsContext,
    NSImage, NSLineBreakMode, NSMenu, NSMenuItem, NSMutableParagraphStyle, NSPanel,
    NSParagraphStyleAttributeName, NSPopUpMenuWindowLevel, NSScreen, NSStringDrawing,
    NSTextAlignment, NSTrackingArea, NSTrackingAreaOptions, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindowAnimationBehavior,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{
    NSAttributedStringKey, NSDictionary, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const CORNER_RADIUS: f64 = 24.0;
const PADDING: f64 = 12.0;
/// Plates and the preview well sit one padding inside the panel, so their corners follow the
/// panel's with the padding taken off.
const PLATE_RADIUS: f64 = CORNER_RADIUS - PADDING;
const TILE_MAX: f64 = 64.0;
/// Many apps shrink the tiles down to this before the strip starts scrolling.
const TILE_MIN: f64 = 44.0;
/// The icon's share of its tile; the rest is the selection plate showing around it.
const ICON_SHARE: f64 = 0.75;
/// The row under the strip that names the selected app.
const NAME_HEIGHT: f64 = 22.0;
const LIST_GAP: f64 = 6.0;
const ROW_HEIGHT: f64 = 36.0;
/// The list grows to this many rows; longer window lists scroll.
const MAX_ROWS: usize = 8;
const MIN_CONTENT_WIDTH: f64 = 456.0;
/// Row text lines up with the visible edge of a full-size icon above it: 8pt of tile around the
/// icon plus the transparent margin macOS app icons carry inside their image, about a tenth of it.
const TEXT_INSET: f64 = 12.0;
/// The column of window numbers before the titles.
const NUMBER_WIDTH: f64 = 20.0;
const CLOSE_SIZE: f64 = 24.0;
const STATE_GAP: f64 = 12.0;
const PREVIEW_WIDTH: f64 = 400.0;
const PREVIEW_MIN_HEIGHT: f64 = 250.0;
const PREVIEW_GAP: f64 = 12.0;
/// The capture sits this far inside the preview well, rounded to the well's radius minus it.
const PREVIEW_INSET: f64 = 8.0;
const LIST_WIDTH_BESIDE_PREVIEW: f64 = 340.0;
/// Scrolled points on a trackpad per selection step; a mouse wheel notch is always one step.
const SCROLL_STEP: f64 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewEvent {
    MouseMoved(f64, f64),
    MouseDown(f64, f64),
    MouseUp(f64, f64),
    RightMouseDown(f64, f64),
    MouseExited,
    /// Positive steps move down the window list.
    Scroll(i32),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CloseButtonVisualState {
    #[default]
    Normal,
    Hovered,
    Pressed,
}

/// Where a window is when it is not plainly on the current desktop.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WindowState {
    #[default]
    Normal,
    Minimized,
    Hidden,
    OtherDesktop,
}

impl WindowState {
    const fn label(self) -> Option<&'static str> {
        match self {
            Self::Normal => None,
            Self::Minimized => Some("Minimized"),
            Self::Hidden => Some("Hidden"),
            Self::OtherDesktop => Some("Other desktop"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Hit {
    Tile(usize),
    Row(usize),
    CloseButton(usize),
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

    fn right(self) -> f64 {
        self.left + self.width
    }

    fn ns(self) -> NSRect {
        NSRect::new(
            NSPoint::new(self.left, self.top),
            NSSize::new(self.width, self.height),
        )
    }
}

/// The panel's geometry for one session: its size, the tile edge, and how many tiles and rows
/// it holds. Points, top-left origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub width: f64,
    pub height: f64,
    tile: f64,
    /// How many tiles the strip shows at once.
    pub tile_slots: usize,
    /// How many window rows the list shows at once.
    pub row_slots: usize,
    preview: bool,
}

impl Layout {
    /// The panel for `apps` apps whose longest window list has `windows` entries, no larger than
    /// `bounds`. The strip sets the width and the longest list the height, so neither changes
    /// while the selection moves.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "app and row counts are small, and the floored quotients are positive"
    )]
    pub fn new(apps: usize, windows: usize, preview: bool, bounds: (f64, f64)) -> Self {
        let apps = apps.max(1);
        let minimum = if preview {
            LIST_WIDTH_BESIDE_PREVIEW + PREVIEW_GAP + PREVIEW_WIDTH
        } else {
            MIN_CONTENT_WIDTH
        };
        let maximum = (bounds.0 - PADDING * 2.0).max(minimum);
        let content = (apps as f64 * TILE_MAX).clamp(minimum, maximum);
        let tile = (content / apps as f64).clamp(TILE_MIN, TILE_MAX).floor();
        let tile_slots = ((content / tile).floor() as usize).clamp(1, apps);
        let list = windows.clamp(1, MAX_ROWS) as f64 * ROW_HEIGHT;
        let list = if preview {
            list.max(PREVIEW_MIN_HEIGHT)
        } else {
            list
        };
        let above = PADDING + tile + NAME_HEIGHT + LIST_GAP;
        let height = (above + list + PADDING).min(bounds.1.max(above + ROW_HEIGHT + PADDING));
        let row_slots = (((height - above - PADDING) / ROW_HEIGHT).floor() as usize).max(1);
        Self {
            width: (content + PADDING * 2.0).round(),
            height: height.round(),
            tile,
            tile_slots,
            row_slots,
            preview,
        }
    }

    #[must_use]
    pub const fn size(&self) -> (f64, f64) {
        (self.width, self.height)
    }

    fn content_width(&self) -> f64 {
        self.width - PADDING * 2.0
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "tile slots are small on-screen counts"
    )]
    fn tile_rect(&self, slot: usize) -> Rect {
        Rect {
            left: PADDING + slot as f64 * self.tile,
            top: PADDING,
            width: self.tile,
            height: self.tile,
        }
    }

    fn name_top(&self) -> f64 {
        PADDING + self.tile
    }

    fn list_rect(&self) -> Rect {
        let top = self.name_top() + NAME_HEIGHT + LIST_GAP;
        let width = if self.preview {
            self.content_width() - PREVIEW_GAP - PREVIEW_WIDTH
        } else {
            self.content_width()
        };
        Rect {
            left: PADDING,
            top,
            width,
            height: (self.height - PADDING - top).max(0.0),
        }
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "row indices are small on-screen counts"
    )]
    fn row_rect(&self, row: usize) -> Rect {
        let list = self.list_rect();
        Rect {
            top: list.top + row as f64 * ROW_HEIGHT,
            height: ROW_HEIGHT,
            ..list
        }
    }

    /// The preview well's size, for sizing the captures that fill it.
    #[must_use]
    pub fn preview_size(&self) -> Option<(f64, f64)> {
        self.preview_rect().map(|area| (area.width, area.height))
    }

    fn preview_rect(&self) -> Option<Rect> {
        self.preview.then(|| {
            let list = self.list_rect();
            Rect {
                left: list.right() + PREVIEW_GAP,
                width: PREVIEW_WIDTH,
                ..list
            }
        })
    }

    /// Where the pointer is, given how many tiles and rows are drawn and which row carries the
    /// close button.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the offsets are checked nonnegative and map to small slot and row indices"
    )]
    pub fn hit(
        &self,
        tiles: usize,
        rows: usize,
        close_row: Option<usize>,
        x: f64,
        y: f64,
    ) -> Option<Hit> {
        let strip = Rect {
            left: PADDING,
            top: PADDING,
            width: self.content_width(),
            height: self.tile,
        };
        if strip.contains(x, y) {
            let slot = ((x - PADDING) / self.tile) as usize;
            return (slot < tiles).then_some(Hit::Tile(slot));
        }
        let list = self.list_rect();
        if !list.contains(x, y) {
            return None;
        }
        let row = ((y - list.top) / ROW_HEIGHT) as usize;
        if row >= rows {
            return None;
        }
        if close_row == Some(row) && close_button_rect(self.row_rect(row)).contains(x, y) {
            return Some(Hit::CloseButton(row));
        }
        Some(Hit::Row(row))
    }
}

fn close_button_rect(row: Rect) -> Rect {
    let inset = (row.height - CLOSE_SIZE) / 2.0;
    Rect {
        left: row.right() - inset - CLOSE_SIZE,
        top: row.top + inset,
        width: CLOSE_SIZE,
        height: CLOSE_SIZE,
    }
}

/// The first of `shown` entries to draw out of `total` so that `selected` is in view, moving
/// the previous start only as far as needed. A pointer resting on a row therefore never makes
/// the list scroll under it.
#[must_use]
pub fn scroll_into_view(start: usize, selected: usize, total: usize, shown: usize) -> usize {
    let start = start.min(total.saturating_sub(shown));
    if selected < start {
        selected
    } else if shown > 0 && selected >= start + shown {
        selected + 1 - shown
    } else {
        start
    }
}

/// The note in the slot under `shown` rows drawn from `start` out of `total`. It sits below the
/// rows, so it counts the windows below them; once the list has scrolled to its end, every
/// hidden window is above.
#[must_use]
pub fn more_note(total: usize, start: usize, shown: usize) -> Option<String> {
    let below = total.saturating_sub(start + shown);
    if below > 0 {
        Some(format!("{below} more"))
    } else if start > 0 {
        Some(format!("{start} more above"))
    } else {
        None
    }
}

pub struct Tile {
    pub name: String,
    pub icon: Option<Retained<NSImage>>,
    pub selected: bool,
}

pub struct Row {
    /// The key that picks this window, for the first nine.
    pub number: Option<usize>,
    pub title: String,
    pub state: WindowState,
    pub selected: bool,
}

pub struct PreviewModel {
    pub image: Option<Retained<NSImage>>,
    pub message: Option<String>,
}

pub struct FrameModel {
    pub layout: Layout,
    pub tokens: SwitcherTokens,
    pub tiles: Vec<Tile>,
    pub rows: Vec<Row>,
    /// Drawn where the rows go when the selected app has none.
    pub empty_note: Option<String>,
    /// "4 more", or "4 more above" at the end of the list, in the slot after the last row when
    /// the list scrolls.
    pub more_note: Option<String>,
    pub close_state: CloseButtonVisualState,
    /// Present when previews are on.
    pub preview: Option<PreviewModel>,
}

pub type ViewHandler = Rc<dyn Fn(ViewEvent)>;

pub struct SwitcherViewIvars {
    model: RefCell<Option<FrameModel>>,
    handler: RefCell<Option<ViewHandler>>,
    tracking_area: RefCell<Option<Retained<NSTrackingArea>>>,
    // Trackpad scrolling arrives in small precise deltas; they add up to whole steps here.
    scrolled: Cell<f64>,
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
                draw_frame(model);
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
            if !event.hasPreciseScrollingDeltas() {
                if delta.abs() >= 0.5 {
                    self.emit(ViewEvent::Scroll(if delta > 0.0 { -1 } else { 1 }));
                }
                return;
            }
            let scrolled = self.ivars().scrolled.get() + delta;
            if scrolled.abs() < SCROLL_STEP {
                self.ivars().scrolled.set(scrolled);
                return;
            }
            self.ivars().scrolled.set(0.0);
            self.emit(ViewEvent::Scroll(if scrolled > 0.0 { -1 } else { 1 }));
        }
    }
);

impl SwitcherView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SwitcherViewIvars {
            model: RefCell::new(None),
            handler: RefCell::new(None),
            tracking_area: RefCell::new(None),
            scrolled: Cell::new(0.0),
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
            self.ivars().chosen.set(command_for_tag(item.tag()));
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

const fn command_for_tag(tag: isize) -> Option<WindowCommand> {
    match tag {
        1 => Some(WindowCommand::Close),
        2 => Some(WindowCommand::Minimize),
        3 => Some(WindowCommand::Hide),
        4 => Some(WindowCommand::Quit),
        5 => Some(WindowCommand::Terminate),
        _ => None,
    }
}

pub struct Overlay {
    panel: Retained<NSPanel>,
    view: Retained<SwitcherView>,
    glass: Option<Retained<NSGlassEffectView>>,
    mtm: MainThreadMarker,
}

impl Overlay {
    pub fn new(mtm: MainThreadMarker, handler: ViewHandler) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(480.0, 200.0));
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
        // The system switcher appears at once; a window animation would only delay it.
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

    fn cursor_screen(&self) -> Option<Retained<NSScreen>> {
        let location = NSEvent::mouseLocation();
        NSScreen::screens(self.mtm)
            .iter()
            .find(|screen| {
                let frame = screen.frame();
                location.x >= frame.origin.x
                    && location.x < frame.origin.x + frame.size.width
                    && location.y >= frame.origin.y
                    && location.y < frame.origin.y + frame.size.height
            })
            .or_else(|| NSScreen::mainScreen(self.mtm))
    }

    /// The largest panel the cursor's display takes.
    #[must_use]
    pub fn max_size(&self) -> (f64, f64) {
        self.cursor_screen().map_or((1000.0, 700.0), |screen| {
            let area = screen.visibleFrame().size;
            ((area.width * 0.9).round(), (area.height * 0.8).round())
        })
    }

    /// Shows the panel at `size`, centered on the cursor's display.
    pub fn show(&self, size: (f64, f64)) {
        if let Some(screen) = self.cursor_screen() {
            let area = screen.visibleFrame();
            let (width, height) = (size.0.min(area.size.width), size.1.min(area.size.height));
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

    /// Resizes the visible panel with its top edge and horizontal center kept, so the strip
    /// stays where it was when the window list grows.
    pub fn resize(&self, size: (f64, f64)) {
        let frame = self.panel.frame();
        if (frame.size.width - size.0).abs() < 0.5 && (frame.size.height - size.1).abs() < 0.5 {
            return;
        }
        let center_x = frame.origin.x + frame.size.width / 2.0;
        let top = frame.origin.y + frame.size.height;
        let resized = NSRect::new(
            NSPoint::new((center_x - size.0 / 2.0).round(), (top - size.1).round()),
            NSSize::new(size.0, size.1),
        );
        self.panel.setFrame_display(resized, true);
        self.view.updateTrackingAreas();
    }

    pub fn hide(&self) {
        self.panel.orderOut(None);
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

    /// Runs the command menu synchronously; call it outside any app-state borrow. Window
    /// commands appear only when a window is selected; the app commands name the app.
    #[must_use]
    pub fn show_context_menu(
        &self,
        x: f64,
        y: f64,
        window: bool,
        app_name: &str,
    ) -> Option<WindowCommand> {
        let target = ContextMenuTarget::new(self.mtm);
        let menu = NSMenu::new(self.mtm);
        menu.setAutoenablesItems(false);
        let mut entries: Vec<Option<(isize, String, &str)>> = Vec::new();
        if window {
            entries.push(Some((1, "Close Window".to_owned(), "w")));
            entries.push(Some((2, "Minimize Window".to_owned(), "m")));
            entries.push(None);
        }
        entries.push(Some((3, format!("Hide {app_name}"), "h")));
        entries.push(Some((4, format!("Quit {app_name}"), "q")));
        entries.push(None);
        entries.push(Some((5, format!("Force Quit {app_name}"), "")));
        for entry in entries {
            let Some((tag, title, key)) = entry else {
                menu.addItem(&NSMenuItem::separatorItem(self.mtm));
                continue;
            };
            let item = unsafe {
                // SAFETY: the selector exists on ContextMenuTarget with a matching signature.
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm),
                    &NSString::from_str(&title),
                    Some(sel!(chooseCommand:)),
                    &NSString::from_str(key),
                )
            };
            item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
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
struct Colors {
    label: Retained<NSColor>,
    secondary: Retained<NSColor>,
    ring: Retained<NSColor>,
    well: Retained<NSColor>,
    selection: Retained<NSColor>,
    control_hover: Retained<NSColor>,
    control_pressed: Retained<NSColor>,
}

fn colors(tokens: SwitcherTokens) -> Colors {
    Colors {
        label: color(tokens.text, 1.0),
        secondary: color(tokens.text_secondary, 1.0),
        ring: rgba(tokens.ring),
        well: rgba(tokens.well),
        selection: rgba(tokens.selection),
        control_hover: rgba(tokens.control_hover),
        control_pressed: rgba(tokens.control_pressed),
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
    name: Retained<NSFont>,
    detail: Retained<NSFont>,
    number: Retained<NSFont>,
}

fn fonts() -> Fonts {
    unsafe {
        // SAFETY: the font weight constants are static values exported by AppKit.
        Fonts {
            title: NSFont::systemFontOfSize_weight(13.0, NSFontWeightRegular),
            name: NSFont::systemFontOfSize_weight(12.0, NSFontWeightMedium),
            detail: NSFont::systemFontOfSize_weight(12.0, NSFontWeightRegular),
            number: NSFont::monospacedDigitSystemFontOfSize_weight(13.0, NSFontWeightRegular),
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
    let height = size.height.min(bounds.height);
    let centered = Rect {
        top: bounds.top + (bounds.height - height) / 2.0,
        height,
        ..bounds
    };
    unsafe {
        // SAFETY: drawing happens inside drawRect: with a current graphics context.
        string.drawInRect_withAttributes(centered.ns(), Some(&attributes));
    }
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

/// Where `size` lands when aspect-fitted and centered in `bounds`.
fn fitted(size: NSSize, bounds: Rect) -> Option<Rect> {
    if size.width <= 0.0 || size.height <= 0.0 || bounds.width <= 0.0 || bounds.height <= 0.0 {
        return None;
    }
    let scale = (bounds.width / size.width).min(bounds.height / size.height);
    let width = size.width * scale;
    let height = size.height * scale;
    Some(Rect {
        left: bounds.left + (bounds.width - width) / 2.0,
        top: bounds.top + (bounds.height - height) / 2.0,
        width,
        height,
    })
}

/// Draws `image` aspect-fitted into `bounds` and returns where it landed.
fn draw_image_fit(image: &NSImage, bounds: Rect) -> Option<Rect> {
    let rect = fitted(image.size(), bounds)?;
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

fn draw_frame(model: &FrameModel) {
    let fonts = fonts();
    let colors = colors(model.tokens);
    draw_strip(model, &fonts, &colors);
    let layout = model.layout;
    if let Some(note) = &model.empty_note {
        draw_note(note, layout.row_rect(0), 0.0, &fonts, &colors);
    }
    for (index, row) in model.rows.iter().enumerate() {
        draw_row(model, row, layout.row_rect(index), &fonts, &colors);
    }
    if let Some(note) = &model.more_note {
        // Under numbered rows, the count lines up with their titles.
        draw_note(
            note,
            layout.row_rect(model.rows.len()),
            NUMBER_WIDTH,
            &fonts,
            &colors,
        );
    }
    if let (Some(preview), Some(area)) = (&model.preview, layout.preview_rect()) {
        draw_preview(preview, area, &fonts, &colors);
    }
}

fn draw_strip(model: &FrameModel, fonts: &Fonts, colors: &Colors) {
    let layout = model.layout;
    for (slot, tile) in model.tiles.iter().enumerate() {
        let rect = layout.tile_rect(slot);
        if tile.selected {
            fill_rounded(rect, PLATE_RADIUS, &colors.selection);
        }
        let icon = layout.tile * ICON_SHARE;
        let icon_rect = Rect {
            left: rect.left + (rect.width - icon) / 2.0,
            top: rect.top + (rect.height - icon) / 2.0,
            width: icon,
            height: icon,
        };
        if let Some(image) = &tile.icon {
            let _ = draw_image_fit(image, icon_rect);
        } else {
            let initial = tile.name.chars().take(1).collect::<String>();
            draw_text(
                &initial,
                icon_rect,
                &fonts.name,
                &colors.secondary,
                NSTextAlignment::Center,
            );
        }
        if tile.selected {
            // The name sits centered under its tile, pushed inward at the panel's edges.
            let width = measure(&tile.name, &fonts.name)
                .width
                .ceil()
                .min(layout.content_width());
            let left = (rect.left + (rect.width - width) / 2.0)
                .clamp(PADDING, layout.width - PADDING - width);
            draw_text(
                &tile.name,
                Rect {
                    left,
                    top: layout.name_top(),
                    width,
                    height: NAME_HEIGHT,
                },
                &fonts.name,
                &colors.label,
                NSTextAlignment::Center,
            );
        }
    }
}

fn draw_row(model: &FrameModel, row: &Row, bounds: Rect, fonts: &Fonts, colors: &Colors) {
    if row.selected {
        fill_rounded(bounds, PLATE_RADIUS, &colors.selection);
    }
    let mut right = bounds.right() - TEXT_INSET;
    if row.selected {
        let button = close_button_rect(bounds);
        draw_close_button(model.close_state, button, colors);
        right = button.left - STATE_GAP / 2.0;
    }
    if let Some(label) = row.state.label() {
        let width = measure(label, &fonts.detail).width.ceil();
        draw_text(
            label,
            Rect {
                left: right - width,
                width,
                ..bounds
            },
            &fonts.detail,
            &colors.secondary,
            NSTextAlignment::Right,
        );
        right -= width + STATE_GAP;
    }
    let left = bounds.left + TEXT_INSET;
    if let Some(number) = row.number {
        draw_text(
            &number.to_string(),
            Rect {
                left,
                width: NUMBER_WIDTH,
                ..bounds
            },
            &fonts.number,
            &colors.secondary,
            NSTextAlignment::Left,
        );
    }
    // Rows past the ninth have no number but keep the column, so the titles stay in line.
    let left = left + NUMBER_WIDTH;
    draw_text(
        &row.title,
        Rect {
            left,
            width: (right - left).max(0.0),
            ..bounds
        },
        &fonts.title,
        &colors.label,
        NSTextAlignment::Left,
    );
}

fn draw_note(text: &str, bounds: Rect, indent: f64, fonts: &Fonts, colors: &Colors) {
    draw_text(
        text,
        Rect {
            left: bounds.left + TEXT_INSET + indent,
            width: (bounds.width - TEXT_INSET * 2.0 - indent).max(0.0),
            ..bounds
        },
        &fonts.title,
        &colors.secondary,
        NSTextAlignment::Left,
    );
}

fn draw_close_button(state: CloseButtonVisualState, button: Rect, colors: &Colors) {
    let background = match state {
        CloseButtonVisualState::Normal => None,
        CloseButtonVisualState::Hovered => Some(&colors.control_hover),
        CloseButtonVisualState::Pressed => Some(&colors.control_pressed),
    };
    if let Some(background) = background {
        // The button sits inside the row's plate, so its corners follow the plate's.
        let inset = (ROW_HEIGHT - CLOSE_SIZE) / 2.0;
        fill_rounded(button, PLATE_RADIUS - inset, background);
    }
    let glyph = 8.0;
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

fn draw_preview(preview: &PreviewModel, area: Rect, fonts: &Fonts, colors: &Colors) {
    fill_rounded(area, PLATE_RADIUS, &colors.well);
    if let Some(image) = &preview.image {
        let inner = Rect {
            left: area.left + PREVIEW_INSET,
            top: area.top + PREVIEW_INSET,
            width: (area.width - PREVIEW_INSET * 2.0).max(0.0),
            height: (area.height - PREVIEW_INSET * 2.0).max(0.0),
        };
        if let Some(rect) = fitted(image.size(), inner) {
            let radius = PLATE_RADIUS - PREVIEW_INSET;
            NSGraphicsContext::saveGraphicsState_class();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect.ns(), radius, radius)
                .addClip();
            let _ = draw_image_fit(image, rect);
            NSGraphicsContext::restoreGraphicsState_class();
            ring_rounded(rect, radius, &colors.ring);
        }
    } else if let Some(message) = &preview.message {
        let paragraph = Rect {
            left: area.left + 24.0,
            width: (area.width - 48.0).max(0.0),
            ..area
        };
        draw_text(
            message,
            paragraph,
            &fonts.detail,
            &colors.secondary,
            NSTextAlignment::Center,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (f64, f64) = (1512.0, 900.0);

    #[test]
    fn the_strip_sets_the_width_and_the_longest_list_the_height() {
        let few = Layout::new(3, 2, false, SCREEN);
        assert!((few.width - (MIN_CONTENT_WIDTH + PADDING * 2.0)).abs() < f64::EPSILON);
        assert_eq!(few.row_slots, 2);
        assert_eq!(few.tile_slots, 3);

        let many = Layout::new(12, 30, false, SCREEN);
        assert!((many.width - (12.0 * TILE_MAX + PADDING * 2.0)).abs() < f64::EPSILON);
        assert_eq!(many.row_slots, MAX_ROWS);
        assert!(many.height > few.height);
    }

    #[test]
    fn crowded_strips_shrink_their_tiles_and_then_scroll() {
        let crowded = Layout::new(24, 1, false, (1000.0, 700.0));
        assert!(crowded.width <= 1000.0);
        assert!(crowded.tile < TILE_MAX && crowded.tile >= TILE_MIN);
        assert!(crowded.tile_slots < 24);
    }

    #[test]
    fn a_short_display_limits_the_rows() {
        let short = Layout::new(2, 20, false, (1000.0, 300.0));
        assert!(short.height <= 300.0);
        assert!(short.row_slots < MAX_ROWS);
        assert!(short.row_slots >= 1);
    }

    #[test]
    fn the_preview_sits_beside_the_list_and_sets_a_minimum_height() {
        let layout = Layout::new(2, 1, true, SCREEN);
        let list = layout.list_rect();
        let preview = layout.preview_rect();

        assert!(list.height >= PREVIEW_MIN_HEIGHT);
        assert_eq!(
            preview.map(|area| (area.left, area.top, area.height)),
            Some((list.right() + PREVIEW_GAP, list.top, list.height))
        );
        assert!(Layout::new(2, 1, false, SCREEN).preview_rect().is_none());
    }

    #[test]
    fn hits_find_tiles_rows_and_the_close_button() {
        let layout = Layout::new(3, 4, false, SCREEN);
        let second_tile = layout.tile_rect(1);
        assert_eq!(
            layout.hit(3, 4, None, second_tile.left + 1.0, second_tile.top + 1.0),
            Some(Hit::Tile(1))
        );
        let row = layout.row_rect(2);
        assert_eq!(
            layout.hit(3, 4, None, row.left + 5.0, row.top + 5.0),
            Some(Hit::Row(2))
        );
        let close = close_button_rect(row);
        assert_eq!(
            layout.hit(3, 4, Some(2), close.left + 2.0, close.top + 2.0),
            Some(Hit::CloseButton(2))
        );
        assert_eq!(
            layout.hit(3, 4, Some(1), close.left + 2.0, close.top + 2.0),
            Some(Hit::Row(2))
        );
        // Slots past the drawn tiles and rows, and the name line, are not targets.
        let fourth_tile = layout.tile_rect(3);
        assert_eq!(
            layout.hit(3, 4, None, fourth_tile.left + 1.0, fourth_tile.top + 1.0),
            None
        );
        assert_eq!(layout.hit(3, 2, None, row.left + 5.0, row.top + 5.0), None);
        assert_eq!(layout.hit(3, 4, None, 100.0, layout.name_top() + 2.0), None);
    }

    #[test]
    fn scrolling_into_view_moves_only_as_far_as_needed() {
        assert_eq!(scroll_into_view(0, 3, 10, 5), 0);
        assert_eq!(scroll_into_view(0, 5, 10, 5), 1);
        assert_eq!(scroll_into_view(4, 2, 10, 5), 2);
        assert_eq!(scroll_into_view(8, 9, 10, 5), 5);
        assert_eq!(scroll_into_view(3, 1, 2, 5), 0);
    }

    #[test]
    fn the_more_note_counts_the_windows_on_the_side_they_are_hidden() {
        assert_eq!(more_note(10, 0, 7).as_deref(), Some("3 more"));
        assert_eq!(more_note(10, 2, 7).as_deref(), Some("1 more"));
        assert_eq!(more_note(10, 3, 7).as_deref(), Some("3 more above"));
        assert_eq!(more_note(5, 0, 5), None);
    }

    #[test]
    fn menu_tags_name_the_commands() {
        assert_eq!(command_for_tag(1), Some(WindowCommand::Close));
        assert_eq!(command_for_tag(5), Some(WindowCommand::Terminate));
        assert_eq!(command_for_tag(0), None);
    }
}
