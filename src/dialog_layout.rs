//! DPI scaling and placement math shared by the native dialogs, in physical pixels.

pub const BASE_DPI: u32 = 96;
pub const MIN_DPI: u32 = BASE_DPI / 2;

/// Converts a length authored at 96 DPI to `dpi`, rounding to the nearest pixel.
#[must_use]
pub fn scale(value: i32, dpi: u32) -> i32 {
    let numerator = i64::from(value) * i64::from(dpi) + i64::from(BASE_DPI / 2);
    i32::try_from(numerator / i64::from(BASE_DPI)).unwrap_or(i32::MAX)
}

/// The width of a hairline at `dpi`, never thinner than one pixel.
#[must_use]
pub fn hairline(dpi: u32) -> i32 {
    scale(1, dpi).max(1)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Size {
    pub width: i32,
    pub height: i32,
}

impl Size {
    #[must_use]
    pub const fn new(width: i32, height: i32) -> Self {
        Self { width, height }
    }

    #[must_use]
    pub fn scaled(self, dpi: u32) -> Self {
        Self::new(scale(self.width, dpi), scale(self.height, dpi))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    #[must_use]
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub const fn from_edges(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self::new(
            left,
            top,
            right.saturating_sub(left),
            bottom.saturating_sub(top),
        )
    }

    #[must_use]
    pub const fn right(self) -> i32 {
        self.x.saturating_add(self.width)
    }

    #[must_use]
    pub const fn bottom(self) -> i32 {
        self.y.saturating_add(self.height)
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) const fn contains(self, child: Self) -> bool {
        child.x >= self.x
            && child.y >= self.y
            && child.right() <= self.right()
            && child.bottom() <= self.bottom()
    }

    #[must_use]
    pub const fn contains_point(self, point: Point) -> bool {
        point.x >= self.x && point.x < self.right() && point.y >= self.y && point.y < self.bottom()
    }

    /// Scales the edges rather than the size, so neighbouring rectangles stay flush.
    #[must_use]
    pub fn scaled(self, dpi: u32) -> Self {
        let x = scale(self.x, dpi);
        let y = scale(self.y, dpi);
        Self::new(
            x,
            y,
            scale(self.right(), dpi).saturating_sub(x),
            scale(self.bottom(), dpi).saturating_sub(y),
        )
    }

    /// The origin that centers a `size` window in this area, which may lie at negative
    /// coordinates on a monitor left of or above the primary one.
    #[must_use]
    pub const fn centered(self, size: Size) -> Point {
        Point::new(
            self.x
                .saturating_add(self.width.saturating_sub(size.width) / 2),
            self.y
                .saturating_add(self.height.saturating_sub(size.height) / 2),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_rounds_to_the_nearest_pixel() {
        assert_eq!(scale(10, BASE_DPI), 10);
        assert_eq!(scale(10, 144), 15);
        assert_eq!(scale(1, 120), 1);
        assert_eq!(scale(3, 120), 4);
        assert_eq!(hairline(MIN_DPI), 1);
        assert_eq!(hairline(192), 2);
    }

    #[test]
    fn centering_handles_negative_monitor_coordinates() {
        let origin = Rect::from_edges(-1920, -120, 0, 960).centered(Size::new(560, 709));

        assert_eq!(origin, Point::new(-1240, 65));
    }

    #[test]
    fn scaled_neighbours_stay_flush() {
        let left = Rect::new(0, 0, 37, 10);
        let right = Rect::new(left.right(), 0, 41, 10);

        for dpi in [96, 120, 144, 168, 192] {
            assert_eq!(
                left.scaled(dpi).right(),
                right.scaled(dpi).x,
                "at {dpi} DPI"
            );
        }
    }
}
