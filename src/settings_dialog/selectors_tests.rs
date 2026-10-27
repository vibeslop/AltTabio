use super::super::test_support::RenderTarget;
use super::*;
use crate::dialog_host::ModalDialog;
use crate::native_drawing::measure_text;
use crate::settings_dialog::{register_class, theme::ThemePalette};
use crate::win32;
use alttabio::dialog_layout::{Point, Size};
use alttabio::settings::Settings;
use alttabio::settings_form::SELECTOR_ITEM_HEIGHT;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Controls::{ODA_DRAWENTIRE, ODS_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{
    CB_GETCOUNT, CB_GETITEMHEIGHT, CB_SETCURSEL, SendMessageW, WM_DRAWITEM, WM_PRINTCLIENT,
};

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
        let width = scale(alttabio::settings_form::SELECTOR_WIDTH, dpi);
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
