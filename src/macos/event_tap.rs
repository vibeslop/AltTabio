//! `CGEventTap` adapter: turns HID events into `TapEvent`s and applies the suppress decision.
//!
//! The callback runs on the main run loop. It does only the bounded translation and the
//! synchronous suppress decision; every switcher effect is queued back to the app afterwards so
//! a slow frame can never trip the system's tap timeout.

use super::hotkey::{ModifierState, TapEvent};
use super::keymap::key_for_code;
use objc2_core_foundation::{
    CFMachPort, CFRetained, CFRunLoop, CFRunLoopSource, CGPoint, kCFRunLoopCommonModes,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventTapProxy, CGEventType, CGMouseButton,
};
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;

pub type TapHandler = Box<dyn FnMut(TapEvent, CGPoint) -> bool>;

// Marks events this process posts so the tap does not feed them back into the gesture state.
const SYNTHETIC_EVENT_MARKER: i64 = 0x616C_7474; // "altt"

struct TapContext {
    handler: TapHandler,
    port: Option<CFRetained<CFMachPort>>,
}

pub struct EventTap {
    port: CFRetained<CFMachPort>,
    source: CFRetained<CFRunLoopSource>,
    context: *mut TapContext,
}

impl EventTap {
    /// Installs the tap at the head of the session taps, so it sees keys before any tap that
    /// was there earlier. The app reinstalls it when the front app changes: remote desktop and
    /// VM clients grab ⌘ Tab for their guest with a head-inserted tap of their own, and only
    /// the newest tap at the head sees the keys first.
    pub fn install(handler: TapHandler) -> Result<Self, String> {
        let context = Box::into_raw(Box::new(TapContext {
            handler,
            port: None,
        }));
        let mask = event_mask(&[
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
            CGEventType::LeftMouseDown,
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGEventType::ScrollWheel,
        ]);
        let port = unsafe {
            // SAFETY: `context` stays allocated until `Drop` disables the tap and frees it after
            // no callback can run; the callback signature matches CGEventTapCallBack.
            CGEvent::tap_create(
                CGEventTapLocation::SessionEventTap,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                mask,
                Some(tap_callback),
                context.cast(),
            )
        };
        let Some(port) = port else {
            unsafe {
                // SAFETY: no tap retained `context`, so this is the unique allocation from above.
                drop(Box::from_raw(context));
            }
            return Err(
                "Could not install the keyboard event tap. Allow AltTabio under System Settings > \
                 Privacy & Security > Accessibility, then start it again."
                    .to_owned(),
            );
        };
        let Some(source) = CFMachPort::new_run_loop_source(None, Some(&port), 0) else {
            unsafe {
                // SAFETY: the tap was never enabled; no callback can reference `context`.
                drop(Box::from_raw(context));
            }
            return Err("Could not create the event tap run loop source".to_owned());
        };
        let Some(run_loop) = CFRunLoop::main() else {
            unsafe {
                // SAFETY: the tap was never enabled; no callback can reference `context`.
                drop(Box::from_raw(context));
            }
            return Err("The main run loop is unavailable".to_owned());
        };
        run_loop.add_source(Some(&source), unsafe {
            // SAFETY: the common-modes constant is a static string owned by CoreFoundation.
            kCFRunLoopCommonModes
        });
        CGEvent::tap_enable(&port, true);
        unsafe {
            // SAFETY: the callback only reads `port` after this write and both happen on the
            // main thread.
            (*context).port = Some(port.clone());
        }
        Ok(Self {
            port,
            source,
            context,
        })
    }

    /// Balances a right-button press that already reached the app under the cursor before the
    /// wheel gesture claimed the button.
    pub fn post_right_button_release(location: CGPoint) {
        let event = CGEvent::new_mouse_event(
            None,
            CGEventType::RightMouseUp,
            location,
            CGMouseButton::Right,
        );
        let Some(event) = event else {
            eprintln!("Could not create the synthetic right-button release");
            return;
        };
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::EventSourceUserData,
            SYNTHETIC_EVENT_MARKER,
        );
        CGEvent::post(CGEventTapLocation::SessionEventTap, Some(&event));
    }
}

impl Drop for EventTap {
    fn drop(&mut self) {
        CGEvent::tap_enable(&self.port, false);
        if let Some(run_loop) = CFRunLoop::main() {
            run_loop.remove_source(Some(&self.source), unsafe {
                // SAFETY: the common-modes constant is a static string owned by CoreFoundation.
                kCFRunLoopCommonModes
            });
        }
        unsafe {
            // SAFETY: the tap is disabled and its source removed, so no callback runs again and
            // this is the unique allocation created in `install`.
            drop(Box::from_raw(self.context));
        }
    }
}

fn event_mask(types: &[CGEventType]) -> u64 {
    types
        .iter()
        .fold(0_u64, |mask, kind| mask | (1_u64 << kind.0))
}

unsafe extern "C-unwind" fn tap_callback(
    _proxy: CGEventTapProxy,
    event_type: CGEventType,
    event: NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    let context = unsafe {
        // SAFETY: `user_info` is the TapContext allocation owned by the live EventTap.
        user_info.cast::<TapContext>().as_mut()
    };
    let Some(context) = context else {
        return event.as_ptr();
    };
    let suppress = catch_unwind(AssertUnwindSafe(|| {
        let event_ref = unsafe {
            // SAFETY: the system passes a live event for the duration of the callback.
            event.as_ref()
        };
        handle_event(context, event_type, event_ref)
    }))
    .unwrap_or_else(|_| {
        eprintln!("The event tap handler panicked; passing the event through");
        false
    });
    if suppress {
        std::ptr::null_mut()
    } else {
        event.as_ptr()
    }
}

fn handle_event(context: &mut TapContext, event_type: CGEventType, event: &CGEvent) -> bool {
    if event_type == CGEventType::TapDisabledByTimeout
        || event_type == CGEventType::TapDisabledByUserInput
    {
        if let Some(port) = &context.port {
            CGEvent::tap_enable(port, true);
        }
        return false;
    }
    if CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUserData)
        == SYNTHETIC_EVENT_MARKER
    {
        return false;
    }
    let location = CGEvent::location(Some(event));
    let tap_event = match event_type {
        CGEventType::KeyDown => TapEvent::KeyDown {
            key: key_for_code(key_code(event)),
            text: typed_character(event),
            repeated: CGEvent::integer_value_field(
                Some(event),
                CGEventField::KeyboardEventAutorepeat,
            ) != 0,
        },
        CGEventType::KeyUp => TapEvent::KeyUp,
        CGEventType::FlagsChanged => {
            let flags = CGEvent::flags(Some(event));
            TapEvent::ModifiersChanged(ModifierState {
                command: flags.contains(CGEventFlags::MaskCommand),
                option: flags.contains(CGEventFlags::MaskAlternate),
                shift: flags.contains(CGEventFlags::MaskShift),
                control: flags.contains(CGEventFlags::MaskControl),
            })
        }
        CGEventType::LeftMouseDown => TapEvent::LeftMouseDown {
            inside_overlay: false,
        },
        CGEventType::RightMouseDown => TapEvent::RightMouseDown,
        CGEventType::RightMouseUp => TapEvent::RightMouseUp,
        CGEventType::ScrollWheel => {
            let delta = CGEvent::integer_value_field(
                Some(event),
                CGEventField::ScrollWheelEventPointDeltaAxis1,
            );
            if delta == 0 {
                return false;
            }
            TapEvent::ScrollWheel(delta.signum().try_into().unwrap_or(1))
        }
        _ => return false,
    };
    (context.handler)(tap_event, location)
}

fn key_code(event: &CGEvent) -> u16 {
    u16::try_from(CGEvent::integer_value_field(
        Some(event),
        CGEventField::KeyboardEventKeycode,
    ))
    .unwrap_or(u16::MAX)
}

fn typed_character(event: &CGEvent) -> Option<char> {
    let mut buffer = [0_u16; 4];
    let mut length = 0_u64;
    unsafe {
        // SAFETY: `buffer` has room for the declared maximum and `length` is writable.
        CGEvent::keyboard_get_unicode_string(
            Some(event),
            buffer.len() as u64,
            &raw mut length,
            buffer.as_mut_ptr(),
        );
    }
    let written = buffer.get(..usize::try_from(length).ok()?)?;
    char::decode_utf16(written.iter().copied())
        .next()
        .and_then(Result::ok)
}
