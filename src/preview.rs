use alttabio::overlay_layout::{for_compact_list, layout_scale};
use alttabio::preview_layout::{Rect as LayoutRect, Size as LayoutSize, calculate};
use std::mem::size_of;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{
    DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE,
    DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE, DWMWA_EXTENDED_FRAME_BOUNDS,
    DwmGetWindowAttribute, DwmQueryThumbnailSourceSize, DwmRegisterThumbnail,
    DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowPlacement, GetWindowRect, IsIconic, WINDOWPLACEMENT,
};
use windows::core::Result;

pub struct DwmPreview {
    destination: HWND,
    source: HWND,
    thumbnail: Option<isize>,
    frame: Option<RECT>,
    full_desktop: bool,
    compact_list: bool,
}

impl DwmPreview {
    pub fn new(destination: HWND, full_desktop: bool, compact_list: bool) -> Self {
        Self {
            destination,
            source: HWND::default(),
            thumbnail: None,
            frame: None,
            full_desktop,
            compact_list,
        }
    }

    pub fn set_source(&mut self, source: Option<HWND>) -> Result<()> {
        let source = source.unwrap_or_default();
        if source == self.source && self.thumbnail.is_some() {
            return self.update();
        }

        self.unregister();
        self.source = source;
        if source == HWND::default() || source == self.destination {
            return Ok(());
        }
        let thumbnail = unsafe {
            // SAFETY: both HWND values are borrowed live windows; the returned registration is
            // uniquely owned by this DwmPreview until unregister or drop.
            DwmRegisterThumbnail(self.destination, source)
        }?;
        self.thumbnail = Some(thumbnail);
        self.update()
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "the list edge is clamped to the physical on-screen client width"
    )]
    pub fn update(&mut self) -> Result<()> {
        self.frame = None;
        let Some(thumbnail) = self.thumbnail else {
            return Ok(());
        };
        let source_size = unsafe {
            // SAFETY: the thumbnail registration is live and uniquely owned by this DwmPreview.
            DwmQueryThumbnailSourceSize(thumbnail)
        }?;
        let mut client = RECT::default();
        unsafe {
            // SAFETY: `client` is writable and destination is the live overlay HWND.
            GetClientRect(self.destination, &raw mut client)?;
        }

        let width = client.right.saturating_sub(client.left);
        let height = client.bottom.saturating_sub(client.top);
        let padding = (width.min(height) / 32).max(12);
        let window_dpi = unsafe {
            // SAFETY: destination is the live overlay window and the call returns a scalar DPI.
            GetDpiForWindow(self.destination)
        };
        let scale = layout_scale(window_dpi);
        let layout = for_compact_list(self.compact_list);
        let logical_width = width as f32 / scale;
        let list_right = (layout.list_width(logical_width, scale) * scale).round() as i32;
        let host = RECT {
            left: list_right + (padding * 2),
            top: padding,
            right: width - padding,
            bottom: height - padding,
        };
        let border_width = (scale.round() as i32).max(1);
        let content_host = inset_rectangle(host, border_width);
        let placement = if self.full_desktop {
            desktop_preview_layout(self.source, content_host, source_size.cx, source_size.cy)
                .unwrap_or_else(|| {
                    let destination = fit_rectangle(content_host, source_size.cx, source_size.cy);
                    PreviewPlacement::new(destination, destination, RECT::default(), false)
                })
        } else {
            window_preview_layout(content_host, source_size.cx, source_size.cy)
        };
        let destination = placement.thumbnail_destination;
        if destination.right <= destination.left || destination.bottom <= destination.top {
            return Ok(());
        }

        let mut flags = DWM_TNP_VISIBLE
            | DWM_TNP_RECTDESTINATION
            | DWM_TNP_OPACITY
            | DWM_TNP_SOURCECLIENTAREAONLY;
        if placement.source.right > placement.source.left
            && placement.source.bottom > placement.source.top
        {
            flags |= DWM_TNP_RECTSOURCE;
        }
        let properties = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: flags,
            rcDestination: destination,
            rcSource: placement.source,
            opacity: 255,
            fVisible: true.into(),
            fSourceClientAreaOnly: placement.source_client_area_only.into(),
        };
        unsafe {
            // SAFETY: the registration is live and `properties` remains valid for the synchronous
            // update call.
            DwmUpdateThumbnailProperties(thumbnail, &raw const properties)?;
        }
        self.frame = preview_background_frame(placement, self.full_desktop, border_width, host);
        Ok(())
    }

    pub const fn frame(&self) -> Option<RECT> {
        self.frame
    }

    pub fn clear(&mut self) {
        self.unregister();
        self.source = HWND::default();
    }

    fn unregister(&mut self) {
        self.frame = None;
        let Some(thumbnail) = self.thumbnail.take() else {
            return;
        };
        let result = unsafe {
            // SAFETY: this registration is uniquely owned and removed exactly once.
            DwmUnregisterThumbnail(thumbnail)
        };
        if let Err(error) = result {
            eprintln!("Could not unregister the DWM preview: {error}");
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PreviewPlacement {
    thumbnail_destination: RECT,
    frame_content: RECT,
    source: RECT,
    source_client_area_only: bool,
}

impl PreviewPlacement {
    const fn new(
        thumbnail_destination: RECT,
        frame_content: RECT,
        source: RECT,
        source_client_area_only: bool,
    ) -> Self {
        Self {
            thumbnail_destination,
            frame_content,
            source,
            source_client_area_only,
        }
    }
}

impl Drop for DwmPreview {
    fn drop(&mut self) {
        self.unregister();
    }
}

fn desktop_preview_layout(
    source: HWND,
    host: RECT,
    source_width: i32,
    source_height: i32,
) -> Option<PreviewPlacement> {
    let restored = restored_window_bounds(source);
    let monitor = if let Some(restored) = restored {
        unsafe {
            // SAFETY: restored is an initialized physical-pixel rectangle and the fallback flag
            // guarantees a monitor for off-screen saved placements.
            MonitorFromRect(&raw const restored, MONITOR_DEFAULTTONEAREST)
        }
    } else {
        unsafe {
            // SAFETY: source is a borrowed live HWND and the fallback flag requests the nearest
            // monitor if the window is not currently intersecting one.
            MonitorFromWindow(source, MONITOR_DEFAULTTONEAREST)
        }
    };
    if monitor.is_invalid() {
        return None;
    }
    let mut monitor_info = MONITORINFO {
        cbSize: u32::try_from(size_of::<MONITORINFO>()).ok()?,
        ..MONITORINFO::default()
    };
    let monitor_read = unsafe {
        // SAFETY: monitor_info has a correct cbSize and is writable for the synchronous call.
        GetMonitorInfoW(monitor, &raw mut monitor_info)
    };
    if !monitor_read.as_bool() {
        return None;
    }
    let window = source_window_bounds(source)?;
    let layout = calculate(
        to_layout_rect(host),
        to_layout_rect(monitor_info.rcMonitor),
        to_layout_rect(window),
        LayoutSize::new(f64::from(source_width), f64::from(source_height)),
        restored.map(to_layout_rect),
    );
    if layout.window_destination.is_empty() {
        return None;
    }
    Some(PreviewPlacement::new(
        to_native_rect(layout.window_destination),
        to_native_rect(layout.desktop_destination),
        to_native_rect(layout.window_source),
        false,
    ))
}

fn window_preview_layout(host: RECT, source_width: i32, source_height: i32) -> PreviewPlacement {
    let destination = fit_rectangle(host, source_width, source_height);
    PreviewPlacement::new(destination, destination, RECT::default(), false)
}

const fn inset_rectangle(rectangle: RECT, amount: i32) -> RECT {
    RECT {
        left: rectangle.left.saturating_add(amount),
        top: rectangle.top.saturating_add(amount),
        right: rectangle.right.saturating_sub(amount),
        bottom: rectangle.bottom.saturating_sub(amount),
    }
}

fn outset_rectangle(rectangle: RECT, amount: i32, bounds: RECT) -> RECT {
    RECT {
        left: rectangle.left.saturating_sub(amount).max(bounds.left),
        top: rectangle.top.saturating_sub(amount).max(bounds.top),
        right: rectangle.right.saturating_add(amount).min(bounds.right),
        bottom: rectangle.bottom.saturating_add(amount).min(bounds.bottom),
    }
}

fn preview_background_frame(
    placement: PreviewPlacement,
    full_desktop: bool,
    border_width: i32,
    host: RECT,
) -> Option<RECT> {
    full_desktop.then(|| outset_rectangle(placement.frame_content, border_width, host))
}

fn source_window_bounds(source: HWND) -> Option<RECT> {
    let mut bounds = RECT::default();
    let extended_frame = unsafe {
        // SAFETY: bounds is writable for its exact byte size and source is a borrowed live HWND.
        DwmGetWindowAttribute(
            source,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut bounds).cast(),
            u32::try_from(size_of::<RECT>()).ok()?,
        )
    };
    if extended_frame.is_ok() && valid_rect(bounds) {
        return Some(bounds);
    }
    unsafe {
        // SAFETY: bounds is writable and source is a borrowed live HWND.
        GetWindowRect(source, &raw mut bounds).ok()?;
    }
    valid_rect(bounds).then_some(bounds)
}

fn restored_window_bounds(source: HWND) -> Option<RECT> {
    let minimized = unsafe {
        // SAFETY: source is a borrowed live HWND and IsIconic has no pointer preconditions.
        IsIconic(source).as_bool()
    };
    if !minimized {
        return None;
    }
    let mut placement = WINDOWPLACEMENT {
        length: u32::try_from(size_of::<WINDOWPLACEMENT>()).ok()?,
        ..WINDOWPLACEMENT::default()
    };
    unsafe {
        // SAFETY: placement has a correct length and is writable for the synchronous call.
        GetWindowPlacement(source, &raw mut placement).ok()?;
    }
    valid_rect(placement.rcNormalPosition).then_some(placement.rcNormalPosition)
}

const fn valid_rect(rectangle: RECT) -> bool {
    rectangle.right > rectangle.left && rectangle.bottom > rectangle.top
}

fn to_layout_rect(rectangle: RECT) -> LayoutRect {
    LayoutRect::new(
        f64::from(rectangle.left),
        f64::from(rectangle.top),
        f64::from(rectangle.right.saturating_sub(rectangle.left)),
        f64::from(rectangle.bottom.saturating_sub(rectangle.top)),
    )
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "preview layout is clamped to physical on-screen Win32 rectangles"
)]
fn to_native_rect(rectangle: LayoutRect) -> RECT {
    RECT {
        left: rectangle.left.round() as i32,
        top: rectangle.top.round() as i32,
        right: rectangle.right().round() as i32,
        bottom: rectangle.bottom().round() as i32,
    }
}

fn fit_rectangle(bounds: RECT, source_width: i32, source_height: i32) -> RECT {
    let host_width = bounds.right.saturating_sub(bounds.left);
    let host_height = bounds.bottom.saturating_sub(bounds.top);
    if host_width <= 0 || host_height <= 0 || source_width <= 0 || source_height <= 0 {
        return RECT::default();
    }

    let host_width_64 = i64::from(host_width);
    let host_height_64 = i64::from(host_height);
    let source_width_64 = i64::from(source_width);
    let source_height_64 = i64::from(source_height);
    let (width, height) = if host_width_64 * source_height_64 <= host_height_64 * source_width_64 {
        (
            host_width,
            i32::try_from(source_height_64 * host_width_64 / source_width_64).unwrap_or_default(),
        )
    } else {
        (
            i32::try_from(source_width_64 * host_height_64 / source_height_64).unwrap_or_default(),
            host_height,
        )
    };
    let left = bounds.left + ((host_width - width) / 2);
    let top = bounds.top + ((host_height - height) / 2);
    RECT {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_fit_preserves_aspect_ratio_and_centers() {
        assert_eq!(
            fit_rectangle(
                RECT {
                    left: 100,
                    top: 50,
                    right: 900,
                    bottom: 850,
                },
                1600,
                900,
            ),
            RECT {
                left: 100,
                top: 225,
                right: 900,
                bottom: 675,
            }
        );
    }

    #[test]
    fn preview_fit_rejects_empty_input() {
        assert_eq!(fit_rectangle(RECT::default(), 1600, 900), RECT::default());
        assert_eq!(
            fit_rectangle(
                RECT {
                    left: 0,
                    top: 0,
                    right: 800,
                    bottom: 600,
                },
                0,
                900,
            ),
            RECT::default()
        );
    }

    #[test]
    fn window_preview_includes_the_non_client_title_bar() {
        let placement = window_preview_layout(
            RECT {
                left: 100,
                top: 50,
                right: 900,
                bottom: 850,
            },
            1600,
            900,
        );

        assert!(!placement.source_client_area_only);
    }

    #[test]
    fn preview_border_stays_inside_the_host() {
        let host = RECT {
            left: 100,
            top: 50,
            right: 900,
            bottom: 850,
        };
        let content = inset_rectangle(host, 2);
        let preview = fit_rectangle(content, 1600, 900);

        assert_eq!(outset_rectangle(preview, 2, host).left, 100);
        assert_eq!(outset_rectangle(preview, 2, host).right, 900);
        assert!(outset_rectangle(preview, 2, host).top >= host.top);
        assert!(outset_rectangle(preview, 2, host).bottom <= host.bottom);
    }

    #[test]
    fn only_full_desktop_preview_includes_a_background_frame() {
        let host = RECT {
            left: 100,
            top: 50,
            right: 900,
            bottom: 850,
        };
        let placement = PreviewPlacement::new(
            RECT {
                left: 250,
                top: 200,
                right: 650,
                bottom: 500,
            },
            RECT {
                left: 120,
                top: 180,
                right: 880,
                bottom: 720,
            },
            RECT::default(),
            false,
        );

        assert_eq!(
            preview_background_frame(placement, true, 2, host),
            Some(RECT {
                left: 118,
                top: 178,
                right: 882,
                bottom: 722,
            })
        );
        assert_eq!(preview_background_frame(placement, false, 2, host), None);
    }
}
