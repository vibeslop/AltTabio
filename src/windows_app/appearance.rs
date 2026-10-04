use super::App;
use crate::native_theme::resolve_current_theme;
use alttabio::overlay_window::compositor_border_color;
use alttabio::theme::ResolvedTheme;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
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
    let preference = DWMWCP_ROUND;
    let border_color = compositor_border_color(visible_borders, theme).unwrap_or(DWMWA_COLOR_NONE);
    let use_dark_mode = i32::from(theme == ResolvedTheme::Dark);
    unsafe {
        // SAFETY: hwnd is the live top-level overlay window and the preference pointer remains
        // valid for the duration of this synchronous compositor call.
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const preference).cast(),
            u32::try_from(std::mem::size_of_val(&preference)).unwrap_or(u32::MAX),
        )?;
        // SAFETY: use_dark_mode is a valid BOOL-compatible value and the pointer remains valid for
        // the duration of this synchronous compositor call.
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const use_dark_mode).cast(),
            u32::try_from(std::mem::size_of_val(&use_dark_mode)).unwrap_or(u32::MAX),
        )?;
        // SAFETY: hwnd is unchanged and border_color is a valid COLORREF sentinel accepted by DWM.
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            (&raw const border_color).cast(),
            u32::try_from(std::mem::size_of_val(&border_color)).unwrap_or(u32::MAX),
        )
    }
}
