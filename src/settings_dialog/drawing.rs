//! Small GDI primitives for the Settings controls.
use crate::native_drawing::{OwnedBrush, fill_color};
use alttabio::dialog_layout::{Point, scale};
use alttabio::settings_form::Category;
use std::ffi::c_void;
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateRoundRectRgn, DeleteObject, FillRgn, HDC, HGDIOBJ, HPEN,
};
use windows::core::{BOOL, Error, Result};

const SOLID_PEN: i32 = 0;
#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreatePen(style: i32, width: i32, color: COLORREF) -> HPEN;
    fn SelectObject(dc: HDC, object: HGDIOBJ) -> HGDIOBJ;
    fn MoveToEx(dc: HDC, x: i32, y: i32, previous: *mut c_void) -> BOOL;
    fn LineTo(dc: HDC, x: i32, y: i32) -> BOOL;
}

pub(super) fn rounded(dc: HDC, rect: RECT, radius: i32, color: COLORREF) -> Result<()> {
    let brush = OwnedBrush::new(color)?;
    // SAFETY: bounded coordinates describe a temporary, uniquely owned GDI region.
    let region = unsafe {
        CreateRoundRectRgn(
            rect.left,
            rect.top,
            rect.right + 1,
            rect.bottom + 1,
            radius * 2,
            radius * 2,
        )
    };
    if region.is_invalid() {
        return Err(Error::from_thread());
    }
    // SAFETY: dc is borrowed for this paint; region and brush remain owned until the fill ends.
    let filled = unsafe { FillRgn(dc, region, brush.0) };
    let error = (!filled.as_bool()).then(Error::from_thread);
    // SAFETY: this function uniquely owns the region, which is no longer in use.
    if !unsafe { DeleteObject(HGDIOBJ::from(region)) }.as_bool() {
        eprintln!("Could not release a settings drawing region");
    }
    error.map_or(Ok(()), Err)
}

pub(super) const fn inset(rect: RECT, amount: i32) -> RECT {
    RECT {
        left: rect.left + amount,
        top: rect.top + amount,
        right: rect.right - amount,
        bottom: rect.bottom - amount,
    }
}

pub(super) fn icon(
    dc: HDC,
    category: Category,
    x: i32,
    y: i32,
    dpi: u32,
    ink: COLORREF,
    surface: COLORREF,
) -> Result<()> {
    let point = |a: i32, b: i32| Point::new(x + scale(a * 3, dpi) / 4, y + scale(b * 3, dpi) / 4);
    let bounds = |left, top, right, bottom| {
        let origin = point(left, top);
        let end = point(right, bottom);
        RECT {
            left: origin.x,
            top: origin.y,
            right: end.x,
            bottom: end.y,
        }
    };
    let line = |points: &[(i32, i32)]| {
        polyline(
            dc,
            &points.iter().map(|&(a, b)| point(a, b)).collect::<Vec<_>>(),
            ink,
            scale(1, dpi).max(1),
        )
    };
    let ring = |rect: RECT, radius| -> Result<()> {
        rounded(dc, rect, radius, ink)?;
        rounded(
            dc,
            inset(rect, scale(1, dpi).max(1)),
            (radius - scale(1, dpi)).max(1),
            surface,
        )
    };
    match category {
        Category::General => {
            for (y, cx) in [(6, 11), (12, 15), (18, 9)] {
                line(&[(4, y), (20, y)])?;
                ring(bounds(cx - 2, y - 2, cx + 2, y + 2), scale(2, dpi))?;
            }
        }
        Category::Switching => {
            ring(bounds(8, 3, 21, 16), scale(2, dpi))?;
            rounded(dc, bounds(3, 7, 16, 20), scale(2, dpi), surface)?;
            ring(bounds(3, 7, 16, 20), scale(2, dpi))?;
        }
        Category::Appearance => {
            ring(bounds(4, 4, 20, 20), scale(6, dpi))?;
            // Horizontal chords fill the right half of the circle without a palette asset.
            for (dy, half) in [
                (0, 0),
                (1, 3),
                (2, 4),
                (3, 5),
                (4, 6),
                (5, 6),
                (6, 7),
                (7, 7),
                (8, 7),
                (9, 7),
                (10, 7),
                (11, 6),
                (12, 6),
                (13, 5),
                (14, 4),
                (15, 3),
            ] {
                line(&[(12, 4 + dy), (12 + half, 4 + dy)])?;
            }
        }
        Category::Display => {
            ring(bounds(3, 4, 21, 17), scale(2, dpi))?;
            line(&[(12, 17), (12, 21)])?;
            line(&[(8, 21), (16, 21)])?;
        }
        Category::About => {
            ring(bounds(4, 4, 20, 20), scale(6, dpi))?;
            line(&[(12, 11), (12, 17)])?;
            fill_color(dc, bounds(11, 7, 13, 9), ink)?;
        }
    }
    Ok(())
}

pub(super) fn polyline(dc: HDC, points: &[Point], color: COLORREF, width: i32) -> Result<()> {
    let Some((first, remaining)) = points.split_first() else {
        return Ok(());
    };
    let pen = unsafe {
        // SAFETY: scalar parameters describe a solid GDI pen.
        CreatePen(SOLID_PEN, width, color)
    };
    if pen == HPEN::default() {
        return Err(Error::from_thread());
    }
    let previous = unsafe {
        // SAFETY: dc is live and pen remains owned through the complete drawing operation.
        SelectObject(dc, HGDIOBJ(pen.0))
    };
    let mut success = previous != HGDIOBJ::default();
    if success {
        success = unsafe {
            // SAFETY: dc is live and the previous-point output is intentionally unused.
            MoveToEx(dc, first.x, first.y, std::ptr::null_mut()).as_bool()
        };
        for point in remaining {
            success &= unsafe {
                // SAFETY: dc remains live with the owned pen selected.
                LineTo(dc, point.x, point.y).as_bool()
            };
        }
        let restored = unsafe {
            // SAFETY: previous is the object returned by SelectObject for this dc.
            SelectObject(dc, previous)
        };
        if restored == HGDIOBJ::default() {
            eprintln!("Could not restore the settings drawing pen");
        }
    }
    let deleted = unsafe {
        // SAFETY: pen is uniquely owned and no longer selected into dc.
        DeleteObject(HGDIOBJ(pen.0))
    };
    if !deleted.as_bool() {
        eprintln!("Could not release a settings drawing pen");
    }
    if success {
        Ok(())
    } else {
        Err(Error::from_thread())
    }
}
