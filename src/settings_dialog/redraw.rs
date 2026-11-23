//! Batch native text and combo updates without exposing partially updated controls.
use super::controls::DialogControls;
use windows::Win32::Foundation::{HWND, WPARAM};
use windows::Win32::Graphics::Gdi::{
    RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RedrawWindow,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetWindowLongPtrW, SendMessageW, WM_SETREDRAW, WS_VISIBLE,
};

pub(super) struct RedrawBatch {
    parent: HWND,
    windows: Vec<HWND>,
}

impl RedrawBatch {
    pub(super) fn begin(parent: HWND, controls: &DialogControls) -> Self {
        let windows: Vec<_> = std::iter::once(parent)
            .chain(controls.iter().map(|(_, window)| window))
            .filter(|window| {
                // SAFETY: all handles are borrowed from the live dialog on its owning thread.
                unsafe { GetWindowLongPtrW(*window, GWL_STYLE) }.cast_unsigned()
                    & WS_VISIBLE.0 as usize
                    != 0
            })
            .collect();
        for window in &windows {
            // SAFETY: the dialog and controls outlive this guard. WM_SETREDRAW has scalar
            // payloads and no failure result. Each native control must suppress its own redraw.
            unsafe { SendMessageW(*window, WM_SETREDRAW, Some(WPARAM(0)), None) };
        }
        Self { parent, windows }
    }
}

impl Drop for RedrawBatch {
    fn drop(&mut self) {
        for window in self.windows.iter().rev() {
            // SAFETY: this guard cannot outlive the dialog or any of its controls.
            unsafe { SendMessageW(*window, WM_SETREDRAW, Some(WPARAM(1)), None) };
        }
        // SAFETY: the tree is live and fully updated, including its paint snapshot. Queue
        // one repaint after the mutable dialog-state borrow returns to the message loop.
        if !unsafe {
            RedrawWindow(
                Some(self.parent),
                None,
                None,
                RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN,
            )
        }
        .as_bool()
        {
            eprintln!("Could not redraw settings after updating its language");
        }
    }
}
