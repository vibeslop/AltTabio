//! Shared logical-pixel geometry for the task list and preview host.

const BASE_DPI: u16 = 96;
// The overlay already occupies a fixed fraction of its monitor. Capping presentation density keeps
// its list proportions stable on denser monitors without undoing monitor-local window sizing.
const MAX_LAYOUT_DPI: u16 = 168;

#[must_use]
pub fn layout_dpi(window_dpi: u32) -> u16 {
    let bounded = window_dpi.clamp(u32::from(BASE_DPI), u32::from(MAX_LAYOUT_DPI));
    u16::try_from(bounded).unwrap_or(MAX_LAYOUT_DPI)
}

#[must_use]
pub fn layout_scale(window_dpi: u32) -> f32 {
    f32::from(layout_dpi(window_dpi)) / f32::from(BASE_DPI)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayLayout {
    pub outer_padding: f32,
    pub list_width_fraction: f32,
    pub minimum_list_pixel_width: f32,
    pub row_height: f32,
    pub row_gap: f32,
    pub number_width: f32,
    pub icon_slot_width: f32,
    pub icon_text_gap: f32,
    pub large_icon_size: f32,
    pub small_icon_size: f32,
    pub selection_radius: f32,
    pub close_button_size: f32,
    pub close_button_inset: f32,
    pub close_button_gap: f32,
}

impl OverlayLayout {
    #[must_use]
    pub fn list_width(self, client_width: f32, scale: f32) -> f32 {
        let scale = scale.max(1.0);
        let proportional_pixel_width = (client_width * self.list_width_fraction * scale).round();
        proportional_pixel_width.max(self.minimum_list_pixel_width) / scale
    }

    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the clamped positive client height yields a small on-screen row count"
    )]
    pub fn visible_row_count(self, client_height: f32) -> usize {
        let available_height =
            (client_height - self.list_top() - self.outer_padding).max(self.row_height);
        ((available_height + self.row_gap) / (self.row_height + self.row_gap)) as usize
    }

    #[must_use]
    pub const fn list_top(self) -> f32 {
        self.outer_padding
    }

    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the bounded nonnegative list offset maps to a small on-screen row index"
    )]
    pub fn visible_row_at(self, client_height: f32, y: f32) -> Option<usize> {
        let row_offset = y - self.list_top();
        if row_offset < 0.0 {
            return None;
        }
        let stride = self.row_height + self.row_gap;
        let row = (row_offset / stride) as usize;
        (row < self.visible_row_count(client_height) && row_offset % stride < self.row_height)
            .then_some(row)
    }
}

#[must_use]
pub const fn for_compact_list(compact: bool) -> OverlayLayout {
    if compact {
        OverlayLayout {
            outer_padding: 18.0,
            list_width_fraction: 0.27,
            minimum_list_pixel_width: 260.0,
            row_height: 44.0,
            row_gap: 2.0,
            number_width: 30.0,
            icon_slot_width: 36.0,
            icon_text_gap: 3.0,
            large_icon_size: 28.0,
            small_icon_size: 20.0,
            selection_radius: 5.0,
            close_button_size: 24.0,
            close_button_inset: 8.0,
            close_button_gap: 6.0,
        }
    } else {
        OverlayLayout {
            outer_padding: 20.0,
            list_width_fraction: 0.46,
            minimum_list_pixel_width: 320.0,
            row_height: 58.0,
            row_gap: 6.0,
            number_width: 38.0,
            icon_slot_width: 44.0,
            icon_text_gap: 4.0,
            large_icon_size: 32.0,
            small_icon_size: 20.0,
            selection_radius: 7.0,
            close_button_size: 30.0,
            close_button_inset: 8.0,
            close_button_gap: 8.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_mode_gives_more_width_to_the_preview() {
        let compact = for_compact_list(true);
        let roomy = for_compact_list(false);

        assert!((compact.list_width(1_920.0, 1.0) - 518.0).abs() < 0.01);
        assert!((roomy.list_width(1_920.0, 1.0) - 883.0).abs() < 0.01);
    }

    #[test]
    fn compact_list_width_keeps_its_physical_proportion_at_common_display_scales() {
        let layout = for_compact_list(true);
        let client_pixel_width = 1_260.0;
        let expected_list_right = 340.0;

        for window_dpi in [96, 120, 144, 168, 192] {
            let scale = layout_scale(window_dpi);
            let client_width = client_pixel_width / scale;
            let list_right = (layout.list_width(client_width, scale) * scale).round();

            assert!(
                (list_right - expected_list_right).abs() < f32::EPSILON,
                "compact list edge changed at {window_dpi} DPI: expected {expected_list_right}, got {list_right}"
            );
            let divider_right = (list_right + (layout.outer_padding * scale)) / client_pixel_width;
            assert!(
                (0.28..0.30).contains(&divider_right),
                "divider stopped occupying a little less than one third at {window_dpi} DPI"
            );
        }
    }

    #[test]
    fn compact_mode_fits_more_rows() {
        let compact = for_compact_list(true);
        let roomy = for_compact_list(false);

        assert!(compact.visible_row_count(600.0) > roomy.visible_row_count(600.0));
    }

    #[test]
    fn list_starts_at_outer_padding_and_uses_the_complete_height() {
        let layout = for_compact_list(true);

        assert!((layout.list_top() - layout.outer_padding).abs() < f32::EPSILON);
        assert_eq!(layout.visible_row_count(600.0), 12);
    }

    #[test]
    fn enabled_typed_filtering_uses_no_search_box_geometry() {
        let layout = for_compact_list(true);

        assert!((layout.list_top() - layout.outer_padding).abs() < f32::EPSILON);
        assert_eq!(layout.visible_row_count(600.0), 12);
        assert_eq!(layout.visible_row_at(600.0, 18.0), Some(0));
        assert_eq!(layout.visible_row_at(600.0, 64.0), Some(1));
        assert_eq!(layout.visible_row_at(600.0, 62.0), None);
    }

    #[test]
    fn default_compact_layout_keeps_the_established_2400_by_1350_screenshot_geometry() {
        let layout = for_compact_list(true);
        let scale = layout_scale(192);
        let client_pixel_width = 2_400.0;
        let client_width = client_pixel_width / scale;
        let list_right = layout.list_width(client_width, scale);
        let content_left = (layout.outer_padding
            + layout.number_width
            + layout.icon_slot_width
            + layout.icon_text_gap)
            * scale;

        assert!((layout.list_top() * scale - 31.5).abs() < 0.01);
        assert!((content_left - 152.25).abs() < 0.01);
        assert!((layout.row_height * scale - 77.0).abs() < 0.01);
        assert!(((layout.row_height + layout.row_gap) * scale - 80.5).abs() < 0.01);
        assert!((layout.large_icon_size * scale - 49.0).abs() < 0.01);
        assert!(((list_right + layout.outer_padding) * scale - 679.5).abs() < 0.01);
    }
}
