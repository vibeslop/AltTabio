//! The switcher panel: a non-activating floating `NSPanel` over Liquid Glass with one custom view
//! that draws a strip of app icons, the selected app's windows under it, and, when previews are
//! on, the selected window beside them.

mod draw;
mod menu;
mod view;

use super::screen::{cursor_screen, frame_contains};
use alttabio::close_button::CloseButtonVisualState;
use alttabio::input::WindowCommand;
use alttabio::panel_layout::{CORNER_RADIUS, Layout, WindowState};
use alttabio::theme::{ResolvedTheme, SwitcherTokens};
use draw::rgba;
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSBackingStoreType, NSColor, NSEvent, NSGlassEffectView, NSGlassEffectViewStyle, NSImage,
    NSPanel, NSPopUpMenuWindowLevel, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindowAnimationBehavior, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::rc::Rc;
use view::SwitcherView;

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

    /// Runs the command menu synchronously; call it outside any app-state borrow.
    #[must_use]
    pub fn show_context_menu(
        &self,
        x: f64,
        y: f64,
        window: bool,
        app_name: &str,
    ) -> Option<WindowCommand> {
        menu::show(self.mtm, &self.view, NSPoint::new(x, y), window, app_name)
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
}
