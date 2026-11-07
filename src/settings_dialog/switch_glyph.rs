//! Antialiased toggle shapes composited into the native control's DC.
use super::theme::ThemePalette;
use alttabio::dialog_layout::{hairline, scale};
use std::cell::RefCell;
use windows::Win32::Foundation::{COLORREF, E_FAIL, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_ROUNDED_RECT, D2D1CreateFactory, ID2D1DCRenderTarget,
    ID2D1Factory, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::HDC;
use windows::core::{Error, Result};
use windows_numerics::Vector2;

#[derive(Default)]
pub(super) struct Painter {
    // One target per dialog, shared by its immutable paint snapshots on the UI thread.
    resources: RefCell<Option<Resources>>,
}

impl Painter {
    pub(super) fn paint(
        &self,
        dc: HDC,
        bounds: RECT,
        dpi: u32,
        palette: ThemePalette,
        checked: bool,
        enabled: bool,
    ) -> Result<()> {
        let mut resources = self
            .resources
            .try_borrow_mut()
            .map_err(|_| Error::new(E_FAIL, "Settings toggle painter is already in use"))?;
        if resources.is_none() {
            *resources = Some(Resources::new()?);
        }
        let result = resources
            .as_ref()
            .ok_or_else(|| Error::from_hresult(E_FAIL))?
            .paint(dc, bounds, dpi, palette, checked, enabled);
        // A failed DC bind or draw can invalidate the target. Recreate it on the next paint.
        if result.is_err() {
            *resources = None;
        }
        result
    }
}

struct Resources {
    target: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
}

impl Resources {
    fn new() -> Result<Self> {
        let properties = D2D1_RENDER_TARGET_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            // Geometry uses the native control's already-scaled pixel coordinates.
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        // SAFETY: the interface matches the factory; properties describe a BGRA DC target.
        // The COM interfaces own their references and remain on this dialog's UI thread.
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let target = factory.CreateDCRenderTarget(&raw const properties)?;
            let brush = target.CreateSolidColorBrush(&color(COLORREF(0)), None)?;
            Ok(Self { target, brush })
        }
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "bounded control pixel dimensions fit f32"
    )]
    fn paint(
        &self,
        dc: HDC,
        bounds: RECT,
        dpi: u32,
        palette: ThemePalette,
        checked: bool,
        enabled: bool,
    ) -> Result<()> {
        let width = (bounds.right - bounds.left) as f32;
        let height = (bounds.bottom - bounds.top) as f32;
        let rounded = |inset: f32| D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: inset,
                top: inset,
                right: width - inset,
                bottom: height - inset,
            },
            radiusX: height / 2.0 - inset,
            radiusY: height / 2.0 - inset,
        };
        let radius = scale(12, dpi) as f32 / 2.0;
        let thumb = D2D1_ELLIPSE {
            point: Vector2 {
                X: scale(if checked { 17 } else { 3 }, dpi) as f32 + radius,
                Y: scale(4, dpi) as f32 + radius,
            },
            radiusX: radius,
            radiusY: radius,
        };
        // SAFETY: dc is borrowed for this synchronous paint and bounds covers only the glyph.
        // Every shape and COM resource is live on this UI thread. There are no fallible calls
        // or early returns between BeginDraw and EndDraw. Clearing the opaque background
        // blends curved edges against the current theme and removes the previous state.
        unsafe {
            self.target.BindDC(dc, &raw const bounds)?;
            self.target
                .SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.target.BeginDraw();
            self.target.Clear(Some(&color(palette.background)));
            self.brush.SetColor(&color(if checked {
                palette.label_text(enabled)
            } else {
                palette.control_border
            }));
            self.target.FillRoundedRectangle(&rounded(0.0), &self.brush);
            if !checked {
                self.brush.SetColor(&color(palette.background));
                self.target
                    .FillRoundedRectangle(&rounded(hairline(dpi) as f32), &self.brush);
            }
            self.brush.SetColor(&color(if checked {
                palette.background
            } else {
                palette.secondary
            }));
            self.target.FillEllipse(&raw const thumb, &self.brush);
            self.target.EndDraw(None, None)
        }
    }
}

fn color(value: COLORREF) -> D2D1_COLOR_F {
    let [r, g, b, _] = value.0.to_le_bytes();
    D2D1_COLOR_F {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: 1.0,
    }
}
