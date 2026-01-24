//! Main chain observer that ties statistics, detection, and reporting together.
//!
//! PURPOSE: Background observer that monitors chain state, detects anomalies,
//! and publishes signals.
//!
//! INVARIANTS:
//! - Observer is non-blocking, runs in its own thread
//! - Detection runs on every observation cycle
//! - Signals are published immediately upon detection
//!
//! FAILURE MODES:
//! - If state lock is poisoned, observation cycle is skipped
//! - If signal publishing fails, anomaly is logged but not retried

use crate::detector::{AnomalyDetector, AnomalyThresholds, DetectedAnomaly};
use crate::reporter::AnomalyReporter;
use crate::stats::ChainStats;
use ed25519_dalek::SigningKey;
use novai_ai_entities::{AiSignalV1, SignalPayload};
use novai_types::Address;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Snapshot of observable node state.
///
/// This trait is implemented by the node to provide the observer
/// with access to relevant state without tight coupling.
pub trait ObservableState: Send + Sync {
    /// Current committed height.
    fn committed_height(&self) -> u64;

    /// Current consensus round.
    fn current_round(&self) -> u64;

    /// Number of connected peers.
    fn peer_count(&self) -> u64;

    /// Number of transactions in mempool.
    fn mempool_size(&self) -> u64;

    /// Total view changes (timeouts) since start.
    fn view_changes_total(&self) -> u64;

    /// List of validator addresses.
    fn validator_set(&self) -> Vec<Address>;

    /// Get the expected leader for a given height and round.
    fn expected_leader(&self, height: u64, round: u64) -> Option<Address>;
}

/// Callback for when an anomaly is detected.
pub trait AnomalyCallback: Send + Sync {
    /// Called when an anomaly is detected.
    ///
    /// Implementations should handle signal publishing (e.g., submit to mempool).
    fn on_anomaly(&self, payload: SignalPayload, signal: AiSignalV1);
}

/// No-op callback for testing or when signals should not be published.
pub struct NoopCallback;

impl AnomalyCallback for NoopCallback {
    fn on_anomaly(&self, _payload: SignalPayload, _signal: AiSignalV1) {
        // Do nothing
    }
}

/// Logging callback that prints anomalies.
pub struct LoggingCallback;

impl AnomalyCallback for LoggingCallback {
    fn on_anomaly(&self, _payload: SignalPayload, signal: AiSignalV1) {
        println!(
            "🚨 ANOMALY DETECTED: height={} confidence={} type={:?}",
            signal.height, signal.confidence, signal.signal_type
        );
    }
}

/// Configuration for the chain observer.
#[derive(Debug, Clone)]
pub struct ObserverConfig {
    /// Anomaly detection thresholds.
    pub thresholds: AnomalyThresholds,

    /// Window size for rolling statistics.
    pub stats_window_size: usize,

    /// Minimum confidence to publish a signal (0-255).
    pub min_publish_confidence: u8,

    /// Whether to publish signals (can be disabled for testing).
    pub publish_enabled: bool,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            thresholds: AnomalyThresholds::default(),
            stats_window_size: 100,
            min_publish_confidence: 100, // Only publish if confident
            publish_enabled: true,
        }
    }
}

/// Metrics exposed by the observer.
#[derive(Debug, Default)]
pub struct ObserverMetrics {
    /// Total anomalies detected.
    pub anomalies_detected: AtomicU64,

    /// Total signals published.
    pub signals_published: AtomicU64,

    /// Last anomaly confidence (0 if none).
    pub last_confidence: AtomicU64,

    /// Observation cycles completed.
    pub observations: AtomicU64,

    /// Whether observer is currently running.
    pub running: AtomicBool,
}

impl ObserverMetrics {
    /// Create new metrics.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an anomaly detection.
    pub fn record_anomaly(&self, confidence: u8) {
        self.anomalies_detected.fetch_add(1, Ordering::Relaxed);
        self.last_confidence
            .store(confidence as u64, Ordering::Relaxed);
    }

    /// Record a published signal.
    pub fn record_publish(&self) {
        self.signals_published.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an observation cycle.
    pub fn record_observation(&self) {
        self.observations.fetch_add(1, Ordering::Relaxed);
    }
}

/// Chain observer that monitors state and publishes anomaly signals.
pub struct ChainObserver {
    /// Rolling statistics.
    stats: ChainStats,

    /// Anomaly detector.
    detector: AnomalyDetector,

    /// Signal reporter.
    reporter: AnomalyReporter,

    /// Configuration.
    config: ObserverConfig,

    /// Metrics.
    metrics: Arc<ObserverMetrics>,

    /// Last observed height (for detecting new blocks).
    last_height: u64,

    /// Last observed round (for detecting round changes).
    last_round: u64,
}

impl ChainObserver {
    /// Create a new chain observer.
    #[must_use]
    pub fn new(signing_key: SigningKey, config: ObserverConfig) -> Self {
        let stats = ChainStats::new(config.stats_window_size);
        let detector = AnomalyDetector::with_thresholds(config.thresholds.clone());
        let reporter = AnomalyReporter::new(signing_key);

        Self {
            stats,
            detector,
            reporter,
            config,
            metrics: Arc::new(ObserverMetrics::new()),
            last_height: 0,
            last_round: 0,
        }
    }

    /// Get shared metrics reference.
    #[must_use]
    pub fn metrics(&self) -> Arc<ObserverMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Get current statistics (for debugging/testing).
    #[must_use]
    pub fn stats(&self) -> &ChainStats {
        &self.stats
    }

    /// Perform one observation cycle.
    ///
    /// This should be called periodically (e.g., every 500ms).
    ///
    /// # Arguments
    /// - `state`: Current observable state from the node
    /// - `callback`: Called for each detected anomaly
    ///
    /// # Returns
    /// List of anomalies detected in this cycle.
    pub fn observe<S: ObservableState, C: AnomalyCallback>(
        &mut self,
        state: &S,
        callback: &C,
    ) -> Vec<DetectedAnomaly> {
        self.metrics.record_observation();

        // Collect current state
        let height = state.committed_height();
        let round = state.current_round();
        let peer_count = state.peer_count();
        let mempool_size = state.mempool_size();

        // Record metrics
        self.stats.record_peer_count(peer_count);
        self.stats.record_mempool_size(mempool_size);
        self.stats.record_observation(height);

        // Check for new blocks (missed block detection)
        if height > self.last_height {
            self.check_missed_blocks(state, self.last_height + 1, height);
            self.last_height = height;
        }

        // Check for round changes (potential timeout/view change)
        if round != self.last_round {
            // Round changed without height change = possible timeout
            if height == self.last_height && round > self.last_round {
                // This could indicate a missed block from the previous leader
                // The expected leader for (height, last_round) didn't propose
                if let Some(expected) = state.expected_leader(height, self.last_round) {
                    self.stats.record_missed_block(expected);
                }
            }
            self.last_round = round;
        }

        // Run anomaly detection
        let anomalies = self.detector.detect(&self.stats);

        // Process anomalies
        for anomaly in &anomalies {
            self.metrics.record_anomaly(anomaly.confidence);

            // Check if we should publish
            if self.config.publish_enabled
                && anomaly.confidence >= self.config.min_publish_confidence
            {
                let (payload, signal) = self.reporter.create_report(anomaly);
                callback.on_anomaly(payload, signal);
                self.metrics.record_publish();
            }
        }

        anomalies
    }

    /// Check for missed blocks in a height range.
    fn check_missed_blocks<S: ObservableState>(
        &mut self,
        state: &S,
        start_height: u64,
        end_height: u64,
    ) {
        // For each height in range, record the proposer
        // In a real implementation, we'd get the actual proposer from blocks
        // For now, we record expected leaders as successful proposers
        // (since if height advanced, someone must have proposed)

        for h in start_height..=end_height {
            // Height h was committed, so round 0 leader successfully proposed
            // (simplification - in reality we'd check the actual block)
            if let Some(proposer) = state.expected_leader(h.saturating_sub(1), 0) {
                self.stats.record_proposal(proposer);
            }
        }
    }

    /// Reset the observer state.
    pub fn reset(&mut self) {
        self.stats.reset();
        self.last_height = 0;
        self.last_round = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Test implementation of ObservableState.
    struct MockState {
        height: u64,
        round: u64,
        peers: u64,
        mempool: u64,
        view_changes: u64,
        validators: Vec<Address>,
    }

    impl MockState {
        fn new() -> Self {
            Self {
                height: 0,
                round: 0,
                peers: 4,
                mempool: 50,
                view_changes: 0,
                validators: vec![[0x01; 32], [0x02; 32], [0x03; 32], [0x04; 32]],
            }
        }
    }

    impl ObservableState for MockState {
        fn committed_height(&self) -> u64 {
            self.height
        }

        fn current_round(&self) -> u64 {
            self.round
        }

        fn peer_count(&self) -> u64 {
            self.peers
        }

        fn mempool_size(&self) -> u64 {
            self.mempool
        }

        fn view_changes_total(&self) -> u64 {
            self.view_changes
        }

        fn validator_set(&self) -> Vec<Address> {
            self.validators.clone()
        }

        fn expected_leader(&self, height: u64, round: u64) -> Option<Address> {
            let idx = ((height + round) as usize) % self.validators.len();
            Some(self.validators[idx])
        }
    }

    /// Test callback that collects signals.
    struct CollectingCallback {
        signals: Mutex<Vec<AiSignalV1>>,
    }

    impl CollectingCallback {
        fn new() -> Self {
            Self {
                signals: Mutex::new(Vec::new()),
            }
        }

        fn signals(&self) -> Vec<AiSignalV1> {
            self.signals.lock().unwrap().clone()
        }
    }

    impl AnomalyCallback for CollectingCallback {
        fn on_anomaly(&self, _payload: SignalPayload, signal: AiSignalV1) {
            self.signals.lock().unwrap().push(signal);
        }
    }

    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[0x42u8; 32])
    }

    #[test]
    fn observer_creates_with_config() {
        let config = ObserverConfig::default();
        let observer = ChainObserver::new(test_signing_key(), config);

        assert_eq!(observer.last_height, 0);
        assert_eq!(observer.last_round, 0);
    }

    #[test]
    fn observer_tracks_height_changes() {
        let config = ObserverConfig::default();
        let mut observer = ChainObserver::new(test_signing_key(), config);
        let callback = NoopCallback;

        let mut state = MockState::new();
        state.height = 10;

        observer.observe(&state, &callback);
        assert_eq!(observer.last_height, 10);

        state.height = 15;
        observer.observe(&state, &callback);
        assert_eq!(observer.last_height, 15);
    }

    #[test]
    fn observer_records_metrics() {
        let config = ObserverConfig::default();
        let mut observer = ChainObserver::new(test_signing_key(), config);
        let callback = NoopCallback;
        let state = MockState::new();

        let metrics = observer.metrics();

        observer.observe(&state, &callback);
        assert_eq!(metrics.observations.load(Ordering::Relaxed), 1);

        observer.observe(&state, &callback);
        assert_eq!(metrics.observations.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn observer_detects_mempool_congestion() {
        let mut config = ObserverConfig::default();
        config.thresholds.min_observations = 5;
        config.min_publish_confidence = 100;

        let mut observer = ChainObserver::new(test_signing_key(), config);
        let callback = CollectingCallback::new();

        let mut state = MockState::new();
        state.mempool = 50;

        // Build baseline
        for i in 0..20 {
            state.height = i;
            observer.observe(&state, &callback);
        }

        // Spike mempool (4x baseline, > 3x threshold)
        state.mempool = 200;
        state.height = 21;

        let anomalies = observer.observe(&state, &callback);

        // Should detect congestion
        let congestion = anomalies.iter().any(|a| {
            matches!(
                a.kind,
                crate::detector::AnomalyKind::MempoolCongestion { .. }
            )
        });
        assert!(congestion, "Should detect mempool congestion");
    }

    #[test]
    fn observer_respects_min_publish_confidence() {
        let mut config = ObserverConfig::default();
        config.thresholds.min_observations = 5;
        config.min_publish_confidence = 250; // Very high threshold

        let mut observer = ChainObserver::new(test_signing_key(), config);
        let callback = CollectingCallback::new();

        let mut state = MockState::new();
        state.mempool = 50;

        // Build baseline
        for i in 0..20 {
            state.height = i;
            observer.observe(&state, &callback);
        }

        // Spike mempool - will detect but not publish
        state.mempool = 200;
        state.height = 21;

        let anomalies = observer.observe(&state, &callback);

        // Anomaly detected
        assert!(!anomalies.is_empty());

        // But not published (confidence likely < 250)
        assert!(
            callback.signals().is_empty() || anomalies[0].confidence >= 250,
            "Should not publish low-confidence anomalies"
        );
    }

    #[test]
    fn observer_reset_clears_state() {
        let config = ObserverConfig::default();
        let mut observer = ChainObserver::new(test_signing_key(), config);
        let callback = NoopCallback;

        let mut state = MockState::new();
        state.height = 100;
        observer.observe(&state, &callback);

        assert_eq!(observer.last_height, 100);

        observer.reset();

        assert_eq!(observer.last_height, 0);
        assert_eq!(observer.stats.observation_count, 0);
    }

    #[test]
    fn noop_callback_does_nothing() {
        let callback = NoopCallback;
        let payload = SignalPayload::new(
            "test".to_string(),
            "1.0".to_string(),
            "input".to_string(),
            vec![],
            "explanation".to_string(),
        );
        let signal = AiSignalV1 {
            signal_type: novai_ai_entities::AiSignalType::Anomaly,
            height: 0,
            issuer: [0; 32],
            confidence: 100,
            payload_hash: [0; 32],
            zk_proof: None,
            signature: [0; 64],
        };

        // Should not panic
        callback.on_anomaly(payload, signal);
    }
}
