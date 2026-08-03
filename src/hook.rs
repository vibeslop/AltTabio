use alttabio::input::{
    HookOutcome, HookSettings, HookState, InputAction, Key, KeyEvent, KeyTransition, Modifiers,
    MouseEvent, ReplayedKeyEvent,
};
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
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyboardLayout, GetKeyboardState, INPUT, INPUT_0, INPUT_KEYBOARD,
    KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, SendInput, ToUnicodeEx,
    VIRTUAL_KEY, VK_0, VK_1, VK_9, VK_BACK, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_F4, VK_F5,
    VK_F6, VK_F7, VK_F8, VK_F9, VK_HOME, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN,
    VK_MENU, VK_NUMPAD0, VK_NUMPAD1, VK_NUMPAD9, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU,
    VK_RSHIFT, VK_RWIN, VK_SNAPSHOT, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, GetWindowThreadProcessId, HHOOK,
    KBDLLHOOKSTRUCT, LLKHF_ALTDOWN, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, PeekMessageW, PostMessageW,
    PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_APP, WM_KEYDOWN, WM_KEYUP, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows::core::Error;

pub const WM_HOOK_ACTION: u32 = WM_APP + 1;
const WM_RESET_GESTURES: u32 = WM_APP + 2;
const WM_REPORT_HOOK_ERRORS: u32 = WM_APP + 3;

const HOOK_ERROR_REPLAY_INPUT: u8 = 1;
const HOOK_ERROR_POST_ACTION: u8 = 2;

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

struct HookContext {
    target: HWND,
    state: HookState,
    settings: HookSettings,
    search_active: Arc<AtomicBool>,
    overlay_active: Arc<AtomicBool>,
    target_thread_id: u32,
    hook_thread_id: u32,
    pending_errors: u8,
}

impl HookContext {
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
    search_active: Arc<AtomicBool>,
    overlay_active: Arc<AtomicBool>,
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
        let search_active = Arc::new(AtomicBool::new(false));
        let hook_search_active = Arc::clone(&search_active);
        let overlay_active = Arc::new(AtomicBool::new(false));
        let hook_overlay_active = Arc::clone(&overlay_active);
        let (sender, receiver) = mpsc::sync_channel(1);
        let join_handle = thread::Builder::new()
            .name("alttabio-hooks".to_owned())
            .spawn(move || {
                let target = HWND(target_value as *mut core::ffi::c_void);
                if let Err(error) = run_hook_thread(
                    target,
                    target_thread_id,
                    settings,
                    hook_search_active,
                    hook_overlay_active,
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
                search_active,
                overlay_active,
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
        self.search_active.store(active, Ordering::Relaxed);
    }

    pub fn set_overlay_active(&self, active: bool) {
        self.overlay_active.store(active, Ordering::Release);
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
    match wparam.0 {
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
    target: HWND,
    target_thread_id: u32,
    settings: HookSettings,
    search_active: Arc<AtomicBool>,
    overlay_active: Arc<AtomicBool>,
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

    CONTEXT.with(|context| {
        *context.borrow_mut() = Some(HookContext {
            target,
            state: HookState::default(),
            settings,
            search_active,
            overlay_active,
            target_thread_id,
            hook_thread_id: thread_id,
            pending_errors: 0,
        });
    });

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
    CONTEXT.with(|context| *context.borrow_mut() = None);
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
        finish_callback(
            code,
            wparam,
            lparam,
            process_keyboard_message(wparam, lparam),
        )
    })
    .unwrap_or_else(|_| call_next(code, wparam, lparam))
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
        let search_active = context.search_active.load(Ordering::Relaxed);
        context
            .state
            .set_overlay_active(context.overlay_active.load(Ordering::Acquire));
        let mut settings = context.settings;
        settings.search_active = search_active;
        let text = if search_active && transition == KeyTransition::Pressed {
            translate_search_character(data, context.target_thread_id)
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
    Some(outcome)
}

fn process_mouse_message(wparam: WPARAM, lparam: LPARAM) -> Option<HookOutcome> {
    let event = match u32::try_from(wparam.0).ok()? {
        WM_RBUTTONDOWN => MouseEvent::RightButtonPressed,
        WM_RBUTTONUP => MouseEvent::RightButtonReleased,
        WM_MOUSEWHEEL => {
            let data = unsafe {
                // SAFETY: for a nonnegative low-level mouse hook code and WM_MOUSEWHEEL, Windows
                // guarantees lParam points to an MSLLHOOKSTRUCT for the callback duration.
                (lparam.0 as *const MSLLHOOKSTRUCT).as_ref()
            }?;
            MouseEvent::Wheel((data.mouseData >> 16) as i16)
        }
        _ => return None,
    };
    process_with_context(|context| {
        context
            .state
            .set_overlay_active(context.overlay_active.load(Ordering::Acquire));
        context.state.process_mouse(event, context.settings)
    })
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
            Some(post_context_actions(context, outcome, post_action))
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
        let opens_overlay = matches!(
            action,
            InputAction::Switch(_) | InputAction::RightButtonPressed
        );
        if opens_overlay {
            context.overlay_active.store(true, Ordering::Release);
            context
                .search_active
                .store(context.settings.typed_search, Ordering::Relaxed);
        }
        if !post(context.target, action) {
            context.record_error(HOOK_ERROR_POST_ACTION);
            if opens_overlay {
                context.overlay_active.store(false, Ordering::Release);
                context.search_active.store(false, Ordering::Relaxed);
            }
            return false;
        }
    }
    true
}

fn post_action(target: HWND, action: InputAction) -> bool {
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
        PostMessageW(Some(target), WM_HOOK_ACTION, WPARAM(code), LPARAM(value))
    }
    .is_ok()
}

fn reset_context() {
    let _outcome = process_with_context(|context| {
        context.state.reset_gestures();
        HookOutcome::default()
    });
}

fn record_callback_error(error: u8) {
    let _recorded = process_with_context(|context| context.record_error(error));
}

fn report_callback_errors() {
    let pending = process_with_context(|context| core::mem::take(&mut context.pending_errors))
        .unwrap_or_default();
    if pending & HOOK_ERROR_REPLAY_INPUT != 0 {
        eprintln!("Could not replay a Windows-key sequence from the input hook");
    }
    if pending & HOOK_ERROR_POST_ACTION != 0 {
        eprintln!("Could not post an input action from the input hook");
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

fn replay_key_events(events: [Option<ReplayedKeyEvent>; 3], outcome: &mut HookOutcome) -> bool {
    replay_key_events_with(events, outcome, |inputs, input_size| unsafe {
        // SAFETY: `replay_key_events_with` passes only its initialized INPUT prefix, and
        // SendInput copies the records synchronously without retaining the borrowed slice.
        SendInput(inputs, input_size)
    })
}

fn replay_key_events_with(
    events: [Option<ReplayedKeyEvent>; 3],
    outcome: &mut HookOutcome,
    mut sender: impl FnMut(&[INPUT], i32) -> u32,
) -> bool {
    let mut inputs = [INPUT::default(); 3];
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

const fn is_own_replayed_input(extra_info: usize) -> bool {
    extra_info == REPLAYED_INPUT_MARKER
}

const fn is_extended_virtual_key(virtual_key: u16) -> bool {
    matches!(
        virtual_key,
        0x21..=0x28 | 0x2D..=0x2E | 0x5B..=0x5D | 0x6F | 0x90 | 0xA3 | 0xA5
    )
}

fn translate_search_character(data: &KBDLLHOOKSTRUCT, target_thread_id: u32) -> Option<char> {
    let mut keyboard_state = [0_u8; 256];
    unsafe {
        // SAFETY: the complete fixed-size keyboard-state buffer is writable for this call.
        GetKeyboardState(&mut keyboard_state).ok()?;
    }
    for key in [VK_MENU, VK_LMENU, VK_RMENU] {
        keyboard_state[usize::from(key.0)] = 0;
    }
    let virtual_key = u8::try_from(data.vkCode).ok()?;
    keyboard_state[usize::from(virtual_key)] |= 0x80;
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
        let search_active = Arc::new(AtomicBool::new(false));
        let overlay_active = Arc::new(AtomicBool::new(false));
        let mut context = HookContext {
            target: HWND::default(),
            state: HookState::default(),
            settings: HookSettings::default(),
            search_active: Arc::clone(&search_active),
            overlay_active: Arc::clone(&overlay_active),
            target_thread_id: 0,
            hook_thread_id: 0,
            pending_errors: 0,
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
        assert!(overlay_active.load(Ordering::Acquire));
        assert!(search_active.load(Ordering::Relaxed));
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
