//! Live preview frames through `ScreenCaptureKit` screenshots.
//!
//! A repeated `SCScreenshotManager` capture is enough for the switcher's preview: it needs no
//! stream lifecycle, costs nothing while the overlay is hidden, and yields a frame within tens of
//! milliseconds after the selection moves.

use super::{MainThreadValue, post_to_app};
use block2::RcBlock;
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCCaptureResolutionType, SCContentFilter, SCScreenshotManager, SCShareableContent,
    SCStreamConfiguration,
};

pub enum PreviewResult {
    Image(objc2_core_foundation::CFRetained<CGImage>),
    Unavailable(&'static str),
}

/// What a capture is for, which decides where the app files the frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureKind {
    /// The large preview of the selected window.
    Preview,
    /// A row thumbnail in icon mode.
    Thumbnail,
}

pub struct CaptureRequest {
    pub window_id: u32,
    pub kind: CaptureKind,
    pub full_desktop: bool,
    /// Pixel size of the preview area; the capture is fitted into it with its aspect ratio kept.
    pub pixel_width: usize,
    pub pixel_height: usize,
}

/// The largest size with `content`'s aspect ratio that fits the preview area.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "preview areas and window frames are small positive sizes"
)]
pub fn fitted_size(area: (usize, usize), content: (f64, f64)) -> (usize, usize) {
    if content.0 <= 0.0 || content.1 <= 0.0 || area.0 == 0 || area.1 == 0 {
        return (area.0.max(1), area.1.max(1));
    }
    let scale = (area.0 as f64 / content.0).min(area.1 as f64 / content.1);
    (
        (content.0 * scale).round().max(1.0) as usize,
        (content.1 * scale).round().max(1.0) as usize,
    )
}

#[derive(Default)]
pub struct PreviewSource {
    content: Option<Retained<SCShareableContent>>,
    fetching: bool,
}

impl PreviewSource {
    /// Asks `ScreenCaptureKit` for the current window list; the app receives it through
    /// `preview_content_ready`.
    pub fn refresh_content(&mut self) {
        if self.fetching {
            return;
        }
        self.fetching = true;
        let handler = RcBlock::new(
            move |content: *mut SCShareableContent, _error: *mut NSError| {
                let content = if content.is_null() {
                    None
                } else {
                    unsafe {
                        // SAFETY: ScreenCaptureKit hands over a live object for the callback;
                        // retaining it keeps it valid after the block returns.
                        Retained::retain(content)
                    }
                };
                let content = MainThreadValue(content);
                post_to_app(move |app| app.preview_content_ready(content));
            },
        );
        unsafe {
            // SAFETY: the completion block is retained by ScreenCaptureKit until it runs.
            SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
                true, false, &handler,
            );
        }
    }

    pub fn set_content(&mut self, content: Option<Retained<SCShareableContent>>) {
        self.fetching = false;
        if content.is_some() {
            self.content = content;
        }
    }

    #[must_use]
    pub fn has_content(&self) -> bool {
        self.content.is_some()
    }

    /// Starts one capture; the app receives the frame through `preview_captured`.
    pub fn capture(&self, request: &CaptureRequest) -> Result<(), &'static str> {
        let Some(content) = &self.content else {
            return Err("Preview is loading");
        };
        let windows = unsafe {
            // SAFETY: the content object is immutable once delivered.
            content.windows()
        };
        let Some(window) = windows.iter().find(|window| unsafe {
            // SAFETY: SCWindow properties are plain immutable values.
            window.windowID() == request.window_id
        }) else {
            return Err("This window cannot be captured");
        };
        let window_frame = unsafe {
            // SAFETY: SCWindow properties are plain immutable values.
            window.frame()
        };
        let mut content_size = (window_frame.size.width, window_frame.size.height);
        let filter = if request.full_desktop {
            let frame = window_frame;
            let displays = unsafe {
                // SAFETY: the content object is immutable once delivered.
                content.displays()
            };
            let center_x = frame.origin.x + frame.size.width / 2.0;
            let center_y = frame.origin.y + frame.size.height / 2.0;
            let display = displays
                .iter()
                .find(|display| {
                    let bounds = unsafe {
                        // SAFETY: SCDisplay properties are plain immutable values.
                        display.frame()
                    };
                    center_x >= bounds.origin.x
                        && center_x < bounds.origin.x + bounds.size.width
                        && center_y >= bounds.origin.y
                        && center_y < bounds.origin.y + bounds.size.height
                })
                .or_else(|| displays.iter().next());
            let Some(display) = display else {
                return Err("No display is available for the desktop preview");
            };
            let display_frame = unsafe {
                // SAFETY: SCDisplay properties are plain immutable values.
                display.frame()
            };
            content_size = (display_frame.size.width, display_frame.size.height);
            unsafe {
                // SAFETY: both arguments are live objects for the initializer.
                SCContentFilter::initWithDisplay_includingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::from_retained_slice(&[window]),
                )
            }
        } else {
            unsafe {
                // SAFETY: the window is a live object for the initializer.
                SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window)
            }
        };
        let (width, height) =
            fitted_size((request.pixel_width, request.pixel_height), content_size);
        let configuration = unsafe {
            // SAFETY: a fresh configuration has no preconditions; all setters take plain values.
            let configuration = SCStreamConfiguration::new();
            configuration.setWidth(width);
            configuration.setHeight(height);
            configuration.setShowsCursor(false);
            configuration.setIgnoreShadowsSingleWindow(true);
            configuration.setCaptureResolution(SCCaptureResolutionType::Best);
            configuration
        };
        let window_id = request.window_id;
        let kind = request.kind;
        let handler = RcBlock::new(move |image: *mut CGImage, _error: *mut NSError| {
            let result = if image.is_null() {
                PreviewResult::Unavailable("Preview is not available for this window")
            } else {
                let image = unsafe {
                    // SAFETY: the callback receives a live image; retaining it keeps it valid.
                    objc2_core_foundation::CFRetained::retain(std::ptr::NonNull::new_unchecked(
                        image,
                    ))
                };
                PreviewResult::Image(image)
            };
            let result = MainThreadValue(result);
            post_to_app(move |app| match kind {
                CaptureKind::Preview => app.preview_captured(window_id, result),
                CaptureKind::Thumbnail => app.thumbnail_captured(window_id, result),
            });
        });
        unsafe {
            // SAFETY: the filter, configuration, and retained block stay valid for the call.
            SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                &filter,
                &configuration,
                Some(&handler),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::fitted_size;

    #[test]
    fn captures_are_fitted_into_the_preview_area_without_distortion() {
        assert_eq!(fitted_size((800, 600), (1600.0, 900.0)), (800, 450));
        assert_eq!(fitted_size((800, 600), (500.0, 1000.0)), (300, 600));
        assert_eq!(fitted_size((800, 600), (0.0, 10.0)), (800, 600));
        assert_eq!(fitted_size((0, 0), (10.0, 10.0)), (1, 1));
    }
}
