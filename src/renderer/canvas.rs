use super::{PreviewFrame, RenderOptions, RenderResources, TextFormats};
use alttabio::close_button::CloseButtonVisualState;
use alttabio::overlay_layout::{
    LogicalRect, OverlayLayout, close_glyph_geometry, task_text_vertical_layout,
    window_frame_geometry,
};
use alttabio::switcher::{SwitchTask, Switcher};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D_SIZE_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ROUNDED_RECT, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{DWRITE_MEASURING_MODE_NATURAL, IDWriteTextFormat};
use windows::core::Result;
use windows_numerics::Vector2;

/// What one paint draws with. The parts it draws run only between the `BeginDraw` and `EndDraw`
/// of `draw_switcher`.
pub(super) struct Canvas<'a> {
    pub(super) resources: &'a RenderResources,
    pub(super) text: &'a TextFormats,
    pub(super) layout: OverlayLayout,
    pub(super) options: RenderOptions,
    pub(super) scale: f32,
    pub(super) close_button_state: CloseButtonVisualState,
}

impl Canvas<'_> {
    pub(super) fn draw_switcher(
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
