//! The switcher panel: a non-activating floating `NSPanel` over Liquid Glass with one custom view
//! that draws a strip of app icons, the selected app's windows under it, and, when previews are
//! on, the selected window beside them.

use super::screen::{cursor_screen, frame_contains};
use alttabio::close_button::CloseButtonVisualState;
use alttabio::input::WindowCommand;
use alttabio::panel_layout::{
    CLOSE_SIZE, CORNER_RADIUS, ICON_SHARE, Layout, NAME_HEIGHT, NUMBER_WIDTH, PADDING,
    PLATE_RADIUS, PREVIEW_INSET, ROW_HEIGHT, STATE_GAP, TEXT_INSET, WindowState, close_button_rect,
    image_area,
};
use alttabio::preview_layout::{Rect, Size, fit};
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
    NSParagraphStyleAttributeName, NSPopUpMenuWindowLevel, NSStringDrawing, NSTextAlignment,
    NSTrackingArea, NSTrackingAreaOptions, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindowAnimationBehavior,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{
    NSAttributedStringKey, NSDictionary, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

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

pub struct Tile {
    pub name: String,
    pub icon: Option<Retained<NSImage>>,
    pub selected: bool,
}

impl PartialEq for Tile {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.selected == other.selected
            && same_image(self.icon.as_deref(), other.icon.as_deref())
    }
}

/// Images compare by identity: the app hands the view the same object until the picture changes.
fn same_image(first: Option<&NSImage>, second: Option<&NSImage>) -> bool {
    match (first, second) {
        (Some(first), Some(second)) => std::ptr::eq(first, second),
        (None, None) => true,
        _ => false,
    }
}

#[derive(PartialEq)]
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

impl PartialEq for PreviewModel {
    fn eq(&self, other: &Self) -> bool {
        self.message == other.message && same_image(self.image.as_deref(), other.image.as_deref())
    }
}

#[derive(PartialEq)]
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
        // FullScreenAuxiliary alone left the switcher invisible over another app's fullscreen
        // space; joining that space takes CanJoinAllApplications.
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::CanJoinAllApplications
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

    /// The largest panel the cursor's display takes.
    #[must_use]
    pub fn max_size(&self) -> (f64, f64) {
        cursor_screen(self.mtm).map_or((1000.0, 700.0), |screen| {
            let area = screen.visibleFrame().size;
            ((area.width * 0.9).round(), (area.height * 0.8).round())
        })
    }

    /// Shows the panel at `size`, centered on the cursor's display.
    pub fn show(&self, size: (f64, f64)) {
        if let Some(screen) = cursor_screen(self.mtm) {
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
        // The last frame holds the preview capture and the app icons; the next session draws its
        // own, so nothing needs them while the panel is hidden.
        *self.view.ivars().model.borrow_mut() = None;
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
        self.panel.isVisible() && frame_contains(self.panel.frame(), NSEvent::mouseLocation())
    }

    pub fn present(&self, model: FrameModel) {
        let mut shown = self.view.ivars().model.borrow_mut();
        // A refresh that changes nothing on screen, such as the list arriving while the panel
        // shows, redraws nothing.
        if shown.as_ref() == Some(&model) {
            return;
        }
        *shown = Some(model);
        drop(shown);
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
        string.drawInRect_withAttributes(ns_rect(centered), Some(&attributes));
    }
}

fn fill_rounded(rect: Rect, radius: f64, color: &NSColor) {
    color.setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(rect), radius, radius).fill();
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
        ns_rect(inset),
        (radius - 0.5).max(0.0),
        (radius - 0.5).max(0.0),
    );
    path.setLineWidth(1.0);
    path.stroke();
}

fn ns_rect(rect: Rect) -> NSRect {
    NSRect::new(
        NSPoint::new(rect.left, rect.top),
        NSSize::new(rect.width, rect.height),
    )
}

/// Where `size` lands when aspect-fitted and centered in `bounds`.
fn fitted(size: NSSize, bounds: Rect) -> Option<Rect> {
    let rect = fit(bounds, Size::new(size.width, size.height));
    (!rect.is_empty()).then_some(rect)
}

/// Draws `image` aspect-fitted into `bounds` and returns where it landed.
fn draw_image_fit(image: &NSImage, bounds: Rect) -> Option<Rect> {
    let rect = fitted(image.size(), bounds)?;
    unsafe {
        // SAFETY: drawing happens inside drawRect: with a current graphics context.
        image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
            ns_rect(rect),
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
        if let Some(rect) = fitted(image.size(), image_area(area)) {
            let radius = PLATE_RADIUS - PREVIEW_INSET;
            NSGraphicsContext::saveGraphicsState_class();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(rect), radius, radius)
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

    fn frame(title: &str) -> FrameModel {
        FrameModel {
            layout: Layout::new(1, 1, true, SCREEN),
            tokens: SwitcherTokens::new(ResolvedTheme::Dark),
            tiles: vec![Tile {
                name: "App".to_owned(),
                icon: None,
                selected: true,
            }],
            rows: vec![Row {
                number: Some(1),
                title: title.to_owned(),
                state: WindowState::Normal,
                selected: true,
            }],
            empty_note: None,
            more_note: None,
            close_state: CloseButtonVisualState::Normal,
            preview: Some(PreviewModel {
                image: None,
                message: None,
            }),
        }
    }

    #[test]
    fn a_frame_with_the_same_content_is_equal_and_a_new_title_is_not() {
        assert!(frame("Doc") == frame("Doc"));
        assert!(frame("Doc") != frame("Sheet"));
    }

    #[test]
    fn menu_tags_name_the_commands() {
        assert_eq!(command_for_tag(1), Some(WindowCommand::Close));
        assert_eq!(command_for_tag(5), Some(WindowCommand::Terminate));
        assert_eq!(command_for_tag(0), None);
    }
}
