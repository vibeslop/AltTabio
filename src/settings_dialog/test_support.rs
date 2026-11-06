//! Offscreen GDI targets shared by the native Settings regression tests.
use std::{ffi::c_void, mem::size_of};
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, HBITMAP, HDC, HGDIOBJ, SRCCOPY, SelectObject,
};
use windows::core::{Error, Result};

use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, CreateDesktopW, DESKTOP_CONTROL_FLAGS, GetThreadDesktop, HDESK, SetThreadDesktop,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::core::PCWSTR;

/// A drawable fixture desktop that is never selected as the user's input desktop.
pub(super) struct PrivateDesktop {
    original: HDESK,
    desktop: HDESK,
}

impl PrivateDesktop {
    /// Call before creating windows; destroy those windows before dropping this guard.
    pub(super) fn new() -> Result<Self> {
        // SAFETY: GetThreadDesktop borrows the live test thread's desktop.
        let original = unsafe { GetThreadDesktop(GetCurrentThreadId()) }?;
        let name = crate::win32::wide(&format!("AltTabio-language-{}", std::process::id()));
        // SAFETY: the terminated name is live; this guard owns the created desktop.
        let desktop = unsafe {
            CreateDesktopW(
                PCWSTR(name.as_ptr()),
                None,
                None,
                DESKTOP_CONTROL_FLAGS(0),
                0x01ff,
                None,
            )
        }?;
        let guard = Self { original, desktop };
        // SAFETY: this fresh test thread has no windows or hooks. No input desktop is switched.
        unsafe { SetThreadDesktop(desktop) }?;
        Ok(guard)
    }
}

impl Drop for PrivateDesktop {
    fn drop(&mut self) {
        // SAFETY: every fixture window has been destroyed. Restore the borrowed original
        // before releasing our desktop, including when a test assertion unwinds.
        unsafe {
            assert!(SetThreadDesktop(self.original).is_ok());
            assert!(CloseDesktop(self.desktop).is_ok());
        }
    }
}

pub(super) struct RenderTarget {
    pub(super) dc: HDC,
    bitmap: HBITMAP,
    bits: *mut c_void,
    pub(super) width: i32,
    pub(super) height: i32,
}

impl RenderTarget {
    pub(super) fn new(width: i32, height: i32) -> Result<Self> {
        let mut image = Self {
            // SAFETY: a private memory DC borrows no window and is uniquely owned by image.
            dc: unsafe { CreateCompatibleDC(None) },
            bitmap: HBITMAP::default(),
            bits: std::ptr::null_mut(),
            width,
            height,
        };
        if image.dc.is_invalid() {
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
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: the sized header and bits output are live; positive dimensions bound the DIB.
        image.bitmap = unsafe {
            CreateDIBSection(
                Some(image.dc),
                &raw const info,
                DIB_RGB_COLORS,
                &raw mut image.bits,
                None,
                0,
            )
        }?;
        // SAFETY: image uniquely owns both handles; its DC is deleted before its bitmap.
        let previous = unsafe { SelectObject(image.dc, HGDIOBJ::from(image.bitmap)) };
        if previous.is_invalid() {
            return Err(Error::from_thread());
        }
        Ok(image)
    }

    pub(super) fn pixels(&self) -> Result<&[u8]> {
        // SAFETY: every drawing operation has finished and the memory DC is still owned.
        if !unsafe { GdiFlush() }.as_bool() {
            return Err(Error::from_thread());
        }
        let length = usize::try_from(self.width * self.height * 4).unwrap_or_default();
        // SAFETY: the live 32-bit top-down DIB owns this bounded, flushed pixel buffer.
        Ok(unsafe { std::slice::from_raw_parts(self.bits.cast(), length) })
    }

    pub(super) fn save_named(&self, name: &str) -> Result<()> {
        if let Some(folder) = std::env::var_os("ALTTABIO_SETTINGS_RENDER_DIR") {
            std::fs::write(
                std::path::PathBuf::from(folder)
                    .join(format!("{name}-{}x{}.bgra", self.width, self.height)),
                self.pixels()?,
            )
            .map_err(|error| Error::new(E_FAIL, error.to_string()))?;
        }
        Ok(())
    }

    pub(super) fn save_for_visual_qa(&self, dpi: u32, dark: bool) -> Result<()> {
        // Render artifacts require no visible window or desktop capture.
        if let Some(folder) = std::env::var_os("ALTTABIO_SETTINGS_RENDER_DIR") {
            let path = std::path::PathBuf::from(folder)
                .join(format!("settings-choices-{dpi}-{dark}.bgra"));
            std::fs::write(path, self.pixels()?)
                .map_err(|error| Error::new(E_FAIL, error.to_string()))?;
        }
        Ok(())
    }

    pub(super) fn copy_to(&self, target: &Self, x: i32, y: i32) -> Result<()> {
        // SAFETY: both private DCs and their selected bitmaps stay live for this bounded copy.
        unsafe {
            BitBlt(
                target.dc,
                x,
                y,
                self.width,
                self.height,
                Some(self.dc),
                0,
                0,
                SRCCOPY,
            )
        }
    }
}

impl Drop for RenderTarget {
    fn drop(&mut self) {
        // SAFETY: no drawing operation remains; deleting the DC deselects its bitmap first.
        if !self.dc.is_invalid() {
            assert!(unsafe { DeleteDC(self.dc) }.as_bool());
        }
        // SAFETY: this test owns the bitmap, and its DC has already been deleted.
        if !self.bitmap.is_invalid() {
            assert!(unsafe { DeleteObject(HGDIOBJ::from(self.bitmap)) }.as_bool());
        }
    }
}
