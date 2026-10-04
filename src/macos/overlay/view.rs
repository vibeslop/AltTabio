//! The view that draws the panel and turns pointer input into `ViewEvent`s.

use super::draw::draw_frame;
use super::{FrameModel, ViewEvent, ViewHandler};
use objc2::rc::Retained;
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
};
use objc2_app_kit::{NSEvent, NSTrackingArea, NSTrackingAreaOptions, NSView};
use objc2_foundation::{NSObjectProtocol, NSRect};
use std::cell::{Cell, RefCell};

/// Scrolled points on a trackpad per selection step; a mouse wheel notch is always one step.
const SCROLL_STEP: f64 = 24.0;

pub struct SwitcherViewIvars {
    pub(super) model: RefCell<Option<FrameModel>>,
    pub(super) handler: RefCell<Option<ViewHandler>>,
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
    pub(super) fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
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
