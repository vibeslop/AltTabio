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

/// A rectangle in logical pixels. It is `f32` with right and bottom edges, like the `D2D_RECT_F`
/// the Windows renderer draws it as, so every edge has the bits Direct2D would have computed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl LogicalRect {
    /// Whether the point is inside, counting the left and top edges but not the right and
    /// bottom ones.
    #[must_use]
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
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

    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "a visible row index is a small integer represented exactly as f32"
    )]
    pub fn row_top(self, row: usize) -> f32 {
        self.list_top() + (row as f32 * (self.row_height + self.row_gap))
    }

    #[must_use]
    pub fn row_bounds(self, row: usize, list_width: f32) -> LogicalRect {
        let top = self.row_top(row);
        LogicalRect {
            left: self.outer_padding,
            top,
            right: list_width,
            bottom: top + self.row_height,
        }
    }

    #[must_use]
    pub const fn icon_size(self, large_icons: bool) -> f32 {
        if large_icons {
            self.large_icon_size
        } else {
            self.small_icon_size
        }
    }

    #[must_use]
    pub fn icon_slot_left(self, show_numbers: bool) -> f32 {
        self.outer_padding + if show_numbers { self.number_width } else { 0.0 }
    }

    #[must_use]
    pub fn icon_bounds(self, row: usize, show_numbers: bool, large_icons: bool) -> LogicalRect {
        let size = self.icon_size(large_icons);
        let left = self.icon_slot_left(show_numbers) + ((self.icon_slot_width - size) / 2.0);
        let top = self.row_top(row) + ((self.row_height - size) / 2.0);
        LogicalRect {
            left,
            top,
            right: left + size,
            bottom: top + size,
        }
    }

    #[must_use]
    pub fn text_left(self, show_numbers: bool) -> f32 {
        self.icon_slot_left(show_numbers) + self.icon_slot_width + self.icon_text_gap
    }

    /// Where a row's title and app name are clipped: short of the close button the selected
    /// row carries, or of the row's end.
    #[must_use]
    pub fn text_right(self, row_bounds: LogicalRect, close_button: Option<LogicalRect>) -> f32 {
        close_button.map_or(row_bounds.right - 12.0, |button| {
            button.left - self.close_button_gap
        })
    }

    #[must_use]
    pub fn close_button_bounds(self, row_bounds: LogicalRect) -> LogicalRect {
        let top = row_bounds.top + ((self.row_height - self.close_button_size) / 2.0);
        LogicalRect {
            left: row_bounds.right - self.close_button_inset - self.close_button_size,
            top,
            right: row_bounds.right - self.close_button_inset,
            bottom: top + self.close_button_size,
        }
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloseGlyphGeometry {
    pub bounds: LogicalRect,
    pub stroke_width: f32,
}

#[must_use]
pub fn close_glyph_geometry(
    hit_target: LogicalRect,
    compact_list: bool,
    scale: f32,
) -> CloseGlyphGeometry {
    let scale = scale.max(1.0);
    let nominal_extent = if compact_list { 8.0 } else { 10.0 };
    let extent = (nominal_extent * scale).round() / scale;
    let center_x = f32::midpoint(hit_target.left, hit_target.right);
    let center_y = f32::midpoint(hit_target.top, hit_target.bottom);
    let half_extent = extent / 2.0;
    CloseGlyphGeometry {
        bounds: LogicalRect {
            left: center_x - half_extent,
            top: center_y - half_extent,
            right: center_x + half_extent,
            bottom: center_y + half_extent,
        },
        stroke_width: 1.5,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowFrameGeometry {
    pub rect: LogicalRect,
    pub radius: f32,
    pub stroke_width: f32,
}

#[must_use]
pub fn window_frame_geometry(width: f32, height: f32, scale: f32) -> WindowFrameGeometry {
    let pixel = 1.0 / scale;
    WindowFrameGeometry {
        rect: LogicalRect {
            left: pixel,
            top: pixel,
            right: (width - pixel).max(pixel),
            bottom: (height - pixel).max(pixel),
        },
        radius: 10.0,
        stroke_width: 2.0 / scale,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TaskTextVerticalLayout {
    pub title_top: f32,
    pub title_bottom: f32,
    pub app_name: Option<(f32, f32)>,
}

#[must_use]
pub fn task_text_vertical_layout(
    row_top: f32,
    row_bottom: f32,
    show_app_names: bool,
    compact_list: bool,
) -> TaskTextVerticalLayout {
    if show_app_names {
        if compact_list {
            TaskTextVerticalLayout {
                title_top: row_top + 1.0,
                title_bottom: row_top + 26.0,
                app_name: Some((row_top + 22.0, row_bottom - 1.0)),
            }
        } else {
            TaskTextVerticalLayout {
                title_top: row_top + 3.0,
                title_bottom: row_top + 35.0,
                app_name: Some((row_top + 31.0, row_bottom - 2.0)),
            }
        }
    } else {
        TaskTextVerticalLayout {
            title_top: row_top,
            title_bottom: row_bottom,
            app_name: None,
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

    #[test]
    fn drawn_rows_are_the_rows_the_pointer_finds() {
        for compact in [false, true] {
            let layout = for_compact_list(compact);
            let rows = layout.visible_row_count(600.0);
            for row in 0..rows {
                let bounds = layout.row_bounds(row, 300.0);
                assert_eq!(layout.visible_row_at(600.0, bounds.top), Some(row));
                assert_eq!(layout.visible_row_at(600.0, bounds.bottom - 0.5), Some(row));
                assert_eq!(layout.visible_row_at(600.0, bounds.bottom), None);
            }
            assert_eq!(
                layout.visible_row_at(600.0, layout.row_bounds(rows, 300.0).top),
                None
            );
        }
    }

    #[test]
    fn rows_step_down_from_the_list_top() {
        let compact = for_compact_list(true);
        assert_eq!(
            compact.row_bounds(2, 243.2),
            LogicalRect {
                left: 18.0,
                top: 110.0,
                right: 243.2,
                bottom: 154.0,
            }
        );

        let roomy = for_compact_list(false);
        assert_close(roomy.row_top(0), 20.0);
        assert_close(roomy.row_top(3), 212.0);
    }

    #[test]
    fn icons_center_in_their_slot_and_titles_start_after_it() {
        let compact = for_compact_list(true);
        assert_eq!(
            compact.icon_bounds(1, true, true),
            LogicalRect {
                left: 52.0,
                top: 72.0,
                right: 80.0,
                bottom: 100.0,
            }
        );
        assert_close(compact.text_left(true), 87.0);

        let roomy = for_compact_list(false);
        assert_eq!(
            roomy.icon_bounds(0, false, false),
            LogicalRect {
                left: 32.0,
                top: 39.0,
                right: 52.0,
                bottom: 59.0,
            }
        );
        assert_close(roomy.text_left(false), 68.0);
    }

    #[test]
    fn text_stops_short_of_the_close_button_or_the_row_end() {
        let compact = for_compact_list(true);
        let row = compact.row_bounds(0, 260.0);
        let close_button = compact.close_button_bounds(row);
        assert_close(compact.text_right(row, None), 248.0);
        assert_close(compact.text_right(row, Some(close_button)), 222.0);

        let roomy = for_compact_list(false);
        let row = roomy.row_bounds(0, 414.0);
        let close_button = roomy.close_button_bounds(row);
        assert_close(roomy.text_right(row, None), 402.0);
        assert_close(roomy.text_right(row, Some(close_button)), 368.0);
    }

    #[test]
    fn title_uses_the_full_row_when_app_names_are_hidden() {
        let layout = task_text_vertical_layout(20.0, 78.0, false, false);

        assert_close(layout.title_top, 20.0);
        assert_close(layout.title_bottom, 78.0);
        assert_eq!(layout.app_name, None);
    }

    #[test]
    fn title_moves_up_when_app_names_are_shown() {
        let layout = task_text_vertical_layout(20.0, 78.0, true, false);

        assert_close(layout.title_top, 23.0);
        assert_close(layout.title_bottom, 55.0);
        assert!(layout.app_name.is_some());
        let (app_name_top, app_name_bottom) = layout.app_name.unwrap_or_default();
        assert_close(app_name_top, 51.0);
        assert_close(app_name_bottom, 76.0);
    }

    #[test]
    fn compact_app_names_fit_the_shorter_row() {
        let layout = task_text_vertical_layout(18.0, 62.0, true, true);

        assert_close(layout.title_top, 19.0);
        assert_close(layout.title_bottom, 44.0);
        assert_eq!(layout.app_name, Some((40.0, 61.0)));
    }

    #[test]
    fn window_border_aligns_to_two_physical_pixels_at_fractional_dpi() {
        let scale = 1.5;
        let frame = window_frame_geometry(1_600.0, 900.0, scale);

        assert_close(frame.rect.left * scale, 1.0);
        assert_close(frame.rect.top * scale, 1.0);
        assert_close(frame.stroke_width * scale, 2.0);
    }

    #[test]
    fn close_button_is_inset_and_centered_in_each_selected_row_style() {
        let roomy = for_compact_list(false);
        let roomy_bounds = roomy.close_button_bounds(LogicalRect {
            left: 20.0,
            top: 20.0,
            right: 414.0,
            bottom: 78.0,
        });
        assert_eq!(
            roomy_bounds,
            LogicalRect {
                left: 376.0,
                top: 34.0,
                right: 406.0,
                bottom: 64.0,
            }
        );

        let compact = for_compact_list(true);
        let compact_bounds = compact.close_button_bounds(LogicalRect {
            left: 18.0,
            top: 18.0,
            right: 260.0,
            bottom: 62.0,
        });
        assert_eq!(
            compact_bounds,
            LogicalRect {
                left: 228.0,
                top: 28.0,
                right: 252.0,
                bottom: 52.0,
            }
        );
    }

    #[test]
    fn close_glyph_is_smaller_and_centered_inside_each_hit_target() {
        for (compact, scale, expected_hit_extent, expected_dip_extent, expected_physical_extent) in [
            (false, 1.0, 30.0, 10.0, 10.0),
            (false, 1.25, 30.0, 10.4, 13.0),
            (false, 1.5, 30.0, 10.0, 15.0),
            (true, 1.0, 24.0, 8.0, 8.0),
            (true, 1.25, 24.0, 8.0, 10.0),
            (true, 1.5, 24.0, 8.0, 12.0),
        ] {
            let layout = for_compact_list(compact);
            let row_bounds = LogicalRect {
                left: layout.outer_padding,
                top: layout.outer_padding,
                right: layout.list_width(900.0, scale),
                bottom: layout.outer_padding + layout.row_height,
            };
            let hit_target = layout.close_button_bounds(row_bounds);
            let glyph = close_glyph_geometry(hit_target, compact, scale);

            let glyph_center_x = f32::midpoint(glyph.bounds.left, glyph.bounds.right);
            let glyph_center_y = f32::midpoint(glyph.bounds.top, glyph.bounds.bottom);
            let glyph_extent = glyph.bounds.right - glyph.bounds.left;
            let hit_extent = hit_target.right - hit_target.left;
            let hit_center_x = f32::midpoint(hit_target.left, hit_target.right);
            let hit_center_y = f32::midpoint(hit_target.top, hit_target.bottom);
            assert_near(glyph_center_x, hit_center_x);
            assert_near(glyph_center_y, hit_center_y);
            assert_near(glyph_center_x * scale, hit_center_x * scale);
            assert_near(glyph_center_y * scale, hit_center_y * scale);
            assert_near(hit_extent, expected_hit_extent);
            assert_near(hit_extent * scale, expected_hit_extent * scale);
            assert_near(glyph_extent, expected_dip_extent);
            assert_near(glyph_extent * scale, expected_physical_extent);
            assert!(glyph_extent < hit_extent);
            assert!(glyph_extent < hit_target.bottom - hit_target.top);
        }
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < f32::EPSILON);
    }

    fn assert_near(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 0.001);
    }
}
