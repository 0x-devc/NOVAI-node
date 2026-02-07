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
use tiny_http::{Response, Server, StatusCode};

/// Point-in-time snapshot of node metrics.
pub struct MetricsSnapshot {
    /// Height of last committed block.
    pub committed_height: u64,
    /// Current consensus round.
    pub current_round: u64,
    /// Number of connected peers.
    pub peer_count: u64,
    /// Transactions in mempool.
    pub mempool_size: u64,
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

# HELP novai_current_round Current consensus round
# TYPE novai_current_round gauge
novai_current_round {}

# HELP novai_peer_count Number of connected peers
# TYPE novai_peer_count gauge
novai_peer_count {}

# HELP novai_mempool_size Transactions pending in mempool
# TYPE novai_mempool_size gauge
novai_mempool_size {}

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
            self.current_round,
            self.peer_count,
            self.mempool_size,
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
            current_round: 3,
            peer_count: 4,
            mempool_size: 127,
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
            current_round: 0,
            peer_count: 0,
            mempool_size: 0,
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
        assert!(output.contains("novai_peer_count 0"));
        assert!(output.contains("novai_block_tx_count 0"));
        assert!(output.contains("novai_copilot_observations_total 0"));
        assert!(output.contains("novai_anomaly_signals_total 0"));
        assert!(output.contains("novai_anomaly_last_confidence 0"));
    }
}
