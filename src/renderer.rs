use alttabio::close_button::CloseButtonVisualState;
use alttabio::overlay_layout::{
    LogicalRect, OverlayLayout, close_glyph_geometry, for_compact_list, layout_dpi, layout_scale,
    task_text_vertical_layout, window_frame_geometry,
};
use alttabio::settings::AppearanceSettings;
use alttabio::switcher::{SwitchTask, Switcher};
use alttabio::theme::{ResolvedTheme, Rgb8};
use std::ffi::c_void;
use std::fmt;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_F, D2D_SIZE_U, D2D1_COLOR_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_ROUNDED_RECT, D2D1CreateFactory, ID2D1Factory,
    ID2D1HwndRenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Gdi::{COLOR_BACKGROUND, GetSysColor, HDC};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx, GetClientRect, HICON};
use windows::core::{Error, Result, w};
use windows_numerics::Vector2;

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

/// What one paint draws with. The parts it draws run only between the `BeginDraw` and `EndDraw`
/// of `draw_switcher`.
struct Canvas<'a> {
    resources: &'a RenderResources,
    text: &'a TextFormats,
    layout: OverlayLayout,
    options: RenderOptions,
    scale: f32,
    close_button_state: CloseButtonVisualState,
}

impl Canvas<'_> {
    fn draw_switcher(
        &self,
        switcher: &Switcher,
        preview_frame: Option<PreviewFrame>,
        utf16: &mut Vec<u16>,
    ) -> Result<()> {
        let target = &self.resources.target;
        let size = unsafe {
            // SAFETY: the target is valid for this UI-thread paint operation.
            target.GetSize()
        };
        let layout = self.layout;
        let list_width = layout.list_width(size.width, self.scale);
        let visible_rows = layout.visible_row_count(size.height);
        let start = switcher.visible_range(visible_rows).start;
        let selected_handle = switcher.selected_task().map(|task| task.window_handle);

        unsafe {
            // SAFETY: the target is valid on this UI thread and the color outlives the call.
            target.BeginDraw();
            target.Clear(Some(&raw const self.resources.background_color));
        }
        if self.options.visible_borders {
            self.draw_window_border(size);
        }
        if let Some(frame) = preview_frame {
            self.draw_preview_frame(frame);
        }
        self.draw_divider(list_width, size.height);
        for (visible_position, task) in switcher
            .positioned_visible_tasks()
            .skip(start)
            .take(visible_rows)
        {
            let visible_index = visible_position.saturating_sub(1);
            self.draw_row(
                layout.row_bounds(visible_index - start, list_width),
                selected_handle == Some(task.window_handle),
                visible_position,
                task,
                utf16,
            );
        }
        unsafe {
            // SAFETY: this ends the BeginDraw above on the same target.
            target.EndDraw(None, None)
        }
    }

    fn draw_window_border(&self, size: D2D_SIZE_F) {
        let frame = window_frame_geometry(size.width, size.height, self.scale);
        let rect = rounded_rect(frame.rect, frame.radius);
        unsafe {
            // SAFETY: the target and brush are valid on this UI thread and `rect` outlives the
            // call.
            self.resources.target.DrawRoundedRectangle(
                &raw const rect,
                &self.resources.window_border_brush,
                frame.stroke_width,
                None,
            );
        }
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "the preview frame is an on-screen pixel rectangle represented exactly as f32"
    )]
    fn draw_preview_frame(&self, frame: PreviewFrame) {
        let rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: frame.rect.left as f32 / frame.scale + 0.5,
                top: frame.rect.top as f32 / frame.scale + 0.5,
                right: frame.rect.right as f32 / frame.scale - 0.5,
                bottom: frame.rect.bottom as f32 / frame.scale - 0.5,
            },
            radiusX: 3.0,
            radiusY: 3.0,
        };
        let target = &self.resources.target;
        unsafe {
            // SAFETY: the target and brushes are valid on this UI thread and `rect` outlives both
            // calls.
            target.FillRoundedRectangle(&raw const rect, &self.resources.preview_background_brush);
            if self.options.visible_borders {
                target.DrawRoundedRectangle(
                    &raw const rect,
                    &self.resources.preview_border_brush,
                    1.0,
                    None,
                );
            }
        }
    }

    fn draw_divider(&self, list_width: f32, height: f32) {
        let padding = self.layout.outer_padding;
        let divider = D2D_RECT_F {
            left: list_width + padding,
            top: padding,
            right: list_width + padding + 1.0,
            bottom: height - padding,
        };
        unsafe {
            // SAFETY: the target and brush are valid on this UI thread and `divider` outlives the
            // call.
            self.resources
                .target
                .FillRectangle(&raw const divider, &self.resources.divider_brush);
        }
    }

    fn draw_row(
        &self,
        bounds: LogicalRect,
        selected: bool,
        visible_position: usize,
        task: &SwitchTask,
        utf16: &mut Vec<u16>,
    ) {
        let layout = self.layout;
        let options = self.options;
        let close_button = selected.then(|| layout.close_button_bounds(bounds));
        if selected {
            unsafe {
                // SAFETY: the target and brush are valid on this UI thread and the rectangle
                // outlives the call.
                self.resources.target.FillRoundedRectangle(
                    &rounded_rect(bounds, layout.selection_radius),
                    &self.resources.selected_brush,
                );
            }
        }

        if options.show_numbers {
            self.draw_text(
                encode_number(utf16, visible_position),
                &self.text.number,
                LogicalRect {
                    right: bounds.left + layout.number_width,
                    ..bounds
                },
                &self.resources.number_brush,
            );
        }
        let left = layout.text_left(options.show_numbers);
        let right = layout.text_right(bounds, close_button);
        let text_layout = task_text_vertical_layout(
            bounds.top,
            bounds.bottom,
            options.show_app_names,
            options.compact_list,
        );
        self.draw_text(
            encode(utf16, &task.title),
            &self.text.title,
            LogicalRect {
                left,
                top: text_layout.title_top,
                right,
                bottom: text_layout.title_bottom,
            },
            &self.resources.primary_brush,
        );
        if let Some((top, bottom)) = text_layout.app_name {
            self.draw_text(
                encode(utf16, &task.process_name),
                &self.text.detail,
                LogicalRect {
                    left,
                    top,
                    right,
                    bottom,
                },
                &self.resources.secondary_brush,
            );
        }
        if let Some(close_button) = close_button {
            self.draw_close_button(close_button);
        }
    }

    fn draw_text(
        &self,
        text: &[u16],
        format: &IDWriteTextFormat,
        bounds: LogicalRect,
        brush: &ID2D1SolidColorBrush,
    ) {
        unsafe {
            // SAFETY: the target, format and brush are valid on this UI thread, and the text and
            // rectangle outlive the synchronous call.
            self.resources.target.DrawText(
                text,
                format,
                &d2d_rect(bounds),
                brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    fn draw_close_button(&self, bounds: LogicalRect) {
        let resources = self.resources;
        let target = &resources.target;
        let background = match self.close_button_state {
            CloseButtonVisualState::Normal => None,
            CloseButtonVisualState::Hovered => Some(&resources.close_hover_brush),
            CloseButtonVisualState::Pressed => Some(&resources.close_pressed_brush),
        };
        let glyph = close_glyph_geometry(bounds, self.options.compact_list, self.scale);
        unsafe {
            // SAFETY: the target and brushes are valid on this UI thread and the rectangle
            // outlives the call.
            if let Some(background) = background {
                target.FillRoundedRectangle(
                    &rounded_rect(bounds, self.layout.selection_radius - 1.0),
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
}

enum IconPassFailure {
    ClientArea(Error),
    Icons { failed: usize, first: Error },
}

impl fmt::Display for IconPassFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClientArea(error) => write!(
                formatter,
                "Could not read the overlay's client area to draw task icons: {error}"
            ),
            Self::Icons { failed: 1, first } => {
                write!(formatter, "Could not draw a task icon: {first}")
            }
            Self::Icons { failed, first } => {
                write!(formatter, "Could not draw {failed} task icons: {first}")
            }
        }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "icon geometry is clamped to the on-screen client area and Win32 HICON values"
)]
fn draw_icon_pass(
    hwnd: HWND,
    hdc: HDC,
    switcher: &Switcher,
    options: RenderOptions,
) -> std::result::Result<(), IconPassFailure> {
    let window_dpi = unsafe {
        // SAFETY: hwnd is the live overlay window and the call returns a scalar DPI value.
        GetDpiForWindow(hwnd)
    };
    let scale = layout_scale(window_dpi);
    let mut client = RECT::default();
    unsafe {
        // SAFETY: client is writable and hwnd is the live overlay window.
        GetClientRect(hwnd, &raw mut client)
    }
    .map_err(IconPassFailure::ClientArea)?;
    let height = client.bottom.saturating_sub(client.top) as f32 / scale;
    let layout = for_compact_list(options.compact_list);
    let visible_rows = layout.visible_row_count(height);
    let start = switcher.visible_range(visible_rows).start;
    let icon_pixels = (layout.icon_size(options.large_icons) * scale).round() as i32;

    let mut failed = 0;
    let mut first_error = None;
    for (visible_position, task) in switcher
        .positioned_visible_tasks()
        .skip(start)
        .take(visible_rows)
    {
        let visible_index = visible_position.saturating_sub(1);
        if task.icon_handle == 0 {
            continue;
        }
        let bounds = layout.icon_bounds(
            visible_index - start,
            options.show_numbers,
            options.large_icons,
        );
        let icon = HICON(task.icon_handle as *mut c_void);
        let result = unsafe {
            // SAFETY: hdc is the current BeginPaint DC, the HICON is borrowed from a live
            // window/class snapshot, and all pixel dimensions are positive and on-screen.
            DrawIconEx(
                hdc,
                (bounds.left * scale).round() as i32,
                (bounds.top * scale).round() as i32,
                icon,
                icon_pixels,
                icon_pixels,
                0,
                None,
                DI_NORMAL,
            )
        };
        if let Err(error) = result {
            failed += 1;
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), |first| {
        Err(IconPassFailure::Icons { failed, first })
    })
}

fn encode<'a>(buffer: &'a mut Vec<u16>, text: &str) -> &'a [u16] {
    buffer.clear();
    buffer.extend(text.encode_utf16());
    buffer
}

fn encode_number(buffer: &mut Vec<u16>, number: usize) -> &[u16] {
    buffer.clear();
    let mut rest = number;
    loop {
        let digit = u16::try_from(rest % 10).unwrap_or_default();
        buffer.push(u16::from(b'0') + digit);
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    buffer.reverse();
    buffer
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

    #[test]
    fn row_text_reuses_one_buffer() {
        let mut buffer = Vec::new();

        for (number, expected) in [(0, "0"), (7, "7"), (10, "10"), (123, "123")] {
            assert_eq!(
                encode_number(&mut buffer, number),
                expected.encode_utf16().collect::<Vec<_>>()
            );
        }
        assert_eq!(
            encode(&mut buffer, "Größe 📁"),
            "Größe 📁".encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(encode(&mut buffer, ""), []);
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < f32::EPSILON);
    }
}
