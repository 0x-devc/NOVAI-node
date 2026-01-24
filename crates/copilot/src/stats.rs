//! Rolling window statistics for chain observation.
//!
//! PURPOSE: Collect and compute statistics over a sliding window of observations.
//!
//! INVARIANTS:
//! - RingBuffer maintains fixed capacity, drops oldest on overflow
//! - All statistics computed using integer math where possible
//! - Statistics are reset-safe (handle empty buffers gracefully)
//!
//! FAILURE MODES:
//! - Empty buffer returns zero/default for all statistics

use novai_types::Address;
use std::collections::HashMap;

/// Fixed-capacity ring buffer for rolling statistics.
///
/// Stores the most recent N values, automatically evicting the oldest
/// when capacity is exceeded.
#[derive(Debug, Clone)]
pub struct RingBuffer<T> {
    data: Vec<T>,
    capacity: usize,
    write_pos: usize,
    len: usize,
}

impl<T: Clone + Default> RingBuffer<T> {
    /// Create a new ring buffer with the given capacity.
    ///
    /// # Panics
    /// Panics if capacity is zero.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "RingBuffer capacity must be > 0");
        Self {
            data: vec![T::default(); capacity],
            capacity,
            write_pos: 0,
            len: 0,
        }
    }

    /// Push a value, evicting the oldest if at capacity.
    pub fn push(&mut self, value: T) {
        self.data[self.write_pos] = value;
        self.write_pos = (self.write_pos + 1) % self.capacity;
        if self.len < self.capacity {
            self.len += 1;
        }
    }

    /// Number of values currently stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// True if no values stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Iterate over all stored values (oldest to newest).
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        let start = if self.len < self.capacity {
            0
        } else {
            self.write_pos
        };

        (0..self.len).map(move |i| &self.data[(start + i) % self.capacity])
    }

    /// Clear all values.
    pub fn clear(&mut self) {
        self.len = 0;
        self.write_pos = 0;
    }
}

impl RingBuffer<u64> {
    /// Compute the sum of all values.
    #[must_use]
    pub fn sum(&self) -> u64 {
        self.iter().sum()
    }

    /// Compute the average (integer division).
    /// Returns 0 if empty.
    #[must_use]
    pub fn average(&self) -> u64 {
        if self.is_empty() {
            0
        } else {
            self.sum() / self.len as u64
        }
    }

    /// Compute the p95 value (95th percentile).
    /// Returns 0 if empty.
    #[must_use]
    pub fn p95(&self) -> u64 {
        if self.is_empty() {
            return 0;
        }

        let mut sorted: Vec<u64> = self.iter().copied().collect();
        sorted.sort_unstable();

        // p95 index: 95% of the way through
        let idx = (sorted.len() * 95) / 100;
        let idx = idx.min(sorted.len().saturating_sub(1));
        sorted[idx]
    }

    /// Get the most recent value, or 0 if empty.
    #[must_use]
    pub fn last(&self) -> u64 {
        if self.is_empty() {
            0
        } else {
            let idx = if self.write_pos == 0 {
                self.capacity - 1
            } else {
                self.write_pos - 1
            };
            self.data[idx]
        }
    }

    /// Get the oldest value, or 0 if empty.
    #[must_use]
    pub fn first(&self) -> u64 {
        if self.is_empty() {
            0
        } else if self.len < self.capacity {
            self.data[0]
        } else {
            self.data[self.write_pos]
        }
    }
}

/// Rolling statistics for chain observation.
///
/// Tracks per-validator and global metrics over a sliding window.
#[derive(Debug)]
pub struct ChainStats {
    /// Number of proposals observed per validator.
    pub proposals_by_validator: HashMap<Address, u64>,

    /// Number of missed blocks (expected but not proposed) per validator.
    pub missed_blocks_by_validator: HashMap<Address, u64>,

    /// Vote delays in milliseconds (time from proposal to vote).
    pub vote_delays_ms: RingBuffer<u64>,

    /// Peer count history.
    pub peer_count_history: RingBuffer<u64>,

    /// Mempool size history.
    pub mempool_size_history: RingBuffer<u64>,

    /// Last observed committed height.
    pub last_committed_height: u64,

    /// Last observed peer count (for churn detection).
    pub last_peer_count: u64,

    /// Total observation cycles completed.
    pub observation_count: u64,
}

impl ChainStats {
    /// Create new chain statistics with the given window size.
    ///
    /// # Arguments
    /// - `window_size`: Number of observations to keep in rolling buffers
    #[must_use]
    pub fn new(window_size: usize) -> Self {
        let window_size = window_size.max(10); // Minimum 10 samples
        Self {
            proposals_by_validator: HashMap::new(),
            missed_blocks_by_validator: HashMap::new(),
            vote_delays_ms: RingBuffer::new(window_size),
            peer_count_history: RingBuffer::new(window_size),
            mempool_size_history: RingBuffer::new(window_size),
            last_committed_height: 0,
            last_peer_count: 0,
            observation_count: 0,
        }
    }

    /// Record a block proposal from a validator.
    pub fn record_proposal(&mut self, proposer: Address) {
        *self.proposals_by_validator.entry(proposer).or_insert(0) += 1;
    }

    /// Record a missed block (expected proposer didn't propose).
    pub fn record_missed_block(&mut self, expected_proposer: Address) {
        *self
            .missed_blocks_by_validator
            .entry(expected_proposer)
            .or_insert(0) += 1;
    }

    /// Record a vote delay in milliseconds.
    pub fn record_vote_delay(&mut self, delay_ms: u64) {
        self.vote_delays_ms.push(delay_ms);
    }

    /// Record current peer count.
    pub fn record_peer_count(&mut self, count: u64) {
        self.peer_count_history.push(count);
        self.last_peer_count = count;
    }

    /// Record current mempool size.
    pub fn record_mempool_size(&mut self, size: u64) {
        self.mempool_size_history.push(size);
    }

    /// Update committed height and increment observation count.
    pub fn record_observation(&mut self, committed_height: u64) {
        self.last_committed_height = committed_height;
        self.observation_count += 1;
    }

    /// Get total missed blocks for a validator.
    #[must_use]
    pub fn missed_blocks_for(&self, validator: &Address) -> u64 {
        self.missed_blocks_by_validator
            .get(validator)
            .copied()
            .unwrap_or(0)
    }

    /// Get average missed blocks across all validators.
    /// Returns 0 if no validators tracked.
    #[must_use]
    pub fn average_missed_blocks(&self) -> u64 {
        if self.missed_blocks_by_validator.is_empty() {
            return 0;
        }
        let total: u64 = self.missed_blocks_by_validator.values().sum();
        total / self.missed_blocks_by_validator.len() as u64
    }

    /// Get p95 vote delay in milliseconds.
    #[must_use]
    pub fn vote_delay_p95(&self) -> u64 {
        self.vote_delays_ms.p95()
    }

    /// Get average peer count from history.
    #[must_use]
    pub fn peer_count_baseline(&self) -> u64 {
        self.peer_count_history.average()
    }

    /// Compute peer churn (absolute change from baseline).
    #[must_use]
    pub fn peer_churn(&self) -> u64 {
        let baseline = self.peer_count_baseline();
        self.last_peer_count.abs_diff(baseline)
    }

    /// Get average mempool size from history.
    #[must_use]
    pub fn mempool_size_baseline(&self) -> u64 {
        self.mempool_size_history.average()
    }

    /// Get current mempool size (most recent observation).
    #[must_use]
    pub fn current_mempool_size(&self) -> u64 {
        self.mempool_size_history.last()
    }

    /// Reset all statistics.
    pub fn reset(&mut self) {
        self.proposals_by_validator.clear();
        self.missed_blocks_by_validator.clear();
        self.vote_delays_ms.clear();
        self.peer_count_history.clear();
        self.mempool_size_history.clear();
        self.last_committed_height = 0;
        self.last_peer_count = 0;
        self.observation_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_push_and_len() {
        let mut buf: RingBuffer<u64> = RingBuffer::new(3);
        assert!(buf.is_empty());

        buf.push(1);
        assert_eq!(buf.len(), 1);

        buf.push(2);
        buf.push(3);
        assert_eq!(buf.len(), 3);

        // Overflow - should evict oldest
        buf.push(4);
        assert_eq!(buf.len(), 3);
    }

    #[test]
    fn ring_buffer_iter_order() {
        let mut buf: RingBuffer<u64> = RingBuffer::new(3);
        buf.push(1);
        buf.push(2);
        buf.push(3);

        let values: Vec<u64> = buf.iter().copied().collect();
        assert_eq!(values, vec![1, 2, 3]);

        // After overflow
        buf.push(4);
        let values: Vec<u64> = buf.iter().copied().collect();
        assert_eq!(values, vec![2, 3, 4]);
    }

    #[test]
    fn ring_buffer_sum_and_average() {
        let mut buf: RingBuffer<u64> = RingBuffer::new(4);
        buf.push(10);
        buf.push(20);
        buf.push(30);
        buf.push(40);

        assert_eq!(buf.sum(), 100);
        assert_eq!(buf.average(), 25);
    }

    #[test]
    fn ring_buffer_p95() {
        let mut buf: RingBuffer<u64> = RingBuffer::new(100);
        for i in 1..=100 {
            buf.push(i);
        }

        // p95 of 1-100 should be 95 or 96
        let p95 = buf.p95();
        assert!(p95 >= 95 && p95 <= 96);
    }

    #[test]
    fn ring_buffer_last_and_first() {
        let mut buf: RingBuffer<u64> = RingBuffer::new(3);
        buf.push(10);
        buf.push(20);
        buf.push(30);

        assert_eq!(buf.first(), 10);
        assert_eq!(buf.last(), 30);

        buf.push(40); // Evicts 10
        assert_eq!(buf.first(), 20);
        assert_eq!(buf.last(), 40);
    }

    #[test]
    fn ring_buffer_empty_returns_zero() {
        let buf: RingBuffer<u64> = RingBuffer::new(10);
        assert_eq!(buf.sum(), 0);
        assert_eq!(buf.average(), 0);
        assert_eq!(buf.p95(), 0);
        assert_eq!(buf.first(), 0);
        assert_eq!(buf.last(), 0);
    }

    #[test]
    fn chain_stats_record_proposal() {
        let mut stats = ChainStats::new(100);
        let validator = [0x42u8; 32];

        stats.record_proposal(validator);
        stats.record_proposal(validator);

        assert_eq!(stats.proposals_by_validator.get(&validator), Some(&2));
    }

    #[test]
    fn chain_stats_missed_blocks() {
        let mut stats = ChainStats::new(100);
        let v1 = [0x01u8; 32];
        let v2 = [0x02u8; 32];

        stats.record_missed_block(v1);
        stats.record_missed_block(v1);
        stats.record_missed_block(v2);

        assert_eq!(stats.missed_blocks_for(&v1), 2);
        assert_eq!(stats.missed_blocks_for(&v2), 1);
        assert_eq!(stats.average_missed_blocks(), 1); // (2+1)/2 = 1
    }

    #[test]
    fn chain_stats_peer_churn() {
        let mut stats = ChainStats::new(10);

        // Build baseline
        for _ in 0..10 {
            stats.record_peer_count(4);
        }

        assert_eq!(stats.peer_count_baseline(), 4);
        assert_eq!(stats.peer_churn(), 0);

        // Record a change
        stats.record_peer_count(6);
        assert_eq!(stats.peer_churn(), 2); // |6 - 4| = 2
    }

    #[test]
    fn chain_stats_reset() {
        let mut stats = ChainStats::new(10);
        let validator = [0x42u8; 32];

        stats.record_proposal(validator);
        stats.record_peer_count(5);
        stats.record_observation(100);

        stats.reset();

        assert!(stats.proposals_by_validator.is_empty());
        assert_eq!(stats.last_committed_height, 0);
        assert_eq!(stats.observation_count, 0);
    }
}
