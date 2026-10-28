//! Capture nested native repainting while a category or scroll command owns dialog state.
use super::*;
use crate::dialog_host::ModalDialog;
use crate::settings_dialog::{DialogState, page_tests::dialog, test_support::RenderTarget};
use alttabio::settings_form::Selector;
use std::cell::Cell;
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_CLICK, CB_SETCURSEL, CB_SETITEMHEIGHT, GWL_STYLE, GetClientRect, GetWindowLongPtrW,
    GetWindowTextW, WM_SETTEXT, WM_WINDOWPOSCHANGED, WS_VISIBLE,
};
use windows::Win32::UI::WindowsAndMessaging::{WM_KILLFOCUS, WM_SETFOCUS};

const PROBE_ID: usize = 2;
struct RepaintProbe {
    hwnd: HWND,
    image: RenderTarget,
    captures: Cell<usize>,
    trigger: u32,
    capture_window: HWND,
    paint_message: u32,
}

impl RepaintProbe {
    fn install(hwnd: HWND) -> Result<Box<Self>> {
        Self::install_trigger(hwnd, WM_SETTEXT, 32)
    }

    fn install_trigger(hwnd: HWND, trigger: u32, surface: u8) -> Result<Box<Self>> {
        Self::install_capture(hwnd, trigger, hwnd, WM_PRINTCLIENT, surface)
    }

    fn install_capture(
        hwnd: HWND,
        trigger: u32,
        capture_window: HWND,
        paint_message: u32,
        surface: u8,
    ) -> Result<Box<Self>> {
        let mut rect = RECT::default();
        // SAFETY: capture_window belongs to this live test dialog and rect is writable.
        unsafe {
            GetClientRect(capture_window, &raw mut rect)?;
        }
        let probe = Box::new(Self {
            hwnd,
            image: RenderTarget::new(rect.right, rect.bottom)?,
            captures: Cell::new(0),
            trigger,
            capture_window,
            paint_message,
        });
        // Seed the capture with the previous surface, or a sentinel for background probes.
        crate::native_drawing::fill_color(
            probe.image.dc,
            rect,
            crate::native_drawing::rgb(surface, surface, surface),
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
        if message == probe.trigger {
            // SAFETY: this private DC remains live through the synchronous print. It forces
            // native repaint re-entry during the real text update or control move.
            unsafe {
                SendMessageW(
                    probe.capture_window,
                    probe.paint_message,
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
fn scrolling_keeps_the_dialog_background_themed_during_dropdown_moves() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    for dpi in [96, 144, 192] {
        host.state_mut()?.update_dpi(dpi)?;
        for theme_index in [1, 2] {
            {
                let mut state = host.state_mut()?;
                let combo = state.controls.get(Control::Selector(Selector::Theme));
                // SAFETY: indices one and two are the live selector's Light and Dark choices.
                unsafe {
                    SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(theme_index)), None);
                }
                state.apply_selected_theme();
                state.select_category(Category::Appearance)?;
            }
            for selector in [Selector::Theme, Selector::Icon] {
                for message in [WM_ERASEBKGND, WM_PRINTCLIENT] {
                    check_scrolling_background(&dialog, selector, message)?;
                }
            }
        }
    }
    dialog.destroy()?;
    Ok(())
}

fn check_scrolling_background(
    dialog: &ModalDialog<DialogState>,
    selector: Selector,
    message: u32,
) -> Result<()> {
    let host = dialog.host()?;
    let combo = host.state_mut()?.controls.get(Control::Selector(selector));
    let probe =
        RepaintProbe::install_capture(combo, WM_WINDOWPOSCHANGED, dialog.window(), message, 0)?;
    for delta in [-120_i16, 120, -120, 120] {
        let previous = probe.captures.get();
        crate::native_drawing::fill_color(
            probe.image.dc,
            RECT {
                right: probe.image.width,
                bottom: probe.image.height,
                ..Default::default()
            },
            crate::native_drawing::rgb(0, 0, 0),
        )?;
        // SAFETY: the real wheel handler moves the dropdown while owning dialog state.
        unsafe {
            SendMessageW(
                dialog.window(),
                WM_MOUSEWHEEL,
                Some(WPARAM(usize::from(delta.cast_unsigned()) << 16)),
                None,
            );
        }
        assert!(
            probe.captures.get() > previous,
            "the dropdown must move during scrolling"
        );
        let state = host.state_mut()?;
        let layout = state.layout();
        let palette = state.palette;
        for (name, x, y, expected) in [
            ("page", layout.viewport.x, 4, palette.background),
            ("sidebar", 4, 4, palette.sidebar),
            ("footer", 4, layout.footer.y + 4, palette.sidebar),
        ] {
            let offset = usize::try_from((y * probe.image.width + x) * 4).unwrap_or_default();
            assert_eq!(
                &probe.image.pixels()?[offset..offset + 3],
                &expected.0.to_le_bytes()[..3],
                "{name} background lost its theme during {selector:?} scrolling at {} DPI",
                layout.dpi,
            );
        }
    }
    drop(probe);
    Ok(())
}

#[test]
fn category_change_keeps_dropdown_fields_dark_during_native_resize() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    let (theme, icon, navigation) = {
        let mut state = host.state_mut()?;
        let theme = state.controls.get(Control::Selector(Selector::Theme));
        // SAFETY: index two is the live selector's Dark choice.
        unsafe {
            SendMessageW(theme, CB_SETCURSEL, Some(WPARAM(2)), None);
        }
        state.apply_selected_theme();
        (
            theme,
            state.controls.get(Control::Selector(Selector::Icon)),
            state
                .controls
                .get(Control::Navigation(Category::Appearance)),
        )
    };
    let theme = RepaintProbe::install_trigger(theme, CB_SETITEMHEIGHT, 45)?;
    let icon = RepaintProbe::install_trigger(icon, CB_SETITEMHEIGHT, 45)?;
    // SAFETY: the real navigation button performs its native category transition.
    unsafe {
        SendMessageW(navigation, BM_CLICK, None, None);
    }
    for probe in [&theme, &icon] {
        assert!(probe.captures.get() > 0);
        let offset = usize::try_from(((probe.image.height / 2) * probe.image.width + 4) * 4)
            .unwrap_or_default();
        assert_eq!(
            &probe.image.pixels()?[offset..offset + 3],
            &[45, 45, 45],
            "dropdown field fell back to a system-colored surface during category change"
        );
    }
    drop(theme);
    drop(icon);
    dialog.destroy()?;
    Ok(())
}

#[test]
fn dropdown_fields_keep_their_theme_during_native_focus_updates() -> Result<()> {
    let dialog = dialog()?;
    let host = dialog.host()?;
    {
        let mut state = host.state_mut()?;
        let theme = state.controls.get(Control::Selector(Selector::Theme));
        // SAFETY: index two is this test's live Dark choice.
        unsafe {
            SendMessageW(theme, CB_SETCURSEL, Some(WPARAM(2)), None);
        }
        state.apply_selected_theme();
        state.select_category(Category::Appearance)?;
    }
    for selector in [Selector::Theme, Selector::Icon] {
        let combo = host.state_mut()?.controls.get(Control::Selector(selector));
        let probe = RepaintProbe::install_trigger(combo, WM_SETFOCUS, 45)?;
        let busy = host.state_mut()?;
        // SAFETY: exercise the live control's focus processing and its nested repaint.
        unsafe {
            SendMessageW(combo, WM_SETFOCUS, None, None);
        }
        drop(busy);
        assert!(probe.captures.get() > 0);
        let offset = usize::try_from(((probe.image.height / 2) * probe.image.width + 4) * 4)
            .unwrap_or_default();
        assert_eq!(
            &probe.image.pixels()?[offset..offset + 3],
            &[45, 45, 45],
            "{selector:?} focus update painted a light system surface"
        );
        // SAFETY: balance the synthetic focus notification before disposing the hidden control.
        unsafe {
            SendMessageW(combo, WM_KILLFOCUS, None, None);
        }
    }
    dialog.destroy()?;
    Ok(())
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
