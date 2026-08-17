//! Coalesced switcher-list refresh notices and close-follow-up retries.

use crate::switcher::SwitchTask;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

pub const CLOSE_REFRESH_ATTEMPTS: u8 = 20;
pub const LISTED_REFRESH_SLOTS: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshWakeup {
    PostNow,
    AlreadyQueued,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefreshBatch {
    handles: [isize; LISTED_REFRESH_SLOTS],
    count: u8,
    overflow: bool,
}

impl RefreshBatch {
    #[must_use]
    pub const fn new(handles: [isize; LISTED_REFRESH_SLOTS], count: u8, overflow: bool) -> Self {
        Self {
            handles,
            count,
            overflow,
        }
    }

    #[must_use]
    pub const fn empty() -> Self {
        Self::new([0; LISTED_REFRESH_SLOTS], 0, false)
    }

    #[must_use]
    pub fn handles(&self) -> &[isize] {
        &self.handles[..usize::from(self.count)]
    }

    #[must_use]
    pub const fn requires_full_enumeration(&self) -> bool {
        self.overflow
    }
}

pub struct ListedRefreshSignal {
    slots: [AtomicIsize; LISTED_REFRESH_SLOTS],
    overflow: AtomicBool,
    queued: AtomicBool,
    dirty: AtomicBool,
}

impl Default for ListedRefreshSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl ListedRefreshSignal {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: [
                AtomicIsize::new(0),
                AtomicIsize::new(0),
                AtomicIsize::new(0),
                AtomicIsize::new(0),
            ],
            overflow: AtomicBool::new(false),
            queued: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
        }
    }

    pub fn record(&self, window_handle: isize) -> RefreshWakeup {
        if !self.store(window_handle) {
            self.overflow.store(true, Ordering::Release);
        }
        // Publish dirty only after the payload so a concurrent UI drain cannot clear the retry
        // marker before this notice becomes observable.
        self.dirty.store(true, Ordering::Release);
        if self.queued.swap(true, Ordering::AcqRel) {
            RefreshWakeup::AlreadyQueued
        } else {
            RefreshWakeup::PostNow
        }
    }

    fn store(&self, window_handle: isize) -> bool {
        if window_handle == 0 {
            return false;
        }
        for slot in &self.slots {
            if slot.load(Ordering::Acquire) == window_handle {
                return true;
            }
        }
        for slot in &self.slots {
            match slot.compare_exchange(0, window_handle, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return true,
                Err(existing) if existing == window_handle => return true,
                Err(_) => {}
            }
        }
        false
    }

    pub fn post_failed(&self) {
        self.queued.store(false, Ordering::Release);
    }

    pub fn take(&self) -> RefreshBatch {
        self.queued.store(false, Ordering::Release);
        self.dirty.store(false, Ordering::Release);
        let overflow = self.overflow.swap(false, Ordering::AcqRel);
        let mut handles = [0; LISTED_REFRESH_SLOTS];
        let mut count = 0_u8;
        for slot in &self.slots {
            let handle = slot.swap(0, Ordering::AcqRel);
            if handle != 0 {
                handles[usize::from(count)] = handle;
                count = count.saturating_add(1);
            }
        }
        RefreshBatch::new(handles, count, overflow)
    }

    #[must_use]
    pub fn is_queued(&self) -> bool {
        self.queued.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn needs_retry_wakeup(&self) -> bool {
        self.dirty.load(Ordering::Acquire) && !self.queued.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn take_retry(&self) -> Option<RefreshBatch> {
        self.needs_retry_wakeup().then(|| self.take())
    }
}

/// Callback-owned listed-refresh notify: record the handle, then at most one coalesced post.
/// A failed post leaves the dirty signal for a UI-owned wakeup; this never arms a timer.
pub fn request_listed_refresh(
    signal: &ListedRefreshSignal,
    window_handle: isize,
    post: impl FnOnce() -> bool,
) -> RefreshWakeup {
    let wakeup = signal.record(window_handle);
    if wakeup == RefreshWakeup::PostNow && !post() {
        signal.post_failed();
    }
    wakeup
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextMenuCommandOutcome {
    None,
    Rejected,
    Failed,
    Succeeded { close_window: Option<isize> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshDecision {
    Ignore,
    Defer,
    Refresh,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CloseRefresh {
    window_handle: isize,
    attempts_remaining: u8,
}

#[derive(Debug, Default)]
pub struct CloseRefreshTracker {
    pending: Vec<CloseRefresh>,
}

impl CloseRefreshTracker {
    pub fn track(&mut self, window_handle: isize) -> bool {
        if let Some(refresh) = self
            .pending
            .iter_mut()
            .find(|refresh| refresh.window_handle == window_handle)
        {
            refresh.attempts_remaining = CLOSE_REFRESH_ATTEMPTS;
            return false;
        }
        let timer_needed = self.pending.is_empty();
        self.pending.push(CloseRefresh {
            window_handle,
            attempts_remaining: CLOSE_REFRESH_ATTEMPTS,
        });
        timer_needed
    }

    pub fn reconcile(&mut self, tasks: &[SwitchTask]) -> bool {
        self.advance(|window_handle| tasks.iter().any(|task| task.window_handle == window_handle))
    }

    pub fn advance_after_enumeration_error(&mut self) -> bool {
        self.advance(|_| true)
    }

    fn advance(&mut self, target_still_present: impl Fn(isize) -> bool) -> bool {
        self.pending.retain_mut(|refresh| {
            if !target_still_present(refresh.window_handle) || refresh.attempts_remaining <= 1 {
                return false;
            }
            refresh.attempts_remaining -= 1;
            true
        });
        !self.pending.is_empty()
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }

    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct TaskListRefresh {
    pending_handles: Vec<isize>,
    close_tracker: CloseRefreshTracker,
    enumerate_requested: bool,
}

impl TaskListRefresh {
    pub fn note_handles(&mut self, handles: impl IntoIterator<Item = isize>) {
        for handle in handles {
            if !self.pending_handles.contains(&handle) {
                self.pending_handles.push(handle);
            }
        }
    }

    pub fn request_enumeration(&mut self) {
        self.enumerate_requested = true;
    }

    pub fn apply_command_outcome(&mut self, outcome: ContextMenuCommandOutcome) {
        if let ContextMenuCommandOutcome::Succeeded { close_window } = outcome {
            self.request_enumeration();
            if let Some(window_handle) = close_window {
                self.note_handles([window_handle]);
            }
        }
    }

    #[must_use]
    pub fn decision(
        &self,
        session_visible: bool,
        is_listed: impl Fn(isize) -> bool,
        context_menu_open: bool,
    ) -> RefreshDecision {
        let has_listed_notice =
            session_visible && self.pending_handles.iter().copied().any(is_listed);
        let has_work =
            self.enumerate_requested || has_listed_notice || self.close_tracker.has_pending();
        if !has_work {
            RefreshDecision::Ignore
        } else if context_menu_open {
            RefreshDecision::Defer
        } else {
            RefreshDecision::Refresh
        }
    }

    pub fn take_pending_handles(&mut self) -> Vec<isize> {
        self.enumerate_requested = false;
        std::mem::take(&mut self.pending_handles)
    }

    #[must_use]
    pub const fn close_tracker(&self) -> &CloseRefreshTracker {
        &self.close_tracker
    }

    pub fn close_tracker_mut(&mut self) -> &mut CloseRefreshTracker {
        &mut self.close_tracker
    }

    pub fn clear_notices(&mut self) {
        self.pending_handles.clear();
        self.enumerate_requested = false;
    }
}

pub fn apply_listed_refresh_batch(refresh: &mut TaskListRefresh, batch: RefreshBatch) {
    if batch.requires_full_enumeration() {
        refresh.request_enumeration();
    }
    refresh.note_handles(batch.handles().iter().copied());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::switcher::SwitchTask;

    #[test]
    fn hide_notifications_coalesce_to_one_post_and_one_handle_batch() {
        let signal = ListedRefreshSignal::new();

        assert_eq!(signal.record(10), RefreshWakeup::PostNow);
        assert_eq!(signal.record(20), RefreshWakeup::AlreadyQueued);
        assert_eq!(signal.record(10), RefreshWakeup::AlreadyQueued);
        assert_eq!(signal.take().handles(), &[10, 20]);
        assert_eq!(signal.record(30), RefreshWakeup::PostNow);
    }

    #[test]
    fn failed_post_is_observed_by_the_ui_without_another_lifecycle_event() {
        let signal = ListedRefreshSignal::new();

        assert_eq!(
            request_listed_refresh(&signal, 10, || false),
            RefreshWakeup::PostNow
        );
        assert!(signal.needs_retry_wakeup());
        assert!(signal.is_dirty());
        assert!(!signal.is_queued());

        let batch = signal.take_retry();
        assert!(batch.is_some());
        let batch = batch.unwrap_or_else(RefreshBatch::empty);
        assert_eq!(batch.handles(), &[10]);
        assert!(!signal.is_dirty());
        assert!(!signal.needs_retry_wakeup());
        assert!(!signal.is_queued());
        assert_eq!(signal.record(20), RefreshWakeup::PostNow);
    }

    #[test]
    fn listed_refresh_survives_when_the_posted_message_cannot_borrow_app() {
        let signal = ListedRefreshSignal::new();
        let mut refresh = TaskListRefresh::default();

        assert_eq!(signal.record(10), RefreshWakeup::PostNow);
        signal.post_failed();
        assert!(signal.is_dirty());
        assert!(!signal.is_queued());
        assert_eq!(
            refresh.decision(true, |_| true, true),
            RefreshDecision::Ignore
        );

        let batch = signal.take_retry();
        assert!(batch.is_some());
        apply_listed_refresh_batch(&mut refresh, batch.unwrap_or_else(RefreshBatch::empty));
        assert!(!signal.is_dirty());
        assert!(!signal.is_queued());
        assert_eq!(
            refresh.decision(true, |_| true, false),
            RefreshDecision::Refresh
        );
        assert_eq!(refresh.take_pending_handles(), vec![10]);
        assert_eq!(signal.record(20), RefreshWakeup::PostNow);
    }

    #[test]
    fn listed_refresh_ingested_during_an_open_menu_defers_mutation_and_allows_a_new_post() {
        let signal = ListedRefreshSignal::new();
        let mut refresh = TaskListRefresh::default();

        assert_eq!(signal.record(10), RefreshWakeup::PostNow);
        apply_listed_refresh_batch(&mut refresh, signal.take());
        assert!(!signal.is_queued());
        assert_eq!(
            refresh.decision(true, |_| true, true),
            RefreshDecision::Defer
        );
        assert_eq!(signal.record(20), RefreshWakeup::PostNow);

        apply_listed_refresh_batch(&mut refresh, signal.take());
        assert_eq!(
            refresh.decision(true, |_| true, false),
            RefreshDecision::Refresh
        );
        assert_eq!(refresh.take_pending_handles(), vec![10, 20]);
        assert_eq!(signal.record(30), RefreshWakeup::PostNow);
    }

    #[test]
    fn slot_overflow_requests_a_full_enumeration() {
        let signal = ListedRefreshSignal::new();
        for handle in 1..=5 {
            signal.record(handle);
        }
        let mut refresh = TaskListRefresh::default();
        apply_listed_refresh_batch(&mut refresh, signal.take());
        assert_eq!(
            refresh.decision(true, |_| false, false),
            RefreshDecision::Refresh
        );
    }

    #[test]
    fn context_menu_defers_then_flushes_affected_windows_as_one_refresh() {
        let mut refresh = TaskListRefresh::default();
        refresh.note_handles([10, 20]);

        assert_eq!(
            refresh.decision(true, |_| true, true),
            RefreshDecision::Defer
        );
        assert_eq!(
            refresh.decision(true, |_| true, false),
            RefreshDecision::Refresh
        );
        assert_eq!(refresh.take_pending_handles(), vec![10, 20]);
        assert_eq!(
            refresh.decision(true, |_| true, false),
            RefreshDecision::Ignore
        );
    }

    #[test]
    fn hidden_sessions_and_unlisted_windows_ignore_lifecycle_events() {
        let mut refresh = TaskListRefresh::default();
        refresh.note_handles([20]);

        assert_eq!(
            refresh.decision(false, |_| true, false),
            RefreshDecision::Ignore
        );
        assert_eq!(
            refresh.decision(true, |handle| handle == 10, false),
            RefreshDecision::Ignore
        );

        refresh.note_handles([10]);
        assert_eq!(
            refresh.decision(true, |handle| handle == 10, false),
            RefreshDecision::Refresh
        );
        assert_eq!(
            refresh.decision(true, |handle| handle == 10, true),
            RefreshDecision::Defer
        );
    }

    #[test]
    fn command_enumeration_runs_once_after_the_context_menu_closes() {
        let mut refresh = TaskListRefresh::default();
        refresh.note_handles([10]);
        refresh.apply_command_outcome(ContextMenuCommandOutcome::Succeeded { close_window: None });

        assert_eq!(
            refresh.decision(true, |_| true, true),
            RefreshDecision::Defer
        );
        assert_eq!(
            refresh.decision(true, |_| true, false),
            RefreshDecision::Refresh
        );
        let _batch = refresh.take_pending_handles();
        assert_eq!(
            refresh.decision(true, |_| true, false),
            RefreshDecision::Ignore
        );
    }

    #[test]
    fn every_context_menu_outcome_preserves_deferred_refresh_work() {
        let outcomes = [
            ContextMenuCommandOutcome::None,
            ContextMenuCommandOutcome::Rejected,
            ContextMenuCommandOutcome::Failed,
            ContextMenuCommandOutcome::Succeeded { close_window: None },
            ContextMenuCommandOutcome::Succeeded {
                close_window: Some(20),
            },
        ];

        for outcome in outcomes {
            let mut refresh = TaskListRefresh::default();
            refresh.note_handles([10]);
            refresh.apply_command_outcome(outcome);

            assert_eq!(
                refresh.decision(true, |_| true, false),
                RefreshDecision::Refresh,
                "deferred refresh was stranded for {outcome:?}"
            );
            let handles = refresh.take_pending_handles();
            assert!(handles.contains(&10));
            if outcome
                == (ContextMenuCommandOutcome::Succeeded {
                    close_window: Some(20),
                })
            {
                assert!(handles.contains(&20));
            }
            assert_eq!(
                refresh.decision(true, |_| true, false),
                RefreshDecision::Ignore
            );
        }
    }

    #[test]
    fn consecutive_closes_keep_independent_follow_up_refreshes() {
        let mut tracker = CloseRefreshTracker::default();

        assert!(tracker.track(10));
        assert!(!tracker.track(20));
        assert!(tracker.reconcile(&[SwitchTask::new(1, 20, "Second", "second")]));
        assert_eq!(tracker.pending.len(), 1);
        assert_eq!(tracker.pending[0].window_handle, 20);
    }

    #[test]
    fn slow_close_rearms_refresh_while_the_target_is_still_present() {
        let mut tracker = CloseRefreshTracker::default();
        assert!(tracker.track(20));
        tracker.pending[0].attempts_remaining = 2;

        let stale_snapshot = [SwitchTask::new(1, 20, "Closing", "editor")];
        assert!(tracker.reconcile(&stale_snapshot));
        assert_eq!(tracker.pending[0].attempts_remaining, 1);
        assert!(!tracker.reconcile(&[]));
        assert!(tracker.pending.is_empty());
    }

    #[test]
    fn external_close_retries_while_the_hwnd_remains_listed() {
        let stale = [
            SwitchTask::new(1, 10, "Browser", "browser"),
            SwitchTask::new(2, 20, "Closing", "editor"),
        ];
        let mut tracker = CloseRefreshTracker::default();
        assert!(tracker.track(20));
        assert!(tracker.reconcile(&stale));
        assert!(!tracker.reconcile(&[SwitchTask::new(1, 10, "Browser", "browser")]));
        assert!(tracker.pending.is_empty());
    }
}
