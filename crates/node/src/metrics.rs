//! Prometheus metrics HTTP endpoint.
//!
//! PURPOSE: Expose node metrics in Prometheus text format for monitoring.
//!
//! INVARIANTS:
//! - Server binds to specified address on startup
//! - /metrics returns valid Prometheus text format
//! - /health returns 200 OK if server is running
//!
//! FAILURE MODES:
//! - Port already in use → returns error on start
//! - Panic in collect_fn → request returns 500

use std::net::SocketAddr;
use std::thread;
use std::time::Instant;
use tiny_http::{Response, Server, StatusCode};

/// Point-in-time snapshot of node metrics.
/// Gate SOAK C1/C2: process-wide admission and pool-shape counters.
///
/// Statics rather than threaded parameters because there is one node per
/// process and these are pure observation: threading five counters through
/// the RPC server signature would touch far more code than the measurement
/// is worth.
///
/// The pool-shape gauges are CACHED here by a periodic pass and read
/// lock-free at scrape time. Computing the census inside the scrape would
/// hold the mempool mutex against admission on every poll, and that mutex is
/// already shared by five threads (RPC, gossip, propose loop, observer,
/// metrics).
pub mod pool_metrics {
    use std::sync::atomic::{AtomicU64, Ordering};

    pub static READY: AtomicU64 = AtomicU64::new(0);
    pub static WAITING: AtomicU64 = AtomicU64::new(0);
    pub static GAPPED: AtomicU64 = AtomicU64::new(0);
    pub static SENDERS: AtomicU64 = AtomicU64::new(0);

    pub static REJ_NONCE_TOO_LOW: AtomicU64 = AtomicU64::new(0);
    pub static REJ_NONCE_TOO_HIGH: AtomicU64 = AtomicU64::new(0);
    pub static REJ_SENDER_LIMIT: AtomicU64 = AtomicU64::new(0);
    pub static REJ_FEE_TOO_LOW: AtomicU64 = AtomicU64::new(0);
    pub static REJ_FULL: AtomicU64 = AtomicU64::new(0);

    /// Publish a freshly computed census. Called by the periodic pass.
    pub fn publish_census(c: &mempool::PoolCensus) {
        READY.store(c.ready as u64, Ordering::Relaxed);
        WAITING.store(c.waiting as u64, Ordering::Relaxed);
        GAPPED.store(c.gapped as u64, Ordering::Relaxed);
        SENDERS.store(c.senders as u64, Ordering::Relaxed);
    }

    /// Count one admission rejection by reason. Called from every admission
    /// path, including gossip, whose rejections were previously invisible.
    pub fn record_rejection(err: &mempool::TxMempoolError) {
        let counter = match err {
            mempool::TxMempoolError::NonceTooLow { .. } => &REJ_NONCE_TOO_LOW,
            mempool::TxMempoolError::NonceTooHigh { .. } => &REJ_NONCE_TOO_HIGH,
            mempool::TxMempoolError::SenderLimitExceeded { .. } => &REJ_SENDER_LIMIT,
            mempool::TxMempoolError::FeeTooLow { .. } => &REJ_FEE_TOO_LOW,
            mempool::TxMempoolError::MempoolFull { .. } => &REJ_FULL,
            _ => return,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

pub struct MetricsSnapshot {
    /// Height of last committed block.
    pub committed_height: u64,
    /// Height certified by this node's highest QC (the consensus
    /// frontier). WEDGE-20260718: the frontier ran 818,258 heights above
    /// the committed floor with no metric making it visible; this gauge
    /// plus the derived gap is that visibility.
    pub highest_qc_height: u64,
    /// Seconds since this node's metrics collector last observed the
    /// committed height advance (see `CommitClock`). The rate-independent
    /// half of the monitor's commit_stall dual-trigger alarm.
    pub seconds_since_last_commit: u64,
    /// Current consensus round.
    pub current_round: u64,
    /// Number of connected peers.
    pub peer_count: u64,
    /// Transactions in mempool.
    pub mempool_size: u64,
    /// Gate SOAK C1: the pool split by how close each transaction is to
    /// inclusion. mempool_size alone cannot tell a healthy deep backlog from
    /// a jam, because it counts both, so no threshold on it is right in both
    /// directions. Flat unlabeled names on purpose: the monitor's parser
    /// drops any labeled sample, so a counter vector would be silently
    /// discarded and every alarm built on it would sit at insufficient_data.
    pub mempool_ready: u64,
    pub mempool_waiting: u64,
    pub mempool_gapped: u64,
    pub mempool_senders: u64,
    /// Gate SOAK C2: admission rejections by reason. Nothing on the dashboard
    /// could previously tell "the generator stopped" from "every submit is
    /// being refused"; gossip rejections in particular were swallowed
    /// silently.
    pub mempool_rejects_nonce_too_low: u64,
    pub mempool_rejects_nonce_too_high: u64,
    pub mempool_rejects_sender_limit: u64,
    pub mempool_rejects_fee_too_low: u64,
    pub mempool_rejects_full: u64,
    /// Total view changes (round advances due to timeouts).
    pub view_changes_total: u64,
    /// Number of transactions in last committed block.
    pub block_tx_count: u64,
    /// Total transactions committed across all blocks.
    pub total_txs_committed: u64,

    // Copilot metrics
    /// Total copilot observation cycles.
    pub copilot_observations_total: u64,
    /// Total anomalies detected by copilot.
    pub anomaly_signals_total: u64,
    /// Total signals published on-chain.
    pub anomaly_signals_published: u64,
    /// Confidence level of last detected anomaly (0-255).
    pub anomaly_last_confidence: u64,
}

impl MetricsSnapshot {
    /// Format metrics as Prometheus text exposition format.
    ///
    /// Returns metrics in Prometheus text format:
    /// - One block per metric with HELP, TYPE, and value lines
    /// - Gauges for instantaneous values (height, round, peer_count, mempool_size)
    /// - Counter for monotonically increasing values (view_changes_total)
    pub fn to_prometheus(&self) -> String {
        format!(
            r#"# HELP novai_committed_height Height of last committed block
# TYPE novai_committed_height gauge
novai_committed_height {}

# HELP novai_highest_qc_height Height certified by the highest QC (consensus frontier)
# TYPE novai_highest_qc_height gauge
novai_highest_qc_height {}

# HELP novai_consensus_commit_gap Consensus frontier minus committed height (healthy: 2 to 3 at any block rate)
# TYPE novai_consensus_commit_gap gauge
novai_consensus_commit_gap {}

# HELP novai_seconds_since_last_commit Seconds since the committed height last advanced
# TYPE novai_seconds_since_last_commit gauge
novai_seconds_since_last_commit {}

# HELP novai_current_round Current consensus round
# TYPE novai_current_round gauge
novai_current_round {}

# HELP novai_peer_count Number of connected peers
# TYPE novai_peer_count gauge
novai_peer_count {}

# HELP novai_mempool_size Transactions pending in mempool
# TYPE novai_mempool_size gauge
novai_mempool_size {}

# HELP novai_mempool_ready Pooled txs at the sender's expected nonce (includable next block)
# TYPE novai_mempool_ready gauge
novai_mempool_ready {}

# HELP novai_mempool_waiting Pooled txs in the reachable run above expected (healthy backlog)
# TYPE novai_mempool_waiting gauge
novai_mempool_waiting {}

# HELP novai_mempool_gapped Pooled txs unreachable from the sender's expected nonce
# TYPE novai_mempool_gapped gauge
novai_mempool_gapped {}

# HELP novai_mempool_senders Distinct senders holding at least one pooled tx
# TYPE novai_mempool_senders gauge
novai_mempool_senders {}

# HELP novai_mempool_rejects_nonce_too_low Admission rejections: nonce below expected
# TYPE novai_mempool_rejects_nonce_too_low counter
novai_mempool_rejects_nonce_too_low {}

# HELP novai_mempool_rejects_nonce_too_high Admission rejections: nonce past the horizon
# TYPE novai_mempool_rejects_nonce_too_high counter
novai_mempool_rejects_nonce_too_high {}

# HELP novai_mempool_rejects_sender_limit Admission rejections: per-sender slot cap
# TYPE novai_mempool_rejects_sender_limit counter
novai_mempool_rejects_sender_limit {}

# HELP novai_mempool_rejects_fee_too_low Admission rejections: below the effective fee floor
# TYPE novai_mempool_rejects_fee_too_low counter
novai_mempool_rejects_fee_too_low {}

# HELP novai_mempool_rejects_full Admission rejections: mempool byte cap
# TYPE novai_mempool_rejects_full counter
novai_mempool_rejects_full {}

# HELP novai_consensus_view_changes_total Total view changes (round advances)
# TYPE novai_consensus_view_changes_total counter
novai_consensus_view_changes_total {}

# HELP novai_block_tx_count Transactions in last committed block
# TYPE novai_block_tx_count gauge
novai_block_tx_count {}

# HELP novai_total_txs_committed Total transactions committed across all blocks
# TYPE novai_total_txs_committed counter
novai_total_txs_committed {}

# HELP novai_copilot_observations_total Total copilot observation cycles
# TYPE novai_copilot_observations_total counter
novai_copilot_observations_total {}

# HELP novai_anomaly_signals_total Total anomalies detected by copilot
# TYPE novai_anomaly_signals_total counter
novai_anomaly_signals_total {}

# HELP novai_anomaly_signals_published Total signals published on-chain
# TYPE novai_anomaly_signals_published counter
novai_anomaly_signals_published {}

# HELP novai_anomaly_last_confidence Confidence of last detected anomaly (0-255)
# TYPE novai_anomaly_last_confidence gauge
novai_anomaly_last_confidence {}
"#,
            self.committed_height,
            self.highest_qc_height,
            // Saturating: a fresh node (no QC yet) reports frontier 0 with a
            // positive committed height after recovery; the gap is 0, never
            // an underflow.
            self.highest_qc_height.saturating_sub(self.committed_height),
            self.seconds_since_last_commit,
            self.current_round,
            self.peer_count,
            self.mempool_size,
            self.mempool_ready,
            self.mempool_waiting,
            self.mempool_gapped,
            self.mempool_senders,
            self.mempool_rejects_nonce_too_low,
            self.mempool_rejects_nonce_too_high,
            self.mempool_rejects_sender_limit,
            self.mempool_rejects_fee_too_low,
            self.mempool_rejects_full,
            self.view_changes_total,
            self.block_tx_count,
            self.total_txs_committed,
            self.copilot_observations_total,
            self.anomaly_signals_total,
            self.anomaly_signals_published,
            self.anomaly_last_confidence,
        )
    }
}

/// Wall-clock age tracker for the committed height, feeding the
/// `novai_seconds_since_last_commit` gauge (WEDGE-20260718, the
/// rate-independent half of the monitor's commit_stall dual-trigger
/// alarm).
///
/// Scrape driven: the metrics collector calls `observe` with the current
/// committed height on every scrape; the clock stamps each advance and
/// reports how long the height has been flat. The clock starts at
/// construction (process boot), so a node that never commits reports a
/// growing age from boot, which is exactly the alarm-worthy condition. A
/// node restart resets the clock, giving a restarted node its 30 second
/// grace before the time trigger can page again.
pub struct CommitClock {
    last_height: u64,
    last_advance: Instant,
}

impl CommitClock {
    #[must_use]
    pub fn new() -> Self {
        Self::new_at(Instant::now())
    }

    /// `new` with an explicit boot instant, so tests are deterministic.
    fn new_at(boot: Instant) -> Self {
        Self {
            last_height: 0,
            last_advance: boot,
        }
    }

    /// Record the currently observed committed height and return how many
    /// whole seconds it has been since the height last advanced.
    pub fn observe(&mut self, committed_height: u64) -> u64 {
        self.observe_at(committed_height, Instant::now())
    }

    /// `observe` with an explicit clock, so tests are deterministic. A
    /// height that does not advance (or regresses, which cannot happen
    /// from a monotone committed cursor) leaves the stamp untouched: the
    /// age keeps growing, the conservative reading for an alarm input.
    fn observe_at(&mut self, committed_height: u64, now: Instant) -> u64 {
        if committed_height > self.last_height {
            self.last_height = committed_height;
            self.last_advance = now;
        }
        now.duration_since(self.last_advance).as_secs()
    }
}

impl Default for CommitClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Start the metrics HTTP server.
///
/// Spawns a dedicated thread to handle HTTP requests on the specified address.
/// Returns immediately after starting the listener.
///
/// # Endpoints
/// - `GET /metrics` - Prometheus text format metrics
/// - `GET /health` - Health check (returns 200 OK)
/// - All other paths return 404 Not Found
///
/// # Arguments
/// - `bind_addr` - Address to bind the HTTP server (e.g., "0.0.0.0:8080")
/// - `collect_fn` - Closure that collects metrics snapshot from node state
///
/// # Errors
/// Returns error if the server cannot bind to the address (e.g., port in use).
pub fn start_metrics_server<F>(bind_addr: &str, collect_fn: F) -> Result<(), String>
where
    F: Fn() -> MetricsSnapshot + Send + 'static,
{
    let addr: SocketAddr = bind_addr
        .parse()
        .map_err(|e| format!("invalid address: {e}"))?;

    let server = Server::http(addr).map_err(|e| format!("failed to start HTTP server: {e}"))?;

    tracing::info!(%addr, "Metrics server listening");

    thread::spawn(move || {
        for request in server.incoming_requests() {
            let response = match request.url() {
                "/metrics" => {
                    let metrics = collect_fn();
                    let body = metrics.to_prometheus();
                    Response::from_string(body).with_header(
                        "Content-Type: text/plain; version=0.0.4; charset=utf-8"
                            .parse::<tiny_http::Header>()
                            .unwrap(),
                    )
                }
                "/health" => Response::from_string("OK\n"),
                _ => Response::from_string("Not Found").with_status_code(StatusCode(404)),
            };

            // Ignore send errors (client may have disconnected)
            let _ = request.respond(response);
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prometheus_format() {
        let snapshot = MetricsSnapshot {
            committed_height: 42,
            highest_qc_height: 44,
            seconds_since_last_commit: 1,
            current_round: 3,
            peer_count: 4,
            mempool_size: 127,
            mempool_ready: 0,
            mempool_waiting: 0,
            mempool_gapped: 0,
            mempool_senders: 0,
            mempool_rejects_nonce_too_low: 0,
            mempool_rejects_nonce_too_high: 0,
            mempool_rejects_sender_limit: 0,
            mempool_rejects_fee_too_low: 0,
            mempool_rejects_full: 0,
            view_changes_total: 5,
            block_tx_count: 25,
            total_txs_committed: 1050,
            copilot_observations_total: 200,
            anomaly_signals_total: 3,
            anomaly_signals_published: 2,
            anomaly_last_confidence: 180,
        };

        let output = snapshot.to_prometheus();

        // Check that all metrics are present
        assert!(output.contains("novai_committed_height 42"));
        assert!(output.contains("novai_highest_qc_height 44"));
        // The gap is derived inside the renderer from the two heights, so
        // the exposed pair can never disagree with the exposed gap.
        assert!(output.contains("novai_consensus_commit_gap 2"));
        assert!(output.contains("novai_seconds_since_last_commit 1"));
        assert!(output.contains("novai_current_round 3"));
        assert!(output.contains("novai_peer_count 4"));
        assert!(output.contains("novai_mempool_size 127"));
        assert!(output.contains("novai_consensus_view_changes_total 5"));
        assert!(output.contains("novai_block_tx_count 25"));
        assert!(output.contains("novai_total_txs_committed 1050"));

        // Check copilot metrics
        assert!(output.contains("novai_copilot_observations_total 200"));
        assert!(output.contains("novai_anomaly_signals_total 3"));
        assert!(output.contains("novai_anomaly_signals_published 2"));
        assert!(output.contains("novai_anomaly_last_confidence 180"));

        // Check that HELP and TYPE lines are present
        assert!(output.contains("# HELP novai_committed_height"));
        assert!(output.contains("# TYPE novai_committed_height gauge"));
        assert!(output.contains("# TYPE novai_consensus_view_changes_total counter"));
        assert!(output.contains("# TYPE novai_total_txs_committed counter"));
        assert!(output.contains("# TYPE novai_copilot_observations_total counter"));
        assert!(output.contains("# TYPE novai_anomaly_signals_total counter"));
        assert!(output.contains("# TYPE novai_anomaly_last_confidence gauge"));
    }

    #[test]
    fn test_zero_values() {
        let snapshot = MetricsSnapshot {
            committed_height: 0,
            highest_qc_height: 0,
            seconds_since_last_commit: 0,
            current_round: 0,
            peer_count: 0,
            mempool_size: 0,
            mempool_ready: 0,
            mempool_waiting: 0,
            mempool_gapped: 0,
            mempool_senders: 0,
            mempool_rejects_nonce_too_low: 0,
            mempool_rejects_nonce_too_high: 0,
            mempool_rejects_sender_limit: 0,
            mempool_rejects_fee_too_low: 0,
            mempool_rejects_full: 0,
            view_changes_total: 0,
            block_tx_count: 0,
            total_txs_committed: 0,
            copilot_observations_total: 0,
            anomaly_signals_total: 0,
            anomaly_signals_published: 0,
            anomaly_last_confidence: 0,
        };

        let output = snapshot.to_prometheus();
        assert!(output.contains("novai_committed_height 0"));
        assert!(output.contains("novai_highest_qc_height 0"));
        assert!(output.contains("novai_consensus_commit_gap 0"));
        assert!(output.contains("novai_seconds_since_last_commit 0"));
        assert!(output.contains("novai_peer_count 0"));
        assert!(output.contains("novai_block_tx_count 0"));
        assert!(output.contains("novai_copilot_observations_total 0"));
        assert!(output.contains("novai_anomaly_signals_total 0"));
        assert!(output.contains("novai_anomaly_last_confidence 0"));
    }

    #[test]
    fn test_commit_gap_saturates_at_zero() {
        // A recovered node has its committed height before its first QC
        // adoption lands in a snapshot; the gap must clamp to 0, never
        // wrap.
        let snapshot = MetricsSnapshot {
            committed_height: 500,
            highest_qc_height: 0,
            seconds_since_last_commit: 0,
            current_round: 0,
            peer_count: 0,
            mempool_size: 0,
            mempool_ready: 0,
            mempool_waiting: 0,
            mempool_gapped: 0,
            mempool_senders: 0,
            mempool_rejects_nonce_too_low: 0,
            mempool_rejects_nonce_too_high: 0,
            mempool_rejects_sender_limit: 0,
            mempool_rejects_fee_too_low: 0,
            mempool_rejects_full: 0,
            view_changes_total: 0,
            block_tx_count: 0,
            total_txs_committed: 0,
            copilot_observations_total: 0,
            anomaly_signals_total: 0,
            anomaly_signals_published: 0,
            anomaly_last_confidence: 0,
        };
        let output = snapshot.to_prometheus();
        assert!(output.contains("novai_consensus_commit_gap 0"));
    }

    #[test]
    fn test_commit_clock_ages_while_flat_and_resets_on_advance() {
        use std::time::Duration;

        let t0 = Instant::now();
        let mut clock = CommitClock::new_at(t0);

        // First observation stamps the height; age reads 0.
        assert_eq!(clock.observe_at(100, t0), 0);
        // The height stays flat: the age grows with the wall clock. At 31
        // seconds the monitor's time trigger (30 s) would fire.
        assert_eq!(clock.observe_at(100, t0 + Duration::from_secs(10)), 10);
        assert_eq!(clock.observe_at(100, t0 + Duration::from_secs(31)), 31);
        // A committed advance resets the age.
        assert_eq!(clock.observe_at(101, t0 + Duration::from_secs(32)), 0);
        assert_eq!(clock.observe_at(101, t0 + Duration::from_secs(35)), 3);
    }

    #[test]
    fn test_commit_clock_healthy_cadence_stays_near_zero_at_any_rate() {
        use std::time::Duration;

        // A healthy chain commits continuously at any block rate, so every
        // scrape observes a fresh advance and the age never accumulates.
        // One scrape every 5 seconds, heights advancing by the per-interval
        // block count for rates from 1 to 1000 blocks/s.
        for rate in [1u64, 4, 25, 100, 1000] {
            let t0 = Instant::now();
            let mut clock = CommitClock::new_at(t0);
            let mut height = 1_000_000u64;
            for scrape in 1..=12u64 {
                height += rate * 5;
                let age = clock.observe_at(height, t0 + Duration::from_secs(scrape * 5));
                assert_eq!(
                    age, 0,
                    "healthy cadence at {rate} blocks/s must keep the commit age at zero"
                );
            }
        }
    }

    #[test]
    fn test_commit_clock_starts_at_boot_for_a_never_committing_node() {
        use std::time::Duration;

        // A node that boots and never commits reports a growing age from
        // boot: stalled-from-boot is alarm-worthy, not a blind spot.
        let t0 = Instant::now();
        let mut clock = CommitClock::new_at(t0);
        assert_eq!(clock.observe_at(0, t0 + Duration::from_secs(45)), 45);
    }
}
