//! Drawing of Settings pages, navigation, and native controls.
use super::controls::{is_checked, is_control_enabled, selected_index};
use super::drawing::{inset, polyline, rounded};
use super::paint_state::PaintState;
use crate::native_drawing::{
    DRAW_TEXT_CENTER, DRAW_TEXT_END_ELLIPSIS, DRAW_TEXT_NO_PREFIX, DRAW_TEXT_SINGLE_LINE,
    DRAW_TEXT_VCENTER, DRAW_TEXT_WORD_BREAK, draw_text_with_font, fill_color, frame_color,
};
use alttabio::dialog_layout::{Point, hairline, scale};
use alttabio::settings_form::{Category, Control, Selector, SettingOption};
use windows::Win32::Foundation::{COLORREF, E_FAIL, HWND, LRESULT, RECT};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, DrawFocusRect, EndPaint, GetWindowDC, HDC, PAINTSTRUCT, ReleaseDC,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETSTATE, DI_NORMAL, DrawIconEx, GetClientRect, GetWindowRect, SendMessageW, WM_QUERYUISTATE,
};
use windows::core::{Error, Result};
const BUTTON_STATE_PUSHED: usize = 0x0004;

pub(super) fn paint_popup_border(window: HWND, state: &PaintState) -> Result<()> {
    let mut rect = RECT::default();
    // SAFETY: the live combo list owns this nonclient paint and rect is writable.
    unsafe { GetWindowRect(window, &raw mut rect)? };
    // SAFETY: window is the live popup; the borrowed window DC is released below.
    let dc = unsafe { GetWindowDC(Some(window)) };
    if dc.is_invalid() {
        return Err(Error::from_thread());
    }
    let result = frame_color(
        dc,
        RECT {
            left: 0,
            top: 0,
            right: rect.right - rect.left,
            bottom: rect.bottom - rect.top,
        },
        state.palette.field_border,
        hairline(state.dpi),
    );
    // SAFETY: this releases exactly the window DC acquired above, even when painting failed.
    let released = unsafe { ReleaseDC(Some(window), dc) };
    if released == 0 {
        eprintln!("Could not release the settings popup window DC");
    }
    result?;
    if released == 0 {
        return Err(Error::from_thread());
    }
    Ok(())
}
pub(super) fn paint_control_message(hwnd: HWND, state: &PaintState) -> LRESULT {
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: hwnd is live for WM_PAINT and paint is writable.
    let dc = unsafe { BeginPaint(hwnd, &raw mut paint) };
    if dc == HDC::default() {
        if state.begin_paint_failures.fail() {
            eprintln!("Could not begin painting a settings control");
        }
        return LRESULT(0);
    }
    state.begin_paint_failures.succeed();
    paint_settings_control(hwnd, dc, state);
    // SAFETY: paint belongs to the successful BeginPaint for this hwnd. EndPaint always succeeds.
    unsafe {
        let _always_nonzero = EndPaint(hwnd, &raw const paint);
    }
    LRESULT(0)
}

pub(super) fn paint_window_background(dc: HDC, state: &PaintState) -> Result<()> {
    let layout = state.layout();
    fill_color(
        dc,
        native(alttabio::dialog_layout::Rect::new(
            0,
            0,
            layout.client.width,
            layout.client.height,
        )),
        state.palette.background,
    )?;
    fill_color(dc, native(layout.sidebar), state.palette.sidebar)?;
    fill_color(dc, native(layout.footer), state.palette.sidebar)
}

fn native(rect: alttabio::dialog_layout::Rect) -> RECT {
    RECT {
        left: rect.x,
        top: rect.y,
        right: rect.right(),
        bottom: rect.bottom(),
    }
}

fn has_rule(control: Control) -> bool {
    if !matches!(control, Control::Checkbox(_) | Control::Label(_)) {
        return false;
    }
    control.category().is_some_and(|category| {
        category.controls().windows(2).any(|pair| {
            pair[1] == control && matches!(pair[0], Control::Checkbox(_) | Control::Selector(_))
        })
    })
}

fn rule(dc: HDC, rect: RECT, control: Control, state: &PaintState) -> Result<()> {
    if has_rule(control) {
        fill_color(
            dc,
            RECT {
                bottom: rect.top + hairline(state.dpi),
                ..rect
            },
            state.palette.rule,
        )?;
    }
    Ok(())
}

pub(super) fn paint_settings_control(hwnd: HWND, dc: HDC, state: &PaintState) {
    let mut client = RECT::default();
    // SAFETY: hwnd is a live child and client is writable.
    if let Err(error) = unsafe { GetClientRect(hwnd, &raw mut client) } {
        eprintln!("Could not read settings control bounds: {error}");
        return;
    }
    if let Err(error) = paint_control(hwnd, dc, client, state) {
        eprintln!("Could not paint a settings control: {error}");
    }
}

fn paint_control(hwnd: HWND, dc: HDC, client: RECT, state: &PaintState) -> Result<()> {
    let Some(control) = state.controls.find(hwnd) else {
        return Ok(());
    };
    let fonts = state
        .fonts
        .as_ref()
        .ok_or_else(|| Error::from_hresult(E_FAIL))?;
    let sidebar = matches!(
        control,
        Control::Navigation(_) | Control::SidebarTitle | Control::Button(_)
    );
    fill_color(
        dc,
        client,
        if sidebar {
            state.palette.sidebar
        } else {
            state.palette.background
        },
    )?;
    match control {
        Control::Viewport => {
            let layout = state.layout();
            for &control in state.pages.category.controls() {
                let mut rect = native(control.rect(&layout));
                rect.top -= state.pages.offset();
                rect.bottom -= state.pages.offset();
                if let Control::Label(selector) = control {
                    rect.right = Control::Selector(selector).rect(&layout).right();
                }
                rule(dc, rect, control, state)?;
            }
            Ok(())
        }
        Control::Navigation(category) => paint_navigation(hwnd, dc, client, category, state),
        Control::Checkbox(option) => paint_switch(hwnd, dc, client, option, state),
        Control::Selector(selector) => paint_combo_box(dc, client, selector, state),
        Control::Button(_) => paint_push_button(hwnd, dc, client, control, state),
        Control::AboutRepository => paint_repository_link(hwnd, dc, client, state),
        Control::AboutIdentity => paint_identity(dc, client, state, fonts),
        Control::PageHeading | Control::PageIntro => text(
            dc,
            if control == Control::PageHeading {
                state.pages.category.title()
            } else {
                state.pages.category.intro()
            },
            client,
            if control == Control::PageIntro {
                state.palette.secondary
            } else {
                state.palette.text
            },
            fonts.of(control),
        ),
        Control::Warning(_) => paint_warning(dc, client, control, state),
        Control::AboutDescription => text(
            dc,
            control.text(),
            RECT {
                bottom: client.top + scale(22, state.dpi),
                ..client
            },
            state.palette.text,
            fonts.body.0,
        ),
        Control::Section(_) => text(
            dc,
            control.text(),
            RECT {
                bottom: client.top + scale(20, state.dpi),
                ..client
            },
            state.palette.text,
            fonts.section.0,
        ),
        Control::AboutCopyright | Control::AboutLicense => text(
            dc,
            control.text(),
            client,
            state.palette.secondary,
            fonts.of(control),
        ),
        _ => {
            rule(dc, client, control, state)?;
            text(
                dc,
                control.text(),
                client,
                state.palette.text,
                fonts.of(control),
            )
        }
    }
}

fn paint_warning(dc: HDC, client: RECT, control: Control, state: &PaintState) -> Result<()> {
    let fonts = state
        .fonts
        .as_ref()
        .ok_or_else(|| Error::from_hresult(E_FAIL))?;
    draw_text_with_font(
        dc,
        control.text(),
        RECT {
            left: client.left + scale(4, state.dpi),
            right: client.right - scale(4, state.dpi),
            ..client
        },
        state.palette.secondary,
        DRAW_TEXT_NO_PREFIX | DRAW_TEXT_WORD_BREAK,
        fonts.help.0,
    )
}

fn paint_identity(
    dc: HDC,
    client: RECT,
    state: &PaintState,
    fonts: &super::DialogFonts,
) -> Result<()> {
    if let Some(previews) = state.previews.as_ref() {
        // SAFETY: the snapshot retains the owned icon, and the borrowed DC stays live.
        unsafe {
            DrawIconEx(
                dc,
                0,
                0,
                previews.logo(state.selected_icon()),
                scale(42, state.dpi),
                scale(42, state.dpi),
                0,
                None,
                DI_NORMAL,
            )?;
        }
    }
    let mut rect = client;
    rect.left += scale(55, state.dpi);
    rect.bottom = scale(22, state.dpi);
    text(dc, "AltTabio", rect, state.palette.text, fonts.identity.0)?;
    rect.top = scale(24, state.dpi);
    rect.bottom = client.bottom;
    text(
        dc,
        concat!("Version ", env!("CARGO_PKG_VERSION")),
        rect,
        state.palette.secondary,
        fonts.help.0,
    )
}

fn text(
    dc: HDC,
    label: &str,
    rect: RECT,
    color: COLORREF,
    font: windows::Win32::Graphics::Gdi::HFONT,
) -> Result<()> {
    if label.is_empty() {
        return Ok(());
    }
    draw_text_with_font(
        dc,
        label,
        rect,
        color,
        DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX | DRAW_TEXT_END_ELLIPSIS,
        font,
    )
}

fn focus(hwnd: HWND, dc: HDC, rect: RECT, state: &PaintState) -> Result<()> {
    // SAFETY: GetFocus is a query; WM_QUERYUISTATE has scalar payloads on a live child.
    let visible =
        unsafe { GetFocus() == hwnd && SendMessageW(hwnd, WM_QUERYUISTATE, None, None).0 & 1 == 0 };
    if visible {
        // SAFETY: the live paint DC and bounded client rectangle belong to this control.
        if !unsafe { DrawFocusRect(dc, &inset(rect, scale(2, state.dpi))) }.as_bool() {
            return Err(Error::from_thread());
        }
    }
    Ok(())
}

fn paint_navigation(
    hwnd: HWND,
    dc: HDC,
    client: RECT,
    category: Category,
    state: &PaintState,
) -> Result<()> {
    let selected = state.pages.category == category;
    let surface = if selected {
        state.palette.selected
    } else if state.hovered.get() == hwnd {
        state.palette.hover
    } else {
        state.palette.sidebar
    };
    rounded(dc, client, scale(4, state.dpi), surface)?;
    let ink = if selected || state.hovered.get() == hwnd {
        state.palette.text
    } else {
        state.palette.secondary
    };
    super::drawing::icon(
        dc,
        category,
        scale(12, state.dpi),
        scale(12, state.dpi),
        state.dpi,
        ink,
        surface,
    )?;
    let mut rect = client;
    rect.left += scale(40, state.dpi);
    rect.right -= scale(8, state.dpi);
    let fonts = state
        .fonts
        .as_ref()
        .ok_or_else(|| Error::from_hresult(E_FAIL))?;
    text(
        dc,
        category.title(),
        rect,
        ink,
        if selected {
            fonts.strong.0
        } else {
            fonts.body.0
        },
    )?;
    focus(hwnd, dc, client, state)
}

fn paint_switch(
    hwnd: HWND,
    dc: HDC,
    client: RECT,
    option: SettingOption,
    state: &PaintState,
) -> Result<()> {
    rule(dc, client, Control::Checkbox(option), state)?;
    let fonts = state
        .fonts
        .as_ref()
        .ok_or_else(|| Error::from_hresult(E_FAIL))?;
    let enabled = is_control_enabled(hwnd);
    let checked = is_checked(hwnd);
    let ink = state.palette.label_text(enabled);
    let mut copy = client;
    copy.left += scale(4, state.dpi);
    copy.right -= scale(48, state.dpi);
    copy.top += scale(7, state.dpi);
    copy.bottom = copy.top + scale(20, state.dpi);
    let help = option.help();
    if help.is_empty() {
        text(dc, option.label(), copy, ink, fonts.body.0)?;
    } else {
        text(dc, option.label(), copy, ink, fonts.body.0)?;
        copy.top += scale(22, state.dpi);
        copy.bottom = copy.top + scale(18, state.dpi);
        text(
            dc,
            help,
            copy,
            if enabled {
                state.palette.secondary
            } else {
                ink
            },
            fonts.help.0,
        )?;
    }
    let height = scale(20, state.dpi);
    let rect = RECT {
        left: client.right - scale(38, state.dpi),
        right: client.right - scale(4, state.dpi),
        top: client.top + scale(7, state.dpi),
        bottom: client.top + scale(7, state.dpi) + height,
    };
    rounded(
        dc,
        rect,
        scale(10, state.dpi),
        if checked {
            ink
        } else {
            state.palette.control_border
        },
    )?;
    if !checked {
        rounded(
            dc,
            inset(rect, hairline(state.dpi)),
            scale(9, state.dpi),
            state.palette.background,
        )?;
    }
    let x = rect.left + scale(if checked { 17 } else { 3 }, state.dpi);
    rounded(
        dc,
        RECT {
            left: x,
            top: rect.top + scale(4, state.dpi),
            right: x + scale(12, state.dpi),
            bottom: rect.top + scale(16, state.dpi),
        },
        scale(6, state.dpi),
        if checked {
            state.palette.background
        } else {
            state.palette.secondary
        },
    )?;
    focus(hwnd, dc, client, state)
}

fn paint_repository_link(hwnd: HWND, dc: HDC, client: RECT, state: &PaintState) -> Result<()> {
    let fonts = state
        .fonts
        .as_ref()
        .ok_or_else(|| Error::from_hresult(E_FAIL))?;
    let previews = state
        .previews
        .as_ref()
        .ok_or_else(|| Error::from_hresult(E_FAIL))?;
    let size = scale(18, state.dpi);
    // SAFETY: the snapshot retains the owned, size-matched icon through this synchronous paint.
    unsafe {
        DrawIconEx(
            dc,
            scale(4, state.dpi),
            (client.bottom - size) / 2,
            previews.github(state.palette.dark),
            size,
            size,
            0,
            None,
            DI_NORMAL,
        )?;
    }
    let mut label = client;
    label.left += scale(30, state.dpi);
    // SAFETY: GetFocus only queries the current UI thread's focus.
    let active = state.hovered.get() == hwnd || unsafe { GetFocus() == hwnd };
    text(
        dc,
        Control::AboutRepository.text(),
        label,
        state.palette.text,
        if active { fonts.link.0 } else { fonts.body.0 },
    )?;
    focus(hwnd, dc, client, state)
}

fn paint_push_button(
    hwnd: HWND,
    dc: HDC,
    client: RECT,
    control: Control,
    state: &PaintState,
) -> Result<()> {
    let enabled = is_control_enabled(hwnd);
    // SAFETY: hwnd is a live BUTTON and BM_GETSTATE has no pointer payload.
    let pressed = unsafe { SendMessageW(hwnd, BM_GETSTATE, None, None) }
        .0
        .cast_unsigned()
        & BUTTON_STATE_PUSHED
        != 0;
    let hovered = state.hovered.get() == hwnd && enabled;
    let background = if pressed {
        state.palette.pressed_surface
    } else {
        state.palette.control_surface
    };
    rounded(
        dc,
        client,
        scale(4, state.dpi),
        if hovered {
            state.palette.field_hot_border
        } else {
            state.palette.field_border
        },
    )?;
    rounded(
        dc,
        inset(client, hairline(state.dpi)),
        scale(3, state.dpi),
        background,
    )?;
    let fonts = state
        .fonts
        .as_ref()
        .ok_or_else(|| Error::from_hresult(E_FAIL))?;
    draw_text_with_font(
        dc,
        control.text(),
        client,
        state.palette.label_text(enabled),
        DRAW_TEXT_CENTER | DRAW_TEXT_VCENTER | DRAW_TEXT_SINGLE_LINE | DRAW_TEXT_NO_PREFIX,
        fonts.button.0,
    )?;
    focus(hwnd, dc, client, state)
}

fn paint_combo_box(dc: HDC, client: RECT, selector: Selector, state: &PaintState) -> Result<()> {
    rounded(
        dc,
        client,
        scale(3, state.dpi),
        if state.hovered.get() == state.controls.get(Control::Selector(selector)) {
            state.palette.field_hot_border
        } else {
            state.palette.field_border
        },
    )?;
    rounded(
        dc,
        inset(client, hairline(state.dpi)),
        scale(2, state.dpi),
        state.palette.control_surface,
    )?;
    let button_left = client.right - scale(28, state.dpi);
    if let Some(index) = selected_index(state.controls.get(Control::Selector(selector))) {
        super::selectors::paint_entry(
            dc,
            RECT {
                left: client.left + scale(2, state.dpi),
                right: button_left,
                ..client
            },
            selector,
            index,
            state.palette.text,
            state,
        )?;
    }
    let center_x = button_left + scale(14, state.dpi);
    let center_y = client.top + (client.bottom - client.top) / 2;
    polyline(
        dc,
        &[
            Point::new(
                center_x - scale(4, state.dpi),
                center_y - scale(2, state.dpi),
            ),
            Point::new(center_x, center_y + scale(2, state.dpi)),
            Point::new(
                center_x + scale(4, state.dpi),
                center_y - scale(2, state.dpi),
            ),
        ],
        state.palette.text,
        hairline(state.dpi),
    )?;
    focus(
        state.controls.get(Control::Selector(selector)),
        dc,
        client,
        state,
    )
}
