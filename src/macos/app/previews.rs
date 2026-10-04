//! The selected window's preview: captures, the ones kept this session, and Screen Recording.

use super::App;
use crate::macos::overlay::PreviewModel;
use crate::macos::permissions;
use crate::macos::preview::{Capture, CaptureRequest, PreviewResult};
use crate::macos::runtime::MainThreadValue;
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_app_kit::NSImage;
use objc2_foundation::NSSize;
use objc2_screen_capture_kit::SCShareableContent;

// Each kept capture holds about a megabyte and a half.
const PREVIEWS_KEPT: usize = 8;

pub(super) enum Preview {
    Image(Retained<NSImage>),
    Unavailable(&'static str),
}

impl App {
    fn selected_window_id(&self) -> Option<u32> {
        self.switcher
            .selected_window()
            .and_then(|handle| u32::try_from(handle).ok())
    }

    fn kept_preview(&self, window_id: u32) -> Option<&Preview> {
        self.previews
            .iter()
            .find(|(id, _)| *id == window_id)
            .map(|(_, preview)| preview)
    }

    fn keep_preview(&mut self, window_id: u32, preview: Preview) {
        self.previews.retain(|(id, _)| *id != window_id);
        if self.previews.len() >= PREVIEWS_KEPT {
            self.previews.remove(0);
        }
        self.previews.push((window_id, preview));
    }

    /// Forgets the session's captures; one still running belongs to the session that is over.
    pub(super) fn reset_previews(&mut self) {
        self.session = self.session.wrapping_add(1);
        self.previews.clear();
        self.preview_in_flight = None;
    }

    /// The selected window's capture, or why there is none.
    pub(super) fn preview_model(&self) -> PreviewModel {
        let window = self.selected_window_id();
        match window.and_then(|id| self.kept_preview(id)) {
            Some(Preview::Image(image)) => PreviewModel {
                image: Some(image.clone()),
                message: None,
            },
            Some(Preview::Unavailable(message)) => PreviewModel {
                image: None,
                message: Some((*message).to_owned()),
            },
            None => PreviewModel {
                image: None,
                message: (window.is_some() && !permissions::screen_recording_granted()).then(
                    || "Allow Screen Recording in System Settings to see previews".to_owned(),
                ),
            },
        }
    }

    /// Captures the selected window unless this session has it already.
    pub(super) fn request_preview_capture(&mut self) {
        if self.preview_in_flight.is_some()
            || !self.switcher.is_active()
            || !self.settings.appearance.preview
        {
            return;
        }
        let Some(overlay) = self.overlay.clone() else {
            return;
        };
        let Some(window_id) = self.selected_window_id() else {
            return;
        };
        if self.kept_preview(window_id).is_some() {
            return;
        }
        let Some((width, height)) = self.shown.and_then(|shown| shown.layout.preview_size()) else {
            return;
        };
        if !permissions::screen_recording_granted() {
            return;
        }
        let scale = overlay.backing_scale();
        let window_size = self
            .records
            .iter()
            .find(|record| record.window_id == window_id && record.is_on_screen)
            .map(|record| (record.bounds[2], record.bounds[3]))
            .filter(|(width, height)| *width > 0.0 && *height > 0.0);
        let request = CaptureRequest {
            window_id,
            pixel_width: pixel_length(width, scale),
            pixel_height: pixel_length(height, scale),
            window_size,
            session: self.session,
        };
        match self.preview.capture(&request) {
            Capture::Started => self.preview_in_flight = Some(window_id),
            Capture::Waiting => {}
            Capture::NotListed => {
                self.keep_preview(
                    window_id,
                    Preview::Unavailable("This window cannot be captured"),
                );
                self.redraw();
            }
        }
    }

    pub(crate) fn preview_content_ready(
        &mut self,
        content: MainThreadValue<Option<Retained<SCShareableContent>>>,
    ) {
        self.preview.set_content(content.0);
        if !self.settings.appearance.preview {
            self.preview.clear();
            return;
        }
        self.request_preview_capture();
    }

    pub(crate) fn preview_captured(
        &mut self,
        session: u64,
        window_id: u32,
        result: MainThreadValue<PreviewResult>,
    ) {
        if session != self.session {
            return;
        }
        self.preview_in_flight = None;
        let preview = match result.0 {
            PreviewResult::Image(image) => Preview::Image(NSImage::initWithCGImage_size(
                NSImage::alloc(),
                &image,
                NSSize::ZERO,
            )),
            PreviewResult::Unavailable(message) => Preview::Unavailable(message),
        };
        self.keep_preview(window_id, preview);
        if self.selected_window_id() == Some(window_id) {
            self.redraw();
        } else {
            self.request_preview_capture();
        }
    }

    /// Previews are the only thing that needs Screen Recording, so the system prompt comes with
    /// them: after the first switch that showed the preview area, and when they are turned on.
    pub(super) fn ask_for_screen_recording(&mut self) {
        if permissions::screen_recording_granted() {
            self.preview.refresh_content();
            return;
        }
        self.screen_recording_asked = true;
        if !permissions::request_screen_recording() {
            eprintln!("Screen Recording is not granted; previews stay blank until it is allowed.");
        }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "preview areas are small positive point sizes"
)]
fn pixel_length(points: f64, scale: f64) -> usize {
    (points * scale).round().max(1.0) as usize
}
