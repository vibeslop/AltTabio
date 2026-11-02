use super::super::test_support::RenderTarget;
use super::*;
use crate::dialog_host::ModalDialog;
use crate::native_drawing::measure_text;
use crate::settings_dialog::{DialogState, register_class, theme::ThemePalette};
use crate::win32;
use alttabio::dialog_layout::{Point, Size};
use alttabio::settings::Settings;
use alttabio::settings_form::SELECTOR_ITEM_HEIGHT;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Controls::{ODA_DRAWENTIRE, ODS_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{
    CB_GETCOUNT, CB_GETITEMHEIGHT, CB_SETCURSEL, CB_SHOWDROPDOWN, GetClientRect, GetWindowRect,
    SendMessageW, WM_DRAWITEM, WM_PRINTCLIENT,
};

fn assert_selector_metrics(
    combo: windows::Win32::Foundation::HWND,
    selector: Selector,
    dpi: u32,
) -> Result<()> {
    let mut bounds = RECT::default();
    // SAFETY: combo belongs to this live test dialog; these queries have scalar payloads.
    let (count, item_height) = unsafe {
        GetWindowRect(combo, &raw mut bounds)?;
        (
            SendMessageW(combo, CB_GETCOUNT, None, None),
            SendMessageW(combo, CB_GETITEMHEIGHT, Some(WPARAM(0)), None),
        )
    };
    assert_eq!(
        count.0,
        isize::try_from(selector.entries().len()).unwrap_or_default()
    );
    assert_eq!(item_height.0, scale(SELECTOR_ITEM_HEIGHT, dpi) as isize);
    assert_eq!(bounds.bottom - bounds.top, scale(32, dpi));
    Ok(())
}

#[test]
fn native_popup_rows_keep_their_theme_while_dialog_state_is_in_use() -> Result<()> {
    let dialog = crate::settings_dialog::page_tests::dialog()?;
    let host = dialog.host()?;
    {
        let mut state = host.state_mut()?;
        let theme = state.controls.get(Control::Selector(Selector::Theme));
        // SAFETY: index two is the live Theme selector's Dark choice.
        unsafe {
            SendMessageW(theme, CB_SETCURSEL, Some(WPARAM(2)), None);
        }
        state.apply_selected_theme();
        state.select_category(alttabio::settings_form::Category::Appearance)?;
    }
    for selector in [Selector::Theme, Selector::Icon] {
        let combo = host.state_mut()?.controls.get(Control::Selector(selector));
        let list = crate::settings_dialog::theme::combo_list_window(combo)?;
        let busy = host.state_mut()?;
        // SAFETY: open only this test's popup, exercising its actual native draw callbacks
        // while the dialog is busy, as it can be during theme and layout updates.
        unsafe {
            SendMessageW(combo, CB_SHOWDROPDOWN, Some(WPARAM(1)), None);
        }
        let mut rect = RECT::default();
        // SAFETY: the hidden native popup is live and rect is writable.
        unsafe {
            GetClientRect(list, &raw mut rect)?;
        }
        let image = RenderTarget::new(rect.right.max(1), rect.bottom.max(1))?;
        crate::native_drawing::fill_color(
            image.dc,
            rect,
            crate::native_drawing::rgb(255, 255, 255),
        )?;
        // SAFETY: this owned DC stays live while the real popup requests owner-drawn rows.
        unsafe {
            SendMessageW(
                list,
                WM_PRINTCLIENT,
                Some(WPARAM(image.dc.0 as usize)),
                Some(LPARAM(0x04)),
            );
        }
        // SAFETY: close the popup opened above before checking its captured pixels.
        unsafe {
            SendMessageW(combo, CB_SHOWDROPDOWN, Some(WPARAM(0)), None);
        }
        drop(busy);
        let visible_rows = usize::try_from(image.height / SELECTOR_ITEM_HEIGHT)
            .unwrap_or_default()
            .min(selector.entries().len());
        assert!(visible_rows > 0);
        for row in 0..visible_rows {
            let y = i32::try_from(row).unwrap_or_default() * SELECTOR_ITEM_HEIGHT + 16;
            let offset = usize::try_from((image.width * y + 4) * 4).unwrap_or_default();
            let pixel = &image.pixels()?[offset..offset + 3];
            assert!(
                pixel == [45, 45, 45] || pixel == [58, 58, 58],
                "{selector:?} popup row {row} fell back to system colors: {pixel:?}"
            );
        }
    }
    dialog.destroy()?;
    Ok(())
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
    host.state_mut()?.create_controls(window, instance)?;
    for dpi in [96, 144, 192] {
        host.state_mut()?.update_dpi(dpi)?;
        let width = scale(alttabio::settings_form::SELECTOR_WIDTH, dpi);
        let height = scale(SELECTOR_ITEM_HEIGHT, dpi);
        let image = RenderTarget::new(width * 3, height * 11)?;
        for dark in [false, true] {
            host.state_mut()?.palette = ThemePalette::new(dark);
            host.state_mut()?.refresh_paint_state();
            let mut row = 0;
            for selector in [Selector::Theme, Selector::Icon] {
                let combo = host.state_mut()?.controls.get(Control::Selector(selector));
                assert_selector_metrics(combo, selector, dpi)?;
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
