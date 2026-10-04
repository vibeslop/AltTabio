//! How native callbacks reach the one `App` on the main thread.

use super::App;
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2_foundation::NSTimer;
use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;

thread_local! {
    static APP: RefCell<Option<Rc<RefCell<App>>>> = const { RefCell::new(None) };
}

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

/// Runs `work` on the next main run loop pass without holding any app-state borrow.
pub(super) fn run_later(work: impl FnOnce() + 'static) {
    let slot = RefCell::new(Some(Box::new(work) as Box<dyn FnOnce()>));
    let _timer = main_loop_timer(0.0, false, move || {
        if let Some(work) = slot.borrow_mut().take() {
            work();
        }
    });
}

/// Schedules `work` on the main run loop after `seconds`.
pub(super) fn schedule(seconds: f64, work: impl Fn(&mut App) + 'static) -> Retained<NSTimer> {
    main_loop_timer(seconds, false, move || {
        let _ = with_app(&work);
    })
}

/// Schedules `work` on the main run loop every `seconds` until the timer is invalidated.
pub(super) fn schedule_repeating(
    seconds: f64,
    work: impl Fn(&mut App) + 'static,
) -> Retained<NSTimer> {
    main_loop_timer(seconds, true, move || {
        let _ = with_app(&work);
    })
}

/// A timer on the main run loop. Call it from the main thread only.
fn main_loop_timer(seconds: f64, repeats: bool, fire: impl Fn() + 'static) -> Retained<NSTimer> {
    let block = RcBlock::new(move |_timer: NonNull<NSTimer>| fire());
    unsafe {
        // SAFETY: the timer is scheduled from the main thread onto the main run loop, so the
        // block runs on the same thread that created its non-Send captures.
        NSTimer::scheduledTimerWithTimeInterval_repeats_block(seconds, repeats, &block)
    }
}
