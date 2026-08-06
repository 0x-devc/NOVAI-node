//! Transaction generator with rate control.
//!
//! INVARIANTS:
//! - Generates at most target TPS (may be less under backpressure)
//! - Pauses when mempool signals full (adaptive rate control)
//! - Channel never exceeds capacity (blocks on full)
//!
//! FAILURE MODES:
//! - Channel closed - terminates gracefully
//! - Paused indefinitely - waits until unpaused or shutdown

use crate::sender::{SenderAccount, SenderPool};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Transaction type to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxType {
    /// Simple balance transfer (empty payload).
    Transfer,
    /// AI entity registration (encoded AiEntity in payload).
    AiRegister,
    /// AI signal emission (encoded AiSignalV1 in payload).
    AiSignal,
}

impl TxType {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "transfer" => Some(TxType::Transfer),
            "ai_register" => Some(TxType::AiRegister),
            "ai_signal" => Some(TxType::AiSignal),
            _ => None,
        }
    }
}

/// Unsigned transaction template for deferred nonce assignment.
///
/// The generator creates templates without claiming a nonce or signing.
/// The submitter worker claims the nonce just before submission, preventing
/// nonce gaps when transactions are rejected (e.g., MempoolFull).
pub struct TxTemplate {
    /// Sender account (holds signing key, verifying key, address).
    pub sender: Arc<SenderAccount>,
    /// Fee for this transaction.
    pub fee: u64,
    /// Transaction payload.
    pub payload: Vec<u8>,
}

/// Configuration for the generator.
#[derive(Debug, Clone)]
pub struct GeneratorConfig {
    /// Target transactions per second.
    pub target_tps: u64,
    /// Type of transactions to generate.
    pub tx_type: TxType,
    /// Fee to set on each transaction.
    pub fee: u64,
    /// Maximum duration (None = run forever until shutdown).
    pub max_duration: Option<Duration>,
}

impl Default for GeneratorConfig {
    fn default() -> Self {
        Self {
            target_tps: 100,
            tx_type: TxType::Transfer,
            fee: 1,
            max_duration: None,
        }
    }
}

/// Handle to control a running generator.
pub struct GeneratorHandle {
    /// Join handle for the generator task.
    join_handle: tokio::task::JoinHandle<GeneratorStats>,
}

impl GeneratorHandle {
    /// Request graceful shutdown by awaiting the handle.
    ///
    /// The generator will stop after its duration expires or when explicitly dropped.
    pub async fn wait(self) -> GeneratorStats {
        self.join_handle.await.unwrap_or_else(|e| {
            warn!("Generator task panicked: {}", e);
            GeneratorStats::default()
        })
    }

    /// Shutdown the generator immediately by aborting the task.
    #[allow(dead_code)]
    pub fn shutdown(self) {
        self.join_handle.abort();
    }
}

/// Statistics from a generator run.
#[derive(Debug, Clone, Default)]
pub struct GeneratorStats {
    /// Total transactions generated.
    pub generated_count: u64,
    /// Transactions dropped due to channel full (should be 0).
    pub dropped_count: u64,
    /// Total runtime in milliseconds.
    pub runtime_ms: u64,
    /// Actual achieved TPS.
    pub actual_tps: f64,
}

/// Transaction generator that produces unsigned templates at a target rate.
///
/// Templates are finalized (nonce + signature) by the submitter workers
/// just before submission, ensuring nonces stay synchronized with the chain.
pub struct Generator {
    config: GeneratorConfig,
    sender_pool: Arc<SenderPool>,
    paused: Arc<AtomicBool>,
    /// Optional adaptive throttle. When present the generator stretches its
    /// interval while the node is refusing a large share of what it offers.
    throttle: Option<Arc<crate::throttle::Throttle>>,
}

impl Generator {
    /// Create a new generator with the given configuration.
    ///
    /// The `paused` flag is shared with the submitter; when set to `true`
    /// the generator skips tick intervals until the flag clears.
    pub fn new(
        config: GeneratorConfig,
        sender_pool: Arc<SenderPool>,
        paused: Arc<AtomicBool>,
    ) -> Self {
        Self {
            config,
            sender_pool,
            paused,
            throttle: None,
        }
    }

    /// Attach the adaptive throttle shared with the submitter workers.
    #[must_use]
    pub fn with_throttle(mut self, throttle: Arc<crate::throttle::Throttle>) -> Self {
        self.throttle = Some(throttle);
        self
    }

    /// Start generating transaction templates, sending them to the provided channel.
    /// Returns a handle to control/await the generator.
    pub fn start(self, tx_sender: mpsc::Sender<TxTemplate>) -> GeneratorHandle {
        let join_handle = tokio::spawn(async move { self.run(tx_sender).await });

        GeneratorHandle { join_handle }
    }

    /// Main generation loop.
    async fn run(self, tx_sender: mpsc::Sender<TxTemplate>) -> GeneratorStats {
        let start_time = Instant::now();
        let mut generated_count = 0u64;
        let dropped_count = 0u64;

        // Calculate interval between transactions
        if self.config.target_tps == 0 {
            warn!("Target TPS is 0, generator will not produce transactions");
            return GeneratorStats::default();
        }

        let interval_duration = Duration::from_secs_f64(1.0 / self.config.target_tps as f64);
        let mut interval = tokio::time::interval(interval_duration);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        info!(
            "Generator started: {} TPS, type={:?}, fee={}, interval={:?}",
            self.config.target_tps, self.config.tx_type, self.config.fee, interval_duration
        );

        // Determine when to stop
        let deadline = self.config.max_duration.map(|d| start_time + d);

        loop {
            // Check if we've reached the deadline
            if let Some(deadline) = deadline {
                if Instant::now() >= deadline {
                    info!("Generator reached max duration, stopping");
                    break;
                }
            }

            // Adaptive rate control: skip generation when mempool is full
            if self.paused.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }

            // Wait for next tick
            interval.tick().await;

            // Gate SOAK B6: if the node is refusing a large share of what we
            // offer, offering it faster only sustains the pressure. The
            // multiplier is bounded, so this slows the generator but never
            // stops it.
            if let Some(throttle) = &self.throttle {
                let extra = throttle.delay_multiplier().saturating_sub(1);
                if extra > 0 {
                    tokio::time::sleep(interval_duration * extra).await;
                }
            }

            // Generate template
            let template = self.generate_template();

            // Try to send (blocks if channel is full)
            match tx_sender.send(template).await {
                Ok(_) => {
                    generated_count += 1;
                    if generated_count.is_multiple_of(1000) {
                        debug!("Generated {} transactions", generated_count);
                    }
                }
                Err(_) => {
                    // Channel closed, exit
                    info!("Transaction channel closed, stopping generator");
                    break;
                }
            }
        }

        let elapsed = start_time.elapsed();
        let runtime_ms = elapsed.as_millis() as u64;
        let actual_tps = if runtime_ms > 0 {
            (generated_count as f64) / (runtime_ms as f64 / 1000.0)
        } else {
            0.0
        };

        info!(
            "Generator stopped: generated={}, dropped={}, runtime={}ms, actual_tps={:.2}",
            generated_count, dropped_count, runtime_ms, actual_tps
        );

        GeneratorStats {
            generated_count,
            dropped_count,
            runtime_ms,
            actual_tps,
        }
    }

    /// Generate a single unsigned transaction template.
    ///
    /// No nonce is claimed and no signature is produced — that happens
    /// in the submitter worker just before the RPC call.
    fn generate_template(&self) -> TxTemplate {
        // Get next sender in round-robin
        let sender = self.sender_pool.next_sender();

        // Generate payload based on tx type
        let payload = match self.config.tx_type {
            TxType::Transfer => generate_transfer_payload(&sender),
            TxType::AiRegister => generate_ai_register_payload(),
            TxType::AiSignal => generate_ai_signal_payload(),
        };

        TxTemplate {
            sender,
            fee: self.config.fee,
            payload,
        }
    }
}

/// Generate a valid TransferPayloadV1 for load testing.
///
/// Format: [version:1][to:32][amount:8 BE] = 41 bytes.
/// Sends to a deterministic recipient derived from the sender index
/// (shifted by 1000 to avoid overlap with sender accounts).
/// Amount is 5000 (above MIN_ACCOUNT_BALANCE = 1000).
fn generate_transfer_payload(sender: &Arc<SenderAccount>) -> Vec<u8> {
    // Deterministic recipient: offset sender index to avoid self-transfers
    let recipient_index = sender.index + 1000;
    let recipient = SenderAccount::from_index(recipient_index);

    let mut payload = Vec::with_capacity(41);
    payload.push(1); // Transfer payload version
    payload.extend_from_slice(&recipient.address);
    payload.extend_from_slice(&5000u64.to_be_bytes()); // Amount: 5000 (> MIN_ACCOUNT_BALANCE)
    payload
}

/// Generate a synthetic AI entity registration payload.
///
/// TODO: This should encode a real AiEntity once we implement AI transaction types.
/// For now, just return a placeholder payload.
fn generate_ai_register_payload() -> Vec<u8> {
    // Placeholder: 100-byte payload representing encoded AiEntity
    vec![0xAA; 100]
}

/// Generate a synthetic AI signal payload.
///
/// TODO: This should encode a real AiSignalV1 once we implement AI transaction types.
/// For now, just return a placeholder payload.
fn generate_ai_signal_payload() -> Vec<u8> {
    // Placeholder: 50-byte payload representing encoded AiSignalV1
    vec![0xBB; 50]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tx_type_from_str_parses_all_types() {
        assert_eq!(TxType::from_str("transfer"), Some(TxType::Transfer));
        assert_eq!(TxType::from_str("ai_register"), Some(TxType::AiRegister));
        assert_eq!(TxType::from_str("ai_signal"), Some(TxType::AiSignal));
        assert_eq!(TxType::from_str("invalid"), None);
    }

    #[test]
    fn template_has_correct_fee() {
        let pool = Arc::new(SenderPool::new(1));
        let paused = Arc::new(AtomicBool::new(false));
        let config = GeneratorConfig {
            fee: 42,
            ..Default::default()
        };
        let generator = Generator::new(config, pool, paused);

        let template = generator.generate_template();
        assert_eq!(template.fee, 42);
    }

    #[test]
    fn transfer_template_has_valid_payload() {
        let pool = Arc::new(SenderPool::new(1));
        let paused = Arc::new(AtomicBool::new(false));
        let config = GeneratorConfig {
            tx_type: TxType::Transfer,
            ..Default::default()
        };
        let generator = Generator::new(config, pool, paused);

        let template = generator.generate_template();
        // Transfer payload: [version:1][to:32][amount:8] = 41 bytes
        assert_eq!(template.payload.len(), 41);
        assert_eq!(template.payload[0], 1); // version byte
    }

    #[test]
    fn ai_register_template_has_payload() {
        let pool = Arc::new(SenderPool::new(1));
        let paused = Arc::new(AtomicBool::new(false));
        let config = GeneratorConfig {
            tx_type: TxType::AiRegister,
            ..Default::default()
        };
        let generator = Generator::new(config, pool, paused);

        let template = generator.generate_template();
        assert!(!template.payload.is_empty());
    }

    #[test]
    fn template_sender_cycles_round_robin() {
        let pool = Arc::new(SenderPool::new(3));
        let paused = Arc::new(AtomicBool::new(false));
        let config = GeneratorConfig::default();
        let generator = Generator::new(config, Arc::clone(&pool), paused);

        let t0 = generator.generate_template();
        let t1 = generator.generate_template();
        let t2 = generator.generate_template();
        let t3 = generator.generate_template();

        assert_eq!(t0.sender.index, 0);
        assert_eq!(t1.sender.index, 1);
        assert_eq!(t2.sender.index, 2);
        assert_eq!(t3.sender.index, 0); // wraps around
    }

    #[tokio::test]
    async fn generator_respects_max_duration() {
        let pool = Arc::new(SenderPool::new(1));
        let paused = Arc::new(AtomicBool::new(false));
        let config = GeneratorConfig {
            target_tps: 1000,
            max_duration: Some(Duration::from_millis(100)),
            ..Default::default()
        };

        let (tx_sender, mut tx_receiver) = mpsc::channel(100);
        let generator = Generator::new(config, pool, paused);
        let handle = generator.start(tx_sender);

        // Consume templates in background
        tokio::spawn(async move {
            while tx_receiver.recv().await.is_some() {
                // Drain channel
            }
        });

        let stats = handle.wait().await;

        // Should stop after ~100ms
        assert!(stats.runtime_ms >= 100 && stats.runtime_ms < 200);
        assert!(stats.generated_count > 0);
    }

    #[tokio::test]
    async fn generator_stops_on_channel_close() {
        let pool = Arc::new(SenderPool::new(1));
        let paused = Arc::new(AtomicBool::new(false));
        let config = GeneratorConfig {
            target_tps: 100,
            max_duration: None,
            ..Default::default()
        };

        let (tx_sender, tx_receiver) = mpsc::channel(10);
        let generator = Generator::new(config, pool, paused);
        let handle = generator.start(tx_sender);

        // Drop receiver immediately to close channel
        drop(tx_receiver);

        // Generator should stop quickly
        let stats = handle.wait().await;
        assert!(stats.runtime_ms < 1000);
    }

    #[tokio::test]
    async fn generator_pauses_when_flag_set() {
        let pool = Arc::new(SenderPool::new(1));
        let paused = Arc::new(AtomicBool::new(true)); // Start paused
        let config = GeneratorConfig {
            target_tps: 1000,
            max_duration: Some(Duration::from_millis(200)),
            ..Default::default()
        };

        let (tx_sender, mut tx_receiver) = mpsc::channel(100);
        let generator = Generator::new(config, pool, Arc::clone(&paused));
        let handle = generator.start(tx_sender);

        // Consume in background
        tokio::spawn(async move { while tx_receiver.recv().await.is_some() {} });

        // Stay paused for 100ms, then unpause
        tokio::time::sleep(Duration::from_millis(100)).await;
        paused.store(false, Ordering::Relaxed);

        let stats = handle.wait().await;

        // Should have generated far fewer txs than 200ms at 1000 TPS would allow
        // because it was paused for the first 100ms
        assert!(stats.generated_count < 150);
    }
}
