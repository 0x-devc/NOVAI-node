//! Threshold-based spam pattern detection.
//!
//! PURPOSE: Detect spam patterns from collected statistics.
//! This module is purely computational - it analyzes data and returns results.
//!
//! INVARIANTS:
//! - Detection is stateless (depends only on SpamStats snapshot)
//! - All threshold comparisons use integer math for determinism
//! - No side effects (no tx rejection, no peer banning, no mempool mutation)
//! - Returns pure data structs only
//!
//! FAILURE MODES:
//! - Returns empty vec if insufficient observations
//! - Never panics on edge cases (division by zero protected)

use crate::spam_stats::SpamStats;
use novai_types::Address;

/// Threshold configuration for spam detection.
///
/// Values are percentages where 100 = 1x, 200 = 2x, 300 = 3x, etc.
/// This avoids floating point in the detection path.
#[derive(Debug, Clone)]
pub struct SpamThresholds {
    /// High invalid rate threshold: `invalid_pct > threshold` triggers.
    /// Default: 50 (50% rejection rate)
    pub high_invalid_rate_pct: u64,

    /// High tx rate threshold: submissions per window.
    /// Default: 50 (50 txs in observation window)
    pub high_tx_rate_per_window: u64,

    /// Mempool spike threshold: `current * 100 > baseline * threshold_pct`.
    /// Default: 300 (3x baseline)
    pub mempool_spike_threshold_pct: u64,

    /// Low fee flood threshold: count of txs below 10th percentile.
    /// Default: 20 (20 low-fee txs from single sender)
    pub low_fee_flood_count: u64,

    /// Minimum total submissions before flagging a sender.
    /// Default: 5
    pub min_submissions_to_flag: u64,

    /// Minimum observations before detection is active.
    /// Default: 10
    pub min_observations: u64,

    /// Minimum mempool baseline before spike detection.
    /// Default: 10
    pub min_mempool_baseline: u64,
}

impl Default for SpamThresholds {
    fn default() -> Self {
        Self {
            high_invalid_rate_pct: 50,
            high_tx_rate_per_window: 50,
            mempool_spike_threshold_pct: 300,
            low_fee_flood_count: 20,
            min_submissions_to_flag: 5,
            min_observations: 10,
            min_mempool_baseline: 10,
        }
    }
}

/// Types of spam patterns that can be detected.
///
/// This is a pure data struct with no behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpamPatternKind {
    /// Sender has high rate of invalid/rejected transactions.
    HighInvalidRate {
        sender: Address,
        invalid_count: u64,
        total_count: u64,
        rejection_pct: u64,
    },

    /// Sender is submitting transactions at abnormally high rate.
    HighTxRate {
        sender: Address,
        tx_count: u64,
        threshold: u64,
    },

    /// Mempool size spiked significantly above baseline.
    MempoolSpike {
        current_size: u64,
        baseline_size: u64,
    },

    /// Sender flooding with low-fee transactions.
    LowFeeFlood {
        sender: Address,
        low_fee_count: u64,
        threshold_fee: u64,
    },
}

impl SpamPatternKind {
    /// Get a human-readable description of the spam pattern.
    #[must_use]
    pub fn description(&self) -> String {
        match self {
            Self::HighInvalidRate {
                sender,
                invalid_count,
                total_count,
                rejection_pct,
            } => {
                format!(
                    "Sender {:?} has {}% rejection rate ({}/{} txs)",
                    &sender[..4],
                    rejection_pct,
                    invalid_count,
                    total_count
                )
            }
            Self::HighTxRate {
                sender,
                tx_count,
                threshold,
            } => {
                format!(
                    "Sender {:?} submitted {} txs (threshold: {})",
                    &sender[..4],
                    tx_count,
                    threshold
                )
            }
            Self::MempoolSpike {
                current_size,
                baseline_size,
            } => {
                format!(
                    "Mempool spike: {} txs (baseline: {})",
                    current_size, baseline_size
                )
            }
            Self::LowFeeFlood {
                sender,
                low_fee_count,
                threshold_fee,
            } => {
                format!(
                    "Sender {:?} sent {} low-fee txs (threshold: {})",
                    &sender[..4],
                    low_fee_count,
                    threshold_fee
                )
            }
        }
    }

    /// Get the pattern type name for logging/metrics.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::HighInvalidRate { .. } => "high_invalid_rate",
            Self::HighTxRate { .. } => "high_tx_rate",
            Self::MempoolSpike { .. } => "mempool_spike",
            Self::LowFeeFlood { .. } => "low_fee_flood",
        }
    }

    /// Get the affected sender address, if applicable.
    #[must_use]
    pub fn affected_sender(&self) -> Option<Address> {
        match self {
            Self::HighInvalidRate { sender, .. }
            | Self::HighTxRate { sender, .. }
            | Self::LowFeeFlood { sender, .. } => Some(*sender),
            Self::MempoolSpike { .. } => None,
        }
    }
}

/// A detected spam pattern with computed confidence.
///
/// This is a pure data struct - it contains detection results only.
/// It does NOT trigger any action on its own.
#[derive(Debug, Clone)]
pub struct DetectedSpamPattern {
    /// The kind of spam pattern detected.
    pub kind: SpamPatternKind,

    /// Confidence level 0-255.
    /// Higher values indicate more severe spam behavior.
    pub confidence: u8,

    /// Block height when detection occurred.
    pub height: u64,
}

/// Threshold-based spam pattern detector.
///
/// This detector is purely computational. It:
/// - Takes an immutable reference to SpamStats
/// - Returns a list of detected patterns (pure data)
/// - Has NO side effects (does not modify any state)
/// - Does NOT reject transactions or ban peers
#[derive(Debug, Clone)]
pub struct SpamDetector {
    thresholds: SpamThresholds,
}

impl SpamDetector {
    /// Create a new detector with default thresholds.
    #[must_use]
    pub fn new() -> Self {
        Self {
            thresholds: SpamThresholds::default(),
        }
    }

    /// Create a detector with custom thresholds.
    #[must_use]
    pub fn with_thresholds(thresholds: SpamThresholds) -> Self {
        Self { thresholds }
    }

    /// Get current thresholds (read-only).
    #[must_use]
    pub fn thresholds(&self) -> &SpamThresholds {
        &self.thresholds
    }

    /// Run spam detection on current statistics.
    ///
    /// This method is purely computational:
    /// - Takes immutable reference to stats
    /// - Returns pure data (Vec of detected patterns)
    /// - Has NO side effects
    ///
    /// # Arguments
    /// - `stats`: Immutable reference to spam statistics
    /// - `current_height`: Current block height for tagging detections
    ///
    /// # Returns
    /// List of detected spam patterns (may be empty).
    #[must_use]
    pub fn detect(&self, stats: &SpamStats, current_height: u64) -> Vec<DetectedSpamPattern> {
        let mut patterns = Vec::new();

        // Skip detection if insufficient observations
        if stats.observation_count < self.thresholds.min_observations {
            return patterns;
        }

        // Check per-sender patterns
        for (sender, sender_stats) in stats.all_sender_stats() {
            self.check_high_invalid_rate(*sender, sender_stats, current_height, &mut patterns);
            self.check_high_tx_rate(*sender, sender_stats, current_height, &mut patterns);
            self.check_low_fee_flood(*sender, sender_stats, stats, current_height, &mut patterns);
        }

        // Check global patterns
        self.check_mempool_spike(stats, current_height, &mut patterns);

        patterns
    }

    /// Check for high invalid/rejection rate from a sender.
    fn check_high_invalid_rate(
        &self,
        sender: Address,
        sender_stats: &crate::spam_stats::SenderStats,
        height: u64,
        patterns: &mut Vec<DetectedSpamPattern>,
    ) {
        // Skip if not enough submissions
        if sender_stats.total_submitted < self.thresholds.min_submissions_to_flag {
            return;
        }

        let rejection_pct = sender_stats.rejection_rate_pct();

        if rejection_pct > self.thresholds.high_invalid_rate_pct {
            let confidence =
                self.compute_confidence(rejection_pct, self.thresholds.high_invalid_rate_pct);

            patterns.push(DetectedSpamPattern {
                kind: SpamPatternKind::HighInvalidRate {
                    sender,
                    invalid_count: sender_stats.rejected_count(),
                    total_count: sender_stats.total_submitted,
                    rejection_pct,
                },
                confidence,
                height,
            });
        }
    }

    /// Check for high transaction rate from a sender.
    fn check_high_tx_rate(
        &self,
        sender: Address,
        sender_stats: &crate::spam_stats::SenderStats,
        height: u64,
        patterns: &mut Vec<DetectedSpamPattern>,
    ) {
        let tx_count = sender_stats.recent_submission_count();

        if tx_count > self.thresholds.high_tx_rate_per_window {
            let confidence =
                self.compute_rate_confidence(tx_count, self.thresholds.high_tx_rate_per_window);

            patterns.push(DetectedSpamPattern {
                kind: SpamPatternKind::HighTxRate {
                    sender,
                    tx_count,
                    threshold: self.thresholds.high_tx_rate_per_window,
                },
                confidence,
                height,
            });
        }
    }

    /// Check for low-fee transaction flooding.
    fn check_low_fee_flood(
        &self,
        sender: Address,
        sender_stats: &crate::spam_stats::SenderStats,
        stats: &SpamStats,
        height: u64,
        patterns: &mut Vec<DetectedSpamPattern>,
    ) {
        let low_fee_threshold = stats.low_fee_threshold();

        // Skip if no fee baseline established
        if low_fee_threshold == 0 {
            return;
        }

        // Count low-fee txs from this sender
        // We approximate by checking if their min fee is below threshold
        // and they have high submission count
        if sender_stats.min_fee_seen < low_fee_threshold
            && sender_stats.total_submitted > self.thresholds.low_fee_flood_count
        {
            // Estimate low-fee count based on accepted txs
            // (conservative: we can't know exact count without per-tx tracking)
            let estimated_low_fee_count = sender_stats.accepted_count;

            if estimated_low_fee_count >= self.thresholds.low_fee_flood_count {
                let confidence = self.compute_rate_confidence(
                    estimated_low_fee_count,
                    self.thresholds.low_fee_flood_count,
                );

                patterns.push(DetectedSpamPattern {
                    kind: SpamPatternKind::LowFeeFlood {
                        sender,
                        low_fee_count: estimated_low_fee_count,
                        threshold_fee: low_fee_threshold,
                    },
                    confidence,
                    height,
                });
            }
        }
    }

    /// Check for mempool size spike.
    fn check_mempool_spike(
        &self,
        stats: &SpamStats,
        height: u64,
        patterns: &mut Vec<DetectedSpamPattern>,
    ) {
        let baseline = stats.mempool_size_baseline();

        // Skip if baseline too low
        if baseline < self.thresholds.min_mempool_baseline {
            return;
        }

        let current = stats.current_mempool_size();

        // Integer comparison: current * 100 > baseline * threshold_pct
        let lhs = current.saturating_mul(100);
        let rhs = baseline.saturating_mul(self.thresholds.mempool_spike_threshold_pct);

        if lhs > rhs {
            let confidence = self.compute_spike_confidence(
                current,
                baseline,
                self.thresholds.mempool_spike_threshold_pct,
            );

            patterns.push(DetectedSpamPattern {
                kind: SpamPatternKind::MempoolSpike {
                    current_size: current,
                    baseline_size: baseline,
                },
                confidence,
                height,
            });
        }
    }

    /// Compute confidence for percentage-based detection.
    ///
    /// Formula: confidence = min(255, value - threshold + 100)
    fn compute_confidence(&self, value_pct: u64, threshold_pct: u64) -> u8 {
        let raw = value_pct.saturating_sub(threshold_pct).saturating_add(100);
        raw.min(255) as u8
    }

    /// Compute confidence for rate-based detection.
    ///
    /// Formula: confidence = min(255, (value * 100 / threshold) - 100 + 100)
    fn compute_rate_confidence(&self, value: u64, threshold: u64) -> u8 {
        if threshold == 0 {
            return 255;
        }
        let ratio_pct = value.saturating_mul(100) / threshold;
        let raw = ratio_pct.min(255);
        raw as u8
    }

    /// Compute confidence for spike detection.
    ///
    /// Formula: confidence = min(255, (current * 100 / baseline) - threshold_pct + 100)
    fn compute_spike_confidence(&self, current: u64, baseline: u64, threshold_pct: u64) -> u8 {
        if baseline == 0 {
            return 255;
        }
        let ratio_pct = current.saturating_mul(100) / baseline;
        let raw = ratio_pct.saturating_sub(threshold_pct).saturating_add(100);
        raw.min(255) as u8
    }
}

impl Default for SpamDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spam_stats::TxRejectionReason;

    fn make_stats_with_observations(n: u64) -> SpamStats {
        let mut stats = SpamStats::new(100);
        for _ in 0..n {
            stats.record_mempool_size(50);
        }
        stats
    }

    #[test]
    fn detector_skips_insufficient_observations() {
        let detector = SpamDetector::new();
        let stats = make_stats_with_observations(5); // Below min (10)

        let patterns = detector.detect(&stats, 100);
        assert!(patterns.is_empty());
    }

    #[test]
    fn detector_finds_high_invalid_rate() {
        let mut stats = make_stats_with_observations(20);
        let sender = [0x01u8; 32];

        // 2 accepted, 8 rejected = 80% rejection rate
        stats.record_submission(sender, 100, true);
        stats.record_submission(sender, 100, true);
        for _ in 0..8 {
            stats.record_rejection(sender, 10, TxRejectionReason::InvalidSignature);
        }

        let detector = SpamDetector::new();
        let patterns = detector.detect(&stats, 100);

        let invalid_rate = patterns
            .iter()
            .find(|p| matches!(p.kind, SpamPatternKind::HighInvalidRate { .. }));
        assert!(invalid_rate.is_some(), "Should detect high invalid rate");
    }

    #[test]
    fn detector_finds_high_tx_rate() {
        let mut stats = make_stats_with_observations(20);
        let sender = [0x01u8; 32];

        // Submit 60 txs (above 50 threshold)
        for _ in 0..60 {
            stats.record_submission(sender, 100, true);
        }

        let detector = SpamDetector::new();
        let patterns = detector.detect(&stats, 100);

        let high_rate = patterns
            .iter()
            .find(|p| matches!(p.kind, SpamPatternKind::HighTxRate { .. }));
        assert!(high_rate.is_some(), "Should detect high tx rate");
    }

    #[test]
    fn detector_finds_mempool_spike() {
        let mut stats = make_stats_with_observations(20);

        // Build baseline of 50
        for _ in 0..50 {
            stats.record_mempool_size(50);
        }

        // Spike to 200 (4x baseline)
        stats.record_mempool_size(200);

        let detector = SpamDetector::new();
        let patterns = detector.detect(&stats, 100);

        let spike = patterns
            .iter()
            .find(|p| matches!(p.kind, SpamPatternKind::MempoolSpike { .. }));
        assert!(spike.is_some(), "Should detect mempool spike");
    }

    #[test]
    fn detector_no_false_positives_normal_operation() {
        let mut stats = make_stats_with_observations(50);
        let sender = [0x01u8; 32];

        // Normal operation: 10 txs, 1 rejected (10% rate)
        for _ in 0..9 {
            stats.record_submission(sender, 100, true);
        }
        stats.record_rejection(sender, 10, TxRejectionReason::Duplicate);

        // Stable mempool
        for _ in 0..50 {
            stats.record_mempool_size(50);
        }

        let detector = SpamDetector::new();
        let patterns = detector.detect(&stats, 100);

        assert!(
            patterns.is_empty(),
            "Should not detect spam in normal operation"
        );
    }

    #[test]
    fn pattern_descriptions_are_readable() {
        let pattern = SpamPatternKind::HighInvalidRate {
            sender: [0x42; 32],
            invalid_count: 80,
            total_count: 100,
            rejection_pct: 80,
        };

        let desc = pattern.description();
        assert!(desc.contains("80%"));
        assert_eq!(pattern.type_name(), "high_invalid_rate");
    }

    #[test]
    fn affected_sender_extraction() {
        let sender = [0x42; 32];

        let pattern1 = SpamPatternKind::HighTxRate {
            sender,
            tx_count: 100,
            threshold: 50,
        };
        assert_eq!(pattern1.affected_sender(), Some(sender));

        let pattern2 = SpamPatternKind::MempoolSpike {
            current_size: 200,
            baseline_size: 50,
        };
        assert_eq!(pattern2.affected_sender(), None);
    }

    #[test]
    fn confidence_scales_with_severity() {
        let detector = SpamDetector::new();

        // At 60% rejection (10% above 50% threshold)
        let c1 = detector.compute_confidence(60, 50);
        assert!(c1 >= 100 && c1 <= 120, "c1={}", c1);

        // At 90% rejection (40% above threshold)
        let c2 = detector.compute_confidence(90, 50);
        assert!(c2 > c1, "Higher violation should have higher confidence");

        // At extreme values (400% - 50% + 100% = 450% -> capped at 255)
        let c3 = detector.compute_confidence(400, 50);
        assert_eq!(c3, 255, "Extreme should cap at 255");
    }

    #[test]
    fn detection_is_deterministic() {
        let mut stats = make_stats_with_observations(50);
        let sender = [0x01u8; 32];

        for _ in 0..60 {
            stats.record_submission(sender, 100, true);
        }

        let detector = SpamDetector::new();

        let patterns1 = detector.detect(&stats, 100);
        let patterns2 = detector.detect(&stats, 100);

        assert_eq!(patterns1.len(), patterns2.len(), "Must be deterministic");
    }

    #[test]
    fn detector_does_not_mutate_stats() {
        let mut stats = make_stats_with_observations(50);
        let sender = [0x01u8; 32];

        for _ in 0..60 {
            stats.record_submission(sender, 100, true);
        }

        let observation_count_before = stats.observation_count;
        let sender_count_before = stats.sender_count();

        let detector = SpamDetector::new();
        let _ = detector.detect(&stats, 100);

        // Stats should be unchanged
        assert_eq!(stats.observation_count, observation_count_before);
        assert_eq!(stats.sender_count(), sender_count_before);
    }
}
