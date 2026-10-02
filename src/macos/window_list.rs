//! Window enumeration that merges the on-screen z-order with Accessibility window lists.
//!
//! `CGWindowListCopyWindowInfo` knows the z-order, bounds, and owner of every on-screen window but
//! not minimized windows, windows on other Spaces, or hidden apps. Accessibility knows every
//! window of every app with its minimized state and the element needed to raise or close it,
//! but not the z-order. The merge keeps the front-to-back order for visible windows and appends
//! everything else so the switcher can reach windows the system switcher hides.

use super::ax::AxElement;
use alttabio::metadata::{MAX_METADATA_CHARS, MAX_WINDOWS, MAX_WINDOWS_PER_APP, bounded_text};
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};
use objc2_application_services::AXError;
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType, Type};
use objc2_core_graphics::{
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGWindowAlpha, kCGWindowBounds,
    kCGWindowLayer, kCGWindowName, kCGWindowNumber, kCGWindowOwnerPID,
};
use std::collections::{HashMap, HashSet};
use std::ptr::NonNull;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct WindowRecord {
    pub window_id: u32,
    pub pid: i32,
    pub launched_at: u64,
    pub title: String,
    pub app_name: String,
    pub is_minimized: bool,
    pub is_hidden: bool,
    pub is_on_screen: bool,
    /// Top-left origin screen coordinates: x, y, width, height.
    pub bounds: [f64; 4],
    pub ax: Option<AxElement>,
}

/// A running app with no window the switcher can list, as the system switcher still shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowlessApp {
    pub pid: i32,
    pub launched_at: u64,
    pub name: String,
}

/// What one enumeration found.
#[derive(Debug, Default)]
pub struct Listing {
    pub windows: Vec<WindowRecord>,
    /// Apps with no window at all. An app whose windows the display filter leaves out is not
    /// here, because it does have windows.
    pub windowless: Vec<WindowlessApp>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EnumerationOptions {
    pub current_pid: i32,
    /// Top-left origin bounds of the display whose windows are wanted, if filtering.
    pub display_bounds: Option<[f64; 4]>,
}

struct AppInfo {
    pid: i32,
    name: String,
    hidden: bool,
    active: bool,
    launched_at: u64,
}

struct CgWindow {
    id: u32,
    pid: i32,
    /// `kCGWindowName`, which the window server only reveals with Screen Recording access.
    name: String,
    bounds: [f64; 4],
}

struct AxWindow {
    id: u32,
    title: String,
    minimized: bool,
    element: AxElement,
}

// Accessibility calls block until the target app answers; a frozen app must not stall the list.
pub const AX_TIMEOUT_SECONDS: f32 = 0.25;
// How long an app that let a request time out is left out of the Accessibility pass. The apps
// are asked one after another, so one that never answers, such as a headless Blender, added the
// whole timeout to every listing.
const UNRESPONSIVE_SKIP: Duration = Duration::from_mins(5);

/// Apps that let an Accessibility request time out, and until when they are not asked again.
#[derive(Debug, Default)]
pub struct Unresponsive(HashMap<i32, Instant>);

impl Unresponsive {
    /// Whether to leave `app` out now. The front app is asked regardless: someone is using it,
    /// so it has likely recovered.
    fn skips(&self, app: &AppInfo, now: Instant) -> bool {
        !app.active && self.0.get(&app.pid).is_some_and(|until| now < *until)
    }

    fn note(&mut self, pid: i32, answered: bool, now: Instant) {
        if answered {
            self.0.remove(&pid);
        } else {
            self.0.insert(pid, now + UNRESPONSIVE_SKIP);
        }
    }
}

#[must_use]
pub fn enumerate(options: EnumerationOptions, unresponsive: &mut Unresponsive) -> Listing {
    let apps = regular_applications(options.current_pid);
    let on_screen = on_screen_windows();
    let mut records = Vec::new();
    let mut ax_windows = Vec::new();
    let now = Instant::now();
    unresponsive
        .0
        .retain(|pid, _| apps.iter().any(|app| app.pid == *pid));
    for app in &apps {
        if ax_windows.len() >= MAX_WINDOWS {
            break;
        }
        // An app left out keeps its on-screen windows, as one that answers nothing does.
        if unresponsive.skips(app, now) {
            continue;
        }
        let windows = accessibility_windows(app.pid);
        unresponsive.note(
            app.pid,
            !matches!(windows, Err(AXError::CannotComplete)),
            now,
        );
        for window in windows
            .unwrap_or_default()
            .into_iter()
            .take(MAX_WINDOWS - ax_windows.len())
        {
            ax_windows.push((app.pid, window));
        }
    }

    let mut counts: HashMap<i32, usize> = HashMap::new();
    for cg_window in &on_screen {
        if records.len() >= MAX_WINDOWS {
            break;
        }
        if *counts.get(&cg_window.pid).unwrap_or(&0) >= MAX_WINDOWS_PER_APP {
            continue;
        }
        let Some(app) = apps.iter().find(|app| app.pid == cg_window.pid) else {
            continue;
        };
        let ax = ax_windows
            .iter()
            .position(|(_, window)| window.id == cg_window.id)
            .map(|index| ax_windows.swap_remove(index).1);
        // Chromium browsers keep unnamed layer-0 helper windows (download bubbles, info bars,
        // tab hover cards) that come and go; Accessibility never lists them as windows. When
        // the app answers Accessibility at all, its window list is the authority, so a
        // helper without a standard-window counterpart is not a row. Apps that answered
        // nothing (frozen, or not yet accessible) keep their on-screen windows as before.
        if ax.is_none() && app_answers_accessibility(&ax_windows, &records, app.pid) {
            continue;
        }
        let title = ax
            .as_ref()
            .map(|window| window.title.clone())
            .filter(|title| !title.is_empty())
            .or_else(|| (!cg_window.name.is_empty()).then(|| cg_window.name.clone()))
            .unwrap_or_else(|| app.name.clone());
        *counts.entry(app.pid).or_default() += 1;
        records.push(WindowRecord {
            window_id: cg_window.id,
            pid: app.pid,
            launched_at: app.launched_at,
            title,
            app_name: app.name.clone(),
            is_minimized: false,
            is_hidden: app.hidden,
            is_on_screen: true,
            bounds: cg_window.bounds,
            ax: ax.map(|window| window.element),
        });
    }

    for (pid, window) in ax_windows {
        if records.len() >= MAX_WINDOWS {
            break;
        }
        if *counts.get(&pid).unwrap_or(&0) >= MAX_WINDOWS_PER_APP {
            continue;
        }
        let Some(app) = apps.iter().find(|app| app.pid == pid) else {
            continue;
        };
        if window.title.is_empty() {
            // Off-screen windows without a title are usually helper panels; the user cannot
            // recognise them in the list.
            continue;
        }
        *counts.entry(pid).or_default() += 1;
        records.push(WindowRecord {
            window_id: window.id,
            pid,
            launched_at: app.launched_at,
            title: window.title,
            app_name: app.name.clone(),
            is_minimized: window.minimized,
            is_hidden: app.hidden,
            is_on_screen: false,
            bounds: [0.0; 4],
            ax: Some(window.element),
        });
    }

    let windowless = apps
        .iter()
        .filter(|app| !records.iter().any(|record| record.pid == app.pid))
        .map(|app| WindowlessApp {
            pid: app.pid,
            launched_at: app.launched_at,
            name: app.name.clone(),
        })
        .collect();
    if let Some(display) = options.display_bounds {
        records.retain(|record| !record.is_on_screen || bounds_on_display(record.bounds, display));
    }
    Listing {
        windows: records,
        windowless,
    }
}

/// Whether `pid` produced at least one Accessibility window: either one still waiting in
/// `ax_windows` or one already matched into `records`.
fn app_answers_accessibility(
    ax_windows: &[(i32, AxWindow)],
    records: &[WindowRecord],
    pid: i32,
) -> bool {
    ax_windows.iter().any(|(owner, _)| *owner == pid)
        || records
            .iter()
            .any(|record| record.pid == pid && record.ax.is_some())
}

/// Front-to-back order for visible windows, then the previously known order for the rest.
#[must_use]
pub fn merge_order(previous: &[u32], on_screen: &[u32], others: &[u32]) -> Vec<u32> {
    let mut order = on_screen
        .iter()
        .copied()
        .take(MAX_WINDOWS)
        .collect::<Vec<_>>();
    let others = &others[..others.len().min(MAX_WINDOWS)];
    let available: HashSet<_> = others.iter().copied().collect();
    let mut included: HashSet<_> = order.iter().copied().collect();
    for id in previous.iter().take(MAX_WINDOWS) {
        if order.len() >= MAX_WINDOWS {
            break;
        }
        if available.contains(id) && included.insert(*id) {
            order.push(*id);
        }
    }
    for id in others {
        if order.len() >= MAX_WINDOWS {
            break;
        }
        if included.insert(*id) {
            order.push(*id);
        }
    }
    order
}

/// The on-screen windows of `pid`, front to back. It asks only the window server, not
/// Accessibility, so a hung app cannot stall it.
#[must_use]
pub fn on_screen_windows_of(pid: i32) -> Vec<u32> {
    on_screen_windows()
        .into_iter()
        .filter(|window| window.pid == pid)
        .map(|window| window.id)
        .collect()
}

fn bounds_on_display(bounds: [f64; 4], display: [f64; 4]) -> bool {
    let center_x = bounds[0] + bounds[2] / 2.0;
    let center_y = bounds[1] + bounds[3] / 2.0;
    center_x >= display[0]
        && center_x < display[0] + display[2]
        && center_y >= display[1]
        && center_y < display[1] + display[3]
}

fn regular_applications(current_pid: i32) -> Vec<AppInfo> {
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter(|app| {
            app.activationPolicy() == NSApplicationActivationPolicy::Regular
                && app.processIdentifier() != current_pid
                && !app.isTerminated()
        })
        .take(MAX_WINDOWS)
        .map(|app| AppInfo {
            pid: app.processIdentifier(),
            name: app
                .localizedName()
                .map(|name| {
                    name.substringToIndex(name.length().min(MAX_METADATA_CHARS))
                        .to_string()
                })
                .unwrap_or_default(),
            hidden: app.isHidden(),
            active: app.isActive(),
            launched_at: launch_time(&app),
        })
        .collect()
}

#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "launch dates are positive Unix timestamps that fit u64 for any realistic clock"
)]
pub fn launch_time(app: &NSRunningApplication) -> u64 {
    app.launchDate()
        .map(|date| date.timeIntervalSince1970().max(0.0) as u64)
        .unwrap_or_default()
}

fn on_screen_windows() -> Vec<CgWindow> {
    let Some(list) = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    ) else {
        return Vec::new();
    };
    let count = usize::try_from(list.count()).unwrap_or_default().min(4096);
    (0..count)
        .filter_map(|index| {
            let raw = unsafe {
                // SAFETY: `index` is below the count reported by the same array.
                list.value_at_index(index.try_into().ok()?)
            };
            let pointer = NonNull::new(raw.cast_mut().cast::<CFType>())?;
            let value = unsafe {
                // SAFETY: the list owns its entries for as long as it is alive.
                pointer.as_ref()
            };
            let dictionary = value.downcast_ref::<CFDictionary>()?;
            let (layer_key, alpha_key, number_key, pid_key, bounds_key, name_key) = unsafe {
                // SAFETY: the window-info keys are static strings exported by CoreGraphics.
                (
                    kCGWindowLayer,
                    kCGWindowAlpha,
                    kCGWindowNumber,
                    kCGWindowOwnerPID,
                    kCGWindowBounds,
                    kCGWindowName,
                )
            };
            let layer = dictionary_number(dictionary, layer_key)?;
            let alpha = dictionary_float(dictionary, alpha_key).unwrap_or(1.0);
            if layer != 0 || alpha <= 0.0 {
                return None;
            }
            let id = dictionary_number(dictionary, number_key)?;
            let pid = dictionary_number(dictionary, pid_key)?;
            let bounds = dictionary_value::<CFDictionary>(dictionary, bounds_key)?;
            Some(CgWindow {
                id: u32::try_from(id).ok()?,
                pid: i32::try_from(pid).ok()?,
                name: dictionary_value::<CFString>(dictionary, name_key)
                    .map(|name| super::ax::bounded_cf_string(&name))
                    .unwrap_or_default(),
                bounds: [
                    bounds_component(&bounds, "X"),
                    bounds_component(&bounds, "Y"),
                    bounds_component(&bounds, "Width"),
                    bounds_component(&bounds, "Height"),
                ],
            })
        })
        .take(MAX_WINDOWS)
        .collect()
}

fn accessibility_windows(pid: i32) -> Result<Vec<AxWindow>, AXError> {
    let application = AxElement::application(pid);
    application.set_messaging_timeout(AX_TIMEOUT_SECONDS);
    Ok(application
        .elements("AXWindows")?
        .into_iter()
        .filter_map(|element| {
            let subrole = element.string("AXSubrole").unwrap_or_default();
            if subrole != "AXStandardWindow" && subrole != "AXDialog" {
                return None;
            }
            Some(AxWindow {
                id: element.window_id()?,
                title: bounded_text(&element.string("AXTitle").unwrap_or_default()),
                minimized: element.boolean("AXMinimized").unwrap_or(false),
                element,
            })
        })
        .collect())
}

fn dictionary_value<T: objc2_core_foundation::ConcreteType + Type>(
    dictionary: &CFDictionary,
    key: &CFString,
) -> Option<CFRetained<T>> {
    let raw = unsafe {
        // SAFETY: the dictionary and key are live for the synchronous lookup.
        dictionary.value(std::ptr::from_ref(key).cast())
    };
    let pointer = NonNull::new(raw.cast_mut().cast::<CFType>())?;
    let value = unsafe {
        // SAFETY: the dictionary owns the value for as long as it is alive.
        pointer.as_ref()
    };
    value.downcast_ref::<T>().map(T::retain)
}

fn dictionary_number(dictionary: &CFDictionary, key: &CFString) -> Option<i64> {
    dictionary_value::<CFNumber>(dictionary, key)?.as_i64()
}

fn dictionary_float(dictionary: &CFDictionary, key: &CFString) -> Option<f64> {
    dictionary_value::<CFNumber>(dictionary, key)?.as_f64()
}

fn bounds_component(bounds: &CFDictionary, name: &str) -> f64 {
    dictionary_float(bounds, &CFString::from_str(name)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_windows_lead_and_hidden_ones_keep_their_previous_order() {
        let order = merge_order(&[7, 3, 9, 4], &[4, 1], &[9, 3, 8]);

        assert_eq!(order, vec![4, 1, 3, 9, 8]);
    }

    #[test]
    fn unknown_windows_are_appended_once() {
        let order = merge_order(&[], &[2, 2], &[5, 5, 2]);

        assert_eq!(order, vec![2, 2, 5]);
    }

    #[test]
    fn an_app_answers_accessibility_through_pending_or_matched_windows() {
        let pending = vec![(
            7,
            AxWindow {
                id: 1,
                title: "Doc".to_owned(),
                minimized: false,
                element: AxElement::application(7),
            },
        )];
        let matched = vec![WindowRecord {
            window_id: 2,
            pid: 9,
            launched_at: 0,
            title: "Sheet".to_owned(),
            app_name: "App".to_owned(),
            is_minimized: false,
            is_hidden: false,
            is_on_screen: true,
            bounds: [0.0; 4],
            ax: Some(AxElement::application(9)),
        }];

        assert!(app_answers_accessibility(&pending, &matched, 7));
        assert!(app_answers_accessibility(&pending, &matched, 9));
        assert!(!app_answers_accessibility(&pending, &matched, 11));
    }

    fn app(pid: i32, active: bool) -> AppInfo {
        AppInfo {
            pid,
            name: "App".to_owned(),
            hidden: false,
            active,
            launched_at: 0,
        }
    }

    #[test]
    fn an_app_that_timed_out_is_skipped_until_the_wait_is_over() {
        let mut unresponsive = Unresponsive::default();
        let start = Instant::now();
        unresponsive.note(7, false, start);

        assert!(unresponsive.skips(&app(7, false), start + Duration::from_secs(1)));
        assert!(!unresponsive.skips(&app(7, false), start + UNRESPONSIVE_SKIP));
        assert!(!unresponsive.skips(&app(8, false), start));
    }

    #[test]
    fn the_front_app_and_an_app_that_answers_are_asked_again() {
        let mut unresponsive = Unresponsive::default();
        let start = Instant::now();
        unresponsive.note(7, false, start);

        assert!(!unresponsive.skips(&app(7, true), start));
        unresponsive.note(7, true, start);
        assert!(!unresponsive.skips(&app(7, false), start));
    }

    #[test]
    fn display_filter_uses_the_window_center() {
        let display = [0.0, 0.0, 1000.0, 500.0];

        assert!(bounds_on_display([-100.0, 10.0, 400.0, 300.0], display));
        assert!(!bounds_on_display([900.0, 10.0, 400.0, 300.0], display));
        assert!(!bounds_on_display([100.0, 400.0, 400.0, 300.0], display));
    }
}
