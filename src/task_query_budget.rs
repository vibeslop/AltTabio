//! Work limits for synchronous task snapshots, independent of native window APIs.

use std::time::Duration;

pub const MAX_TOP_LEVEL_WINDOWS: usize = 1_024;
pub const MAX_TASKS: usize = 256;
pub const MAX_CHILD_WINDOWS: usize = 1_024;
pub const MAX_CHILDREN_PER_FRAME: usize = 128;
pub const MAX_TITLE_UNITS: usize = 1_024;
pub const SNAPSHOT_TIME_LIMIT: Duration = Duration::from_millis(50);
const MAX_ICON_MESSAGES: usize = 32;
const ICON_MESSAGE_TIME_LIMIT: Duration = Duration::from_millis(10);

/// A deadline to stop starting work, not a guarantee about native call completion time.
#[must_use]
pub fn can_query_snapshot(elapsed: Duration) -> bool {
    elapsed < SNAPSHOT_TIME_LIMIT
}

#[must_use]
pub fn title_buffer_capacity(reported_length: i32) -> usize {
    usize::try_from(reported_length)
        .unwrap_or_default()
        .min(MAX_TITLE_UNITS)
        + 1
}

/// Shared by every window and icon size in one snapshot. Charge sends before calling native code,
/// including immediate replies, and account for actual time because native timeouts may overshoot.
pub struct IconQueryBudget {
    remaining_messages: usize,
    elapsed: Duration,
}

impl Default for IconQueryBudget {
    fn default() -> Self {
        Self {
            remaining_messages: MAX_ICON_MESSAGES,
            elapsed: Duration::ZERO,
        }
    }
}

impl IconQueryBudget {
    pub fn next_timeout_ms(&mut self) -> Option<u32> {
        if self.remaining_messages == 0 || self.elapsed >= ICON_MESSAGE_TIME_LIMIT {
            return None;
        }
        self.remaining_messages -= 1;
        Some(1)
    }

    pub fn record_elapsed(&mut self, elapsed: Duration) {
        self.elapsed = self.elapsed.saturating_add(elapsed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_of_windows_cannot_multiply_icon_timeout_requests() {
        let mut budget = IconQueryBudget::default();
        let mut total_timeout_ms = 0;
        for _window in 0..10_000 {
            for _size in 0..3 {
                if let Some(timeout) = budget.next_timeout_ms() {
                    total_timeout_ms += timeout;
                }
            }
        }
        assert_eq!(total_timeout_ms, 32);
    }

    #[test]
    fn timeout_time_is_shared_across_windows_and_stops_at_the_boundary() {
        let mut budget = IconQueryBudget::default();
        for _window in 0..10 {
            assert_eq!(budget.next_timeout_ms(), Some(1));
            budget.record_elapsed(Duration::from_millis(1));
        }
        assert_eq!(budget.next_timeout_ms(), None);
    }

    #[test]
    fn one_overshooting_native_call_prevents_further_sends() {
        let mut budget = IconQueryBudget::default();
        assert_eq!(budget.next_timeout_ms(), Some(1));
        budget.record_elapsed(Duration::from_millis(20));
        assert_eq!(budget.next_timeout_ms(), None);
        assert_eq!(IconQueryBudget::default().next_timeout_ms(), Some(1));
    }

    #[test]
    fn snapshot_work_stops_at_the_deadline() {
        assert!(can_query_snapshot(Duration::from_millis(49)));
        assert!(!can_query_snapshot(SNAPSHOT_TIME_LIMIT));
        assert!(!can_query_snapshot(Duration::MAX));
    }

    #[test]
    fn foreign_title_lengths_cannot_control_allocation_size() {
        for length in [i32::MIN, -1, 0] {
            assert_eq!(title_buffer_capacity(length), 1);
        }
        assert_eq!(title_buffer_capacity(25), 26);
        assert_eq!(title_buffer_capacity(i32::MAX), MAX_TITLE_UNITS + 1);
    }
}
