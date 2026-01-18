//! Transaction submitter with worker pool and retry logic.
//!
//! INVARIANTS:
//! - All received transactions are submitted (or failed after retries)
//! - Metrics events are sent for every submission attempt
//! - Workers respect shutdown signal
//!
//! FAILURE MODES:
//! - Network unreachable - retries then fails
//! - Node rejects tx - logged as validation error
//! - Channel closed - workers terminate

use crate::metrics::MetricEvent;
use novai_codec::{encode_tx_v1_signed, txid_v1};
use novai_types::{TxId, TxV1};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::{debug, info, warn};

/// Configuration for the submitter.
#[derive(Debug, Clone)]
pub struct SubmitterConfig {
    /// RPC endpoint URL (e.g., "http://localhost:8080").
    pub endpoint: String,
    /// Number of concurrent worker tasks.
    pub worker_count: usize,
    /// Maximum retries per transaction.
    pub max_retries: u32,
    /// Base delay for exponential backoff.
    pub base_retry_delay: Duration,
    /// Request timeout.
    pub request_timeout: Duration,
    /// Enable confirmation tracking via polling.
    #[allow(dead_code)]
    pub track_confirmations: bool,
    /// Polling interval for confirmation tracking.
    #[allow(dead_code)]
    pub confirmation_poll_interval: Duration,
}

impl Default for SubmitterConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://localhost:8080".to_string(),
            worker_count: 4,
            max_retries: 3,
            base_retry_delay: Duration::from_millis(100),
            request_timeout: Duration::from_secs(5),
            track_confirmations: false,
            confirmation_poll_interval: Duration::from_secs(1),
        }
    }
}

/// Result of a submission attempt.
#[derive(Debug, Clone)]
pub enum SubmitResult {
    /// Successfully submitted, node returned TxId.
    Accepted {
        #[allow(dead_code)]
        txid: TxId,
    },
    /// Node rejected transaction (validation error).
    Rejected {
        #[allow(dead_code)]
        reason: String,
    },
    /// Network/timeout error after all retries.
    Failed {
        #[allow(dead_code)]
        error: String,
    },
}

/// Handle to control a running submitter pool.
pub struct SubmitterHandle {
    /// Send to request shutdown.
    shutdown_tx: broadcast::Sender<()>,
    /// Join handles for worker tasks.
    worker_handles: Vec<tokio::task::JoinHandle<WorkerStats>>,
}

impl SubmitterHandle {
    /// Request graceful shutdown of all workers.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(());
    }

    /// Wait for all workers to complete.
    pub async fn wait(self) -> SubmitterStats {
        let mut total_stats = SubmitterStats::default();

        for handle in self.worker_handles {
            match handle.await {
                Ok(stats) => {
                    total_stats.total_submitted += stats.submitted_count;
                    total_stats.total_accepted += stats.accepted_count;
                    total_stats.total_rejected += stats.rejected_count;
                    total_stats.total_failed += stats.failed_count;
                    total_stats.total_retries += stats.total_retries;
                }
                Err(e) => {
                    warn!("Worker task error: {}", e);
                }
            }
        }

        total_stats
    }
}

/// Statistics from a single worker.
#[derive(Debug, Clone, Default)]
pub struct WorkerStats {
    pub submitted_count: u64,
    pub accepted_count: u64,
    pub rejected_count: u64,
    pub failed_count: u64,
    pub total_retries: u64,
}

/// Aggregated statistics from all workers.
#[derive(Debug, Clone, Default)]
pub struct SubmitterStats {
    pub total_submitted: u64,
    pub total_accepted: u64,
    pub total_rejected: u64,
    pub total_failed: u64,
    pub total_retries: u64,
    #[allow(dead_code)]
    pub confirmed_count: u64, // TODO: Implement confirmation tracking
}

/// Transaction submitter with worker pool.
pub struct Submitter {
    config: SubmitterConfig,
    http_client: reqwest::Client,
}

impl Submitter {
    /// Create a new submitter with the given configuration.
    pub fn new(config: SubmitterConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            config,
            http_client,
        }
    }

    /// Start the worker pool consuming from the transaction channel.
    /// Sends metric events to the provided channel.
    pub fn start(
        self,
        tx_receiver: mpsc::Receiver<TxV1>,
        metric_tx: mpsc::UnboundedSender<MetricEvent>,
    ) -> SubmitterHandle {
        let (shutdown_tx, _) = broadcast::channel(1);
        let mut worker_handles = vec![];

        info!(
            "Starting {} submitter workers, endpoint={}",
            self.config.worker_count, self.config.endpoint
        );

        // Wrap receiver in Arc<Mutex<>> for sharing across workers
        let tx_receiver = Arc::new(Mutex::new(tx_receiver));

        // Start worker pool
        for worker_id in 0..self.config.worker_count {
            let http_client = self.http_client.clone();
            let endpoint = self.config.endpoint.clone();
            let max_retries = self.config.max_retries;
            let base_delay = self.config.base_retry_delay;
            let metric_tx = metric_tx.clone();
            let mut shutdown_rx = shutdown_tx.subscribe();
            let tx_receiver = Arc::clone(&tx_receiver);

            let handle = tokio::spawn(async move {
                let mut stats = WorkerStats::default();

                loop {
                    tokio::select! {
                        Some(tx) = async {
                            let mut rx = tx_receiver.lock().await;
                            rx.recv().await
                        } => {
                            stats.submitted_count += 1;

                            let result = submit_with_retry(
                                &http_client,
                                &endpoint,
                                tx,
                                max_retries,
                                base_delay,
                                &metric_tx,
                            )
                            .await;

                            match result {
                                SubmitResult::Accepted { .. } => {
                                    stats.accepted_count += 1;
                                }
                                SubmitResult::Rejected { .. } => {
                                    stats.rejected_count += 1;
                                }
                                SubmitResult::Failed { .. } => {
                                    stats.failed_count += 1;
                                }
                            }

                            if stats.submitted_count % 1000 == 0 {
                                debug!(
                                    "Worker {} submitted {} transactions",
                                    worker_id, stats.submitted_count
                                );
                            }
                        }
                        _ = shutdown_rx.recv() => {
                            info!("Worker {} received shutdown signal", worker_id);
                            break;
                        }
                        else => {
                            // Channel closed
                            info!("Worker {} channel closed", worker_id);
                            break;
                        }
                    }
                }

                info!(
                    "Worker {} stopped: submitted={}, accepted={}, rejected={}, failed={}",
                    worker_id,
                    stats.submitted_count,
                    stats.accepted_count,
                    stats.rejected_count,
                    stats.failed_count
                );
                stats
            });

            worker_handles.push(handle);
        }

        SubmitterHandle {
            shutdown_tx,
            worker_handles,
        }
    }
}

/// Submit transaction with retry logic and exponential backoff.
async fn submit_with_retry(
    http_client: &reqwest::Client,
    endpoint: &str,
    tx: TxV1,
    max_retries: u32,
    base_delay: Duration,
    metric_tx: &mpsc::UnboundedSender<MetricEvent>,
) -> SubmitResult {
    let txid = txid_v1(&tx).unwrap_or([0u8; 32]); // Should never fail for valid tx
    let start_time = Instant::now();

    // Send submitted event
    let _ = metric_tx.send(MetricEvent::Submitted {
        txid,
        timestamp: start_time,
    });

    let mut attempt = 0;

    loop {
        match submit_once(http_client, endpoint, &tx).await {
            Ok(returned_txid) => {
                let latency = start_time.elapsed();

                // Send accepted event
                let _ = metric_tx.send(MetricEvent::Accepted {
                    txid: returned_txid,
                    latency,
                });

                return SubmitResult::Accepted {
                    txid: returned_txid,
                };
            }
            Err(e) if is_validation_error(&e) => {
                let latency = start_time.elapsed();
                let reason = e.to_string();

                // Send rejected event
                let _ = metric_tx.send(MetricEvent::Rejected {
                    txid,
                    reason: reason.clone(),
                    latency,
                });

                return SubmitResult::Rejected { reason };
            }
            Err(e) => {
                attempt += 1;
                if attempt >= max_retries {
                    let latency = start_time.elapsed();
                    let error = e.to_string();

                    // Send failed event
                    let _ = metric_tx.send(MetricEvent::Failed {
                        txid,
                        error: error.clone(),
                        latency,
                    });

                    return SubmitResult::Failed { error };
                }

                // Exponential backoff
                let delay = base_delay * 2u32.pow(attempt - 1);
                debug!("Retry attempt {} after {:?}: {}", attempt, delay, e);
                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// Submit transaction once (no retries).
async fn submit_once(
    http_client: &reqwest::Client,
    endpoint: &str,
    tx: &TxV1,
) -> Result<TxId, SubmitError> {
    // Encode transaction
    let tx_bytes = encode_tx_v1_signed(tx).map_err(|e| SubmitError::Codec(format!("{:?}", e)))?;
    let tx_hex = hex::encode(&tx_bytes);

    // Build RPC request
    let rpc_request = RpcRequest {
        jsonrpc: "2.0",
        method: "novai_submitTransaction",
        params: serde_json::json!({ "tx": tx_hex }),
        id: 1,
    };

    // Send HTTP request
    let response = http_client
        .post(endpoint)
        .json(&rpc_request)
        .send()
        .await
        .map_err(|e| SubmitError::Network(e.to_string()))?;

    // Parse response
    let rpc_response: RpcResponse<SubmitTxResponse> = response
        .json()
        .await
        .map_err(|e| SubmitError::Parse(e.to_string()))?;

    // Check for RPC error
    if let Some(error) = rpc_response.error {
        return Err(SubmitError::Rpc(error.code, error.message));
    }

    // Extract txid from result
    let result = rpc_response
        .result
        .ok_or_else(|| SubmitError::Parse("Missing result field".to_string()))?;

    let txid_bytes = hex::decode(&result.txid)
        .map_err(|e| SubmitError::Parse(format!("Invalid txid hex: {}", e)))?;

    if txid_bytes.len() != 32 {
        return Err(SubmitError::Parse(format!(
            "Invalid txid length: {}",
            txid_bytes.len()
        )));
    }

    let mut txid = [0u8; 32];
    txid.copy_from_slice(&txid_bytes);

    Ok(txid)
}

/// Check if error is a validation error (should not retry).
fn is_validation_error(error: &SubmitError) -> bool {
    match error {
        SubmitError::Rpc(code, _) => {
            // RPC error codes for validation failures (do not retry)
            // -32000 to -32099 are server errors (may be validation)
            *code >= -32099 && *code <= -32000
        }
        _ => false,
    }
}

/// Submission error types.
#[derive(Debug)]
enum SubmitError {
    /// Codec error (encoding transaction).
    Codec(String),
    /// Network error (connection, timeout).
    Network(String),
    /// Parse error (invalid response).
    Parse(String),
    /// RPC error from server.
    Rpc(i32, String),
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubmitError::Codec(s) => write!(f, "Codec error: {}", s),
            SubmitError::Network(s) => write!(f, "Network error: {}", s),
            SubmitError::Parse(s) => write!(f, "Parse error: {}", s),
            SubmitError::Rpc(code, msg) => write!(f, "RPC error {}: {}", code, msg),
        }
    }
}

/// JSON-RPC request structure.
#[derive(serde::Serialize)]
struct RpcRequest<T> {
    jsonrpc: &'static str,
    method: &'static str,
    params: T,
    id: u64,
}

/// JSON-RPC response structure.
#[derive(serde::Deserialize)]
struct RpcResponse<T> {
    #[allow(dead_code)]
    jsonrpc: String,
    result: Option<T>,
    error: Option<RpcError>,
    #[allow(dead_code)]
    id: u64,
}

/// JSON-RPC error structure.
#[derive(serde::Deserialize)]
struct RpcError {
    code: i32,
    message: String,
}

/// Response from novai_submitTransaction.
#[derive(serde::Deserialize)]
struct SubmitTxResponse {
    txid: String, // Hex-encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_request_serializes_correctly() {
        let req = RpcRequest {
            jsonrpc: "2.0",
            method: "novai_submitTransaction",
            params: serde_json::json!({ "tx": "abcd1234" }),
            id: 42,
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"novai_submitTransaction\""));
        assert!(json.contains("\"id\":42"));
    }

    #[test]
    fn rpc_response_deserializes_success() {
        let json = r#"{"jsonrpc":"2.0","result":{"txid":"abcd1234"},"id":1}"#;
        let response: RpcResponse<SubmitTxResponse> = serde_json::from_str(json).unwrap();

        assert!(response.result.is_some());
        assert!(response.error.is_none());
        assert_eq!(response.result.unwrap().txid, "abcd1234");
    }

    #[test]
    fn rpc_response_deserializes_error() {
        let json = r#"{"jsonrpc":"2.0","error":{"code":-32000,"message":"NonceTooLow"},"id":1}"#;
        let response: RpcResponse<SubmitTxResponse> = serde_json::from_str(json).unwrap();

        assert!(response.result.is_none());
        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert_eq!(error.code, -32000);
        assert_eq!(error.message, "NonceTooLow");
    }

    #[test]
    fn validation_error_check() {
        assert!(is_validation_error(&SubmitError::Rpc(
            -32000,
            "NonceTooLow".to_string()
        )));
        assert!(is_validation_error(&SubmitError::Rpc(
            -32050,
            "InvalidSignature".to_string()
        )));
        assert!(!is_validation_error(&SubmitError::Network(
            "timeout".to_string()
        )));
        assert!(!is_validation_error(&SubmitError::Parse(
            "bad json".to_string()
        )));
    }

    #[tokio::test]
    async fn workers_shutdown_gracefully() {
        use crate::metrics;

        let (tx_sender, tx_receiver) = mpsc::channel(10);
        let (metric_tx, _metric_rx) = metrics::metric_channel();

        let config = SubmitterConfig {
            worker_count: 2,
            ..Default::default()
        };

        let submitter = Submitter::new(config);
        let handle = submitter.start(tx_receiver, metric_tx);

        // Send shutdown signal immediately
        handle.shutdown();

        // Drop sender to close channel
        drop(tx_sender);

        // Wait for workers
        let stats = handle.wait().await;

        // Should have 0 submissions since we shut down immediately
        assert_eq!(stats.total_submitted, 0);
    }
}
