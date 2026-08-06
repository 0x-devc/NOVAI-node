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
use crate::sender::{SenderAccount, SenderPool};
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

/// Maximum SenderLimitExceeded retries before giving up on a single transaction.
/// At 200 ms per retry, this is ~4 seconds of waiting.
const MAX_SENDER_LIMIT_RETRIES: u32 = 20;

/// Maximum RateLimited retries before giving up on a single transaction.
/// At RATE_LIMITED_BACKOFF_SECS per retry, this is a bounded backoff window.
const MAX_RATE_LIMITED_RETRIES: u32 = 12;

/// Worker heartbeat interval. Each worker's tokio::select! includes a sleep
/// arm at this interval so a worker parked on rx.recv() cannot be wedged
/// indefinitely if upstream stops sending. On fire the worker continues the
/// loop and re-attempts recv; tokio's mpsc::Receiver::recv and tokio::sync::Mutex
/// are cancellation-safe so no template is lost.
const HEARTBEAT_DURATION: Duration = Duration::from_secs(1);

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
                // Last resync attempt per sender, so a failed query that leaves
                // the streak armed cannot turn every subsequent rejection into
                // another query against an already-degraded endpoint.
                let mut last_resync_attempt: HashMap<Address, Instant> = HashMap::new();

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
                                        // Saturating: the streak now stays armed across a
                                        // failed resync, so against a permanently dead
                                        // endpoint it would otherwise climb without bound.
                                        *count = count.saturating_add(1);
                                        if *count >= NONCE_RESET_THRESHOLD {
                                            let now = Instant::now();
                                            let due = match last_resync_attempt
                                                .get(&sender_address)
                                            {
                                                Some(at) => {
                                                    now.duration_since(*at) >= RESYNC_MIN_INTERVAL
                                                }
                                                None => true,
                                            };
                                            match sender_pool.find_by_address(&sender_address) {
                                                Some(sender) if due => {
                                                    last_resync_attempt
                                                        .insert(sender_address, now);
                                                    if resync_one_sender(
                                                        &http_client,
                                                        &endpoint,
                                                        &sender,
                                                    )
                                                    .await
                                                    {
                                                        // Corrected from a value read off the
                                                        // chain: the streak has been answered.
                                                        *count = 0;
                                                    }
                                                    // Query failed: leave the streak armed so
                                                    // the next rejection past the cooldown
                                                    // retries, and leave the nonce untouched.
                                                }
                                                Some(_) => {
                                                    // Inside the cooldown. Leave the streak
                                                    // armed and wait.
                                                }
                                                None => {
                                                    // Sender is not in this pool, nothing to
                                                    // correct.
                                                    *count = 0;
                                                }
                                            }
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
                        _ = tokio::time::sleep(HEARTBEAT_DURATION) => {
                            // Periodic wakeup. The worker re-attempts recv on
                            // the next loop iteration so a parked recv()
                            // cannot wedge indefinitely if upstream stops
                            // sending. recv and Mutex are cancellation-safe
                            // so no template is lost when this arm fires.
                            continue;
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
    let mut sender_limit_retries = 0u32;
    let mut rate_limited_retries = 0u32;

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
                    // Exhausted: clear paused so the generator (and any
                    // future workers) can resume. Leaving paused=true after
                    // a Failed return strands subsequent workers on recv.
                    if paused.load(Ordering::Relaxed) {
                        paused.store(false, Ordering::Relaxed);
                        warn!(
                            "MempoolFull retries exhausted ({}), clearing pause",
                            mempool_full_retries
                        );
                    }
                    let latency = start_time.elapsed();
                    let reason = format!("MempoolFull after {mempool_full_retries} retries: {msg}");
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
            Err(SubmitError::SenderLimitExceeded(msg)) => {
                // Sender has too many pending txs in the mempool. Backoff
                // briefly and retry: the pending txs will drain into blocks.
                // The retry loop is bounded so a permanently-stuck mempool
                // does not wedge the worker indefinitely.
                //
                // CONSEQUENCE OF EXHAUSTION: the nonce claimed for this tx
                // is now a permanent gap for this sender. Future txs from
                // this sender will be rejected at the node until the sender
                // is restarted with a fresh nonce. Acceptable for a load
                // tool with many senders (the worker just rotates to the
                // next sender in the pool).
                sender_limit_retries += 1;
                if sender_limit_retries >= MAX_SENDER_LIMIT_RETRIES {
                    let latency = start_time.elapsed();
                    let reason =
                        format!("SenderLimitExceeded after {sender_limit_retries} retries: {msg}");
                    warn!(
                        "SenderLimitExceeded retries exhausted ({}), giving up on this tx (sender nonce gap until restart)",
                        sender_limit_retries
                    );
                    let _ = metric_tx.send(MetricEvent::Failed {
                        txid,
                        error: reason.clone(),
                        latency,
                    });
                    return SubmitResult::Failed { error: reason };
                }
                debug!(
                    "SenderLimitExceeded, backing off 200ms (retry {}/{})",
                    sender_limit_retries, MAX_SENDER_LIMIT_RETRIES
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
            Err(SubmitError::RateLimited) => {
                rate_limited_retries += 1;
                if rate_limited_retries >= MAX_RATE_LIMITED_RETRIES {
                    let latency = start_time.elapsed();
                    let reason =
                        format!("RateLimited after {rate_limited_retries} retries (HTTP 429)");
                    warn!(
                        "RateLimited retries exhausted ({}), giving up on this tx",
                        rate_limited_retries
                    );
                    let _ = metric_tx.send(MetricEvent::Failed {
                        txid,
                        error: reason.clone(),
                        latency,
                    });
                    return SubmitResult::Failed { error: reason };
                }
                debug!(
                    "Rate limited by server, backing off {}s (retry {}/{})",
                    RATE_LIMITED_BACKOFF_SECS, rate_limited_retries, MAX_RATE_LIMITED_RETRIES
                );
                tokio::time::sleep(Duration::from_secs(RATE_LIMITED_BACKOFF_SECS)).await;
                continue;
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
    let tx_bytes = encode_tx_v1_signed(tx).map_err(|e| SubmitError::Codec(format!("{e:?}")))?;
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
            -32012 => return Err(SubmitError::SenderLimitExceeded(error.message)),
            _ => return Err(SubmitError::Rpc(error.code, error.message)),
        }
    }

    // Extract txid from result
    let result = rpc_response
        .result
        .ok_or_else(|| SubmitError::Parse("Missing result field".to_string()))?;

    let txid_bytes = hex::decode(&result.txid)
        .map_err(|e| SubmitError::Parse(format!("Invalid txid hex: {e}")))?;

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

/// Single novai_getNonce round trip.
///
/// novai_getNonce returns the node's `expected_nonce` (rpc.rs
/// `handle_get_nonce` into `InMemoryNonceProvider`), which is the exact value
/// `TxMempool::insert` compares against at admission, so it is the right
/// answer to "what nonce will the node accept next". novai_getBalance returns
/// the committed account row instead, which can lag whenever a transaction
/// commits but execution skips it. Neither counts pooled transactions.
///
/// Every failure is surfaced as an error rather than collapsed into a
/// default. The caller must never write a nonce it did not read.
async fn query_nonce_once(
    http_client: &reqwest::Client,
    endpoint: &str,
    address: &Address,
) -> anyhow::Result<u64> {
    use anyhow::{anyhow, bail};

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
        .map_err(|e| anyhow!("network error: {e}"))?;

    let status = response.status().as_u16();
    if status == 429 {
        bail!("rate limited (HTTP 429)");
    }

    let rpc_response: RpcResponse<GetNonceResponse> = response
        .json()
        .await
        .map_err(|e| anyhow!("parse error (HTTP {status}): {e}"))?;

    if let Some(err) = rpc_response.error {
        bail!("RPC error {}: {}", err.code, err.message);
    }

    rpc_response
        .result
        .map(|r| r.nonce)
        .ok_or_else(|| anyhow!("RPC response carried neither result nor error"))
}

/// Query one sender's chain nonce with bounded retries.
///
/// Retries transient failures (network errors, HTTP 429 from the node's
/// per-IP RPC rate limit, malformed bodies). The rate limit is the one that
/// matters: the mid-run resync fires precisely when the transport is under
/// pressure, which is exactly when a single-shot query is most likely to be
/// rate limited, so a resync that treats 429 as fatal fails hardest at the
/// moment it is needed most.
async fn query_nonce_retrying(
    http_client: &reqwest::Client,
    endpoint: &str,
    address: &Address,
) -> anyhow::Result<u64> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match query_nonce_once(http_client, endpoint, address).await {
            Ok(nonce) => return Ok(nonce),
            Err(e) if attempt >= NONCE_QUERY_MAX_ATTEMPTS => {
                return Err(e.context(format!("nonce query failed after {attempt} attempts")));
            }
            Err(e) => {
                warn!(attempt, error = %e, "nonce query failed, retrying");
                tokio::time::sleep(NONCE_QUERY_BACKOFF).await;
            }
        }
    }
}

/// Resync one sender's local nonce to the chain's expected nonce.
///
/// Returns true when the local nonce was corrected from a value that was
/// actually read from the chain, false when the chain nonce could not be
/// determined.
async fn resync_one_sender(
    http_client: &reqwest::Client,
    endpoint: &str,
    sender: &SenderAccount,
) -> bool {
    let old_nonce = sender.current_nonce();
    match query_nonce_retrying(http_client, endpoint, &sender.address).await {
        Ok(chain_nonce) => {
            sender.reset_nonce(chain_nonce);
            info!(
                sender = ?&sender.address[..4],
                old_nonce,
                new_nonce = chain_nonce,
                "sender resynced to chain nonce"
            );
            true
        }
        Err(e) => {
            // Never write a nonce that was not read from the chain. Leaving
            // the local value alone keeps the sender submitting at its
            // current nonce, which stays recoverable; writing a guess does
            // not, because nonce 0 against a live chain is rejected as
            // NonceTooLow forever and every rejection burns another nonce.
            warn!(
                sender = ?&sender.address[..4],
                old_nonce,
                error = %e,
                "nonce resync query failed, local nonce left unchanged"
            );
            false
        }
    }
}

/// Response from novai_getNonce.
#[derive(serde::Deserialize)]
struct GetNonceResponse {
    nonce: u64,
}

/// Maximum attempts per sender for any chain nonce query, startup or mid run.
const NONCE_QUERY_MAX_ATTEMPTS: u32 = 5;

/// Backoff between nonce query attempts (covers HTTP 429 from the node's
/// per-IP RPC rate limit and transient network errors).
const NONCE_QUERY_BACKOFF: Duration = Duration::from_millis(250);

/// Minimum interval between mid-run resync attempts for one sender.
///
/// The streak counter stays armed after a failed query so the next rejection
/// retries it, which without a floor would turn a degraded endpoint into a
/// query storm from every worker at once. This is the "do not resync into a
/// degraded transport" rule.
const RESYNC_MIN_INTERVAL: Duration = Duration::from_secs(5);

/// Resync every sender in the pool to its current on-chain nonce.
///
/// Called once at startup, after the pool is built and before any load
/// begins. Sender accounts are deterministic and long-lived, so after a
/// prior run their on-chain nonces are far above 0; without this resync
/// the generator submits nonce 0 and the node rejects every tx with
/// NonceTooLow (-32010).
///
/// Fails loud: if any sender's nonce cannot be determined after bounded
/// retries, the whole resync errors and the generator must not start.
/// Never silently submits a wrong nonce.
pub async fn resync_sender_nonces(
    http_client: &reqwest::Client,
    endpoint: &str,
    pool: &SenderPool,
) -> anyhow::Result<()> {
    use anyhow::Context;

    let mut min_nonce = u64::MAX;
    let mut max_nonce = 0u64;

    for account in pool.all_accounts() {
        let nonce = query_nonce_retrying(http_client, endpoint, &account.address)
            .await
            .with_context(|| {
                format!(
                    "startup nonce resync failed for sender {} (address {}); \
                     refusing to start load with unknown nonces",
                    account.index,
                    hex::encode(account.address)
                )
            })?;
        account.reset_nonce(nonce);
        info!(
            sender = account.index,
            nonce, "sender resynced to on-chain nonce"
        );
        min_nonce = min_nonce.min(nonce);
        max_nonce = max_nonce.max(nonce);
    }

    info!(
        senders = pool.len(),
        min_nonce, max_nonce, "startup nonce resync complete"
    );
    Ok(())
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
        // SenderLimitExceeded is NOT a validation error — backoff and retry.
        SubmitError::SenderLimitExceeded(_) => false,
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
    /// Per-sender mempool limit exceeded — backoff and retry.
    SenderLimitExceeded(String),
    /// HTTP 429 rate limited — retry with backoff.
    RateLimited,
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubmitError::Codec(s) => write!(f, "Codec error: {s}"),
            SubmitError::Network(s) => write!(f, "Network error: {s}"),
            SubmitError::Parse(s) => write!(f, "Parse error: {s}"),
            SubmitError::Rpc(code, msg) => write!(f, "RPC error {code}: {msg}"),
            SubmitError::MempoolFull(msg) => write!(f, "Mempool full: {msg}"),
            SubmitError::NonceTooLow(msg) => write!(f, "Nonce too low: {msg}"),
            SubmitError::SenderLimitExceeded(msg) => write!(f, "Sender limit exceeded: {msg}"),
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

/// Response from novai_getLatestBlock (subset of fields needed for monitoring).
#[derive(serde::Deserialize)]
struct LatestBlockResponse {
    height: u64,
}

/// Monitors chain progress and logs a warning when the chain height
/// stops advancing for a configurable threshold.
///
/// Advisory only: this monitor does not pause the generator or workers.
/// Its sole purpose is to surface chain-stall conditions as an operator-
/// visible log line (e.g., during a leader stall, partition, or resource
/// exhaustion event). Tx submission backpressure is handled exclusively
/// by the MempoolFull retry path in submit_with_retry. A previous version
/// of this type flipped a shared `paused` flag on stall detection; that
/// path could leave workers permanently parked on recv if the monitor
/// task died silently after setting the flag, so it was removed.
pub struct ChainMonitor {
    endpoint: String,
    poll_interval: Duration,
    stall_threshold: Duration,
    http_client: reqwest::Client,
}

impl ChainMonitor {
    /// Create a new chain monitor that polls `endpoint` every
    /// `poll_interval` and warns once when no height advance has been
    /// observed for `stall_threshold`.
    pub fn new(endpoint: String, poll_interval: Duration, stall_threshold: Duration) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("Failed to create HTTP client for ChainMonitor");
        Self {
            endpoint,
            poll_interval,
            stall_threshold,
            http_client,
        }
    }

    /// Spawn the monitor task. Returns a handle that can be aborted on
    /// shutdown. The task runs forever until aborted.
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(self.run())
    }

    async fn run(self) {
        info!(
            "Chain monitor started (advisory): poll_interval={:?}, stall_threshold={:?}",
            self.poll_interval, self.stall_threshold
        );
        let mut last_height: Option<u64> = None;
        let mut last_advance = Instant::now();
        let mut stalled = false;

        loop {
            tokio::time::sleep(self.poll_interval).await;
            match self.fetch_height().await {
                Ok(Some(h)) => {
                    // Per-poll heartbeat so silent monitor death is observable.
                    debug!(height = h, stalled, "Chain monitor poll");
                    if Some(h) != last_height {
                        last_height = Some(h);
                        last_advance = Instant::now();
                        if stalled {
                            info!(height = h, "Chain progress resumed (advisory)");
                            stalled = false;
                        }
                    } else if !stalled && last_advance.elapsed() >= self.stall_threshold {
                        warn!(
                            height = h,
                            elapsed_ms = last_advance.elapsed().as_millis() as u64,
                            "Chain stalled (advisory, no pause action taken)"
                        );
                        stalled = true;
                    }
                }
                Ok(None) => {
                    last_advance = Instant::now();
                }
                Err(e) => {
                    debug!("Chain monitor RPC error: {}", e);
                }
            }
        }
    }

    async fn fetch_height(&self) -> Result<Option<u64>, String> {
        let rpc_request = RpcRequest {
            jsonrpc: "2.0",
            method: "novai_getLatestBlock",
            params: serde_json::json!([]),
            id: 1,
        };
        let response = self
            .http_client
            .post(&self.endpoint)
            .json(&rpc_request)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let rpc_response: RpcResponse<LatestBlockResponse> =
            response.json().await.map_err(|e| e.to_string())?;
        if let Some(err) = rpc_response.error {
            return Err(format!("RPC error {}: {}", err.code, err.message));
        }
        Ok(rpc_response.result.map(|r| r.height))
    }
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
        assert_eq!(format!("{err}"), "Mempool full: at capacity");
    }

    #[test]
    fn rate_limited_error_display() {
        let err = SubmitError::RateLimited;
        assert_eq!(format!("{err}"), "Rate limited (HTTP 429)");
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

    // ============================================================
    // Regression tests for the lockup fix:
    //
    // 1. worker_wakes_within_heartbeat_when_template_arrives_late
    //    confirms the worker select loop still processes templates
    //    after the heartbeat arm has cycled, i.e. the heartbeat
    //    does not break the recv path.
    // 2. mempool_full_clears_paused_on_exhaustion confirms a worker
    //    that hits MAX_MEMPOOL_FULL_RETRIES does not strand the
    //    paused flag at true (the original architectural defect).
    // 3. sender_limit_exceeded_bounded confirms the SenderLimitExceeded
    //    retry loop is no longer unbounded.
    // 4. rate_limited_bounded confirms the RateLimited retry loop is
    //    no longer unbounded.
    //
    // The three bound tests use paused time so they complete in test
    // budget rather than real-time backoff (~120s for mempool, ~12s
    // for rate-limited, ~4s for sender-limit). The heartbeat test
    // uses real time because its timing assertion only makes sense
    // against the wall clock.
    // ============================================================

    fn make_test_tx() -> TxV1 {
        let acc = crate::sender::SenderAccount::from_index(0);
        TxV1 {
            version: TxVersion::V1,
            from: acc.address,
            pubkey: acc.verifying_key.to_bytes(),
            nonce: 0,
            fee: 1,
            payload: vec![],
            sig: [0u8; 64],
        }
    }

    #[tokio::test]
    async fn worker_wakes_within_heartbeat_when_template_arrives_late() {
        use crate::generator::TxTemplate;
        use crate::metrics;
        use crate::sender::SenderPool;

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"jsonrpc":"2.0","result":{"txid":"0000000000000000000000000000000000000000000000000000000000000000"},"id":1}"#,
            )
            .expect_at_least(1)
            .create_async()
            .await;

        let (tx_sender, tx_receiver) = mpsc::channel(10);
        let (metric_tx, _metric_rx) = metrics::metric_channel();
        let config = SubmitterConfig {
            endpoint: server.url(),
            worker_count: 1,
            ..Default::default()
        };
        let pool = Arc::new(SenderPool::new(1));
        let paused = Arc::new(AtomicBool::new(false));
        let submitter = Submitter::new(config, Arc::clone(&pool), paused);
        let handle = submitter.start(tx_receiver, metric_tx);

        // Stay idle past HEARTBEAT_DURATION so the heartbeat arm cycles
        // at least once before any template is sent.
        tokio::time::sleep(HEARTBEAT_DURATION + Duration::from_millis(300)).await;

        let sender = pool.next_sender();
        let template = TxTemplate {
            sender,
            fee: 1,
            payload: vec![],
        };
        tx_sender
            .send(template)
            .await
            .expect("channel not closed before send");

        // Allow recv + submit + mock response.
        tokio::time::sleep(Duration::from_millis(500)).await;

        handle.shutdown();
        drop(tx_sender);
        let stats = handle.wait().await;

        assert_eq!(
            stats.total_submitted, 1,
            "worker should have processed the late-arriving template"
        );
        assert_eq!(
            stats.total_accepted, 1,
            "mock returned success, accepted count should be 1"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn mempool_full_clears_paused_on_exhaustion() {
        use crate::metrics;

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"jsonrpc":"2.0","error":{"code":-32001,"message":"MempoolFull"},"id":1}"#,
            )
            .expect_at_least(MAX_MEMPOOL_FULL_RETRIES as usize)
            .create_async()
            .await;

        let http_client = reqwest::Client::new();
        let (metric_tx, _metric_rx) = metrics::metric_channel();
        // Start with paused=true to mimic the architectural-defect scenario
        // where a previous worker has already engaged the pause.
        let paused = AtomicBool::new(true);
        let tx = make_test_tx();

        let result = submit_with_retry(
            &http_client,
            &server.url(),
            tx,
            3,
            Duration::from_millis(10),
            &metric_tx,
            &paused,
        )
        .await;

        match result {
            SubmitResult::Failed { .. } => {}
            other => {
                panic!("expected SubmitResult::Failed after MempoolFull exhaustion, got {other:?}")
            }
        }
        assert!(
            !paused.load(Ordering::Relaxed),
            "paused must be cleared on MempoolFull exhaustion so subsequent workers are not stranded"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn sender_limit_exceeded_bounded() {
        use crate::metrics;

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"jsonrpc":"2.0","error":{"code":-32012,"message":"SenderLimitExceeded"},"id":1}"#,
            )
            .expect_at_least(MAX_SENDER_LIMIT_RETRIES as usize)
            .create_async()
            .await;

        let http_client = reqwest::Client::new();
        let (metric_tx, _metric_rx) = metrics::metric_channel();
        let paused = AtomicBool::new(false);
        let tx = make_test_tx();

        let result = submit_with_retry(
            &http_client,
            &server.url(),
            tx,
            3,
            Duration::from_millis(10),
            &metric_tx,
            &paused,
        )
        .await;

        match result {
            SubmitResult::Failed { error } => {
                assert!(
                    error.contains("SenderLimitExceeded"),
                    "expected SenderLimitExceeded in error, got {error}"
                );
            }
            other => panic!(
                "expected SubmitResult::Failed after SenderLimitExceeded exhaustion, got {other:?}"
            ),
        }
    }

    // ============================================================
    // Startup nonce resync tests.
    //
    // The bug these pin: sender accounts are deterministic and
    // long-lived, so their on-chain nonces are far above 0 after any
    // prior run (measured live: ~272), but SenderAccount::from_index
    // always starts at nonce 0. Startup must resync every sender to
    // the chain's nonce via novai_getBalance before load begins.
    // ============================================================

    #[tokio::test]
    async fn startup_resync_initializes_sender_to_chain_nonce() {
        use crate::sender::SenderPool;

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","result":{"balance":"998000000","nonce":272},"id":1}"#)
            .expect_at_least(1)
            .create_async()
            .await;

        let pool = SenderPool::new(1);
        let client = reqwest::Client::new();

        resync_sender_nonces(&client, &server.url(), &pool)
            .await
            .expect("resync must succeed when the RPC answers");

        let sender = pool.get_sender(0).unwrap();
        assert_eq!(
            sender.claim_nonce(),
            272,
            "first claimed nonce must be the on-chain nonce, not 0"
        );
        assert_eq!(
            sender.claim_nonce(),
            273,
            "nonces continue from the resynced value"
        );
    }

    #[tokio::test]
    async fn startup_resync_keeps_fresh_sender_at_zero() {
        use crate::sender::SenderPool;

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","result":{"balance":"0","nonce":0},"id":1}"#)
            .expect_at_least(1)
            .create_async()
            .await;

        let pool = SenderPool::new(1);
        let client = reqwest::Client::new();

        resync_sender_nonces(&client, &server.url(), &pool)
            .await
            .expect("resync must succeed for a fresh account");

        let sender = pool.get_sender(0).unwrap();
        assert_eq!(sender.claim_nonce(), 0, "fresh account stays at nonce 0");
    }

    #[tokio::test(start_paused = true)]
    async fn startup_resync_fails_loud_when_query_fails() {
        use crate::sender::SenderPool;

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(500)
            .with_body("internal error")
            .expect_at_least(NONCE_QUERY_MAX_ATTEMPTS as usize)
            .create_async()
            .await;

        let pool = SenderPool::new(1);
        let client = reqwest::Client::new();

        let result = resync_sender_nonces(&client, &server.url(), &pool).await;
        assert!(
            result.is_err(),
            "resync must fail loud (not silently keep nonce 0) when the nonce query fails"
        );
    }

    // ============================================================
    // Gate SOAK phase 1 (B1, B2): the mid-run nonce resync must never
    // write a nonce it did not read from the chain, and must treat a
    // rate limit as transient rather than fatal.
    //
    // Pre-fix, resync_one_sender did `chain_nonce.unwrap_or(0)` on a
    // single-shot query with no 429 guard, so one rate-limited round
    // trip reset a live sender to nonce 0 and every subsequent tx from
    // that sender was rejected NonceTooLow forever. That is the
    // poisoning path that takes the devnet down overnight.
    // ============================================================

    /// One failure shape: the sender's local nonce must survive it intact.
    async fn assert_local_nonce_survives(status: usize, body: &str, label: &str) {
        use crate::sender::SenderAccount;

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(status)
            .with_body(body)
            .expect_at_least(1)
            .create_async()
            .await;

        let sender = SenderAccount::from_index(0);
        sender.reset_nonce(300);

        let corrected =
            resync_one_sender(&reqwest::Client::new(), &server.url(), &sender).await;

        assert!(
            !corrected,
            "{label}: resync must report no correction when the chain nonce is unknown"
        );
        assert_eq!(
            sender.current_nonce(),
            300,
            "{label}: local nonce must be left untouched when the query fails, never reset to 0"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn resync_never_writes_a_nonce_it_did_not_read() {
        assert_local_nonce_survives(429, "Too Many Requests", "http 429").await;
        assert_local_nonce_survives(500, "internal error", "http 500").await;
        assert_local_nonce_survives(200, "not json at all", "malformed body").await;
    }

    #[tokio::test(start_paused = true)]
    async fn resync_retries_through_a_rate_limit_then_succeeds() {
        use crate::sender::SenderAccount;

        let mut server = mockito::Server::new_async().await;
        let _rate_limited = server
            .mock("POST", "/")
            .with_status(429)
            .with_body("Too Many Requests")
            .expect(1)
            .create_async()
            .await;
        let _ok = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","result":{"nonce":412},"id":1}"#)
            .expect_at_least(1)
            .create_async()
            .await;

        let sender = SenderAccount::from_index(0);
        sender.reset_nonce(7);

        let corrected =
            resync_one_sender(&reqwest::Client::new(), &server.url(), &sender).await;

        assert!(
            corrected,
            "a 429 is transient and must be retried, not treated as a hard failure"
        );
        assert_eq!(
            sender.current_nonce(),
            412,
            "after the retry succeeds the sender must hold the chain nonce"
        );
    }

    /// D3: startup and mid-run must both read the value admission actually
    /// gates on, which is novai_getNonce (rpc.rs:2090 -> expected_nonce),
    /// not novai_getBalance's committed account row.
    #[tokio::test(start_paused = true)]
    async fn startup_resync_queries_novai_get_nonce() {
        use crate::sender::SenderPool;

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "method": "novai_getNonce"
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","result":{"nonce":272},"id":1}"#)
            .expect_at_least(1)
            .create_async()
            .await;

        let pool = SenderPool::new(1);

        resync_sender_nonces(&reqwest::Client::new(), &server.url(), &pool)
            .await
            .expect("startup resync must succeed against a novai_getNonce endpoint");

        assert_eq!(pool.get_sender(0).unwrap().current_nonce(), 272);
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limited_bounded() {
        use crate::metrics;

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/")
            .with_status(429)
            .expect_at_least(MAX_RATE_LIMITED_RETRIES as usize)
            .create_async()
            .await;

        let http_client = reqwest::Client::new();
        let (metric_tx, _metric_rx) = metrics::metric_channel();
        let paused = AtomicBool::new(false);
        let tx = make_test_tx();

        let result = submit_with_retry(
            &http_client,
            &server.url(),
            tx,
            3,
            Duration::from_millis(10),
            &metric_tx,
            &paused,
        )
        .await;

        match result {
            SubmitResult::Failed { error } => {
                assert!(
                    error.contains("RateLimited"),
                    "expected RateLimited in error, got {error}"
                );
            }
            other => {
                panic!("expected SubmitResult::Failed after RateLimited exhaustion, got {other:?}")
            }
        }
    }
}
