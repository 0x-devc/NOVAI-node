//! Congestion statistics collection for forecasting.
//!
//! PURPOSE: Collect mempool, block fullness, and fee trends for congestion forecasting.
//! This module is purely observational - it collects data but takes no action.
//!
//! INVARIANTS:
//! - All statistics are read-only observations
//! - All computations use integer math for determinism
//! - No side effects (no parameter changes, no enforcement)
//!
//! FAILURE MODES:
//! - Empty statistics return zero/default values
//! - Insufficient data returns conservative estimates

use crate::stats::RingBuffer;

/// Statistics for congestion trend analysis.
#[derive(Debug)]
pub struct CongestionStats {
    /// Mempool size over last N blocks.
    mempool_sizes: RingBuffer<u64>,

    /// Block fullness percentage (0-100) over last N blocks.
    block_fullness_pct: RingBuffer<u64>,

    /// Total pending transaction value over last N blocks.
    pending_value: RingBuffer<u64>,

    /// Average fee per block over last N blocks.
    avg_fees: RingBuffer<u64>,

    /// Number of blocks observed.
    blocks_observed: u64,

    /// Current block height.
    current_height: u64,

    /// Window size for analysis.
    window_size: usize,
}

impl CongestionStats {
    /// Create new congestion statistics with given window size.
    #[must_use]
    pub fn new(window_size: usize) -> Self {
        let window_size = window_size.max(10);
        Self {
            mempool_sizes: RingBuffer::new(window_size),
            block_fullness_pct: RingBuffer::new(window_size),
            pending_value: RingBuffer::new(window_size),
            avg_fees: RingBuffer::new(window_size),
            blocks_observed: 0,
            current_height: 0,
            window_size,
        }
    }

    /// Record a block observation.
    ///
    /// # Arguments
    /// - `height`: Block height
    /// - `mempool_size`: Current mempool transaction count
    /// - `block_tx_count`: Number of transactions in block
    /// - `max_block_txs`: Maximum transactions allowed in block
    /// - `pending_total_value`: Sum of pending transaction values
    /// - `avg_fee`: Average fee in this block
    pub fn record_block(
        &mut self,
        height: u64,
        mempool_size: u64,
        block_tx_count: u64,
        max_block_txs: u64,
        pending_total_value: u64,
        avg_fee: u64,
    ) {
        self.current_height = height;
        self.blocks_observed += 1;

        self.mempool_sizes.push(mempool_size);

        // Compute block fullness percentage (0-100)
        let fullness = if max_block_txs > 0 {
            (block_tx_count * 100) / max_block_txs
        } else {
            0
        };
        self.block_fullness_pct.push(fullness);

        self.pending_value.push(pending_total_value);
        self.avg_fees.push(avg_fee);
    }

    /// Get current block height.
    #[must_use]
    pub fn current_height(&self) -> u64 {
        self.current_height
    }

    /// Get number of blocks observed.
    #[must_use]
    pub fn blocks_observed(&self) -> u64 {
        self.blocks_observed
    }

    /// Get average mempool size over window.
    #[must_use]
    pub fn avg_mempool_size(&self) -> u64 {
        self.mempool_sizes.average()
    }

    /// Get current mempool size (most recent).
    #[must_use]
    pub fn current_mempool_size(&self) -> u64 {
        self.mempool_sizes.last()
    }

    /// Get mempool growth rate (current vs average).
    /// Returns percentage where 100 = no change, 200 = doubled.
    #[must_use]
    pub fn mempool_growth_pct(&self) -> u64 {
        let avg = self.avg_mempool_size();
        let current = self.current_mempool_size();
        if avg == 0 {
            100 // No change if no baseline
        } else {
            (current * 100) / avg
        }
    }

    /// Get average block fullness percentage (0-100).
    #[must_use]
    pub fn avg_block_fullness(&self) -> u64 {
        self.block_fullness_pct.average()
    }

    /// Get current block fullness (most recent).
    #[must_use]
    pub fn current_block_fullness(&self) -> u64 {
        self.block_fullness_pct.last()
    }

    /// Get p95 block fullness.
    #[must_use]
    pub fn block_fullness_p95(&self) -> u64 {
        self.block_fullness_pct.p95()
    }

    /// Get average pending value.
    #[must_use]
    pub fn avg_pending_value(&self) -> u64 {
        self.pending_value.average()
    }

    /// Get average fee over window.
    #[must_use]
    pub fn avg_fee(&self) -> u64 {
        self.avg_fees.average()
    }

    /// Get p95 fee (high end of fee distribution).
    #[must_use]
    pub fn fee_p95(&self) -> u64 {
        self.avg_fees.p95()
    }

    /// Check if we have enough data for reliable forecasting.
    /// Requires at least 50% of window filled.
    #[must_use]
    pub fn has_sufficient_data(&self) -> bool {
        self.blocks_observed >= (self.window_size / 2) as u64
    }

    /// Reset all statistics.
    pub fn reset(&mut self) {
        self.mempool_sizes.clear();
        self.block_fullness_pct.clear();
        self.pending_value.clear();
        self.avg_fees.clear();
        self.blocks_observed = 0;
        self.current_height = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_empty_stats() {
        let stats = CongestionStats::new(100);
        assert_eq!(stats.blocks_observed(), 0);
        assert_eq!(stats.avg_mempool_size(), 0);
        assert!(!stats.has_sufficient_data());
    }

    #[test]
    fn record_block_updates_stats() {
        let mut stats = CongestionStats::new(10);

        stats.record_block(100, 50, 10, 100, 1000, 5);

        assert_eq!(stats.blocks_observed(), 1);
        assert_eq!(stats.current_height(), 100);
        assert_eq!(stats.current_mempool_size(), 50);
        assert_eq!(stats.current_block_fullness(), 10); // 10/100 = 10%
        assert_eq!(stats.avg_fee(), 5);
    }

    #[test]
    fn mempool_growth_pct_calculation() {
        let mut stats = CongestionStats::new(10);

        // Build baseline of 50
        for _ in 0..5 {
            stats.record_block(1, 50, 10, 100, 1000, 5);
        }

        // Current is 100 = 2x baseline
        stats.record_block(6, 100, 10, 100, 1000, 5);

        // Growth should be 200% (doubled)
        let growth = stats.mempool_growth_pct();
        assert!((150..=250).contains(&growth), "growth={}", growth);
    }

    #[test]
    fn block_fullness_percentage() {
        let mut stats = CongestionStats::new(10);

        // Block with 50 txs out of 100 max = 50%
        stats.record_block(1, 10, 50, 100, 1000, 5);
        assert_eq!(stats.current_block_fullness(), 50);

        // Block with 100 txs out of 100 max = 100%
        stats.record_block(2, 10, 100, 100, 1000, 5);
        assert_eq!(stats.current_block_fullness(), 100);
    }

    #[test]
    fn sufficient_data_threshold() {
        let mut stats = CongestionStats::new(10);

        // Need at least 5 blocks (50% of window)
        for i in 0..4 {
            stats.record_block(i, 50, 10, 100, 1000, 5);
            assert!(!stats.has_sufficient_data());
        }

        stats.record_block(5, 50, 10, 100, 1000, 5);
        assert!(stats.has_sufficient_data());
    }

    #[test]
    fn reset_clears_all() {
        let mut stats = CongestionStats::new(10);

        for i in 0..10 {
            stats.record_block(i, 50, 10, 100, 1000, 5);
        }

        stats.reset();

        assert_eq!(stats.blocks_observed(), 0);
        assert_eq!(stats.current_height(), 0);
        assert_eq!(stats.avg_mempool_size(), 0);
    }

    #[test]
    fn zero_max_block_txs_handles_gracefully() {
        let mut stats = CongestionStats::new(10);
        stats.record_block(1, 50, 10, 0, 1000, 5);
        assert_eq!(stats.current_block_fullness(), 0);
    }

    #[test]
    fn empty_baseline_returns_100_pct() {
        let stats = CongestionStats::new(10);
        assert_eq!(stats.mempool_growth_pct(), 100);
    }
}
