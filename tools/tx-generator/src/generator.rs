//! Transaction generator with rate control.
//!
//! INVARIANTS:
//! - Generates at most target TPS (may be less under backpressure)
//! - All transactions are validly signed
//! - Channel never exceeds capacity (blocks on full)
//!
//! FAILURE MODES:
//! - Signing failure (codec error) - logged and skipped
//! - Channel closed - terminates gracefully

use crate::sender::SenderPool;
use novai_crypto::sign_tx_v1;
use novai_types::{TxV1, TxVersion};
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
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "transfer" => Some(TxType::Transfer),
            "ai_register" => Some(TxType::AiRegister),
            "ai_signal" => Some(TxType::AiSignal),
            _ => None,
        }
    }
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

/// Transaction generator that produces signed transactions at a target rate.
pub struct Generator {
    config: GeneratorConfig,
    sender_pool: Arc<SenderPool>,
}

impl Generator {
    /// Create a new generator with the given configuration.
    pub fn new(config: GeneratorConfig, sender_pool: Arc<SenderPool>) -> Self {
        Self {
            config,
            sender_pool,
        }
    }

    /// Start generating transactions, sending them to the provided channel.
    /// Returns a handle to control/await the generator.
    pub fn start(self, tx_sender: mpsc::Sender<TxV1>) -> GeneratorHandle {
        let join_handle = tokio::spawn(async move { self.run(tx_sender).await });

        GeneratorHandle { join_handle }
    }

    /// Main generation loop.
    async fn run(self, tx_sender: mpsc::Sender<TxV1>) -> GeneratorStats {
        let start_time = Instant::now();
        let mut generated_count = 0u64;
        let mut dropped_count = 0u64;

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

            // Wait for next tick
            interval.tick().await;

            // Generate transaction
            match self.generate_transaction() {
                Ok(tx) => {
                    // Try to send (non-blocking check, but will block if channel has space)
                    match tx_sender.send(tx).await {
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
                Err(e) => {
                    warn!("Failed to generate transaction: {}, skipping", e);
                    dropped_count += 1;
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

    /// Generate a single transaction.
    fn generate_transaction(&self) -> Result<TxV1, String> {
        // Get next sender in round-robin
        let sender = self.sender_pool.next_sender();

        // Get next nonce
        let nonce = sender.claim_nonce();

        // Generate payload based on tx type
        let payload = match self.config.tx_type {
            TxType::Transfer => vec![], // Empty payload for simple transfer
            TxType::AiRegister => generate_ai_register_payload(),
            TxType::AiSignal => generate_ai_signal_payload(),
        };

        // Build unsigned transaction
        let mut tx = TxV1 {
            version: TxVersion::V1,
            from: sender.address,
            pubkey: sender.verifying_key.to_bytes(),
            nonce,
            fee: self.config.fee,
            payload,
            sig: [0u8; 64], // Will be filled by sign_tx_v1
        };

        // Sign transaction
        sign_tx_v1(&sender.signing_key, &mut tx)
            .map_err(|e| format!("Failed to sign transaction: {:?}", e))?;

        Ok(tx)
    }
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
    fn generated_tx_is_validly_signed() {
        let pool = Arc::new(SenderPool::new(5));
        let config = GeneratorConfig::default();
        let generator = Generator::new(config, pool);

        let tx = generator.generate_transaction().unwrap();

        // Verify signature using crypto module
        use novai_crypto::{pubkey_from_bytes, verify_tx_v1};
        let pk = pubkey_from_bytes(&tx.pubkey).unwrap();
        assert!(verify_tx_v1(&pk, &tx).unwrap());
    }

    #[test]
    fn generated_tx_has_correct_nonce() {
        let pool = Arc::new(SenderPool::new(1));
        let config = GeneratorConfig::default();
        let generator = Generator::new(config, Arc::clone(&pool));

        let tx1 = generator.generate_transaction().unwrap();
        let tx2 = generator.generate_transaction().unwrap();
        let tx3 = generator.generate_transaction().unwrap();

        // Nonces should increment
        assert_eq!(tx1.nonce, 0);
        assert_eq!(tx2.nonce, 1);
        assert_eq!(tx3.nonce, 2);
    }

    #[test]
    fn generated_tx_has_correct_fee() {
        let pool = Arc::new(SenderPool::new(1));
        let config = GeneratorConfig {
            fee: 42,
            ..Default::default()
        };
        let generator = Generator::new(config, pool);

        let tx = generator.generate_transaction().unwrap();
        assert_eq!(tx.fee, 42);
    }

    #[test]
    fn transfer_tx_has_empty_payload() {
        let pool = Arc::new(SenderPool::new(1));
        let config = GeneratorConfig {
            tx_type: TxType::Transfer,
            ..Default::default()
        };
        let generator = Generator::new(config, pool);

        let tx = generator.generate_transaction().unwrap();
        assert!(tx.payload.is_empty());
    }

    #[test]
    fn ai_register_tx_has_payload() {
        let pool = Arc::new(SenderPool::new(1));
        let config = GeneratorConfig {
            tx_type: TxType::AiRegister,
            ..Default::default()
        };
        let generator = Generator::new(config, pool);

        let tx = generator.generate_transaction().unwrap();
        assert!(!tx.payload.is_empty());
    }

    #[tokio::test]
    async fn generator_respects_max_duration() {
        let pool = Arc::new(SenderPool::new(1));
        let config = GeneratorConfig {
            target_tps: 1000, // High TPS to ensure we hit duration limit, not generation limit
            max_duration: Some(Duration::from_millis(100)),
            ..Default::default()
        };

        let (tx_sender, mut tx_receiver) = mpsc::channel(100);
        let generator = Generator::new(config, pool);
        let handle = generator.start(tx_sender);

        // Consume transactions in background
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
        let config = GeneratorConfig {
            target_tps: 100,
            max_duration: None, // Run forever unless channel closes
            ..Default::default()
        };

        let (tx_sender, tx_receiver) = mpsc::channel(10);
        let generator = Generator::new(config, pool);
        let handle = generator.start(tx_sender);

        // Drop receiver immediately to close channel
        drop(tx_receiver);

        // Generator should stop quickly
        let stats = handle.wait().await;
        assert!(stats.runtime_ms < 1000); // Should stop within 1 second
    }
}
