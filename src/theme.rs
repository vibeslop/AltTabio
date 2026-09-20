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

/// A color in OKLCH: perceptual lightness 0..=1, chroma from 0, hue in degrees.
///
/// The switcher's tokens are authored here so that lightness gaps, which carry contrast, and
/// hue, which every token shares, can be reasoned about directly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklch {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

impl Oklch {
    #[must_use]
    pub const fn new(l: f64, c: f64, h: f64) -> Self {
        Self { l, c, h }
    }

    #[must_use]
    pub fn from_rgb8(color: Rgb8) -> Self {
        let red = srgb_to_linear(f64::from(color.red) / 255.0);
        let green = srgb_to_linear(f64::from(color.green) / 255.0);
        let blue = srgb_to_linear(f64::from(color.blue) / 255.0);
        let long =
            (0.412_221_470_8 * red + 0.536_332_536_3 * green + 0.051_445_992_9 * blue).cbrt();
        let medium =
            (0.211_903_498_2 * red + 0.680_699_545_1 * green + 0.107_396_956_6 * blue).cbrt();
        let short =
            (0.088_302_461_9 * red + 0.281_718_837_6 * green + 0.629_978_700_5 * blue).cbrt();
        let lightness = 0.210_454_255_3 * long + 0.793_617_785 * medium - 0.004_072_046_8 * short;
        let axis_a = 1.977_998_495_1 * long - 2.428_592_205 * medium + 0.450_593_709_9 * short;
        let axis_b = 0.025_904_037_1 * long + 0.782_771_766_2 * medium - 0.808_675_766 * short;
        let chroma = axis_a.hypot(axis_b);
        let hue = axis_b.atan2(axis_a).to_degrees().rem_euclid(360.0);
        Self {
            l: lightness,
            c: chroma,
            h: if chroma < 1e-4 { 0.0 } else { hue },
        }
    }

    /// The nearest displayable sRGB color: chroma shrinks, with lightness and hue kept, until
    /// every channel fits, so a too-vivid token loses vividness rather than shifting tone.
    #[must_use]
    pub fn to_rgb8(self) -> Rgb8 {
        let mut chroma = self.c;
        for _ in 0..24 {
            if let Some(color) = linear_rgb(self.l, chroma, self.h) {
                return color;
            }
            chroma *= 0.85;
        }
        linear_rgb(self.l, 0.0, self.h).unwrap_or(Rgb8::new(0, 0, 0))
    }

    #[must_use]
    pub const fn with_lightness(mut self, l: f64) -> Self {
        self.l = l;
        self
    }

    #[must_use]
    pub const fn with_chroma(mut self, c: f64) -> Self {
        self.c = c;
        self
    }
}

fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f64) -> f64 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "channels are clamped to 0..=255 before the conversion"
)]
fn linear_rgb(lightness: f64, chroma: f64, hue: f64) -> Option<Rgb8> {
    let (sin, cos) = hue.to_radians().sin_cos();
    let axis_a = chroma * cos;
    let axis_b = chroma * sin;
    let long = lightness + 0.396_337_777_4 * axis_a + 0.215_803_757_3 * axis_b;
    let medium = lightness - 0.105_561_345_8 * axis_a - 0.063_854_172_8 * axis_b;
    let short = lightness - 0.089_484_177_5 * axis_a - 1.291_485_548 * axis_b;
    let (long, medium, short) = (long.powi(3), medium.powi(3), short.powi(3));
    let red = 4.076_741_662_1 * long - 3.307_711_591_3 * medium + 0.230_969_929_2 * short;
    let green = -1.268_438_004_6 * long + 2.609_757_401_1 * medium - 0.341_319_396_5 * short;
    let blue = -0.004_196_086_3 * long - 0.703_418_614_7 * medium + 1.707_614_701 * short;
    if [red, green, blue]
        .iter()
        .any(|value| *value < -0.0005 || *value > 1.0005)
    {
        return None;
    }
    let to_byte = |value: f64| (linear_to_srgb(value.clamp(0.0, 1.0)) * 255.0).round() as u8;
    Some(Rgb8::new(to_byte(red), to_byte(green), to_byte(blue)))
}

/// A token with its opacity, for the few surfaces that let the glass show through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub color: Rgb8,
    pub alpha: f64,
}

impl Rgba {
    #[must_use]
    pub const fn new(color: Rgb8, alpha: f64) -> Self {
        Self { color, alpha }
    }

    #[must_use]
    pub const fn opaque(color: Rgb8) -> Self {
        Self { color, alpha: 1.0 }
    }
}

/// The one palette the macOS switcher draws from: cool greys with a whisper of blue, and the
/// system blue as the only vivid color, in a light and a dark set.
///
/// Every token is authored in OKLCH so that lightness gaps, which carry contrast, can be read
/// off directly; every text token is a real color rather than an opacity of another, so
/// secondary text keeps a little chroma instead of going grey and lifeless.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwitcherTokens {
    /// The glass tint behind everything.
    pub canvas: Rgba,
    /// Inset areas: the preview well and the search row.
    pub well: Rgba,
    /// Raised chips: keycaps and footer keys.
    pub raised: Rgb8,
    pub raised_edge: Rgba,
    /// The darker bottom edge that makes a keycap look pressable.
    pub raised_base: Rgba,
    /// The action panel floating over the preview.
    pub surface: Rgba,
    pub surface_edge: Rgba,
    /// The selected row and the selected action.
    pub selection: Rgba,
    /// A keycap the instant its number was pressed: the blue itself.
    pub keycap_pressed: Rgb8,
    pub keycap_pressed_text: Rgb8,
    pub text: Rgb8,
    pub text_secondary: Rgb8,
    /// Pure white or black at low alpha for image outlines and the panel edge.
    pub ring: Rgba,
    /// Hover and pressed fills for the close button.
    pub control_hover: Rgba,
    pub control_pressed: Rgba,
}

/// The hue every switcher token shares: that of the system blue.
const SWITCHER_HUE: f64 = 256.0;
/// The chroma of the greys; enough to read as cool, not enough to read as colored.
const NEUTRAL_CHROMA: f64 = 0.012;
/// The blue the selection, keycaps, and badges are built from.
pub const SWITCHER_BLUE: Rgb8 = Rgb8::new(0, 122, 255);

const fn neutral(l: f64) -> Oklch {
    Oklch::new(l, NEUTRAL_CHROMA, SWITCHER_HUE)
}

const fn blue(l: f64, c: f64) -> Oklch {
    Oklch::new(l, c, SWITCHER_HUE)
}

impl SwitcherTokens {
    /// The tokens for `theme`. The values are fixed; a readability fix moves a token's
    /// lightness and keeps its chroma and hue.
    #[must_use]
    pub fn new(theme: ResolvedTheme) -> Self {
        let white = Rgb8::new(255, 255, 255);
        let black = Rgb8::new(0, 0, 0);
        match theme {
            ResolvedTheme::Dark => Self {
                canvas: Rgba::new(neutral(0.21).to_rgb8(), 0.74),
                well: Rgba::new(neutral(0.27).to_rgb8(), 0.85),
                raised: neutral(0.34).to_rgb8(),
                raised_edge: Rgba::opaque(neutral(0.45).to_rgb8()),
                raised_base: Rgba::new(neutral(0.12).to_rgb8(), 0.9),
                surface: Rgba::new(neutral(0.25).to_rgb8(), 0.97),
                surface_edge: Rgba::opaque(neutral(0.38).to_rgb8()),
                selection: Rgba::new(blue(0.36, 0.10).to_rgb8(), 0.96),
                keycap_pressed: SWITCHER_BLUE,
                keycap_pressed_text: white,
                text: neutral(0.96).with_chroma(0.005).to_rgb8(),
                text_secondary: neutral(0.72).with_chroma(0.018).to_rgb8(),
                ring: Rgba::new(white, 0.10),
                control_hover: Rgba::new(white, 0.10),
                control_pressed: Rgba::new(white, 0.18),
            },
            ResolvedTheme::Light => Self {
                canvas: Rgba::new(neutral(0.965).to_rgb8(), 0.74),
                well: Rgba::new(neutral(0.92).to_rgb8(), 0.85),
                raised: neutral(0.995).to_rgb8(),
                raised_edge: Rgba::new(black, 0.10),
                raised_base: Rgba::new(black, 0.16),
                surface: Rgba::new(neutral(0.985).to_rgb8(), 0.97),
                surface_edge: Rgba::new(black, 0.10),
                selection: Rgba::new(blue(0.89, 0.092).to_rgb8(), 0.96),
                keycap_pressed: SWITCHER_BLUE,
                keycap_pressed_text: white,
                text: neutral(0.22).with_chroma(0.007).to_rgb8(),
                text_secondary: neutral(0.48).with_chroma(0.018).to_rgb8(),
                ring: Rgba::new(black, 0.10),
                control_hover: Rgba::new(black, 0.07),
                control_pressed: Rgba::new(black, 0.13),
            },
        }
    }
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

    fn lightness(color: Rgb8) -> f64 {
        Oklch::from_rgb8(color).l
    }

    #[test]
    fn oklch_round_trips_known_colors() {
        let white = Oklch::from_rgb8(Rgb8::new(255, 255, 255));
        assert!((white.l - 1.0).abs() < 0.01);
        assert!(white.c < 0.001);

        let blue = Oklch::from_rgb8(Rgb8::new(0, 122, 255));
        assert!((blue.h - 256.0).abs() < 4.0);
        assert!(blue.c > 0.2);
        assert_eq!(blue.to_rgb8(), Rgb8::new(0, 122, 255));

        let grey = Oklch::new(0.5, 0.0, 0.0).to_rgb8();
        assert_eq!(grey.red, grey.green);
        assert_eq!(grey.green, grey.blue);
    }

    #[test]
    fn out_of_gamut_chroma_is_reduced_without_moving_lightness() {
        let vivid = Oklch::new(0.9, 0.3, 256.0).to_rgb8();

        assert!((lightness(vivid) - 0.9).abs() < 0.02);
    }

    #[test]
    fn tokens_keep_text_far_from_its_surfaces_in_both_themes() {
        let dark = SwitcherTokens::new(ResolvedTheme::Dark);
        let light = SwitcherTokens::new(ResolvedTheme::Light);

        // Near-black surfaces want foregrounds at L 0.75 or more; near-white ones at 0.45 or
        // less. Secondary text has to clear the same floors as it is body-sized.
        assert!(lightness(dark.canvas.color) < 0.25);
        assert!(lightness(dark.text) > 0.9);
        assert!(lightness(dark.text_secondary) > 0.7);
        assert!(lightness(dark.raised) - lightness(dark.canvas.color) > 0.1);
        assert!(lightness(dark.text) - lightness(dark.selection.color) > 0.5);

        assert!(lightness(light.canvas.color) > 0.9);
        assert!(lightness(light.text) < 0.25);
        assert!(lightness(light.text_secondary) < 0.5);
        assert!(lightness(light.selection.color) - lightness(light.text) > 0.6);
    }

    #[test]
    fn the_switcher_palette_is_fixed_and_shares_the_blue_hue() {
        for theme in [ResolvedTheme::Dark, ResolvedTheme::Light] {
            let tokens = SwitcherTokens::new(theme);
            assert_eq!(tokens, SwitcherTokens::new(theme));
            assert_eq!(tokens.keycap_pressed, SWITCHER_BLUE);
            for color in [tokens.canvas.color, tokens.selection.color] {
                let lch = Oklch::from_rgb8(color);
                assert!(lch.c > 0.005, "{color:?} is grey");
                assert!((lch.h - SWITCHER_HUE).abs() < 12.0, "{color:?} is off hue");
            }
        }
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
