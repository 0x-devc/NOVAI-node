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
"#,
            self.committed_height,
            self.current_round,
            self.peer_count,
            self.mempool_size,
            self.view_changes_total,
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

    println!("📊 Metrics server listening on http://{}", addr);

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
        };

        let output = snapshot.to_prometheus();

        // Check that all metrics are present
        assert!(output.contains("novai_committed_height 42"));
        assert!(output.contains("novai_current_round 3"));
        assert!(output.contains("novai_peer_count 4"));
        assert!(output.contains("novai_mempool_size 127"));
        assert!(output.contains("novai_consensus_view_changes_total 5"));

        // Check that HELP and TYPE lines are present
        assert!(output.contains("# HELP novai_committed_height"));
        assert!(output.contains("# TYPE novai_committed_height gauge"));
        assert!(output.contains("# TYPE novai_consensus_view_changes_total counter"));
    }

    #[test]
    fn test_zero_values() {
        let snapshot = MetricsSnapshot {
            committed_height: 0,
            current_round: 0,
            peer_count: 0,
            mempool_size: 0,
            view_changes_total: 0,
        };

        let output = snapshot.to_prometheus();
        assert!(output.contains("novai_committed_height 0"));
        assert!(output.contains("novai_peer_count 0"));
    }
}
