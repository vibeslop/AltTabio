use super::page_tests::{dialog, pump};
use super::*;
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_RETURN;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_ESCAPE, VK_UP};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::Win32::UI::WindowsAndMessaging::WM_GETICON;
use windows::Win32::UI::WindowsAndMessaging::{CB_GETCURSEL, CB_SETCURSEL, WM_KEYDOWN};
use windows::Win32::UI::WindowsAndMessaging::{
    CB_GETDROPPEDSTATE, CB_SHOWDROPDOWN, SendMessageW, WM_SETFOCUS,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_LBUTTONDOWN, WM_LBUTTONUP};

// Exercise capture routing instead of sending the press straight to the target.
fn captured_click(window: HWND, combo: HWND, target: HWND) -> Result<()> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};

    // SAFETY: open this test's real native popup.
    unsafe { SendMessageW(combo, CB_SHOWDROPDOWN, Some(WPARAM(1)), None) };
    let list = theme::combo_list_window(combo)?;
    let mut point = POINT { x: 8, y: 8 };
    let mut rect = RECT::default();
    // SAFETY: convert a point on the target to the capturing list's client space.
    unsafe {
        ClientToScreen(target, &raw mut point).ok()?;
        ScreenToClient(list, &raw mut point).ok()?;
        GetClientRect(list, &raw mut rect)?;
    }
    assert!(
        point.x < rect.left
            || point.x >= rect.right
            || point.y < rect.top
            || point.y >= rect.bottom,
        "the target must be outside the popup, not a covered control"
    );
    let packed = LPARAM(isize::try_from((point.x & 0xffff) | (point.y << 16)).unwrap_or_default());
    // SAFETY: the capturing list receives the press; after dismissal the release
    // reaches the target. Queue it first to support native combo tracking loops.
    unsafe {
        PostMessageW(Some(target), WM_LBUTTONUP, WPARAM(0), LPARAM((8 << 16) | 8))?;
        SendMessageW(list, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(packed));
    }
    pump(window);
    // SAFETY: the original list must have released capture and closed.
    assert_eq!(
        unsafe { SendMessageW(combo, CB_GETDROPPEDSTATE, None, None) }.0,
        0
    );
    Ok(())
}

#[test]
fn captured_outside_click_activates_the_settings_control() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    host.state_mut()?.select_category(Category::Appearance)?;
    for selector in [Selector::Theme, Selector::Icon] {
        let (combo, checkbox) = {
            let state = host.state_mut()?;
            (
                state.controls.get(Control::Selector(selector)),
                state.controls.checkbox(SettingOption::ShowNumbers),
            )
        };
        let original = is_checked(checkbox);
        captured_click(dialog.window(), combo, checkbox)?;
        assert_ne!(
            is_checked(checkbox),
            original,
            "{selector:?} swallowed the click"
        );
    }
    dialog.destroy()?;
    Ok(())
}

#[test]
fn captured_outside_click_selects_a_category() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    for selector in [Selector::Theme, Selector::Icon] {
        host.state_mut()?.select_category(Category::Appearance)?;
        let (combo, navigation) = {
            let state = host.state_mut()?;
            (
                state.controls.get(Control::Selector(selector)),
                state.controls.get(Control::Navigation(Category::General)),
            )
        };
        captured_click(dialog.window(), combo, navigation)?;
        assert_eq!(host.state_mut()?.pages.category, Category::General);
    }
    dialog.destroy()?;
    Ok(())
}

#[test]
fn captured_outside_click_opens_the_other_dropdown() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    host.state_mut()?.select_category(Category::Appearance)?;
    let (icon, theme) = {
        let state = host.state_mut()?;
        (
            state.controls.get(Control::Selector(Selector::Icon)),
            state.controls.get(Control::Selector(Selector::Theme)),
        )
    };
    captured_click(dialog.window(), icon, theme)?;
    // SAFETY: the same click must leave the second popup open, then close it for cleanup.
    unsafe {
        assert_ne!(SendMessageW(theme, CB_GETDROPPEDSTATE, None, None).0, 0);
        SendMessageW(theme, CB_SHOWDROPDOWN, Some(WPARAM(0)), None);
    }
    dialog.destroy()?;
    Ok(())
}

#[test]
fn captured_click_on_the_open_combo_only_closes_it() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    host.state_mut()?.select_category(Category::Appearance)?;
    for selector in [Selector::Theme, Selector::Icon] {
        let combo = host.state_mut()?.controls.get(Control::Selector(selector));
        captured_click(dialog.window(), combo, combo)?;
    }
    dialog.destroy()?;
    Ok(())
}

#[test]
fn captured_click_on_disabled_save_only_closes_the_dropdown() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    host.state_mut()?.select_category(Category::Appearance)?;
    let (combo, save) = {
        let state = host.state_mut()?;
        (
            state.controls.get(Control::Selector(Selector::Theme)),
            state.controls.get(Control::Button(DialogButton::Ok)),
        )
    };
    assert!(!controls::is_control_enabled(save));
    captured_click(dialog.window(), combo, save)?;
    assert!(!host.state_mut()?.accepted);
    dialog.destroy()?;
    Ok(())
}

#[test]
fn captured_outside_click_activates_cancel() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    host.state_mut()?.select_category(Category::Appearance)?;
    let (combo, cancel) = {
        let state = host.state_mut()?;
        (
            state.controls.get(Control::Selector(Selector::Theme)),
            state.controls.get(Control::Button(DialogButton::Cancel)),
        )
    };
    captured_click(dialog.window(), combo, cancel)?;
    // SAFETY: Cancel must close the actual native dialog from this one click.
    assert!(
        !unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(dialog.window())) }
            .as_bool()
    );
    dialog.destroy()?;
    Ok(())
}

#[test]
fn queued_focus_reveal_keeps_the_open_dropdown_open() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    host.state_mut()?.select_category(Category::Appearance)?;
    for selector in [Selector::Theme, Selector::Icon] {
        let combo = host.state_mut()?.controls.get(Control::Selector(selector));
        // SAFETY: reproduce the native click sequence: focus queues reveal work, then
        // the same click opens the list before that posted work reaches the dialog.
        unsafe {
            SendMessageW(combo, WM_SETFOCUS, None, None);
            SendMessageW(combo, CB_SHOWDROPDOWN, Some(WPARAM(1)), None);
        }
        // SAFETY: scalar query of this test's live combo.
        assert_ne!(
            unsafe { SendMessageW(combo, CB_GETDROPPEDSTATE, None, None) }.0,
            0
        );
        pump(dialog.window());
        // SAFETY: the popup must survive the pending focus/reveal message.
        assert_ne!(
            unsafe { SendMessageW(combo, CB_GETDROPPEDSTATE, None, None) }.0,
            0,
            "{selector:?} dropdown closed itself after opening"
        );
        // SAFETY: close the popup opened by this test.
        unsafe {
            SendMessageW(combo, CB_SHOWDROPDOWN, Some(WPARAM(0)), None);
        }
    }
    dialog.destroy()?;
    Ok(())
}

#[test]
fn clicking_from_one_dropdown_to_the_other_keeps_the_second_open() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    host.state_mut()?.select_category(Category::Appearance)?;
    let theme = host
        .state_mut()?
        .controls
        .get(Control::Selector(Selector::Theme));
    let icon = host
        .state_mut()?
        .controls
        .get(Control::Selector(Selector::Icon));
    for (first, second) in [(theme, icon), (icon, theme)] {
        for combo in [first, second] {
            let mut rect = RECT::default();
            // SAFETY: this reads only the test combo's bounded client rectangle.
            unsafe {
                GetClientRect(combo, &raw mut rect)?;
            }
            let point = LPARAM(
                isize::try_from((rect.right - 8) | ((rect.bottom / 2) << 16)).unwrap_or_default(),
            );
            // SAFETY: emulate a real click on the native dropdown arrow. Queue release
            // first so any native mouse-tracking loop can finish deterministically.
            unsafe {
                PostMessageW(Some(combo), WM_LBUTTONUP, WPARAM(0), point)?;
                SendMessageW(combo, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(point));
            }
            pump(dialog.window());
            // SAFETY: the clicked native control must retain its open popup.
            assert_ne!(
                unsafe { SendMessageW(combo, CB_GETDROPPEDSTATE, None, None) }.0,
                0
            );
        }
        // SAFETY: only the most recently clicked combo may retain its popup.
        assert_eq!(
            unsafe { SendMessageW(first, CB_GETDROPPEDSTATE, None, None) }.0,
            0
        );
        assert_ne!(
            unsafe { SendMessageW(second, CB_GETDROPPEDSTATE, None, None) }.0,
            0
        );
        // SAFETY: close the remaining test popup with its normal Escape behavior.
        unsafe {
            SendMessageW(
                second,
                WM_KEYDOWN,
                Some(WPARAM(usize::from(VK_ESCAPE.0))),
                None,
            );
        }
        pump(dialog.window());
    }
    dialog.destroy()?;
    Ok(())
}

#[test]
fn clicking_popup_rows_commits_the_theme_and_icon_choices() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    {
        let mut state = host.state_mut()?;
        let theme = state.controls.get(Control::Selector(Selector::Theme));
        controls::select_index(theme, 2)?;
        state.settings.appearance.theme = Theme::Dark;
        state.apply_selected_theme();
        state.select_category(Category::Appearance)?;
        state.sync_save_enabled();
    }
    for selector in [Selector::Theme, Selector::Icon] {
        let combo = host.state_mut()?.controls.get(Control::Selector(selector));
        // SAFETY: open only this test's real native popup.
        unsafe {
            SendMessageW(combo, CB_SHOWDROPDOWN, Some(WPARAM(1)), None);
        }
        let list = theme::combo_list_window(combo)?;
        let point = LPARAM((0x30 << 16) | 0x0a); // Center of the second 32px row.
        // SAFETY: queue the matching release before pressing so native mouse tracking
        // can consume it whether the control processes it inline or in its message loop.
        unsafe {
            PostMessageW(Some(list), WM_LBUTTONUP, WPARAM(0), point)?;
            SendMessageW(list, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(point));
        }
        pump(list);
        pump(dialog.window());
        // SAFETY: scalar native state queries on this test's live combo.
        assert_eq!(
            unsafe { SendMessageW(combo, CB_GETCURSEL, None, None) }.0,
            1
        );
        assert_eq!(
            unsafe { SendMessageW(combo, CB_GETDROPPEDSTATE, None, None) }.0,
            0
        );
        let state = host.state_mut()?;
        assert!(controls::is_control_enabled(
            state.controls.get(Control::Button(DialogButton::Ok))
        ));
        match selector {
            Selector::Theme => {
                assert_eq!(state.current_settings().appearance.theme, Theme::Light);
                assert_eq!(
                    state.palette.background,
                    theme::ThemePalette::new(false).background
                );
            }
            Selector::Icon => {
                assert_eq!(state.current_settings().appearance.icon, IconColor::Copper);
            }
        }
    }
    dialog.destroy()?;
    Ok(())
}

#[test]
fn enter_commits_a_keyboard_choice_without_extra_clicks() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    host.state_mut()?.select_category(Category::Appearance)?;
    for selector in [Selector::Theme, Selector::Icon] {
        let combo = host.state_mut()?.controls.get(Control::Selector(selector));
        // SAFETY: open and choose the second native entry using keyboard messages.
        unsafe {
            SendMessageW(combo, CB_SHOWDROPDOWN, Some(WPARAM(1)), None);
            SendMessageW(
                combo,
                WM_KEYDOWN,
                Some(WPARAM(usize::from(VK_DOWN.0))),
                None,
            );
            SendMessageW(
                combo,
                WM_KEYDOWN,
                Some(WPARAM(usize::from(VK_RETURN.0))),
                None,
            );
        }
        pump(dialog.window());
        // SAFETY: the committed entry and closed popup are queried using scalar messages.
        assert_eq!(
            unsafe { SendMessageW(combo, CB_GETCURSEL, None, None) }.0,
            1
        );
        assert_eq!(
            unsafe { SendMessageW(combo, CB_GETDROPPEDSTATE, None, None) }.0,
            0
        );
        let state = host.state_mut()?;
        assert!(controls::is_control_enabled(
            state.controls.get(Control::Button(DialogButton::Ok))
        ));
        assert!(match selector {
            Selector::Theme => state.current_settings().appearance.theme == Theme::Light,
            Selector::Icon => state.current_settings().appearance.icon == IconColor::Copper,
        });
    }
    dialog.destroy()?;
    Ok(())
}

#[test]
fn escaping_a_dropdown_restores_its_preview_and_save_state() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    {
        let mut state = host.state_mut()?;
        let theme = state.controls.get(Control::Selector(Selector::Theme));
        // SAFETY: select the inserted Dark entry for a deterministic starting palette.
        unsafe {
            SendMessageW(theme, CB_SETCURSEL, Some(WPARAM(2)), None);
        }
        state.settings.appearance.theme = Theme::Dark;
        state.apply_selected_theme();
        state.select_category(Category::Appearance)?;
        state.sync_save_enabled();
    }
    let save = host
        .state_mut()?
        .controls
        .get(Control::Button(DialogButton::Ok));
    for (selector, key) in [(Selector::Theme, VK_UP), (Selector::Icon, VK_DOWN)] {
        let combo = host.state_mut()?.controls.get(Control::Selector(selector));
        let background = host.state_mut()?.palette.background;
        // SAFETY: these scalar queries read only this test's live windows.
        let original = unsafe { SendMessageW(combo, CB_GETCURSEL, None, None) };
        let icon = unsafe { SendMessageW(dialog.window(), WM_GETICON, Some(WPARAM(0)), None) };
        // SAFETY: open the native popup, preview one entry, then cancel using Escape.
        unsafe {
            SendMessageW(combo, CB_SHOWDROPDOWN, Some(WPARAM(1)), None);
            SendMessageW(combo, WM_KEYDOWN, Some(WPARAM(usize::from(key.0))), None);
        }
        assert!(
            !super::controls::is_control_enabled(save),
            "highlighting a popup entry must not commit it yet"
        );
        // SAFETY: deliver Escape to the actual combo that owns the open popup.
        unsafe {
            SendMessageW(
                combo,
                WM_KEYDOWN,
                Some(WPARAM(usize::from(VK_ESCAPE.0))),
                None,
            );
        }
        pump(dialog.window());
        // SAFETY: native selection must be restored after cancellation.
        assert_eq!(
            unsafe { SendMessageW(combo, CB_GETCURSEL, None, None) },
            original
        );
        assert_eq!(
            host.state_mut()?.palette.background,
            background,
            "{selector:?} Escape left a different theme preview applied"
        );
        // SAFETY: read the test window's borrowed icon handle without taking ownership.
        assert_eq!(
            unsafe { SendMessageW(dialog.window(), WM_GETICON, Some(WPARAM(0)), None) },
            icon,
            "{selector:?} Escape left a different icon preview applied"
        );
        assert!(
            !super::controls::is_control_enabled(save),
            "{selector:?} Escape left Save enabled with no changed settings"
        );
    }
    dialog.destroy()?;
    Ok(())
}

#[test]
fn previewing_a_dropdown_choice_keeps_its_popup_open() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    host.state_mut()?.select_category(Category::Appearance)?;
    {
        let mut state = host.state_mut()?;
        let theme = state.controls.get(Control::Selector(Selector::Theme));
        // SAFETY: index two is the inserted Dark choice.
        unsafe {
            SendMessageW(theme, CB_SETCURSEL, Some(WPARAM(2)), None);
        }
        state.apply_selected_theme();
    }
    for (selector, key, expected) in [(Selector::Theme, VK_UP, 1), (Selector::Icon, VK_DOWN, 1)] {
        let combo = host.state_mut()?.controls.get(Control::Selector(selector));
        // SAFETY: open this hidden test's native list and navigate one entry.
        unsafe {
            SendMessageW(combo, CB_SHOWDROPDOWN, Some(WPARAM(1)), None);
            SendMessageW(combo, WM_KEYDOWN, Some(WPARAM(usize::from(key.0))), None);
        }
        // SAFETY: scalar native selection/state queries on the live test combo.
        assert_eq!(
            unsafe { SendMessageW(combo, CB_GETCURSEL, None, None) }.0,
            expected
        );
        pump(dialog.window());
        // SAFETY: changing a preview must not close the popup before a commit or Escape.
        assert_ne!(
            unsafe { SendMessageW(combo, CB_GETDROPPEDSTATE, None, None) }.0,
            0,
            "{selector:?} live preview closed its popup"
        );
        // SAFETY: close the native list opened above.
        unsafe {
            SendMessageW(combo, CB_SHOWDROPDOWN, Some(WPARAM(0)), None);
        }
    }
    dialog.destroy()?;
    Ok(())
}
