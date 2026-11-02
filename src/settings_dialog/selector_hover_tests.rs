//! The checkmark identifies the accepted choice, not the native list's hot row.
use super::*;
use crate::settings_dialog::{page_tests::dialog, test_support::RenderTarget};
use alttabio::settings_form::Category;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CB_SETCURSEL, CB_SHOWDROPDOWN, GetClientRect, SendMessageW, WM_MOUSEMOVE, WM_PRINTCLIENT,
};

#[test]
fn hovering_popup_rows_keeps_only_the_committed_choice_checked() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    host.state_mut()?.select_category(Category::Appearance)?;
    for selector in [Selector::Theme, Selector::Icon] {
        let combo = host.state_mut()?.controls.get(Control::Selector(selector));
        let original = selector.entries().len() - 1;
        // SAFETY: this live combo contains original; show its real native list and notifications.
        unsafe {
            SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(original)), None);
        }
        host.state_mut()?.refresh_paint_state();
        // SAFETY: this test owns the combo and closes its popup below.
        unsafe { SendMessageW(combo, CB_SHOWDROPDOWN, Some(WPARAM(1)), None) };
        let list = crate::settings_dialog::theme::combo_list_window(combo)?;
        let mut bounds = RECT::default();
        // SAFETY: list belongs to this live combo and bounds is writable.
        unsafe { GetClientRect(list, &raw mut bounds)? };
        let image = RenderTarget::new(bounds.right, bounds.bottom)?;
        for row in 0..selector.entries().len() {
            let y = i32::try_from(row).unwrap_or_default() * 32 + 16;
            // SAFETY: the scalar point is inside this actual list row; the private DC
            // remains live while Windows requests all owner-drawn rows for inspection.
            unsafe {
                SendMessageW(
                    list,
                    WM_MOUSEMOVE,
                    None,
                    Some(LPARAM(((y << 16) | 0x000c) as isize)),
                );
                SendMessageW(
                    list,
                    WM_PRINTCLIENT,
                    Some(WPARAM(image.dc.0 as usize)),
                    Some(LPARAM(4)),
                );
            }
            assert_eq!(
                crate::settings_dialog::controls::selected_index(combo),
                Some(row)
            );
            let state = host.state_mut()?;
            assert_checked_rows(
                &image,
                selector.entries().len(),
                original,
                state.palette.text,
            )?;
        }
        // SAFETY: close the popup opened by this test before destroying its dialog.
        unsafe { SendMessageW(combo, CB_SHOWDROPDOWN, Some(WPARAM(0)), None) };
    }
    dialog.destroy()?;
    Ok(())
}

fn assert_checked_rows(
    image: &RenderTarget,
    count: usize,
    expected: usize,
    ink: COLORREF,
) -> Result<()> {
    let pixels = image.pixels()?;
    let checked: Vec<_> = (0..count)
        .filter(|row| {
            let center = i32::try_from(*row).unwrap_or_default() * 32 + 16;
            (center - 3..=center + 3).any(|y| {
                (image.width - 16..image.width - 7).any(|x| {
                    let offset = usize::try_from((y * image.width + x) * 4).unwrap_or_default();
                    pixels[offset..offset + 3] == ink.0.to_le_bytes()[..3]
                })
            })
        })
        .collect();
    assert_eq!(checked, [expected], "hover moved or accumulated checkmarks");
    Ok(())
}
