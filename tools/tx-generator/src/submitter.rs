//! Transaction submitter with worker pool and retry logic.
//!
//! INVARIANTS:
//! - Workers claim nonces and sign just before submission (no stale nonces)
//! - MempoolFull triggers retry with backoff (nonce preserved, no gaps)
//! - HTTP 429 triggers rate-limit-aware retry (longer backoff)
//! - Metrics events are sent for every submission attempt
//! - Workers respect shutdown signal
//!
//! FAILURE MODES:
//! - Network unreachable - retries then fails
//! - Node rejects tx (validation) - logged, tx discarded
//! - MempoolFull - retries indefinitely with 2s backoff
//! - Channel closed - workers terminate

use crate::generator::TxTemplate;
use crate::metrics::MetricEvent;
use crate::sender::SenderPool;
use novai_codec::{encode_tx_v1_signed, txid_v1};
use novai_crypto::sign_tx_v1;
use novai_types::{Address, TxId, TxV1, TxVersion};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Maximum retries per transaction (for network errors).
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

/// Number of consecutive NonceTooLow rejections before querying the chain
/// for the correct nonce and resetting the sender.
const NONCE_RESET_THRESHOLD: u32 = 10;

/// Maximum MempoolFull retries before giving up on a single transaction.
/// At 2 seconds per retry, this is ~2 minutes of waiting.
const MAX_MEMPOOL_FULL_RETRIES: u32 = 60;

/// Backoff duration when mempool is full (seconds).
const MEMPOOL_FULL_BACKOFF_SECS: u64 = 2;

/// Backoff duration when rate limited by server (seconds).
const RATE_LIMITED_BACKOFF_SECS: u64 = 1;

/// Transaction submitter with worker pool.
pub struct Submitter {
    config: SubmitterConfig,
    http_client: reqwest::Client,
    sender_pool: Arc<SenderPool>,
    paused: Arc<AtomicBool>,
}

impl Submitter {
    /// Create a new submitter with the given configuration and sender pool.
    ///
    /// The `paused` flag is shared with the generator; workers set it to `true`
    /// when the mempool is full and clear it when submissions resume.
    pub fn new(
        config: SubmitterConfig,
        sender_pool: Arc<SenderPool>,
        paused: Arc<AtomicBool>,
    ) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            config,
            http_client,
            sender_pool,
            paused,
        }
    }

    /// Start the worker pool consuming from the transaction template channel.
    /// Sends metric events to the provided channel.
    pub fn start(
        self,
        tx_receiver: mpsc::Receiver<TxTemplate>,
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
            let sender_pool = Arc::clone(&self.sender_pool);
            let paused = Arc::clone(&self.paused);

            let handle = tokio::spawn(async move {
                let mut stats = WorkerStats::default();
                // Track consecutive NonceTooLow rejections per sender for nonce reset
                let mut consecutive_nonce_errors: HashMap<Address, u32> = HashMap::new();

                loop {
                    tokio::select! {
                        Some(template) = async {
                            let mut rx = tx_receiver.lock().await;
                            rx.recv().await
                        } => {
                            stats.submitted_count += 1;
                            let sender_address = template.sender.address;

                            // Claim nonce and sign just before submission
                            let nonce = template.sender.claim_nonce();
                            let mut tx = TxV1 {
                                version: TxVersion::V1,
                                from: template.sender.address,
                                pubkey: template.sender.verifying_key.to_bytes(),
                                nonce,
                                fee: template.fee,
                                payload: template.payload,
                                sig: [0u8; 64],
                            };

                            if let Err(e) = sign_tx_v1(&template.sender.signing_key, &mut tx) {
                                warn!("Failed to sign tx: {:?}", e);
                                stats.failed_count += 1;
                                continue;
                            }

                            let result = submit_with_retry(
                                &http_client,
                                &endpoint,
                                tx,
                                max_retries,
                                base_delay,
                                &metric_tx,
                                &paused,
                            )
                            .await;

                            match result {
                                SubmitResult::Accepted { .. } => {
                                    stats.accepted_count += 1;
                                    // Clear nonce error streak on success
                                    consecutive_nonce_errors.remove(&sender_address);
                                }
                                SubmitResult::Rejected { ref reason } => {
                                    stats.rejected_count += 1;
                                    // Track NonceTooLow for nonce reset.
                                    // Uses code-based detection (not string matching)
                                    // since H-06 sanitized error messages.
                                    if reason.starts_with("NonceTooLow") {
                                        let count = consecutive_nonce_errors
                                            .entry(sender_address)
                                            .or_insert(0);
                                        *count += 1;
                                        if *count >= NONCE_RESET_THRESHOLD {
                                            if let Some(sender) =
                                                sender_pool.find_by_address(&sender_address)
                                            {
                                                let old_nonce = sender.current_nonce();
                                                let chain_nonce = query_chain_nonce(
                                                    &http_client,
                                                    &endpoint,
                                                    &sender_address,
                                                )
                                                .await;
                                                let new_nonce = chain_nonce.unwrap_or(0);
                                                sender.reset_nonce(new_nonce);
                                                info!(
                                                    worker_id,
                                                    sender = ?&sender_address[..4],
                                                    old_nonce,
                                                    new_nonce,
                                                    from_chain = chain_nonce.is_some(),
                                                    "Nonce reset after {} NonceTooLow errors",
                                                    NONCE_RESET_THRESHOLD
                                                );
                                            }
                                            *count = 0;
                                        }
                                    } else {
                                        // Non-nonce rejection: clear nonce error streak
                                        consecutive_nonce_errors.remove(&sender_address);
                                    }
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

/// Submit transaction with retry logic, MempoolFull backoff, and rate limit handling.
async fn submit_with_retry(
    http_client: &reqwest::Client,
    endpoint: &str,
    tx: TxV1,
    max_retries: u32,
    base_delay: Duration,
    metric_tx: &mpsc::UnboundedSender<MetricEvent>,
    paused: &AtomicBool,
) -> SubmitResult {
    let txid = txid_v1(&tx).unwrap_or([0u8; 32]); // Should never fail for valid tx
    let start_time = Instant::now();

    // Send submitted event
    let _ = metric_tx.send(MetricEvent::Submitted {
        txid,
        timestamp: start_time,
    });

    let mut attempt = 0;
    let mut mempool_full_retries = 0u32;

    loop {
        match submit_once(http_client, endpoint, &tx).await {
            Ok(returned_txid) => {
                let latency = start_time.elapsed();

                // Unpause generator on success (mempool has capacity)
                if paused.load(Ordering::Relaxed) {
                    paused.store(false, Ordering::Relaxed);
                    info!("Mempool has capacity, resuming generator");
                }

                // Send accepted event
                let _ = metric_tx.send(MetricEvent::Accepted {
                    txid: returned_txid,
                    latency,
                });

                return SubmitResult::Accepted {
                    txid: returned_txid,
                };
            }
            Err(SubmitError::MempoolFull(msg)) => {
                mempool_full_retries += 1;
                if mempool_full_retries >= MAX_MEMPOOL_FULL_RETRIES {
                    let latency = start_time.elapsed();
                    let reason = format!(
                        "MempoolFull after {} retries: {}",
                        mempool_full_retries, msg
                    );
                    let _ = metric_tx.send(MetricEvent::Failed {
                        txid,
                        error: reason.clone(),
                        latency,
                    });
                    return SubmitResult::Failed { error: reason };
                }

                // Pause the generator so it stops flooding the channel
                if !paused.load(Ordering::Relaxed) {
                    paused.store(true, Ordering::Relaxed);
                    info!(
                        "Mempool full, pausing generator (retry {}/{})",
                        mempool_full_retries, MAX_MEMPOOL_FULL_RETRIES
                    );
                }

                tokio::time::sleep(Duration::from_secs(MEMPOOL_FULL_BACKOFF_SECS)).await;
                continue;
            }
            Err(SubmitError::NonceTooLow(msg)) => {
                // NonceTooLow is a rejection — return immediately so the
                // worker's nonce resync logic can handle it.
                let latency = start_time.elapsed();
                let reason = format!("NonceTooLow: {msg}");
                let _ = metric_tx.send(MetricEvent::Rejected {
                    txid,
                    reason: reason.clone(),
                    latency,
                });
                return SubmitResult::Rejected { reason };
            }
            Err(SubmitError::RateLimited) => {
                debug!(
                    "Rate limited by server, backing off {}s",
                    RATE_LIMITED_BACKOFF_SECS
                );
                tokio::time::sleep(Duration::from_secs(RATE_LIMITED_BACKOFF_SECS)).await;
                continue; // Retry indefinitely for rate limiting
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

    // Check HTTP status before parsing JSON body
    let status = response.status().as_u16();
    if status == 429 {
        return Err(SubmitError::RateLimited);
    }

    // Parse response
    let rpc_response: RpcResponse<SubmitTxResponse> = response
        .json()
        .await
        .map_err(|e| SubmitError::Parse(e.to_string()))?;

    // Check for RPC error
    if let Some(error) = rpc_response.error {
        // Distinguish rejection types by error CODE (not message string).
        // Codes defined in crates/node/src/rpc.rs handle_submit_tx.
        match error.code {
            -32001 => return Err(SubmitError::MempoolFull(error.message)),
            -32010 => return Err(SubmitError::NonceTooLow(error.message)),
            _ => return Err(SubmitError::Rpc(error.code, error.message)),
        }
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

/// Query the chain's expected nonce for an address via RPC.
///
/// Returns `Some(nonce)` on success, `None` if the RPC call fails
/// (in which case the caller falls back to 0).
async fn query_chain_nonce(
    http_client: &reqwest::Client,
    endpoint: &str,
    address: &Address,
) -> Option<u64> {
    let rpc_request = RpcRequest {
        jsonrpc: "2.0",
        method: "novai_getNonce",
        params: serde_json::json!({ "address": hex::encode(address) }),
        id: 1,
    };

    let response = http_client
        .post(endpoint)
        .json(&rpc_request)
        .send()
        .await
        .ok()?;

    let rpc_response: RpcResponse<GetNonceResponse> = response.json().await.ok()?;

    if rpc_response.error.is_some() {
        return None;
    }

    rpc_response.result.map(|r| r.nonce)
}

/// Response from novai_getNonce.
#[derive(serde::Deserialize)]
struct GetNonceResponse {
    nonce: u64,
}

/// Check if error is a validation error (should not retry).
fn is_validation_error(error: &SubmitError) -> bool {
    match error {
        SubmitError::Rpc(code, _) => {
            // RPC error codes for validation failures (do not retry).
            // -32001 (MempoolFull) and -32010 (NonceTooLow) are handled
            // separately before reaching this check.
            *code >= -32099 && *code <= -32000
        }
        // NonceTooLow is NOT a validation error — it needs nonce resync, not discard.
        SubmitError::NonceTooLow(_) => false,
        // MempoolFull is NOT a validation error — handled separately with backoff.
        SubmitError::MempoolFull(_) => false,
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
    /// Mempool full — retry with backoff.
    MempoolFull(String),
    /// Nonce too low — sender needs nonce resync.
    NonceTooLow(String),
    /// HTTP 429 rate limited — retry with backoff.
    RateLimited,
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubmitError::Codec(s) => write!(f, "Codec error: {}", s),
            SubmitError::Network(s) => write!(f, "Network error: {}", s),
            SubmitError::Parse(s) => write!(f, "Parse error: {}", s),
            SubmitError::Rpc(code, msg) => write!(f, "RPC error {}: {}", code, msg),
            SubmitError::MempoolFull(msg) => write!(f, "Mempool full: {}", msg),
            SubmitError::NonceTooLow(msg) => write!(f, "Nonce too low: {}", msg),
            SubmitError::RateLimited => write!(f, "Rate limited (HTTP 429)"),
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
        // MempoolFull is NOT a validation error (handled separately)
        assert!(!is_validation_error(&SubmitError::MempoolFull(
            "full".to_string()
        )));
        // NonceTooLow is NOT a validation error (needs nonce resync)
        assert!(!is_validation_error(&SubmitError::NonceTooLow(
            "expected 5, got 0".to_string()
        )));
    }

    #[test]
    fn mempool_full_error_display() {
        let err = SubmitError::MempoolFull("at capacity".to_string());
        assert_eq!(format!("{}", err), "Mempool full: at capacity");
    }

    #[test]
    fn rate_limited_error_display() {
        let err = SubmitError::RateLimited;
        assert_eq!(format!("{}", err), "Rate limited (HTTP 429)");
    }

    #[tokio::test]
    async fn workers_shutdown_gracefully() {
        use crate::metrics;
        use crate::sender::SenderPool;

        let (tx_sender, tx_receiver) = mpsc::channel(10);
        let (metric_tx, _metric_rx) = metrics::metric_channel();

        let config = SubmitterConfig {
            worker_count: 2,
            ..Default::default()
        };

        let pool = Arc::new(SenderPool::new(1));
        let paused = Arc::new(AtomicBool::new(false));
        let submitter = Submitter::new(config, pool, paused);
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
