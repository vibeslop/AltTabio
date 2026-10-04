use super::App;
use crate::preview::DwmPreview;
use crate::win32::{monitor_info, native_rect, rect_from_native, work_area_near_cursor};
use alttabio::dialog_layout::Rect;
use alttabio::overlay_window::overlay_bounds;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromRect};
use windows::Win32::UI::WindowsAndMessaging::{SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos};
use windows::core::Result;

impl App {
    pub(super) fn handle_dpi_changed(&mut self, lparam: LPARAM) {
        let suggested = unsafe {
            // SAFETY: WM_DPICHANGED guarantees lParam points to a RECT for the callback duration.
            (lparam.0 as *const RECT).as_ref()
        };
        if let Some(suggested) = suggested {
            // The suggested rectangle only picks the monitor. It keeps the window's old logical
            // size, while AltTabio sizes the overlay from its monitor, so scaling it can make the
            // overlay fill a high-DPI screen.
            let bounds = match monitor_work_area_from_rect(*suggested) {
                Ok(work_area) => native_rect(overlay_bounds(work_area)),
                Err(error) => {
                    eprintln!("Could not resolve the monitor for the DPI change: {error}");
                    *suggested
                }
            };
            let result = unsafe {
                // SAFETY: `self.hwnd` is live and bounds came from the target monitor selected by
                // the WM_DPICHANGED rectangle.
                SetWindowPos(
                    self.hwnd,
                    None,
                    bounds.left,
                    bounds.top,
                    bounds.right - bounds.left,
                    bounds.bottom - bounds.top,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                )
            };
            if let Err(error) = result {
                eprintln!("Could not apply the DPI change: {error}");
            }
        }
        self.refresh_display_content();
    }

    pub(super) fn handle_display_changed(&mut self) {
        self.recreate_preview();
        if !self.is_visible() {
            return;
        }
        if let Err(error) = position_on_cursor_monitor(self.hwnd) {
            eprintln!("Could not reposition the overlay after the display changed: {error}");
        }
        self.sync_content_size();
        self.request_redraw();
    }

    fn refresh_display_content(&mut self) {
        self.recreate_preview();
        self.sync_content_size();
        self.request_redraw();
    }

    pub(super) fn recreate_preview(&mut self) {
        self.preview = None;
        if self.dwm_preview && self.settings.appearance.preview {
            self.preview = Some(DwmPreview::new(
                self.hwnd,
                self.settings.appearance.full_desktop_preview,
                self.settings.appearance.compact_list,
            ));
        }
    }

    pub(super) fn resize_content(&mut self, width: u32, height: u32) {
        if let Err(error) = self.renderer.resize(self.hwnd, width, height) {
            eprintln!("Could not resize the Direct2D target: {error}");
        }
        if let Some(preview) = &mut self.preview
            && let Err(error) = preview.update()
        {
            eprintln!("Could not resize the DWM preview: {error}");
        }
    }

    pub(super) fn sync_content_size(&mut self) {
        let mut client = RECT::default();
        let result = unsafe {
            // SAFETY: self.hwnd is live and client is writable for the synchronous query.
            windows::Win32::UI::WindowsAndMessaging::GetClientRect(self.hwnd, &raw mut client)
        };
        if let Err(error) = result {
            eprintln!("Could not read the overlay size: {error}");
            return;
        }
        let width = u32::try_from(client.right.saturating_sub(client.left)).unwrap_or_default();
        let height = u32::try_from(client.bottom.saturating_sub(client.top)).unwrap_or_default();
        self.resize_content(width, height);
    }
}

pub(super) fn position_on_cursor_monitor(hwnd: HWND) -> Result<()> {
    let bounds = native_rect(overlay_bounds(work_area_near_cursor()?));
    unsafe {
        // SAFETY: HWND is live; the calculated dimensions are within the selected work area.
        SetWindowPos(
            hwnd,
            None,
            bounds.left,
            bounds.top,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )?;
    }
    Ok(())
}

fn monitor_work_area_from_rect(rectangle: RECT) -> Result<Rect> {
    let monitor = unsafe {
        // SAFETY: rectangle is initialized and the fallback flag requests the nearest monitor.
        MonitorFromRect(&raw const rectangle, MONITOR_DEFAULTTONEAREST)
    };
    Ok(rect_from_native(monitor_info(monitor)?.rcWork))
}
