use super::surface::Surface;
use super::text::TextRun;
use alttabio::filter_feedback::BACKGROUND_OPACITY;
use alttabio::text_edit::QueryText;
use alttabio::theme::{ResolvedTheme, Rgb8};
use std::cell::{Cell, RefCell};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1DCRenderTarget,
    ID2D1Factory, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::core::{Result, w};
use windows_numerics::Vector2;

pub(super) struct Painter {
    factory: ID2D1Factory,
    write: IDWriteFactory,
    query_format: IDWriteTextFormat,
    label_format: IDWriteTextFormat,
    target: Option<ID2D1DCRenderTarget>,
    surface: Option<Surface>,
    scroll: Cell<f32>,
    text_run: RefCell<Option<TextRun>>,
}

impl Painter {
    pub fn new() -> Result<Self> {
        let (factory, write) = unsafe {
            // SAFETY: both requested COM interface types match the factory functions; each
            // returned interface owns its reference count and stays on this UI thread.
            (
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
                DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED)?,
            )
        };
        Ok(Self {
            factory,
            query_format: text_format(&write, 15.0)?,
            label_format: text_format(&write, 11.0)?,
            write,
            target: None,
            surface: None,
            scroll: Cell::new(0.0),
            text_run: RefCell::new(None),
        })
    }

    pub fn draw_editor(
        &mut self,
        editor: &QueryText,
        width: i32,
        height: i32,
        dpi: u16,
        theme: ResolvedTheme,
    ) -> Result<&Surface> {
        if self
            .surface
            .as_ref()
            .is_none_or(|surface| surface.size.cx != width || surface.size.cy != height)
        {
            self.surface = Some(Surface::new(width, height)?);
        }
        if self.target.is_none() {
            let properties = D2D1_RENDER_TARGET_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                ..D2D1_RENDER_TARGET_PROPERTIES::default()
            };
            self.target = Some(unsafe {
                // SAFETY: the initialized properties request a DC-compatible BGRA alpha target.
                self.factory.CreateDCRenderTarget(&raw const properties)
            }?);
        }
        let (Some(surface), Some(target)) = (&self.surface, &self.target) else {
            return Err(windows::core::Error::new(
                windows::core::HRESULT(0x8000_4005_u32.cast_signed()),
                "Filter popup resources are unavailable",
            ));
        };
        let rect = RECT {
            right: width,
            bottom: height,
            ..RECT::default()
        };
        unsafe {
            // SAFETY: the surface owns its selected DIB and DC for the entire paint; rect covers
            // exactly that bitmap. Grayscale antialiasing preserves per-pixel text alpha.
            target.BindDC(surface.dc, &raw const rect)?;
            target.SetDpi(f32::from(dpi), f32::from(dpi));
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        }
        let result = self.paint(target, surface, editor, dpi, theme);
        if let Err(error) = result {
            self.target = None;
            return Err(error);
        }
        self.surface
            .as_ref()
            .ok_or_else(windows::core::Error::from_thread)
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "a popup's bounded pixel dimensions fit f32"
    )]
    fn paint(
        &self,
        target: &ID2D1DCRenderTarget,
        surface: &Surface,
        editor: &QueryText,
        dpi: u16,
        theme: ResolvedTheme,
    ) -> Result<()> {
        let factor = f32::from(dpi) / 96.0;
        let width = surface.size.cx as f32 / factor;
        let height = surface.size.cy as f32 / factor;
        let background = match theme {
            ResolvedTheme::Light => Rgb8::new(255, 255, 255),
            ResolvedTheme::Dark => Rgb8::new(40, 45, 52),
        };
        let palette = theme.palette();
        let (fill, edge, text, secondary, selected) = unsafe {
            // SAFETY: all colors remain valid for their synchronous creation calls and the
            // brushes belong to this live target. Only the background has 50% opacity.
            (
                target.CreateSolidColorBrush(&color(background, BACKGROUND_OPACITY), None)?,
                target.CreateSolidColorBrush(&color(palette.window_border, 0.35), None)?,
                target.CreateSolidColorBrush(&color(palette.primary, 1.0), None)?,
                target.CreateSolidColorBrush(&color(palette.secondary, 1.0), None)?,
                target.CreateSolidColorBrush(&color(palette.selected, 1.0), None)?,
            )
        };
        let run = TextRun::new(
            &self.write,
            &self.query_format,
            editor,
            width,
            height,
            self.scroll.get(),
        )?;
        self.scroll.set(run.scroll);
        let capsule = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: 0.5,
                top: 0.5,
                right: width - 0.5,
                bottom: height - 0.5,
            },
            radiusX: height / 2.0,
            radiusY: height / 2.0,
        };
        unsafe {
            // SAFETY: every COM resource and stack rectangle stays live on this UI thread. No
            // fallible calls or early returns occur between BeginDraw and EndDraw below.
            target.BeginDraw();
            target.Clear(Some(&D2D1_COLOR_F::default()));
            target.FillRoundedRectangle(&raw const capsule, &fill);
            target.DrawRoundedRectangle(&raw const capsule, &edge, 1.0, None);
            target.DrawText(
                &"Filter".encode_utf16().collect::<Vec<_>>(),
                &self.label_format,
                &D2D_RECT_F {
                    left: 14.0,
                    top: 0.0,
                    right: 48.0,
                    bottom: height,
                },
                &secondary,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            run.draw(target, &text, &selected);
            draw_clear_glyph(target, &secondary, width, height);
            target.EndDraw(None, None)?;
        }
        *self.text_run.borrow_mut() = Some(run);
        Ok(())
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "popup-local mouse coordinates fit f32"
    )]
    pub fn hit_test(&self, x: i32, dpi: u16) -> Result<u32> {
        self.text_run
            .borrow()
            .as_ref()
            .map_or(Ok(0), |run| run.hit_test(x as f32 * 96.0 / f32::from(dpi)))
    }

    #[cfg(test)]
    fn draw(
        &mut self,
        query: &str,
        width: i32,
        height: i32,
        dpi: u16,
        theme: ResolvedTheme,
    ) -> Result<&Surface> {
        let mut editor = QueryText::default();
        editor.insert(query);
        self.draw_editor(&editor, width, height, dpi, theme)
    }
}

fn draw_clear_glyph(
    target: &ID2D1DCRenderTarget,
    brush: &ID2D1SolidColorBrush,
    width: f32,
    height: f32,
) {
    let center = width - 18.0;
    let middle = height / 2.0;
    unsafe {
        // SAFETY: the caller keeps this target and brush live inside its BeginDraw/EndDraw pair.
        target.DrawLine(
            Vector2::new(center - 3.0, middle - 3.0),
            Vector2::new(center + 3.0, middle + 3.0),
            brush,
            1.3,
            None,
        );
        target.DrawLine(
            Vector2::new(center + 3.0, middle - 3.0),
            Vector2::new(center - 3.0, middle + 3.0),
            brush,
            1.3,
            None,
        );
    }
}

fn text_format(factory: &IDWriteFactory, size: f32) -> Result<IDWriteTextFormat> {
    unsafe {
        // SAFETY: both strings are static UTF-16; the returned format owns its COM reference.
        let format = factory.CreateTextFormat(
            w!("Segoe UI Variable Text"),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("en-us"),
        )?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        Ok(format)
    }
}

fn color(rgb: Rgb8, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: f32::from(rgb.red) / 255.0,
        g: f32::from(rgb.green) / 255.0,
        b: f32::from(rgb.blue) / 255.0,
        a: alpha,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win32::ComApartment;

    #[test]
    fn background_is_half_opaque_and_text_is_solid_in_both_themes() -> Result<()> {
        let _apartment = ComApartment::initialize()?;
        let mut painter = Painter::new()?;
        for theme in [ResolvedTheme::Light, ResolvedTheme::Dark] {
            let surface = painter.draw("firefox", 240, 36, 96, theme)?;
            let pixels = surface.pixels();
            assert_eq!(pixels[3], 0, "rounded corner must be transparent");
            let plain_background = (10 * 240 + 190) * 4;
            assert!((127..=128).contains(&pixels[plain_background + 3]));
            assert!(
                pixels.chunks_exact(4).any(|pixel| pixel[3] == 255),
                "text must remain opaque"
            );
            assert!(
                pixels
                    .chunks_exact(4)
                    .all(|pixel| pixel[..3].iter().all(|channel| *channel <= pixel[3])),
                "layered pixels must be premultiplied"
            );
        }
        Ok(())
    }

    #[test]
    fn selection_is_painted_and_replacing_long_text_resets_horizontal_scrolling() -> Result<()> {
        let _apartment = ComApartment::initialize()?;
        let mut painter = Painter::new()?;
        let mut editor = QueryText::default();
        editor.insert("firefox");
        editor.command(alttabio::text_edit::TextCommand::SelectAll);
        let pixels = painter
            .draw_editor(&editor, 240, 36, 96, ResolvedTheme::Dark)?
            .pixels();
        let selected = ResolvedTheme::Dark.palette().selected;
        assert!(
            pixels
                .chunks_exact(4)
                .any(|pixel| pixel == [selected.blue, selected.green, selected.red, 255])
        );
        assert_eq!(painter.hit_test(52, 96)?, 0);
        editor.reset(&"long prefix ".repeat(30));
        let _surface = painter.draw_editor(&editor, 240, 36, 96, ResolvedTheme::Dark)?;
        editor.reset("x");
        let reused = painter
            .draw_editor(&editor, 240, 36, 96, ResolvedTheme::Dark)?
            .pixels()
            .to_vec();
        let mut fresh = Painter::new()?;
        assert_eq!(
            reused,
            fresh
                .draw_editor(&editor, 240, 36, 96, ResolvedTheme::Dark)?
                .pixels()
        );
        let _surface = painter.draw_editor(&editor, 420, 63, 168, ResolvedTheme::Light)?;
        assert_eq!(painter.hit_test(0, 168)?, 0);
        Ok(())
    }

    #[test]
    fn long_queries_keep_their_latest_characters_visible() -> Result<()> {
        let _apartment = ComApartment::initialize()?;
        let mut painter = Painter::new()?;
        let query = format!("{}latest query", "a long prefix ".repeat(30));
        let surface = painter.draw(&query, 240, 36, 96, ResolvedTheme::Dark)?;
        let opaque_query_pixels = surface
            .pixels()
            .chunks_exact(4)
            .enumerate()
            .filter(|(index, pixel)| {
                let x = index % 240;
                let y = index / 240;
                (60..195).contains(&x) && (8..28).contains(&y) && pixel[3] > 245
            })
            .count();
        assert!(
            opaque_query_pixels > 20,
            "long text must remain visible before the caret"
        );
        Ok(())
    }
}
