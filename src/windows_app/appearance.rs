use super::App;
use crate::native_theme::{current_windows_build, resolve_current_theme};
use alttabio::overlay_window::{
    DarkModeAttribute, compositor_border_color, dark_mode_attribute,
    is_unsupported_dwm_attribute_hresult, supports_window_frame_attributes,
};
use alttabio::theme::ResolvedTheme;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DWMWINDOWATTRIBUTE, DwmSetWindowAttribute,
};
use windows::core::Result;

impl App {
    pub(super) fn refresh_theme(&mut self) -> Result<bool> {
        let resolved_theme = resolve_current_theme(self.settings.appearance.theme);
        let changed = resolved_theme != self.resolved_theme;
        if changed {
            self.resolved_theme = resolved_theme;
            self.renderer.set_theme(resolved_theme);
            if let Some(tray) = self.tray.as_mut() {
                tray.set_theme(resolved_theme);
            }
        }
        apply_window_appearance(
            self.hwnd,
            self.settings.appearance.visible_borders,
            resolved_theme,
        )?;
        Ok(changed)
    }
}

pub(super) fn apply_window_appearance(
    hwnd: HWND,
    visible_borders: bool,
    theme: ResolvedTheme,
) -> Result<()> {
    apply_window_appearance_with(
        current_windows_build().ok(),
        visible_borders,
        theme,
        |attribute, value| set_u32_attribute(hwnd, attribute, value),
    )
}

fn set_u32_attribute(hwnd: HWND, attribute: DWMWINDOWATTRIBUTE, value: u32) -> Result<()> {
    unsafe {
        // SAFETY: hwnd is the live top-level overlay window and `value` lives on the stack for the
        // duration of this synchronous compositor call.
        DwmSetWindowAttribute(
            hwnd,
            attribute,
            (&raw const value).cast(),
            u32::try_from(std::mem::size_of_val(&value)).unwrap_or(u32::MAX),
        )
    }
}

fn ignore_unsupported_attribute(result: Result<()>) -> Result<()> {
    match result {
        Err(error) if is_unsupported_dwm_attribute_hresult(error.code().0) => Ok(()),
        other => other,
    }
}

fn apply_window_appearance_with(
    windows_build: Option<u32>,
    visible_borders: bool,
    theme: ResolvedTheme,
    mut set_attribute: impl FnMut(DWMWINDOWATTRIBUTE, u32) -> Result<()>,
) -> Result<()> {
    if supports_window_frame_attributes(windows_build) {
        ignore_unsupported_attribute(set_attribute(
            DWMWA_WINDOW_CORNER_PREFERENCE,
            DWMWCP_ROUND.0.cast_unsigned(),
        ))?;
    }
    if let Some(dark_attribute) = dark_mode_attribute(windows_build) {
        let use_dark_mode = u32::from(theme == ResolvedTheme::Dark);
        let result = set_attribute(
            DWMWINDOWATTRIBUTE(dark_attribute.dwm_attribute().cast_signed()),
            use_dark_mode,
        );
        match result {
            Ok(()) => {}
            Err(error)
                if dark_attribute == DarkModeAttribute::Modern
                    && is_unsupported_dwm_attribute_hresult(error.code().0) =>
            {
                ignore_unsupported_attribute(set_attribute(
                    DWMWINDOWATTRIBUTE(DarkModeAttribute::Legacy.dwm_attribute().cast_signed()),
                    use_dark_mode,
                ))?;
            }
            Err(error) => ignore_unsupported_attribute(Err(error))?,
        }
    }
    if supports_window_frame_attributes(windows_build) {
        let border_color =
            compositor_border_color(visible_borders, theme).unwrap_or(DWMWA_COLOR_NONE);
        ignore_unsupported_attribute(set_attribute(DWMWA_BORDER_COLOR, border_color))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{E_HANDLE, E_INVALIDARG};
    use windows::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE;
    use windows::core::Error;

    #[test]
    fn windows_10_20h1_updates_dark_mode_without_calling_windows_11_attributes() {
        let mut calls = Vec::new();
        let result =
            apply_window_appearance_with(Some(19_045), true, ResolvedTheme::Dark, |attr, value| {
                calls.push((attr, value));
                Ok(())
            });
        assert_eq!(result, Ok(()));
        assert_eq!(calls, vec![(DWMWA_USE_IMMERSIVE_DARK_MODE, 1)]);
    }

    #[test]
    fn windows_10_1903_uses_legacy_dark_mode_attribute_only() {
        let mut calls = Vec::new();
        let result =
            apply_window_appearance_with(Some(18_362), true, ResolvedTheme::Dark, |attr, value| {
                calls.push((attr, value));
                Ok(())
            });
        assert_eq!(result, Ok(()));
        assert_eq!(calls, vec![(DWMWINDOWATTRIBUTE(19), 1)]);
    }

    #[test]
    fn windows_11_applies_corners_dark_mode_and_border_color() {
        let mut calls = Vec::new();
        let result = apply_window_appearance_with(
            Some(22_631),
            false,
            ResolvedTheme::Light,
            |attr, value| {
                calls.push((attr, value));
                Ok(())
            },
        );
        assert_eq!(result, Ok(()));
        assert_eq!(
            calls,
            vec![
                (
                    DWMWA_WINDOW_CORNER_PREFERENCE,
                    DWMWCP_ROUND.0.cast_unsigned()
                ),
                (DWMWA_USE_IMMERSIVE_DARK_MODE, 0),
                (DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE),
            ]
        );
    }

    #[test]
    fn invalid_arg_on_unsupported_attributes_does_not_fail_theme_update_or_skip_fallback() {
        let mut calls = Vec::new();
        let result =
            apply_window_appearance_with(None, true, ResolvedTheme::Dark, |attr, value| {
                calls.push((attr, value));
                if attr == DWMWA_WINDOW_CORNER_PREFERENCE
                    || attr == DWMWA_USE_IMMERSIVE_DARK_MODE
                    || attr == DWMWA_BORDER_COLOR
                {
                    Err(Error::from_hresult(E_INVALIDARG))
                } else {
                    Ok(())
                }
            });
        assert_eq!(result, Ok(()));
        assert_eq!(
            calls,
            vec![
                (
                    DWMWA_WINDOW_CORNER_PREFERENCE,
                    DWMWCP_ROUND.0.cast_unsigned()
                ),
                (DWMWA_USE_IMMERSIVE_DARK_MODE, 1),
                (DWMWINDOWATTRIBUTE(19), 1),
                (
                    DWMWA_BORDER_COLOR,
                    compositor_border_color(true, ResolvedTheme::Dark).unwrap_or(DWMWA_COLOR_NONE)
                ),
            ]
        );
    }

    #[test]
    fn genuine_dwm_errors_such_as_invalid_handle_still_propagate() {
        let result =
            apply_window_appearance_with(Some(19_045), false, ResolvedTheme::Dark, |_, _| {
                Err(Error::from_hresult(E_HANDLE))
            });
        assert_eq!(result, Err(Error::from_hresult(E_HANDLE)));
    }
}
