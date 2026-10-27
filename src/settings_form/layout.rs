//! Pure geometry and per-category scrolling. All coordinates are physical pixels.
use super::{Category, Control, DialogButton, SELECTOR_WIDTH};
use crate::dialog_layout::{Rect, Size, scale};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettingsLayout {
    pub client: Size,
    pub sidebar: Rect,
    pub footer: Rect,
    pub viewport: Rect,
    pub dpi: u32,
    content_x: i32,
    content_width: i32,
}

impl SettingsLayout {
    #[must_use]
    pub fn for_dpi(dpi: u32) -> Self {
        Self::for_client(
            Size::new(scale(728, dpi), scale(616, dpi)),
            dpi,
            Category::General,
        )
    }

    #[must_use]
    pub fn for_client(client: Size, dpi: u32, category: Category) -> Self {
        let sidebar_width = scale(184, dpi);
        let footer_y = client.height - scale(58, dpi);
        let viewport_y = scale(if category == Category::About { 76 } else { 100 }, dpi);
        let main_width = (client.width - sidebar_width).max(1);
        // Reserve a scrollbar column on every page so switching never shifts the controls.
        let usable_width = main_width - scale(17, dpi);
        let content_width = (usable_width - scale(56, dpi)).min(scale(488, dpi)).max(1);
        Self {
            client,
            dpi,
            sidebar: Rect::new(0, 0, sidebar_width, footer_y),
            footer: Rect::new(0, footer_y, client.width, scale(58, dpi)),
            viewport: Rect::new(
                sidebar_width,
                viewport_y,
                main_width,
                (footer_y - viewport_y).max(1),
            ),
            content_x: (usable_width - content_width) / 2,
            content_width,
        }
    }

    #[must_use]
    pub fn control_rect(&self, control: Control) -> Rect {
        let s = |value| scale(value, self.dpi);
        match control {
            Control::Viewport => self.viewport,
            Control::SidebarTitle => Rect::new(s(24), s(22), s(148), s(30)),
            Control::Navigation(category) => Rect::new(
                s(12),
                if category == Category::About {
                    self.footer.y - s(60)
                } else {
                    s(76 + category as i32 * 46)
                },
                s(160),
                s(42),
            ),
            Control::PageHeading => Rect::new(
                self.viewport.x + self.content_x,
                s(26),
                self.content_width,
                s(30),
            ),
            Control::PageIntro => Rect::new(
                self.viewport.x + self.content_x,
                s(62),
                self.content_width,
                s(19),
            ),
            Control::Button(button) => {
                let width = s(if button == DialogButton::Ok { 120 } else { 78 });
                let right =
                    self.client.width - s(if button == DialogButton::Ok { 20 } else { 149 });
                Rect::new(right - width, self.footer.y + s(12), width, s(34))
            }
            _ => control
                .category()
                .and_then(|category| {
                    self.page_rows(category)
                        .find(|(candidate, _)| *candidate == control)
                        .map(|(_, rect)| rect)
                })
                .unwrap_or_default(),
        }
    }

    fn page_rows(&self, category: Category) -> impl Iterator<Item = (Control, Rect)> + '_ {
        let s = |value| scale(value, self.dpi);
        category
            .controls()
            .iter()
            .copied()
            .scan(0, move |y, control| {
                let height = match control {
                    Control::Section(_) => {
                        if *y != 0 {
                            *y += s(21);
                        }
                        s(22)
                    }
                    Control::Checkbox(option) if !option.help().is_empty() => s(68),
                    Control::AboutIdentity => s(48),
                    Control::AboutDescription => {
                        *y += s(20);
                        s(50)
                    }
                    _ => s(57),
                };
                let row = Rect::new(self.content_x, *y, self.content_width, height);
                if !matches!(control, Control::Label(_)) {
                    *y += height;
                }
                let rect = match control {
                    Control::Selector(_) => Rect::new(
                        row.right() - s(SELECTOR_WIDTH),
                        row.y + s(11),
                        s(SELECTOR_WIDTH),
                        s(35),
                    ),
                    Control::Label(_) => {
                        Rect::new(row.x, row.y, row.width - s(SELECTOR_WIDTH + 20), row.height)
                    }
                    _ => row,
                };
                Some((control, rect))
            })
    }

    #[must_use]
    pub fn content_height(&self, category: Category) -> i32 {
        self.page_rows(category)
            .map(|(_, rect)| rect.bottom())
            .max()
            .unwrap_or_default()
            + scale(20, self.dpi)
    }

    #[must_use]
    pub fn max_scroll(&self, category: Category) -> i32 {
        (self.content_height(category) - self.viewport.height).max(0)
    }
}

#[derive(Clone, Debug, Default)]
pub struct SettingsPages {
    pub category: Category,
    offsets: [i32; Category::ALL.len()],
    wheel_remainder: i32,
}

impl SettingsPages {
    pub fn select(&mut self, category: Category) {
        self.category = category;
        self.wheel_remainder = 0;
    }

    #[must_use]
    pub fn offset(&self) -> i32 {
        self.offsets[self.category as usize]
    }

    pub fn scroll_to(&mut self, offset: i32, layout: &SettingsLayout) {
        self.offsets[self.category as usize] = offset.clamp(0, layout.max_scroll(self.category));
    }

    pub fn wheel(&mut self, delta: i32, pixels_per_notch: i32, layout: &SettingsLayout) {
        let amount = self.wheel_remainder + delta * pixels_per_notch;
        self.wheel_remainder = amount % 120;
        self.scroll_to(self.offset() - amount / 120, layout);
    }

    pub fn reveal(&mut self, row: Rect, layout: &SettingsLayout) {
        let offset = self.offset();
        if row.y < offset {
            self.scroll_to(row.y, layout);
        } else if row.bottom() > offset + layout.viewport.height {
            self.scroll_to(row.bottom() - layout.viewport.height, layout);
        }
    }

    pub fn rescale(&mut self, from_dpi: u32, to_dpi: u32) {
        for offset in &mut self.offsets {
            *offset =
                i32::try_from(i64::from(*offset) * i64::from(to_dpi) / i64::from(from_dpi.max(1)))
                    .unwrap_or_default();
        }
    }
}
