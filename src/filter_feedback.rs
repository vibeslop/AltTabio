//! The active query's footer and the task viewport above it.

use crate::dialog_layout::{Rect, scale};
use crate::overlay_layout::{OverlayLayout, for_compact_list, layout_dpi, layout_scale};

pub const FOOTER_HEIGHT: f32 = 32.0;

/// Keep the list's origin and row geometry fixed. Its existing bottom padding still holds the
/// continuation chevron, immediately above the footer; the footer has its own bottom inset.
#[must_use]
pub fn list_height(client_height: f32, layout: OverlayLayout, filter_visible: bool) -> f32 {
    if filter_visible {
        (client_height - layout.outer_padding - FOOTER_HEIGHT).max(0.0)
    } else {
        client_height
    }
}

/// Place the editor entirely inside the panel, aligned with the task rows. Work-area edges do
/// not affect its visibility because it never extends outside the panel.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "monitor dimensions and rounded on-screen coordinates fit i32 and f32"
)]
pub fn popup_bounds(panel: Rect, window_dpi: u32, compact_list: bool) -> Option<Rect> {
    let dpi = u32::from(layout_dpi(window_dpi));
    let factor = layout_scale(window_dpi);
    let layout = for_compact_list(compact_list);
    let padding = (layout.outer_padding * factor).round() as i32;
    let client_width = panel.width as f32 / factor;
    let list_width = layout
        .list_width(client_width, factor)
        .min(client_width - layout.outer_padding);
    let width = ((list_width - layout.outer_padding) * factor).round() as i32;
    let height = (FOOTER_HEIGHT * factor).round() as i32;
    let top = list_height(panel.height as f32 / factor, layout, true);
    if width < scale(100, dpi) || top < layout.outer_padding {
        return None;
    }
    Some(Rect::new(
        panel.x.saturating_add(padding),
        panel.y.saturating_add((top * factor).round() as i32),
        width,
        height,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay_window::overlay_bounds;

    #[test]
    fn an_active_query_never_becomes_an_invisible_filter() {
        use crate::input::InputAction;
        use crate::switcher::{SwitchTask, SwitcherSession, SwitcherSessionSettings};
        let mut session = SwitcherSession::new(SwitcherSessionSettings {
            typed_search: true,
            release_alt_switches: true,
        });
        session.open([SwitchTask::new(1, 10, "Editor", "editor")], None);
        let _effect = session.handle_input(InputAction::AppendSearchCharacter('z'));
        assert_eq!(session.switcher().visible_task_count(), 0);
        assert!(session.filter_visible());
        let _effect = session.handle_input(InputAction::Navigate(1));
        assert!(session.filter_visible());
        session.clear_filter();
        assert!(!session.filter_visible());
        assert_eq!(session.switcher().visible_task_count(), 1);
    }

    #[test]
    #[allow(
        clippy::cast_precision_loss,
        reason = "fixture rectangles are bounded monitor pixel coordinates"
    )]
    fn footer_stays_inside_the_panel_below_rows_and_continuation_arrows() {
        for work_area in [
            Rect::new(0, 0, 1_920, 1_080),
            Rect::new(-2_560, -120, 2_560, 1_400),
            Rect::new(0, 0, 1_080, 1_920),
        ] {
            for dpi in [96, 120, 144, 168, 192, 288] {
                for compact in [false, true] {
                    let panel = overlay_bounds(work_area);
                    let bounds = popup_bounds(panel, dpi, compact);
                    assert!(bounds.is_some());
                    let Some(bounds) = bounds else { continue };
                    assert!(panel.contains(bounds));
                    let layout = for_compact_list(compact);
                    let factor = layout_scale(dpi);
                    let height = list_height(panel.height as f32 / factor, layout, true);
                    let rows = layout.visible_row_count(height);
                    assert!(rows <= layout.visible_row_count(panel.height as f32 / factor));
                    let last_row = layout.row_bounds(rows - 1, 500.0);
                    let arrows = layout.scroll_indicators(height, 500.0, 1..rows + 1, rows + 2);
                    let Some(bottom_arrow) = arrows[1] else {
                        panic!("hidden tasks below")
                    };
                    let footer_top = (bounds.y - panel.y) as f32 / factor;
                    assert!(last_row.bottom < bottom_arrow.top - 0.75);
                    assert!(bottom_arrow.bottom + 0.75 < footer_top);
                    assert_eq!(layout.visible_row_at(height, footer_top + 16.0), None);
                }
            }
        }
    }

    #[test]
    fn edge_pinned_panels_retain_the_footer_and_idle_lists_reclaim_its_space() {
        for compact in [false, true] {
            let panel = Rect::new(0, 0, 900, 600);
            assert!(popup_bounds(panel, 96, compact).is_some());
            let layout = for_compact_list(compact);
            assert!((list_height(600.0, layout, false) - 600.0).abs() < f32::EPSILON);
            let active = list_height(600.0, layout, true);
            assert!((active + layout.outer_padding + FOOTER_HEIGHT - 600.0).abs() < f32::EPSILON);
            assert_eq!(layout.visible_row_count(list_height(20.0, layout, true)), 0);
        }
    }
}
