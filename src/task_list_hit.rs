//! Which row of the overlay's task list, or which close button, a point lands on.

use crate::overlay_layout::{LogicalRect, for_compact_list, layout_scale};
use crate::switcher::Switcher;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskListHit {
    Task(usize),
    CloseButton(usize),
}

impl TaskListHit {
    #[must_use]
    pub const fn position(self) -> usize {
        match self {
            Self::Task(position) | Self::CloseButton(position) => position,
        }
    }
}

#[must_use]
#[allow(
    clippy::cast_precision_loss,
    reason = "Win32 client coordinates and DPI values are small integers represented as f32"
)]
pub fn hit_test_pixels(
    switcher: &mut Switcher,
    client_pixels: (i32, i32),
    point_pixels: (i32, i32),
    window_dpi: u32,
    compact_list: bool,
) -> Option<TaskListHit> {
    let scale = layout_scale(window_dpi);
    let logical_scale = 1.0 / scale;
    hit_test_at_scale(
        switcher,
        client_pixels.0 as f32 * logical_scale,
        client_pixels.1 as f32 * logical_scale,
        point_pixels.0 as f32 * logical_scale,
        point_pixels.1 as f32 * logical_scale,
        compact_list,
        scale,
    )
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "logical coordinates are bounded to the small on-screen task list"
)]
fn hit_test_at_scale(
    switcher: &mut Switcher,
    client_width: f32,
    client_height: f32,
    x: f32,
    y: f32,
    compact_list: bool,
    scale: f32,
) -> Option<TaskListHit> {
    let layout = for_compact_list(compact_list);
    let list_width = layout.list_width(client_width, scale);
    let list_top = layout.list_top();
    if x < layout.outer_padding || x >= list_width || y < list_top {
        return None;
    }

    let visible_rows = layout.visible_row_count(client_height);
    let start = switcher.visible_range(visible_rows).start;
    let row = layout.visible_row_at(client_height, y)?;
    let position = start + row + 1;
    if position > switcher.visible_task_count() {
        return None;
    }
    switcher.pin_visible_range(visible_rows);

    let row_top = list_top + (row as f32 * (layout.row_height + layout.row_gap));
    let row_bounds = LogicalRect {
        left: layout.outer_padding,
        top: row_top,
        right: list_width,
        bottom: row_top + layout.row_height,
    };
    let selected_position = switcher.selected_visible_index().map(|index| index + 1);
    if selected_position == Some(position) && layout.close_button_bounds(row_bounds).contains(x, y)
    {
        Some(TaskListHit::CloseButton(position))
    } else {
        Some(TaskListHit::Task(position))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay_layout::{close_glyph_geometry, layout_dpi};
    use crate::settings::Settings;
    use crate::switcher::SwitchTask;
    use TaskListHit::{CloseButton, Task};

    fn hit_test_task_list(
        switcher: &mut Switcher,
        client_width: f32,
        client_height: f32,
        x: f32,
        y: f32,
        compact_list: bool,
    ) -> Option<TaskListHit> {
        hit_test_at_scale(
            switcher,
            client_width,
            client_height,
            x,
            y,
            compact_list,
            1.0,
        )
    }

    #[test]
    fn hit_test_prioritizes_only_the_selected_rows_close_button() {
        let mut switcher = switcher_with_tasks(3);

        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 49.0, false),
            Some(TaskListHit::CloseButton(1))
        );
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 375.0, 49.0, false),
            Some(TaskListHit::Task(1))
        );
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 113.0, false),
            Some(TaskListHit::Task(2))
        );
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 81.0, false),
            None
        );

        assert!(switcher.select_visible_position(2));
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 113.0, false),
            Some(TaskListHit::CloseButton(2))
        );
    }

    #[test]
    fn hit_test_keeps_the_scrolled_selected_close_button_on_its_visible_row() {
        let mut switcher = switcher_with_tasks(10);
        assert!(switcher.select_visible_position(8));

        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 168.0, 390.0, 113.0, false),
            Some(TaskListHit::CloseButton(8))
        );
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 168.0, 390.0, 49.0, false),
            Some(TaskListHit::Task(7))
        );
    }

    #[test]
    fn hover_selection_does_not_move_the_task_under_a_stationary_cursor() {
        let mut switcher = switcher_with_tasks(10);
        assert!(switcher.select_visible_position(8));

        let hit = hit_test_task_list(&mut switcher, 900.0, 168.0, 375.0, 49.0, false);
        assert_eq!(hit, Some(TaskListHit::Task(7)));
        assert!(hit.is_some_and(|hit| switcher.select_visible_position(hit.position())));

        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 168.0, 375.0, 49.0, false),
            Some(TaskListHit::Task(7))
        );
    }

    #[test]
    fn default_typed_search_maps_hidden_fractional_dpi_rows_to_mouse_hits() {
        let defaults = Settings::default();
        assert!(defaults.appearance.compact_list);
        assert!(defaults.general.typed_search);

        // In logical pixels the list spans x 18 to 243.2, its rows y 18 to 62 and 64 to 108,
        // and the selected row's close button x 211.2 to 235.2 and y 10 to 34 below the row's
        // top. Each pair of pixels below straddles one of those edges.
        let mut switcher = scrolled_switcher();
        assert_hits_at_fractional_dpi(
            &mut switcher,
            true,
            &[
                ((22, 50), None),
                ((23, 50), Some(Task(7))),
                ((303, 50), Some(Task(7))),
                ((304, 50), None),
                ((100, 22), None),
                ((100, 23), Some(Task(7))),
                ((100, 77), Some(Task(7))),
                ((100, 78), None),
                ((100, 79), None),
                ((100, 80), Some(Task(8))),
                ((100, 134), Some(Task(8))),
                ((100, 135), None),
                ((100, 209), None),
                ((263, 110), Some(Task(8))),
                ((264, 110), Some(CloseButton(8))),
                ((293, 110), Some(CloseButton(8))),
                ((294, 110), Some(Task(8))),
                ((280, 92), Some(Task(8))),
                ((280, 93), Some(CloseButton(8))),
                ((280, 122), Some(CloseButton(8))),
                ((280, 123), Some(Task(8))),
                ((280, 50), Some(Task(7))),
            ],
        );

        // Hovering the seventh task selects it without scrolling the list.
        assert_eq!(
            hit_at_fractional_dpi(&mut switcher, true, (100, 50)),
            Some(Task(7))
        );
        assert!(switcher.select_visible_position(7));
        assert_hits_at_fractional_dpi(
            &mut switcher,
            true,
            &[
                ((100, 23), Some(Task(7))),
                ((100, 77), Some(Task(7))),
                ((100, 78), None),
                ((100, 80), Some(Task(8))),
                ((100, 134), Some(Task(8))),
                ((100, 135), None),
                ((280, 34), Some(Task(7))),
                ((280, 35), Some(CloseButton(7))),
                ((280, 64), Some(CloseButton(7))),
                ((280, 65), Some(Task(7))),
                ((280, 110), Some(Task(8))),
            ],
        );
    }

    #[test]
    fn normal_list_maps_fractional_dpi_pixels_to_the_row_drawn_there() {
        // In logical pixels the list spans x 20 to 414.4, its rows y 20 to 78 and 84 to 142,
        // and the selected row's close button x 376.4 to 406.4 and y 14 to 44 below the row's
        // top. Each pair of pixels below straddles one of those edges.
        let mut switcher = scrolled_switcher();
        assert_hits_at_fractional_dpi(
            &mut switcher,
            false,
            &[
                ((24, 50), None),
                ((25, 50), Some(Task(7))),
                ((517, 50), Some(Task(7))),
                ((518, 50), None),
                ((100, 24), None),
                ((100, 25), Some(Task(7))),
                ((100, 97), Some(Task(7))),
                ((100, 98), None),
                ((100, 104), None),
                ((100, 105), Some(Task(8))),
                ((100, 177), Some(Task(8))),
                ((100, 178), None),
                ((100, 209), None),
                ((470, 140), Some(Task(8))),
                ((471, 140), Some(CloseButton(8))),
                ((507, 140), Some(CloseButton(8))),
                ((508, 140), Some(Task(8))),
                ((490, 122), Some(Task(8))),
                ((490, 123), Some(CloseButton(8))),
                ((490, 159), Some(CloseButton(8))),
                ((490, 160), Some(Task(8))),
                ((490, 60), Some(Task(7))),
            ],
        );

        // Hovering the seventh task selects it without scrolling the list.
        assert_eq!(
            hit_at_fractional_dpi(&mut switcher, false, (100, 50)),
            Some(Task(7))
        );
        assert!(switcher.select_visible_position(7));
        assert_hits_at_fractional_dpi(
            &mut switcher,
            false,
            &[
                ((100, 25), Some(Task(7))),
                ((100, 97), Some(Task(7))),
                ((100, 98), None),
                ((100, 105), Some(Task(8))),
                ((100, 177), Some(Task(8))),
                ((100, 178), None),
                ((490, 42), Some(Task(7))),
                ((490, 43), Some(CloseButton(7))),
                ((490, 79), Some(CloseButton(7))),
                ((490, 80), Some(Task(7))),
                ((490, 140), Some(Task(8))),
            ],
        );
    }

    #[test]
    fn hidden_typed_search_and_close_button_share_no_box_geometry() {
        let mut switcher = switcher_with_tasks(3);

        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 49.0, false),
            Some(TaskListHit::CloseButton(1))
        );
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 81.0, false),
            None
        );
    }

    #[test]
    fn typed_filtering_uses_hidden_geometry_for_mouse_hit_testing() {
        let defaults = Settings::default();
        assert!(defaults.general.typed_search);
        let mut switcher = Switcher::default();
        switcher.set_tasks([
            SwitchTask::new(1, 1, "Editor", "editor"),
            SwitchTask::new(2, 2, "Browser", "browser"),
        ]);
        switcher.append_filter_character('b');

        assert_eq!(switcher.visible_task_count(), 1);
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 49.0, false),
            Some(TaskListHit::CloseButton(1))
        );
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 99.0, false),
            None
        );
    }

    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the known positive test coordinates fit inside the fixed i32 client rectangle"
    )]
    fn dpi_192_caps_presentation_and_keeps_close_hit_aligned_with_glyph() {
        const WINDOW_DPI: u32 = 192;
        let scale = layout_scale(WINDOW_DPI);
        assert_eq!(layout_dpi(WINDOW_DPI), 168);
        assert_near(scale, 1.75);

        let layout = for_compact_list(false);
        let row_bounds = LogicalRect {
            left: layout.outer_padding,
            top: layout.list_top(),
            right: layout.list_width(900.0, scale),
            bottom: layout.list_top() + layout.row_height,
        };
        let hit_target = layout.close_button_bounds(row_bounds);
        let glyph = close_glyph_geometry(hit_target, false, scale);
        let center_x = f32::midpoint(hit_target.left, hit_target.right);
        let center_y = f32::midpoint(hit_target.top, hit_target.bottom);
        assert_near(hit_target.right - hit_target.left, 30.0);
        assert_near((glyph.bounds.right - glyph.bounds.left) * scale, 18.0);
        assert_near(
            f32::midpoint(glyph.bounds.left, glyph.bounds.right) * scale,
            center_x * scale,
        );
        assert_near(
            f32::midpoint(glyph.bounds.top, glyph.bounds.bottom) * scale,
            center_y * scale,
        );

        let mut switcher = switcher_with_tasks(3);
        assert_eq!(
            hit_test_pixels(
                &mut switcher,
                (1_575, 900),
                (
                    (center_x * scale).round() as i32,
                    (center_y * scale).round() as i32,
                ),
                WINDOW_DPI,
                false,
            ),
            Some(TaskListHit::CloseButton(1))
        );
    }

    #[test]
    fn mouse_hit_pins_viewport_until_keyboard_navigation_recenters() {
        let mut switcher = switcher_with_tasks(10);
        assert!(switcher.select_visible_position(8));
        assert_eq!(switcher.visible_range(2), 6..8);

        let hit = hit_test_task_list(&mut switcher, 900.0, 168.0, 375.0, 49.0, false);
        assert_eq!(hit, Some(TaskListHit::Task(7)));
        assert!(hit.is_some_and(|hit| switcher.select_visible_position(hit.position())));
        assert_eq!(switcher.visible_range(2), 6..8);

        switcher.select_bounded(-1);
        assert_eq!(switcher.visible_range(2), 4..6);
    }

    #[test]
    fn first_and_last_selection_stay_inside_the_rendered_range() {
        let mut switcher = switcher_with_tasks(10);
        let visible_rows = 3;

        switcher.select_first();
        let first_range = switcher.visible_range(visible_rows);
        assert!(first_range.contains(&switcher.selected_visible_index().unwrap_or_default()));

        switcher.select_last();
        let last_range = switcher.visible_range(visible_rows);
        assert!(last_range.contains(&switcher.selected_visible_index().unwrap_or_default()));
    }

    fn switcher_with_tasks(count: usize) -> Switcher {
        let mut switcher = Switcher::default();
        switcher.set_tasks((1..=count).map(|number| {
            let title = format!("Task {number}");
            SwitchTask::new(
                number,
                isize::try_from(number).unwrap_or_default(),
                &title,
                "app",
            )
        }));
        switcher
    }

    /// Ten tasks with the eighth selected, in a 1125 by 210 pixel client at 120 DPI: 900 by 168
    /// logical pixels, which fit two rows, the seventh and eighth task.
    fn scrolled_switcher() -> Switcher {
        let mut switcher = switcher_with_tasks(10);
        assert!(switcher.select_visible_position(8));
        switcher
    }

    fn hit_at_fractional_dpi(
        switcher: &mut Switcher,
        compact_list: bool,
        pixel: (i32, i32),
    ) -> Option<TaskListHit> {
        hit_test_pixels(switcher, (1_125, 210), pixel, 120, compact_list)
    }

    fn assert_hits_at_fractional_dpi(
        switcher: &mut Switcher,
        compact_list: bool,
        expected: &[((i32, i32), Option<TaskListHit>)],
    ) {
        for &(pixel, hit) in expected {
            assert_eq!(
                hit_at_fractional_dpi(switcher, compact_list, pixel),
                hit,
                "unexpected hit at physical pixel {pixel:?}"
            );
        }
    }

    fn assert_near(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 0.001);
    }
}
