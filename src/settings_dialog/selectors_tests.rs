use super::*;
use crate::dialog_host::ModalDialog;
use crate::native_drawing::measure_text;
use crate::settings_dialog::{register_class, theme::ThemePalette};
use crate::win32;
use alttabio::dialog_layout::{Point, Size};
use alttabio::settings::Settings;
use alttabio::settings_form::SELECTOR_ITEM_HEIGHT;
use std::ffi::c_void;
use std::mem::size_of;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, HBITMAP, HGDIOBJ, SRCCOPY, SelectObject,
};
use windows::Win32::UI::Controls::{ODA_DRAWENTIRE, ODS_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{
    CB_GETCOUNT, CB_GETITEMHEIGHT, CB_SETCURSEL, SendMessageW, WM_DRAWITEM, WM_PRINTCLIENT,
};

struct RenderTarget {
    dc: HDC,
    bitmap: HBITMAP,
    bits: *mut c_void,
    width: i32,
    height: i32,
}

impl RenderTarget {
    fn new(width: i32, height: i32) -> Result<Self> {
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

    fn pixels(&self) -> Result<&[u8]> {
        // SAFETY: every drawing operation has finished and the memory DC is still owned.
        if !unsafe { GdiFlush() }.as_bool() {
            return Err(Error::from_thread());
        }
        let length = usize::try_from(self.width * self.height * 4).unwrap_or_default();
        // SAFETY: the live 32-bit top-down DIB owns this bounded, flushed pixel buffer.
        Ok(unsafe { std::slice::from_raw_parts(self.bits.cast(), length) })
    }

    fn save_for_visual_qa(&self, dpi: u32, dark: bool) -> Result<()> {
        // Render artifacts require no visible window or desktop capture.
        if let Some(folder) = std::env::var_os("ALTTABIO_SETTINGS_RENDER_DIR") {
            let path = std::path::PathBuf::from(folder)
                .join(format!("settings-choices-{dpi}-{dark}.bgra"));
            std::fs::write(path, self.pixels()?)
                .map_err(|error| Error::new(E_FAIL, error.to_string()))?;
        }
        Ok(())
    }

    fn copy_to(&self, target: &Self, x: i32, y: i32) -> Result<()> {
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

#[test]
fn native_selectors_render_every_choice_and_keep_labels_at_each_dpi() -> Result<()> {
    let instance = win32::module_instance()?;
    register_class(instance)?;
    let dialog = ModalDialog::create(
        instance,
        Point::default(),
        Size::new(560, 747),
        None,
        DialogState::new(Settings::default(), 96, instance, None),
    )?;
    let window = dialog.window();
    let host = dialog.host()?;
    host.state_mut()?.create_controls(window, instance, host)?;
    for dpi in [96, 144, 192] {
        host.state_mut()?.update_dpi(dpi)?;
        let width = scale(180, dpi);
        let height = scale(SELECTOR_ITEM_HEIGHT, dpi);
        let image = RenderTarget::new(width * 3, height * 11)?;
        for dark in [false, true] {
            host.state_mut()?.palette = ThemePalette::new(dark);
            let mut row = 0;
            for selector in [Selector::Theme, Selector::Icon] {
                let combo = host.state_mut()?.controls.get(Control::Selector(selector));
                // SAFETY: combo is this test's live control; both queries use scalar payloads.
                let count = unsafe { SendMessageW(combo, CB_GETCOUNT, None, None) };
                let item_height =
                    unsafe { SendMessageW(combo, CB_GETITEMHEIGHT, Some(WPARAM(0)), None) };
                assert_eq!(
                    count.0,
                    isize::try_from(selector.entries().len()).unwrap_or_default()
                );
                assert_eq!(item_height.0, height as isize);
                for (index, label) in selector.entries().into_iter().enumerate() {
                    let field = RenderTarget::new(width, height)?;
                    // SAFETY: the index is one of this combo's inserted strings, and the memory
                    // DC stays alive through its synchronous custom-paint callback.
                    unsafe {
                        SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(index)), None);
                        SendMessageW(
                            combo,
                            WM_PRINTCLIENT,
                            Some(WPARAM(field.dc.0 as usize)),
                            None,
                        );
                    }
                    let state = host.state_mut()?;
                    let Some(fonts) = state.fonts.as_ref() else {
                        return Err(Error::from_hresult(E_FAIL));
                    };
                    let text = measure_text(image.dc, label, fonts.body.0)?;
                    // 32px sample, 8px padding on both sides and between sample/text, 28px arrow.
                    assert!(
                        text.cx <= width - scale(84, dpi),
                        "{label} clips at {dpi} DPI"
                    );
                    drop(state);
                    assert!(
                        field
                            .pixels()?
                            .chunks_exact(4)
                            .any(|pixel| pixel[..3] != [0, 0, 0])
                    );
                    field.copy_to(&image, width * 2, row * height)?;
                    for (column, flags) in [
                        ODS_FLAGS::default(),
                        ODS_FLAGS(ODS_SELECTED.0 | ODS_FOCUS.0),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let left = i32::try_from(column).unwrap_or_default() * width;
                        let item = DRAWITEMSTRUCT {
                            CtlType: ODT_COMBOBOX,
                            itemID: u32::try_from(index).unwrap_or_default(),
                            itemAction: ODA_DRAWENTIRE,
                            itemState: flags,
                            hwndItem: combo,
                            hDC: image.dc,
                            rcItem: RECT {
                                left,
                                top: row * height,
                                right: left + width,
                                bottom: (row + 1) * height,
                            },
                            ..Default::default()
                        };
                        // SAFETY: the native owner-draw payload and private DC stay live until
                        // the hidden parent has synchronously rendered this list row.
                        let handled = unsafe {
                            SendMessageW(
                                window,
                                WM_DRAWITEM,
                                None,
                                Some(LPARAM((&raw const item) as isize)),
                            )
                        };
                        assert_eq!(handled, LRESULT(1), "{label} was not owner-drawn");
                    }
                    row += 1;
                }
            }
            image.save_for_visual_qa(dpi, dark)?;
        }
    }
    dialog.destroy()?;
    Ok(())
}
