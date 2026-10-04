//! Where the overlay window sits on its monitor and the border the compositor draws around it.

use crate::dialog_layout::{Rect, Size};
use crate::theme::{ResolvedTheme, Rgb8};

#[must_use]
pub const fn overlay_bounds(work_area: Rect) -> Rect {
    let size = Size::new(
        work_area.width.saturating_mul(5) / 8,
        work_area.height.saturating_mul(5) / 8,
    );
    let origin = work_area.centered(size);
    Rect::new(origin.x, origin.y, size.width, size.height)
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

const fn colorref(color: Rgb8) -> u32 {
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
        let work_area = Rect::from_edges(1_920, 0, 4_480, 1_400);

        assert_eq!(
            overlay_bounds(work_area),
            Rect::from_edges(2_400, 262, 4_000, 1_137)
        );
    }

    #[test]
    fn portrait_topology_uses_current_work_area_instead_of_landscape_bounds() {
        assert_eq!(
            overlay_bounds(Rect::from_edges(0, 0, 1_080, 1_920)),
            Rect::from_edges(202, 360, 877, 1_560)
        );
    }

    #[test]
    fn dpi_change_keeps_overlay_relative_to_negative_monitor_origin() {
        let work_area = Rect::from_edges(-2_560, -120, 0, 1_280);

        assert_eq!(
            overlay_bounds(work_area),
            Rect::from_edges(-2_080, 142, -480, 1_017)
        );
    }
}
