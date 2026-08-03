//! Pure theme resolution and switcher palette definitions.

use crate::settings::Theme;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolvedTheme {
    Light,
    Dark,
}

impl ResolvedTheme {
    #[must_use]
    pub const fn palette(self) -> ThemePalette {
        match self {
            Self::Light => ThemePalette {
                background: Rgb8::new(243, 243, 243),
                window_border: Rgb8::new(154, 154, 154),
                preview_border: Rgb8::new(166, 166, 166),
                selected: Rgb8::new(204, 228, 247),
                close_hover: Rgb8::new(184, 216, 240),
                close_pressed: Rgb8::new(160, 199, 230),
                primary: Rgb8::new(26, 26, 26),
                secondary: Rgb8::new(92, 92, 92),
                number: Rgb8::new(51, 95, 135),
                divider: Rgb8::new(208, 208, 208),
            },
            Self::Dark => ThemePalette {
                background: Rgb8::new(14, 16, 20),
                window_border: Rgb8::new(97, 97, 100),
                preview_border: Rgb8::new(107, 110, 117),
                selected: Rgb8::new(46, 87, 148),
                close_hover: Rgb8::new(64, 110, 173),
                close_pressed: Rgb8::new(31, 61, 105),
                primary: Rgb8::new(240, 242, 247),
                secondary: Rgb8::new(153, 163, 179),
                number: Rgb8::new(194, 209, 235),
                divider: Rgb8::new(64, 71, 84),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgb8 {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Rgb8 {
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThemePalette {
    pub background: Rgb8,
    pub window_border: Rgb8,
    pub preview_border: Rgb8,
    pub selected: Rgb8,
    pub close_hover: Rgb8,
    pub close_pressed: Rgb8,
    pub primary: Rgb8,
    pub secondary: Rgb8,
    pub number: Rgb8,
    pub divider: Rgb8,
}

#[must_use]
pub const fn resolve(theme: Theme, windows_app_theme: ResolvedTheme) -> ResolvedTheme {
    match theme {
        Theme::Auto => windows_app_theme,
        Theme::Light => ResolvedTheme::Light,
        Theme::Dark => ResolvedTheme::Dark,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_follows_the_windows_app_theme() {
        assert_eq!(
            resolve(Theme::Auto, ResolvedTheme::Light),
            ResolvedTheme::Light
        );
        assert_eq!(
            resolve(Theme::Auto, ResolvedTheme::Dark),
            ResolvedTheme::Dark
        );
    }

    #[test]
    fn explicit_themes_override_the_windows_app_theme() {
        assert_eq!(
            resolve(Theme::Light, ResolvedTheme::Dark),
            ResolvedTheme::Light
        );
        assert_eq!(
            resolve(Theme::Dark, ResolvedTheme::Light),
            ResolvedTheme::Dark
        );
    }

    #[test]
    fn light_palette_is_complete_and_deterministic() {
        assert_eq!(
            ResolvedTheme::Light.palette(),
            ThemePalette {
                background: Rgb8::new(243, 243, 243),
                window_border: Rgb8::new(154, 154, 154),
                preview_border: Rgb8::new(166, 166, 166),
                selected: Rgb8::new(204, 228, 247),
                close_hover: Rgb8::new(184, 216, 240),
                close_pressed: Rgb8::new(160, 199, 230),
                primary: Rgb8::new(26, 26, 26),
                secondary: Rgb8::new(92, 92, 92),
                number: Rgb8::new(51, 95, 135),
                divider: Rgb8::new(208, 208, 208),
            }
        );
    }

    #[test]
    fn dark_palette_preserves_the_established_switcher_colors() {
        let palette = ResolvedTheme::Dark.palette();

        assert_eq!(palette.background, Rgb8::new(14, 16, 20));
        assert_eq!(palette.window_border, Rgb8::new(97, 97, 100));
        assert_eq!(palette.selected, Rgb8::new(46, 87, 148));
        assert_eq!(palette.primary, Rgb8::new(240, 242, 247));
        assert_eq!(palette.divider, Rgb8::new(64, 71, 84));
    }
}
