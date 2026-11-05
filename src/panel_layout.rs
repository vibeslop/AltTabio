//! The macOS switcher panel's geometry: a strip of app icons, the selected app's windows under
//! it, and, when previews are on, the selected window beside them. Points, top-left origin.

use crate::app_switcher::AppEntry;
use crate::preview_layout::Rect;
use crate::process_identity::ProcessIdentity;

pub const CORNER_RADIUS: f64 = 24.0;
pub const PADDING: f64 = 12.0;
/// Plates and the preview well sit one padding inside the panel, so their corners follow the
/// panel's with the padding taken off.
pub const PLATE_RADIUS: f64 = CORNER_RADIUS - PADDING;
const TILE_MAX: f64 = 64.0;
/// Many apps shrink the tiles down to this before the strip starts scrolling.
const TILE_MIN: f64 = 44.0;
/// The icon's share of its tile; the rest is the selection plate showing around it.
pub const ICON_SHARE: f64 = 0.75;
/// The row under the strip that names the selected app.
pub const NAME_HEIGHT: f64 = 22.0;
const LIST_GAP: f64 = 6.0;
pub const ROW_HEIGHT: f64 = 36.0;
/// The list grows to this many rows; longer window lists scroll.
const MAX_ROWS: usize = 8;
const MIN_CONTENT_WIDTH: f64 = 456.0;
/// Row text lines up with the visible edge of a full-size icon above it: 8pt of tile around the
/// icon plus the transparent margin macOS app icons carry inside their image, about a tenth of it.
pub const TEXT_INSET: f64 = 12.0;
/// The column of window numbers before the titles.
pub const NUMBER_WIDTH: f64 = 20.0;
pub const CLOSE_SIZE: f64 = 24.0;
pub const STATE_GAP: f64 = 12.0;
const PREVIEW_WIDTH: f64 = 400.0;
const PREVIEW_MIN_HEIGHT: f64 = 250.0;
const PREVIEW_GAP: f64 = 12.0;
/// The capture sits this far inside the preview well, rounded to the well's radius minus it.
pub const PREVIEW_INSET: f64 = 8.0;
const LIST_WIDTH_BESIDE_PREVIEW: f64 = 340.0;
/// The windows the number keys 1 to 9 pick carry their number beside them.
const NUMBERED_ROWS: usize = 9;

/// Where a window is when it is not plainly on the current desktop.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WindowState {
    #[default]
    Normal,
    Minimized,
    Hidden,
    OtherDesktop,
}

impl WindowState {
    #[must_use]
    pub const fn label(self) -> Option<&'static str> {
        match self {
            Self::Normal => None,
            Self::Minimized => Some("Minimized"),
            Self::Hidden => Some("Hidden"),
            Self::OtherDesktop => Some("Other desktop"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Hit {
    Tile(usize),
    Row(usize),
    CloseButton(usize),
}

/// The panel's geometry for one session: its size, the tile edge, and how many tiles and rows
/// it holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub width: f64,
    pub height: f64,
    pub tile: f64,
    /// How many tiles the strip shows at once.
    pub tile_slots: usize,
    /// How many window rows the list shows at once.
    pub row_slots: usize,
    preview: bool,
}

impl Layout {
    /// The panel for `apps` apps whose longest window list has `windows` entries, no larger than
    /// `bounds`. The strip sets the width and the longest list the height, so neither changes
    /// while the selection moves.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "app and row counts are small, and the floored quotients are positive"
    )]
    pub fn new(apps: usize, windows: usize, preview: bool, bounds: (f64, f64)) -> Self {
        let apps = apps.max(1);
        let bounds = (bounds.0.max(0.0).floor(), bounds.1.max(0.0).floor());
        let maximum = (bounds.0 - PADDING * 2.0).max(0.0);
        // Keep useful list and preview widths; the list takes the full panel below this limit.
        let preview = preview && maximum >= LIST_WIDTH_BESIDE_PREVIEW + PREVIEW_GAP + PREVIEW_WIDTH;
        let minimum = if preview {
            LIST_WIDTH_BESIDE_PREVIEW + PREVIEW_GAP + PREVIEW_WIDTH
        } else {
            MIN_CONTENT_WIDTH
        }
        .min(maximum);
        let content = (apps as f64 * TILE_MAX).clamp(minimum, maximum);
        // Leave space for one row even when the display is shorter than the normal minimum.
        let tile_max = TILE_MAX
            .min(content)
            .min((bounds.1 - PADDING * 2.0 - NAME_HEIGHT - LIST_GAP - ROW_HEIGHT).max(0.0));
        let tile = (content / apps as f64)
            .clamp(TILE_MIN.min(tile_max), tile_max)
            .floor();
        let tile_slots = if tile > 0.0 {
            ((content / tile).floor() as usize).clamp(1, apps)
        } else {
            0
        };
        let list = windows.clamp(1, MAX_ROWS) as f64 * ROW_HEIGHT;
        let list = if preview {
            list.max(PREVIEW_MIN_HEIGHT)
        } else {
            list
        };
        let above = PADDING + tile + NAME_HEIGHT + LIST_GAP;
        let height = (above + list + PADDING).min(bounds.1);
        let row_slots = ((height - above - PADDING).max(0.0) / ROW_HEIGHT).floor() as usize;
        Self {
            width: (content + PADDING * 2.0).min(bounds.0),
            height,
            tile,
            tile_slots,
            row_slots,
            preview,
        }
    }

    #[must_use]
    pub const fn size(&self) -> (f64, f64) {
        (self.width, self.height)
    }

    #[must_use]
    pub fn content_width(&self) -> f64 {
        (self.width - PADDING * 2.0).max(0.0)
    }

    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "tile slots are small on-screen counts"
    )]
    pub fn tile_rect(&self, slot: usize) -> Rect {
        Rect {
            left: PADDING + slot as f64 * self.tile,
            top: PADDING,
            width: self.tile,
            height: self.tile,
        }
    }

    #[must_use]
    pub fn name_top(&self) -> f64 {
        PADDING + self.tile
    }

    #[must_use]
    fn list_rect(&self) -> Rect {
        let top = self.name_top() + NAME_HEIGHT + LIST_GAP;
        let width = if self.preview {
            self.content_width() - PREVIEW_GAP - PREVIEW_WIDTH
        } else {
            self.content_width()
        };
        Rect {
            left: PADDING,
            top,
            width,
            height: (self.height - PADDING - top).max(0.0),
        }
    }

    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "row indices are small on-screen counts"
    )]
    pub fn row_rect(&self, row: usize) -> Rect {
        let list = self.list_rect();
        Rect {
            top: list.top + row as f64 * ROW_HEIGHT,
            height: ROW_HEIGHT,
            ..list
        }
    }

    /// The size a capture fills inside the preview well, for sizing the captures.
    #[must_use]
    pub fn preview_size(&self) -> Option<(f64, f64)> {
        self.preview_rect().map(|well| {
            let area = image_area(well);
            (area.width, area.height)
        })
    }

    #[must_use]
    pub fn preview_rect(&self) -> Option<Rect> {
        self.preview.then(|| {
            let list = self.list_rect();
            Rect {
                left: list.right() + PREVIEW_GAP,
                width: PREVIEW_WIDTH,
                ..list
            }
        })
    }

    /// Where the pointer is, given how many tiles and rows are drawn and which row carries the
    /// close button.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the offsets are checked nonnegative and map to small slot and row indices"
    )]
    pub fn hit(
        &self,
        tiles: usize,
        rows: usize,
        close_row: Option<usize>,
        x: f64,
        y: f64,
    ) -> Option<Hit> {
        let strip = Rect {
            left: PADDING,
            top: PADDING,
            width: self.content_width(),
            height: self.tile,
        };
        if strip.contains(x, y) {
            let slot = ((x - PADDING) / self.tile) as usize;
            return (slot < tiles).then_some(Hit::Tile(slot));
        }
        let list = self.list_rect();
        if !list.contains(x, y) {
            return None;
        }
        let row = ((y - list.top) / ROW_HEIGHT) as usize;
        if row >= rows {
            return None;
        }
        if close_row == Some(row) && close_button_rect(self.row_rect(row)).contains(x, y) {
            return Some(Hit::CloseButton(row));
        }
        Some(Hit::Row(row))
    }
}

/// Fits a panel frame inside the display's visible rectangle. Both rectangles use the same
/// coordinate axes; the origin may be on any display. Clamp after rounding to keep fractional
/// display edges inside the bounds too.
#[must_use]
pub fn constrain_frame(frame: Rect, bounds: Rect) -> Rect {
    let width = frame.width.max(0.0).min(bounds.width.max(0.0));
    let height = frame.height.max(0.0).min(bounds.height.max(0.0));
    Rect {
        left: frame
            .left
            .round()
            .max(bounds.left)
            .min(bounds.right() - width),
        top: frame
            .top
            .round()
            .max(bounds.top)
            .min(bounds.bottom() - height),
        width,
        height,
    }
}

/// The most apps and the longest window list a session has listed. The panel is sized for
/// them, so it never shrinks under the pointer while it shows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Extent {
    pub apps: usize,
    pub windows: usize,
}

impl Extent {
    pub fn widen(&mut self, apps: &[AppEntry]) {
        let windows = apps.iter().map(|app| app.windows.len()).max().unwrap_or(0);
        self.apps = self.apps.max(apps.len());
        self.windows = self.windows.max(windows);
    }
}

/// Which of the selected app's windows the list draws, and the notes around them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ListRows {
    /// The first window drawn.
    pub start: usize,
    pub count: usize,
    pub selected_row: Option<usize>,
    /// Drawn where the rows go when the app has no window.
    pub empty_note: Option<String>,
    /// Drawn in the slot after the last row when the list scrolls.
    pub more_note: Option<String>,
}

impl ListRows {
    /// Fits `total` windows into `slots` rows, moving the previous `start` only as far as it
    /// takes to show `selected`.
    #[must_use]
    pub fn new(start: usize, selected: Option<usize>, total: usize, slots: usize) -> Self {
        // A list longer than the panel gives its last slot to the count of the rest.
        let fits = if total > slots {
            slots.saturating_sub(1).max(1)
        } else {
            slots
        };
        let start = scroll_into_view(start, selected.unwrap_or_default(), total, fits);
        let count = total.saturating_sub(start).min(fits);
        Self {
            start,
            count,
            selected_row: selected
                .and_then(|index| index.checked_sub(start))
                .filter(|row| *row < count),
            empty_note: (total == 0).then(|| "No open windows".to_owned()),
            more_note: (count < slots)
                .then(|| more_note(total, start, count))
                .flatten(),
        }
    }
}

/// The number beside the window at `index` in its app's list, the key that picks it.
#[must_use]
pub const fn row_number(index: usize) -> Option<usize> {
    if index < NUMBERED_ROWS {
        Some(index + 1)
    } else {
        None
    }
}

/// The geometry and scroll positions of the last frame drawn.
#[derive(Clone, Copy, Debug)]
pub struct Shown {
    pub layout: Layout,
    pub app: Option<ProcessIdentity>,
    pub tile_start: usize,
    pub tiles: usize,
    pub row_start: usize,
    pub rows: usize,
    pub selected_row: Option<usize>,
}

impl Shown {
    #[must_use]
    pub fn hit(&self, x: f64, y: f64) -> Option<Hit> {
        self.layout
            .hit(self.tiles, self.rows, self.selected_row, x, y)
    }
}

/// Where a capture goes inside the preview well.
#[must_use]
pub fn image_area(well: Rect) -> Rect {
    Rect {
        left: well.left + PREVIEW_INSET,
        top: well.top + PREVIEW_INSET,
        width: (well.width - PREVIEW_INSET * 2.0).max(0.0),
        height: (well.height - PREVIEW_INSET * 2.0).max(0.0),
    }
}

#[must_use]
pub fn close_button_rect(row: Rect) -> Rect {
    let inset = (row.height - CLOSE_SIZE) / 2.0;
    Rect {
        left: row.right() - inset - CLOSE_SIZE,
        top: row.top + inset,
        width: CLOSE_SIZE,
        height: CLOSE_SIZE,
    }
}

/// The first of `shown` entries to draw out of `total` so that `selected` is in view, moving
/// the previous start only as far as needed. A pointer resting on a row therefore never makes
/// the list scroll under it.
#[must_use]
pub fn scroll_into_view(start: usize, selected: usize, total: usize, shown: usize) -> usize {
    let start = start.min(total.saturating_sub(shown));
    if selected < start {
        selected
    } else if shown > 0 && selected >= start + shown {
        selected + 1 - shown
    } else {
        start
    }
}

/// The note in the slot under `shown` rows drawn from `start` out of `total`. It sits below the
/// rows, so it counts the windows below them; once the list has scrolled to its end, every
/// hidden window is above.
#[must_use]
pub fn more_note(total: usize, start: usize, shown: usize) -> Option<String> {
    let below = total.saturating_sub(start + shown);
    if below > 0 {
        Some(format!("{below} more"))
    } else if start > 0 {
        Some(format!("{start} more above"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (f64, f64) = (1512.0, 900.0);

    #[test]
    fn the_strip_sets_the_width_and_the_longest_list_the_height() {
        let few = Layout::new(3, 2, false, SCREEN);
        assert!((few.width - (MIN_CONTENT_WIDTH + PADDING * 2.0)).abs() < f64::EPSILON);
        assert_eq!(few.row_slots, 2);
        assert_eq!(few.tile_slots, 3);

        let many = Layout::new(12, 30, false, SCREEN);
        assert!((many.width - (12.0 * TILE_MAX + PADDING * 2.0)).abs() < f64::EPSILON);
        assert_eq!(many.row_slots, MAX_ROWS);
        assert!(many.height > few.height);
    }

    #[test]
    fn crowded_strips_shrink_their_tiles_and_then_scroll() {
        let crowded = Layout::new(24, 1, false, (1000.0, 700.0));
        assert!(crowded.width <= 1000.0);
        assert!(crowded.tile < TILE_MAX && crowded.tile >= TILE_MIN);
        assert!(crowded.tile_slots < 24);
    }

    #[test]
    fn a_short_display_limits_the_rows() {
        let short = Layout::new(2, 20, false, (1000.0, 300.0));
        assert!(short.height <= 300.0);
        assert!(short.row_slots < MAX_ROWS);
        assert!(short.row_slots >= 1);
    }

    #[test]
    fn a_narrow_display_omits_the_preview_and_keeps_the_list_and_tiles_inside() {
        for width in [320.0, 479.9, 640.0, 775.9] {
            let layout = Layout::new(30, 20, true, (width, 600.0));
            assert!(layout.width <= width);
            assert!(layout.preview_rect().is_none());
            assert!(layout.tile_slots > 0 && layout.tile_slots < 30);
            let last_tile = layout.tile_rect(layout.tile_slots - 1);
            assert!(last_tile.right() <= layout.width - PADDING);
            let row = layout.row_rect(0);
            assert!(row.right() <= layout.width - PADDING);
            let close = close_button_rect(row);
            assert!(close.left >= row.left && close.right() <= row.right());
            assert_eq!(
                layout.hit(
                    layout.tile_slots,
                    1,
                    Some(0),
                    close.left + 1.0,
                    close.top + 1.0
                ),
                Some(Hit::CloseButton(0))
            );
        }
    }

    #[test]
    fn the_preview_returns_as_soon_as_both_columns_fit() {
        let width = PADDING * 2.0 + LIST_WIDTH_BESIDE_PREVIEW + PREVIEW_GAP + PREVIEW_WIDTH;
        let layout = Layout::new(2, 1, true, (width, 600.0));
        let Some(preview) = layout.preview_rect() else {
            panic!("the preview should fit at its width threshold");
        };
        assert!((preview.right() - (width - PADDING)).abs() < f64::EPSILON);
        assert!((layout.list_rect().width - LIST_WIDTH_BESIDE_PREVIEW).abs() < f64::EPSILON);
    }

    #[test]
    fn a_fitting_frame_keeps_its_position_and_size() {
        let requested = Rect::new(100.0, 300.0, 600.0, 400.0);
        let resized = constrain_frame(requested, Rect::new(0.0, 0.0, 1000.0, 900.0));
        assert_eq!(resized, requested);
    }

    #[test]
    fn oversized_frames_fit_displays_with_positive_and_negative_origins() {
        for bounds in [
            Rect::new(-1280.0, -200.0, 640.0, 480.0),
            Rect::new(1920.25, -100.75, 640.5, 480.5),
        ] {
            let fitted = constrain_frame(Rect::new(0.0, 0.0, 1000.0, 900.0), bounds);
            assert_eq!(fitted, bounds);
            let shifted = constrain_frame(
                Rect::new(bounds.right() - 10.0, bounds.bottom() - 10.0, 320.0, 240.0),
                bounds,
            );
            assert!(shifted.left >= bounds.left && shifted.right() <= bounds.right());
            assert!(shifted.top >= bounds.top && shifted.bottom() <= bounds.bottom());
        }
    }

    #[test]
    fn a_display_change_repositions_a_frame_even_when_its_size_still_fits() {
        let moved = constrain_frame(
            Rect::new(-100.0, -50.0, 480.0, 300.0),
            Rect::new(100.0, 200.0, 640.0, 480.0),
        );
        assert_eq!(moved, Rect::new(100.0, 200.0, 480.0, 300.0));
    }

    #[test]
    fn fractional_and_short_bounds_are_hard_limits_with_or_without_previews() {
        for bounds in [(300.8, 155.8), (700.6, 120.6), (900.9, 250.9)] {
            for preview in [false, true] {
                let layout = Layout::new(3, 30, preview, bounds);
                assert!(layout.width <= bounds.0);
                assert!(layout.height <= bounds.1);
                assert!(layout.row_slots >= 1);
                assert!(layout.tile_slots >= 1);
                assert!(layout.row_rect(layout.row_slots - 1).bottom() <= layout.height - PADDING);
            }
        }
    }

    #[test]
    fn the_preview_sits_beside_the_list_and_sets_a_minimum_height() {
        let layout = Layout::new(2, 1, true, SCREEN);
        let list = layout.list_rect();
        let preview = layout.preview_rect();

        assert!(list.height >= PREVIEW_MIN_HEIGHT);
        assert_eq!(
            preview.map(|area| (area.left, area.top, area.height)),
            Some((list.right() + PREVIEW_GAP, list.top, list.height))
        );
        assert!(Layout::new(2, 1, false, SCREEN).preview_rect().is_none());
    }

    #[test]
    fn captures_are_sized_to_the_area_inside_the_well() {
        let layout = Layout::new(2, 1, true, SCREEN);
        let well = layout.preview_rect().map(|well| (well.width, well.height));

        assert_eq!(
            layout.preview_size(),
            well.map(|(width, height)| (width - PREVIEW_INSET * 2.0, height - PREVIEW_INSET * 2.0))
        );
    }

    #[test]
    fn hits_find_tiles_rows_and_the_close_button() {
        let layout = Layout::new(3, 4, false, SCREEN);
        let second_tile = layout.tile_rect(1);
        assert_eq!(
            layout.hit(3, 4, None, second_tile.left + 1.0, second_tile.top + 1.0),
            Some(Hit::Tile(1))
        );
        let row = layout.row_rect(2);
        assert_eq!(
            layout.hit(3, 4, None, row.left + 5.0, row.top + 5.0),
            Some(Hit::Row(2))
        );
        let close = close_button_rect(row);
        assert_eq!(
            layout.hit(3, 4, Some(2), close.left + 2.0, close.top + 2.0),
            Some(Hit::CloseButton(2))
        );
        assert_eq!(
            layout.hit(3, 4, Some(1), close.left + 2.0, close.top + 2.0),
            Some(Hit::Row(2))
        );
        // Slots past the drawn tiles and rows, and the name line, are not targets.
        let fourth_tile = layout.tile_rect(3);
        assert_eq!(
            layout.hit(3, 4, None, fourth_tile.left + 1.0, fourth_tile.top + 1.0),
            None
        );
        assert_eq!(layout.hit(3, 2, None, row.left + 5.0, row.top + 5.0), None);
        assert_eq!(layout.hit(3, 4, None, 100.0, layout.name_top() + 2.0), None);
    }

    #[test]
    fn scrolling_into_view_moves_only_as_far_as_needed() {
        assert_eq!(scroll_into_view(0, 3, 10, 5), 0);
        assert_eq!(scroll_into_view(0, 5, 10, 5), 1);
        assert_eq!(scroll_into_view(4, 2, 10, 5), 2);
        assert_eq!(scroll_into_view(8, 9, 10, 5), 5);
        assert_eq!(scroll_into_view(3, 1, 2, 5), 0);
    }

    #[test]
    fn a_long_list_gives_its_last_slot_to_the_count_of_the_rest() {
        let top = ListRows::new(0, Some(0), 10, 5);
        assert_eq!((top.start, top.count, top.selected_row), (0, 4, Some(0)));
        assert_eq!(top.more_note.as_deref(), Some("6 more"));

        let bottom = ListRows::new(top.start, Some(9), 10, 5);
        assert_eq!(
            (bottom.start, bottom.count, bottom.selected_row),
            (6, 4, Some(3))
        );
        assert_eq!(bottom.more_note.as_deref(), Some("6 more above"));

        let fits = ListRows::new(0, Some(2), 5, 5);
        assert_eq!((fits.start, fits.count, fits.more_note), (0, 5, None));
        // A single slot shows a window rather than only the count.
        let single = ListRows::new(0, Some(1), 3, 1);
        assert_eq!((single.start, single.count, single.more_note), (1, 1, None));
    }

    #[test]
    fn an_app_without_windows_gets_the_empty_note_and_no_selected_row() {
        let empty = ListRows::new(3, None, 0, 5);

        assert_eq!((empty.start, empty.count, empty.selected_row), (0, 0, None));
        assert_eq!(empty.empty_note.as_deref(), Some("No open windows"));
        assert_eq!(empty.more_note, None);
        assert_eq!(ListRows::new(0, Some(0), 1, 5).empty_note, None);
    }

    #[test]
    fn only_the_windows_the_number_keys_reach_are_numbered() {
        assert_eq!(row_number(0), Some(1));
        assert_eq!(row_number(8), Some(9));
        assert_eq!(row_number(9), None);
    }

    #[test]
    fn the_extent_grows_with_the_listing_and_never_shrinks() {
        let app = |id, windows: &[isize]| AppEntry {
            process: ProcessIdentity::new(id, 0),
            name: String::new(),
            windows: windows.to_vec(),
        };
        let mut extent = Extent::default();

        extent.widen(&[app(1, &[10, 11]), app(2, &[])]);
        assert_eq!(
            extent,
            Extent {
                apps: 2,
                windows: 2
            }
        );
        extent.widen(&[app(1, &[10, 11, 12])]);
        assert_eq!(
            extent,
            Extent {
                apps: 2,
                windows: 3
            }
        );
    }

    #[test]
    fn the_more_note_counts_the_windows_on_the_side_they_are_hidden() {
        assert_eq!(more_note(10, 0, 7).as_deref(), Some("3 more"));
        assert_eq!(more_note(10, 2, 7).as_deref(), Some("1 more"));
        assert_eq!(more_note(10, 3, 7).as_deref(), Some("3 more above"));
        assert_eq!(more_note(5, 0, 5), None);
    }
}
