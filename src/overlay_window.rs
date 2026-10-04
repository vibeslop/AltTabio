//! Where the overlay window sits on its monitor and the border the compositor draws around it.

use crate::theme::{ResolvedTheme, Rgb8};

/// Screen pixels with exclusive right and bottom edges, the layout of a Win32 `RECT`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[must_use]
pub const fn overlay_bounds_for_dpi_change(
    _suggested: ScreenRect,
    work_area: ScreenRect,
) -> ScreenRect {
    // The suggested rectangle preserves the window's old logical size. AltTabio instead owns a
    // monitor-relative size, so scaling that rectangle can make the overlay fill a high-DPI screen.
    overlay_bounds(work_area)
}

#[must_use]
pub const fn overlay_bounds(work_area: ScreenRect) -> ScreenRect {
    let area_width = work_area.right.saturating_sub(work_area.left);
    let area_height = work_area.bottom.saturating_sub(work_area.top);
    let width = area_width.saturating_mul(5) / 8;
    let height = area_height.saturating_mul(5) / 8;
    let left = work_area
        .left
        .saturating_add(area_width.saturating_sub(width) / 2);
    let top = work_area
        .top
        .saturating_add(area_height.saturating_sub(height) / 2);
    ScreenRect {
        left,
        top,
        right: left.saturating_add(width),
        bottom: top.saturating_add(height),
    }
}

/// The overlay's border as a `COLORREF`, or `None` when the compositor should draw no border.
#[must_use]
pub const fn compositor_border_color(visible_borders: bool, theme: ResolvedTheme) -> Option<u32> {
    if visible_borders {
        Some(colorref(theme.palette().window_border))
    } else {
        None
    }
}

#[must_use]
pub const fn colorref(color: Rgb8) -> u32 {
    color.red as u32 | ((color.green as u32) << 8) | ((color.blue as u32) << 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compositor_border_tracks_the_visible_borders_setting() {
        assert_eq!(
            compositor_border_color(true, ResolvedTheme::Dark),
            Some(0x0064_6161)
        );
        assert_eq!(
            compositor_border_color(true, ResolvedTheme::Light),
            Some(0x009A_9A9A)
        );
        assert_eq!(compositor_border_color(false, ResolvedTheme::Dark), None);
    }

    #[test]
    fn colorref_preserves_windows_bgr_storage_order() {
        assert_eq!(colorref(Rgb8::new(0x12, 0x34, 0x56)), 0x0056_3412);
    }

    #[test]
    fn dpi_change_keeps_overlay_relative_to_positive_secondary_monitor() {
        let suggested = ScreenRect {
            left: 1_920,
            top: 0,
            right: 3_945,
            bottom: 1_350,
        };
        let work_area = ScreenRect {
            left: 1_920,
            top: 0,
            right: 4_480,
            bottom: 1_400,
        };

        assert_eq!(
            overlay_bounds_for_dpi_change(suggested, work_area),
            ScreenRect {
                left: 2_400,
                top: 262,
                right: 4_000,
                bottom: 1_137,
            }
        );
    }

    #[test]
    fn portrait_topology_uses_current_work_area_instead_of_landscape_bounds() {
        assert_eq!(
            overlay_bounds(ScreenRect {
                left: 0,
                top: 0,
                right: 1_080,
                bottom: 1_920,
            }),
            ScreenRect {
                left: 202,
                top: 360,
                right: 877,
                bottom: 1_560,
            }
        );
    }

    #[test]
    fn dpi_change_keeps_overlay_relative_to_negative_monitor_origin() {
        let suggested = ScreenRect {
            left: -2_560,
            top: -120,
            right: -535,
            bottom: 1_230,
        };
        let work_area = ScreenRect {
            left: -2_560,
            top: -120,
            right: 0,
            bottom: 1_280,
        };

        assert_eq!(
            overlay_bounds_for_dpi_change(suggested, work_area),
            ScreenRect {
                left: -2_080,
                top: 142,
                right: -480,
                bottom: 1_017,
            }
        );
    }
}
