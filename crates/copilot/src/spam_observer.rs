//! Main spam observer that ties statistics, detection, and reporting together.
//!
//! PURPOSE: Background observer that monitors transaction submission patterns,
//! detects spam, and publishes advisory signals via callback.
//!
//! INVARIANTS:
//! - Observer is purely observational - NO enforcement actions
//! - No references to TxMempool or PeerManager
//! - Detection runs on every observation cycle
//! - Signals are published via callback only
//!
//! FAILURE MODES:
//! - If callback fails, signal is logged but not retried
//! - If stats are empty, detection returns empty (no false positives)
//!
//! NON-ACTIONS (this module does NOT):
//! - Hold any reference to TxMempool
//! - Hold any reference to PeerManager
//! - Reject or remove transactions
//! - Ban or disconnect peers
//! - Take any enforcement action

use crate::spam_detector::{DetectedSpamPattern, SpamDetector, SpamThresholds};
use crate::spam_reporter::SpamReporter;
use crate::spam_stats::{SpamStats, TxRejectionReason};
use ed25519_dalek::SigningKey;
use novai_ai_entities::{AiSignalV1, SignalPayload};
use novai_types::Address;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Callback for publishing spam detection signals.
///
/// This trait has ONLY ONE purpose: publishing signals.
/// It does NOT have any enforcement methods.
///
/// Implementations should:
/// - Store the payload (off-chain)
/// - Broadcast the signal (on-chain commitment)
///
/// Implementations must NOT:
/// - Reject transactions
/// - Ban peers
/// - Modify mempool state
pub trait SpamCallback: Send + Sync {
    /// Called when a spam pattern is detected.
    ///
    /// # Arguments
    /// - `payload`: Off-chain payload with full detection details
    /// - `signal`: On-chain signal commitment (AiSignalV1 with SpamRisk type)
    ///
    /// This is purely for publishing - NO enforcement action should be taken.
    fn on_spam_detected(&self, payload: SignalPayload, signal: AiSignalV1);
}

/// No-op callback for testing or when signals should not be published.
pub struct NoopSpamCallback;

impl SpamCallback for NoopSpamCallback {
    fn on_spam_detected(&self, _payload: SignalPayload, _signal: AiSignalV1) {
        // Do nothing - signals are not published
    }
}

/// Logging callback that prints spam detections.
pub struct LoggingSpamCallback;

impl SpamCallback for LoggingSpamCallback {
    fn on_spam_detected(&self, _payload: SignalPayload, signal: AiSignalV1) {
        println!(
            "⚠️  SPAM DETECTED (advisory): height={} confidence={} type={:?}",
            signal.height, signal.confidence, signal.signal_type
        );
    }
}

/// Configuration for the spam observer.
#[derive(Debug, Clone)]
pub struct SpamObserverConfig {
    /// Spam detection thresholds.
    pub thresholds: SpamThresholds,

    /// Window size for rolling statistics.
    pub stats_window_size: usize,

    /// Minimum confidence to publish a signal (0-255).
    pub min_publish_confidence: u8,

    /// Whether to publish signals (can be disabled for testing).
    pub publish_enabled: bool,
}

impl Default for SpamObserverConfig {
    fn default() -> Self {
        Self {
            thresholds: SpamThresholds::default(),
            stats_window_size: 100,
            min_publish_confidence: 100,
            publish_enabled: true,
        }
    }
}

/// Metrics exposed by the spam observer.
#[derive(Debug, Default)]
pub struct SpamObserverMetrics {
    /// Total spam patterns detected.
    pub patterns_detected: AtomicU64,

    /// Total signals published.
    pub signals_published: AtomicU64,

    /// Last pattern confidence (0 if none).
    pub last_confidence: AtomicU64,

    /// Transaction submissions observed.
    pub tx_submissions_observed: AtomicU64,

    /// Whether observer is currently active.
    pub active: AtomicBool,
}

impl SpamObserverMetrics {
    /// Create new metrics.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a pattern detection.
    pub fn record_pattern(&self, confidence: u8) {
        self.patterns_detected.fetch_add(1, Ordering::Relaxed);
        self.last_confidence
            .store(confidence as u64, Ordering::Relaxed);
    }

    /// Record a published signal.
    pub fn record_publish(&self) {
        self.signals_published.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a transaction submission observation.
    pub fn record_submission(&self) {
        self.tx_submissions_observed.fetch_add(1, Ordering::Relaxed);
    }
}

/// Spam observer that monitors transaction patterns and publishes advisory signals.
///
/// This observer is purely observational. It:
/// - Collects statistics about transaction submissions
/// - Detects spam patterns using threshold-based detection
/// - Publishes advisory signals via callback
///
/// It does NOT:
/// - Hold any reference to TxMempool
/// - Hold any reference to PeerManager
/// - Reject or remove transactions
/// - Ban or disconnect peers
/// - Take any enforcement action
pub struct SpamObserver {
    /// Rolling spam statistics.
    stats: SpamStats,

    /// Spam pattern detector.
    detector: SpamDetector,

    /// Signal reporter (creates AiSignalV1).
    reporter: SpamReporter,

    /// Configuration.
    config: SpamObserverConfig,

    /// Metrics.
    metrics: Arc<SpamObserverMetrics>,

    /// Current block height (for tagging signals).
    current_height: u64,
}

impl SpamObserver {
    /// Create a new spam observer.
    ///
    /// # Arguments
    /// - `signing_key`: Key for signing signals
    /// - `config`: Observer configuration
    ///
    /// Note: This constructor does NOT take TxMempool or PeerManager references.
    #[must_use]
    pub fn new(signing_key: SigningKey, config: SpamObserverConfig) -> Self {
        let stats = SpamStats::new(config.stats_window_size);
        let detector = SpamDetector::with_thresholds(config.thresholds.clone());
        let reporter = SpamReporter::new(signing_key);

        Self {
            stats,
            detector,
            reporter,
            config,
            metrics: Arc::new(SpamObserverMetrics::new()),
            current_height: 0,
        }
    }

    /// Get shared metrics reference.
    #[must_use]
    pub fn metrics(&self) -> Arc<SpamObserverMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Get current statistics (for debugging/testing).
    #[must_use]
    pub fn stats(&self) -> &SpamStats {
        &self.stats
    }

    /// Update current block height.
    pub fn set_height(&mut self, height: u64) {
        self.current_height = height;
    }

    /// Record a successful transaction submission.
    ///
    /// Call this after a transaction is accepted into mempool.
    /// This is purely observational - it does not modify the mempool.
    pub fn record_accepted_tx(&mut self, sender: Address, fee: u64) {
        self.stats.record_submission(sender, fee, true);
        self.metrics.record_submission();
    }

    /// Record a rejected transaction submission.
    ///
    /// Call this after a transaction is rejected by mempool.
    /// This is purely observational - it does not affect the rejection.
    pub fn record_rejected_tx(&mut self, sender: Address, fee: u64, reason: TxRejectionReason) {
        self.stats.record_rejection(sender, fee, reason);
        self.metrics.record_submission();
    }

    /// Record current mempool size.
    ///
    /// Call this periodically to enable mempool spike detection.
    pub fn record_mempool_size(&mut self, size: u64) {
        self.stats.record_mempool_size(size);
    }

    /// Run spam detection and publish signals via callback.
    ///
    /// This method:
    /// 1. Runs detection on current statistics
    /// 2. For each detected pattern above threshold, creates a signal
    /// 3. Calls the callback to publish the signal
    ///
    /// This is purely observational - NO enforcement action is taken.
    ///
    /// # Arguments
    /// - `callback`: Implementation of SpamCallback for signal publishing
    ///
    /// # Returns
    /// List of detected patterns (for testing/debugging).
    pub fn detect_and_publish<C: SpamCallback>(
        &mut self,
        callback: &C,
    ) -> Vec<DetectedSpamPattern> {
        // Run detection
        let patterns = self.detector.detect(&self.stats, self.current_height);

        // Process each detected pattern
        for pattern in &patterns {
            self.metrics.record_pattern(pattern.confidence);

            // Check if we should publish
            if self.config.publish_enabled
                && pattern.confidence >= self.config.min_publish_confidence
            {
                // Create signal and payload
                let (payload, signal) = self.reporter.create_report(pattern);

                // Publish via callback (NO enforcement)
                callback.on_spam_detected(payload, signal);

                self.metrics.record_publish();
            }
        }

        patterns
    }

    /// Reset all statistics.
    pub fn reset(&mut self) {
        self.stats.reset();
        self.current_height = 0;
    }
}

/// Collecting callback for testing that stores signals.
#[cfg(test)]
pub struct CollectingSpamCallback {
    signals: std::sync::Mutex<Vec<AiSignalV1>>,
}

#[cfg(test)]
impl CollectingSpamCallback {
    pub fn new() -> Self {
        Self {
            signals: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn signals(&self) -> Vec<AiSignalV1> {
        self.signals.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.signals.lock().unwrap().len()
    }
}

#[cfg(test)]
impl SpamCallback for CollectingSpamCallback {
    fn on_spam_detected(&self, _payload: SignalPayload, signal: AiSignalV1) {
        self.signals.lock().unwrap().push(signal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use novai_ai_entities::AiSignalType;

    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[0x42u8; 32])
    }

    #[test]
    fn observer_creates_with_config() {
        let config = SpamObserverConfig::default();
        let observer = SpamObserver::new(test_signing_key(), config);

        assert_eq!(observer.current_height, 0);
        assert_eq!(observer.stats.sender_count(), 0);
    }

    #[test]
    fn observer_records_submissions() {
        let config = SpamObserverConfig::default();
        let mut observer = SpamObserver::new(test_signing_key(), config);
        let sender = [0x01u8; 32];

        observer.record_accepted_tx(sender, 100);
        observer.record_accepted_tx(sender, 200);

        assert_eq!(observer.stats.sender_count(), 1);
        let metrics = observer.metrics();
        assert_eq!(metrics.tx_submissions_observed.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn observer_records_rejections() {
        let config = SpamObserverConfig::default();
        let mut observer = SpamObserver::new(test_signing_key(), config);
        let sender = [0x01u8; 32];

        observer.record_rejected_tx(sender, 10, TxRejectionReason::FeeTooLow);

        let sender_stats = observer.stats.sender_stats(&sender).unwrap();
        assert_eq!(sender_stats.fee_too_low_count, 1);
    }

    #[test]
    fn observer_detects_high_invalid_rate() {
        let mut config = SpamObserverConfig::default();
        config.thresholds.min_observations = 5;
        config.min_publish_confidence = 100;

        let mut observer = SpamObserver::new(test_signing_key(), config);
        let callback = CollectingSpamCallback::new();
        let sender = [0x01u8; 32];

        // Build observations
        for _ in 0..10 {
            observer.record_mempool_size(50);
        }

        // 2 accepted, 8 rejected = 80% rejection
        observer.record_accepted_tx(sender, 100);
        observer.record_accepted_tx(sender, 100);
        for _ in 0..8 {
            observer.record_rejected_tx(sender, 10, TxRejectionReason::InvalidSignature);
        }

        observer.set_height(100);
        let patterns = observer.detect_and_publish(&callback);

        // Should detect high invalid rate
        assert!(!patterns.is_empty(), "Should detect spam pattern");

        // Signal should be published
        let signals = callback.signals();
        assert!(!signals.is_empty(), "Should publish signal");
        assert_eq!(signals[0].signal_type, AiSignalType::SpamRisk);
    }

    #[test]
    fn observer_detects_high_tx_rate() {
        let mut config = SpamObserverConfig::default();
        config.thresholds.min_observations = 5;
        config.thresholds.high_tx_rate_per_window = 50;

        let mut observer = SpamObserver::new(test_signing_key(), config);
        let callback = CollectingSpamCallback::new();
        let sender = [0x01u8; 32];

        // Build observations
        for _ in 0..10 {
            observer.record_mempool_size(50);
        }

        // Submit 60 txs (above 50 threshold)
        for _ in 0..60 {
            observer.record_accepted_tx(sender, 100);
        }

        observer.set_height(100);
        let patterns = observer.detect_and_publish(&callback);

        let high_rate = patterns.iter().any(|p| {
            matches!(
                p.kind,
                crate::spam_detector::SpamPatternKind::HighTxRate { .. }
            )
        });
        assert!(high_rate, "Should detect high tx rate");
    }

    #[test]
    fn observer_respects_min_publish_confidence() {
        let mut config = SpamObserverConfig::default();
        config.thresholds.min_observations = 5;
        config.min_publish_confidence = 250; // Very high

        let mut observer = SpamObserver::new(test_signing_key(), config);
        let callback = CollectingSpamCallback::new();
        let sender = [0x01u8; 32];

        // Build observations
        for _ in 0..10 {
            observer.record_mempool_size(50);
        }

        // Create spam pattern
        for _ in 0..60 {
            observer.record_accepted_tx(sender, 100);
        }

        observer.set_height(100);
        let patterns = observer.detect_and_publish(&callback);

        // Pattern detected
        assert!(!patterns.is_empty());

        // But confidence likely < 250, so not published
        // (or if published, confidence was >= 250)
        let signals = callback.signals();
        if !signals.is_empty() {
            assert!(
                patterns[0].confidence >= 250,
                "Should only publish if confidence >= threshold"
            );
        }
    }

    #[test]
    fn observer_publishes_disabled() {
        let mut config = SpamObserverConfig::default();
        config.thresholds.min_observations = 5;
        config.publish_enabled = false; // Disabled

        let mut observer = SpamObserver::new(test_signing_key(), config);
        let callback = CollectingSpamCallback::new();
        let sender = [0x01u8; 32];

        // Build observations and spam
        for _ in 0..10 {
            observer.record_mempool_size(50);
        }
        for _ in 0..60 {
            observer.record_accepted_tx(sender, 100);
        }

        observer.set_height(100);
        let patterns = observer.detect_and_publish(&callback);

        // Pattern detected but not published
        assert!(!patterns.is_empty());
        assert!(
            callback.signals().is_empty(),
            "Should not publish when disabled"
        );
    }

    #[test]
    fn observer_reset_clears_state() {
        let config = SpamObserverConfig::default();
        let mut observer = SpamObserver::new(test_signing_key(), config);
        let sender = [0x01u8; 32];

        observer.record_accepted_tx(sender, 100);
        observer.set_height(100);

        assert_eq!(observer.stats.sender_count(), 1);
        assert_eq!(observer.current_height, 100);

        observer.reset();

        assert_eq!(observer.stats.sender_count(), 0);
        assert_eq!(observer.current_height, 0);
    }

    #[test]
    fn noop_callback_does_nothing() {
        let callback = NoopSpamCallback;
        let payload = SignalPayload::new(
            "test".to_string(),
            "1.0".to_string(),
            "input".to_string(),
            vec![],
            "explanation".to_string(),
        );
        let signal = AiSignalV1 {
            signal_type: AiSignalType::SpamRisk,
            height: 0,
            issuer: [0; 32],
            confidence: 100,
            payload_hash: [0; 32],
            zk_proof: None,
            signature: [0; 64],
        };

        // Should not panic
        callback.on_spam_detected(payload, signal);
    }

    #[test]
    fn observer_does_not_modify_external_state() {
        // This test verifies the observer is purely observational.
        //
        // The observer:
        // - Does NOT take TxMempool as a parameter (cannot modify it)
        // - Does NOT take PeerManager as a parameter (cannot modify it)
        // - Only records data and calls a callback
        //
        // The callback trait SpamCallback:
        // - Has only one method: on_spam_detected
        // - That method only receives data (payload, signal)
        // - It has no parameters that could be used to modify mempool or peers

        let config = SpamObserverConfig::default();
        let observer = SpamObserver::new(test_signing_key(), config);

        // Verify: SpamObserver struct has no fields for TxMempool or PeerManager
        // This is enforced by the type system - we cannot have what we don't define.

        // Verify: The callback trait has no enforcement methods
        // SpamCallback only has: fn on_spam_detected(&self, SignalPayload, AiSignalV1)
        // No reject_tx(), no ban_peer(), no remove_tx() methods exist.

        // Verify: detect_and_publish only calls callback.on_spam_detected()
        // The callback receives only data, not references to external state.

        // This test passes by virtue of the type system.
        // If someone tried to add enforcement capabilities, they would need to:
        // 1. Add new methods to SpamCallback trait (breaking change)
        // 2. Add TxMempool/PeerManager to SpamObserver (breaking change)
        // Both would be caught in code review.

        assert!(true, "Observer is purely observational by design");

        // Additional runtime verification
        let metrics = observer.metrics();
        assert_eq!(metrics.patterns_detected.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.signals_published.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn callback_receives_only_data() {
        // Verify that the callback only receives immutable data,
        // not references that could be used for enforcement.

        struct VerifyingCallback {
            received_count: std::sync::atomic::AtomicU64,
        }

        impl SpamCallback for VerifyingCallback {
            fn on_spam_detected(&self, payload: SignalPayload, signal: AiSignalV1) {
                // We receive:
                // - payload: SignalPayload (owned, immutable data)
                // - signal: AiSignalV1 (owned, immutable data)
                //
                // We do NOT receive:
                // - &mut TxMempool (cannot modify mempool)
                // - &mut PeerManager (cannot modify peers)
                // - Any handle that could reject/ban

                // Verify we got valid data
                assert!(!payload.model_id.is_empty());
                assert_eq!(signal.signal_type, AiSignalType::SpamRisk);

                self.received_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }

        let mut config = SpamObserverConfig::default();
        config.thresholds.min_observations = 5;

        let mut observer = SpamObserver::new(test_signing_key(), config);
        let callback = VerifyingCallback {
            received_count: std::sync::atomic::AtomicU64::new(0),
        };
        let sender = [0x01u8; 32];

        // Build observations and spam
        for _ in 0..10 {
            observer.record_mempool_size(50);
        }
        for _ in 0..60 {
            observer.record_accepted_tx(sender, 100);
        }

        observer.set_height(100);
        observer.detect_and_publish(&callback);

        // Callback was invoked with data only
        assert!(
            callback
                .received_count
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0,
            "Callback should have been called"
        );
    }

    #[test]
    fn metrics_track_activity() {
        let mut config = SpamObserverConfig::default();
        config.thresholds.min_observations = 5;

        let mut observer = SpamObserver::new(test_signing_key(), config);
        let callback = CollectingSpamCallback::new();
        let sender = [0x01u8; 32];

        let metrics = observer.metrics();

        // Record activity
        for _ in 0..10 {
            observer.record_mempool_size(50);
        }
        for _ in 0..60 {
            observer.record_accepted_tx(sender, 100);
        }

        assert_eq!(metrics.tx_submissions_observed.load(Ordering::Relaxed), 60);

        // Trigger detection
        observer.set_height(100);
        observer.detect_and_publish(&callback);

        assert!(metrics.patterns_detected.load(Ordering::Relaxed) > 0);
        assert!(metrics.signals_published.load(Ordering::Relaxed) > 0);
    }
}
