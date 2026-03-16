//! Performance metrics collection and reporting.
//!
//! INVARIANTS:
//! - All events are processed in order received
//! - Histogram accurately reflects recorded latencies
//! - Thread-safe snapshot access
//!
//! FAILURE MODES:
//! - Channel closed - collector terminates
//! - Histogram overflow - saturates at max value

use hdrhistogram::Histogram;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info};

/// Events sent from submitter to metrics collector.
#[derive(Debug, Clone)]
pub enum MetricEvent {
    /// Transaction submitted to node.
    Submitted {
        #[allow(dead_code)]
        txid: [u8; 32],
        #[allow(dead_code)]
        timestamp: Instant,
    },
    /// Transaction accepted by node (in mempool).
    Accepted {
        #[allow(dead_code)]
        txid: [u8; 32],
        latency: Duration,
    },
    /// Transaction rejected by node (validation error).
    Rejected {
        #[allow(dead_code)]
        txid: [u8; 32],
        #[allow(dead_code)]
        reason: String,
        latency: Duration,
    },
    /// Submission failed (network error after retries).
    Failed {
        #[allow(dead_code)]
        txid: [u8; 32],
        #[allow(dead_code)]
        error: String,
        latency: Duration,
    },
    /// Transaction confirmed in a block.
    #[allow(dead_code)]
    Confirmed {
        txid: [u8; 32],
        block_height: u64,
        latency_from_submit: Duration,
    },
}

/// Point-in-time snapshot of metrics.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    /// Total transactions submitted.
    pub submitted_count: u64,
    /// Transactions accepted by node.
    pub accepted_count: u64,
    /// Transactions rejected by node.
    pub rejected_count: u64,
    /// Submissions that failed (network errors).
    pub failed_count: u64,
    /// Transactions confirmed in blocks.
    pub confirmed_count: u64,

    /// Submission latency percentiles (microseconds).
    pub latency_p50_us: u64,
    pub latency_p95_us: u64,
    pub latency_p99_us: u64,
    pub latency_max_us: u64,
    pub latency_mean_us: f64,

    /// Confirmation latency percentiles (if tracked).
    pub confirm_latency_p50_us: Option<u64>,
    pub confirm_latency_p95_us: Option<u64>,
    pub confirm_latency_p99_us: Option<u64>,

    /// Time since metrics collection started.
    pub elapsed: Duration,
    /// Computed TPS (accepted / elapsed_seconds).
    pub actual_tps: f64,
}

impl MetricsSnapshot {
    /// Format as human-readable text.
    pub fn to_text(&self) -> String {
        let mut output = String::new();
        output.push_str("=== Load Test Results ===\n");
        output.push_str(&format!("Duration: {:.1}s\n", self.elapsed.as_secs_f64()));
        output.push_str("\nTransactions:\n");
        output.push_str(&format!("  Submitted:  {}\n", self.submitted_count));
        output.push_str(&format!(
            "  Accepted:   {} ({:.2}%)\n",
            self.accepted_count,
            self.percentage(self.accepted_count, self.submitted_count)
        ));
        output.push_str(&format!(
            "  Rejected:   {} ({:.2}%)\n",
            self.rejected_count,
            self.percentage(self.rejected_count, self.submitted_count)
        ));
        output.push_str(&format!(
            "  Failed:     {} ({:.2}%)\n",
            self.failed_count,
            self.percentage(self.failed_count, self.submitted_count)
        ));
        output.push_str(&format!(
            "  Confirmed:  {} ({:.2}%)\n",
            self.confirmed_count,
            self.percentage(self.confirmed_count, self.submitted_count)
        ));

        output.push_str(&format!("\nActual TPS: {:.2}\n", self.actual_tps));

        output.push_str("\nSubmission Latency:\n");
        output.push_str(&format!(
            "  p50:  {:.1}ms\n",
            self.latency_p50_us as f64 / 1000.0
        ));
        output.push_str(&format!(
            "  p95:  {:.1}ms\n",
            self.latency_p95_us as f64 / 1000.0
        ));
        output.push_str(&format!(
            "  p99:  {:.1}ms\n",
            self.latency_p99_us as f64 / 1000.0
        ));
        output.push_str(&format!(
            "  max:  {:.1}ms\n",
            self.latency_max_us as f64 / 1000.0
        ));
        output.push_str(&format!("  mean: {:.1}ms\n", self.latency_mean_us / 1000.0));

        if let Some(p50) = self.confirm_latency_p50_us {
            output.push_str("\nConfirmation Latency:\n");
            output.push_str(&format!("  p50:  {:.1}ms\n", p50 as f64 / 1000.0));
            if let Some(p95) = self.confirm_latency_p95_us {
                output.push_str(&format!("  p95:  {:.1}ms\n", p95 as f64 / 1000.0));
            }
            if let Some(p99) = self.confirm_latency_p99_us {
                output.push_str(&format!("  p99:  {:.1}ms\n", p99 as f64 / 1000.0));
            }
        }

        output
    }

    /// Format as JSON.
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "submitted_count": self.submitted_count,
            "accepted_count": self.accepted_count,
            "rejected_count": self.rejected_count,
            "failed_count": self.failed_count,
            "confirmed_count": self.confirmed_count,
            "latency_p50_us": self.latency_p50_us,
            "latency_p95_us": self.latency_p95_us,
            "latency_p99_us": self.latency_p99_us,
            "latency_max_us": self.latency_max_us,
            "latency_mean_us": self.latency_mean_us,
            "confirm_latency_p50_us": self.confirm_latency_p50_us,
            "confirm_latency_p95_us": self.confirm_latency_p95_us,
            "confirm_latency_p99_us": self.confirm_latency_p99_us,
            "elapsed_ms": self.elapsed.as_millis(),
            "actual_tps": self.actual_tps,
        })
        .to_string()
    }

    /// Format as CSV row (with optional header).
    pub fn to_csv(&self, include_header: bool) -> String {
        let mut output = String::new();

        if include_header {
            output.push_str("submitted,accepted,rejected,failed,confirmed,");
            output.push_str(
                "latency_p50_us,latency_p95_us,latency_p99_us,latency_max_us,latency_mean_us,",
            );
            output.push_str("elapsed_ms,actual_tps\n");
        }

        output.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{:.2}\n",
            self.submitted_count,
            self.accepted_count,
            self.rejected_count,
            self.failed_count,
            self.confirmed_count,
            self.latency_p50_us,
            self.latency_p95_us,
            self.latency_p99_us,
            self.latency_max_us,
            self.latency_mean_us,
            self.elapsed.as_millis(),
            self.actual_tps,
        ));

        output
    }

    /// Helper to compute percentage.
    fn percentage(&self, numerator: u64, denominator: u64) -> f64 {
        if denominator == 0 {
            0.0
        } else {
            (numerator as f64 / denominator as f64) * 100.0
        }
    }
}

/// Handle to the metrics collector.
pub struct MetricsHandle {
    /// Shared state for snapshot access.
    state: Arc<RwLock<MetricsState>>,
    /// Join handle for collector task.
    collector_handle: tokio::task::JoinHandle<()>,
}

/// Lightweight read handle for periodic stats logging.
///
/// Provides direct access to counters without the full snapshot overhead.
pub struct MetricsReadHandle {
    state: Arc<RwLock<MetricsState>>,
}

impl MetricsReadHandle {
    pub async fn read(&self) -> MetricsStateView<'_> {
        MetricsStateView(self.state.read().await)
    }
}

/// RAII view into metrics state for reading counters.
pub struct MetricsStateView<'a>(tokio::sync::RwLockReadGuard<'a, MetricsState>);

impl MetricsStateView<'_> {
    pub fn accepted_count(&self) -> u64 {
        self.0.accepted_count
    }
    pub fn rejected_count(&self) -> u64 {
        self.0.rejected_count
    }
    pub fn elapsed_secs(&self) -> f64 {
        self.0.start_time.elapsed().as_secs_f64()
    }
}

impl MetricsHandle {
    /// Get current metrics snapshot.
    pub async fn snapshot(&self) -> MetricsSnapshot {
        let state = self.state.read().await;
        state.snapshot()
    }

    /// Clone the shared state for periodic stats logging.
    pub fn clone_state(&self) -> MetricsReadHandle {
        MetricsReadHandle {
            state: Arc::clone(&self.state),
        }
    }

    /// Wait for collector to finish (after channel closes).
    pub async fn wait(self) {
        let _ = self.collector_handle.await;
    }
}

/// Internal state (not public).
struct MetricsState {
    start_time: Instant,
    submitted_count: u64,
    accepted_count: u64,
    rejected_count: u64,
    failed_count: u64,
    confirmed_count: u64,
    submission_latency: Histogram<u64>,
    confirmation_latency: Option<Histogram<u64>>,
}

impl MetricsState {
    fn new(track_confirmations: bool) -> Self {
        Self {
            start_time: Instant::now(),
            submitted_count: 0,
            accepted_count: 0,
            rejected_count: 0,
            failed_count: 0,
            confirmed_count: 0,
            // Histogram: 1us to 60s, 3 significant figures
            submission_latency: Histogram::new(3).expect("Failed to create histogram"),
            confirmation_latency: if track_confirmations {
                Some(Histogram::new(3).expect("Failed to create histogram"))
            } else {
                None
            },
        }
    }

    fn process_event(&mut self, event: MetricEvent) {
        match event {
            MetricEvent::Submitted { .. } => {
                self.submitted_count += 1;
            }
            MetricEvent::Accepted { latency, .. } => {
                self.accepted_count += 1;
                let _ = self.submission_latency.record(latency.as_micros() as u64);
            }
            MetricEvent::Rejected { latency, .. } => {
                self.rejected_count += 1;
                let _ = self.submission_latency.record(latency.as_micros() as u64);
            }
            MetricEvent::Failed { latency, .. } => {
                self.failed_count += 1;
                let _ = self.submission_latency.record(latency.as_micros() as u64);
            }
            MetricEvent::Confirmed {
                latency_from_submit,
                ..
            } => {
                self.confirmed_count += 1;
                if let Some(ref mut hist) = self.confirmation_latency {
                    let _ = hist.record(latency_from_submit.as_micros() as u64);
                }
            }
        }
    }

    fn snapshot(&self) -> MetricsSnapshot {
        let elapsed = self.start_time.elapsed();
        let actual_tps = if elapsed.as_secs() > 0 {
            self.accepted_count as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };

        // Compute submission latency percentiles
        let (p50, p95, p99, max, mean) = if !self.submission_latency.is_empty() {
            (
                self.submission_latency.value_at_quantile(0.5),
                self.submission_latency.value_at_quantile(0.95),
                self.submission_latency.value_at_quantile(0.99),
                self.submission_latency.max(),
                self.submission_latency.mean(),
            )
        } else {
            (0, 0, 0, 0, 0.0)
        };

        // Compute confirmation latency percentiles (if tracked)
        let (confirm_p50, confirm_p95, confirm_p99) =
            if let Some(ref hist) = self.confirmation_latency {
                if !hist.is_empty() {
                    (
                        Some(hist.value_at_quantile(0.5)),
                        Some(hist.value_at_quantile(0.95)),
                        Some(hist.value_at_quantile(0.99)),
                    )
                } else {
                    (None, None, None)
                }
            } else {
                (None, None, None)
            };

        MetricsSnapshot {
            submitted_count: self.submitted_count,
            accepted_count: self.accepted_count,
            rejected_count: self.rejected_count,
            failed_count: self.failed_count,
            confirmed_count: self.confirmed_count,
            latency_p50_us: p50,
            latency_p95_us: p95,
            latency_p99_us: p99,
            latency_max_us: max,
            latency_mean_us: mean,
            confirm_latency_p50_us: confirm_p50,
            confirm_latency_p95_us: confirm_p95,
            confirm_latency_p99_us: confirm_p99,
            elapsed,
            actual_tps,
        }
    }
}

/// Metrics collector that processes events and maintains statistics.
pub struct MetricsCollector {
    track_confirmations: bool,
}

impl MetricsCollector {
    /// Create a new metrics collector.
    pub fn new(track_confirmations: bool) -> Self {
        Self {
            track_confirmations,
        }
    }

    /// Start the collector, consuming events from the provided channel.
    /// Returns a handle for snapshot access.
    pub fn start(self, mut event_rx: mpsc::UnboundedReceiver<MetricEvent>) -> MetricsHandle {
        let state = Arc::new(RwLock::new(MetricsState::new(self.track_confirmations)));
        let state_clone = Arc::clone(&state);

        let collector_handle = tokio::spawn(async move {
            info!("Metrics collector started");
            let mut event_count = 0u64;

            while let Some(event) = event_rx.recv().await {
                event_count += 1;
                if event_count.is_multiple_of(1000) {
                    debug!("Processed {} metric events", event_count);
                }

                let mut state = state_clone.write().await;
                state.process_event(event);
            }

            info!(
                "Metrics collector stopped (processed {} events)",
                event_count
            );
        });

        MetricsHandle {
            state,
            collector_handle,
        }
    }
}

/// Create a channel pair for metric events.
pub fn metric_channel() -> (
    mpsc::UnboundedSender<MetricEvent>,
    mpsc::UnboundedReceiver<MetricEvent>,
) {
    mpsc::unbounded_channel()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_records_latencies() {
        let mut hist: Histogram<u64> = Histogram::new(3).unwrap();

        hist.record(1000).unwrap(); // 1ms
        hist.record(5000).unwrap(); // 5ms
        hist.record(10000).unwrap(); // 10ms

        assert_eq!(hist.len(), 3);
        assert!(hist.value_at_quantile(0.5) >= 1000);
    }

    #[test]
    fn snapshot_computes_percentiles() {
        let mut state = MetricsState::new(false);

        // Record some latencies
        for i in 1..=100 {
            state.process_event(MetricEvent::Accepted {
                txid: [i as u8; 32],
                latency: Duration::from_micros(i * 1000), // 1ms to 100ms
            });
        }

        let snapshot = state.snapshot();
        assert_eq!(snapshot.accepted_count, 100);
        assert!(snapshot.latency_p50_us > 0);
        assert!(snapshot.latency_p95_us > snapshot.latency_p50_us);
        assert!(snapshot.latency_p99_us >= snapshot.latency_p95_us);
    }

    #[test]
    fn to_json_format_is_valid() {
        let state = MetricsState::new(false);
        let snapshot = state.snapshot();

        let json = snapshot.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(parsed.get("submitted_count").is_some());
        assert!(parsed.get("accepted_count").is_some());
        assert!(parsed.get("actual_tps").is_some());
    }

    #[test]
    fn to_csv_includes_header() {
        let state = MetricsState::new(false);
        let snapshot = state.snapshot();

        let csv_with_header = snapshot.to_csv(true);
        assert!(csv_with_header.contains("submitted,accepted"));

        let csv_no_header = snapshot.to_csv(false);
        assert!(!csv_no_header.contains("submitted,accepted"));
    }

    #[tokio::test]
    async fn collector_processes_events() {
        let (tx, rx) = metric_channel();
        let collector = MetricsCollector::new(false);
        let handle = collector.start(rx);

        // Send some events
        tx.send(MetricEvent::Submitted {
            txid: [1; 32],
            timestamp: Instant::now(),
        })
        .unwrap();

        tx.send(MetricEvent::Accepted {
            txid: [1; 32],
            latency: Duration::from_millis(5),
        })
        .unwrap();

        // Give collector time to process
        tokio::time::sleep(Duration::from_millis(50)).await;

        let snapshot = handle.snapshot().await;
        assert_eq!(snapshot.submitted_count, 1);
        assert_eq!(snapshot.accepted_count, 1);

        drop(tx); // Close channel
        handle.wait().await;
    }

    #[test]
    fn percentage_calculation() {
        let snapshot = MetricsSnapshot {
            submitted_count: 100,
            accepted_count: 95,
            rejected_count: 3,
            failed_count: 2,
            confirmed_count: 90,
            latency_p50_us: 1000,
            latency_p95_us: 5000,
            latency_p99_us: 10000,
            latency_max_us: 15000,
            latency_mean_us: 2500.0,
            confirm_latency_p50_us: None,
            confirm_latency_p95_us: None,
            confirm_latency_p99_us: None,
            elapsed: Duration::from_secs(10),
            actual_tps: 9.5,
        };

        assert_eq!(snapshot.percentage(95, 100), 95.0);
        assert_eq!(snapshot.percentage(0, 100), 0.0);
        assert_eq!(snapshot.percentage(50, 0), 0.0); // Avoid division by zero
    }
}
