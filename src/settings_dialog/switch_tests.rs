//! Exercise the real toggle paint path, including both curved edges.
use super::{
    controls::is_checked, page_tests::dialog, test_support::RenderTarget, theme::ThemePalette,
};
use alttabio::dialog_layout::scale;
use alttabio::settings_form::{Control, SettingOption};
use windows::Win32::Foundation::{COLORREF, LPARAM, RECT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_SETCHECK, GetClientRect, SendMessageW, WM_PRINTCLIENT,
};
use windows::core::Result;

#[test]
fn switches_antialias_track_and_thumb_at_each_dpi() -> Result<()> {
    let _desktop = super::test_support::PrivateDesktop::new()?;
    let dialog = dialog()?;
    let host = dialog.host()?;
    for dpi in [96, 120, 144, 168, 192] {
        host.state_mut()?.update_dpi(dpi)?;
        let window = host
            .state_mut()?
            .controls
            .get(Control::Checkbox(SettingOption::Autostart));
        let mut client = RECT::default();
        // SAFETY: the test owns this live checkbox and the writable rectangle.
        unsafe { GetClientRect(window, &raw mut client)? };
        let image = RenderTarget::new(client.right, client.bottom)?;
        for dark in [false, true] {
            let palette = ThemePalette::new(dark);
            {
                let mut state = host.state_mut()?;
                state.palette = palette;
                state.refresh_paint_state();
            }
            for enabled in [true, false] {
                // SAFETY: this only changes the test-owned native checkbox.
                let _previously_enabled = unsafe { EnableWindow(window, enabled) };
                for checked in [false, true, false] {
                    // SAFETY: the checkbox and offscreen DC remain live throughout painting.
                    unsafe {
                        SendMessageW(
                            window,
                            BM_SETCHECK,
                            Some(WPARAM(usize::from(checked))),
                            None,
                        );
                        SendMessageW(
                            window,
                            WM_PRINTCLIENT,
                            Some(WPARAM(image.dc.0 as usize)),
                            Some(LPARAM(0x04)),
                        );
                    }
                    assert_eq!(is_checked(window), checked);
                    let pixels = image.pixels()?;
                    let pixel = |x: i32, y: i32| {
                        let offset = usize::try_from((y * image.width + x) * 4).unwrap_or_default();
                        let [b, g, r] = pixels[offset..offset + 3] else {
                            unreachable!()
                        };
                        COLORREF(u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16))
                    };
                    let left = client.right - scale(38, dpi);
                    let top = scale(7, dpi);
                    let ink = palette.label_text(enabled);
                    let track = if checked { ink } else { palette.control_border };
                    let thumb = if checked {
                        palette.background
                    } else {
                        palette.secondary
                    };
                    let blended = |x: i32, y: i32| {
                        let color = pixel(x, y);
                        color != palette.background && color != track && color != thumb
                    };
                    assert!(
                        (top..top + scale(3, dpi))
                            .any(|y| { (left..left + scale(10, dpi)).any(|x| blended(x, y)) }),
                        "aliased track: dpi={dpi}, dark={dark}, enabled={enabled}, checked={checked}"
                    );
                    let thumb_left = left + scale(if checked { 17 } else { 3 }, dpi);
                    assert!(
                        (top + scale(4, dpi)..top + scale(16, dpi)).any(|y| {
                            (thumb_left..thumb_left + scale(12, dpi)).any(|x| blended(x, y))
                        }),
                        "aliased thumb: dpi={dpi}, dark={dark}, enabled={enabled}, checked={checked}"
                    );
                    assert_eq!(
                        pixel(thumb_left + scale(6, dpi), top + scale(10, dpi)),
                        thumb
                    );
                    assert_eq!(pixel(left - 1, top + scale(10, dpi)), palette.background);
                    image.save_named(&format!("switch-{dpi}-{dark}-{enabled}-{checked}"))?;
                }
            }
        }
    }
    dialog.destroy()?;
    Ok(())
}
