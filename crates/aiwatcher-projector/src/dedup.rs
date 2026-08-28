//! A bounded "have I seen this message id" set.
//!
//! At-least-once means the same event arrives more than once. Most of the
//! pipeline is idempotent by construction — derived span ids, last-write-wins
//! read model — but metrics are not: counting the same 812 prompt tokens twice
//! inflates a cost dashboard, and nothing downstream can tell.
//!
//! Bounded on purpose. An unbounded set is a memory leak with a long fuse, and
//! a redelivery arriving after the window has scrolled past is rare enough, and
//! harmless enough elsewhere, to accept.

use std::collections::{HashSet, VecDeque};

use aiwatcher_core::MessageId;

#[derive(Debug)]
pub struct Deduplicator {
    seen: HashSet<MessageId>,
    order: VecDeque<MessageId>,
    capacity: usize,
}

impl Deduplicator {
    /// `capacity` is how many recent message ids to remember. Size it well
    /// above the broker's redelivery window: at a few thousand events per
    /// second, 100k covers roughly half a minute.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            seen: HashSet::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Records the id and reports whether it is new.
    pub fn admit(&mut self, id: &MessageId) -> bool {
        if self.seen.contains(id) {
            return false;
        }
        if self.order.len() >= self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.seen.remove(&oldest);
        }
        self.seen.insert(id.clone());
        self.order.push_back(id.clone());
        true
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeat_within_the_window_is_rejected() {
        let mut dedup = Deduplicator::new(16);
        let id = MessageId::new("evt-1");
        assert!(dedup.admit(&id), "first delivery");
        assert!(!dedup.admit(&id), "redelivery");
        assert!(dedup.admit(&MessageId::new("evt-2")));
    }

    #[test]
    fn the_window_evicts_oldest_first_and_stays_bounded() {
        let mut dedup = Deduplicator::new(3);
        for index in 0..3 {
            assert!(dedup.admit(&MessageId::new(format!("evt-{index}"))));
        }
        assert_eq!(dedup.len(), 3);

        // Pushes evt-0 out.
        assert!(dedup.admit(&MessageId::new("evt-3")));
        assert_eq!(dedup.len(), 3, "capacity is respected");
        assert!(
            dedup.admit(&MessageId::new("evt-0")),
            "a redelivery past the window is admitted again — the documented trade-off"
        );
        assert!(!dedup.admit(&MessageId::new("evt-3")), "still remembered");
    }

    #[test]
    fn a_zero_capacity_is_clamped_rather_than_dividing_by_zero() {
        let mut dedup = Deduplicator::new(0);
        assert!(dedup.admit(&MessageId::new("evt-1")));
        assert!(!dedup.admit(&MessageId::new("evt-1")));
    }
}
