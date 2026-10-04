mod canvas;
mod icons;

use alttabio::close_button::CloseButtonVisualState;
use alttabio::overlay_layout::{for_compact_list, layout_dpi, layout_scale};
use alttabio::settings::AppearanceSettings;
use alttabio::switcher::Switcher;
use alttabio::theme::{ResolvedTheme, Rgb8};
use canvas::Canvas;
use icons::draw_icon_pass;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{D2D_SIZE_U, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
    D2D1CreateFactory, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Gdi::{COLOR_BACKGROUND, GetSysColor, HDC};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::{Result, w};

pub struct Renderer {
    d2d_factory: ID2D1Factory,
    roomy_text: TextFormats,
    compact_text: TextFormats,
    theme: ResolvedTheme,
    resources: Option<RenderResources>,
    // DirectWrite takes UTF-16, so every title, app name and number is re-encoded on each paint;
    // one buffer reused for all of them keeps painting from allocating.
    utf16: Vec<u16>,
    icon_pass_failing: bool,
}

struct TextFormats {
    title: IDWriteTextFormat,
    detail: IDWriteTextFormat,
    number: IDWriteTextFormat,
}

struct RenderResources {
    target: ID2D1HwndRenderTarget,
    metrics: RenderTargetMetrics,
    background_color: D2D1_COLOR_F,
    window_border_brush: ID2D1SolidColorBrush,
    preview_background_brush: ID2D1SolidColorBrush,
    preview_border_brush: ID2D1SolidColorBrush,
    selected_brush: ID2D1SolidColorBrush,
    close_hover_brush: ID2D1SolidColorBrush,
    close_pressed_brush: ID2D1SolidColorBrush,
    primary_brush: ID2D1SolidColorBrush,
    secondary_brush: ID2D1SolidColorBrush,
    number_brush: ID2D1SolidColorBrush,
    divider_brush: ID2D1SolidColorBrush,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RenderTargetMetrics {
    pixel_width: u32,
    pixel_height: u32,
    dpi: u16,
}

impl RenderTargetMetrics {
    fn for_window(hwnd: HWND, pixel_width: u32, pixel_height: u32) -> Self {
        let window_dpi = unsafe {
            // SAFETY: hwnd is the live overlay window and the call returns a scalar DPI value.
            GetDpiForWindow(hwnd)
        };
        Self {
            pixel_width,
            pixel_height,
            dpi: layout_dpi(window_dpi),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RenderTargetUpdate {
    Unchanged,
    Resize,
    Recreate,
}

const fn render_target_update(
    current: RenderTargetMetrics,
    next: RenderTargetMetrics,
) -> RenderTargetUpdate {
    if current.dpi != next.dpi {
        RenderTargetUpdate::Recreate
    } else if current.pixel_width != next.pixel_width || current.pixel_height != next.pixel_height {
        RenderTargetUpdate::Resize
    } else {
        RenderTargetUpdate::Unchanged
    }
}

#[derive(Clone, Copy)]
struct PreviewFrame {
    rect: RECT,
    scale: f32,
}

#[derive(Clone, Copy)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "fields are the independent appearance switches consumed by one render pass"
)]
pub struct RenderOptions {
    visible_borders: bool,
    show_numbers: bool,
    show_app_names: bool,
    compact_list: bool,
    large_icons: bool,
}

impl From<&AppearanceSettings> for RenderOptions {
    fn from(settings: &AppearanceSettings) -> Self {
        Self {
            visible_borders: settings.visible_borders,
            show_numbers: settings.show_numbers,
            show_app_names: settings.show_app_names,
            compact_list: settings.compact_list,
            large_icons: settings.large_icons,
        }
    }
}

impl Renderer {
    pub fn new(theme: ResolvedTheme) -> Result<Self> {
        let d2d_factory = unsafe {
            // SAFETY: the requested COM interface type matches D2D1CreateFactory and the returned
            // windows-rs interface owns its reference count.
            D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
        }?;
        let write_factory = unsafe {
            // SAFETY: the requested COM interface type matches DWriteCreateFactory and the returned
            // windows-rs interface owns its reference count.
            DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED)
        }?;
        let roomy_text = create_text_formats(&write_factory, 18.0, 12.0, 14.0)?;
        let compact_text = create_text_formats(&write_factory, 15.0, 11.0, 12.0)?;

        Ok(Self {
            d2d_factory,
            roomy_text,
            compact_text,
            theme,
            resources: None,
            utf16: Vec::new(),
            icon_pass_failing: false,
        })
    }

    pub fn set_theme(&mut self, theme: ResolvedTheme) {
        if self.theme != theme {
            self.theme = theme;
            self.resources = None;
        }
    }

    pub fn resize(&mut self, hwnd: HWND, width: u32, height: u32) -> Result<()> {
        let next = RenderTargetMetrics::for_window(hwnd, width, height);
        let update = self
            .resources
            .as_ref()
            .map(|resources| render_target_update(resources.metrics, next));
        match update {
            Some(RenderTargetUpdate::Recreate) => self.resources = None,
            Some(RenderTargetUpdate::Resize) => {
                let resize_result = if let Some(resources) = &mut self.resources {
                    let result = unsafe {
                        // SAFETY: the render target is valid and the size contains no borrowed
                        // pointers.
                        resources.target.Resize(&D2D_SIZE_U { width, height })
                    };
                    if result.is_ok() {
                        resources.metrics = next;
                    }
                    result
                } else {
                    Ok(())
                };
                if resize_result.is_err() {
                    self.resources = None;
                }
                resize_result?;
            }
            Some(RenderTargetUpdate::Unchanged) | None => {}
        }
        Ok(())
    }

    pub fn draw(
        &mut self,
        hwnd: HWND,
        switcher: &Switcher,
        preview_frame: Option<RECT>,
        options: RenderOptions,
        close_button_state: CloseButtonVisualState,
    ) -> Result<()> {
        if self.resources.is_none() {
            self.resources = Some(self.create_resources(hwnd)?);
        }
        let Some(resources) = &self.resources else {
            return Ok(());
        };

        let text = if options.compact_list {
            &self.compact_text
        } else {
            &self.roomy_text
        };
        let window_dpi = unsafe {
            // SAFETY: `hwnd` is the live overlay window and the call returns a scalar DPI value.
            GetDpiForWindow(hwnd)
        };
        let scale = layout_scale(window_dpi);
        let canvas = Canvas {
            resources,
            text,
            layout: for_compact_list(options.compact_list),
            options,
            scale,
            close_button_state,
        };
        let draw_result = canvas.draw_switcher(
            switcher,
            preview_frame.map(|rect| PreviewFrame { rect, scale }),
            &mut self.utf16,
        );
        if draw_result.is_err() {
            self.resources = None;
        }
        draw_result
    }

    pub fn draw_icons(
        &mut self,
        hwnd: HWND,
        hdc: HDC,
        switcher: &Switcher,
        options: RenderOptions,
    ) {
        let failure = draw_icon_pass(hwnd, hdc, switcher, options).err();
        // The overlay repaints on every selection change, so an icon that keeps failing would log
        // on each paint. Logging the first paint of a run of failures is enough.
        if let Some(failure) = &failure
            && !self.icon_pass_failing
        {
            eprintln!("{failure}");
        }
        self.icon_pass_failing = failure.is_some();
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "Windows DPI values are small integers represented exactly as f32"
    )]
    fn create_resources(&self, hwnd: HWND) -> Result<RenderResources> {
        let mut client = RECT::default();
        unsafe {
            // SAFETY: `client` is writable for the call and `hwnd` is the live overlay window.
            GetClientRect(hwnd, &raw mut client)?;
        }
        let width = u32::try_from(client.right.saturating_sub(client.left)).unwrap_or_default();
        let height = u32::try_from(client.bottom.saturating_sub(client.top)).unwrap_or_default();
        let window_dpi = unsafe {
            // SAFETY: `hwnd` is a live top-level window owned by this UI thread.
            GetDpiForWindow(hwnd)
        };
        let metrics = RenderTargetMetrics {
            pixel_width: width,
            pixel_height: height,
            dpi: layout_dpi(window_dpi),
        };
        let dpi = f32::from(metrics.dpi);
        let render_properties = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            dpiX: dpi,
            dpiY: dpi,
            ..D2D1_RENDER_TARGET_PROPERTIES::default()
        };
        let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: D2D_SIZE_U { width, height },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        let target = unsafe {
            // SAFETY: both property pointers remain valid for the call and `hwnd` remains owned by
            // the UI thread for the target lifetime.
            self.d2d_factory
                .CreateHwndRenderTarget(&raw const render_properties, &raw const hwnd_properties)
        }?;
        let palette = self.theme.palette();

        Ok(RenderResources {
            metrics,
            background_color: color_from_rgb8(palette.background),
            window_border_brush: create_brush(&target, color_from_rgb8(palette.window_border))?,
            preview_background_brush: create_brush(&target, windows_desktop_color())?,
            preview_border_brush: create_brush(&target, color_from_rgb8(palette.preview_border))?,
            selected_brush: create_brush(&target, color_from_rgb8(palette.selected))?,
            close_hover_brush: create_brush(&target, color_from_rgb8(palette.close_hover))?,
            close_pressed_brush: create_brush(&target, color_from_rgb8(palette.close_pressed))?,
            primary_brush: create_brush(&target, color_from_rgb8(palette.primary))?,
            secondary_brush: create_brush(&target, color_from_rgb8(palette.secondary))?,
            number_brush: create_brush(&target, color_from_rgb8(palette.number))?,
            divider_brush: create_brush(&target, color_from_rgb8(palette.divider))?,
            target,
        })
    }
}

fn create_text_formats(
    factory: &IDWriteFactory,
    title_size: f32,
    detail_size: f32,
    number_size: f32,
) -> Result<TextFormats> {
    Ok(TextFormats {
        title: create_text_format(factory, title_size, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        detail: create_text_format(factory, detail_size, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        number: create_text_format(factory, number_size, DWRITE_TEXT_ALIGNMENT_CENTER)?,
    })
}

fn create_text_format(
    factory: &IDWriteFactory,
    size: f32,
    alignment: DWRITE_TEXT_ALIGNMENT,
) -> Result<IDWriteTextFormat> {
    let format = unsafe {
        // SAFETY: both string arguments are static null-terminated UTF-16 strings and the returned
        // windows-rs interface owns its reference count.
        factory.CreateTextFormat(
            w!("Segoe UI Variable Text"),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("en-us"),
        )
    }?;
    unsafe {
        // SAFETY: the format is a valid DirectWrite interface created on this thread.
        format.SetTextAlignment(alignment)?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
    }
    Ok(format)
}

fn create_brush(
    target: &ID2D1HwndRenderTarget,
    color: D2D1_COLOR_F,
) -> Result<ID2D1SolidColorBrush> {
    unsafe {
        // SAFETY: `color` remains valid for the call and the render target owns the created brush.
        target.CreateSolidColorBrush(&raw const color, None)
    }
}

const fn color(red: f32, green: f32, blue: f32, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: red,
        g: green,
        b: blue,
        a: alpha,
    }
}

fn color_from_rgb8(value: Rgb8) -> D2D1_COLOR_F {
    color(
        f32::from(value.red) / 255.0,
        f32::from(value.green) / 255.0,
        f32::from(value.blue) / 255.0,
        1.0,
    )
}

fn windows_desktop_color() -> D2D1_COLOR_F {
    let colorref = unsafe {
        // SAFETY: GetSysColor reads the process-independent Windows desktop color and has no
        // pointer or lifetime preconditions.
        GetSysColor(COLOR_BACKGROUND)
    };
    color_from_colorref(colorref)
}

fn color_from_colorref(colorref: u32) -> D2D1_COLOR_F {
    color(
        f32::from(u8::try_from(colorref & 0xFF).unwrap_or_default()) / 255.0,
        f32::from(u8::try_from((colorref >> 8) & 0xFF).unwrap_or_default()) / 255.0,
        f32::from(u8::try_from((colorref >> 16) & 0xFF).unwrap_or_default()) / 255.0,
        1.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_reconnect_recreates_target_when_dpi_changes_with_the_bounds() {
        let before = RenderTargetMetrics {
            pixel_width: 2_400,
            pixel_height: 1_350,
            dpi: 168,
        };
        let after = RenderTargetMetrics {
            pixel_width: 2_048,
            pixel_height: 1_152,
            dpi: 144,
        };

        assert_eq!(
            render_target_update(before, after),
            RenderTargetUpdate::Recreate
        );
    }

    #[test]
    fn orientation_change_resizes_target_when_dpi_is_unchanged() {
        let landscape = RenderTargetMetrics {
            pixel_width: 1_200,
            pixel_height: 675,
            dpi: 144,
        };
        let portrait = RenderTargetMetrics {
            pixel_width: 675,
            pixel_height: 1_200,
            dpi: 144,
        };

        assert_eq!(
            render_target_update(landscape, portrait),
            RenderTargetUpdate::Resize
        );
        assert_eq!(
            render_target_update(portrait, portrait),
            RenderTargetUpdate::Unchanged
        );
    }

    #[test]
    fn windows_desktop_color_preserves_colorref_channel_order() {
        let background = color_from_colorref(0x00_2F_2C_2D);

        assert_close(background.r, 45.0 / 255.0);
        assert_close(background.g, 44.0 / 255.0);
        assert_close(background.b, 47.0 / 255.0);
        assert_close(background.a, 1.0);
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < f32::EPSILON);
    }
}
