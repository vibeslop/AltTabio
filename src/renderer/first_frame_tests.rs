use super::*;
use alttabio::switcher::SwitchTask;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIcon, CreateWindowExW, DestroyIcon, DestroyWindow, HICON, SW_SHOWNOACTIVATE, ShowWindow,
    WS_EX_TOOLWINDOW, WS_POPUP,
};

struct Fixture {
    window: HWND,
    icon: HICON,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: the fixture exclusively owns both resources, with no surviving renderer.
        unsafe {
            assert!(DestroyWindow(self.window).is_ok());
            assert!(DestroyIcon(self.icon).is_ok());
        }
    }
}

#[test]
fn presented_frame_already_contains_task_icons() -> Result<()> {
    let _apartment = crate::win32::ComApartment::initialize()?;
    let origin = std::env::var("ALTTABIO_TEST_ORIGIN").unwrap_or_else(|_| "50,50".to_owned());
    let (left, top) = origin
        .split_once(',')
        .unwrap_or_else(|| panic!("test origin must be x,y"));
    let left = left
        .parse::<i32>()
        .unwrap_or_else(|error| panic!("invalid test x: {error}"));
    let top = top
        .parse::<i32>()
        .unwrap_or_else(|error| panic!("invalid test y: {error}"));
    let and_bits = [0_u8; 128];
    let xor_bits: Vec<u8> = (0..32 * 32)
        .flat_map(|pixel| {
            if pixel % 32 < 16 {
                [0, 0, 0, 0]
            } else {
                [0, 0, 255, 255]
            }
        })
        .collect();
    // SAFETY: buffers fit a 32x32, 32-bit icon with a transparent left half and opaque red right.
    let icon = unsafe { CreateIcon(None, 32, 32, 1, 32, and_bits.as_ptr(), xor_bits.as_ptr()) }?;
    // SAFETY: STATIC is a built-in class; no custom callback or retained pointer is supplied.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("STATIC"),
            w!("Complete frame test"),
            WS_POPUP,
            left,
            top,
            640,
            240,
            None,
            None,
            None,
            None,
        )
    };
    let window = match window {
        Ok(window) => window,
        Err(error) => {
            // SAFETY: creation failed, so the icon has no fixture to release it.
            unsafe { DestroyIcon(icon) }?;
            return Err(error);
        }
    };
    let fixture = Fixture { window, icon };
    // SAFETY: the fixture owns the live window; it is shown without taking foreground focus.
    unsafe {
        let _previous = ShowWindow(window, SW_SHOWNOACTIVATE);
    }
    for theme in [ResolvedTheme::Light, ResolvedTheme::Dark] {
        for compact_list in [false, true] {
            for large_icons in [false, true] {
                let appearance = alttabio::settings::AppearanceSettings {
                    compact_list,
                    large_icons,
                    ..alttabio::settings::Settings::default().appearance
                };
                assert_complete_frame(&fixture, &appearance, theme)?;
            }
        }
    }
    Ok(())
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "fixture icon coordinates are small positive client pixel positions"
)]
fn assert_complete_frame(
    fixture: &Fixture,
    appearance: &alttabio::settings::AppearanceSettings,
    theme: ResolvedTheme,
) -> Result<()> {
    let window = fixture.window;
    let mut switcher = Switcher::default();
    switcher
        .refresh_tasks([SwitchTask::new(1, 1, "First frame", "Fixture")
            .with_icon_handle(fixture.icon.0 as isize)]);
    let options = RenderOptions::from(appearance);
    let mut renderer = Renderer::new(theme)?;
    let icons = crate::task_icon::TaskIcons::default();
    renderer.draw(
        window,
        &switcher,
        &icons,
        None,
        options,
        Some(CloseButtonVisualState::Normal),
    )?;
    // SAFETY: drawing committed this test window's first frame. DwmFlush waits for this
    // process's queued presentation before the screen-backed client DC is sampled below.
    unsafe { DwmFlush() }?;
    let layout = for_compact_list(appearance.compact_list);
    let bounds = layout.icon_bounds(0, appearance.show_numbers, appearance.large_icons);
    // SAFETY: the window is live; DPI is a scalar and the borrowed client DC is released below.
    let (scale, dc) = unsafe { (layout_scale(GetDpiForWindow(window)), GetDC(Some(window))) };
    assert!(!dc.is_invalid());
    // SAFETY: the sample is at the center of the opaque fixture icon in this live client DC.
    let color = unsafe {
        GetPixel(
            dc,
            ((bounds.left + (bounds.right - bounds.left) * 0.75) * scale) as i32,
            (f32::midpoint(bounds.top, bounds.bottom) * scale) as i32,
        )
    };
    // SAFETY: this second sample is inside the transparent half of the same fixture icon.
    let transparent = unsafe {
        GetPixel(
            dc,
            ((bounds.left + (bounds.right - bounds.left) * 0.25) * scale) as i32,
            (f32::midpoint(bounds.top, bounds.bottom) * scale) as i32,
        )
    };
    // SAFETY: this balances this thread's GetDC without destroying the window's borrowed DC.
    assert_eq!(unsafe { ReleaseDC(Some(window), dc) }, 1);
    drop(renderer);
    assert_eq!(
        color,
        COLORREF(255),
        "the first presentation must include the icon, without a later window-DC pass"
    );
    let background = if switcher.selected_task().is_some() {
        theme.palette().selected
    } else {
        theme.palette().background
    };
    assert_eq!(
        transparent,
        COLORREF(
            u32::from(background.red)
                | (u32::from(background.green) << 8)
                | (u32::from(background.blue) << 16)
        ),
        "transparent icon pixels must preserve the first frame's selection/background"
    );
    Ok(())
}
