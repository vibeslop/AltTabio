//! Low-level keyboard and mouse hooks on a dedicated thread. Callbacks translate input through
//! the pure `HookState` and post the resulting actions to the UI thread.

mod delivery;
mod flags;
mod keyboard_state;
mod recovery;
mod replay;

use alttabio::input::{
    HookOutcome, HookSettings, HookState, KeyEvent, KeyTransition, Modifiers, MouseEvent,
};
use alttabio::passthrough::PassthroughPolicy;
use delivery::{dispatch_registered_switch, post_actions, route_registered_switch};
use flags::{
    HookFlags, HookInterceptionGuard, INTERCEPTION_SUSPENDED, OVERLAY_ACTIVE, OVERLAY_FLAGS,
    SEARCH_ACTIVE,
};
use keyboard_state::{KeyboardState, translate_search_character};
use recovery::{KeyboardRecoveryWatcher, publish_recovery_flags, recover_keyboard_state};
use replay::{is_own_replayed_input, replay_key_events, replay_mouse_event};
use std::cell::RefCell;
use std::sync::mpsc::{self, SyncSender};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LWIN, VK_RWIN};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, GetWindowThreadProcessId, HHOOK,
    KBDLLHOOKSTRUCT, LLKHF_ALTDOWN, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, PeekMessageW,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_APP, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows::core::Error;

pub(crate) use alttabio::input::decode_virtual_key;
pub use flags::decode_action;
pub use replay::send_shell_escape;

pub const WM_HOOK_ACTION: u32 = WM_APP + 1;
pub const WM_HOOK_HOTKEY_ACTION: u32 = WM_APP + 21;
const WM_RESET_GESTURES: u32 = WM_APP + 2;
const WM_REPORT_HOOK_ERRORS: u32 = WM_APP + 3;

const HOOK_ERROR_REPLAY_INPUT: u8 = 1;
const HOOK_ERROR_POST_ACTION: u8 = 2;
const HOOK_ERROR_REGISTERED_SWITCH: u8 = 4;

struct HookContext {
    registered_switch: Option<crate::switch_hotkey::PendingSwitch>,
    registered_tab_down: bool,
    target: HWND,
    state: HookState,
    settings: HookSettings,
    remote_desktop_passthrough: Arc<AtomicBool>,
    target_thread_id: u32,
    hook_thread_id: u32,
    pending_errors: u8,
    flags: Arc<HookFlags>,
    interception_generation: usize,
    keyboard_state: KeyboardState,
    recovering: bool,
}

impl HookContext {
    fn sync_interception(&mut self) -> usize {
        let flags = self.flags.load();
        let generation = flags & !OVERLAY_FLAGS;
        let recovering = self.flags.recovery_pending.load(Ordering::Acquire);
        let suspended = flags & INTERCEPTION_SUSPENDED != 0 || recovering;
        if generation != self.interception_generation || recovering != self.recovering {
            self.registered_switch = None;
            self.state.set_interception_suspended(suspended);
            self.interception_generation = generation;
            self.recovering = recovering;
        }
        if suspended { 0 } else { flags & OVERLAY_FLAGS }
    }

    fn record_error(&mut self, error: u8) {
        if self.pending_errors & error != 0 {
            return;
        }
        self.pending_errors |= error;
        unsafe {
            // SAFETY: the hook thread created its queue before installing hooks. The pending bit is
            // retained even if this best-effort wake-up races with thread shutdown.
            let _wakeup = PostThreadMessageW(
                self.hook_thread_id,
                WM_REPORT_HOOK_ERRORS,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

thread_local! {
    static CONTEXT: RefCell<Option<HookContext>> = const { RefCell::new(None) };
}

pub struct HookThread {
    thread_id: u32,
    join_handle: Option<JoinHandle<()>>,
    remote_desktop_passthrough: Arc<AtomicBool>,
    flags: Arc<HookFlags>,
}

impl HookThread {
    pub fn start(target: HWND, settings: HookSettings) -> Result<Self, String> {
        let target_thread_id = unsafe {
            // SAFETY: target is the live overlay HWND and no process-id output is requested.
            GetWindowThreadProcessId(target, None)
        };
        if target_thread_id == 0 {
            return Err(format!(
                "Could not resolve the overlay input thread: {}",
                Error::from_thread()
            ));
        }
        let target_value = target.0 as isize;
        let keyboard_state = KeyboardState::snapshot()?;
        let flags = Arc::new(HookFlags::new());
        let hook_flags = Arc::clone(&flags);
        let remote_desktop_passthrough = Arc::new(AtomicBool::new(
            PassthroughPolicy::INITIAL.bypasses_local_switching(),
        ));
        let hook_remote_desktop_passthrough = Arc::clone(&remote_desktop_passthrough);
        let (sender, receiver) = mpsc::sync_channel(1);
        let join_handle = thread::Builder::new()
            .name("alttabio-hooks".to_owned())
            .spawn(move || {
                let target = HWND(target_value as *mut core::ffi::c_void);
                if let Err(error) = run_hook_thread(
                    HookContext {
                        registered_switch: None,
                        registered_tab_down: false,
                        target,
                        target_thread_id,
                        settings,
                        state: HookState::default(),
                        remote_desktop_passthrough: hook_remote_desktop_passthrough,
                        hook_thread_id: 0,
                        pending_errors: 0,
                        flags: hook_flags,
                        interception_generation: 0,
                        keyboard_state,
                        recovering: false,
                    },
                    &sender,
                ) && sender.send(Err(error)).is_err()
                {
                    eprintln!("Input hook thread failed after its owner exited");
                }
            })
            .map_err(|error| format!("Could not start the input hook thread: {error}"))?;

        match receiver.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                thread_id,
                join_handle: Some(join_handle),
                remote_desktop_passthrough,
                flags,
            }),
            Ok(Err(error)) => {
                report_join_error(join_handle.join(), "after setup failed");
                Err(error)
            }
            Err(error) => {
                report_join_error(join_handle.join(), "before setup completed");
                Err(format!("Input hook setup ended unexpectedly: {error}"))
            }
        }
    }

    pub fn set_search_active(&self, active: bool) {
        self.flags.set(SEARCH_ACTIVE, active);
    }

    pub fn set_overlay_active(&self, active: bool) {
        self.flags.set(OVERLAY_ACTIVE, active);
    }

    /// Cancels interception across modal boundaries without touching the UI's restored flags.
    pub fn set_interception_suspended(&self, suspended: bool) {
        self.flags.suspend(suspended);
    }

    /// Suspends interception until drop without borrowing the hook owner across a modal call.
    /// Restores the saved search, overlay, and suspension flags, including an outer suspension.
    pub fn suspend_interception(&self) -> HookInterceptionGuard {
        HookInterceptionGuard::new(Arc::clone(&self.flags))
    }

    pub fn action_is_current(&self, wparam: WPARAM) -> bool {
        self.flags.action_is_current(wparam)
    }

    pub fn set_remote_desktop_passthrough(&self, policy: PassthroughPolicy) -> Result<(), String> {
        let bypass = policy.bypasses_local_switching();
        let was_bypass = self
            .remote_desktop_passthrough
            .swap(bypass, Ordering::Release);
        if was_bypass == bypass {
            return Ok(());
        }
        self.reset_gestures()
    }

    pub fn reset_gestures(&self) -> Result<(), String> {
        unsafe {
            // SAFETY: `thread_id` identifies the live hook thread whose queue is created before
            // `HookThread::start` returns. The message carries no borrowed data.
            PostThreadMessageW(self.thread_id, WM_RESET_GESTURES, WPARAM(0), LPARAM(0))
        }
        .map_err(|error| format!("Could not release input-hook gesture ownership: {error}"))
    }
}

impl Drop for HookThread {
    fn drop(&mut self) {
        let post_result = unsafe {
            // SAFETY: `thread_id` identifies the live hook thread whose queue is created before
            // `HookThread::start` returns.
            PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0))
        };
        if let Err(error) = post_result {
            eprintln!("Could not request input hook shutdown: {error}");
        }
        if let Some(join_handle) = self.join_handle.take() {
            report_join_error(join_handle.join(), "during shutdown");
        }
    }
}

fn run_hook_thread(
    mut hook_context: HookContext,
    ready: &SyncSender<Result<u32, String>>,
) -> Result<(), String> {
    let thread_id = unsafe {
        // SAFETY: GetCurrentThreadId has no preconditions.
        GetCurrentThreadId()
    };
    let mut message = MSG::default();
    unsafe {
        // SAFETY: the pointer is valid for the call; PM_NOREMOVE ensures this thread's message
        // queue exists before another thread posts WM_QUIT.
        let _queue_ready = PeekMessageW(&raw mut message, None, 0, 0, PM_NOREMOVE);
    }

    hook_context.hook_thread_id = thread_id;
    publish_recovery_flags(Some(Arc::clone(&hook_context.flags)));
    CONTEXT.with(|context| *context.borrow_mut() = Some(hook_context));

    let module = unsafe {
        // SAFETY: None requests a borrowed handle for this executable module.
        GetModuleHandleW(None)
    }
    .map_err(|error| format!("Could not resolve the executable module: {error}"))?;
    let instance = HINSTANCE(module.0);
    let keyboard = unsafe {
        // SAFETY: `keyboard_proc` has the required ABI and remains valid until this thread removes
        // the hook after its message loop exits.
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(instance), 0)
    }
    .map_err(|error| format!("Could not install the keyboard hook: {error}"))?;
    let mouse = match unsafe {
        // SAFETY: `mouse_proc` has the required ABI and remains valid until this thread removes the
        // hook after its message loop exits.
        SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(instance), 0)
    } {
        Ok(hook) => hook,
        Err(error) => {
            remove_hook(keyboard, "keyboard");
            return Err(format!("Could not install the mouse hook: {error}"));
        }
    };

    let recovery = match KeyboardRecoveryWatcher::install() {
        Ok(watcher) => watcher,
        Err(error) => {
            remove_hook(mouse, "mouse");
            remove_hook(keyboard, "keyboard");
            return Err(error);
        }
    };

    if let Err(error) = ready.send(Ok(thread_id)) {
        remove_hook(mouse, "mouse");
        remove_hook(keyboard, "keyboard");
        CONTEXT.with(|context| *context.borrow_mut() = None);
        return Err(format!("Could not report successful hook setup: {error}"));
    }

    let loop_result = loop {
        let result = unsafe {
            // SAFETY: `message` is writable for the call and this thread owns the message loop.
            GetMessageW(&raw mut message, None, 0, 0)
        };
        report_callback_errors();
        if result.0 == -1 {
            break Err("The input hook message loop failed".to_owned());
        }
        if result.0 == 0 {
            break Ok(());
        }
        dispatch_registered_switch(None);
        if message.message == WM_HOTKEY && crate::switch_hotkey::owns_id(message.wParam.0) {
            dispatch_registered_switch(Some(message.wParam.0));
            continue;
        }
        if recovery.handles(&message) {
            recover_keyboard_state(message.time);
            continue;
        }
        if message.message == WM_RESET_GESTURES {
            reset_context();
            continue;
        }
        if message.message == WM_REPORT_HOOK_ERRORS {
            continue;
        }
        unsafe {
            // SAFETY: GetMessageW initialized `message` for this thread.
            let _translated = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    };

    remove_hook(mouse, "mouse");
    remove_hook(keyboard, "keyboard");
    drop(recovery);
    CONTEXT.with(|context| *context.borrow_mut() = None);
    publish_recovery_flags(None);
    loop_result
}

fn remove_hook(hook: HHOOK, kind: &str) {
    let result = unsafe {
        // SAFETY: the HHOOK was successfully created on this thread and is removed exactly once.
        UnhookWindowsHookEx(hook)
    };
    if let Err(error) = result {
        eprintln!("Could not remove the {kind} hook: {error}");
    }
}

fn report_join_error(result: thread::Result<()>, context: &str) {
    if let Err(error) = result {
        eprintln!("Input hook thread panicked {context}: {error:?}");
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return call_next(code, wparam, lparam);
    }

    std::panic::catch_unwind(|| {
        finish_callback_with(
            process_keyboard_message(wparam, lparam),
            post_actions,
            reset_context,
            || forward_keyboard(code, wparam, lparam),
        )
    })
    .unwrap_or_else(|_| forward_keyboard(code, wparam, lparam))
}

fn forward_keyboard(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    forward_keyboard_with(
        || call_next(code, wparam, lparam),
        || commit_keyboard_delivery(wparam, lparam),
    )
}

fn forward_keyboard_with(next: impl FnOnce() -> LRESULT, commit: impl FnOnce()) -> LRESULT {
    let result = next();
    if result.0 == 0 {
        commit();
    }
    result
}

fn commit_keyboard_delivery(wparam: WPARAM, lparam: LPARAM) {
    let transition = match u32::try_from(wparam.0) {
        Ok(WM_KEYDOWN | WM_SYSKEYDOWN) => KeyTransition::Pressed,
        Ok(WM_KEYUP | WM_SYSKEYUP) => KeyTransition::Released,
        _ => return,
    };
    let data = unsafe {
        // SAFETY: called only while handling the original nonnegative keyboard hook callback;
        // Windows keeps its KBDLLHOOKSTRUCT alive until we return to the hook chain.
        (lparam.0 as *const KBDLLHOOKSTRUCT).as_ref()
    };
    if let Some(data) = data {
        let _committed = process_with_context(|context| {
            context
                .keyboard_state
                .commit_forwarded(data.vkCode, transition);
        });
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return call_next(code, wparam, lparam);
    }

    std::panic::catch_unwind(|| {
        finish_callback(code, wparam, lparam, process_mouse_message(wparam, lparam))
    })
    .unwrap_or_else(|_| call_next(code, wparam, lparam))
}

fn process_keyboard_message(wparam: WPARAM, lparam: LPARAM) -> Option<HookOutcome> {
    let transition = match u32::try_from(wparam.0).ok()? {
        WM_KEYDOWN | WM_SYSKEYDOWN => KeyTransition::Pressed,
        WM_KEYUP | WM_SYSKEYUP => KeyTransition::Released,
        _ => return None,
    };
    let data = unsafe {
        // SAFETY: for a nonnegative low-level keyboard hook code, Windows guarantees lParam points
        // to a KBDLLHOOKSTRUCT for the callback duration.
        (lparam.0 as *const KBDLLHOOKSTRUCT).as_ref()
    }?;
    if is_own_replayed_input(data.dwExtraInfo) {
        return None;
    }
    let key = decode_virtual_key(data.vkCode);
    let modifiers = Modifiers {
        alt: data.flags.contains(LLKHF_ALTDOWN),
        left_windows: key_pressed(VK_LWIN.0),
        right_windows: key_pressed(VK_RWIN.0),
    };

    let (mut outcome, replayed_key_events) = process_with_context(|context| {
        context
            .keyboard_state
            .observe_at(data.vkCode, transition, data.time);
        let flags = context.sync_interception();
        let search_active = flags & SEARCH_ACTIVE != 0;
        context
            .state
            .set_overlay_active(flags & OVERLAY_ACTIVE != 0);
        let mut settings = PassthroughPolicy::from_bypass_flag(
            context.remote_desktop_passthrough.load(Ordering::Acquire),
        )
        .apply(context.settings);
        settings.search_active = search_active;
        let text = if search_active && transition == KeyTransition::Pressed {
            translate_search_character(data, context.target_thread_id, &context.keyboard_state)
        } else {
            None
        };
        let outcome = context.state.process_key(
            KeyEvent {
                key,
                transition,
                modifiers,
                text,
            },
            settings,
        );
        let replayed_key_events = context.state.take_replayed_key_events();
        (outcome, replayed_key_events)
    })?;
    if !replay_key_events(replayed_key_events, &mut outcome) {
        record_callback_error(HOOK_ERROR_REPLAY_INPUT);
    }
    outcome =
        process_with_context(|context| route_registered_switch(context, outcome, data, transition))
            .unwrap_or(outcome);
    Some(outcome)
}

fn process_mouse_message(wparam: WPARAM, lparam: LPARAM) -> Option<HookOutcome> {
    let data = unsafe {
        // SAFETY: for a nonnegative low-level mouse hook code, Windows guarantees lParam points to
        // an MSLLHOOKSTRUCT for the callback duration.
        (lparam.0 as *const MSLLHOOKSTRUCT).as_ref()
    }?;
    if is_own_replayed_input(data.dwExtraInfo) {
        return None;
    }
    let event = match u32::try_from(wparam.0).ok()? {
        WM_RBUTTONDOWN => MouseEvent::RightButtonPressed,
        WM_RBUTTONUP => MouseEvent::RightButtonReleased,
        WM_MOUSEWHEEL => MouseEvent::Wheel((data.mouseData >> 16) as i16),
        _ => return None,
    };
    let (mut outcome, replayed_mouse_event) = process_with_context(|context| {
        let flags = context.sync_interception();
        context
            .state
            .set_overlay_active(flags & OVERLAY_ACTIVE != 0);
        let outcome = context.state.process_mouse(event, context.settings);
        let replayed_mouse_event = context.state.take_replayed_mouse_event();
        (outcome, replayed_mouse_event)
    })?;
    if !replay_mouse_event(replayed_mouse_event, &mut outcome) {
        record_callback_error(HOOK_ERROR_REPLAY_INPUT);
        let _abandoned =
            process_with_context(|context| context.state.abandon_right_button_gesture());
        reset_context();
        outcome = HookOutcome::default();
    }
    Some(outcome)
}

fn process_with_context<T>(process: impl FnOnce(&mut HookContext) -> T) -> Option<T> {
    CONTEXT
        .try_with(|context| {
            let mut context = context.try_borrow_mut().ok()?;
            context.as_mut().map(process)
        })
        .ok()
        .flatten()
}

fn finish_callback(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
    outcome: Option<HookOutcome>,
) -> LRESULT {
    finish_callback_with(outcome, post_actions, reset_context, || {
        call_next(code, wparam, lparam)
    })
}

fn finish_callback_with(
    outcome: Option<HookOutcome>,
    post: impl FnOnce(HookOutcome) -> bool,
    reset: impl FnOnce(),
    call_next: impl FnOnce() -> LRESULT,
) -> LRESULT {
    let Some(outcome) = outcome else {
        return call_next();
    };
    let posted = post(outcome);
    if !posted {
        reset();
    }
    if outcome.suppress && posted {
        LRESULT(1)
    } else {
        call_next()
    }
}

fn reset_context() {
    let _outcome = process_with_context(|context| {
        context.registered_switch = None;
        context.state.reset_gestures();
        HookOutcome::default()
    });
}

fn record_callback_error(error: u8) {
    let _recorded = process_with_context(|context| context.record_error(error));
}

fn report_callback_errors() {
    if let Some(error) = crate::switch_hotkey::take_registration_error() {
        eprintln!("Could not register physical switch hotkey: {error}");
    }
    if let Some(error) = crate::switch_hotkey::take_cleanup_error() {
        eprintln!("Could not unregister physical switch hotkey: {error}");
    }
    let pending = process_with_context(|context| core::mem::take(&mut context.pending_errors))
        .unwrap_or_default();
    if pending & HOOK_ERROR_REPLAY_INPUT != 0 {
        eprintln!("Could not replay a suppressed input sequence from the input hook");
    }
    if pending & HOOK_ERROR_POST_ACTION != 0 {
        eprintln!("Could not post an input action from the input hook");
    }
    if pending & HOOK_ERROR_REGISTERED_SWITCH != 0 {
        eprintln!("Could not route physical Tab through a registered switch hotkey");
    }
}

fn call_next(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        // SAFETY: forwarding the unmodified callback arguments is required by the hook contract.
        CallNextHookEx(None, code, wparam, lparam)
    }
}

fn key_pressed(virtual_key: u16) -> bool {
    unsafe {
        // SAFETY: GetAsyncKeyState accepts any virtual-key code and has no pointer preconditions.
        GetAsyncKeyState(i32::from(virtual_key)) < 0
    }
}

#[cfg(test)]
fn test_context() -> HookContext {
    HookContext {
        registered_switch: None,
        registered_tab_down: false,
        target: HWND::default(),
        state: HookState::default(),
        settings: HookSettings::default(),
        remote_desktop_passthrough: Arc::new(AtomicBool::new(false)),
        target_thread_id: 0,
        hook_thread_id: 0,
        pending_errors: 0,
        flags: Arc::new(HookFlags::default()),
        interception_generation: 0,
        keyboard_state: KeyboardState::default(),
        recovering: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alttabio::input::{InputAction, Key};
    use replay::REPLAYED_INPUT_MARKER;
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_CAPITAL, VK_LSHIFT, VK_SHIFT};

    #[test]
    fn swallowed_and_replayed_toggles_follow_actual_chain_delivery() {
        let mut context = test_context();
        let settings = context.settings;
        let _win = context.state.process_key(
            KeyEvent::pressed(Key::LeftWindows, Modifiers::default()),
            settings,
        );
        let _tab = context
            .state
            .process_key(KeyEvent::pressed(Key::Tab, Modifiers::default()), settings);
        CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));
        for message in [WM_KEYDOWN, WM_KEYDOWN, WM_KEYUP] {
            let data = KBDLLHOOKSTRUCT {
                vkCode: u32::from(VK_CAPITAL.0),
                ..KBDLLHOOKSTRUCT::default()
            };
            let wparam = WPARAM(message as usize);
            let lparam = LPARAM((&raw const data) as isize);
            let outcome = process_keyboard_message(wparam, lparam);
            let result = finish_callback_with(
                outcome,
                |_| true,
                || {},
                || {
                    commit_keyboard_delivery(wparam, lparam);
                    LRESULT(0)
                },
            );
            assert_eq!(result, LRESULT(1));
            assert_eq!(
                process_with_context(|context| context.keyboard_state.keys
                    [usize::from(VK_CAPITAL.0)]
                    & 1),
                Some(0)
            );
        }
        // Tagged SendInput replay bypasses interception, but only commits when lower hooks agree.
        let data = KBDLLHOOKSTRUCT {
            vkCode: u32::from(VK_CAPITAL.0),
            dwExtraInfo: REPLAYED_INPUT_MARKER,
            ..KBDLLHOOKSTRUCT::default()
        };
        let wparam = WPARAM(WM_KEYDOWN as usize);
        let lparam = LPARAM((&raw const data) as isize);
        assert_eq!(process_keyboard_message(wparam, lparam), None);
        assert_eq!(
            forward_keyboard_with(|| LRESULT(1), || commit_keyboard_delivery(wparam, lparam)),
            LRESULT(1)
        );
        assert_eq!(
            process_with_context(
                |context| context.keyboard_state.keys[usize::from(VK_CAPITAL.0)] & 1
            ),
            Some(0)
        );
        for _repeat in 0..2 {
            assert_eq!(
                forward_keyboard_with(|| LRESULT(0), || commit_keyboard_delivery(wparam, lparam)),
                LRESULT(0)
            );
            assert_eq!(
                process_with_context(|context| context.keyboard_state.keys
                    [usize::from(VK_CAPITAL.0)]
                    & 1),
                Some(1)
            );
        }
        CONTEXT.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn repeated_ui_synchronization_does_not_cancel_a_live_gesture() {
        let mut context = test_context();
        let _tab = context.state.process_key(
            KeyEvent::pressed(
                Key::Tab,
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
            ),
            context.settings,
        );
        context.flags.set(OVERLAY_ACTIVE, true);
        context.flags.suspend(false);
        assert_eq!(context.sync_interception(), OVERLAY_ACTIVE);
        assert_eq!(
            context
                .state
                .process_key(
                    KeyEvent::released(Key::Alt, Modifiers::default()),
                    context.settings
                )
                .actions()
                .next(),
            Some(InputAction::AltReleased)
        );
        context.flags.suspend(true);
        let suspended_generation = context.flags.load();
        context.flags.suspend(true);
        assert_eq!(context.flags.load(), suspended_generation);
    }

    #[test]
    fn suspended_keyboard_callbacks_keep_modifier_and_caps_state_for_resume() {
        let context = test_context();
        let flags = Arc::clone(&context.flags);
        flags.suspend(true);
        CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));
        for key in [VK_LSHIFT, VK_CAPITAL, VK_CAPITAL] {
            let data = KBDLLHOOKSTRUCT {
                vkCode: u32::from(key.0),
                ..KBDLLHOOKSTRUCT::default()
            };
            assert_eq!(
                process_keyboard_message(
                    WPARAM(WM_KEYDOWN as usize),
                    LPARAM((&raw const data) as isize)
                ),
                Some(HookOutcome::default())
            );
            commit_keyboard_delivery(
                WPARAM(WM_KEYDOWN as usize),
                LPARAM((&raw const data) as isize),
            );
        }
        flags.set(SEARCH_ACTIVE, true);
        flags.suspend(false);
        let context = CONTEXT.with(|slot| slot.borrow_mut().take());
        let Some(mut context) = context else {
            panic!("missing test context")
        };
        assert_eq!(context.sync_interception(), SEARCH_ACTIVE);
        assert_eq!(context.keyboard_state.keys[usize::from(VK_SHIFT.0)], 0x80);
        assert_eq!(context.keyboard_state.keys[usize::from(VK_CAPITAL.0)], 0x81);
        let mut expected = KeyboardState::default();
        expected.keys[usize::from(VK_SHIFT.0)] = 0x80;
        expected.keys[usize::from(VK_CAPITAL.0)] = 1;
        let data = KBDLLHOOKSTRUCT {
            vkCode: u32::from(b'A'),
            ..KBDLLHOOKSTRUCT::default()
        };
        let expected_text = translate_search_character(&data, 0, &expected);
        assert!(expected_text.is_some());
        assert_eq!(
            translate_search_character(&data, 0, &context.keyboard_state),
            expected_text
        );
    }

    #[test]
    fn failed_ui_delivery_forwards_the_suppressed_key_to_windows() {
        let mut state = HookState::default();
        let outcome = state.process_key(
            KeyEvent::pressed(
                Key::Tab,
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
            ),
            HookSettings {
                replace_alt_tab: true,
                ..HookSettings::default()
            },
        );
        let mut reset = false;
        let next_result = LRESULT(42);

        let result =
            finish_callback_with(Some(outcome), |_| false, || reset = true, || next_result);

        assert_eq!(result, next_result);
        assert!(reset);
    }
}
