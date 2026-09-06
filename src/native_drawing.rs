//! Shared GDI drawing and owned resources for native dialogs.

use windows::Win32::Foundation::{COLORREF, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    BACKGROUND_MODE, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush,
    DEFAULT_CHARSET, DeleteObject, FF_DONTCARE, FillRect, GetSysColor, GetTextExtentPoint32W,
    HBRUSH, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, SelectObject, SetBkMode, SetTextColor,
    TRANSPARENT,
};
use windows::core::{Error, Result, w};

#[link(name = "user32")]
unsafe extern "system" {
    fn DrawTextW(dc: HDC, text: *const u16, count: i32, rect: *mut RECT, format: u32) -> i32;
}

pub(crate) const DRAW_TEXT_CENTER: u32 = 0x0001;
pub(crate) const DRAW_TEXT_VCENTER: u32 = 0x0004;
pub(crate) const DRAW_TEXT_SINGLE_LINE: u32 = 0x0020;
pub(crate) const DRAW_TEXT_NO_PREFIX: u32 = 0x0800;
pub(crate) const DRAW_TEXT_END_ELLIPSIS: u32 = 0x8000;

pub(crate) fn draw_text_with_font(
    dc: HDC,
    label: &str,
    mut rect: RECT,
    color: COLORREF,
    format: u32,
    font: HFONT,
) -> Result<()> {
    let _font = SelectedFont::new(dc, font)?;
    let _style = TextStyle::new(dc, color)?;
    let text = label.encode_utf16().collect::<Vec<_>>();
    let drawn = unsafe {
        // SAFETY: text and rect remain live throughout this synchronous GDI call.
        DrawTextW(
            dc,
            text.as_ptr(),
            i32::try_from(text.len()).unwrap_or(i32::MAX),
            &raw mut rect,
            format,
        )
    };
    if drawn == 0 {
        Err(Error::from_thread())
    } else {
        Ok(())
    }
}

pub(crate) fn fill_color(dc: HDC, rect: RECT, color: COLORREF) -> Result<()> {
    let brush = OwnedBrush::new(color)?;
    let filled = unsafe {
        // SAFETY: dc is live and brush remains owned for the synchronous fill.
        FillRect(dc, &raw const rect, brush.0)
    };
    if filled == 0 {
        Err(Error::from_thread())
    } else {
        Ok(())
    }
}

pub(crate) fn frame_color(dc: HDC, rect: RECT, color: COLORREF, thickness: i32) -> Result<()> {
    let thickness = thickness.max(1);
    for edge in [
        RECT {
            right: rect.right,
            bottom: rect.top.saturating_add(thickness),
            ..rect
        },
        RECT {
            top: rect.bottom.saturating_sub(thickness),
            right: rect.right,
            ..rect
        },
        RECT {
            right: rect.left.saturating_add(thickness),
            bottom: rect.bottom,
            ..rect
        },
        RECT {
            left: rect.right.saturating_sub(thickness),
            bottom: rect.bottom,
            ..rect
        },
    ] {
        fill_color(dc, edge, color)?;
    }
    Ok(())
}

pub(crate) fn measure_text(dc: HDC, label: &str, font: HFONT) -> Result<SIZE> {
    let _font = SelectedFont::new(dc, font)?;
    let text = label.encode_utf16().collect::<Vec<_>>();
    let mut size = SIZE::default();
    let measured = unsafe {
        // SAFETY: text and size remain live for the synchronous measurement call.
        GetTextExtentPoint32W(dc, &text, &raw mut size)
    };
    if measured.as_bool() {
        Ok(size)
    } else {
        Err(Error::from_thread())
    }
}

// These guards borrow the DC and selected resources for a synchronous drawing call.
// Restoration runs on every exit path before the caller can release its font or DC.
struct SelectedFont {
    dc: HDC,
    previous: HGDIOBJ,
}

impl SelectedFont {
    fn new(dc: HDC, font: HFONT) -> Result<Self> {
        let previous = unsafe {
            // SAFETY: the caller keeps dc and font live for this synchronous paint operation.
            SelectObject(dc, HGDIOBJ::from(font))
        };
        if previous == HGDIOBJ::default() {
            Err(Error::from_thread())
        } else {
            Ok(Self { dc, previous })
        }
    }
}

impl Drop for SelectedFont {
    fn drop(&mut self) {
        let restored = unsafe {
            // SAFETY: dc is still borrowed and previous was returned by its font selection.
            SelectObject(self.dc, self.previous)
        };
        if restored == HGDIOBJ::default() {
            eprintln!("Could not restore a native dialog drawing font");
        }
    }
}

struct TextStyle {
    dc: HDC,
    previous_mode: i32,
    previous_color: Option<COLORREF>,
}

impl TextStyle {
    fn new(dc: HDC, color: COLORREF) -> Result<Self> {
        let previous_mode = unsafe {
            // SAFETY: the caller keeps dc live for this synchronous paint operation.
            SetBkMode(dc, TRANSPARENT)
        };
        if previous_mode == 0 {
            return Err(Error::from_thread());
        }
        let mut style = Self {
            dc,
            previous_mode,
            previous_color: None,
        };
        let previous_color = unsafe {
            // SAFETY: dc remains live and color is a scalar COLORREF.
            SetTextColor(dc, color)
        };
        if previous_color.0 == u32::MAX {
            return Err(Error::from_thread());
        }
        style.previous_color = Some(previous_color);
        Ok(style)
    }
}

impl Drop for TextStyle {
    fn drop(&mut self) {
        let restored_mode = unsafe {
            // SAFETY: the borrowed DC remains live; this mode came from SetBkMode above.
            SetBkMode(self.dc, BACKGROUND_MODE(self.previous_mode.cast_unsigned()))
        };
        if restored_mode == 0 {
            eprintln!("Could not restore a native dialog background mode");
        }
        if let Some(color) = self.previous_color {
            let restored_color = unsafe {
                // SAFETY: the borrowed DC remains live; this color came from SetTextColor above.
                SetTextColor(self.dc, color)
            };
            if restored_color.0 == u32::MAX {
                eprintln!("Could not restore a native dialog text color");
            }
        }
    }
}

pub(crate) struct OwnedFont(pub(crate) HFONT);

impl OwnedFont {
    pub(crate) fn new(dpi: u32, points: u32, weight: i32, underline: bool) -> Result<Self> {
        let point_height = i32::try_from((u64::from(points) * u64::from(dpi) + 36) / 72)
            .unwrap_or(i32::MAX)
            .max(1);
        let font = unsafe {
            // SAFETY: scalar values describe a standard Segoe UI font and the face name is static.
            CreateFontW(
                -point_height,
                0,
                0,
                0,
                weight,
                0,
                u32::from(u8::from(underline)),
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                u32::from(FF_DONTCARE.0),
                w!("Segoe UI"),
            )
        };
        if font == HFONT::default() {
            Err(Error::from_thread())
        } else {
            Ok(Self(font))
        }
    }
}

impl Drop for OwnedFont {
    fn drop(&mut self) {
        let deleted = unsafe {
            // SAFETY: this wrapper uniquely owns the font and no paint callback is active on drop.
            DeleteObject(HGDIOBJ::from(self.0))
        };
        if !deleted.as_bool() {
            eprintln!("Could not release a native dialog font");
        }
    }
}

pub(crate) struct OwnedBrush(pub(crate) HBRUSH);

impl OwnedBrush {
    pub(crate) fn new(color: COLORREF) -> Result<Self> {
        let brush = unsafe {
            // SAFETY: color is a scalar COLORREF and the returned brush is uniquely owned.
            CreateSolidBrush(color)
        };
        if brush == HBRUSH::default() {
            Err(Error::from_thread())
        } else {
            Ok(Self(brush))
        }
    }
}

impl Drop for OwnedBrush {
    fn drop(&mut self) {
        let deleted = unsafe {
            // SAFETY: this wrapper uniquely owns the brush and no fill is active on drop.
            DeleteObject(HGDIOBJ::from(self.0))
        };
        if !deleted.as_bool() {
            eprintln!("Could not release a native dialog brush");
        }
    }
}

pub(crate) const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF(red as u32 | (green as u32) << 8 | (blue as u32) << 16)
}

pub(crate) fn system_color(index: windows::Win32::Graphics::Gdi::SYS_COLOR_INDEX) -> COLORREF {
    let color = unsafe {
        // SAFETY: index is one of the documented system-color constants.
        GetSysColor(index)
    };
    COLORREF(color)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, FW_NORMAL, FW_SEMIBOLD, GetBkMode, GetCurrentObject,
        GetObjectW, GetTextColor, LOGFONTW, OBJ_FONT,
    };

    struct MemoryDc(HDC);

    impl Drop for MemoryDc {
        fn drop(&mut self) {
            let deleted = unsafe {
                // SAFETY: the test uniquely owns this DC and all drawing guards have dropped.
                DeleteDC(self.0)
            };
            assert!(deleted.as_bool(), "could not release test DC");
        }
    }

    #[test]
    fn text_drawing_and_measurement_restore_the_borrowed_dc() -> Result<()> {
        let dc = MemoryDc(unsafe {
            // SAFETY: None creates a private memory DC without borrowing any window.
            CreateCompatibleDC(None)
        });
        assert_ne!(dc.0, HDC::default());
        let font = OwnedFont::new(96, 9, FW_NORMAL.0.cast_signed(), false)?;
        let original = unsafe {
            // SAFETY: the test DC is live and these read-only queries retain no pointers.
            (
                GetCurrentObject(dc.0, OBJ_FONT),
                GetBkMode(dc.0),
                GetTextColor(dc.0),
            )
        };
        let rect = RECT {
            left: 0,
            top: 0,
            right: 200,
            bottom: 40,
        };
        draw_text_with_font(
            dc.0,
            "Settings",
            rect,
            rgb(90, 100, 110),
            DRAW_TEXT_SINGLE_LINE,
            font.0,
        )?;
        let size = measure_text(dc.0, "Settings", font.0)?;
        assert!(size.cx > 0 && size.cy > 0);
        let restored = unsafe {
            // SAFETY: both synchronous drawing operations ended and the DC remains live.
            (
                GetCurrentObject(dc.0, OBJ_FONT),
                GetBkMode(dc.0),
                GetTextColor(dc.0),
            )
        };
        assert_eq!(restored, original);
        Ok(())
    }

    #[test]
    fn shared_fonts_preserve_dialog_sizes_weights_and_underlining() -> Result<()> {
        for dpi in [96, 144, 192] {
            for (points, weight, underline) in [
                (9, FW_NORMAL, false),
                (9, FW_SEMIBOLD, false),
                (11, FW_NORMAL, false),
                (17, FW_SEMIBOLD, false),
                (11, FW_NORMAL, true),
            ] {
                let font = OwnedFont::new(dpi, points, weight.0.cast_signed(), underline)?;
                let mut descriptor = LOGFONTW::default();
                let bytes = i32::try_from(size_of::<LOGFONTW>()).unwrap_or(i32::MAX);
                let read = unsafe {
                    // SAFETY: font is owned and descriptor is a correctly sized writable LOGFONTW.
                    GetObjectW(
                        HGDIOBJ::from(font.0),
                        bytes,
                        Some((&raw mut descriptor).cast()),
                    )
                };
                assert_eq!(read, bytes);
                assert_eq!(
                    descriptor.lfHeight,
                    -i32::try_from((points * dpi + 36) / 72).unwrap_or(i32::MAX)
                );
                assert_eq!(descriptor.lfWeight, weight.0.cast_signed());
                assert_eq!(descriptor.lfUnderline, u8::from(underline));
                assert_eq!(descriptor.lfQuality, CLEARTYPE_QUALITY);
            }
        }
        Ok(())
    }
}
