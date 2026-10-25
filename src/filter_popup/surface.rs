//! A private, top-down BGRA bitmap for the layered popup. Its DC never escapes the painter.

use std::ffi::c_void;
use std::mem::size_of;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
};
use windows::core::{Error, Result};

pub(super) struct Surface {
    pub dc: HDC,
    pub size: SIZE,
    bitmap: HBITMAP,
    #[cfg(test)]
    bits: *mut c_void,
}

impl Surface {
    pub fn new(width: i32, height: i32) -> Result<Self> {
        let mut surface = Self {
            dc: unsafe {
                // SAFETY: None creates a uniquely owned memory DC without borrowing a window.
                CreateCompatibleDC(None)
            },
            size: SIZE {
                cx: width,
                cy: height,
            },
            bitmap: HBITMAP::default(),
            #[cfg(test)]
            bits: std::ptr::null_mut(),
        };
        if surface.dc.is_invalid() {
            return Err(Error::from_thread());
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: u32::try_from(size_of::<BITMAPINFOHEADER>()).unwrap_or_default(),
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        surface.bitmap = unsafe {
            // SAFETY: the initialized bitmap info and writable bits output outlive the call;
            // positive, bounded screen dimensions describe a private top-down 32-bit bitmap.
            CreateDIBSection(
                Some(surface.dc),
                &raw const info,
                DIB_RGB_COLORS,
                &raw mut bits,
                None,
                0,
            )
        }?;
        let previous = unsafe {
            // SAFETY: this bitmap and DC are uniquely owned; the DC dies before the bitmap.
            SelectObject(surface.dc, HGDIOBJ::from(surface.bitmap))
        };
        if previous.is_invalid() {
            return Err(Error::from_thread());
        }
        #[cfg(test)]
        {
            surface.bits = bits;
        }
        Ok(surface)
    }

    #[cfg(test)]
    pub fn pixels(&self) -> &[u8] {
        let length = usize::try_from(self.size.cx * self.size.cy * 4).unwrap_or_default();
        unsafe {
            // SAFETY: the successful top-down DIB allocation contains exactly these pixels and
            // remains owned by self. Tests read only after EndDraw has flushed the DC.
            std::slice::from_raw_parts(self.bits.cast(), length)
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        if !self.dc.is_invalid() {
            let deleted = unsafe {
                // SAFETY: the private DC has no remaining drawing operation; destroying it also
                // deselects the bitmap, so the uniquely owned bitmap can be deleted next.
                DeleteDC(self.dc)
            };
            if !deleted.as_bool() {
                eprintln!("Could not release the filter popup DC");
            }
        }
        if !self.bitmap.is_invalid() {
            let deleted = unsafe {
                // SAFETY: this wrapper uniquely owns the bitmap, whose private DC was destroyed.
                DeleteObject(HGDIOBJ::from(self.bitmap))
            };
            if !deleted.as_bool() {
                eprintln!("Could not release the filter popup bitmap");
            }
        }
    }
}
