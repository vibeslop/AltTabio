use alttabio::close_button::CloseButtonVisualState;
use alttabio::overlay_layout::{
    LogicalRect, close_glyph_geometry, for_compact_list, layout_dpi, layout_scale,
    task_text_vertical_layout, window_frame_geometry,
};
use alttabio::settings::AppearanceSettings;
use alttabio::switcher::Switcher;
use alttabio::theme::{ResolvedTheme, Rgb8};
use std::ffi::c_void;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D_SIZE_U, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_ROUNDED_RECT, D2D1CreateFactory, ID2D1Factory,
    ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Gdi::{COLOR_BACKGROUND, GetSysColor, HDC};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx, GetClientRect, HICON};
use windows::core::{Result, w};
use windows_numerics::Vector2;

pub struct Renderer {
    d2d_factory: ID2D1Factory,
    roomy_text: TextFormats,
    compact_text: TextFormats,
    theme: ResolvedTheme,
    resources: Option<RenderResources>,
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
        let draw_result = draw_switcher(
            resources,
            text,
            switcher,
            preview_frame.map(|rect| PreviewFrame { rect, scale }),
            scale,
            options,
            close_button_state,
        );
        if draw_result.is_err() {
            self.resources = None;
        }
        draw_result
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        reason = "icon geometry is clamped to the on-screen client area and Win32 HICON values"
    )]
    pub fn draw_icons(hwnd: HWND, hdc: HDC, switcher: &Switcher, options: RenderOptions) {
        let window_dpi = unsafe {
            // SAFETY: hwnd is the live overlay window and the call returns a scalar DPI value.
            GetDpiForWindow(hwnd)
        };
        let scale = layout_scale(window_dpi);
        let mut client = RECT::default();
        let client_read = unsafe {
            // SAFETY: client is writable and hwnd is the live overlay window.
            GetClientRect(hwnd, &raw mut client)
        };
        if client_read.is_err() {
            return;
        }
        let height = client.bottom.saturating_sub(client.top) as f32 / scale;
        let layout = for_compact_list(options.compact_list);
        let visible_rows = layout.visible_row_count(height);
        let list_top = layout.list_top();
        let start = switcher.visible_range(visible_rows).start;
        let icon_size = if options.large_icons {
            layout.large_icon_size
        } else {
            layout.small_icon_size
        };
        let leading_width = if options.show_numbers {
            layout.number_width
        } else {
            0.0
        };
        let icon_left =
            layout.outer_padding + leading_width + ((layout.icon_slot_width - icon_size) / 2.0);

        for (visible_position, task) in switcher
            .positioned_visible_tasks()
            .skip(start)
            .take(visible_rows)
        {
            let visible_index = visible_position.saturating_sub(1);
            if task.icon_handle == 0 {
                continue;
            }
            let row = visible_index - start;
            let top = list_top + (row as f32 * (layout.row_height + layout.row_gap));
            let icon_top = top + ((layout.row_height - icon_size) / 2.0);
            let icon = HICON(task.icon_handle as *mut c_void);
            let result = unsafe {
                // SAFETY: hdc is the current BeginPaint DC, the HICON is borrowed from a live
                // window/class snapshot, and all pixel dimensions are positive and on-screen.
                DrawIconEx(
                    hdc,
                    (icon_left * scale).round() as i32,
                    (icon_top * scale).round() as i32,
                    icon,
                    (icon_size * scale).round() as i32,
                    (icon_size * scale).round() as i32,
                    0,
                    None,
                    DI_NORMAL,
                )
            };
            if let Err(error) = result {
                eprintln!("Could not draw a task icon: {error}");
            }
        }
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

fn create_text_format(factory: &IDWriteFactory, size: f32) -> Result<IDWriteTextFormat> {
    unsafe {
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
    }
}

fn create_text_formats(
    factory: &IDWriteFactory,
    title_size: f32,
    detail_size: f32,
    number_size: f32,
) -> Result<TextFormats> {
    let formats = TextFormats {
        title: create_text_format(factory, title_size)?,
        detail: create_text_format(factory, detail_size)?,
        number: create_text_format(factory, number_size)?,
    };
    unsafe {
        // SAFETY: all three formats are valid DirectWrite interfaces created on this thread.
        formats
            .title
            .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
        formats
            .detail
            .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
        formats
            .number
            .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        formats
            .title
            .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        formats
            .detail
            .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        formats
            .number
            .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        formats
            .title
            .SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        formats
            .detail
            .SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        formats
            .number
            .SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
    }
    Ok(formats)
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

#[allow(
    clippy::cast_precision_loss,
    clippy::too_many_lines,
    reason = "rendering one bounded on-screen task-list pass keeps the geometry together"
)]
fn draw_switcher(
    resources: &RenderResources,
    text: &TextFormats,
    switcher: &Switcher,
    preview_frame: Option<PreviewFrame>,
    scale: f32,
    options: RenderOptions,
    close_button_state: CloseButtonVisualState,
) -> Result<()> {
    let target = &resources.target;
    let size = unsafe {
        // SAFETY: the target is valid for this UI-thread paint operation.
        target.GetSize()
    };
    let layout = for_compact_list(options.compact_list);
    let list_width = layout.list_width(size.width, scale);
    let visible_rows = layout.visible_row_count(size.height);
    let list_top = layout.list_top();
    let start = switcher.visible_range(visible_rows).start;
    let selected_handle = switcher.selected_task().map(|task| task.window_handle);

    unsafe {
        // SAFETY: all Direct2D interfaces are valid on this UI thread; all rectangles and UTF-16
        // buffers remain alive for their respective synchronous drawing calls.
        target.BeginDraw();
        target.Clear(Some(&raw const resources.background_color));

        if options.visible_borders {
            let window_frame = window_frame_geometry(size.width, size.height, scale);
            let frame_rect = rounded_rect(window_frame.rect, window_frame.radius);
            target.DrawRoundedRectangle(
                &raw const frame_rect,
                &resources.window_border_brush,
                window_frame.stroke_width,
                None,
            );
        }

        if let Some(frame) = preview_frame {
            let preview_frame = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: frame.rect.left as f32 / frame.scale + 0.5,
                    top: frame.rect.top as f32 / frame.scale + 0.5,
                    right: frame.rect.right as f32 / frame.scale - 0.5,
                    bottom: frame.rect.bottom as f32 / frame.scale - 0.5,
                },
                radiusX: 3.0,
                radiusY: 3.0,
            };
            target.FillRoundedRectangle(
                &raw const preview_frame,
                &resources.preview_background_brush,
            );
            if options.visible_borders {
                target.DrawRoundedRectangle(
                    &raw const preview_frame,
                    &resources.preview_border_brush,
                    1.0,
                    None,
                );
            }
        }

        let divider = D2D_RECT_F {
            left: list_width + layout.outer_padding,
            top: layout.outer_padding,
            right: list_width + layout.outer_padding + 1.0,
            bottom: size.height - layout.outer_padding,
        };
        target.FillRectangle(&raw const divider, &resources.divider_brush);

        for (visible_position, task) in switcher
            .positioned_visible_tasks()
            .skip(start)
            .take(visible_rows)
        {
            let visible_index = visible_position.saturating_sub(1);
            let row = visible_index - start;
            let top = list_top + (row as f32 * (layout.row_height + layout.row_gap));
            let bounds = LogicalRect {
                left: layout.outer_padding,
                top,
                right: list_width,
                bottom: top + layout.row_height,
            };
            if selected_handle == Some(task.window_handle) {
                target.FillRoundedRectangle(
                    &rounded_rect(bounds, layout.selection_radius),
                    &resources.selected_brush,
                );
            }

            let close_bounds = (selected_handle == Some(task.window_handle))
                .then(|| layout.close_button_bounds(bounds));

            let number = visible_position
                .to_string()
                .encode_utf16()
                .collect::<Vec<_>>();
            let title = task.title.encode_utf16().collect::<Vec<_>>();
            if options.show_numbers {
                target.DrawText(
                    &number,
                    &text.number,
                    &D2D_RECT_F {
                        left: bounds.left,
                        top: bounds.top,
                        right: bounds.left + layout.number_width,
                        bottom: bounds.bottom,
                    },
                    &resources.number_brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
            let content_left = bounds.left
                + if options.show_numbers {
                    layout.number_width
                } else {
                    0.0
                }
                + layout.icon_slot_width
                + layout.icon_text_gap;
            let text_layout = task_text_vertical_layout(
                bounds.top,
                bounds.bottom,
                options.show_app_names,
                options.compact_list,
            );
            target.DrawText(
                &title,
                &text.title,
                &D2D_RECT_F {
                    left: content_left,
                    top: text_layout.title_top,
                    right: close_bounds.map_or(bounds.right - 12.0, |button| {
                        button.left - layout.close_button_gap
                    }),
                    bottom: text_layout.title_bottom,
                },
                &resources.primary_brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            if let Some((app_name_top, app_name_bottom)) = text_layout.app_name {
                let app_name = task.process_name.encode_utf16().collect::<Vec<_>>();
                target.DrawText(
                    &app_name,
                    &text.detail,
                    &D2D_RECT_F {
                        left: content_left,
                        top: app_name_top,
                        right: close_bounds.map_or(bounds.right - 12.0, |button| {
                            button.left - layout.close_button_gap
                        }),
                        bottom: app_name_bottom,
                    },
                    &resources.secondary_brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
            if let Some(close_bounds) = close_bounds {
                let glyph = close_glyph_geometry(close_bounds, options.compact_list, scale);
                let background = match close_button_state {
                    CloseButtonVisualState::Normal => None,
                    CloseButtonVisualState::Hovered => Some(&resources.close_hover_brush),
                    CloseButtonVisualState::Pressed => Some(&resources.close_pressed_brush),
                };
                if let Some(background) = background {
                    target.FillRoundedRectangle(
                        &rounded_rect(close_bounds, layout.selection_radius - 1.0),
                        background,
                    );
                }
                target.DrawLine(
                    Vector2::new(glyph.bounds.left, glyph.bounds.top),
                    Vector2::new(glyph.bounds.right, glyph.bounds.bottom),
                    &resources.primary_brush,
                    glyph.stroke_width,
                    None,
                );
                target.DrawLine(
                    Vector2::new(glyph.bounds.right, glyph.bounds.top),
                    Vector2::new(glyph.bounds.left, glyph.bounds.bottom),
                    &resources.primary_brush,
                    glyph.stroke_width,
                    None,
                );
            }
        }

        target.EndDraw(None, None)
    }
}

const fn d2d_rect(rect: LogicalRect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

const fn rounded_rect(rect: LogicalRect, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: d2d_rect(rect),
        radiusX: radius,
        radiusY: radius,
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
