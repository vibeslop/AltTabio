//! Preview frames through `ScreenCaptureKit` screenshots.
//!
//! One `SCScreenshotManager` capture per window is enough for the switcher's preview: it needs no
//! stream lifecycle, costs nothing while the overlay is hidden, and yields a frame within tens of
//! milliseconds after the selection moves. Capturing the selected window again every 150 ms kept
//! the window server and `replayd` busy for a picture that barely changes while the panel shows.

use super::runtime::{MainThreadValue, post_to_app};
use block2::RcBlock;
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_core_graphics::CGImage;
use objc2_foundation::NSError;
use objc2_screen_capture_kit::{
    SCCaptureResolutionType, SCContentFilter, SCScreenshotManager, SCShareableContent,
    SCStreamConfiguration,
};

pub enum PreviewResult {
    Image(objc2_core_foundation::CFRetained<CGImage>),
    Unavailable(&'static str),
}

pub struct CaptureRequest {
    pub window_id: u32,
    /// Pixel size of the preview area; the capture is fitted into it with its aspect ratio kept.
    pub pixel_width: usize,
    pub pixel_height: usize,
    /// The window's size in points from the latest window list, which is fresher than the
    /// frame `ScreenCaptureKit` reported when its own list was fetched.
    pub window_size: Option<(f64, f64)>,
    /// The switcher session asking, so a capture that ends after its session is dropped.
    pub session: u64,
}

/// What asking for a capture did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Capture {
    Started,
    /// The window list is loading; the app asks again from `preview_content_ready`.
    Waiting,
    /// The window list, asked for during this session, does not have the window.
    NotListed,
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
    // Whether this session asked for the window list already. Fetching it costs the window
    // server more than a capture, so it is fetched only for a window it lacks, once a session;
    // a list that failed to load is not asked for again until the next one.
    fetched: bool,
}

impl PreviewSource {
    /// Asks `ScreenCaptureKit` for the current window list; the app receives it through
    /// `preview_content_ready`.
    pub fn refresh_content(&mut self) {
        if self.fetching {
            return;
        }
        self.fetching = true;
        self.fetched = true;
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

    /// Lets the session that starts now fetch the window list once.
    pub fn start_session(&mut self) {
        self.fetched = false;
    }

    /// Lets go of the window list while previews are off.
    pub fn clear(&mut self) {
        self.content = None;
    }

    /// Starts one capture; the app receives the frame through `preview_captured`.
    pub fn capture(&mut self, request: &CaptureRequest) -> Capture {
        let window = self.content.as_ref().and_then(|content| {
            let windows = unsafe {
                // SAFETY: the content object is immutable once delivered.
                content.windows()
            };
            windows.iter().find(|window| unsafe {
                // SAFETY: SCWindow properties are plain immutable values.
                window.windowID() == request.window_id
            })
        });
        let Some(window) = window else {
            if self.fetching {
                return Capture::Waiting;
            }
            if self.fetched {
                return Capture::NotListed;
            }
            // The window may have opened since the list was fetched.
            self.refresh_content();
            return Capture::Waiting;
        };
        let content_size = request.window_size.unwrap_or_else(|| {
            let frame = unsafe {
                // SAFETY: SCWindow properties are plain immutable values.
                window.frame()
            };
            (frame.size.width, frame.size.height)
        });
        let filter = unsafe {
            // SAFETY: the window is a live object for the initializer.
            SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window)
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
        let session = request.session;
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
            post_to_app(move |app| app.preview_captured(session, window_id, result));
        });
        unsafe {
            // SAFETY: the filter, configuration, and retained block stay valid for the call.
            SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                &filter,
                &configuration,
                Some(&handler),
            );
        }
        Capture::Started
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
