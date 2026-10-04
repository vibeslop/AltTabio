//! Rebuilds modifier state after an input-desktop switch, since the hook never sees the key
//! changes made on the other desktop.

use super::keyboard_state::{MODIFIER_KEYS, timestamp_at_or_after};
use super::{WM_RECONCILE_KEYBOARD, key_pressed, process_with_context};
use alttabio::hook_flags::HookFlags;
use std::cell::{Cell, RefCell};
use std::sync::Arc;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::StationsAndDesktops::{
    GetThreadDesktop, GetUserObjectInformationW, UOI_IO,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_SYSTEM_DESKTOPSWITCH, KillTimer, MSG, PostThreadMessageW, SetTimer,
    WINEVENT_OUTOFCONTEXT, WM_TIMER,
};
use windows::core::{BOOL, Error};

thread_local! {
    // Independent publication lets reentrant desktop callbacks gate delivery during state borrows.
    static RECOVERY_FLAGS: RefCell<Option<Arc<HookFlags>>> = const { RefCell::new(None) };
    static DESKTOP_BOUNDARY: Cell<Option<u32>> = const { Cell::new(None) };
    static RECOVERY_QUEUED: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn publish_recovery_flags(flags: Option<Arc<HookFlags>>) {
    RECOVERY_FLAGS.with(|slot| *slot.borrow_mut() = flags);
}

pub(super) struct KeyboardRecoveryWatcher {
    hook: HWINEVENTHOOK,
    timer: usize,
}

impl KeyboardRecoveryWatcher {
    pub(super) fn install() -> Result<Self, String> {
        let hook = unsafe {
            // SAFETY: the out-of-context callback has a static lifetime and the required ABI.
            // This message-loop thread owns registration and unregistration.
            SetWinEventHook(
                EVENT_SYSTEM_DESKTOPSWITCH,
                EVENT_SYSTEM_DESKTOPSWITCH,
                None,
                Some(desktop_switch_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if hook.is_invalid() {
            return Err(format!(
                "Could not watch input desktop changes: {}",
                Error::from_thread()
            ));
        }
        let mut watcher = Self { hook, timer: 0 };
        watcher.timer = unsafe {
            // SAFETY: no window or callback is supplied; the timer posts to this owning thread.
            SetTimer(None, 0, 100, None)
        };
        if watcher.timer == 0 {
            return Err(format!(
                "Could not start keyboard recovery timer: {}",
                Error::from_thread()
            ));
        }
        Ok(watcher)
    }

    pub(super) fn handles(&self, message: &MSG) -> bool {
        message.message == WM_RECONCILE_KEYBOARD
            || (message.message == WM_TIMER && message.wParam.0 == self.timer)
    }
}

impl Drop for KeyboardRecoveryWatcher {
    fn drop(&mut self) {
        if self.timer != 0 {
            let result = unsafe {
                // SAFETY: this thread owns the timer and removes it exactly once.
                KillTimer(None, self.timer)
            };
            if let Err(error) = result {
                eprintln!("Could not remove keyboard recovery timer: {error}");
            }
        }
        let removed = unsafe {
            // SAFETY: this thread owns the successful registration and removes it exactly once.
            UnhookWinEvent(self.hook)
        };
        if !removed.as_bool() {
            eprintln!(
                "Could not remove input desktop watcher: {}",
                Error::from_thread()
            );
        }
    }
}

fn note_desktop_boundary(time: u32) {
    DESKTOP_BOUNDARY.with(|pending| {
        if pending
            .get()
            .is_none_or(|previous| timestamp_at_or_after(time, previous))
        {
            pending.set(Some(time));
        }
    });
    RECOVERY_FLAGS.with(|flags| {
        if let Some(flags) = flags.borrow().as_ref() {
            flags.set_recovery_pending(true);
        } else {
            let _marked = process_with_context(|context| context.flags.set_recovery_pending(true));
        }
    });
}

unsafe extern "system" fn desktop_switch_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    _hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    time: u32,
) {
    let _contained = std::panic::catch_unwind(|| {
        if event != EVENT_SYSTEM_DESKTOPSWITCH {
            return;
        }
        note_desktop_boundary(time);
        if !RECOVERY_QUEUED.replace(true) {
            let result = unsafe {
                // SAFETY: out-of-context WinEvents run on this hook-owning thread, whose message
                // queue already exists. Only scalar values are posted; no input state is sampled.
                PostThreadMessageW(
                    GetCurrentThreadId(),
                    WM_RECONCILE_KEYBOARD,
                    WPARAM(0),
                    LPARAM(0),
                )
            };
            if result.is_err() {
                // The periodic owning-thread check retries recovery if this wake-up was dropped.
                RECOVERY_QUEUED.set(false);
            }
        }
    });
}

fn own_desktop_receives_input() -> Result<bool, Error> {
    let mut active = BOOL::default();
    unsafe {
        // SAFETY: the desktop is borrowed from this thread and must not be closed. UOI_IO
        // writes exactly one BOOL into the initialized buffer; no thread queues are attached.
        let desktop = GetThreadDesktop(GetCurrentThreadId())?;
        GetUserObjectInformationW(
            HANDLE(desktop.0),
            UOI_IO,
            Some((&raw mut active).cast()),
            u32::try_from(core::mem::size_of::<BOOL>()).unwrap_or(4),
            None,
        )?;
    }
    Ok(active.as_bool())
}

pub(super) fn recover_keyboard_state(now: u32) {
    RECOVERY_QUEUED.set(false);
    recover_keyboard_state_with(now, own_desktop_receives_input, || {
        MODIFIER_KEYS.map(|key| key_pressed(key.0))
    });
}

fn recover_keyboard_state_with(
    now: u32,
    mut desktop_active: impl FnMut() -> Result<bool, Error>,
    sample: impl FnOnce() -> [bool; 8],
) {
    // Only message-loop work calls this function. Never interpret inaccessible-desktop zeros
    // from GetAsyncKeyState as an all-up keyboard, and retain pending recovery on query failure.
    match desktop_active() {
        Ok(true) => {}
        inactive => {
            if DESKTOP_BOUNDARY.get().is_none() {
                note_desktop_boundary(now);
                if let Err(error) = inactive {
                    eprintln!("Could not inspect input desktop: {error}");
                }
            }
            return;
        }
    }
    let Some(boundary) = DESKTOP_BOUNDARY.get() else {
        return;
    };
    let Some(before) = process_with_context(|context| context.keyboard_state.modifier_observations)
    else {
        return;
    };
    // Sample outside the RefCell borrow: reentrant hook events can record fresh evidence.
    let down = sample();
    if desktop_active() != Ok(true) || DESKTOP_BOUNDARY.get() != Some(boundary) {
        return;
    }
    let _recovered = process_with_context(|context| {
        context
            .keyboard_state
            .rebase_modifiers(boundary, before, down, &mut context.state);
        context.flags.advance_generation();
        DESKTOP_BOUNDARY.set(None);
        context.flags.set_recovery_pending(false);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::{CONTEXT, test_context};

    #[test]
    fn desktop_recovery_waits_for_access_and_coalesces_a_complete_gap() {
        CONTEXT.with(|slot| *slot.borrow_mut() = Some(test_context()));
        DESKTOP_BOUNDARY.set(None);
        note_desktop_boundary(200);
        note_desktop_boundary(300);
        note_desktop_boundary(250);
        assert_eq!(DESKTOP_BOUNDARY.get(), Some(300));
        for active in [Ok(false), Err(Error::empty())] {
            recover_keyboard_state_with(
                400,
                || active.clone(),
                || panic!("must not sample an inaccessible desktop"),
            );
            assert_eq!(DESKTOP_BOUNDARY.get(), Some(300));
            assert_eq!(
                process_with_context(|context| context.flags.recovery_pending()),
                Some(true)
            );
        }
        recover_keyboard_state_with(400, || Ok(true), || [false; 8]);
        assert_eq!(DESKTOP_BOUNDARY.get(), None);
        assert_eq!(
            process_with_context(|context| context.flags.recovery_pending()),
            Some(false)
        );
        CONTEXT.with(|slot| *slot.borrow_mut() = None);
    }
}
