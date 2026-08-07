use alttabio::overlay_layout::{for_compact_list, layout_dpi, layout_scale};
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CloseButtonVisualState {
    #[default]
    Normal,
    Hovered,
    Pressed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskListHit {
    Task(usize),
    CloseButton(usize),
}

impl TaskListHit {
    pub const fn position(self) -> usize {
        match self {
            Self::Task(position) | Self::CloseButton(position) => position,
        }
    }
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

#[derive(Clone, Copy)]
struct WindowFrameGeometry {
    rounded_rect: D2D1_ROUNDED_RECT,
    stroke_width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CloseGlyphGeometry {
    bounds: D2D_RECT_F,
    stroke_width: f32,
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
        let target_dpi = f32::from(layout_dpi(window_dpi));
        unsafe {
            // SAFETY: the target is valid on this UI thread and both DPI values are finite and
            // positive, keeping its logical coordinate space synchronized with the live window.
            resources.target.SetDpi(target_dpi, target_dpi);
        }
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
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        reason = "Win32 mouse/client coordinates are bounded to an on-screen i32 rectangle"
    )]
    pub fn hit_test(
        hwnd: HWND,
        switcher: &mut Switcher,
        pixel_x: i32,
        pixel_y: i32,
        compact_list: bool,
    ) -> Option<TaskListHit> {
        let window_dpi = unsafe {
            // SAFETY: `hwnd` is the live overlay window and the call returns a scalar DPI value.
            GetDpiForWindow(hwnd)
        };
        let mut client = RECT::default();
        unsafe {
            // SAFETY: `client` is writable for the call and `hwnd` is the live overlay window.
            GetClientRect(hwnd, &raw mut client).ok()?;
        }
        hit_test_task_list_pixels(
            switcher,
            (
                client.right.saturating_sub(client.left),
                client.bottom.saturating_sub(client.top),
            ),
            (pixel_x, pixel_y),
            window_dpi,
            compact_list,
        )
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
            target.DrawRoundedRectangle(
                &raw const window_frame.rounded_rect,
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
            let bounds = D2D_RECT_F {
                left: layout.outer_padding,
                top,
                right: list_width,
                bottom: top + layout.row_height,
            };
            if selected_handle == Some(task.window_handle) {
                target.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: bounds,
                        radiusX: layout.selection_radius,
                        radiusY: layout.selection_radius,
                    },
                    &resources.selected_brush,
                );
            }

            let close_bounds = (selected_handle == Some(task.window_handle))
                .then(|| close_button_bounds(bounds, layout));

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
                        &D2D1_ROUNDED_RECT {
                            rect: close_bounds,
                            radiusX: layout.selection_radius - 1.0,
                            radiusY: layout.selection_radius - 1.0,
                        },
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

fn close_button_bounds(
    row_bounds: D2D_RECT_F,
    layout: alttabio::overlay_layout::OverlayLayout,
) -> D2D_RECT_F {
    let top = row_bounds.top + ((layout.row_height - layout.close_button_size) / 2.0);
    D2D_RECT_F {
        left: row_bounds.right - layout.close_button_inset - layout.close_button_size,
        top,
        right: row_bounds.right - layout.close_button_inset,
        bottom: top + layout.close_button_size,
    }
}

fn close_glyph_geometry(
    hit_target: D2D_RECT_F,
    compact_list: bool,
    scale: f32,
) -> CloseGlyphGeometry {
    let scale = scale.max(1.0);
    let nominal_extent = if compact_list { 8.0 } else { 10.0 };
    let extent = (nominal_extent * scale).round() / scale;
    let center_x = f32::midpoint(hit_target.left, hit_target.right);
    let center_y = f32::midpoint(hit_target.top, hit_target.bottom);
    let half_extent = extent / 2.0;
    CloseGlyphGeometry {
        bounds: D2D_RECT_F {
            left: center_x - half_extent,
            top: center_y - half_extent,
            right: center_x + half_extent,
            bottom: center_y + half_extent,
        },
        stroke_width: 1.5,
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "logical coordinates are bounded to the small on-screen task list"
)]
fn hit_test_task_list_at_scale(
    switcher: &mut Switcher,
    client_width: f32,
    client_height: f32,
    x: f32,
    y: f32,
    compact_list: bool,
    scale: f32,
) -> Option<TaskListHit> {
    let layout = for_compact_list(compact_list);
    let list_width = layout.list_width(client_width, scale);
    let list_top = layout.list_top();
    if x < layout.outer_padding || x >= list_width || y < list_top {
        return None;
    }

    let visible_rows = layout.visible_row_count(client_height);
    let start = switcher.visible_range(visible_rows).start;
    let row = layout.visible_row_at(client_height, y)?;
    let position = start + row + 1;
    if position > switcher.visible_task_count() {
        return None;
    }
    switcher.pin_visible_range(visible_rows);

    let row_top = list_top + (row as f32 * (layout.row_height + layout.row_gap));
    let row_bounds = D2D_RECT_F {
        left: layout.outer_padding,
        top: row_top,
        right: list_width,
        bottom: row_top + layout.row_height,
    };
    let selected_position = switcher.selected_visible_index().map(|index| index + 1);
    let close_bounds = close_button_bounds(row_bounds, layout);
    if selected_position == Some(position)
        && x >= close_bounds.left
        && x < close_bounds.right
        && y >= close_bounds.top
        && y < close_bounds.bottom
    {
        Some(TaskListHit::CloseButton(position))
    } else {
        Some(TaskListHit::Task(position))
    }
}

#[cfg(test)]
fn hit_test_task_list(
    switcher: &mut Switcher,
    client_width: f32,
    client_height: f32,
    x: f32,
    y: f32,
    compact_list: bool,
) -> Option<TaskListHit> {
    hit_test_task_list_at_scale(
        switcher,
        client_width,
        client_height,
        x,
        y,
        compact_list,
        1.0,
    )
}

#[allow(
    clippy::cast_precision_loss,
    reason = "Win32 client coordinates and DPI values are small integers represented as f32"
)]
fn hit_test_task_list_pixels(
    switcher: &mut Switcher,
    client_pixels: (i32, i32),
    point_pixels: (i32, i32),
    window_dpi: u32,
    compact_list: bool,
) -> Option<TaskListHit> {
    let scale = layout_scale(window_dpi);
    let logical_scale = 1.0 / scale;
    hit_test_task_list_at_scale(
        switcher,
        client_pixels.0 as f32 * logical_scale,
        client_pixels.1 as f32 * logical_scale,
        point_pixels.0 as f32 * logical_scale,
        point_pixels.1 as f32 * logical_scale,
        compact_list,
        scale,
    )
}

fn window_frame_geometry(width: f32, height: f32, scale: f32) -> WindowFrameGeometry {
    let pixel = 1.0 / scale;
    WindowFrameGeometry {
        rounded_rect: D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: pixel,
                top: pixel,
                right: (width - pixel).max(pixel),
                bottom: (height - pixel).max(pixel),
            },
            radiusX: 10.0,
            radiusY: 10.0,
        },
        stroke_width: 2.0 / scale,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TaskTextVerticalLayout {
    title_top: f32,
    title_bottom: f32,
    app_name: Option<(f32, f32)>,
}

fn task_text_vertical_layout(
    row_top: f32,
    row_bottom: f32,
    show_app_names: bool,
    compact_list: bool,
) -> TaskTextVerticalLayout {
    if show_app_names {
        if compact_list {
            TaskTextVerticalLayout {
                title_top: row_top + 1.0,
                title_bottom: row_top + 26.0,
                app_name: Some((row_top + 22.0, row_bottom - 1.0)),
            }
        } else {
            TaskTextVerticalLayout {
                title_top: row_top + 3.0,
                title_bottom: row_top + 35.0,
                app_name: Some((row_top + 31.0, row_bottom - 2.0)),
            }
        }
    } else {
        TaskTextVerticalLayout {
            title_top: row_top,
            title_bottom: row_bottom,
            app_name: None,
        }
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
    use alttabio::settings::Settings;
    use alttabio::switcher::SwitchTask;

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
    fn title_uses_the_full_row_when_app_names_are_hidden() {
        let layout = task_text_vertical_layout(20.0, 78.0, false, false);

        assert_close(layout.title_top, 20.0);
        assert_close(layout.title_bottom, 78.0);
        assert_eq!(layout.app_name, None);
    }

    #[test]
    fn title_moves_up_when_app_names_are_shown() {
        let layout = task_text_vertical_layout(20.0, 78.0, true, false);

        assert_close(layout.title_top, 23.0);
        assert_close(layout.title_bottom, 55.0);
        assert!(layout.app_name.is_some());
        let (app_name_top, app_name_bottom) = layout.app_name.unwrap_or_default();
        assert_close(app_name_top, 51.0);
        assert_close(app_name_bottom, 76.0);
    }

    #[test]
    fn compact_app_names_fit_the_shorter_row() {
        let layout = task_text_vertical_layout(18.0, 62.0, true, true);

        assert_close(layout.title_top, 19.0);
        assert_close(layout.title_bottom, 44.0);
        assert_eq!(layout.app_name, Some((40.0, 61.0)));
    }

    #[test]
    fn window_border_aligns_to_two_physical_pixels_at_fractional_dpi() {
        let scale = 1.5;
        let frame = window_frame_geometry(1_600.0, 900.0, scale);

        assert_close(frame.rounded_rect.rect.left * scale, 1.0);
        assert_close(frame.rounded_rect.rect.top * scale, 1.0);
        assert_close(frame.stroke_width * scale, 2.0);
    }

    #[test]
    fn windows_desktop_color_preserves_colorref_channel_order() {
        let background = color_from_colorref(0x00_2F_2C_2D);

        assert_close(background.r, 45.0 / 255.0);
        assert_close(background.g, 44.0 / 255.0);
        assert_close(background.b, 47.0 / 255.0);
        assert_close(background.a, 1.0);
    }

    #[test]
    fn close_button_is_inset_and_centered_in_each_selected_row_style() {
        let roomy = for_compact_list(false);
        let roomy_bounds = close_button_bounds(
            D2D_RECT_F {
                left: 20.0,
                top: 20.0,
                right: 414.0,
                bottom: 78.0,
            },
            roomy,
        );
        assert_eq!(
            roomy_bounds,
            D2D_RECT_F {
                left: 376.0,
                top: 34.0,
                right: 406.0,
                bottom: 64.0,
            }
        );

        let compact = for_compact_list(true);
        let compact_bounds = close_button_bounds(
            D2D_RECT_F {
                left: 18.0,
                top: 18.0,
                right: 260.0,
                bottom: 62.0,
            },
            compact,
        );
        assert_eq!(
            compact_bounds,
            D2D_RECT_F {
                left: 228.0,
                top: 28.0,
                right: 252.0,
                bottom: 52.0,
            }
        );
    }

    #[test]
    fn close_glyph_is_smaller_and_centered_inside_each_hit_target() {
        for (compact, scale, expected_hit_extent, expected_dip_extent, expected_physical_extent) in [
            (false, 1.0, 30.0, 10.0, 10.0),
            (false, 1.25, 30.0, 10.4, 13.0),
            (false, 1.5, 30.0, 10.0, 15.0),
            (true, 1.0, 24.0, 8.0, 8.0),
            (true, 1.25, 24.0, 8.0, 10.0),
            (true, 1.5, 24.0, 8.0, 12.0),
        ] {
            let layout = for_compact_list(compact);
            let row_bounds = D2D_RECT_F {
                left: layout.outer_padding,
                top: layout.outer_padding,
                right: layout.list_width(900.0, scale),
                bottom: layout.outer_padding + layout.row_height,
            };
            let hit_target = close_button_bounds(row_bounds, layout);
            let glyph = close_glyph_geometry(hit_target, compact, scale);

            let glyph_center_x = f32::midpoint(glyph.bounds.left, glyph.bounds.right);
            let glyph_center_y = f32::midpoint(glyph.bounds.top, glyph.bounds.bottom);
            let glyph_extent = glyph.bounds.right - glyph.bounds.left;
            let hit_extent = hit_target.right - hit_target.left;
            let hit_center_x = f32::midpoint(hit_target.left, hit_target.right);
            let hit_center_y = f32::midpoint(hit_target.top, hit_target.bottom);
            assert_near(glyph_center_x, hit_center_x);
            assert_near(glyph_center_y, hit_center_y);
            assert_near(glyph_center_x * scale, hit_center_x * scale);
            assert_near(glyph_center_y * scale, hit_center_y * scale);
            assert_near(hit_extent, expected_hit_extent);
            assert_near(hit_extent * scale, expected_hit_extent * scale);
            assert_near(glyph_extent, expected_dip_extent);
            assert_near(glyph_extent * scale, expected_physical_extent);
            assert!(glyph_extent < hit_extent);
            assert!(glyph_extent < hit_target.bottom - hit_target.top);
        }
    }

    #[test]
    fn hit_test_prioritizes_only_the_selected_rows_close_button() {
        let mut switcher = switcher_with_tasks(3);

        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 49.0, false),
            Some(TaskListHit::CloseButton(1))
        );
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 375.0, 49.0, false),
            Some(TaskListHit::Task(1))
        );
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 113.0, false),
            Some(TaskListHit::Task(2))
        );
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 81.0, false),
            None
        );

        assert!(switcher.select_visible_position(2));
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 113.0, false),
            Some(TaskListHit::CloseButton(2))
        );
    }

    #[test]
    fn hit_test_keeps_the_scrolled_selected_close_button_on_its_visible_row() {
        let mut switcher = switcher_with_tasks(10);
        assert!(switcher.select_visible_position(8));

        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 168.0, 390.0, 113.0, false),
            Some(TaskListHit::CloseButton(8))
        );
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 168.0, 390.0, 49.0, false),
            Some(TaskListHit::Task(7))
        );
    }

    #[test]
    fn hover_selection_does_not_move_the_task_under_a_stationary_cursor() {
        let mut switcher = switcher_with_tasks(10);
        assert!(switcher.select_visible_position(8));

        let hit = hit_test_task_list(&mut switcher, 900.0, 168.0, 375.0, 49.0, false);
        assert_eq!(hit, Some(TaskListHit::Task(7)));
        assert!(hit.is_some_and(|hit| switcher.select_visible_position(hit.position())));

        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 168.0, 375.0, 49.0, false),
            Some(TaskListHit::Task(7))
        );
    }

    #[test]
    fn default_typed_search_maps_hidden_fractional_dpi_rows_to_mouse_hits() {
        let defaults = Settings::default();
        assert!(defaults.appearance.compact_list);
        assert!(defaults.general.typed_search);

        assert_fractional_dpi_pixel_grid(defaults.appearance.compact_list);
    }

    #[test]
    fn normal_list_maps_every_fractional_dpi_pixel_to_the_row_drawn_there() {
        assert_fractional_dpi_pixel_grid(false);
    }

    #[test]
    fn hidden_typed_search_and_close_button_share_no_box_geometry() {
        let mut switcher = switcher_with_tasks(3);

        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 49.0, false),
            Some(TaskListHit::CloseButton(1))
        );
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 81.0, false),
            None
        );
    }

    #[test]
    fn typed_filtering_uses_hidden_geometry_for_mouse_hit_testing() {
        let defaults = Settings::default();
        assert!(defaults.general.typed_search);
        let mut switcher = Switcher::default();
        switcher.set_tasks([
            SwitchTask::new(1, 1, "Editor", "editor"),
            SwitchTask::new(2, 2, "Browser", "browser"),
        ]);
        switcher.append_filter_character('b');

        assert_eq!(switcher.visible_task_count(), 1);
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 49.0, false),
            Some(TaskListHit::CloseButton(1))
        );
        assert_eq!(
            hit_test_task_list(&mut switcher, 900.0, 600.0, 390.0, 99.0, false),
            None
        );
    }

    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the known positive test coordinates fit inside the fixed i32 client rectangle"
    )]
    fn dpi_192_caps_presentation_and_keeps_close_hit_aligned_with_glyph() {
        const WINDOW_DPI: u32 = 192;
        let scale = layout_scale(WINDOW_DPI);
        assert_eq!(layout_dpi(WINDOW_DPI), 168);
        assert_near(scale, 1.75);

        let layout = for_compact_list(false);
        let row_bounds = D2D_RECT_F {
            left: layout.outer_padding,
            top: layout.list_top(),
            right: layout.list_width(900.0, scale),
            bottom: layout.list_top() + layout.row_height,
        };
        let hit_target = close_button_bounds(row_bounds, layout);
        let glyph = close_glyph_geometry(hit_target, false, scale);
        let center_x = f32::midpoint(hit_target.left, hit_target.right);
        let center_y = f32::midpoint(hit_target.top, hit_target.bottom);
        assert_near(hit_target.right - hit_target.left, 30.0);
        assert_near((glyph.bounds.right - glyph.bounds.left) * scale, 18.0);
        assert_near(
            f32::midpoint(glyph.bounds.left, glyph.bounds.right) * scale,
            center_x * scale,
        );
        assert_near(
            f32::midpoint(glyph.bounds.top, glyph.bounds.bottom) * scale,
            center_y * scale,
        );

        let mut switcher = switcher_with_tasks(3);
        assert_eq!(
            hit_test_task_list_pixels(
                &mut switcher,
                (1_575, 900),
                (
                    (center_x * scale).round() as i32,
                    (center_y * scale).round() as i32,
                ),
                WINDOW_DPI,
                false,
            ),
            Some(TaskListHit::CloseButton(1))
        );
    }

    #[test]
    fn mouse_hit_pins_viewport_until_keyboard_navigation_recenters() {
        let mut switcher = switcher_with_tasks(10);
        assert!(switcher.select_visible_position(8));
        assert_eq!(switcher.visible_range(2), 6..8);

        let hit = hit_test_task_list(&mut switcher, 900.0, 168.0, 375.0, 49.0, false);
        assert_eq!(hit, Some(TaskListHit::Task(7)));
        assert!(hit.is_some_and(|hit| switcher.select_visible_position(hit.position())));
        assert_eq!(switcher.visible_range(2), 6..8);

        switcher.select_bounded(-1);
        assert_eq!(switcher.visible_range(2), 4..6);
    }

    #[test]
    fn first_and_last_selection_stay_inside_the_rendered_range() {
        let mut switcher = switcher_with_tasks(10);
        let visible_rows = 3;

        switcher.select_first();
        let first_range = switcher.visible_range(visible_rows);
        assert!(first_range.contains(&switcher.selected_visible_index().unwrap_or_default()));

        switcher.select_last();
        let last_range = switcher.visible_range(visible_rows);
        assert!(last_range.contains(&switcher.selected_visible_index().unwrap_or_default()));
    }

    fn switcher_with_tasks(count: usize) -> Switcher {
        let mut switcher = Switcher::default();
        switcher.set_tasks((1..=count).map(|number| {
            let title = format!("Task {number}");
            SwitchTask::new(
                number,
                isize::try_from(number).unwrap_or_default(),
                &title,
                "app",
            )
        }));
        switcher
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "known positive logical test coordinates fit exactly inside an i32 client area"
    )]
    fn assert_fractional_dpi_pixel_grid(compact_list: bool) {
        const DPI: u32 = 120;
        const CLIENT_PIXEL_WIDTH: i32 = 1_125;
        const CLIENT_PIXEL_HEIGHT: i32 = 210;
        const EXPECTED_VISIBLE_START: usize = 6;

        let mut switcher = switcher_with_tasks(10);
        assert!(switcher.select_visible_position(8));
        assert_pixel_grid(&mut switcher, compact_list, EXPECTED_VISIBLE_START);

        let layout = for_compact_list(compact_list);
        let scale = layout_scale(DPI);
        let row_x = ((layout.outer_padding + 1.0) * scale).ceil() as i32;
        let row_y = ((layout.list_top() + 1.0) * scale).ceil() as i32;
        let hovered = hit_test_task_list_pixels(
            &mut switcher,
            (CLIENT_PIXEL_WIDTH, CLIENT_PIXEL_HEIGHT),
            (row_x, row_y),
            DPI,
            compact_list,
        );
        assert_eq!(hovered, Some(TaskListHit::Task(7)));
        assert!(hovered.is_some_and(|hit| switcher.select_visible_position(hit.position())));

        assert_pixel_grid(&mut switcher, compact_list, EXPECTED_VISIBLE_START);
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "the exhaustive test grid uses small client pixel coordinates"
    )]
    fn assert_pixel_grid(
        switcher: &mut Switcher,
        compact_list: bool,
        expected_visible_start: usize,
    ) {
        const DPI: u32 = 120;
        const CLIENT_PIXEL_WIDTH: i32 = 1_125;
        const CLIENT_PIXEL_HEIGHT: i32 = 210;
        let logical_scale = 1.0 / layout_scale(DPI);
        let client_width = CLIENT_PIXEL_WIDTH as f32 * logical_scale;
        let client_height = CLIENT_PIXEL_HEIGHT as f32 * logical_scale;

        for pixel_y in 0..CLIENT_PIXEL_HEIGHT {
            for pixel_x in 0..CLIENT_PIXEL_WIDTH {
                let expected = expected_hit(
                    switcher,
                    client_width,
                    client_height,
                    pixel_x as f32 * logical_scale,
                    pixel_y as f32 * logical_scale,
                    compact_list,
                    expected_visible_start,
                );
                let actual = hit_test_task_list_pixels(
                    switcher,
                    (CLIENT_PIXEL_WIDTH, CLIENT_PIXEL_HEIGHT),
                    (pixel_x, pixel_y),
                    DPI,
                    compact_list,
                );
                assert_eq!(
                    actual, expected,
                    "unexpected hit at physical pixel ({pixel_x}, {pixel_y})"
                );
            }
        }
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "the exhaustive test grid maps a small row index to logical coordinates"
    )]
    fn expected_hit(
        switcher: &Switcher,
        client_width: f32,
        client_height: f32,
        x: f32,
        y: f32,
        compact_list: bool,
        visible_start: usize,
    ) -> Option<TaskListHit> {
        let layout = for_compact_list(compact_list);
        let list_width = layout.list_width(client_width, layout_scale(120));
        let list_top = layout.list_top();
        if x < layout.outer_padding || x >= list_width || y < list_top {
            return None;
        }

        let row = layout.visible_row_at(client_height, y)?;
        let position = visible_start + row + 1;
        if position > switcher.visible_task_count() {
            return None;
        }

        let row_top = list_top + (row as f32 * (layout.row_height + layout.row_gap));
        let close_bounds = close_button_bounds(
            D2D_RECT_F {
                left: layout.outer_padding,
                top: row_top,
                right: list_width,
                bottom: row_top + layout.row_height,
            },
            layout,
        );
        if switcher.selected_visible_index().map(|index| index + 1) == Some(position)
            && x >= close_bounds.left
            && x < close_bounds.right
            && y >= close_bounds.top
            && y < close_bounds.bottom
        {
            Some(TaskListHit::CloseButton(position))
        } else {
            Some(TaskListHit::Task(position))
        }
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < f32::EPSILON);
    }

    fn assert_near(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 0.001);
    }
}
