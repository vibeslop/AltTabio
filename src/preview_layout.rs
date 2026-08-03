//! Deterministic DWM preview placement calculations in physical pixels.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    #[must_use]
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    #[must_use]
    pub const fn new(left: f64, top: f64, width: f64, height: f64) -> Self {
        Self {
            left,
            top,
            width,
            height,
        }
    }

    #[must_use]
    pub const fn size(self) -> Size {
        Size::new(self.width, self.height)
    }

    #[must_use]
    pub const fn right(self) -> f64 {
        self.left + self.width
    }

    #[must_use]
    pub const fn bottom(self) -> f64 {
        self.top + self.height
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    #[must_use]
    pub fn intersection(self, other: Self) -> Self {
        let left = self.left.max(other.left);
        let top = self.top.max(other.top);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Self::new(left, top, right - left, bottom - top)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DesktopWindowPreviewLayout {
    pub desktop_destination: Rect,
    pub window_destination: Rect,
    pub window_source: Rect,
}

#[must_use]
pub fn fit(bounds: Rect, content_size: Size) -> Rect {
    if bounds.is_empty() || content_size.width <= 0.0 || content_size.height <= 0.0 {
        return Rect::default();
    }

    let scale = (bounds.width / content_size.width).min(bounds.height / content_size.height);
    let width = content_size.width * scale;
    let height = content_size.height * scale;
    Rect::new(
        bounds.left + ((bounds.width - width) / 2.0),
        bounds.top + ((bounds.height - height) / 2.0),
        width,
        height,
    )
}

#[must_use]
pub fn calculate(
    host_bounds: Rect,
    desktop_bounds: Rect,
    window_bounds: Rect,
    source_size: Size,
    restored_window_bounds: Option<Rect>,
) -> DesktopWindowPreviewLayout {
    let desktop_destination = fit(host_bounds, desktop_bounds.size());
    if desktop_destination.is_empty() {
        return DesktopWindowPreviewLayout::default();
    }

    if window_bounds.is_empty() || source_size.width <= 0.0 || source_size.height <= 0.0 {
        return fallback_layout(desktop_destination, source_size);
    }

    let mut positioned_window_bounds = window_bounds;
    let mut visible_window = desktop_bounds.intersection(positioned_window_bounds);
    if visible_window.is_empty()
        && let Some(restored) = restored_window_bounds.filter(|bounds| !bounds.is_empty())
    {
        positioned_window_bounds = restored;
        visible_window = desktop_bounds.intersection(positioned_window_bounds);
    }
    if visible_window.is_empty() {
        return fallback_layout(desktop_destination, source_size);
    }

    let scale = desktop_destination.width / desktop_bounds.width;
    let window_destination = Rect::new(
        desktop_destination.left + ((visible_window.left - desktop_bounds.left) * scale),
        desktop_destination.top + ((visible_window.top - desktop_bounds.top) * scale),
        visible_window.width * scale,
        visible_window.height * scale,
    );

    let source_scale_x = source_size.width / positioned_window_bounds.width;
    let source_scale_y = source_size.height / positioned_window_bounds.height;
    let window_source = Rect::new(
        (visible_window.left - positioned_window_bounds.left) * source_scale_x,
        (visible_window.top - positioned_window_bounds.top) * source_scale_y,
        visible_window.width * source_scale_x,
        visible_window.height * source_scale_y,
    );

    DesktopWindowPreviewLayout {
        desktop_destination,
        window_destination,
        window_source,
    }
}

fn fallback_layout(desktop_destination: Rect, source_size: Size) -> DesktopWindowPreviewLayout {
    DesktopWindowPreviewLayout {
        desktop_destination,
        window_destination: fit(desktop_destination, source_size),
        window_source: Rect::new(0.0, 0.0, source_size.width, source_size.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_centers_content_without_distorting_aspect_ratio() {
        assert_eq!(
            fit(Rect::new(0.0, 0.0, 800.0, 800.0), Size::new(1600.0, 900.0)),
            Rect::new(0.0, 175.0, 800.0, 450.0)
        );
    }

    #[test]
    fn desktop_layout_preserves_negative_monitor_coordinates() {
        let layout = calculate(
            Rect::new(10.0, 20.0, 960.0, 540.0),
            Rect::new(-1920.0, 0.0, 1920.0, 1080.0),
            Rect::new(-1440.0, 270.0, 960.0, 540.0),
            Size::new(960.0, 540.0),
            None,
        );

        assert_eq!(
            layout.desktop_destination,
            Rect::new(10.0, 20.0, 960.0, 540.0)
        );
        assert_eq!(
            layout.window_destination,
            Rect::new(250.0, 155.0, 480.0, 270.0)
        );
        assert_eq!(layout.window_source, Rect::new(0.0, 0.0, 960.0, 540.0));
    }

    #[test]
    fn desktop_layout_on_positive_secondary_monitor_uses_client_local_destination() {
        let layout = calculate(
            Rect::new(620.0, 24.0, 900.0, 620.0),
            Rect::new(1920.0, -120.0, 2560.0, 1440.0),
            Rect::new(2560.0, 240.0, 1280.0, 720.0),
            Size::new(1280.0, 720.0),
            None,
        );

        assert_eq!(
            layout.desktop_destination,
            Rect::new(620.0, 80.875, 900.0, 506.25)
        );
        assert_eq!(
            layout.window_destination,
            Rect::new(845.0, 207.4375, 450.0, 253.125)
        );
        assert_eq!(layout.window_source, Rect::new(0.0, 0.0, 1280.0, 720.0));
    }

    #[test]
    fn default_overlay_host_keeps_the_full_desktop_preview_proportions() {
        let layout = calculate(
            Rect::new(627.0, 38.0, 1_383.0, 1_076.0),
            Rect::new(0.0, 0.0, 1_920.0, 1_080.0),
            Rect::new(400.0, 200.0, 1_000.0, 600.0),
            Size::new(1_000.0, 600.0),
            None,
        );

        assert_eq!(
            layout.desktop_destination,
            Rect::new(627.0, 187.03125, 1_383.0, 777.9375)
        );
        assert_eq!(
            layout.window_destination,
            Rect::new(915.125, 331.09375, 720.3125, 432.1875)
        );
        assert_eq!(layout.window_source, Rect::new(0.0, 0.0, 1_000.0, 600.0));
    }

    #[test]
    fn desktop_layout_clips_window_and_maps_source_rectangle() {
        let layout = calculate(
            Rect::new(0.0, 0.0, 960.0, 540.0),
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            Rect::new(-200.0, 100.0, 1000.0, 500.0),
            Size::new(1000.0, 500.0),
            None,
        );

        assert_eq!(
            layout.window_destination,
            Rect::new(0.0, 50.0, 400.0, 250.0)
        );
        assert_eq!(layout.window_source, Rect::new(200.0, 0.0, 800.0, 500.0));
    }

    #[test]
    fn minimized_window_uses_restored_placement_when_current_bounds_are_off_desktop() {
        let layout = calculate(
            Rect::new(0.0, 0.0, 960.0, 540.0),
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            Rect::new(-32_000.0, -32_000.0, 800.0, 600.0),
            Size::new(800.0, 600.0),
            Some(Rect::new(100.0, 200.0, 800.0, 600.0)),
        );

        assert_eq!(
            layout.window_destination,
            Rect::new(50.0, 100.0, 400.0, 300.0)
        );
        assert_eq!(layout.window_source, Rect::new(0.0, 0.0, 800.0, 600.0));
    }
}
