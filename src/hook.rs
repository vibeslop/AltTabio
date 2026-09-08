use alttabio::input::{
    HookOutcome, HookSettings, HookState, InputAction, Key, KeyEvent, KeyTransition, Modifiers,
    MouseEvent, ReplayedKeyEvent, ReplayedMouseEvent,
};
use alttabio::passthrough::PassthroughPolicy;
use std::cell::{Cell, RefCell};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread::{self, JoinHandle};
use windows::Win32::Foundation::{HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::StationsAndDesktops::{
    GetThreadDesktop, GetUserObjectInformationW, UOI_IO,
};
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyboardLayout, GetKeyboardState, INPUT, INPUT_0, INPUT_KEYBOARD,
    INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    MOUSEEVENTF_RIGHTUP, MOUSEINPUT, SendInput, ToUnicodeEx, VIRTUAL_KEY, VK_0, VK_1, VK_9,
    VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8,
    VK_F9, VK_HOME, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_NUMLOCK,
    VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD9, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT,
    VK_RWIN, VK_SCROLL, VK_SHIFT, VK_SNAPSHOT, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CallNextHookEx, DispatchMessageW, EVENT_SYSTEM_DESKTOPSWITCH,
    GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, HHOOK, KBDLLHOOKSTRUCT, KillTimer,
    LLKHF_ALTDOWN, LLKHF_INJECTED, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, PeekMessageW, PostMessageW,
    PostThreadMessageW, SetTimer, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WINEVENT_OUTOFCONTEXT, WM_APP, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP,
    WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER,
};
use windows::core::{BOOL, Error};

pub const WM_HOOK_ACTION: u32 = WM_APP + 1;
pub const WM_HOOK_HOTKEY_ACTION: u32 = WM_APP + 21;
const WM_RESET_GESTURES: u32 = WM_APP + 2;
const WM_REPORT_HOOK_ERRORS: u32 = WM_APP + 3;
const WM_RECONCILE_KEYBOARD: u32 = WM_APP + 20;

const HOOK_ERROR_REPLAY_INPUT: u8 = 1;
const HOOK_ERROR_POST_ACTION: u8 = 2;
const HOOK_ERROR_REGISTERED_SWITCH: u8 = 4;

const ACTION_SWITCH: usize = 1;
const ACTION_ACTIVATE_POSITION: usize = 2;
const ACTION_ALT_RELEASED: usize = 3;
const ACTION_RIGHT_BUTTON_PRESSED: usize = 4;
const ACTION_RIGHT_BUTTON_RELEASED: usize = 5;
const ACTION_MOUSE_WHEEL: usize = 6;
const ACTION_APPEND_SEARCH_CHARACTER: usize = 8;
const ACTION_BACKSPACE_SEARCH: usize = 9;
const ACTION_NAVIGATE: usize = 10;
const ACTION_ACTIVATE_SELECTED: usize = 11;
const ACTION_SELECT_FIRST: usize = 12;
const ACTION_SELECT_LAST: usize = 13;
const ACTION_DISMISS_OVERLAY: usize = 14;
const ACTION_CLOSE_SELECTED: usize = 15;
const ACTION_WINDOW_COMMAND: usize = 16;
const REPLAYED_INPUT_MARKER: usize = 0x0A17_AB10;
const INTERCEPTION_SUSPENDED: usize = 1;
const SEARCH_ACTIVE: usize = 2;
const OVERLAY_ACTIVE: usize = 4;
const OVERLAY_FLAGS: usize = SEARCH_ACTIVE | OVERLAY_ACTIVE;
const ACTION_CODE_MASK: usize = 0xFF;
const ACTION_EPOCH_MASK: usize = usize::MAX >> 8;
static NEXT_HOOK_EPOCH: AtomicUsize = AtomicUsize::new(1);

fn next_hook_generation() -> usize {
    (NEXT_HOOK_EPOCH.fetch_add(1, Ordering::Relaxed) & ACTION_EPOCH_MASK) << 3
}

const fn action_wparam(code: usize, generation: usize) -> WPARAM {
    WPARAM((((generation >> 3) & ACTION_EPOCH_MASK) << 8) | (code & ACTION_CODE_MASK))
}

#[derive(Default)]
struct HookFlags {
    value: AtomicUsize,
    recovery_pending: AtomicBool,
    shell_window: AtomicUsize,
}

impl HookFlags {
    fn new() -> Self {
        Self::with_value(next_hook_generation())
    }

    fn with_value(value: usize) -> Self {
        Self {
            value: AtomicUsize::new(value),
            recovery_pending: AtomicBool::new(false),
            shell_window: AtomicUsize::new(0),
        }
    }

    fn action_is_current(&self, wparam: WPARAM) -> bool {
        let flags = self.load();
        !self.recovery_pending.load(Ordering::Acquire)
            && flags & INTERCEPTION_SUSPENDED == 0
            && (wparam.0 >> 8) == ((flags >> 3) & ACTION_EPOCH_MASK)
    }

    fn load(&self) -> usize {
        self.value.load(Ordering::Acquire)
    }

    fn set(&self, flag: usize, active: bool) {
        if active {
            self.value.fetch_or(flag, Ordering::AcqRel);
        } else {
            self.value.fetch_and(!flag, Ordering::AcqRel);
        }
    }

    fn suspend(&self, suspended: bool) {
        // Each modal boundary advances the generation, even if no callback ran in between.
        let _previous = self
            .value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                if (value & INTERCEPTION_SUSPENDED != 0) == suspended {
                    return None;
                }
                let flags = if suspended {
                    INTERCEPTION_SUSPENDED
                } else {
                    value & OVERLAY_FLAGS
                };
                Some(next_hook_generation() | flags)
            });
    }

    fn replace_modal_flags(&self, flags: usize) -> usize {
        // The closure always supplies a value, so fetch_update retries until it swaps the
        // complete snapshot. Entry and exit advance the generation even when already suspended.
        self.value
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |_value| {
                Some(next_hook_generation() | (flags & 7))
            })
            .unwrap_or_else(|value| value)
            & 7
    }

    fn update_overlay(&self, generation: usize, active: bool, typed_search: bool) -> bool {
        let value = self.load();
        if self.recovery_pending.load(Ordering::Acquire)
            || value & !OVERLAY_FLAGS != generation
            || value & INTERCEPTION_SUSPENDED != 0
        {
            return false;
        }
        let flags = if active {
            OVERLAY_ACTIVE | if typed_search { SEARCH_ACTIVE } else { 0 }
        } else {
            0
        };
        // A single attempt keeps the hook callback bounded. A concurrent UI update wins.
        self.value
            .compare_exchange(
                value,
                generation | flags,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

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
    // Independent publication lets reentrant desktop callbacks gate delivery during state borrows.
    static RECOVERY_FLAGS: RefCell<Option<Arc<HookFlags>>> = const { RefCell::new(None) };
    static DESKTOP_BOUNDARY: Cell<Option<u32>> = const { Cell::new(None) };
    static RECOVERY_QUEUED: Cell<bool> = const { Cell::new(false) };
}

pub struct HookThread {
    thread_id: u32,
    join_handle: Option<JoinHandle<()>>,
    remote_desktop_passthrough: Arc<AtomicBool>,
    flags: Arc<HookFlags>,
}

/// Restores a modal scope's saved flags on drop. Nested guards must drop in reverse order.
#[must_use = "keep the guard alive for the entire modal call"]
pub struct HookInterceptionGuard {
    flags: Arc<HookFlags>,
    saved_flags: usize,
}

impl HookInterceptionGuard {
    fn new(flags: Arc<HookFlags>) -> Self {
        let saved_flags = flags.replace_modal_flags(INTERCEPTION_SUSPENDED);
        Self { flags, saved_flags }
    }
}

impl Drop for HookInterceptionGuard {
    fn drop(&mut self) {
        let _previous = self.flags.replace_modal_flags(self.saved_flags);
    }
}

impl HookThread {
    pub fn set_shell_window(&self, window: Option<HWND>) {
        self.flags.shell_window.store(
            window.map_or(0, |window| window.0 as usize),
            Ordering::Release,
        );
    }

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

pub fn decode_action(wparam: WPARAM, lparam: LPARAM) -> Option<InputAction> {
    match wparam.0 & ACTION_CODE_MASK {
        ACTION_SWITCH => i32::try_from(lparam.0).ok().map(InputAction::Switch),
        ACTION_ACTIVATE_POSITION => usize::try_from(lparam.0)
            .ok()
            .map(InputAction::ActivateVisiblePosition),
        ACTION_ALT_RELEASED => Some(InputAction::AltReleased),
        ACTION_RIGHT_BUTTON_PRESSED => Some(InputAction::RightButtonPressed),
        ACTION_RIGHT_BUTTON_RELEASED => Some(InputAction::RightButtonReleased),
        ACTION_MOUSE_WHEEL => i32::try_from(lparam.0).ok().map(InputAction::MouseWheel),
        ACTION_APPEND_SEARCH_CHARACTER => u32::try_from(lparam.0)
            .ok()
            .and_then(char::from_u32)
            .map(InputAction::AppendSearchCharacter),
        ACTION_BACKSPACE_SEARCH => Some(InputAction::BackspaceSearch),
        ACTION_NAVIGATE => i32::try_from(lparam.0).ok().map(InputAction::Navigate),
        ACTION_ACTIVATE_SELECTED => Some(InputAction::ActivateSelected),
        ACTION_SELECT_FIRST => Some(InputAction::SelectFirst),
        ACTION_SELECT_LAST => Some(InputAction::SelectLast),
        ACTION_DISMISS_OVERLAY => Some(InputAction::DismissOverlay),
        ACTION_CLOSE_SELECTED => Some(InputAction::CloseSelected),
        ACTION_WINDOW_COMMAND => u8::try_from(lparam.0)
            .ok()
            .and_then(alttabio::input::WindowCommand::from_function_key)
            .map(InputAction::WindowCommand),
        _ => None,
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
    RECOVERY_FLAGS.with(|flags| *flags.borrow_mut() = Some(Arc::clone(&hook_context.flags)));
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
        if message.message == WM_RECONCILE_KEYBOARD
            || (message.message == WM_TIMER && message.wParam.0 == recovery.timer)
        {
            RECOVERY_QUEUED.set(false);
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
    RECOVERY_FLAGS.with(|flags| *flags.borrow_mut() = None);
    loop_result
}

struct KeyboardRecoveryWatcher {
    hook: HWINEVENTHOOK,
    timer: usize,
}

impl KeyboardRecoveryWatcher {
    fn install() -> Result<Self, String> {
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
            flags.recovery_pending.store(true, Ordering::Release);
        } else {
            let _marked = process_with_context(|context| {
                context
                    .flags
                    .recovery_pending
                    .store(true, Ordering::Release);
            });
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

fn recover_keyboard_state(now: u32) {
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
        // Invalidate queued actions without overwriting concurrent UI flag changes.
        let _previous =
            context
                .flags
                .value
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    Some(next_hook_generation() | (value & 7))
                });
        DESKTOP_BOUNDARY.set(None);
        context
            .flags
            .recovery_pending
            .store(false, Ordering::Release);
    });
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

fn post_actions(outcome: HookOutcome) -> bool {
    CONTEXT
        .try_with(|context| {
            let mut context = context.try_borrow_mut().ok()?;
            let context = context.as_mut()?;
            let generation = context.interception_generation;
            Some(post_context_actions(context, outcome, |target, action| {
                post_action(target, action, generation)
            }))
        })
        .ok()
        .flatten()
        .unwrap_or(false)
}

fn post_context_actions(
    context: &mut HookContext,
    outcome: HookOutcome,
    mut post: impl FnMut(HWND, InputAction) -> bool,
) -> bool {
    for action in outcome.actions() {
        if context.flags.recovery_pending.load(Ordering::Acquire)
            || context.flags.load() & !OVERLAY_FLAGS != context.interception_generation
            || context.interception_generation & INTERCEPTION_SUSPENDED != 0
        {
            // Keep release ownership even when a modal boundary races with this callback.
            return true;
        }
        let opens_overlay = matches!(
            action,
            InputAction::Switch(_) | InputAction::RightButtonPressed
        );
        if opens_overlay
            && !context.flags.update_overlay(
                context.interception_generation,
                true,
                context.settings.typed_search,
            )
        {
            context.state.reset_gestures();
            return true;
        }
        if !post(context.target, action) {
            context.record_error(HOOK_ERROR_POST_ACTION);
            if opens_overlay {
                let _cleared =
                    context
                        .flags
                        .update_overlay(context.interception_generation, false, false);
            }
            return false;
        }
    }
    true
}

fn post_action(target: HWND, action: InputAction, generation: usize) -> bool {
    post_action_message(target, action, generation, WM_HOOK_ACTION)
}

fn route_registered_switch(
    context: &mut HookContext,
    mut outcome: HookOutcome,
    data: &KBDLLHOOKSTRUCT,
    transition: KeyTransition,
) -> HookOutcome {
    if data.vkCode == u32::from(VK_TAB.0)
        && transition == KeyTransition::Released
        && context.registered_tab_down
    {
        // Windows received this physical down as a hotkey. Balance it even if the pure
        // switcher would normally own the release of an intercepted Tab.
        context.registered_tab_down = false;
        outcome.suppress = false;
    }
    if context.flags.load() & !OVERLAY_FLAGS != context.interception_generation
        || outcome
            .actions()
            .any(|action| action == InputAction::DismissOverlay)
    {
        context.registered_switch = None;
        return outcome;
    }
    if let Some(pending) = &mut context.registered_switch {
        if !pending.actions.push(outcome) {
            context.registered_switch = None;
            context.state.reset_gestures();
            let _cleared =
                context
                    .flags
                    .update_overlay(context.interception_generation, false, false);
            if !post_action(
                context.target,
                InputAction::DismissOverlay,
                context.interception_generation,
            ) {
                context.record_error(HOOK_ERROR_POST_ACTION);
            }
            context.record_error(HOOK_ERROR_REGISTERED_SWITCH);
        }
        let mut deferred = HookOutcome::default();
        deferred.suppress = outcome.suppress;
        return deferred;
    }
    let shell_window = context.flags.shell_window.load(Ordering::Acquire);
    if data.vkCode != u32::from(VK_TAB.0) || transition != KeyTransition::Pressed
        || data.flags.contains(LLKHF_INJECTED)
        || !outcome.actions().any(|action| matches!(action, InputAction::Switch(_)))
        || shell_window == 0
        // SAFETY: this bounded metadata query does not enumerate or send messages to windows.
        || unsafe { GetForegroundWindow() }.0 as usize != shell_window
    {
        return outcome;
    }
    if let Ok(mut pending) =
        crate::switch_hotkey::PendingSwitch::register(context.interception_generation)
    {
        if !context.flags.update_overlay(
            context.interception_generation,
            true,
            context.settings.typed_search,
        ) {
            return outcome;
        }
        if !pending.actions.push(outcome) {
            return outcome;
        }
        context.registered_switch = Some(pending);
        context.registered_tab_down = true;
        HookOutcome::default()
    } else {
        context.record_error(HOOK_ERROR_REGISTERED_SWITCH);
        outcome
    }
}

fn dispatch_registered_switch(hotkey_id: Option<usize>) {
    let pending = process_with_context(|context| {
        let ready = context.registered_switch.as_ref().is_some_and(|pending| {
            hotkey_id.map_or_else(|| pending.expired(), |id| pending.matches_id(id))
        });
        if ready {
            context.registered_switch.take()
        } else {
            None
        }
    })
    .flatten();
    let Some(mut pending) = pending else {
        return;
    };
    let generation = pending.generation;
    let native = hotkey_id.is_some();
    if native {
        // SAFETY: this thread received the physical registered hotkey. Explicitly share its
        // activation permission with our process before notifying the separate UI thread.
        let granted = unsafe { AllowSetForegroundWindow(GetCurrentProcessId()) };
        if let Err(error) = granted {
            eprintln!("Could not share physical hotkey activation permission: {error}");
        }
    }
    let outcomes = pending.actions.take();
    drop(pending);
    for outcome in outcomes {
        let _posted = process_with_context(|context| {
            if context.interception_generation != generation {
                return;
            }
            let message = if native {
                WM_HOOK_HOTKEY_ACTION
            } else {
                WM_HOOK_ACTION
            };
            if !post_context_actions(context, outcome, |target, action| {
                post_action_message(target, action, generation, message)
            }) {
                context.state.reset_gestures();
            }
        });
    }
}

fn post_action_message(target: HWND, action: InputAction, generation: usize, message: u32) -> bool {
    let (code, value) = match action {
        InputAction::Switch(delta) => (ACTION_SWITCH, delta as isize),
        InputAction::Navigate(delta) => (ACTION_NAVIGATE, delta as isize),
        InputAction::ActivateSelected => (ACTION_ACTIVATE_SELECTED, 0),
        InputAction::SelectFirst => (ACTION_SELECT_FIRST, 0),
        InputAction::SelectLast => (ACTION_SELECT_LAST, 0),
        InputAction::DismissOverlay => (ACTION_DISMISS_OVERLAY, 0),
        InputAction::CloseSelected => (ACTION_CLOSE_SELECTED, 0),
        InputAction::WindowCommand(command) => {
            (ACTION_WINDOW_COMMAND, isize::from(command.function_key()))
        }
        InputAction::ActivateVisiblePosition(position) => (
            ACTION_ACTIVATE_POSITION,
            isize::try_from(position).unwrap_or_default(),
        ),
        InputAction::AltReleased => (ACTION_ALT_RELEASED, 0),
        InputAction::RightButtonPressed => (ACTION_RIGHT_BUTTON_PRESSED, 0),
        InputAction::RightButtonReleased => (ACTION_RIGHT_BUTTON_RELEASED, 0),
        InputAction::MouseWheel(delta) => (ACTION_MOUSE_WHEEL, delta as isize),
        InputAction::AppendSearchCharacter(character) => (
            ACTION_APPEND_SEARCH_CHARACTER,
            isize::try_from(u32::from(character)).unwrap_or_default(),
        ),
        InputAction::BackspaceSearch => (ACTION_BACKSPACE_SEARCH, 0),
    };
    unsafe {
        // SAFETY: `target` is the UI HWND supplied at hook creation; PostMessageW copies the two
        // integer payloads and retains no Rust references.
        PostMessageW(
            Some(target),
            message,
            action_wparam(code, generation),
            LPARAM(value),
        )
    }
    .is_ok()
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

fn replayed_key_event_to_input(event: ReplayedKeyEvent) -> INPUT {
    let virtual_key = event.virtual_key();
    let mut flags = if is_extended_virtual_key(virtual_key) {
        KEYEVENTF_EXTENDEDKEY
    } else {
        KEYBD_EVENT_FLAGS::default()
    };
    if event.transition == KeyTransition::Released {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(virtual_key),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: REPLAYED_INPUT_MARKER,
            },
        },
    }
}

fn replayed_mouse_event_to_input(event: ReplayedMouseEvent) -> INPUT {
    let flags = match event {
        ReplayedMouseEvent::RightButtonReleased => MOUSEEVENTF_RIGHTUP,
    };
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: REPLAYED_INPUT_MARKER,
            },
        },
    }
}

fn replay_key_events(events: [Option<ReplayedKeyEvent>; 3], outcome: &mut HookOutcome) -> bool {
    replay_key_events_with(events, outcome, |inputs, input_size| unsafe {
        // SAFETY: `replay_key_events_with` passes only its initialized INPUT prefix, and
        // SendInput copies the records synchronously without retaining the borrowed slice.
        SendInput(inputs, input_size)
    })
}

/// Called on the UI thread before opening the overlay, never from an input callback.
pub fn send_shell_escape() -> Result<(), String> {
    send_shell_escape_with(
        |key| key_pressed(key.0),
        |inputs, input_size| unsafe {
            // SAFETY: the replay helper supplies initialized INPUT records and SendInput copies
            // them synchronously without retaining the borrowed slice.
            SendInput(inputs, input_size)
        },
    )
}

fn send_shell_escape_with(
    mut is_down: impl FnMut(VIRTUAL_KEY) -> bool,
    sender: impl FnMut(&[INPUT], i32) -> u32,
) -> Result<(), String> {
    // Do not release a modifier forwarded to another application or send its Escape shortcut.
    if [VK_MENU, VK_CONTROL, VK_LWIN, VK_RWIN]
        .into_iter()
        .any(&mut is_down)
    {
        return Err("Cannot dismiss Start while a system modifier is forwarded".to_owned());
    }
    let events = [
        // Suppressing Alt can leave LLKHF_ALTDOWN set even while async state reports up.
        // Clear that context on both sides before Escape. Our tagged releases bypass physical
        // gesture tracking, so held-Alt cycling and the eventual real release remain intact.
        Some(ReplayedKeyEvent::released(Key::LeftAlt)),
        Some(ReplayedKeyEvent::released(Key::RightAlt)),
        Some(ReplayedKeyEvent::pressed(Key::Escape)),
        Some(ReplayedKeyEvent::released(Key::Escape)),
    ];
    if replay_key_events_with(events, &mut HookOutcome::default(), sender) {
        Ok(())
    } else {
        Err("Could not send Escape to the Start/Search menu".to_owned())
    }
}

fn replay_key_events_with<const N: usize>(
    events: [Option<ReplayedKeyEvent>; N],
    outcome: &mut HookOutcome,
    mut sender: impl FnMut(&[INPUT], i32) -> u32,
) -> bool {
    let mut inputs = [INPUT::default(); N];
    let mut input_count = 0;
    for event in events.into_iter().flatten() {
        let Some(slot) = inputs.get_mut(input_count) else {
            return false;
        };
        *slot = replayed_key_event_to_input(event);
        input_count += 1;
    }
    if input_count == 0 {
        return true;
    }

    let Ok(input_size) = i32::try_from(core::mem::size_of::<INPUT>()) else {
        return false;
    };
    let Some(inputs) = inputs.get(..input_count) else {
        return false;
    };
    let mut sent = 0;
    while sent < inputs.len() {
        let remaining = &inputs[sent..];
        let Some(inserted) = usize::try_from(sender(remaining, input_size)).ok() else {
            outcome.suppress = false;
            return false;
        };
        if inserted == 0 || inserted > remaining.len() {
            outcome.suppress = false;
            return false;
        }
        sent += inserted;
    }
    true
}

fn replay_mouse_event(event: Option<ReplayedMouseEvent>, outcome: &mut HookOutcome) -> bool {
    replay_mouse_event_with(event, outcome, |inputs, input_size| unsafe {
        // SAFETY: `replay_mouse_event_with` passes only initialized INPUT records, and
        // SendInput copies the records synchronously without retaining the borrowed slice.
        SendInput(inputs, input_size)
    })
}

fn replay_mouse_event_with(
    event: Option<ReplayedMouseEvent>,
    outcome: &mut HookOutcome,
    sender: impl FnOnce(&[INPUT], i32) -> u32,
) -> bool {
    let Some(event) = event else {
        return true;
    };
    let inputs = [replayed_mouse_event_to_input(event)];
    let Ok(input_size) = i32::try_from(core::mem::size_of::<INPUT>()) else {
        outcome.suppress = false;
        return false;
    };
    if sender(&inputs, input_size) != 1 {
        outcome.suppress = false;
        return false;
    }
    true
}

const fn is_own_replayed_input(extra_info: usize) -> bool {
    extra_info == REPLAYED_INPUT_MARKER
}

const fn is_extended_virtual_key(virtual_key: u16) -> bool {
    matches!(
        virtual_key,
        0x21..=0x28 | 0x2D..=0x2E | 0x5B..=0x5D | 0x6F | 0x90 | 0xA3 | 0xA5
    )
}

struct KeyboardState {
    keys: [u8; 256],
    forwarded_toggle_keys: u8,
    modifier_observations: [ModifierObservation; 8],
}

const MODIFIER_KEYS: [VIRTUAL_KEY; 8] = [
    VK_LSHIFT,
    VK_RSHIFT,
    VK_LCONTROL,
    VK_RCONTROL,
    VK_LMENU,
    VK_RMENU,
    VK_LWIN,
    VK_RWIN,
];

#[derive(Clone, Copy, Default, Eq, PartialEq)]
struct ModifierObservation {
    time: Option<u32>,
    sequence: u64,
}

const fn timestamp_at_or_after(time: u32, boundary: u32) -> bool {
    time.wrapping_sub(boundary) < (1 << 31)
}

impl Default for KeyboardState {
    fn default() -> Self {
        Self {
            keys: [0; 256],
            forwarded_toggle_keys: 0,
            modifier_observations: [ModifierObservation::default(); 8],
        }
    }
}

impl KeyboardState {
    fn snapshot() -> Result<Self, String> {
        let mut keys = [0; 256];
        unsafe {
            // SAFETY: the complete fixed-size buffer is writable. This runs on the owning UI
            // thread before hooks start, so toggle bits are seeded from its input queue.
            GetKeyboardState(&mut keys)
        }
        .map_err(|error| format!("Could not read initial keyboard state: {error}"))?;
        for key in 0_u16..=255 {
            keys[usize::from(key)] =
                (keys[usize::from(key)] & 1) | if key_pressed(key) { 0x80 } else { 0 };
        }
        let forwarded_toggle_keys = [VK_CAPITAL, VK_NUMLOCK, VK_SCROLL]
            .into_iter()
            .enumerate()
            .fold(0, |mask, (index, key)| {
                mask | if keys[usize::from(key.0)] & 0x80 != 0 {
                    1 << index
                } else {
                    0
                }
            });
        Ok(Self {
            keys,
            forwarded_toggle_keys,
            ..Self::default()
        })
    }

    fn observe_at(&mut self, virtual_key: u32, transition: KeyTransition, time: u32) {
        self.observe(virtual_key, transition);
        if let Some(index) = MODIFIER_KEYS
            .into_iter()
            .position(|key| u32::from(key.0) == virtual_key)
        {
            let observation = &mut self.modifier_observations[index];
            observation.time = Some(time);
            observation.sequence = observation.sequence.wrapping_add(1);
        }
    }

    fn rebase_modifiers(
        &mut self,
        boundary: u32,
        before: [ModifierObservation; 8],
        down: [bool; 8],
        state: &mut HookState,
    ) {
        state.reset_gestures();
        for (index, key) in MODIFIER_KEYS.into_iter().enumerate() {
            let observed = self.modifier_observations[index];
            if observed.sequence != before[index].sequence
                || observed
                    .time
                    .is_some_and(|time| timestamp_at_or_after(time, boundary))
            {
                continue;
            }
            self.keys[usize::from(key.0)] = if down[index] { 0x80 } else { 0 };
            state.rebase_modifier(decode_virtual_key(u32::from(key.0)), down[index]);
        }
        for (aggregate, left, right) in [
            (VK_SHIFT, VK_LSHIFT, VK_RSHIFT),
            (VK_CONTROL, VK_LCONTROL, VK_RCONTROL),
            (VK_MENU, VK_LMENU, VK_RMENU),
        ] {
            self.keys[usize::from(aggregate.0)] =
                (self.keys[usize::from(left.0)] | self.keys[usize::from(right.0)]) & 0x80;
        }
    }

    fn observe(&mut self, virtual_key: u32, transition: KeyTransition) {
        let Some(state) = usize::try_from(virtual_key)
            .ok()
            .and_then(|key| self.keys.get_mut(key))
        else {
            return;
        };
        let pressed = transition == KeyTransition::Pressed;
        *state = (*state & 1) | if pressed { 0x80 } else { 0 };
        for (aggregate, left, right) in [
            (VK_SHIFT, VK_LSHIFT, VK_RSHIFT),
            (VK_CONTROL, VK_LCONTROL, VK_RCONTROL),
            (VK_MENU, VK_LMENU, VK_RMENU),
        ] {
            if virtual_key == u32::from(left.0) || virtual_key == u32::from(right.0) {
                self.keys[usize::from(aggregate.0)] =
                    (self.keys[usize::from(left.0)] | self.keys[usize::from(right.0)]) & 0x80;
            }
        }
    }

    fn commit_forwarded(&mut self, virtual_key: u32, transition: KeyTransition) {
        let Some(index) = [VK_CAPITAL, VK_NUMLOCK, VK_SCROLL]
            .into_iter()
            .position(|key| u32::from(key.0) == virtual_key)
        else {
            return;
        };
        let mask = 1 << index;
        if transition == KeyTransition::Pressed {
            if self.forwarded_toggle_keys & mask == 0 {
                self.keys[virtual_key as usize] ^= 1;
            }
            self.forwarded_toggle_keys |= mask;
        } else {
            self.forwarded_toggle_keys &= !mask;
        }
    }

    fn search_state(&self) -> [u8; 256] {
        let mut keys = self.keys;
        // Alt belongs to the switch gesture, not the user's search text.
        for key in [VK_MENU, VK_LMENU, VK_RMENU] {
            keys[usize::from(key.0)] = 0;
        }
        keys
    }
}

fn translate_search_character(
    data: &KBDLLHOOKSTRUCT,
    target_thread_id: u32,
    observed: &KeyboardState,
) -> Option<char> {
    let keyboard_state = observed.search_state();
    let mut text = [0_u16; 4];
    let count = unsafe {
        // SAFETY: both buffers are initialized for their full lengths; the keyboard layout is
        // read from the overlay UI thread and flag 4 prevents mutation of the dead-key state.
        ToUnicodeEx(
            data.vkCode,
            data.scanCode,
            &keyboard_state,
            &mut text,
            4,
            Some(GetKeyboardLayout(target_thread_id)),
        )
    };
    let count = usize::try_from(count).ok()?;
    let mut characters = char::decode_utf16(text.get(..count)?.iter().copied());
    let character = characters.next()?.ok()?;
    characters.next().is_none().then_some(character)
}

pub(crate) fn decode_virtual_key(virtual_key: u32) -> Key {
    match virtual_key {
        value if value == u32::from(VK_TAB.0) => Key::Tab,
        value if value == u32::from(VK_RETURN.0) => Key::Enter,
        value if value == u32::from(VK_HOME.0) => Key::Home,
        value if value == u32::from(VK_END.0) => Key::End,
        value if value == u32::from(VK_ESCAPE.0) => Key::Escape,
        value if value == u32::from(VK_F4.0) => Key::F4,
        value if value == u32::from(VK_F5.0) => Key::Function(5),
        value if value == u32::from(VK_F6.0) => Key::Function(6),
        value if value == u32::from(VK_F7.0) => Key::Function(7),
        value if value == u32::from(VK_F8.0) => Key::Function(8),
        value if value == u32::from(VK_F9.0) => Key::Function(9),
        value if value == u32::from(VK_MENU.0) => Key::Alt,
        value if value == u32::from(VK_LMENU.0) => Key::LeftAlt,
        value if value == u32::from(VK_RMENU.0) => Key::RightAlt,
        value if value == u32::from(VK_LWIN.0) => Key::LeftWindows,
        value if value == u32::from(VK_RWIN.0) => Key::RightWindows,
        value if value == u32::from(VK_CONTROL.0) => Key::Control,
        value if value == u32::from(VK_LCONTROL.0) => Key::LeftControl,
        value if value == u32::from(VK_RCONTROL.0) => Key::RightControl,
        value if value == u32::from(VK_LSHIFT.0) => Key::LeftShift,
        value if value == u32::from(VK_RSHIFT.0) => Key::RightShift,
        value if value == u32::from(VK_SNAPSHOT.0) => Key::PrintScreen,
        value if value == u32::from(VK_BACK.0) => Key::Backspace,
        value if value == u32::from(VK_LEFT.0) => Key::LeftArrow,
        value if value == u32::from(VK_UP.0) => Key::UpArrow,
        value if value == u32::from(VK_RIGHT.0) => Key::RightArrow,
        value if value == u32::from(VK_DOWN.0) => Key::DownArrow,
        value if value >= u32::from(VK_1.0) && value <= u32::from(VK_9.0) => {
            Key::Digit(u8::try_from(value - u32::from(VK_0.0)).unwrap_or_default())
        }
        value if value >= u32::from(VK_NUMPAD1.0) && value <= u32::from(VK_NUMPAD9.0) => {
            Key::NumpadDigit(u8::try_from(value - u32::from(VK_NUMPAD0.0)).unwrap_or_default())
        }
        value => Key::Other(u16::try_from(value).unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn physical_hotkey_tab_release_is_balanced_when_a_modal_cancels_the_gesture() {
        let mut context = test_context();
        let modifiers = Modifiers::default();
        let _ = context
            .state
            .process_key(KeyEvent::pressed(Key::LeftAlt, modifiers), context.settings);
        let _ = context
            .state
            .process_key(KeyEvent::pressed(Key::Tab, modifiers), context.settings);
        context.registered_tab_down = true;
        context.state.set_interception_suspended(true);
        for (key, vk, expected_suppression) in
            [(Key::Tab, VK_TAB, false), (Key::LeftAlt, VK_LMENU, true)]
        {
            let outcome = context
                .state
                .process_key(KeyEvent::released(key, modifiers), context.settings);
            let data = KBDLLHOOKSTRUCT {
                vkCode: u32::from(vk.0),
                ..KBDLLHOOKSTRUCT::default()
            };
            let routed =
                route_registered_switch(&mut context, outcome, &data, KeyTransition::Released);
            assert_eq!(routed.suppress, expected_suppression);
            assert!(routed.actions().next().is_none());
        }
        assert!(!context.registered_tab_down);
    }

    #[test]
    fn queued_destructive_action_cannot_cross_a_menu_or_hook_restart() {
        let flags = Arc::new(HookFlags::new());
        let stale = action_wparam(ACTION_WINDOW_COMMAND, flags.load());
        assert_eq!(
            decode_action(stale, LPARAM(8)),
            Some(InputAction::WindowCommand(
                alttabio::input::WindowCommand::Terminate
            ))
        );
        assert!(flags.action_is_current(stale));
        {
            let _menu = HookInterceptionGuard::new(Arc::clone(&flags));
            assert!(!flags.action_is_current(stale));
        }
        assert!(!flags.action_is_current(stale));
        let current = action_wparam(ACTION_WINDOW_COMMAND, flags.load());
        assert!(flags.action_is_current(current));
        assert!(!HookFlags::new().action_is_current(current));
    }

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
    fn desktop_rebase_clears_old_modifiers_but_preserves_fresh_suppressed_shift() {
        for fresh in [false, true] {
            let mut context = test_context();
            let alt = Modifiers {
                alt: true,
                ..Modifiers::default()
            };
            let _tab = context
                .state
                .process_key(KeyEvent::pressed(Key::Tab, alt), context.settings);
            let _tab_up = context
                .state
                .process_key(KeyEvent::released(Key::Tab, alt), context.settings);
            let shift = context
                .state
                .process_key(KeyEvent::pressed(Key::LeftShift, alt), context.settings);
            assert!(shift.suppress);
            context.keyboard_state.observe_at(
                u32::from(VK_LSHIFT.0),
                KeyTransition::Pressed,
                if fresh { 310 } else { 100 },
            );
            let before = context.keyboard_state.modifier_observations;
            context
                .keyboard_state
                .rebase_modifiers(300, before, [false; 8], &mut context.state);
            assert_eq!(
                context.keyboard_state.keys[usize::from(VK_SHIFT.0)] != 0,
                fresh
            );
            let direction = context
                .state
                .process_key(KeyEvent::pressed(Key::Tab, alt), context.settings);
            assert_eq!(
                direction.actions().next(),
                Some(InputAction::Switch(if fresh { -1 } else { 1 }))
            );
            let release = context
                .state
                .process_key(KeyEvent::released(Key::LeftShift, alt), context.settings);
            assert!(release.suppress);
        }
    }

    #[test]
    fn desktop_rebase_preserves_observations_during_snapshot_and_timestamp_wrap() {
        let mut context = test_context();
        context
            .keyboard_state
            .observe_at(u32::from(VK_LSHIFT.0), KeyTransition::Pressed, 100);
        let before = context.keyboard_state.modifier_observations;
        context
            .keyboard_state
            .observe_at(u32::from(VK_LSHIFT.0), KeyTransition::Released, 400);
        context
            .keyboard_state
            .observe_at(u32::from(VK_RSHIFT.0), KeyTransition::Pressed, 20);
        context.keyboard_state.rebase_modifiers(
            u32::MAX - 10,
            before,
            [true; 8],
            &mut context.state,
        );
        assert_eq!(context.keyboard_state.keys[usize::from(VK_LSHIFT.0)], 0);
        assert_eq!(context.keyboard_state.keys[usize::from(VK_RSHIFT.0)], 0x80);
        assert!(timestamp_at_or_after(20, u32::MAX - 10));
        assert!(!timestamp_at_or_after(u32::MAX - 10, 20));
    }

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
                process_with_context(|context| context
                    .flags
                    .recovery_pending
                    .load(Ordering::Acquire)),
                Some(true)
            );
        }
        recover_keyboard_state_with(400, || Ok(true), || [false; 8]);
        assert_eq!(DESKTOP_BOUNDARY.get(), None);
        assert_eq!(
            process_with_context(|context| context.flags.recovery_pending.load(Ordering::Acquire)),
            Some(false)
        );
        CONTEXT.with(|slot| *slot.borrow_mut() = None);
    }

    #[test]
    fn action_epoch_encoding_and_validation_use_the_same_wrap_mask() {
        for generation in [0, 8, ACTION_EPOCH_MASK << 3, usize::MAX & !7] {
            let flags = HookFlags::with_value(generation);
            let action = action_wparam(ACTION_CLOSE_SELECTED, generation);
            assert_eq!(action.0 & ACTION_CODE_MASK, ACTION_CLOSE_SELECTED);
            assert_eq!(action.0 >> 8, (generation >> 3) & ACTION_EPOCH_MASK);
            assert!(flags.action_is_current(action));
            assert_eq!(
                decode_action(action, LPARAM(0)),
                Some(InputAction::CloseSelected)
            );
        }
    }

    #[test]
    fn scoped_suspension_restores_every_flag_combination_with_new_generations() {
        for saved_flags in 0..=7 {
            let flags = Arc::new(HookFlags::with_value(saved_flags));
            let initial_generation = flags.load() & !7;
            let guard = HookInterceptionGuard::new(Arc::clone(&flags));
            let suspended = flags.load();
            assert_eq!(suspended & 7, INTERCEPTION_SUSPENDED);
            assert_ne!(suspended & !7, initial_generation);
            assert!(!flags.update_overlay(initial_generation, true, true));
            flags.set(SEARCH_ACTIVE, true);
            flags.set(OVERLAY_ACTIVE, true);
            drop(guard);
            assert_eq!(flags.load() & 7, saved_flags);
            assert_ne!(flags.load() & !7, suspended & !7);
            assert!(!flags.update_overlay(suspended & !OVERLAY_FLAGS, true, true));
        }
    }

    #[test]
    fn nested_scoped_suspension_restores_outer_state_without_resuming_it() {
        let flags = Arc::new(HookFlags::with_value(OVERLAY_FLAGS));
        let outer = HookInterceptionGuard::new(Arc::clone(&flags));
        flags.set(SEARCH_ACTIVE, true);
        {
            let _inner = HookInterceptionGuard::new(Arc::clone(&flags));
            assert_eq!(flags.load() & 7, INTERCEPTION_SUSPENDED);
            flags.set(OVERLAY_ACTIVE, true);
        }
        assert_eq!(flags.load() & 7, INTERCEPTION_SUSPENDED | SEARCH_ACTIVE);
        drop(outer);
        assert_eq!(flags.load() & 7, OVERLAY_FLAGS);
    }

    #[test]
    fn scoped_suspension_restores_flags_during_unwinding() {
        let flags = Arc::new(HookFlags::with_value(OVERLAY_FLAGS));
        let result = std::panic::catch_unwind(|| {
            let _guard = HookInterceptionGuard::new(Arc::clone(&flags));
            panic!("modal call failed");
        });
        assert!(result.is_err());
        assert_eq!(flags.load() & 7, OVERLAY_FLAGS);
    }

    #[test]
    fn modal_boundaries_prevent_stale_flag_publication_and_preserve_restored_flags() {
        let mut context = test_context();
        let flags = Arc::clone(&context.flags);
        let outcome = context.state.process_key(
            KeyEvent::pressed(
                Key::Tab,
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
            ),
            context.settings,
        );
        flags.suspend(true);
        assert!(!flags.update_overlay(0, true, true));
        assert!(post_context_actions(&mut context, outcome, |_, _| panic!(
            "suspended action was posted"
        )));
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
        flags.set(OVERLAY_ACTIVE, true);
        flags.set(SEARCH_ACTIVE, true);
        flags.suspend(false);
        assert_eq!(flags.load() & OVERLAY_FLAGS, OVERLAY_FLAGS);
        // The entire modal lifetime occurred between callbacks; its generation still cancels Tab.
        assert_eq!(context.sync_interception(), OVERLAY_FLAGS);
        let release = context.state.process_key(
            KeyEvent::released(Key::Alt, Modifiers::default()),
            context.settings,
        );
        assert!(release.actions().next().is_none());
        assert!(!flags.update_overlay(0, false, false));
        assert_eq!(flags.load() & OVERLAY_FLAGS, OVERLAY_FLAGS);
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
    fn suspension_during_mouse_action_delivery_stops_remaining_actions() {
        let mut context = test_context();
        let flags = Arc::clone(&context.flags);
        let _down = context
            .state
            .process_mouse(MouseEvent::RightButtonPressed, context.settings);
        let outcome = context
            .state
            .process_mouse(MouseEvent::Wheel(120), context.settings);
        let mut posts = 0;
        assert!(post_context_actions(&mut context, outcome, |_, action| {
            posts += 1;
            assert_eq!(action, InputAction::RightButtonPressed);
            flags.suspend(true);
            true
        }));
        assert_eq!(posts, 1);
        assert_eq!(flags.load() & OVERLAY_FLAGS, 0);
        assert_eq!(context.sync_interception(), 0);
        let release = context
            .state
            .process_mouse(MouseEvent::RightButtonReleased, context.settings);
        assert!(release.suppress);
        assert!(release.actions().next().is_none());
    }

    #[test]
    fn observed_modifiers_combine_both_sides_and_toggle_only_on_first_press() {
        let mut state = KeyboardState::default();
        for (aggregate, left, right) in [
            (VK_SHIFT, VK_LSHIFT, VK_RSHIFT),
            (VK_CONTROL, VK_LCONTROL, VK_RCONTROL),
            (VK_MENU, VK_LMENU, VK_RMENU),
        ] {
            state.observe(u32::from(left.0), KeyTransition::Pressed);
            state.observe(u32::from(right.0), KeyTransition::Pressed);
            state.observe(u32::from(left.0), KeyTransition::Released);
            assert_eq!(state.keys[usize::from(aggregate.0)], 0x80);
            state.observe(u32::from(right.0), KeyTransition::Released);
            assert_eq!(state.keys[usize::from(aggregate.0)], 0);
        }
        for key in [VK_CAPITAL, VK_NUMLOCK, VK_SCROLL] {
            state.observe(u32::from(key.0), KeyTransition::Pressed);
            state.commit_forwarded(u32::from(key.0), KeyTransition::Pressed);
            state.observe(u32::from(key.0), KeyTransition::Pressed);
            state.commit_forwarded(u32::from(key.0), KeyTransition::Pressed);
            assert_eq!(state.keys[usize::from(key.0)], 0x81);
            state.observe(u32::from(key.0), KeyTransition::Released);
            state.commit_forwarded(u32::from(key.0), KeyTransition::Released);
            assert_eq!(state.keys[usize::from(key.0)], 1);
            state.observe(u32::from(key.0), KeyTransition::Pressed);
            state.commit_forwarded(u32::from(key.0), KeyTransition::Pressed);
            assert_eq!(state.keys[usize::from(key.0)], 0x80);
        }
    }

    #[test]
    fn translated_punctuation_uses_observed_shift_including_suppressed_presses() {
        let data = KBDLLHOOKSTRUCT {
            vkCode: u32::from(VK_1.0),
            ..KBDLLHOOKSTRUCT::default()
        };
        let mut state = KeyboardState::default();
        state.observe(data.vkCode, KeyTransition::Pressed);
        let plain = translate_search_character(&data, 0, &state);
        let mut expected = KeyboardState::default();
        expected.keys[usize::from(VK_SHIFT.0)] = 0x80;
        expected.keys[usize::from(VK_LSHIFT.0)] = 0x80;
        expected.keys[usize::from(VK_1.0)] = 0x80;
        let shifted = translate_search_character(&data, 0, &expected);
        assert!(shifted.is_some());
        assert_ne!(plain, shifted);
        state.observe(u32::from(VK_LSHIFT.0), KeyTransition::Pressed);
        state.observe(u32::from(VK_LMENU.0), KeyTransition::Pressed);
        assert_eq!(translate_search_character(&data, 0, &state), shifted);
        state.observe(data.vkCode, KeyTransition::Pressed);
        assert_eq!(translate_search_character(&data, 0, &state), shifted);
        state.observe(u32::from(VK_LSHIFT.0), KeyTransition::Released);
        assert_eq!(translate_search_character(&data, 0, &state), plain);
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
    fn arrow_virtual_keys_map_to_directional_hook_keys() {
        assert_eq!(decode_virtual_key(u32::from(VK_LEFT.0)), Key::LeftArrow);
        assert_eq!(decode_virtual_key(u32::from(VK_UP.0)), Key::UpArrow);
        assert_eq!(decode_virtual_key(u32::from(VK_RIGHT.0)), Key::RightArrow);
        assert_eq!(decode_virtual_key(u32::from(VK_DOWN.0)), Key::DownArrow);
    }

    #[test]
    fn windows_virtual_keys_preserve_their_physical_side() {
        assert_eq!(decode_virtual_key(u32::from(VK_LWIN.0)), Key::LeftWindows);
        assert_eq!(decode_virtual_key(u32::from(VK_RWIN.0)), Key::RightWindows);
    }

    #[test]
    fn shell_escape_clears_suppressed_alt_context_before_escape() {
        for held_alt in [VK_LMENU, VK_RMENU] {
            // Captured failure: GetAsyncKeyState permitted dismissal, but both injected Escape
            // events carried LLKHF_ALTDOWN and Start retained focus until the 500 ms timeout.
            let mut alt_context = true;
            let mut escapes = Vec::new();
            assert!(
                send_shell_escape_with(
                    |_| false,
                    |inputs, _| {
                        for input in inputs {
                            assert_eq!(input.r#type, INPUT_KEYBOARD);
                            // SAFETY: the replay helper initializes the keyboard union member.
                            let keyboard = unsafe { input.Anonymous.ki };
                            assert_eq!(keyboard.dwExtraInfo, REPLAYED_INPUT_MARKER);
                            if keyboard.wVk == held_alt
                                && keyboard.dwFlags.contains(KEYEVENTF_KEYUP)
                            {
                                alt_context = false;
                            }
                            if keyboard.wVk == VK_ESCAPE {
                                escapes.push((
                                    keyboard.dwFlags.contains(KEYEVENTF_KEYUP),
                                    alt_context,
                                ));
                            }
                        }
                        u32::try_from(inputs.len()).unwrap_or_default()
                    },
                )
                .is_ok()
            );
            assert_eq!(
                escapes,
                [(false, false), (true, false)],
                "Start must receive plain Escape, even when suppressed Alt is absent from async state"
            );
        }
    }

    #[test]
    fn shell_escape_preserves_forwarded_system_modifiers() {
        for held in [VK_MENU, VK_CONTROL, VK_LWIN, VK_RWIN] {
            assert!(
                send_shell_escape_with(
                    |key| key == held,
                    |_, _| panic!("A forwarded modifier must prevent all injected input"),
                )
                .is_err()
            );
        }
    }

    #[test]
    fn shell_escape_keeps_the_next_gesture_after_start_interrupts_alt_tab() {
        for alt in [Key::LeftAlt, Key::RightAlt] {
            let mut context = test_context();
            let modifiers = Modifiers::default();
            for event in [
                KeyEvent::pressed(alt, modifiers),
                KeyEvent::pressed(Key::Tab, modifiers),
                KeyEvent::released(Key::Tab, modifiers),
                KeyEvent::pressed(Key::LeftWindows, modifiers),
                KeyEvent::released(Key::LeftWindows, modifiers),
                KeyEvent::pressed(Key::Tab, modifiers),
                KeyEvent::released(Key::Tab, modifiers),
                KeyEvent::released(alt, modifiers),
                KeyEvent::pressed(alt, modifiers),
                KeyEvent::pressed(Key::Tab, modifiers),
                KeyEvent::released(Key::Tab, modifiers),
            ] {
                let _outcome = context.state.process_key(event, context.settings);
            }
            CONTEXT.with(|slot| *slot.borrow_mut() = Some(context));
            let result = send_shell_escape_with(
                |_| false,
                |inputs, _| {
                    for input in inputs {
                        // SAFETY: send_shell_escape_with only constructs keyboard INPUT records.
                        let keyboard = unsafe { input.Anonymous.ki };
                        let data = KBDLLHOOKSTRUCT {
                            vkCode: u32::from(keyboard.wVk.0),
                            dwExtraInfo: keyboard.dwExtraInfo,
                            ..KBDLLHOOKSTRUCT::default()
                        };
                        let message = if keyboard.dwFlags.contains(KEYEVENTF_KEYUP) {
                            WM_KEYUP
                        } else {
                            WM_KEYDOWN
                        };
                        // Pass every injected release and Escape through the real adapter.
                        assert_eq!(
                            process_keyboard_message(
                                WPARAM(message as usize),
                                LPARAM((&raw const data) as isize)
                            ),
                            None
                        );
                    }
                    u32::try_from(inputs.len()).unwrap_or_default()
                },
            );
            let Some(mut context) = CONTEXT.with(|slot| slot.borrow_mut().take()) else {
                panic!("test context disappeared");
            };
            assert!(result.is_ok());
            for delta in [1, -1] {
                if delta == -1 {
                    let _shift = context.state.process_key(
                        KeyEvent::pressed(Key::LeftShift, modifiers),
                        context.settings,
                    );
                }
                let outcome = context
                    .state
                    .process_key(KeyEvent::pressed(Key::Tab, modifiers), context.settings);
                assert_eq!(outcome.actions().next(), Some(InputAction::Switch(delta)));
                assert!(outcome.suppress);
                let _release = context
                    .state
                    .process_key(KeyEvent::released(Key::Tab, modifiers), context.settings);
            }
            let release = context
                .state
                .process_key(KeyEvent::released(alt, modifiers), context.settings);
            assert!(release.suppress);
            assert_eq!(release.actions().next(), Some(InputAction::AltReleased));
        }
    }

    #[test]
    fn replayed_windows_events_are_tagged_extended_keyboard_input() {
        for (event, expected_key_up) in [
            (ReplayedKeyEvent::pressed(Key::LeftWindows), false),
            (ReplayedKeyEvent::released(Key::RightWindows), true),
        ] {
            let input = replayed_key_event_to_input(event);
            assert_eq!(input.r#type, INPUT_KEYBOARD);
            let keyboard = unsafe {
                // SAFETY: `replayed_key_event_to_input` initializes the keyboard union member and
                // marks the enclosing INPUT as INPUT_KEYBOARD.
                input.Anonymous.ki
            };
            assert_eq!(keyboard.wVk.0, event.virtual_key());
            assert!(keyboard.dwFlags.contains(KEYEVENTF_EXTENDEDKEY));
            assert_eq!(keyboard.dwFlags.contains(KEYEVENTF_KEYUP), expected_key_up);
            assert_eq!(keyboard.dwExtraInfo, REPLAYED_INPUT_MARKER);
        }
    }

    #[test]
    fn replayed_right_button_release_is_tagged_mouse_input() {
        let input = replayed_mouse_event_to_input(ReplayedMouseEvent::RightButtonReleased);
        assert_eq!(input.r#type, INPUT_MOUSE);
        let mouse = unsafe {
            // SAFETY: `replayed_mouse_event_to_input` initializes the mouse union member and marks
            // the enclosing INPUT as INPUT_MOUSE.
            input.Anonymous.mi
        };
        assert_eq!(mouse.dwFlags, MOUSEEVENTF_RIGHTUP);
        assert_eq!(mouse.dwExtraInfo, REPLAYED_INPUT_MARKER);
    }

    #[test]
    fn failed_right_button_release_replay_forwards_the_wheel_and_resets_suppression() {
        let mut outcome = HookOutcome::default();
        outcome.suppress = true;
        let replayed = replay_mouse_event_with(
            Some(ReplayedMouseEvent::RightButtonReleased),
            &mut outcome,
            |inputs, input_size| {
                assert_eq!(inputs.len(), 1);
                assert_eq!(
                    input_size,
                    i32::try_from(core::mem::size_of::<INPUT>()).unwrap_or_default()
                );
                0
            },
        );

        assert!(!replayed);
        assert!(!outcome.suppress);
    }

    #[test]
    fn only_alttabios_exact_replay_marker_bypasses_hook_processing() {
        assert!(is_own_replayed_input(REPLAYED_INPUT_MARKER));
        assert!(!is_own_replayed_input(0));
        assert!(!is_own_replayed_input(REPLAYED_INPUT_MARKER + 1));
    }

    #[test]
    fn replay_retries_partial_send_input_and_releases_the_physical_fallback_on_failure() {
        let events = [
            Some(ReplayedKeyEvent::pressed(Key::LeftWindows)),
            Some(ReplayedKeyEvent::pressed(Key::Other(u16::from(b'R')))),
            None,
        ];
        let mut outcome = HookOutcome::default();
        outcome.suppress = true;
        let mut calls = 0;
        let replayed = replay_key_events_with(events, &mut outcome, |inputs, input_size| {
            calls += 1;
            assert_eq!(
                input_size,
                i32::try_from(core::mem::size_of::<INPUT>()).unwrap_or_default()
            );
            assert_eq!(inputs.len(), 3_usize.saturating_sub(calls));
            1
        });
        assert!(replayed);
        assert!(outcome.suppress);
        assert_eq!(calls, 2);

        let mut outcome = HookOutcome::default();
        outcome.suppress = true;
        let mut calls = 0;
        let replayed = replay_key_events_with(events, &mut outcome, |_, _| {
            calls += 1;
            u32::from(calls == 1)
        });
        assert!(!replayed);
        assert!(!outcome.suppress);
        assert_eq!(calls, 2);
    }

    #[test]
    fn posting_overlay_open_arms_typed_search_before_the_ui_acknowledges_visibility() {
        let flags = Arc::new(HookFlags::default());
        let mut context = HookContext {
            registered_switch: None,
            registered_tab_down: false,
            target: HWND::default(),
            state: HookState::default(),
            settings: HookSettings::default(),
            remote_desktop_passthrough: Arc::new(AtomicBool::new(false)),
            target_thread_id: 0,
            hook_thread_id: 0,
            pending_errors: 0,
            flags: Arc::clone(&flags),
            interception_generation: 0,
            keyboard_state: KeyboardState::default(),
            recovering: false,
        };
        let outcome = context.state.process_key(
            KeyEvent::pressed(
                Key::Tab,
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
            ),
            context.settings,
        );

        assert!(post_context_actions(&mut context, outcome, |_, _| true));
        assert_eq!(flags.load() & OVERLAY_FLAGS, OVERLAY_FLAGS);
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

    #[test]
    fn control_and_print_screen_virtual_keys_map_to_passthrough_hook_keys() {
        assert_eq!(decode_virtual_key(u32::from(VK_CONTROL.0)), Key::Control);
        assert_eq!(
            decode_virtual_key(u32::from(VK_LCONTROL.0)),
            Key::LeftControl
        );
        assert_eq!(
            decode_virtual_key(u32::from(VK_RCONTROL.0)),
            Key::RightControl
        );
        assert_eq!(
            decode_virtual_key(u32::from(VK_SNAPSHOT.0)),
            Key::PrintScreen
        );
    }

    #[test]
    fn navigation_actions_decode_in_both_directions() {
        assert_eq!(
            decode_action(WPARAM(ACTION_NAVIGATE), LPARAM(-1)),
            Some(InputAction::Navigate(-1))
        );
        assert_eq!(
            decode_action(WPARAM(ACTION_NAVIGATE), LPARAM(1)),
            Some(InputAction::Navigate(1))
        );
    }

    #[test]
    fn enter_virtual_key_and_activation_message_map_to_the_new_input_event() {
        assert_eq!(decode_virtual_key(u32::from(VK_RETURN.0)), Key::Enter);
        assert_eq!(
            decode_action(WPARAM(ACTION_ACTIVATE_SELECTED), LPARAM(0)),
            Some(InputAction::ActivateSelected)
        );
    }

    #[test]
    fn home_and_end_virtual_keys_map_to_boundary_keys() {
        assert_eq!(decode_virtual_key(u32::from(VK_HOME.0)), Key::Home);
        assert_eq!(decode_virtual_key(u32::from(VK_END.0)), Key::End);
    }

    #[test]
    fn boundary_actions_round_trip_through_hook_message_payloads() {
        assert_eq!(
            decode_action(WPARAM(ACTION_SELECT_FIRST), LPARAM(0)),
            Some(InputAction::SelectFirst)
        );
        assert_eq!(
            decode_action(WPARAM(ACTION_SELECT_LAST), LPARAM(0)),
            Some(InputAction::SelectLast)
        );
    }

    #[test]
    fn escape_virtual_key_maps_to_escape_input() {
        assert_eq!(decode_virtual_key(u32::from(VK_ESCAPE.0)), Key::Escape);
    }

    #[test]
    fn dismiss_overlay_action_decodes_for_the_ui_thread() {
        assert_eq!(
            decode_action(WPARAM(ACTION_DISMISS_OVERLAY), LPARAM(0)),
            Some(InputAction::DismissOverlay)
        );
    }

    #[test]
    fn f4_maps_to_close_selected_across_the_hook_message_boundary() {
        assert_eq!(decode_virtual_key(u32::from(VK_F4.0)), Key::F4);
        assert_eq!(
            decode_action(WPARAM(ACTION_CLOSE_SELECTED), LPARAM(0)),
            Some(InputAction::CloseSelected)
        );
    }

    #[test]
    fn f5_through_f9_map_to_semantic_function_keys() {
        for (virtual_key, function_key) in
            [(VK_F5, 5), (VK_F6, 6), (VK_F7, 7), (VK_F8, 8), (VK_F9, 9)]
        {
            assert_eq!(
                decode_virtual_key(u32::from(virtual_key.0)),
                Key::Function(function_key)
            );
        }
    }

    #[test]
    fn window_command_actions_round_trip_through_hook_message_payloads() {
        for command in [
            alttabio::input::WindowCommand::Minimize,
            alttabio::input::WindowCommand::Maximize,
            alttabio::input::WindowCommand::Restore,
            alttabio::input::WindowCommand::Terminate,
            alttabio::input::WindowCommand::Run,
        ] {
            assert_eq!(
                decode_action(
                    WPARAM(ACTION_WINDOW_COMMAND),
                    LPARAM(isize::from(command.function_key()))
                ),
                Some(InputAction::WindowCommand(command))
            );
        }
    }
}
