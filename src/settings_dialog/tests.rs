use super::controls::is_control_enabled;
use super::test_support::RenderTarget;
use super::*;
use crate::dialog_host::DialogHost;
use alttabio::dialog_layout::{BASE_DPI, Point};
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_CLICK, CB_SETCURSEL, DispatchMessageW, GWL_STYLE, GetClientRect, GetParent,
    GetWindowLongPtrW, MSG, PM_REMOVE, PeekMessageW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
    SendMessageW, SetWindowPos, WM_MOUSEWHEEL, WM_PRINTCLIENT, WS_VISIBLE,
};

pub(super) fn dialog() -> Result<ModalDialog<DialogState>> {
    let instance = win32::module_instance()?;
    register_class(instance)?;
    let layout = SettingsLayout::for_dpi(BASE_DPI);
    let dialog = ModalDialog::create(
        instance,
        Point::default(),
        FRAME.window_size(layout.client, BASE_DPI)?,
        None,
        DialogState::new(Settings::default(), BASE_DPI, instance, None),
    )?;
    dialog
        .host()?
        .state_mut()?
        .create_controls(dialog.window(), instance)?;
    Ok(dialog)
}

pub(super) fn pump(window: HWND) {
    let mut message = MSG::default();
    // SAFETY: consume only this hidden test dialog's posted messages on its owning thread.
    while unsafe { PeekMessageW(&raw mut message, Some(window), 0, 0, PM_REMOVE) }.as_bool() {
        // SAFETY: PeekMessageW initialized the scalar message and target HWND.
        unsafe {
            DispatchMessageW(&raw const message);
        }
    }
}

pub(super) fn resize_client(dialog: &ModalDialog<DialogState>, client: Size) -> Result<()> {
    let mut outer = RECT::default();
    let mut current = RECT::default();
    // SAFETY: the test owns this hidden window and both output rectangles are writable.
    unsafe {
        GetWindowRect(dialog.window(), &raw mut outer)?;
        GetClientRect(dialog.window(), &raw mut current)?;
        SetWindowPos(
            dialog.window(),
            None,
            0,
            0,
            client.width + outer.right - outer.left - current.right,
            client.height + outer.bottom - outer.top - current.bottom,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER,
        )?;
    }
    Ok(())
}

#[test]
fn tab_moves_from_the_selected_category_into_its_settings() -> Result<()> {
    let dialog = dialog()?;
    let window = dialog.window();
    let host = dialog.host()?;
    let state = host.state_mut()?;
    let navigation = state.controls.get(Control::Navigation(Category::General));
    let first = state.controls.checkbox(SettingOption::Autostart);
    drop(state);
    // SAFETY: query native Tab traversal inside this hidden test dialog only.
    let next = unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetNextDlgTabItem(window, Some(navigation), false)?
    };
    assert_eq!(
        next, first,
        "Tab must move from the selected category into its settings"
    );
    dialog.destroy()?;
    Ok(())
}

#[test]
fn native_pages_preserve_values_forward_notifications_and_scroll_without_moving_headers()
-> Result<()> {
    let dialog = dialog()?;
    let window = dialog.window();
    let host = dialog.host()?;
    resize_client(&dialog, Size::new(640, 420))?;
    let (navigation, hover, save, viewport) = {
        let state = host.state_mut()?;
        (
            state.controls.get(Control::Navigation(Category::Switching)),
            state.controls.checkbox(SettingOption::MouseOverSelection),
            state.controls.get(Control::Button(DialogButton::Ok)),
            state.controls.get(Control::Viewport),
        )
    };
    assert!(!is_control_enabled(save));
    // SAFETY: these are hidden controls owned by this test; BM_CLICK performs their real native
    // state transition and sends their notifications through the viewport subclass.
    unsafe {
        SendMessageW(navigation, BM_CLICK, None, None);
        SendMessageW(hover, BM_CLICK, None, None);
    }
    assert_eq!(host.state_mut()?.pages.category, Category::Switching);
    assert!(!is_checked(hover));
    assert!(is_control_enabled(save));
    // SAFETY: GetParent only queries the ownership of the live checkbox.
    assert_eq!(unsafe { GetParent(hover) }?, viewport);
    host.state_mut()?.select_category(Category::Appearance)?;
    let combo = host
        .state_mut()?
        .controls
        .get(Control::Selector(Selector::Theme));
    // SAFETY: this selects the second of three inserted combo entries and sends its native
    // notification to the same viewport used by the real popup.
    unsafe {
        SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(1)), None);
        SendMessageW(
            viewport,
            WM_COMMAND,
            Some(WPARAM(0x00c8 | ((CBN_SELCHANGE as usize) << 16))),
            Some(LPARAM(combo.0 as isize)),
        );
    }
    assert_eq!(host.state_mut()?.selected_theme(), Theme::Light);
    let heading = host
        .state_mut()?
        .layout()
        .control_rect(Control::PageHeading);
    // SAFETY: wheel delta -120 is represented in the high word of a pointer-free native event.
    unsafe {
        SendMessageW(combo, WM_MOUSEWHEEL, Some(WPARAM(0xff88_0000)), None);
    }
    pump(window);
    let offset = host.state_mut()?.pages.offset();
    assert!(
        offset > 0,
        "wheel input on the closed combo must scroll the page"
    );
    assert_eq!(
        host.state_mut()?
            .layout()
            .control_rect(Control::PageHeading),
        heading
    );
    assert_about_preserves_edits(host, window, hover, save)?;
    host.state_mut()?.select_category(Category::General)?;
    host.state_mut()?.select_category(Category::Appearance)?;
    assert_eq!(host.state_mut()?.pages.offset(), offset);
    assert_eq!(host.state_mut()?.selected_theme(), Theme::Light);
    assert!(!is_checked(hover));
    for (control, hwnd) in host.state_mut()?.controls.iter() {
        if let Some(category) = control.category() {
            // SAFETY: only the child window's own visibility style is queried, allowing the
            // test to verify pages without showing the parent on the user's desktop.
            let visible = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) }.cast_unsigned()
                & WS_VISIBLE.0 as usize
                != 0;
            assert_eq!(visible, category == Category::Appearance, "{control:?}");
        }
    }
    let baseline = host.state_mut()?.settings.clone();
    host.state_mut()?.cancel();
    assert_eq!(
        host.state_mut()?.settings,
        baseline,
        "Cancel must leave the saved model untouched"
    );
    host.state_mut()?.accept();
    assert!(!host.state_mut()?.settings.general.mouse_over_selection);
    assert_eq!(host.state_mut()?.settings.appearance.theme, Theme::Light);
    dialog.destroy()?;
    Ok(())
}

fn assert_about_preserves_edits(
    host: &DialogHost<DialogState>,
    window: HWND,
    hover: HWND,
    save: HWND,
) -> Result<()> {
    navigate(window, Category::About)?;
    pump(window);
    assert_eq!(host.state_mut()?.pages.category, Category::About);
    assert_eq!(host.state_mut()?.selected_theme(), Theme::Light);
    assert!(!is_checked(hover));
    assert!(is_control_enabled(save));
    let repository = host.state_mut()?.controls.get(Control::AboutRepository);
    // SAFETY: this test clicks its own hidden native button. It must queue the external action
    // without opening a browser, closing Settings, or committing any edited values.
    unsafe { SendMessageW(repository, BM_CLICK, None, None) };
    let mut action = MSG::default();
    // SAFETY: inspect and remove only this test window's deferred browser action.
    assert!(
        unsafe {
            PeekMessageW(
                &raw mut action,
                Some(window),
                WM_SETTINGS_OPEN_REPOSITORY,
                WM_SETTINGS_OPEN_REPOSITORY,
                PM_REMOVE,
            )
        }
        .as_bool()
    );
    assert_eq!(action.hwnd, window);
    assert_eq!(host.state_mut()?.settings.appearance.theme, Theme::Auto);
    assert!(!host.state_mut()?.accepted);
    assert!(Control::AboutRepository.is_tab_stop());
    Ok(())
}

fn render_control(window: HWND) -> Result<RenderTarget> {
    let mut rect = RECT::default();
    // SAFETY: window is a live test control and rect is writable.
    unsafe {
        GetClientRect(window, &raw mut rect)?;
    }
    let image = RenderTarget::new(rect.right.max(1), rect.bottom.max(1))?;
    // SAFETY: the owned memory DC remains live for the synchronous native paint callback.
    unsafe {
        SendMessageW(
            window,
            WM_PRINTCLIENT,
            Some(WPARAM(image.dc.0 as usize)),
            None,
        );
    }
    Ok(image)
}

#[test]
fn about_header_leaves_the_logo_unobscured() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    host.state_mut()?.update_dpi(168)?;
    host.state_mut()?.select_category(Category::About)?;
    let state = host.state_mut()?;
    let intro = state.controls.get(Control::PageIntro);
    let identity = state.controls.get(Control::AboutIdentity);
    let mut intro_bounds = RECT::default();
    let mut icon_bounds = RECT::default();
    // SAFETY: the test owns both live native child windows and writable rectangles.
    unsafe {
        GetWindowRect(intro, &raw mut intro_bounds)?;
        GetWindowRect(identity, &raw mut icon_bounds)?;
    }
    icon_bounds.top += alttabio::dialog_layout::scale(3, state.dpi);
    // SAFETY: only this live child's visibility style is queried, without showing the dialog.
    let visible =
        unsafe { GetWindowLongPtrW(intro, GWL_STYLE) }.cast_unsigned() & WS_VISIBLE.0 as usize != 0;
    assert!(
        !visible || intro_bounds.bottom <= icon_bounds.top,
        "the empty About intro overlaps the top of the app icon"
    );
    drop(state);
    host.state_mut()?.select_category(Category::General)?;
    // SAFETY: the dialog still owns the intro label, which General needs to show again.
    assert_ne!(
        unsafe { GetWindowLongPtrW(intro, GWL_STYLE) }.cast_unsigned() & WS_VISIBLE.0 as usize,
        0
    );
    dialog.destroy()?;
    Ok(())
}

#[test]
fn scrollbar_is_hidden_when_the_selected_page_fits() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    for category in Category::ALL {
        host.state_mut()?.select_category(category)?;
        let state = host.state_mut()?;
        let viewport = state.controls.get(Control::Viewport);
        let overflow = state.layout().max_scroll(category) > 0;
        // SAFETY: the test owns this live native viewport; the query reads only its style.
        let style = unsafe { GetWindowLongPtrW(viewport, GWL_STYLE) }.cast_unsigned();
        assert_eq!(
            style & windows::Win32::UI::WindowsAndMessaging::WS_VSCROLL.0 as usize != 0,
            overflow,
            "{category:?}: show a scrollbar only when the page overflows"
        );
    }
    dialog.destroy()?;
    Ok(())
}

#[test]
fn github_tooltip_exposes_the_full_url_and_closes_with_settings() -> Result<()> {
    use windows::Win32::UI::Controls::{TTM_GETTOOLINFOW, TTTOOLINFOW};
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;
    let dialog = dialog()?;
    let host = dialog.host()?;
    let (tooltip, link) = {
        let state = host.state_mut()?;
        (
            state.controls.repository_tooltip(),
            state.controls.get(Control::AboutRepository),
        )
    };
    let mut text = [0_u16; 512];
    let mut tool = TTTOOLINFOW {
        cbSize: u32::try_from(std::mem::size_of::<TTTOOLINFOW>()).unwrap_or_default(),
        hwnd: dialog.window(),
        uId: link.0 as usize,
        lpszText: windows::core::PWSTR(text.as_mut_ptr()),
        ..Default::default()
    };
    // SAFETY: the live tooltip writes the sized tool structure and copies its URL into the
    // supplied buffer, which exceeds the native TOOLINFO query's 260-character requirement.
    assert_ne!(
        unsafe {
            SendMessageW(
                tooltip,
                TTM_GETTOOLINFOW,
                None,
                Some(LPARAM(std::ptr::from_mut(&mut tool) as isize)),
            )
        }
        .0,
        0
    );
    // SAFETY: the successful query returned the terminated URL in the supplied live buffer.
    assert_eq!(
        unsafe { windows::core::PCWSTR(tool.lpszText.0).to_string() }?,
        REPOSITORY_URL
    );
    dialog.destroy()?;
    // SAFETY: this query verifies that destroying the owner also destroyed its tooltip.
    assert!(!unsafe { IsWindow(Some(tooltip)) }.as_bool());
    Ok(())
}

fn render_page(host: &DialogHost<DialogState>) -> Result<RenderTarget> {
    let state = host.state_mut()?;
    let layout = state.layout();
    let image = RenderTarget::new(layout.client.width, layout.client.height)?;
    paint::paint_window_background(image.dc, &state.paint_state())?;
    let viewport = state.controls.get(Control::Viewport);
    let category = state.pages.category;
    let offset = state.pages.offset();
    let controls = state.controls.iter().collect::<Vec<_>>();
    drop(state);
    let page = render_control(viewport)?;
    for (control, window) in controls {
        if control == Control::Viewport {
            continue;
        }
        let rect = control.rect(&layout);
        if let Some(owner) = control.category() {
            if owner == category {
                render_control(window)?.copy_to(&page, rect.x, rect.y - offset)?;
            }
        } else {
            render_control(window)?.copy_to(&image, rect.x, rect.y)?;
        }
    }
    page.copy_to(&image, layout.viewport.x, layout.viewport.y)?;
    Ok(image)
}

#[test]
fn native_pages_render_in_both_themes_at_each_dpi() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    for dpi in [96, 144, 168, 192] {
        let layout = SettingsLayout::for_dpi(dpi);
        let mut outer = RECT::default();
        let mut client = RECT::default();
        // SAFETY: the hidden test window is live and both rectangles are writable. Use its
        // actual frame so the host desktop's DPI does not change the requested client size.
        unsafe {
            GetWindowRect(dialog.window(), &raw mut outer)?;
            GetClientRect(dialog.window(), &raw mut client)?;
        }
        let size = Size::new(
            layout.client.width + outer.right - outer.left - client.right,
            layout.client.height + outer.bottom - outer.top - client.bottom,
        );
        // SAFETY: only the hidden test dialog is resized, without activation or repositioning.
        unsafe {
            SetWindowPos(
                dialog.window(),
                None,
                0,
                0,
                size.width,
                size.height,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER,
            )?;
        }
        host.state_mut()?.update_dpi(dpi)?;
        for dark in [false, true] {
            host.state_mut()?.palette = ThemePalette::new(dark);
            {
                let state = host.state_mut()?;
                theme::apply_native_theme_hooks(dialog.window(), &state.controls, dark, None);
            }
            for category in Category::ALL {
                host.state_mut()?.select_category(category)?;
                host.state_mut()?.scroll_to(0)?;
                let image = render_page(host)?;
                assert!(
                    image
                        .pixels()?
                        .chunks_exact(4)
                        .any(|pixel| pixel[..3] != [0, 0, 0])
                );
                image.save_named(&format!("settings-page-{category:?}-{dark}"))?;
                if dark
                    && ((dpi == 96 && category == Category::Appearance)
                        || (dpi == 168 && category == Category::About))
                {
                    let mut frame = RECT::default();
                    // SAFETY: the hidden dialog is live and frame is writable.
                    unsafe {
                        GetWindowRect(dialog.window(), &raw mut frame)?;
                    }
                    let native_image =
                        RenderTarget::new(frame.right - frame.left, frame.bottom - frame.top)?;
                    // SAFETY: WM_PRINT uses this owned memory DC synchronously and scalar flags.
                    // PRF_CLIENT | PRF_CHILDREN | PRF_ERASEBKGND include the actual native
                    // scrolling container and its child hierarchy, without showing the window.
                    unsafe {
                        SendMessageW(
                            dialog.window(),
                            windows::Win32::UI::WindowsAndMessaging::WM_PRINT,
                            Some(WPARAM(native_image.dc.0 as usize)),
                            Some(LPARAM(0x02 | 0x04 | 0x10 | 0x08)),
                        );
                    }
                    native_image.save_named(&format!("settings-native-{category:?}-{dpi}"))?;
                }
                let bottom = host.state_mut()?.layout().max_scroll(category);
                if bottom > 0 {
                    host.state_mut()?.scroll_to(bottom)?;
                    render_page(host)?
                        .save_named(&format!("settings-page-{category:?}-bottom-{dark}"))?;
                }
            }
        }
    }
    dialog.destroy()?;
    Ok(())
}
