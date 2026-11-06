//! How native callbacks reach the one `App` on the main thread.

use super::App;
use alttabio::refresh_schedule::{RefreshNotice, RefreshSignal};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_foundation::NSTimer;
use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;

thread_local! {
    static APP: RefCell<Option<Rc<RefCell<App>>>> = const { RefCell::new(None) };
}

static REFRESH_NOTICES: RefreshSignal = RefreshSignal::new();

/// Makes `app` the one that `with_app` and `post_to_app` reach.
pub(super) fn install(app: &Rc<RefCell<App>>) {
    APP.with(|slot| *slot.borrow_mut() = Some(Rc::clone(app)));
}

/// A value that only the main thread unwraps after it crossed a completion queue.
pub(super) struct MainThreadValue<T>(pub T);

// SAFETY: every MainThreadValue is created on a background queue and unwrapped by
// `post_to_app` on the main thread; the wrapped object is never touched in between.
unsafe impl<T> Send for MainThreadValue<T> {}

pub(super) fn with_app<R>(work: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|slot| {
        let slot = slot.borrow();
        let app = slot.as_ref()?;
        if let Ok(mut app) = app.try_borrow_mut() {
            Some(work(&mut app))
        } else {
            eprintln!("Dropped a callback because the app state is busy");
            None
        }
    })
}

pub(super) fn post_to_app(work: impl FnOnce(&mut App) + Send + 'static) {
    DispatchQueue::main().exec_async(move || {
        let _ = with_app(work);
    });
}

/// Called by AX and workspace callbacks: merge before allocating a dispatch block.
pub(super) fn post_refresh_notice(notice: RefreshNotice) {
    if REFRESH_NOTICES.record(notice) {
        post_refresh_to_app(|app| app.apply_refresh_notices(REFRESH_NOTICES.take()));
    }
}

pub(super) fn stop_refresh_notices() {
    REFRESH_NOTICES.stop();
}

/// Refresh producers already bound the number of outstanding deliveries. Keep that
/// delivery alive if a nested run loop encounters a busy app instead of losing it.
pub(super) fn post_refresh_to_app(work: impl FnOnce(&mut App) + Send + 'static) {
    DispatchQueue::main().exec_async(move || deliver_refresh(work));
}

fn deliver_refresh(work: impl FnOnce(&mut App) + 'static) {
    let pending = APP.with(|slot| {
        let app = slot.borrow().as_ref().map(Rc::clone)?;
        let Ok(mut app) = app.try_borrow_mut() else {
            return Some(work);
        };
        work(&mut app);
        None
    });
    if let Some(work) = pending
        && let Some(mtm) = MainThreadMarker::new()
    {
        // Release every borrow before scheduling. One timer retains the existing
        // delivery; native producers continue merging into their fixed flags.
        let slot = RefCell::new(Some(work));
        let _timer = main_loop_timer(mtm, 0.01, false, move || {
            if let Some(work) = slot.borrow_mut().take() {
                deliver_refresh(work);
            }
        });
    }
}

/// Runs `work` on the next main run loop pass without holding any app-state borrow.
pub(super) fn run_later(mtm: MainThreadMarker, work: impl FnOnce() + 'static) {
    let slot = RefCell::new(Some(Box::new(work) as Box<dyn FnOnce()>));
    let _timer = main_loop_timer(mtm, 0.0, false, move || {
        if let Some(work) = slot.borrow_mut().take() {
            work();
        }
    });
}

/// Schedules `work` on the main run loop after `seconds`.
pub(super) fn schedule(
    mtm: MainThreadMarker,
    seconds: f64,
    work: impl Fn(&mut App) + 'static,
) -> Retained<NSTimer> {
    main_loop_timer(mtm, seconds, false, move || {
        let _ = with_app(&work);
    })
}

/// Schedules `work` on the main run loop every `seconds` until the timer is invalidated.
pub(super) fn schedule_repeating(
    mtm: MainThreadMarker,
    seconds: f64,
    work: impl Fn(&mut App) + 'static,
) -> Retained<NSTimer> {
    main_loop_timer(mtm, seconds, true, move || {
        let _ = with_app(&work);
    })
}

/// A timer on the main run loop. A timer joins the run loop of the thread that schedules it, so
/// the marker is what makes that the main one.
fn main_loop_timer(
    _mtm: MainThreadMarker,
    seconds: f64,
    repeats: bool,
    fire: impl Fn() + 'static,
) -> Retained<NSTimer> {
    let block = RcBlock::new(move |_timer: NonNull<NSTimer>| fire());
    // SAFETY: the binding asks for a sendable block, but this one never leaves the thread that
    // made its captures: the timer fires on the run loop of the thread scheduling it, which
    // `_mtm` proves is the main one, the only thread where `with_app` finds the app.
    unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(seconds, repeats, &block) }
}
