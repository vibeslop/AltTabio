//! The process a window or app belongs to.

/// Commands that act on a process check the start time as well as the ID, so one aimed at an
/// exited process cannot reach a later process that reused its ID.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProcessIdentity {
    pub id: u32,
    pub started_at: u64,
}

impl ProcessIdentity {
    #[must_use]
    pub const fn new(id: u32, started_at: u64) -> Self {
        Self { id, started_at }
    }
}
