//! Producer-side bounds for native refresh notices and serial refresh work.

use std::sync::atomic::{AtomicU8, Ordering};

const STOPPED: u8 = 1 << 7;

#[derive(Clone, Copy)]
pub enum RefreshNotice {
    List = 1,
    Focus = 2,
    FrontApp = 4,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RefreshNotices {
    pub list: bool,
    pub focus: bool,
    pub front_app: bool,
}

/// Native callbacks merge flags before allocating a main-queue closure. A nonempty
/// set owns one wakeup until the main thread can borrow the app and take the flags.
pub struct RefreshSignal(AtomicU8);

impl Default for RefreshSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl RefreshSignal {
    #[must_use]
    pub const fn new() -> Self {
        Self(AtomicU8::new(0))
    }

    /// Returns whether the producer must enqueue the single wakeup.
    #[must_use]
    pub fn record(&self, notice: RefreshNotice) -> bool {
        self.0.fetch_or(notice as u8, Ordering::AcqRel) == 0
    }

    pub fn take(&self) -> RefreshNotices {
        let flags = self.0.fetch_and(STOPPED, Ordering::AcqRel);
        if flags & STOPPED != 0 {
            return RefreshNotices::default();
        }
        RefreshNotices {
            list: flags & RefreshNotice::List as u8 != 0,
            focus: flags & RefreshNotice::Focus as u8 != 0,
            front_app: flags & RefreshNotice::FrontApp as u8 != 0,
        }
    }

    pub fn stop(&self) {
        self.0.store(STOPPED, Ordering::Release);
    }
}

/// One batch may be running or waiting for main-thread completion. While it is
/// outstanding, requests replace two fixed slots instead of allocating queue entries.
/// All calls belong to the scheduling thread; the worker only receives returned batches.
#[derive(Debug)]
pub struct RefreshSchedule<L, W> {
    list: Option<L>,
    watch: Option<W>,
    in_flight: bool,
    stopped: bool,
}

impl<L, W> Default for RefreshSchedule<L, W> {
    fn default() -> Self {
        Self {
            list: None,
            watch: None,
            in_flight: false,
            stopped: false,
        }
    }
}

pub type RefreshBatch<L, W> = (Option<L>, Option<W>);

impl<L, W> RefreshSchedule<L, W> {
    pub fn request_list(&mut self, list: L) -> Option<RefreshBatch<L, W>> {
        if self.stopped {
            return None;
        }
        self.list = Some(list);
        self.next()
    }

    pub fn request_watch(&mut self, watch: W) -> Option<RefreshBatch<L, W>> {
        if self.stopped {
            return None;
        }
        self.watch = Some(watch);
        self.next()
    }

    /// Called only after the outstanding batch reaches the main thread.
    pub fn complete(&mut self) -> Option<RefreshBatch<L, W>> {
        self.in_flight = false;
        self.next()
    }

    fn next(&mut self) -> Option<RefreshBatch<L, W>> {
        if self.stopped || self.in_flight || (self.list.is_none() && self.watch.is_none()) {
            return None;
        }
        self.in_flight = true;
        Some((self.list.take(), self.watch.take()))
    }

    #[must_use]
    pub const fn is_stopped(&self) -> bool {
        self.stopped
    }

    pub fn stop(&mut self) {
        self.stopped = true;
        self.list = None;
        self.watch = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_storm_owns_one_wakeup_until_app_is_available() {
        let signal = RefreshSignal::new();
        assert!(signal.record(RefreshNotice::List));
        // Not taking the flags models a busy app, including a nested run loop.
        for _ in 0..100_000 {
            assert!(!signal.record(RefreshNotice::List));
            assert!(!signal.record(RefreshNotice::Focus));
            assert!(!signal.record(RefreshNotice::FrontApp));
        }
        assert_eq!(
            signal.take(),
            RefreshNotices {
                list: true,
                focus: true,
                front_app: true
            }
        );
        assert!(signal.record(RefreshNotice::List));
        assert!(signal.take().list);
        assert_eq!(signal.take(), RefreshNotices::default());
    }

    #[test]
    fn concurrent_producers_share_one_wakeup_and_keep_every_kind() {
        let signal = RefreshSignal::new();
        let wakeups = std::thread::scope(|scope| {
            let handles: Vec<_> = [
                RefreshNotice::List,
                RefreshNotice::Focus,
                RefreshNotice::FrontApp,
            ]
            .into_iter()
            .map(|notice| {
                let signal = &signal;
                scope.spawn(move || (0..10_000).filter(|_| signal.record(notice)).count())
            })
            .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap_or_else(|_| panic!("producer failed")))
                .sum::<usize>()
        });
        assert_eq!(wakeups, 1);
        assert_eq!(
            signal.take(),
            RefreshNotices {
                list: true,
                focus: true,
                front_app: true
            }
        );
    }

    #[test]
    fn notice_during_delivery_gets_a_followup_and_shutdown_cancels_it() {
        let signal = RefreshSignal::new();
        assert!(signal.record(RefreshNotice::List));
        assert!(signal.take().list);
        assert!(signal.record(RefreshNotice::Focus));
        signal.stop();
        assert!(!signal.record(RefreshNotice::List));
        assert_eq!(signal.take(), RefreshNotices::default());
        assert!(!signal.record(RefreshNotice::FrontApp));
    }

    #[test]
    fn stalled_worker_or_main_completion_keeps_only_latest_list_and_watch() {
        let mut schedule = RefreshSchedule::default();
        assert_eq!(schedule.request_list(0), Some((Some(0), None)));
        // No completion while AX enumeration, or its main-thread delivery, is blocked.
        for latest in 1..=100_000 {
            assert_eq!(schedule.request_list(latest), None);
            assert_eq!(schedule.request_watch(Some(latest)), None);
        }
        assert_eq!(
            schedule.complete(),
            Some((Some(100_000), Some(Some(100_000))))
        );
        assert_eq!(schedule.complete(), None);
        assert_eq!(
            schedule.request_watch(Some(42)),
            Some((None, Some(Some(42))))
        );
    }

    #[test]
    fn latest_watch_cancellation_replaces_pending_registration() {
        let mut schedule = RefreshSchedule::default();
        assert!(schedule.request_list(1).is_some());
        assert!(schedule.request_watch(Some(42)).is_none());
        assert!(schedule.request_watch(None).is_none());
        assert_eq!(schedule.complete(), Some((None, Some(None))));
    }

    #[test]
    fn replacing_pending_payloads_releases_old_values_during_a_stall() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;

        struct Payload(Arc<AtomicUsize>);

        impl Payload {
            fn new(live: &Arc<AtomicUsize>) -> Self {
                live.fetch_add(1, Ordering::Relaxed);
                Self(Arc::clone(live))
            }
        }

        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::Relaxed);
            }
        }

        let live = Arc::new(AtomicUsize::new(0));
        let mut schedule = RefreshSchedule::default();
        let running = schedule.request_list(Payload::new(&live));
        assert!(running.is_some());
        for _ in 0..100_000 {
            assert!(schedule.request_list(Payload::new(&live)).is_none());
            assert!(schedule.request_watch(Payload::new(&live)).is_none());
            // The outstanding list and the latest pending value of each kind.
            assert_eq!(live.load(Ordering::Relaxed), 3);
        }
        schedule.stop();
        assert_eq!(live.load(Ordering::Relaxed), 1);
        drop(running);
        assert_eq!(live.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn shutdown_discards_pending_work_and_rejects_late_requests_and_completions() {
        let mut schedule = RefreshSchedule::default();
        assert!(schedule.request_list(1).is_some());
        assert!(schedule.request_watch(Some(42)).is_none());
        assert!(schedule.request_list(2).is_none());
        schedule.stop();
        assert!(schedule.is_stopped());
        assert!(schedule.complete().is_none());
        assert!(schedule.request_list(3).is_none());
        assert!(schedule.request_watch(Some(43)).is_none());
        assert!(schedule.list.is_none());
        assert!(schedule.watch.is_none());
    }
}
