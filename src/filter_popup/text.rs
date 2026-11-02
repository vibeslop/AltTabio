//! DirectWrite selection, caret placement and horizontal scrolling for the query.

use alttabio::text_edit::QueryText;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_DRAW_TEXT_OPTIONS_NONE, ID2D1DCRenderTarget,
    ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_HIT_TEST_METRICS, DWRITE_TEXT_METRICS, IDWriteFactory, IDWriteTextFormat,
    IDWriteTextLayout,
};
use windows::core::{BOOL, Result};
use windows_numerics::Vector2;

pub(super) struct TextRun {
    layout: IDWriteTextLayout,
    origin: Vector2,
    clip: D2D_RECT_F,
    caret_x: f32,
    selection: Vec<DWRITE_HIT_TEST_METRICS>,
    pub scroll: f32,
}

impl TextRun {
    pub fn new(
        factory: &IDWriteFactory,
        format: &IDWriteTextFormat,
        editor: &QueryText,
        width: f32,
        height: f32,
        previous_scroll: f32,
    ) -> Result<Self> {
        let clip = D2D_RECT_F {
            left: 32.0,
            top: 0.0,
            right: width - 35.0,
            bottom: height,
        };
        let room = (clip.right - clip.left).max(1.0);
        let text = editor.text();
        let encoded = text.encode_utf16().collect::<Vec<_>>();
        let layout = unsafe {
            // SAFETY: all UTF-16 text, COM interfaces and bounded dimensions outlive the call.
            factory.CreateTextLayout(&encoded, format, room, height)?
        };
        let caret_position =
            u32::try_from(text[..editor.caret()].encode_utf16().count()).unwrap_or(u32::MAX);
        let (mut x, mut y) = (0.0, 0.0);
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            // SAFETY: the position is a valid text boundary and every output is writable.
            layout.HitTestTextPosition(
                caret_position,
                false,
                &raw mut x,
                &raw mut y,
                &raw mut metrics,
            )?;
        }
        let mut layout_metrics = DWRITE_TEXT_METRICS::default();
        unsafe {
            // SAFETY: the layout remains live and this initialized metrics output is writable.
            layout.GetMetrics(&raw mut layout_metrics)?;
        }
        let scroll = if x < previous_scroll {
            x
        } else {
            previous_scroll.max(x - room + 3.0)
        }
        .max(0.0)
        .min((layout_metrics.widthIncludingTrailingWhitespace - room + 3.0).max(0.0));
        let origin = Vector2::new(clip.left - scroll, 0.0);
        let selected = editor.selection();
        let mut selection = Vec::new();
        if !selected.is_empty() {
            let start =
                u32::try_from(text[..selected.start].encode_utf16().count()).unwrap_or(u32::MAX);
            let length = u32::try_from(text[selected].encode_utf16().count()).unwrap_or(u32::MAX);
            // A single-line text range has no more regions than its UTF-16 units.
            selection.resize(
                encoded.len().saturating_add(1),
                DWRITE_HIT_TEST_METRICS::default(),
            );
            let mut count = 0;
            unsafe {
                // SAFETY: positions derive from valid text boundaries, the supplied slice owns
                // every region Windows may write, and count is a writable scalar output.
                layout.HitTestTextRange(
                    start,
                    length,
                    origin.X,
                    0.0,
                    Some(&mut selection),
                    &raw mut count,
                )?;
            }
            selection.truncate(usize::try_from(count).unwrap_or_default());
        }
        Ok(Self {
            layout,
            origin,
            clip,
            caret_x: (origin.X + x + 1.0).min(clip.right - 1.0),
            selection,
            scroll,
        })
    }

    pub fn draw(
        &self,
        target: &ID2D1DCRenderTarget,
        text: &ID2D1SolidColorBrush,
        selected: &ID2D1SolidColorBrush,
    ) {
        unsafe {
            // SAFETY: the caller owns this live target's BeginDraw/EndDraw pair, every COM
            // resource stays live, and the axis-aligned clip is popped on the same path.
            target.PushAxisAlignedClip(&raw const self.clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            for region in &self.selection {
                target.FillRectangle(
                    &D2D_RECT_F {
                        left: region.left,
                        top: region.top,
                        right: region.left + region.width,
                        bottom: region.top + region.height,
                    },
                    selected,
                );
            }
            target.DrawTextLayout(self.origin, &self.layout, text, D2D1_DRAW_TEXT_OPTIONS_NONE);
            if self.selection.is_empty() {
                target.DrawLine(
                    Vector2::new(self.caret_x, 10.0),
                    Vector2::new(self.caret_x, self.clip.bottom - 10.0),
                    text,
                    1.0,
                    None,
                );
            }
            target.PopAxisAlignedClip();
        }
    }

    pub fn hit_test(&self, x: f32) -> Result<u32> {
        let mut trailing = BOOL::default();
        let mut inside = BOOL::default();
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            // SAFETY: the layout is live and all hit-test outputs are writable for this call.
            self.layout.HitTestPoint(
                x - self.origin.X,
                self.clip.bottom / 2.0,
                &raw mut trailing,
                &raw mut inside,
                &raw mut metrics,
            )?;
        }
        Ok(metrics.textPosition.saturating_add(if trailing.as_bool() {
            metrics.length
        } else {
            0
        }))
    }
}
