use super::*;
use alttabio::update::{Update, Version};
use alttabio::update_status::{Stage, Status};
use windows::Win32::UI::WindowsAndMessaging::{GetDesktopWindow, GetWindowRect, GetWindowTextW};

struct MeasurementDc(windows::Win32::Graphics::Gdi::HDC);
impl Drop for MeasurementDc {
    fn drop(&mut self) {
        // SAFETY: this test guard uniquely owns a memory DC, with no fonts left selected.
        assert!(unsafe { windows::Win32::Graphics::Gdi::DeleteDC(self.0) }.as_bool());
    }
}

#[test]
fn translated_update_actions_fit_without_losing_restart_wording() -> Result<()> {
    use alttabio::localization::{Language, PreviewLanguage};
    let preview = PreviewLanguage::new(Language::English);
    // SAFETY: None creates a test-owned memory DC without borrowing any native window.
    let dc = MeasurementDc(unsafe { windows::Win32::Graphics::Gdi::CreateCompatibleDC(None) });
    assert!(!dc.0.is_invalid());
    let version = Version::parse("1.3.1").unwrap_or_else(|| panic!("fixture version"));
    // SAFETY: desktop is a borrowed system window and is never destroyed by this test.
    let owner = unsafe { GetDesktopWindow() };
    let mut window = UpdateWindow::create(owner, Theme::Dark)?;
    for language in [
        Language::English,
        Language::Russian,
        Language::SimplifiedChinese,
    ] {
        preview.set(language);
        for status in [
            Status::Available(Update {
                version,
                url: "fixture".into(),
            }),
            Status::Ready(version),
            Status::Failed {
                stage: Some(Stage::Download),
                detail: "Settings".into(),
            },
            Status::LocalBuild,
        ] {
            let view = status.view("1.3.0", true, false);
            window.refresh(view.clone(), Theme::Dark)?;
            let mut detail = [0_u16; 64];
            // SAFETY: this edit control and its writable copy buffer belong to the test.
            let count = unsafe { GetWindowTextW(window.controls[8], &mut detail) };
            assert_eq!(
                String::from_utf16_lossy(&detail[..usize::try_from(count).unwrap_or_default()]),
                view.detail
            );
            for dpi in [96, 144, 192] {
                window.dpi = dpi;
                window.layout()?;
                for (index, text) in [
                    (5, view.primary.map_or("", |(label, _)| label)),
                    (6, view.close),
                ] {
                    let mut bounds = RECT::default();
                    // SAFETY: each action is an owned child of this hidden native window.
                    unsafe {
                        GetWindowRect(window.controls[index], &raw mut bounds)?;
                    }
                    let size = crate::native_drawing::measure_text(dc.0, text, window.fonts[1].0)?;
                    assert!(
                        size.cx + scale(12, dpi) <= bounds.right - bounds.left,
                        "{language:?} at {dpi}: {text}"
                    );
                }
            }
        }
    }
    Ok(())
}

#[test]
fn native_states_keep_geometry_and_expose_their_actions() -> Result<()> {
    // SAFETY: desktop is a borrowed system window; the test never destroys it.
    let owner = unsafe { GetDesktopWindow() };
    let version = Version::parse("1.3.1").unwrap_or_else(|| panic!("fixture version"));
    for theme in [Theme::Dark, Theme::Light] {
        let mut window = UpdateWindow::create(owner, theme)?;
        let mut original = RECT::default();
        // SAFETY: window owns the queried HWND and the output rectangle is writable.
        unsafe { GetWindowRect(window.dialog.window(), &raw mut original) }?;
        for status in [
            Status::Checking,
            Status::Current(std::time::SystemTime::now()),
            Status::Available(Update {
                version,
                url: "fixture".into(),
            }),
            Status::Downloading {
                received: 64,
                total: 100,
            },
            Status::Verifying,
            Status::Preparing,
            Status::Ready(version),
            Status::Restarting,
            Status::LocalBuild,
            Status::Failed {
                stage: Some(Stage::Verify),
                detail: "fixture error".repeat(100),
            },
        ] {
            let view = status.view("1.3.0", true, false);
            window.refresh(view.clone(), theme)?;
            let mut rect = RECT::default();
            // SAFETY: the window remains live for this bounds query.
            unsafe { GetWindowRect(window.dialog.window(), &raw mut rect) }?;
            assert_eq!(rect.bottom - rect.top, original.bottom - original.top);
            assert_eq!(rect.right - rect.left, original.right - original.left);
            let mut text = [0_u16; 128];
            // SAFETY: static control and writable buffer are live for this synchronous copy.
            let count = unsafe { GetWindowTextW(window.controls[0], &mut text) };
            assert_eq!(
                String::from_utf16_lossy(&text[..usize::try_from(count).unwrap_or_default()]),
                view.title
            );
            assert_eq!(
                window.dialog.host()?.state_mut()?.command,
                view.primary.map(|p| p.1)
            );
        }
        window.events.borrow_mut().details = true;
        window.take_events()?;
        let mut background_rect = RECT::default();
        let mut client_rect = RECT::default();
        // SAFETY: the native background and parent are both owned by this live test window.
        unsafe {
            GetWindowRect(window.controls[9], &raw mut background_rect)?;
            windows::Win32::UI::WindowsAndMessaging::GetClientRect(
                window.dialog.window(),
                &raw mut client_rect,
            )?;
        }
        assert_eq!(
            background_rect.right - background_rect.left,
            client_rect.right
        );
        assert_eq!(
            background_rect.bottom - background_rect.top,
            client_rect.bottom
        );
        let mut detail_rect = RECT::default();
        let mut action_rect = RECT::default();
        // SAFETY: both controls belong to the live dialog and the RECTs are writable.
        unsafe {
            GetWindowRect(window.controls[8], &raw mut detail_rect)?;
            GetWindowRect(window.controls[6], &raw mut action_rect)?;
        }
        assert!(detail_rect.bottom < action_rect.top);
        for dpi in [144, 192] {
            window.dpi = dpi;
            window.layout()?;
            // SAFETY: requery the owned controls after layout at the requested DPI.
            unsafe {
                GetWindowRect(window.controls[8], &raw mut detail_rect)?;
                GetWindowRect(window.controls[6], &raw mut action_rect)?;
            }
            assert!(detail_rect.bottom < action_rect.top);
        }
        // Simulated commands record events only; closing never records install consent.
        // SAFETY: scalar WM_COMMAND is delivered to this test-owned dialog.
        unsafe {
            SendMessageW(
                window.dialog.window(),
                WM_COMMAND,
                Some(WPARAM(CLOSE)),
                Some(LPARAM(0)),
            );
        }
        let events = window.take_events()?;
        assert!(events.close);
        assert!(events.command.is_none());
    }
    Ok(())
}
