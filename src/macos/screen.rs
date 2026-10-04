//! The display under the cursor, in `AppKit`'s global coordinates: the origin at the primary
//! display's bottom-left corner, y going up.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSEvent, NSScreen};
use objc2_foundation::{NSArray, NSPoint, NSRect};

/// Whether `point` lies in `frame`. The right and top edges are left out, so two displays
/// that touch never both claim a point.
pub fn frame_contains(frame: NSRect, point: NSPoint) -> bool {
    point.x >= frame.origin.x
        && point.x < frame.origin.x + frame.size.width
        && point.y >= frame.origin.y
        && point.y < frame.origin.y + frame.size.height
}

fn under_cursor(screens: &NSArray<NSScreen>) -> Option<Retained<NSScreen>> {
    let location = NSEvent::mouseLocation();
    screens
        .iter()
        .find(|screen| frame_contains(screen.frame(), location))
}

/// The display under the cursor, or the main display when the cursor is between displays.
pub fn cursor_screen(mtm: MainThreadMarker) -> Option<Retained<NSScreen>> {
    under_cursor(&NSScreen::screens(mtm)).or_else(|| NSScreen::mainScreen(mtm))
}

/// Bounds of the display under the cursor in top-left window-list coordinates.
pub fn cursor_display_bounds(mtm: MainThreadMarker) -> Option<[f64; 4]> {
    let screens = NSScreen::screens(mtm);
    let primary_height = screens.iter().next()?.frame().size.height;
    let frame = under_cursor(&screens)?.frame();
    Some([
        frame.origin.x,
        primary_height - (frame.origin.y + frame.size.height),
        frame.size.width,
        frame.size.height,
    ])
}
