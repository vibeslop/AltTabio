//! Narrow Accessibility (`AXUIElement`) wrappers used for window enumeration and control, and
//! the observer that follows the front app's windows.

use super::runtime::post_to_app;
use objc2_application_services::{AXError, AXObserver, AXUIElement};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFRetained, CFRunLoop, CFRunLoopSource, CFString, CFType, Type,
    kCFRunLoopCommonModes,
};
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;

#[derive(Clone)]
pub struct AxElement(CFRetained<AXUIElement>);

// SAFETY: an AXUIElement is an immutable token naming a remote element; Apple documents the
// Accessibility client API as callable from any thread and every call here is a synchronous
// message to the owning process.
unsafe impl Send for AxElement {}
unsafe impl Sync for AxElement {}

impl std::fmt::Debug for AxElement {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AxElement")
    }
}

impl AxElement {
    #[must_use]
    pub fn application(pid: i32) -> Self {
        Self(unsafe {
            // SAFETY: creating an application element has no preconditions beyond a pid value.
            AXUIElement::new_application(pid)
        })
    }

    /// Bounds every call through this element so an unresponsive app cannot stall enumeration.
    pub fn set_messaging_timeout(&self, seconds: f32) {
        let _error = unsafe {
            // SAFETY: the element is live for the duration of the call.
            self.0.set_messaging_timeout(seconds)
        };
    }

    fn copy(&self, attribute: &str) -> Option<CFRetained<CFType>> {
        self.try_copy(attribute).ok()
    }

    fn try_copy(&self, attribute: &str) -> Result<CFRetained<CFType>, AXError> {
        let name = CFString::from_str(attribute);
        let mut value: *const CFType = std::ptr::null();
        let error = unsafe {
            // SAFETY: `value` is a writable out-pointer for the synchronous call and the copied
            // reference is owned by this function afterwards.
            self.0
                .copy_attribute_value(&name, NonNull::from(&mut value))
        };
        if error != AXError::Success {
            return Err(error);
        }
        let pointer = NonNull::new(value.cast_mut()).ok_or(AXError::NoValue)?;
        Ok(unsafe {
            // SAFETY: a successful copy hands over one owned reference.
            CFRetained::from_raw(pointer)
        })
    }

    #[must_use]
    pub fn string(&self, attribute: &str) -> Option<String> {
        self.copy(attribute)?
            .downcast::<CFString>()
            .ok()
            .map(|value| value.to_string())
    }

    #[must_use]
    pub fn boolean(&self, attribute: &str) -> Option<bool> {
        self.copy(attribute)?
            .downcast::<CFBoolean>()
            .ok()
            .map(|value| value.as_bool())
    }

    #[must_use]
    pub fn element(&self, attribute: &str) -> Option<Self> {
        self.copy(attribute)?
            .downcast::<AXUIElement>()
            .ok()
            .map(Self)
    }

    /// The elements an array attribute holds, or why the app gave none.
    pub fn elements(&self, attribute: &str) -> Result<Vec<Self>, AXError> {
        let array = self
            .try_copy(attribute)?
            .downcast::<CFArray>()
            .map_err(|_| AXError::IllegalArgument)?;
        let count = usize::try_from(array.count()).unwrap_or_default();
        Ok((0..count)
            .filter_map(|index| {
                let raw = unsafe {
                    // SAFETY: `index` is below the count reported by the same array.
                    array.value_at_index(index.try_into().ok()?)
                };
                let pointer = NonNull::new(raw.cast_mut().cast::<CFType>())?;
                let value = unsafe {
                    // SAFETY: the array owns the element; borrowing it while the array is alive
                    // and retaining it before use keeps it valid past the array.
                    pointer.as_ref()
                };
                value
                    .downcast_ref::<AXUIElement>()
                    .map(|element| Self(element.retain()))
            })
            .collect())
    }

    pub fn set_boolean(&self, attribute: &str, value: bool) -> bool {
        let name = CFString::from_str(attribute);
        let error = unsafe {
            // SAFETY: both arguments are live CF objects for the synchronous call.
            self.0.set_attribute_value(&name, CFBoolean::new(value))
        };
        error == AXError::Success
    }

    pub fn perform(&self, action: &str) -> bool {
        let name = CFString::from_str(action);
        let error = unsafe {
            // SAFETY: the action name is a live CF string for the synchronous call.
            self.0.perform_action(&name)
        };
        error == AXError::Success
    }

    /// The `CGWindowID` behind a window element. The function is private but stable since 10.x and
    /// is the only bridge between Accessibility windows and window-list or capture APIs.
    #[must_use]
    pub fn window_id(&self) -> Option<u32> {
        unsafe extern "C" {
            fn _AXUIElementGetWindow(element: &AXUIElement, window_id: *mut u32) -> AXError;
        }
        let mut window_id = 0_u32;
        let error = unsafe {
            // SAFETY: the element is live and `window_id` is a writable out-pointer.
            _AXUIElementGetWindow(&self.0, &raw mut window_id)
        };
        (error == AXError::Success && window_id != 0).then_some(window_id)
    }
}

/// What the front app's observer saw.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowChange {
    /// Focus moved to another of the app's windows.
    Focused,
    /// The app opened a window.
    Opened,
}

impl WindowChange {
    const ALL: [Self; 2] = [Self::Focused, Self::Opened];

    const fn notification(self) -> &'static str {
        match self {
            Self::Focused => "AXFocusedWindowChanged",
            Self::Opened => "AXWindowCreated",
        }
    }

    /// The registration's refcon, which tells the callback what it was registered for.
    fn refcon(self) -> *mut c_void {
        std::ptr::without_provenance_mut(self as usize + 1)
    }

    fn from_refcon(refcon: *mut c_void) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|change| change.refcon() == refcon)
    }
}

unsafe extern "C-unwind" {
    // Declared here because the binding panics instead of returning `None`.
    fn AXObserverGetRunLoopSource(observer: &AXObserver) -> Option<NonNull<CFRunLoopSource>>;
}

/// An Accessibility observer on one app's window focus and new windows. Nothing reports those
/// changes otherwise; the switcher used to ask every app for its windows every two seconds
/// instead.
pub struct AppObserver {
    _observer: CFRetained<AXObserver>,
    source: CFRetained<CFRunLoopSource>,
}

// SAFETY: the observer is registered on the window list thread and handed over whole to the main
// thread, which alone schedules its source; no two threads use it at the same time.
unsafe impl Send for AppObserver {}

impl AppObserver {
    /// Registers for `pid`'s focus changes and new windows, or `None` when the app takes no
    /// registration. Registering asks the app over Accessibility, so it runs off the main thread.
    #[must_use]
    pub fn new(pid: i32, timeout_seconds: f32) -> Option<Self> {
        let mut raw: *mut AXObserver = std::ptr::null_mut();
        let error = unsafe {
            // SAFETY: `raw` is a writable out-pointer and the callback matches AXObserverCallback.
            AXObserver::create(pid, Some(observer_callback), NonNull::from(&mut raw))
        };
        if error != AXError::Success {
            return None;
        }
        let observer = unsafe {
            // SAFETY: a successful create hands over one owned reference.
            CFRetained::from_raw(NonNull::new(raw)?)
        };
        let application = AxElement::application(pid);
        application.set_messaging_timeout(timeout_seconds);
        let mut registered = false;
        for change in WindowChange::ALL {
            let error = unsafe {
                // SAFETY: the observer and element are live, and the refcon is a plain tag that
                // is never dereferenced.
                observer.add_notification(
                    &application.0,
                    &CFString::from_static_str(change.notification()),
                    change.refcon(),
                )
            };
            registered |= error == AXError::Success;
        }
        if !registered {
            return None;
        }
        let source = unsafe {
            // SAFETY: the observer is live; the source it returns is retained before use.
            CFRetained::retain(AXObserverGetRunLoopSource(&observer)?)
        };
        Some(Self {
            _observer: observer,
            source,
        })
    }

    /// Starts delivering the app's changes on the main run loop.
    pub fn attach(&self) {
        if let Some(run_loop) = CFRunLoop::main() {
            run_loop.add_source(Some(&self.source), unsafe {
                // SAFETY: the common-modes constant is a static string owned by CoreFoundation.
                kCFRunLoopCommonModes
            });
        }
    }
}

impl Drop for AppObserver {
    fn drop(&mut self) {
        // Unregistering would ask the app, which may not answer; without its source on a run
        // loop the observer delivers nothing, and the app forgets it once it is released.
        if let Some(run_loop) = CFRunLoop::main() {
            run_loop.remove_source(Some(&self.source), unsafe {
                // SAFETY: the common-modes constant is a static string owned by CoreFoundation.
                kCFRunLoopCommonModes
            });
        }
    }
}

unsafe extern "C-unwind" fn observer_callback(
    _observer: NonNull<AXObserver>,
    _element: NonNull<AXUIElement>,
    _notification: NonNull<CFString>,
    refcon: *mut c_void,
) {
    let Some(change) = WindowChange::from_refcon(refcon) else {
        return;
    };
    // Nothing may unwind into Accessibility; a lost change shows up in the next refresh.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        post_to_app(move |app| app.front_window_changed(change));
    }));
}
