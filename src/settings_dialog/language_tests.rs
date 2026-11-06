//! Hidden native controls exercise the real preview, commit, cancellation, and string copies.
use super::page_tests::{dialog, pump, resize_client};
use super::*;
use alttabio::dialog_layout::{BASE_DPI, scale};
use alttabio::settings_form::SELECTOR_WIDTH;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_CLICK, CB_SETCURSEL, GWL_STYLE, GetWindowLongPtrW, GetWindowTextW, SendMessageW, WS_VISIBLE,
};

fn visible_controls(state: &DialogState) -> Vec<Control> {
    state
        .controls
        .iter()
        .filter_map(|(control, window)| {
            // SAFETY: this hidden test dialog owns every queried child.
            let visible = unsafe { GetWindowLongPtrW(window, GWL_STYLE) }.cast_unsigned()
                & WS_VISIBLE.0 as usize
                != 0;
            visible.then_some(control)
        })
        .collect()
}

#[test]
fn language_preview_preserves_page_visibility_and_scroll_position() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    resize_client(&dialog, Size::new(640, 420))?;
    for category in Category::visible() {
        let mut state = host.state_mut()?;
        state.select_category(category)?;
        state.scroll_to(i32::MAX)?;
        let offset = state.pages.offset();
        let visible = visible_controls(&state);
        let combo = state.controls.get(Control::Selector(Selector::Language));
        for index in [2, 3, 1, 0] {
            // SAFETY: the hidden test combo contains all four language entries.
            unsafe { SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(index)), None) };
            state.apply_selected_language()?;
            assert_eq!(
                state.pages.offset(),
                offset,
                "{category:?} scrolled on a language change"
            );
            assert_eq!(
                visible_controls(&state),
                visible,
                "{category:?} changed visible controls"
            );
            // SAFETY: querying the fixture's own visibility does not affect the desktop.
            assert_eq!(
                unsafe { GetWindowLongPtrW(dialog.window(), GWL_STYLE) }.cast_unsigned()
                    & WS_VISIBLE.0 as usize,
                0,
                "language preview revealed the hidden dialog"
            );
        }
    }
    dialog.destroy()?;
    Ok(())
}

fn window_text(window: HWND) -> String {
    let mut buffer = [0_u16; 512];
    // SAFETY: the test owns this hidden window; the buffer is writable for the native copy.
    let count = unsafe { GetWindowTextW(window, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(count).unwrap_or_default()])
}

#[test]
fn language_preview_preserves_drafts_and_cancel_restores_the_ui_language() -> Result<()> {
    let original = alttabio::localization::current_language();
    let dialog = dialog()?;
    let window = dialog.window();
    let host = dialog.host()?;
    let checkbox = host
        .state_mut()?
        .controls
        .checkbox(SettingOption::Autostart);
    // SAFETY: only a hidden, test-owned checkbox is clicked; no autostart side effect is applied.
    unsafe {
        SendMessageW(checkbox, BM_CLICK, None, None);
    }
    for (index, expected) in [
        (2, "Настройки AltTabio"),
        (3, "AltTabio 设置"),
        (0, "AltTabio Settings"),
    ] {
        let combo = host
            .state_mut()?
            .controls
            .get(Control::Selector(Selector::Language));
        // SAFETY: this live hidden combo has all four entries and accepts a scalar index.
        unsafe {
            SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(index)), None);
        }
        host.state_mut()?.queue_selector_preview(Selector::Language);
        pump(window);
        assert_eq!(window_text(window), expected);
        assert!(is_checked(checkbox));
        assert!(host.state_mut()?.current_settings().general.autostart);
        assert_eq!(
            host.state_mut()?.settings.general.language,
            Language::System
        );
        host.state_mut()?.select_category(Category::Appearance)?;
        host.state_mut()?.select_category(Category::General)?;
    }
    host.state_mut()?.cancel();
    assert!(!host.state_mut()?.accepted);
    dialog.destroy()?;
    assert_eq!(alttabio::localization::current_language(), original);
    Ok(())
}

#[test]
fn translated_navigation_and_language_choices_fit_native_fonts() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    let image = super::test_support::RenderTarget::new(800, 800)?;
    for dpi in [BASE_DPI, 144, 192] {
        host.state_mut()?.update_dpi(dpi)?;
        resize_client(&dialog, Size::new(640, 420).scaled(dpi))?;
        for language in [
            Language::English,
            Language::Russian,
            Language::SimplifiedChinese,
        ] {
            let index = Language::ALL
                .iter()
                .position(|item| *item == language)
                .unwrap_or_default();
            let combo = host
                .state_mut()?
                .controls
                .get(Control::Selector(Selector::Language));
            // SAFETY: the combo is owned by this hidden test dialog and the index is in range.
            unsafe {
                SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(index)), None);
            }
            host.state_mut()?.apply_selected_language()?;
            let state = host.state_mut()?;
            let Some(fonts) = state.fonts.as_ref() else {
                panic!("test dialog fonts");
            };
            for category in Category::visible() {
                let size = crate::native_drawing::measure_text(
                    image.dc,
                    category.title(),
                    fonts.strong.0,
                )?;
                let rect = Control::Navigation(category).rect(&state.layout());
                assert!(
                    size.cx <= rect.width - scale(48, dpi),
                    "{language:?} {category:?} at {dpi}: {} > {}",
                    size.cx,
                    rect.width - scale(48, dpi)
                );
            }
            for entry in Selector::Language.entries() {
                let size = crate::native_drawing::measure_text(image.dc, entry, fonts.body.0)?;
                assert!(
                    size.cx <= scale(SELECTOR_WIDTH - 32, dpi),
                    "{language:?}: {entry}"
                );
            }
            for option in SettingOption::ALL {
                let available =
                    Control::Checkbox(option).rect(&state.layout()).width - scale(52, dpi);
                for (text, font) in [
                    (option.label(), fonts.body.0),
                    (option.help(), fonts.help.0),
                ] {
                    if text.is_empty() {
                        continue;
                    }
                    let size = crate::native_drawing::measure_text(image.dc, text, font)?;
                    assert!(
                        size.cx <= available,
                        "{language:?} at {dpi}: {text}, {} > {available}",
                        size.cx
                    );
                }
            }
        }
    }
    host.state_mut()?.accept();
    assert_eq!(
        host.state_mut()?.settings.general.language,
        Language::SimplifiedChinese
    );
    dialog.destroy()?;
    Ok(())
}
