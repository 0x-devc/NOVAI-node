//! Rolling statistics for spam pattern detection.
//!
//! PURPOSE: Collect per-sender and mempool-wide statistics for spam detection.
//! This module is purely observational - it collects data but takes no action.
//!
//! INVARIANTS:
//! - All statistics are read-only observations
//! - No mutable references to mempool or peer manager
//! - No side effects (no tx rejection, no peer banning)
//! - All computations use integer math for determinism
//!
//! FAILURE MODES:
//! - Empty statistics return zero/default values
//! - HashMap growth is bounded by sender count (natural limit)

use crate::stats::RingBuffer;
use novai_types::Address;
use std::collections::HashMap;

/// Statistics for a single sender's transaction behavior.
#[derive(Debug, Clone)]
pub struct SenderStats {
    /// Total transactions submitted (accepted + rejected).
    pub total_submitted: u64,

    /// Transactions that were accepted into mempool.
    pub accepted_count: u64,

    /// Transactions rejected for invalid signature.
    pub invalid_sig_count: u64,

    /// Transactions rejected for nonce too low.
    pub nonce_too_low_count: u64,

    /// Transactions rejected for fee too low.
    pub fee_too_low_count: u64,

    /// Transactions rejected as duplicates.
    pub duplicate_count: u64,

    /// Rolling window of submission timestamps (as observation index).
    /// Used to detect burst patterns.
    pub submission_times: RingBuffer<u64>,

    /// Lowest fee seen from this sender.
    pub min_fee_seen: u64,

    /// Highest fee seen from this sender.
    pub max_fee_seen: u64,
}

impl SenderStats {
    /// Create new sender statistics with given window size.
    #[must_use]
    pub fn new(window_size: usize) -> Self {
        Self {
            total_submitted: 0,
            accepted_count: 0,
            invalid_sig_count: 0,
            nonce_too_low_count: 0,
            fee_too_low_count: 0,
            duplicate_count: 0,
            submission_times: RingBuffer::new(window_size.max(10)),
            min_fee_seen: u64::MAX,
            max_fee_seen: 0,
        }
    }

    /// Get count of rejected transactions (all rejection reasons).
    #[must_use]
    pub fn rejected_count(&self) -> u64 {
        self.invalid_sig_count
            + self.nonce_too_low_count
            + self.fee_too_low_count
            + self.duplicate_count
    }

    /// Get rejection rate as percentage (0-100).
    /// Returns 0 if no submissions.
    #[must_use]
    pub fn rejection_rate_pct(&self) -> u64 {
        if self.total_submitted == 0 {
            return 0;
        }
        (self.rejected_count() * 100) / self.total_submitted
    }

    /// Get submission rate (txs in current window).
    #[must_use]
    pub fn recent_submission_count(&self) -> u64 {
        self.submission_times.len() as u64
    }
}

/// Reason a transaction was rejected (for statistics only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxRejectionReason {
    InvalidSignature,
    NonceTooLow,
    FeeTooLow,
    Duplicate,
    AddressMismatch,
    Other,
}

/// Aggregated spam detection statistics.
///
/// This struct is purely observational. It collects data from
/// transaction submission events but does not affect mempool behavior.
#[derive(Debug)]
pub struct SpamStats {
    /// Per-sender statistics.
    sender_stats: HashMap<Address, SenderStats>,

    /// Global mempool size history.
    mempool_size_history: RingBuffer<u64>,

    /// Global fee distribution (recent accepted tx fees).
    recent_fees: RingBuffer<u64>,

    /// Current observation index (monotonically increasing).
    observation_index: u64,

    /// Window size for per-sender tracking.
    window_size: usize,

    /// Total observations recorded.
    pub observation_count: u64,

    /// Last recorded mempool size.
    pub last_mempool_size: u64,
}

impl SpamStats {
    /// Create new spam statistics with given window size.
    #[must_use]
    pub fn new(window_size: usize) -> Self {
        let window_size = window_size.max(10);
        Self {
            sender_stats: HashMap::new(),
            mempool_size_history: RingBuffer::new(window_size),
            recent_fees: RingBuffer::new(window_size),
            observation_index: 0,
            window_size,
            observation_count: 0,
            last_mempool_size: 0,
        }
    }

    /// Record a transaction submission event.
    ///
    /// This is called after each mempool.insert() attempt, regardless of outcome.
    /// It does NOT modify the mempool - purely observational.
    pub fn record_submission(&mut self, sender: Address, fee: u64, accepted: bool) {
        self.observation_index += 1;

        let stats = self
            .sender_stats
            .entry(sender)
            .or_insert_with(|| SenderStats::new(self.window_size));

        stats.total_submitted += 1;
        stats.submission_times.push(self.observation_index);

        if accepted {
            stats.accepted_count += 1;
            self.recent_fees.push(fee);
        }

        // Track fee range
        stats.min_fee_seen = stats.min_fee_seen.min(fee);
        stats.max_fee_seen = stats.max_fee_seen.max(fee);
    }

    /// Record a transaction rejection with reason.
    ///
    /// Called after mempool.insert() returns an error.
    /// Does NOT modify the mempool - purely observational.
    pub fn record_rejection(&mut self, sender: Address, fee: u64, reason: TxRejectionReason) {
        self.observation_index += 1;

        let stats = self
            .sender_stats
            .entry(sender)
            .or_insert_with(|| SenderStats::new(self.window_size));

        stats.total_submitted += 1;
        stats.submission_times.push(self.observation_index);

        match reason {
            TxRejectionReason::InvalidSignature => stats.invalid_sig_count += 1,
            TxRejectionReason::NonceTooLow => stats.nonce_too_low_count += 1,
            TxRejectionReason::FeeTooLow => stats.fee_too_low_count += 1,
            TxRejectionReason::Duplicate => stats.duplicate_count += 1,
            TxRejectionReason::AddressMismatch | TxRejectionReason::Other => {
                // Count as invalid signature for simplicity
                stats.invalid_sig_count += 1;
            }
        }

        // Track fee range even for rejected txs
        stats.min_fee_seen = stats.min_fee_seen.min(fee);
        stats.max_fee_seen = stats.max_fee_seen.max(fee);
    }

    /// Record current mempool size observation.
    pub fn record_mempool_size(&mut self, size: u64) {
        self.mempool_size_history.push(size);
        self.last_mempool_size = size;
        self.observation_count += 1;
    }

    /// Get statistics for a specific sender (read-only).
    #[must_use]
    pub fn sender_stats(&self, sender: &Address) -> Option<&SenderStats> {
        self.sender_stats.get(sender)
    }

    /// Iterate over all sender statistics (read-only).
    pub fn all_sender_stats(&self) -> impl Iterator<Item = (&Address, &SenderStats)> {
        self.sender_stats.iter()
    }

    /// Get number of unique senders tracked.
    #[must_use]
    pub fn sender_count(&self) -> usize {
        self.sender_stats.len()
    }

    /// Get mempool size baseline (average from history).
    #[must_use]
    pub fn mempool_size_baseline(&self) -> u64 {
        self.mempool_size_history.average()
    }

    /// Get current mempool size.
    #[must_use]
    pub fn current_mempool_size(&self) -> u64 {
        self.last_mempool_size
    }

    /// Get fee percentile (0-100).
    /// Returns 0 if no fee data.
    #[must_use]
    pub fn fee_percentile(&self, percentile: u64) -> u64 {
        if self.recent_fees.is_empty() {
            return 0;
        }

        let mut sorted: Vec<u64> = self.recent_fees.iter().copied().collect();
        sorted.sort_unstable();

        let idx = ((sorted.len() as u64 * percentile) / 100) as usize;
        let idx = idx.min(sorted.len().saturating_sub(1));
        sorted[idx]
    }

    /// Get 10th percentile fee (low-fee threshold).
    #[must_use]
    pub fn low_fee_threshold(&self) -> u64 {
        self.fee_percentile(10)
    }

    /// Get median fee.
    #[must_use]
    pub fn median_fee(&self) -> u64 {
        self.fee_percentile(50)
    }

    /// Reset all statistics.
    pub fn reset(&mut self) {
        self.sender_stats.clear();
        self.mempool_size_history.clear();
        self.recent_fees.clear();
        self.observation_index = 0;
        self.observation_count = 0;
        self.last_mempool_size = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_stats_tracks_submissions() {
        let mut stats = SenderStats::new(100);

        stats.total_submitted = 10;
        stats.accepted_count = 7;
        stats.invalid_sig_count = 2;
        stats.nonce_too_low_count = 1;

        assert_eq!(stats.rejected_count(), 3);
        assert_eq!(stats.rejection_rate_pct(), 30);
    }

    #[test]
    fn sender_stats_empty_returns_zero() {
        let stats = SenderStats::new(100);
        assert_eq!(stats.rejection_rate_pct(), 0);
        assert_eq!(stats.rejected_count(), 0);
    }

    #[test]
    fn spam_stats_records_submissions() {
        let mut stats = SpamStats::new(100);
        let sender = [0x01u8; 32];

        stats.record_submission(sender, 100, true);
        stats.record_submission(sender, 50, true);
        stats.record_rejection(sender, 10, TxRejectionReason::FeeTooLow);

        let sender_stats = stats.sender_stats(&sender).unwrap();
        assert_eq!(sender_stats.total_submitted, 3);
        assert_eq!(sender_stats.accepted_count, 2);
        assert_eq!(sender_stats.fee_too_low_count, 1);
    }

    #[test]
    fn spam_stats_tracks_multiple_senders() {
        let mut stats = SpamStats::new(100);
        let sender1 = [0x01u8; 32];
        let sender2 = [0x02u8; 32];

        stats.record_submission(sender1, 100, true);
        stats.record_submission(sender2, 200, true);

        assert_eq!(stats.sender_count(), 2);
    }

    #[test]
    fn spam_stats_fee_percentiles() {
        let mut stats = SpamStats::new(100);
        let sender = [0x01u8; 32];

        // Record fees: 10, 20, 30, ..., 100
        for fee in (10..=100).step_by(10) {
            stats.record_submission(sender, fee, true);
        }

        // 10th percentile should be around 10-20
        let p10 = stats.fee_percentile(10);
        assert!(p10 <= 20, "p10={}", p10);

        // Median should be around 50-60
        let median = stats.median_fee();
        assert!((40..=60).contains(&median), "median={}", median);
    }

    #[test]
    fn spam_stats_mempool_baseline() {
        let mut stats = SpamStats::new(100);

        for _ in 0..10 {
            stats.record_mempool_size(50);
        }

        assert_eq!(stats.mempool_size_baseline(), 50);
        assert_eq!(stats.current_mempool_size(), 50);

        stats.record_mempool_size(100);
        assert_eq!(stats.current_mempool_size(), 100);
    }

    #[test]
    fn spam_stats_reset_clears_all() {
        let mut stats = SpamStats::new(100);
        let sender = [0x01u8; 32];

        stats.record_submission(sender, 100, true);
        stats.record_mempool_size(50);

        stats.reset();

        assert_eq!(stats.sender_count(), 0);
        assert_eq!(stats.observation_count, 0);
    }
}
