//! Drawing a `FrameModel` with `AppKit`, from the shared panel geometry and theme tokens.

use super::{FrameModel, PreviewModel, Row};
use alttabio::close_button::CloseButtonVisualState;
use alttabio::panel_layout::{
    CLOSE_SIZE, ICON_SHARE, NAME_HEIGHT, NUMBER_WIDTH, PADDING, PLATE_RADIUS, PREVIEW_INSET,
    ROW_HEIGHT, STATE_GAP, TEXT_INSET, close_button_rect, image_area,
};
use alttabio::preview_layout::{Rect, Size, fit};
use alttabio::theme::{Rgb8, Rgba, SwitcherTokens};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSBezierPath, NSColor, NSCompositingOperation, NSFont, NSFontAttributeName, NSFontWeightMedium,
    NSFontWeightRegular, NSForegroundColorAttributeName, NSGraphicsContext, NSImage,
    NSLineBreakMode, NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSStringDrawing,
    NSTextAlignment,
};
use objc2_foundation::{NSAttributedStringKey, NSDictionary, NSPoint, NSRect, NSSize, NSString};

/// The frame's colors as `NSColor`s, straight from the shared semantic tokens.
struct Colors {
    label: Retained<NSColor>,
    secondary: Retained<NSColor>,
    ring: Retained<NSColor>,
    well: Retained<NSColor>,
    selection: Retained<NSColor>,
    control_hover: Retained<NSColor>,
    control_pressed: Retained<NSColor>,
}

fn colors(tokens: SwitcherTokens) -> Colors {
    Colors {
        label: color(tokens.text, 1.0),
        secondary: color(tokens.text_secondary, 1.0),
        ring: rgba(tokens.ring),
        well: rgba(tokens.well),
        selection: rgba(tokens.selection),
        control_hover: rgba(tokens.control_hover),
        control_pressed: rgba(tokens.control_pressed),
    }
}

pub(super) fn rgba(value: Rgba) -> Retained<NSColor> {
    color(value.color, value.alpha)
}

fn color(value: Rgb8, alpha: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(value.red) / 255.0,
        f64::from(value.green) / 255.0,
        f64::from(value.blue) / 255.0,
        alpha,
    )
}

struct Fonts {
    title: Retained<NSFont>,
    name: Retained<NSFont>,
    detail: Retained<NSFont>,
    number: Retained<NSFont>,
}

fn fonts() -> Fonts {
    unsafe {
        // SAFETY: the font weight constants are static values exported by AppKit.
        Fonts {
            title: NSFont::systemFontOfSize_weight(13.0, NSFontWeightRegular),
            name: NSFont::systemFontOfSize_weight(12.0, NSFontWeightMedium),
            detail: NSFont::systemFontOfSize_weight(12.0, NSFontWeightRegular),
            number: NSFont::monospacedDigitSystemFontOfSize_weight(13.0, NSFontWeightRegular),
        }
    }
}

fn text_attributes(
    font: &NSFont,
    color: &NSColor,
    alignment: NSTextAlignment,
) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
    let style = NSMutableParagraphStyle::new();
    style.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    style.setAlignment(alignment);
    let keys: [&NSAttributedStringKey; 3] = unsafe {
        // SAFETY: the attribute name constants are static strings exported by AppKit.
        [
            NSFontAttributeName,
            NSForegroundColorAttributeName,
            NSParagraphStyleAttributeName,
        ]
    };
    let objects: [&AnyObject; 3] = [font, color, &style];
    NSDictionary::from_slices(&keys, &objects)
}

fn measure(text: &str, font: &NSFont) -> NSSize {
    let attributes = text_attributes(font, &NSColor::labelColor(), NSTextAlignment::Left);
    unsafe {
        // SAFETY: the attributes dictionary is live for the synchronous measurement.
        NSString::from_str(text).sizeWithAttributes(Some(&attributes))
    }
}

fn draw_text(text: &str, bounds: Rect, font: &NSFont, color: &NSColor, alignment: NSTextAlignment) {
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }
    let attributes = text_attributes(font, color, alignment);
    let string = NSString::from_str(text);
    let size = unsafe {
        // SAFETY: the attributes dictionary is live for the synchronous measurement.
        string.sizeWithAttributes(Some(&attributes))
    };
    let height = size.height.min(bounds.height);
    let centered = Rect {
        top: bounds.top + (bounds.height - height) / 2.0,
        height,
        ..bounds
    };
    unsafe {
        // SAFETY: drawing happens inside drawRect: with a current graphics context.
        string.drawInRect_withAttributes(ns_rect(centered), Some(&attributes));
    }
}

fn fill_rounded(rect: Rect, radius: f64, color: &NSColor) {
    color.setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(rect), radius, radius).fill();
}

/// Strokes a 1pt ring just inside `rect`, the way an inset outline sits on an image.
fn ring_rounded(rect: Rect, radius: f64, color: &NSColor) {
    let inset = Rect {
        left: rect.left + 0.5,
        top: rect.top + 0.5,
        width: (rect.width - 1.0).max(0.0),
        height: (rect.height - 1.0).max(0.0),
    };
    color.setStroke();
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        ns_rect(inset),
        (radius - 0.5).max(0.0),
        (radius - 0.5).max(0.0),
    );
    path.setLineWidth(1.0);
    path.stroke();
}

fn ns_rect(rect: Rect) -> NSRect {
    NSRect::new(
        NSPoint::new(rect.left, rect.top),
        NSSize::new(rect.width, rect.height),
    )
}

/// Where `size` lands when aspect-fitted and centered in `bounds`.
fn fitted(size: NSSize, bounds: Rect) -> Option<Rect> {
    let rect = fit(bounds, Size::new(size.width, size.height));
    (!rect.is_empty()).then_some(rect)
}

/// Draws `image` aspect-fitted into `bounds` and returns where it landed.
fn draw_image_fit(image: &NSImage, bounds: Rect) -> Option<Rect> {
    let rect = fitted(image.size(), bounds)?;
    unsafe {
        // SAFETY: drawing happens inside drawRect: with a current graphics context.
        image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
            ns_rect(rect),
            NSRect::ZERO,
            NSCompositingOperation::SourceOver,
            1.0,
            true,
            None,
        );
    }
    Some(rect)
}

pub(super) fn draw_frame(model: &FrameModel) {
    let fonts = fonts();
    let colors = colors(model.tokens);
    draw_strip(model, &fonts, &colors);
    let layout = model.layout;
    if let Some(note) = &model.empty_note {
        draw_note(note, layout.row_rect(0), 0.0, &fonts, &colors);
    }
    for (index, row) in model.rows.iter().enumerate() {
        draw_row(model, row, layout.row_rect(index), &fonts, &colors);
    }
    if let Some(note) = &model.more_note {
        // Under numbered rows, the count lines up with their titles.
        draw_note(
            note,
            layout.row_rect(model.rows.len()),
            NUMBER_WIDTH,
            &fonts,
            &colors,
        );
    }
    if let (Some(preview), Some(area)) = (&model.preview, layout.preview_rect()) {
        draw_preview(preview, area, &fonts, &colors);
    }
}

fn draw_strip(model: &FrameModel, fonts: &Fonts, colors: &Colors) {
    let layout = model.layout;
    for (slot, tile) in model.tiles.iter().enumerate() {
        let rect = layout.tile_rect(slot);
        if tile.selected {
            fill_rounded(rect, PLATE_RADIUS, &colors.selection);
        }
        let icon = layout.tile * ICON_SHARE;
        let icon_rect = Rect {
            left: rect.left + (rect.width - icon) / 2.0,
            top: rect.top + (rect.height - icon) / 2.0,
            width: icon,
            height: icon,
        };
        if let Some(image) = &tile.icon {
            let _ = draw_image_fit(image, icon_rect);
        } else {
            let initial = tile.name.chars().take(1).collect::<String>();
            draw_text(
                &initial,
                icon_rect,
                &fonts.name,
                &colors.secondary,
                NSTextAlignment::Center,
            );
        }
        if tile.selected {
            // The name sits centered under its tile, pushed inward at the panel's edges.
            let width = measure(&tile.name, &fonts.name)
                .width
                .ceil()
                .min(layout.content_width());
            let left = (rect.left + (rect.width - width) / 2.0)
                .clamp(PADDING, layout.width - PADDING - width);
            draw_text(
                &tile.name,
                Rect {
                    left,
                    top: layout.name_top(),
                    width,
                    height: NAME_HEIGHT,
                },
                &fonts.name,
                &colors.label,
                NSTextAlignment::Center,
            );
        }
    }
}

fn draw_row(model: &FrameModel, row: &Row, bounds: Rect, fonts: &Fonts, colors: &Colors) {
    if row.selected {
        fill_rounded(bounds, PLATE_RADIUS, &colors.selection);
    }
    let mut right = bounds.right() - TEXT_INSET;
    if row.selected {
        let button = close_button_rect(bounds);
        draw_close_button(model.close_state, button, colors);
        right = button.left - STATE_GAP / 2.0;
    }
    if let Some(label) = row.state.label() {
        let width = measure(label, &fonts.detail).width.ceil();
        draw_text(
            label,
            Rect {
                left: right - width,
                width,
                ..bounds
            },
            &fonts.detail,
            &colors.secondary,
            NSTextAlignment::Right,
        );
        right -= width + STATE_GAP;
    }
    let left = bounds.left + TEXT_INSET;
    if let Some(number) = row.number {
        draw_text(
            &number.to_string(),
            Rect {
                left,
                width: NUMBER_WIDTH,
                ..bounds
            },
            &fonts.number,
            &colors.secondary,
            NSTextAlignment::Left,
        );
    }
    // Rows past the ninth have no number but keep the column, so the titles stay in line.
    let left = left + NUMBER_WIDTH;
    draw_text(
        &row.title,
        Rect {
            left,
            width: (right - left).max(0.0),
            ..bounds
        },
        &fonts.title,
        &colors.label,
        NSTextAlignment::Left,
    );
}

fn draw_note(text: &str, bounds: Rect, indent: f64, fonts: &Fonts, colors: &Colors) {
    draw_text(
        text,
        Rect {
            left: bounds.left + TEXT_INSET + indent,
            width: (bounds.width - TEXT_INSET * 2.0 - indent).max(0.0),
            ..bounds
        },
        &fonts.title,
        &colors.secondary,
        NSTextAlignment::Left,
    );
}

fn draw_close_button(state: CloseButtonVisualState, button: Rect, colors: &Colors) {
    let background = match state {
        CloseButtonVisualState::Normal => None,
        CloseButtonVisualState::Hovered => Some(&colors.control_hover),
        CloseButtonVisualState::Pressed => Some(&colors.control_pressed),
    };
    if let Some(background) = background {
        // The button sits inside the row's plate, so its corners follow the plate's.
        let inset = (ROW_HEIGHT - CLOSE_SIZE) / 2.0;
        fill_rounded(button, PLATE_RADIUS - inset, background);
    }
    let glyph = 8.0;
    let left = button.left + (button.width - glyph) / 2.0;
    let top = button.top + (button.height - glyph) / 2.0;
    colors.label.setStroke();
    let path = NSBezierPath::bezierPath();
    path.setLineWidth(1.5);
    path.moveToPoint(NSPoint::new(left, top));
    path.lineToPoint(NSPoint::new(left + glyph, top + glyph));
    path.moveToPoint(NSPoint::new(left + glyph, top));
    path.lineToPoint(NSPoint::new(left, top + glyph));
    path.stroke();
}

fn draw_preview(preview: &PreviewModel, area: Rect, fonts: &Fonts, colors: &Colors) {
    fill_rounded(area, PLATE_RADIUS, &colors.well);
    if let Some(image) = &preview.image {
        if let Some(rect) = fitted(image.size(), image_area(area)) {
            let radius = PLATE_RADIUS - PREVIEW_INSET;
            NSGraphicsContext::saveGraphicsState_class();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(rect), radius, radius)
                .addClip();
            let _ = draw_image_fit(image, rect);
            NSGraphicsContext::restoreGraphicsState_class();
            ring_rounded(rect, radius, &colors.ring);
        }
    } else if let Some(message) = &preview.message {
        let paragraph = Rect {
            left: area.left + 24.0,
            width: (area.width - 48.0).max(0.0),
            ..area
        };
        draw_text(
            message,
            paragraph,
            &fonts.detail,
            &colors.secondary,
            NSTextAlignment::Center,
        );
    }
}
