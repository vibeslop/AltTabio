//! Capture native repainting while the real category command still owns dialog state.
use super::*;
use crate::settings_dialog::{page_tests::dialog, test_support::RenderTarget};
use alttabio::settings_form::Selector;
use std::cell::Cell;
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_CLICK, CB_SETCURSEL, GWL_STYLE, GetClientRect, GetWindowLongPtrW, GetWindowTextW,
    WM_SETTEXT, WS_VISIBLE,
};

const PROBE_ID: usize = 2;
struct RepaintProbe {
    hwnd: HWND,
    image: RenderTarget,
    captures: Cell<usize>,
}

impl RepaintProbe {
    fn install(hwnd: HWND) -> Result<Box<Self>> {
        let mut rect = RECT::default();
        // SAFETY: hwnd is one of this test's live heading controls and rect is writable.
        unsafe {
            GetClientRect(hwnd, &raw mut rect)?;
        }
        let probe = Box::new(Self {
            hwnd,
            image: RenderTarget::new(rect.right, rect.bottom)?,
            captures: Cell::new(0),
        });
        // The visible header already has its dark background before the category changes.
        crate::native_drawing::fill_color(
            probe.image.dc,
            rect,
            crate::native_drawing::rgb(32, 32, 32),
        )?;
        // SAFETY: the Box remains stable until this subclass has been removed by Drop.
        if !unsafe {
            SetWindowSubclass(
                hwnd,
                Some(repaint_probe),
                PROBE_ID,
                std::ptr::from_ref(probe.as_ref()) as usize,
            )
        }
        .as_bool()
        {
            return Err(Error::from_thread());
        }
        Ok(probe)
    }
}

impl Drop for RepaintProbe {
    fn drop(&mut self) {
        // SAFETY: the test removes this exact subclass while its control is still live.
        assert!(
            unsafe { RemoveWindowSubclass(self.hwnd, Some(repaint_probe), PROBE_ID) }.as_bool()
        );
    }
}

unsafe extern "system" fn repaint_probe(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the owning Box remains live until the subclass is removed.
        let probe = unsafe { &*(data as *const RepaintProbe) };
        // SAFETY: pass the original native message and payload to the installed control chain.
        let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
        if message == WM_SETTEXT {
            // SAFETY: this private DC remains live through the synchronous print. It forces
            // native repaint re-entry at the same point as SetWindowText during category change.
            unsafe {
                SendMessageW(
                    hwnd,
                    WM_PRINTCLIENT,
                    Some(WPARAM(probe.image.dc.0 as usize)),
                    None,
                );
            }
            probe.captures.set(probe.captures.get() + 1);
        }
        result
    }))
    .unwrap_or(LRESULT(0))
}

#[test]
fn category_change_keeps_dark_headers_during_synchronous_native_repaint() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    let (heading, intro) = {
        let mut state = host.state_mut()?;
        let theme = state.controls.get(Control::Selector(Selector::Theme));
        // SAFETY: index two is the inserted Dark choice in this test's native selector.
        unsafe {
            SendMessageW(theme, CB_SETCURSEL, Some(WPARAM(2)), None);
        }
        state.apply_selected_theme();
        (
            state.controls.get(Control::PageHeading),
            state.controls.get(Control::PageIntro),
        )
    };
    let heading = RepaintProbe::install(heading)?;
    let intro = RepaintProbe::install(intro)?;
    for category in Category::ALL.into_iter().cycle().take(15) {
        let navigation = host
            .state_mut()?
            .controls
            .get(Control::Navigation(category));
        // SAFETY: use the real native button notification and production category handler.
        unsafe {
            SendMessageW(navigation, BM_CLICK, None, None);
        }
        assert_eq!(host.state_mut()?.pages.category, category);
        for (control, hwnd) in host.state_mut()?.controls.iter() {
            if let Some(owner) = control.category() {
                // SAFETY: query this live child window's own visibility style only.
                let visible = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) }.cast_unsigned()
                    & WS_VISIBLE.0 as usize
                    != 0;
                assert_eq!(
                    visible,
                    owner == category,
                    "stale category content: {control:?}"
                );
            }
        }
        for probe in [&heading, &intro] {
            assert_eq!(
                &probe.image.pixels()?[..3],
                &[32, 32, 32],
                "category switching painted a system-colored heading background"
            );
            // SAFETY: the category handler has now released state. Complete the normal
            // custom repaint into the same private DC to verify the final header as well.
            unsafe {
                SendMessageW(
                    probe.hwnd,
                    WM_PRINTCLIENT,
                    Some(WPARAM(probe.image.dc.0 as usize)),
                    None,
                );
            }
            assert_eq!(&probe.image.pixels()?[..3], &[32, 32, 32]);
        }
        assert!(
            heading
                .image
                .pixels()?
                .chunks_exact(4)
                .any(|pixel| pixel[..3] != [32, 32, 32]),
            "the completed heading must draw its label"
        );
        let mut title = [0_u16; 64];
        // SAFETY: the live heading writes only into the provided bounded buffer.
        let written = unsafe { GetWindowTextW(heading.hwnd, &mut title) };
        assert_eq!(
            String::from_utf16_lossy(&title[..usize::try_from(written).unwrap_or_default()]),
            category.title()
        );
    }
    assert!(heading.captures.get() > 0 && intro.captures.get() > 0);
    drop(heading);
    drop(intro);
    dialog.destroy()?;
    Ok(())
}
