use super::RenderOptions;
use alttabio::overlay_layout::{for_compact_list, layout_scale};
use alttabio::switcher::Switcher;
use std::ffi::c_void;
use std::fmt;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx, GetClientRect, HICON};
use windows::core::Error;

pub(super) enum IconPassFailure {
    ClientArea(Error),
    Icons { failed: usize, first: Error },
}

impl fmt::Display for IconPassFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClientArea(error) => write!(
                formatter,
                "Could not read the overlay's client area to draw task icons: {error}"
            ),
            Self::Icons { failed: 1, first } => {
                write!(formatter, "Could not draw a task icon: {first}")
            }
            Self::Icons { failed, first } => {
                write!(formatter, "Could not draw {failed} task icons: {first}")
            }
        }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "icon geometry is clamped to the on-screen client area and Win32 HICON values"
)]
pub(super) fn draw_icon_pass(
    hwnd: HWND,
    hdc: HDC,
    switcher: &Switcher,
    options: RenderOptions,
) -> std::result::Result<(), IconPassFailure> {
    let window_dpi = unsafe {
        // SAFETY: hwnd is the live overlay window and the call returns a scalar DPI value.
        GetDpiForWindow(hwnd)
    };
    let scale = layout_scale(window_dpi);
    let mut client = RECT::default();
    unsafe {
        // SAFETY: client is writable and hwnd is the live overlay window.
        GetClientRect(hwnd, &raw mut client)
    }
    .map_err(IconPassFailure::ClientArea)?;
    let height = client.bottom.saturating_sub(client.top) as f32 / scale;
    let layout = for_compact_list(options.compact_list);
    let visible_rows = layout.visible_row_count(height);
    let start = switcher.visible_range(visible_rows).start;
    let icon_pixels = (layout.icon_size(options.large_icons) * scale).round() as i32;

    let mut failed = 0;
    let mut first_error = None;
    for (visible_position, task) in switcher
        .positioned_visible_tasks()
        .skip(start)
        .take(visible_rows)
    {
        let visible_index = visible_position.saturating_sub(1);
        if task.icon_handle == 0 {
            continue;
        }
        let bounds = layout.icon_bounds(
            visible_index - start,
            options.show_numbers,
            options.large_icons,
        );
        let icon = HICON(task.icon_handle as *mut c_void);
        let result = unsafe {
            // SAFETY: hdc is the current BeginPaint DC, the HICON is borrowed from a live
            // window/class snapshot, and all pixel dimensions are positive and on-screen.
            DrawIconEx(
                hdc,
                (bounds.left * scale).round() as i32,
                (bounds.top * scale).round() as i32,
                icon,
                icon_pixels,
                icon_pixels,
                0,
                None,
                DI_NORMAL,
            )
        };
        if let Err(error) = result {
            failed += 1;
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), |first| {
        Err(IconPassFailure::Icons { failed, first })
    })
}
