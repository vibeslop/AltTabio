use super::controls::is_control_enabled;
use super::test_support::RenderTarget;
use super::*;
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
        .create_controls(dialog.window(), instance, dialog.host()?)?;
    Ok(dialog)
}

fn pump(window: HWND) {
    let mut message = MSG::default();
    // SAFETY: consume only this hidden test dialog's posted messages on its owning thread.
    while unsafe { PeekMessageW(&raw mut message, Some(window), 0, 0, PM_REMOVE) }.as_bool() {
        // SAFETY: PeekMessageW initialized the scalar message and target HWND.
        unsafe {
            DispatchMessageW(&raw const message);
        }
    }
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
    let (navigation, wheel, release, save, viewport) = {
        let state = host.state_mut()?;
        (
            state.controls.get(Control::Navigation(Category::Switching)),
            state
                .controls
                .checkbox(SettingOption::RightButtonWheelSwitching),
            state
                .controls
                .checkbox(SettingOption::ReleaseRightButtonSwitches),
            state.controls.get(Control::Button(DialogButton::Ok)),
            state.controls.get(Control::Viewport),
        )
    };
    assert!(!is_control_enabled(save));
    assert!(!is_control_enabled(release));
    // SAFETY: these are hidden controls owned by this test; BM_CLICK performs their real native
    // state transition and sends their notifications through the viewport subclass.
    unsafe {
        SendMessageW(navigation, BM_CLICK, None, None);
        SendMessageW(wheel, BM_CLICK, None, None);
    }
    assert_eq!(host.state_mut()?.pages.category, Category::Switching);
    assert!(is_checked(wheel));
    assert!(is_control_enabled(release));
    assert!(is_control_enabled(save));
    // SAFETY: GetParent only queries the ownership of the live checkbox.
    assert_eq!(unsafe { GetParent(wheel) }?, viewport);
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
    host.state_mut()?.select_category(Category::General)?;
    host.state_mut()?.select_category(Category::Appearance)?;
    assert_eq!(host.state_mut()?.pages.offset(), offset);
    assert_eq!(host.state_mut()?.selected_theme(), Theme::Light);
    assert!(is_checked(wheel));
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
    assert!(
        host.state_mut()?
            .settings
            .general
            .right_button_wheel_switching
    );
    assert_eq!(host.state_mut()?.settings.appearance.theme, Theme::Light);
    dialog.destroy()?;
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

fn render_page(host: &DialogHost<DialogState>) -> Result<RenderTarget> {
    let state = host.state_mut()?;
    let layout = state.layout();
    let image = RenderTarget::new(layout.client.width, layout.client.height)?;
    paint::paint_window_background(image.dc, &state)?;
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
    for dpi in [96, 144, 192] {
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
                if dpi == 96 && dark && category == Category::Appearance {
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
                    native_image.save_named("settings-native-hierarchy")?;
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
