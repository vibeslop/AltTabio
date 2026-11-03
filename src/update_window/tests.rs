use super::*;
use alttabio::update::{Update, Version};
use alttabio::update_status::{Stage, Status};
use windows::Win32::UI::WindowsAndMessaging::{GetDesktopWindow, GetWindowRect, GetWindowTextW};

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
