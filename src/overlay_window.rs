//! Where the overlay window sits on its monitor and the border the compositor draws around it.

use crate::dialog_layout::{Rect, Size};
use crate::theme::{ResolvedTheme, Rgb8};

/// Foreground ownership for one opening. Failed activation is not a loss of owned focus.
#[derive(Debug, Default)]
pub struct OverlayFocus {
    acquired: bool,
}

impl OverlayFocus {
    pub fn reset(&mut self) {
        self.acquired = false;
    }

    /// Records a foreground observation and reports a loss only after ownership was acquired.
    #[must_use]
    pub fn observe(&mut self, owns_foreground: bool) -> bool {
        if owns_foreground {
            self.acquired = true;
            false
        } else {
            self.acquired
        }
    }
}

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

/// DWM dark-mode attribute supported by the running Windows build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DarkModeAttribute {
    /// `DWMWA_USE_IMMERSIVE_DARK_MODE` (20), supported starting on Windows 10 20H1 (build 19041).
    Modern,
    /// Pre-20H1 `DWMWA_USE_IMMERSIVE_DARK_MODE` (19), supported on Windows 10 1809–1909.
    Legacy,
}

impl DarkModeAttribute {
    #[must_use]
    pub const fn dwm_attribute(self) -> u32 {
        match self {
            Self::Modern => 20,
            Self::Legacy => 19,
        }
    }
}

const WINDOWS_10_1809_BUILD: u32 = 17_763;
const WINDOWS_10_20H1_BUILD: u32 = 19_041;
const WINDOWS_11_BUILD: u32 = 22_000;
const E_INVALIDARG_HRESULT: i32 = 0x8007_0057_u32.cast_signed();

/// Returns which DWM dark-mode attribute ID applies on `windows_build`, if any.
#[must_use]
pub const fn dark_mode_attribute(windows_build: Option<u32>) -> Option<DarkModeAttribute> {
    match windows_build {
        None | Some(WINDOWS_10_20H1_BUILD..) => Some(DarkModeAttribute::Modern),
        Some(WINDOWS_10_1809_BUILD..WINDOWS_10_20H1_BUILD) => Some(DarkModeAttribute::Legacy),
        Some(..WINDOWS_10_1809_BUILD) => None,
    }
}

/// Rounded corners (`DWMWA_WINDOW_CORNER_PREFERENCE`) and compositor border colors
/// (`DWMWA_BORDER_COLOR`) require Windows 11 (build 22000 or later).
#[must_use]
pub const fn supports_window_frame_attributes(windows_build: Option<u32>) -> bool {
    matches!(windows_build, None | Some(WINDOWS_11_BUILD..))
}

/// DWM returns `E_INVALIDARG` (`0x80070057`) for attributes the running Windows build does not
/// recognize, so optional compositor styling treats that code as unsupported rather than fatal.
#[must_use]
pub const fn is_unsupported_dwm_attribute_hresult(hresult: i32) -> bool {
    hresult == E_INVALIDARG_HRESULT
}

const fn colorref(color: Rgb8) -> u32 {
    color.red as u32 | ((color.green as u32) << 8) | ((color.blue as u32) << 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dwm_attributes_match_windows_10_and_windows_11_builds() {
        assert_eq!(dark_mode_attribute(Some(17_762)), None);
        assert!(!supports_window_frame_attributes(Some(17_762)));

        for build in [17_763, 18_362, 18_363, 19_040] {
            assert_eq!(
                dark_mode_attribute(Some(build)),
                Some(DarkModeAttribute::Legacy)
            );
            assert!(!supports_window_frame_attributes(Some(build)));
        }

        for build in [19_041, 19_044, 19_045, 21_999] {
            assert_eq!(
                dark_mode_attribute(Some(build)),
                Some(DarkModeAttribute::Modern)
            );
            assert!(!supports_window_frame_attributes(Some(build)));
        }

        for build in [22_000, 22_631, 26_100, 26_200] {
            assert_eq!(
                dark_mode_attribute(Some(build)),
                Some(DarkModeAttribute::Modern)
            );
            assert!(supports_window_frame_attributes(Some(build)));
        }

        assert_eq!(dark_mode_attribute(None), Some(DarkModeAttribute::Modern));
        assert!(supports_window_frame_attributes(None));
        assert_eq!(DarkModeAttribute::Legacy.dwm_attribute(), 19);
        assert_eq!(DarkModeAttribute::Modern.dwm_attribute(), 20);
        assert!(is_unsupported_dwm_attribute_hresult(
            0x8007_0057_u32.cast_signed()
        ));
        assert!(!is_unsupported_dwm_attribute_hresult(
            0x8007_0006_u32.cast_signed()
        ));
    }

    #[test]
    fn failed_activation_does_not_count_as_losing_foreground_ownership() {
        let mut focus = OverlayFocus::default();
        assert!(!focus.observe(false));
        assert!(!focus.observe(false));
        assert!(!focus.observe(true));
        assert!(focus.observe(false));
        focus.reset();
        assert!(!focus.observe(false));
    }

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
