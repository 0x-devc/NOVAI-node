//! Main chain observer that ties statistics, detection, and reporting together.
//!
//! PURPOSE: Background observer that monitors chain state, detects anomalies,
//! and publishes signals. Also creates L1 memory objects for AI entities (Week 21).
//!
//! INVARIANTS:
//! - Observer is non-blocking, runs in its own thread
//! - Detection runs on every observation cycle
//! - Signals are published immediately upon detection
//! - Memory objects created at configurable intervals
//!
//! FAILURE MODES:
//! - If state lock is poisoned, observation cycle is skipped
//! - If signal publishing fails, anomaly is logged but not retried
//! - If memory callback fails, error is logged but observation continues

use crate::detector::{AnomalyDetector, AnomalyThresholds, DetectedAnomaly};
use crate::reporter::AnomalyReporter;
use crate::stats::ChainStats;
use ed25519_dalek::SigningKey;
use novai_ai_entities::{
    AiSignalV1, ChainSummaryData, MemoryObjectType, SignalPayload, StatisticsSnapshotData,
};
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

// ============================================================================
// MEMORY CALLBACKS (Week 21 - D21.5)
// ============================================================================

/// Callback for creating L1 memory objects.
///
/// Implementations should store the provided data as memory objects for the
/// AI entity associated with this observer.
pub trait MemoryCallback: Send + Sync {
    /// Called when a chain summary should be stored.
    ///
    /// # Arguments
    /// - `object_type`: The type of memory object (ChainSummary)
    /// - `data`: Encoded chain summary data
    fn on_chain_summary(&self, object_type: MemoryObjectType, data: Vec<u8>);

    /// Called when a statistics snapshot should be stored.
    ///
    /// # Arguments
    /// - `object_type`: The type of memory object (StatisticsSnapshot)
    /// - `data`: Encoded statistics snapshot data
    fn on_statistics_snapshot(&self, object_type: MemoryObjectType, data: Vec<u8>);
}

/// No-op memory callback for testing or when memory objects should not be created.
pub struct NoopMemoryCallback;

impl MemoryCallback for NoopMemoryCallback {
    fn on_chain_summary(&self, _object_type: MemoryObjectType, _data: Vec<u8>) {
        // Do nothing
    }

    fn on_statistics_snapshot(&self, _object_type: MemoryObjectType, _data: Vec<u8>) {
        // Do nothing
    }
}

/// Logging memory callback that prints when memory objects are created.
pub struct LoggingMemoryCallback;

impl MemoryCallback for LoggingMemoryCallback {
    fn on_chain_summary(&self, object_type: MemoryObjectType, data: Vec<u8>) {
        println!(
            "📊 CHAIN SUMMARY: type={:?} data_len={}",
            object_type,
            data.len()
        );
    }

    fn on_statistics_snapshot(&self, object_type: MemoryObjectType, data: Vec<u8>) {
        println!(
            "📈 STATISTICS SNAPSHOT: type={:?} data_len={}",
            object_type,
            data.len()
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

    // Week 21 - Memory object configuration (D21.5)
    /// Interval (in observations) between statistics snapshots.
    /// Set to 0 to disable.
    pub snapshot_interval: u64,

    /// Interval (in observations) between chain summaries.
    /// Set to 0 to disable.
    pub summary_interval: u64,

    /// Whether memory object creation is enabled.
    pub memory_enabled: bool,
}

impl Default for ObserverConfig {
    fn default() -> Self {
        Self {
            thresholds: AnomalyThresholds::default(),
            stats_window_size: 100,
            min_publish_confidence: 100, // Only publish if confident
            publish_enabled: true,
            // Week 21 defaults - create snapshot every 10 observations,
            // chain summary every 100 observations
            snapshot_interval: 10,
            summary_interval: 100,
            memory_enabled: true,
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

    // Week 21 - Memory object metrics (D21.5)
    /// Total statistics snapshots created.
    pub snapshots_created: AtomicU64,

    /// Total chain summaries created.
    pub summaries_created: AtomicU64,
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

    /// Record a statistics snapshot creation.
    pub fn record_snapshot(&self) {
        self.snapshots_created.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a chain summary creation.
    pub fn record_summary(&self) {
        self.summaries_created.fetch_add(1, Ordering::Relaxed);
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

    // Week 21 - Chain summary tracking (D21.5)
    /// Height at start of current summary epoch.
    summary_start_height: u64,

    /// Cumulative tx count for current summary epoch.
    summary_tx_count: u64,

    /// Cumulative fee total for current summary epoch.
    summary_fee_total: u64,
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
            summary_start_height: 0,
            summary_tx_count: 0,
            summary_fee_total: 0,
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

    // ========================================================================
    // MEMORY OBJECT CREATION (Week 21 - D21.5)
    // ========================================================================

    /// Create a statistics snapshot from current state.
    ///
    /// Captures point-in-time metrics for AI analysis.
    #[must_use]
    pub fn create_statistics_snapshot<S: ObservableState>(
        &self,
        state: &S,
    ) -> StatisticsSnapshotData {
        StatisticsSnapshotData {
            height: state.committed_height(),
            mempool_size: self.stats.current_mempool_size(),
            avg_fee: 0, // TODO: Track in future when we have fee data
            fee_p95: 0, // TODO: Track in future when we have fee data
            validator_count: state.validator_set().len() as u32,
            avg_block_fullness: 50, // TODO: Track actual block fullness
        }
    }

    /// Create a chain summary for the current epoch.
    ///
    /// Summarizes block statistics over a range of heights.
    #[must_use]
    pub fn create_chain_summary(&self) -> ChainSummaryData {
        ChainSummaryData {
            start_height: self.summary_start_height,
            end_height: self.last_height,
            tx_count: self.summary_tx_count,
            fee_total: self.summary_fee_total,
            avg_block_fullness: 50, // TODO: Track actual block fullness
        }
    }

    /// Reset the chain summary epoch, starting fresh from current height.
    pub fn reset_summary_epoch(&mut self) {
        self.summary_start_height = self.last_height;
        self.summary_tx_count = 0;
        self.summary_fee_total = 0;
    }

    /// Record block data for chain summary accumulation.
    ///
    /// Call this when a new block is observed.
    pub fn record_block_for_summary(&mut self, tx_count: u64, fee_total: u64) {
        self.summary_tx_count = self.summary_tx_count.saturating_add(tx_count);
        self.summary_fee_total = self.summary_fee_total.saturating_add(fee_total);
    }

    /// Perform one observation cycle with memory callback.
    ///
    /// This extends `observe` to also create memory objects at configured intervals.
    ///
    /// # Arguments
    /// - `state`: Current observable state from the node
    /// - `anomaly_callback`: Called for each detected anomaly
    /// - `memory_callback`: Called when memory objects should be created
    ///
    /// # Returns
    /// List of anomalies detected in this cycle.
    pub fn observe_with_memory<S: ObservableState, A: AnomalyCallback, M: MemoryCallback>(
        &mut self,
        state: &S,
        anomaly_callback: &A,
        memory_callback: &M,
    ) -> Vec<DetectedAnomaly> {
        // Run normal observation
        let anomalies = self.observe(state, anomaly_callback);

        // Check if memory creation is enabled
        if !self.config.memory_enabled {
            return anomalies;
        }

        let obs_count = self.metrics.observations.load(Ordering::Relaxed);

        // Create statistics snapshot at interval
        if self.config.snapshot_interval > 0
            && obs_count.is_multiple_of(self.config.snapshot_interval)
        {
            let snapshot = self.create_statistics_snapshot(state);
            memory_callback
                .on_statistics_snapshot(MemoryObjectType::StatisticsSnapshot, snapshot.encode());
            self.metrics.record_snapshot();
        }

        // Create chain summary at interval
        if self.config.summary_interval > 0
            && obs_count.is_multiple_of(self.config.summary_interval)
        {
            let summary = self.create_chain_summary();
            memory_callback.on_chain_summary(MemoryObjectType::ChainSummary, summary.encode());
            self.metrics.record_summary();

            // Reset summary epoch for next interval
            self.reset_summary_epoch();
        }

        anomalies
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
        self.summary_start_height = 0;
        self.summary_tx_count = 0;
        self.summary_fee_total = 0;
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

    // ========================================================================
    // MEMORY CALLBACK TESTS (Week 21 - D21.5)
    // ========================================================================

    /// Collecting memory callback for testing.
    struct CollectingMemoryCallback {
        snapshots: Mutex<Vec<Vec<u8>>>,
        summaries: Mutex<Vec<Vec<u8>>>,
    }

    impl CollectingMemoryCallback {
        fn new() -> Self {
            Self {
                snapshots: Mutex::new(Vec::new()),
                summaries: Mutex::new(Vec::new()),
            }
        }

        fn snapshot_count(&self) -> usize {
            self.snapshots.lock().unwrap().len()
        }

        fn summary_count(&self) -> usize {
            self.summaries.lock().unwrap().len()
        }
    }

    impl MemoryCallback for CollectingMemoryCallback {
        fn on_chain_summary(&self, _object_type: MemoryObjectType, data: Vec<u8>) {
            self.summaries.lock().unwrap().push(data);
        }

        fn on_statistics_snapshot(&self, _object_type: MemoryObjectType, data: Vec<u8>) {
            self.snapshots.lock().unwrap().push(data);
        }
    }

    #[test]
    fn noop_memory_callback_does_nothing() {
        let callback = NoopMemoryCallback;

        // Should not panic
        callback.on_chain_summary(MemoryObjectType::ChainSummary, vec![1, 2, 3]);
        callback.on_statistics_snapshot(MemoryObjectType::StatisticsSnapshot, vec![4, 5, 6]);
    }

    #[test]
    fn observer_creates_statistics_snapshot() {
        let config = ObserverConfig::default();
        let observer = ChainObserver::new(test_signing_key(), config);

        let mut state = MockState::new();
        state.height = 100;
        state.mempool = 75;

        let snapshot = observer.create_statistics_snapshot(&state);

        assert_eq!(snapshot.height, 100);
        assert_eq!(snapshot.validator_count, 4);
    }

    #[test]
    fn observer_creates_chain_summary() {
        let config = ObserverConfig::default();
        let mut observer = ChainObserver::new(test_signing_key(), config);

        // Simulate some block observations
        observer.last_height = 50;
        observer.summary_start_height = 10;
        observer.summary_tx_count = 500;
        observer.summary_fee_total = 5000;

        let summary = observer.create_chain_summary();

        assert_eq!(summary.start_height, 10);
        assert_eq!(summary.end_height, 50);
        assert_eq!(summary.tx_count, 500);
        assert_eq!(summary.fee_total, 5000);
    }

    #[test]
    fn observer_resets_summary_epoch() {
        let config = ObserverConfig::default();
        let mut observer = ChainObserver::new(test_signing_key(), config);

        observer.last_height = 100;
        observer.summary_tx_count = 500;
        observer.summary_fee_total = 5000;

        observer.reset_summary_epoch();

        assert_eq!(observer.summary_start_height, 100);
        assert_eq!(observer.summary_tx_count, 0);
        assert_eq!(observer.summary_fee_total, 0);
    }

    #[test]
    fn observer_records_block_for_summary() {
        let config = ObserverConfig::default();
        let mut observer = ChainObserver::new(test_signing_key(), config);

        observer.record_block_for_summary(10, 100);
        observer.record_block_for_summary(20, 200);

        assert_eq!(observer.summary_tx_count, 30);
        assert_eq!(observer.summary_fee_total, 300);
    }

    #[test]
    fn observe_with_memory_creates_snapshots_at_interval() {
        let config = ObserverConfig {
            snapshot_interval: 5,
            summary_interval: 10,
            memory_enabled: true,
            ..Default::default()
        };

        let mut observer = ChainObserver::new(test_signing_key(), config);
        let anomaly_callback = NoopCallback;
        let memory_callback = CollectingMemoryCallback::new();

        let state = MockState::new();

        // Run 15 observations
        for _ in 0..15 {
            observer.observe_with_memory(&state, &anomaly_callback, &memory_callback);
        }

        // Should have 3 snapshots (at obs 5, 10, 15)
        assert_eq!(memory_callback.snapshot_count(), 3);

        // Should have 1 summary (at obs 10)
        assert_eq!(memory_callback.summary_count(), 1);
    }

    #[test]
    fn observe_with_memory_respects_disabled() {
        let config = ObserverConfig {
            snapshot_interval: 1,
            summary_interval: 1,
            memory_enabled: false, // Disabled
            ..Default::default()
        };

        let mut observer = ChainObserver::new(test_signing_key(), config);
        let anomaly_callback = NoopCallback;
        let memory_callback = CollectingMemoryCallback::new();

        let state = MockState::new();

        // Run 10 observations
        for _ in 0..10 {
            observer.observe_with_memory(&state, &anomaly_callback, &memory_callback);
        }

        // No callbacks should have been made
        assert_eq!(memory_callback.snapshot_count(), 0);
        assert_eq!(memory_callback.summary_count(), 0);
    }

    #[test]
    fn observer_metrics_track_memory_creation() {
        let config = ObserverConfig {
            snapshot_interval: 2,
            summary_interval: 4,
            memory_enabled: true,
            ..Default::default()
        };

        let mut observer = ChainObserver::new(test_signing_key(), config);
        let anomaly_callback = NoopCallback;
        let memory_callback = NoopMemoryCallback;
        let metrics = observer.metrics();

        let state = MockState::new();

        // Run 8 observations
        for _ in 0..8 {
            observer.observe_with_memory(&state, &anomaly_callback, &memory_callback);
        }

        // Should have 4 snapshots (at obs 2, 4, 6, 8)
        assert_eq!(metrics.snapshots_created.load(Ordering::Relaxed), 4);

        // Should have 2 summaries (at obs 4, 8)
        assert_eq!(metrics.summaries_created.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn observer_full_reset_clears_summary_state() {
        let config = ObserverConfig::default();
        let mut observer = ChainObserver::new(test_signing_key(), config);

        observer.last_height = 100;
        observer.summary_start_height = 50;
        observer.summary_tx_count = 500;
        observer.summary_fee_total = 5000;

        observer.reset();

        assert_eq!(observer.last_height, 0);
        assert_eq!(observer.summary_start_height, 0);
        assert_eq!(observer.summary_tx_count, 0);
        assert_eq!(observer.summary_fee_total, 0);
    }

    #[test]
    fn statistics_snapshot_data_encodes_correctly() {
        use novai_ai_entities::StatisticsSnapshotData;

        let snapshot = StatisticsSnapshotData {
            height: 1000,
            mempool_size: 250,
            avg_fee: 50,
            fee_p95: 100,
            validator_count: 4,
            avg_block_fullness: 75,
        };

        let encoded = snapshot.encode();
        let decoded = StatisticsSnapshotData::decode(&encoded).expect("Should decode");

        assert_eq!(decoded.height, 1000);
        assert_eq!(decoded.mempool_size, 250);
        assert_eq!(decoded.validator_count, 4);
        assert_eq!(decoded.avg_block_fullness, 75);
    }

    #[test]
    fn chain_summary_data_encodes_correctly() {
        use novai_ai_entities::ChainSummaryData;

        let summary = ChainSummaryData {
            start_height: 100,
            end_height: 200,
            tx_count: 500,
            fee_total: 5000,
            avg_block_fullness: 60,
        };

        let encoded = summary.encode();
        let decoded = ChainSummaryData::decode(&encoded).expect("Should decode");

        assert_eq!(decoded.start_height, 100);
        assert_eq!(decoded.end_height, 200);
        assert_eq!(decoded.tx_count, 500);
        assert_eq!(decoded.fee_total, 5000);
        assert_eq!(decoded.avg_block_fullness, 60);
    }
}
