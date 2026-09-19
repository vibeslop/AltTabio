//! The switcher panel: a non-activating floating `NSPanel` over Liquid Glass with one custom view
//! that draws the numbered task list, the selected row's close button, and the live preview.

use alttabio::input::WindowCommand;
use alttabio::overlay_layout::OverlayLayout;
use alttabio::theme::{Rgb8, ThemePalette};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSBackingStoreType, NSBezierPath, NSColor, NSCompositingOperation, NSEvent, NSFont,
    NSFontAttributeName, NSFontWeightMedium, NSFontWeightRegular, NSForegroundColorAttributeName,
    NSGlassEffectView, NSGlassEffectViewStyle, NSImage, NSLineBreakMode, NSMenu, NSMenuItem,
    NSMutableParagraphStyle, NSPanel, NSParagraphStyleAttributeName, NSPopUpMenuWindowLevel,
    NSScreen, NSStringDrawing, NSTextAlignment, NSTrackingArea, NSTrackingAreaOptions, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{
    NSAttributedStringKey, NSDictionary, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

// The overlay takes five eighths of the display like the Windows build.
const OVERLAY_FRACTION: f64 = 5.0 / 8.0;
const CORNER_RADIUS: f64 = 16.0;

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

pub struct RowModel {
    pub position: usize,
    pub title: String,
    pub app_name: String,
    pub icon: Option<Retained<NSImage>>,
    pub selected: bool,
}

pub struct FrameModel {
    pub rows: Vec<RowModel>,
    pub layout: OverlayLayout,
    pub options: RenderOptions,
    pub palette: ThemePalette,
    pub close_state: CloseButtonVisualState,
    pub preview: Option<Retained<NSImage>>,
    pub preview_message: Option<String>,
    pub filter: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Hit {
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
pub fn preview_rect(size: (f64, f64), layout: OverlayLayout) -> Rect {
    let padding = f64::from(layout.outer_padding);
    let left = list_width(size, layout) + padding * 2.0 + 1.0;
    Rect {
        left,
        top: padding,
        width: (size.0 - padding - left).max(0.0),
        height: (size.1 - padding * 2.0).max(0.0),
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
    x: f64,
    y: f64,
) -> Option<Hit> {
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

    pub fn set_palette(&self, palette: ThemePalette) {
        if let Some(glass) = &self.glass {
            // A translucent tint keeps AltTabio's own dark or light surface while the glass
            // still refracts whatever sits behind the switcher.
            glass.setTintColor(Some(&color(palette.background, 0.62)));
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
        for (tag, title, function_key) in [
            (1, "Close", 4_u32),
            (2, "Minimize", 5),
            (3, "Maximize", 6),
            (4, "Restore", 7),
            (5, "Terminate", 8),
            (6, "Run", 9),
        ] {
            let key = char::from_u32(0xF704 + function_key - 1)
                .map(|value| value.to_string())
                .unwrap_or_default();
            let item = unsafe {
                // SAFETY: the selector exists on ContextMenuTarget with a matching signature.
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm),
                    &NSString::from_str(title),
                    Some(sel!(chooseCommand:)),
                    &NSString::from_str(&key),
                )
            };
            item.setKeyEquivalentModifierMask(objc2_app_kit::NSEventModifierFlags::empty());
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
    detail: Retained<NSFont>,
    number: Retained<NSFont>,
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
            detail: NSFont::systemFontOfSize_weight(detail, NSFontWeightRegular),
            number: NSFont::monospacedDigitSystemFontOfSize_weight(number, NSFontWeightMedium),
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
    let rect = Rect {
        left: bounds.left,
        top: bounds.top + (bounds.height - height) / 2.0,
        width: bounds.width,
        height,
    };
    unsafe {
        // SAFETY: drawing happens inside drawRect: with a current graphics context.
        string.drawInRect_withAttributes(rect.ns(), Some(&attributes));
    }
}

fn fill_rounded(rect: Rect, radius: f64, color: &NSColor) {
    color.setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect.ns(), radius, radius).fill();
}

fn stroke_rounded(rect: Rect, radius: f64, width: f64, color: &NSColor) {
    color.setStroke();
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect.ns(), radius, radius);
    path.setLineWidth(width);
    path.stroke();
}

fn draw_image_fit(image: &NSImage, bounds: Rect) {
    let size = image.size();
    if size.width <= 0.0 || size.height <= 0.0 || bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
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
}

#[allow(
    clippy::too_many_lines,
    reason = "one bounded pass draws the whole frame; splitting it would scatter the geometry"
)]
fn draw_frame(bounds: NSRect, model: &FrameModel) {
    let size = (bounds.size.width, bounds.size.height);
    let layout = model.layout;
    let palette = model.palette;
    let padding = f64::from(layout.outer_padding);
    let list_right = list_width(size, layout);
    let fonts = fonts(model.options.compact_list);
    let primary = color(palette.primary, 1.0);
    let secondary = color(palette.secondary, 1.0);

    if model.options.visible_borders {
        stroke_rounded(
            Rect {
                left: 1.0,
                top: 1.0,
                width: size.0 - 2.0,
                height: size.1 - 2.0,
            },
            CORNER_RADIUS - 1.0,
            2.0,
            &color(palette.window_border, 1.0),
        );
    }

    if model.options.preview {
        let preview = preview_rect(size, layout);
        fill_rounded(preview, 6.0, &color(palette.divider, 0.35));
        if let Some(image) = &model.preview {
            let inset = Rect {
                left: preview.left + 1.0,
                top: preview.top + 1.0,
                width: (preview.width - 2.0).max(0.0),
                height: (preview.height - 2.0).max(0.0),
            };
            draw_image_fit(image, inset);
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
                &secondary,
                NSTextAlignment::Center,
            );
        }
        if model.options.visible_borders {
            stroke_rounded(preview, 6.0, 1.0, &color(palette.preview_border, 1.0));
        }
        if !model.filter.is_empty() {
            let text = format!("Search: {}", model.filter);
            let attributes = text_attributes(&fonts.detail, &secondary, NSTextAlignment::Left);
            let measured = unsafe {
                // SAFETY: the attributes dictionary is live for the synchronous measurement.
                NSString::from_str(&text).sizeWithAttributes(Some(&attributes))
            };
            let pill = Rect {
                left: preview.left + 10.0,
                top: preview.top + 10.0,
                width: measured.width + 20.0,
                height: measured.height + 8.0,
            };
            fill_rounded(pill, 6.0, &color(palette.selected, 0.9));
            draw_text(
                &text,
                Rect {
                    left: pill.left + 10.0,
                    top: pill.top,
                    width: pill.width - 20.0,
                    height: pill.height,
                },
                &fonts.detail,
                &primary,
                NSTextAlignment::Left,
            );
        }
    }

    color(palette.divider, 1.0).setFill();
    NSBezierPath::fillRect(
        Rect {
            left: list_right + padding,
            top: padding,
            width: 1.0,
            height: size.1 - padding * 2.0,
        }
        .ns(),
    );

    let icon_size = f64::from(if model.options.large_icons {
        layout.large_icon_size
    } else {
        layout.small_icon_size
    });
    for (row, item) in model.rows.iter().enumerate() {
        let bounds = row_rect(size, layout, row);
        if bounds.top + bounds.height > size.1 - padding + 0.5 {
            break;
        }
        if item.selected {
            fill_rounded(
                bounds,
                f64::from(layout.selection_radius),
                &color(palette.selected, 1.0),
            );
        }
        let mut left = bounds.left;
        if model.options.show_numbers {
            draw_text(
                &item.position.to_string(),
                Rect {
                    left,
                    top: bounds.top,
                    width: f64::from(layout.number_width),
                    height: bounds.height,
                },
                &fonts.number,
                &color(palette.number, 1.0),
                NSTextAlignment::Center,
            );
            left += f64::from(layout.number_width);
        }
        if let Some(icon) = &item.icon {
            let slot = f64::from(layout.icon_slot_width);
            draw_image_fit(
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
        let text_right = close.map_or(bounds.left + bounds.width - 12.0, |button| {
            button.left - f64::from(layout.close_button_gap)
        });
        let text_width = (text_right - left).max(0.0);
        if model.options.show_app_names {
            let (title_top, title_bottom, name_top, name_bottom) = if model.options.compact_list {
                (1.0, 26.0, 22.0, bounds.height - 1.0)
            } else {
                (3.0, 35.0, 31.0, bounds.height - 2.0)
            };
            draw_text(
                &item.title,
                Rect {
                    left,
                    top: bounds.top + title_top,
                    width: text_width,
                    height: title_bottom - title_top,
                },
                &fonts.title,
                &primary,
                NSTextAlignment::Left,
            );
            draw_text(
                &item.app_name,
                Rect {
                    left,
                    top: bounds.top + name_top,
                    width: text_width,
                    height: name_bottom - name_top,
                },
                &fonts.detail,
                &secondary,
                NSTextAlignment::Left,
            );
        } else {
            draw_text(
                &item.title,
                Rect {
                    left,
                    top: bounds.top,
                    width: text_width,
                    height: bounds.height,
                },
                &fonts.title,
                &primary,
                NSTextAlignment::Left,
            );
        }
        if let Some(button) = close {
            let background = match model.close_state {
                CloseButtonVisualState::Normal => None,
                CloseButtonVisualState::Hovered => Some(color(palette.close_hover, 1.0)),
                CloseButtonVisualState::Pressed => Some(color(palette.close_pressed, 1.0)),
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
            primary.setStroke();
            let path = NSBezierPath::bezierPath();
            path.setLineWidth(1.5);
            path.moveToPoint(NSPoint::new(left, top));
            path.lineToPoint(NSPoint::new(left + glyph, top + glyph));
            path.moveToPoint(NSPoint::new(left + glyph, top));
            path.lineToPoint(NSPoint::new(left, top + glyph));
            path.stroke();
        }
    }
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
            hit_test(size, layout, Some(1), inside_close.0, inside_close.1),
            Some(Hit::CloseButton(1))
        );
        assert_eq!(
            hit_test(size, layout, Some(0), inside_close.0, inside_close.1),
            Some(Hit::Row(1))
        );
        assert_eq!(hit_test(size, layout, None, 5.0, row.top + 5.0), None);
        assert_eq!(
            hit_test(
                size,
                layout,
                None,
                row.left + 5.0,
                row.top + row.height + 1.0
            ),
            None
        );
    }

    #[test]
    fn preview_sits_right_of_the_divider() {
        let layout = for_compact_list(true);
        let size = (1000.0, 600.0);
        let preview = preview_rect(size, layout);

        assert!(preview.left > list_width(size, layout));
        assert!((preview.left + preview.width - (size.0 - 18.0)).abs() < f64::EPSILON);
        assert!((preview.top - 18.0).abs() < f64::EPSILON);
    }
}
