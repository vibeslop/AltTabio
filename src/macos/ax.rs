//! Narrow Accessibility (`AXUIElement`) wrappers used for window enumeration and control.

use objc2_application_services::{AXError, AXUIElement};
use objc2_core_foundation::{CFArray, CFBoolean, CFRetained, CFString, CFType, Type};
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
        let name = CFString::from_str(attribute);
        let mut value: *const CFType = std::ptr::null();
        let error = unsafe {
            // SAFETY: `value` is a writable out-pointer for the synchronous call and the copied
            // reference is owned by this function afterwards.
            self.0
                .copy_attribute_value(&name, NonNull::from(&mut value))
        };
        if error != AXError::Success {
            return None;
        }
        let pointer = NonNull::new(value.cast_mut())?;
        Some(unsafe {
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

    #[must_use]
    pub fn elements(&self, attribute: &str) -> Vec<Self> {
        let Some(array) = self
            .copy(attribute)
            .and_then(|value| value.downcast::<CFArray>().ok())
        else {
            return Vec::new();
        };
        let count = usize::try_from(array.count()).unwrap_or_default();
        (0..count)
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
            .collect()
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
