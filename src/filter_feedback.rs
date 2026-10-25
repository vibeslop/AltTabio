//! The typed query's short-lived feedback and its placement over the panel's top edge.

use crate::dialog_layout::{Rect, scale};
use crate::overlay_layout::{for_compact_list, layout_dpi, layout_scale};
use std::time::{Duration, Instant};

pub const HIDE_DELAY: Duration = Duration::from_millis(1_500);
pub const BACKGROUND_OPACITY: f32 = 0.5;

#[derive(Debug, Default)]
pub struct FilterFeedback {
    until: Option<Instant>,
}

impl FilterFeedback {
    pub fn edited(&mut self, query: &str, now: Instant) {
        self.until = (!query.is_empty()).then(|| now + HIDE_DELAY);
    }

    #[must_use]
    pub fn is_visible(&self, now: Instant) -> bool {
        self.until.is_some_and(|until| now < until)
    }

    pub fn hide(&mut self) {
        self.until = None;
    }
}

/// The popup shares the panel's DPI cap and overlaps only its existing top padding. When a
/// manually moved panel leaves no room above it, omit the feedback instead of covering a row.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "monitor dimensions and rounded on-screen coordinates fit i32 and f32"
)]
pub fn popup_bounds(
    panel: Rect,
    work_area: Rect,
    window_dpi: u32,
    compact_list: bool,
) -> Option<Rect> {
    let dpi = u32::from(layout_dpi(window_dpi));
    let factor = layout_scale(window_dpi);
    let layout = for_compact_list(compact_list);
    let padding = (layout.outer_padding * factor).round() as i32;
    let list_width =
        (layout.list_width(panel.width as f32 / factor, factor) * factor).round() as i32;
    let width = scale(240, dpi)
        .min(list_width.saturating_sub(padding.saturating_mul(2)))
        .min(work_area.width);
    let height = scale(36, dpi);
    if width < scale(100, dpi) || work_area.height < height {
        return None;
    }
    let x = panel
        .x
        .saturating_add(padding)
        .clamp(work_area.x, work_area.right().saturating_sub(width));
    let y = panel.y.saturating_sub(height / 2).max(work_area.y);
    let bounds = Rect::new(x, y, width, height);
    (bounds.bottom() <= panel.y.saturating_add(padding) && bounds.bottom() <= work_area.bottom())
        .then_some(bounds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay_window::overlay_bounds;

    #[test]
    fn editing_renews_feedback_but_a_stale_timer_cannot_hide_new_text() {
        let start = Instant::now();
        let mut feedback = FilterFeedback::default();
        assert!(!feedback.is_visible(start));
        feedback.edited("f", start);
        assert!(feedback.is_visible(start + HIDE_DELAY / 2));
        feedback.edited("fi", start + HIDE_DELAY / 2);
        assert!(feedback.is_visible(start + HIDE_DELAY));
        assert!(!feedback.is_visible(start + HIDE_DELAY * 2));
    }

    #[test]
    fn clearing_and_dismissing_hide_feedback_immediately() {
        let now = Instant::now();
        let mut feedback = FilterFeedback::default();
        feedback.edited("file", now);
        feedback.edited("", now);
        assert!(!feedback.is_visible(now));
        feedback.edited("file", now);
        feedback.hide();
        assert!(!feedback.is_visible(now));
    }

    #[test]
    fn capsule_overlaps_the_edge_without_touching_any_row() {
        for work_area in [
            Rect::new(0, 0, 1_920, 1_080),
            Rect::new(-2_560, -120, 2_560, 1_400),
            Rect::new(0, 0, 1_080, 1_920),
        ] {
            for dpi in [96, 120, 144, 168, 192, 288] {
                for compact in [false, true] {
                    let panel = overlay_bounds(work_area);
                    let bounds = popup_bounds(panel, work_area, dpi, compact);
                    assert!(bounds.is_some());
                    let Some(bounds) = bounds else { continue };
                    assert!(work_area.contains(bounds));
                    assert!(bounds.y < panel.y && bounds.bottom() > panel.y);
                    let layout = for_compact_list(compact);
                    let first_row = layout.row_bounds(0, 500.0);
                    assert!(
                        f64::from(bounds.bottom())
                            <= f64::from(panel.y)
                                + f64::from(first_row.top * layout_scale(dpi))
                                + 0.5
                    );
                }
            }
        }
    }

    #[test]
    fn an_edge_pinned_panel_never_pushes_the_capsule_over_a_row() {
        let area = Rect::new(0, 0, 1_920, 1_080);
        assert_eq!(
            popup_bounds(Rect::new(0, 0, 900, 600), area, 96, true),
            None
        );
    }
}
