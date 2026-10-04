//! Placement of the About dialog's content in physical pixels.

use crate::dialog_layout::{Rect, scale};

pub const CLIENT_WIDTH: i32 = 430;
pub const CLIENT_HEIGHT: i32 = 344;
const CONTENT_HORIZONTAL_MARGIN: i32 = 94;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AboutLayout {
    pub icon: Rect,
    pub title: Rect,
    pub version: Rect,
    pub description: Rect,
    pub repository: Rect,
    pub copyright: Rect,
    pub license: Rect,
    pub footer: Rect,
    pub close_button: Rect,
}

impl AboutLayout {
    #[must_use]
    pub fn new(client_width: i32, client_height: i32, dpi: u32) -> Self {
        let footer_height = scale(76, dpi).min(client_height);
        let footer_top = client_height.saturating_sub(footer_height);
        let button_width = scale(116, dpi).min(client_width);
        let button_height = scale(42, dpi).min(footer_height);
        let right_padding = scale(20, dpi);
        let button_y = footer_top.saturating_add(
            footer_height
                .saturating_sub(button_height)
                .saturating_div(2),
        );
        let content_left = scale(CONTENT_HORIZONTAL_MARGIN, dpi);
        let content_width = client_width.saturating_sub(content_left.saturating_mul(2));
        Self {
            icon: Rect::new(
                scale(20, dpi),
                scale(18, dpi),
                scale(48, dpi),
                scale(48, dpi),
            ),
            title: Rect::new(content_left, scale(20, dpi), content_width, scale(38, dpi)),
            version: Rect::new(content_left, scale(68, dpi), content_width, scale(28, dpi)),
            description: Rect::new(content_left, scale(104, dpi), content_width, scale(28, dpi)),
            repository: Rect::new(content_left, scale(140, dpi), content_width, scale(30, dpi)),
            copyright: Rect::new(content_left, scale(184, dpi), content_width, scale(28, dpi)),
            license: Rect::new(content_left, scale(214, dpi), content_width, scale(28, dpi)),
            footer: Rect::new(0, footer_top, client_width, footer_height),
            close_button: Rect::new(
                client_width
                    .saturating_sub(right_padding)
                    .saturating_sub(button_width),
                button_y,
                button_width,
                button_height,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog_layout::BASE_DPI;

    #[test]
    fn about_layout_keeps_the_link_and_close_button_inside_the_client() {
        let layout = AboutLayout::new(CLIENT_WIDTH, CLIENT_HEIGHT, BASE_DPI);

        assert!(layout.repository.x >= 0);
        assert!(layout.repository.x + layout.repository.width <= CLIENT_WIDTH);
        assert!(layout.close_button.x >= 0);
        assert!(layout.close_button.x + layout.close_button.width <= CLIENT_WIDTH);
        assert!(layout.close_button.y >= layout.footer.y);
        assert!(layout.close_button.y + layout.close_button.height <= CLIENT_HEIGHT);
    }

    #[test]
    fn about_layout_uses_compact_vertical_text_spacing() {
        let layout = AboutLayout::new(CLIENT_WIDTH, CLIENT_HEIGHT, BASE_DPI);

        assert_eq!(
            layout.version.y - (layout.title.y + layout.title.height),
            10
        );
        assert_eq!(
            layout.description.y - (layout.version.y + layout.version.height),
            8
        );
        assert_eq!(
            layout.repository.y - (layout.description.y + layout.description.height),
            8
        );
        assert_eq!(
            layout.copyright.y - (layout.repository.y + layout.repository.height),
            14
        );
        assert_eq!(
            layout.license.y - (layout.copyright.y + layout.copyright.height),
            2
        );
    }

    #[test]
    fn about_text_column_has_equal_horizontal_margins() {
        let layout = AboutLayout::new(CLIENT_WIDTH, CLIENT_HEIGHT, BASE_DPI);

        for text in [
            layout.title,
            layout.version,
            layout.description,
            layout.repository,
            layout.copyright,
            layout.license,
        ] {
            assert_eq!(text.x, CLIENT_WIDTH - (text.x + text.width));
        }
    }
}
