//! Chaos testing framework for fault injection and property verification.
//!
//! This framework provides infrastructure for testing consensus under adverse conditions:
//! - Network partitions
//! - Message delays and drops
//! - Node crashes and restarts
//! - Byzantine behavior
//!
//! INVARIANTS:
//! - All message delivery is deterministic (based on RNG seed)
//! - Validators can be controlled independently
//! - Network faults can be injected and healed
//! - Tests are reproducible (same seed = same behavior)

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus::ConsensusState;
use novai_consensus_types::{Block, Vote, QC};
use novai_crypto::address_from_pubkey;
use novai_p2p::NetworkMessage;
use novai_state::MemKv;
use novai_types::Address;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// =============================================================================
// Core Types
// =============================================================================

/// Message with delayed delivery time for latency simulation.
#[derive(Debug, Clone)]
struct DelayedMessage {
    #[allow(dead_code)]
    from: usize,
    #[allow(dead_code)]
    to: usize,
    #[allow(dead_code)]
    message: NetworkMessage,
    #[allow(dead_code)]
    deliver_at: Instant,
}

/// Network simulator with fault injection capabilities.
///
/// Routes messages between validators with configurable faults:
/// - Partitions: Groups of validators that can't communicate
/// - Latency: Artificial delay before message delivery
/// - Drops: Random message loss based on drop rate
///
/// All randomness is deterministic (seeded RNG) for reproducible tests.
pub struct ChaosNetwork {
    /// Message queues for each validator (indexed by validator ID)
    message_queues: Arc<Mutex<HashMap<usize, VecDeque<DelayedMessage>>>>,

    /// Network partition groups (validators in same group can communicate)
    partition_groups: Arc<Mutex<Vec<HashSet<usize>>>>,

    /// Per-validator latency injection (delay for messages TO this validator)
    latency_map: Arc<Mutex<HashMap<usize, Duration>>>,

    /// Per-validator message drop rate (0.0 = no drops, 1.0 = drop all)
    drop_rate_map: Arc<Mutex<HashMap<usize, f64>>>,

    /// Deterministic RNG for reproducible chaos
    #[allow(dead_code)]
    rng: Arc<Mutex<StdRng>>,

    /// Number of validators in the network
    num_validators: usize,
}

impl ChaosNetwork {
    /// Create new chaos network with N validators and deterministic seed.
    pub fn new(num_validators: usize, seed: u64) -> Self {
        let mut message_queues = HashMap::new();
        for i in 0..num_validators {
            message_queues.insert(i, VecDeque::new());
        }

        // Initially all validators in one partition (no network partition)
        let initial_partition: HashSet<usize> = (0..num_validators).collect();

        Self {
            message_queues: Arc::new(Mutex::new(message_queues)),
            partition_groups: Arc::new(Mutex::new(vec![initial_partition])),
            latency_map: Arc::new(Mutex::new(HashMap::new())),
            drop_rate_map: Arc::new(Mutex::new(HashMap::new())),
            rng: Arc::new(Mutex::new(StdRng::seed_from_u64(seed))),
            num_validators,
        }
    }

    /// Send message from one validator to another (with fault injection).
    ///
    /// Message may be:
    /// - Dropped (if from and to are in different partitions)
    /// - Dropped (if random drop rate check fails)
    /// - Delayed (based on latency map)
    #[allow(dead_code)]
    pub fn send_message(
        &self,
        from: usize,
        to: usize,
        message: NetworkMessage,
    ) -> Result<(), String> {
        // Validate validator IDs
        if from >= self.num_validators || to >= self.num_validators {
            return Err(format!("Invalid validator ID: from={}, to={}", from, to));
        }

        // Check partition: can from and to communicate?
        if !self.can_communicate(from, to)? {
            // Dropped due to network partition
            return Ok(());
        }

        // Check drop rate
        if self.should_drop_message(to)? {
            // Randomly dropped
            return Ok(());
        }

        // Calculate delivery time based on latency
        let delay = self.get_latency(to)?;
        let deliver_at = Instant::now() + delay;

        // Enqueue message
        let delayed_msg = DelayedMessage {
            from,
            to,
            message,
            deliver_at,
        };

        self.message_queues
            .lock()
            .unwrap()
            .get_mut(&to)
            .ok_or_else(|| format!("Validator {} not found", to))?
            .push_back(delayed_msg);

        Ok(())
    }

    /// Broadcast message from one validator to all others.
    #[allow(dead_code)]
    pub fn broadcast_message(&self, from: usize, message: NetworkMessage) -> Result<(), String> {
        for to in 0..self.num_validators {
            if to != from {
                self.send_message(from, to, message.clone())?;
            }
        }
        Ok(())
    }

    /// Deliver all messages that are ready (past their deliver_at time).
    ///
    /// Returns: Vec of (validator_id, message) ready for delivery.
    #[allow(dead_code)]
    pub fn deliver_pending_messages(&self) -> Vec<(usize, NetworkMessage)> {
        let now = Instant::now();
        let mut delivered = Vec::new();

        let mut queues = self.message_queues.lock().unwrap();
        for (validator_id, queue) in queues.iter_mut() {
            // Deliver all messages whose time has come
            while let Some(msg) = queue.front() {
                if msg.deliver_at <= now {
                    let msg = queue.pop_front().unwrap();
                    delivered.push((*validator_id, msg.message));
                } else {
                    break; // Messages are ordered by deliver_at
                }
            }
        }

        delivered
    }

    /// Check if there are any pending messages in any queue.
    pub fn has_pending_messages(&self) -> bool {
        let queues = self.message_queues.lock().unwrap();
        queues.values().any(|q| !q.is_empty())
    }

    /// Get count of pending messages across all queues.
    pub fn pending_message_count(&self) -> usize {
        let queues = self.message_queues.lock().unwrap();
        queues.values().map(|q| q.len()).sum()
    }

    // -------------------------------------------------------------------------
    // Helper Methods
    // -------------------------------------------------------------------------

    /// Check if two validators can communicate (same partition group).
    pub fn can_communicate(&self, from: usize, to: usize) -> Result<bool, String> {
        let groups = self.partition_groups.lock().unwrap();
        for group in groups.iter() {
            if group.contains(&from) && group.contains(&to) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Determine if message should be dropped based on drop rate.
    #[allow(dead_code)]
    fn should_drop_message(&self, validator: usize) -> Result<bool, String> {
        let drop_rates = self.drop_rate_map.lock().unwrap();
        let drop_rate = drop_rates.get(&validator).copied().unwrap_or(0.0);

        let mut rng = self.rng.lock().unwrap();
        let random: f64 = rng.gen();
        Ok(random < drop_rate)
    }

    /// Get latency for a specific validator.
    pub fn get_latency(&self, validator: usize) -> Result<Duration, String> {
        let latencies = self.latency_map.lock().unwrap();
        Ok(latencies
            .get(&validator)
            .copied()
            .unwrap_or(Duration::from_millis(0)))
    }

    /// Get drop rate for a specific validator.
    #[allow(dead_code)]
    pub fn get_drop_rate(&self, validator: usize) -> Result<f64, String> {
        let drop_rates = self.drop_rate_map.lock().unwrap();
        Ok(drop_rates.get(&validator).copied().unwrap_or(0.0))
    }
}

// =============================================================================
// Validator Handle
// =============================================================================

/// Handle to control a single validator in chaos tests.
///
/// Provides:
/// - Lifecycle control (crash, restart)
/// - State inspection (height, round)
/// - Message injection
pub struct ValidatorHandle {
    pub id: usize,
    pub address: Address,
    #[allow(dead_code)]
    pub signing_key: SigningKey,
    pub verifying_key: VerifyingKey,
    pub state: Arc<Mutex<ConsensusState>>,
    pub db: Arc<Mutex<MemKv>>,
    #[allow(dead_code)]
    pub mempool: Arc<Mutex<mempool::TxMempool>>,
    pub is_crashed: Arc<Mutex<bool>>,
}

impl ValidatorHandle {
    /// Create new validator handle.
    pub fn new(
        id: usize,
        address: Address,
        signing_key: SigningKey,
        verifying_key: VerifyingKey,
    ) -> Self {
        Self {
            id,
            address,
            signing_key,
            verifying_key,
            state: Arc::new(Mutex::new(ConsensusState::new(address))),
            db: Arc::new(Mutex::new(MemKv::new())),
            mempool: Arc::new(Mutex::new(mempool::TxMempool::new(1, 100))),
            is_crashed: Arc::new(Mutex::new(false)),
        }
    }

    /// Simulate validator crash (stops processing messages).
    pub fn crash(&self) -> Result<(), String> {
        *self.is_crashed.lock().unwrap() = true;
        println!("💥 Validator {} crashed", self.id);
        Ok(())
    }

    /// Restart validator from persistent state.
    pub fn restart(&self) -> Result<(), String> {
        // Recover consensus state from database
        let db = self.db.lock().unwrap();
        let recovered_state =
            ConsensusState::recover(self.address, &*db).map_err(|e| format!("{:?}", e))?;

        *self.state.lock().unwrap() = recovered_state;
        *self.is_crashed.lock().unwrap() = false;

        println!("🔄 Validator {} restarted", self.id);
        Ok(())
    }

    /// Check if validator is currently crashed.
    pub fn is_crashed(&self) -> bool {
        *self.is_crashed.lock().unwrap()
    }

    /// Get current committed height.
    pub fn committed_height(&self) -> u64 {
        self.state.lock().unwrap().committed_height
    }

    /// Get current round.
    #[allow(dead_code)]
    pub fn current_round(&self) -> u64 {
        self.state.lock().unwrap().round
    }

    /// Get current height.
    #[allow(dead_code)]
    pub fn current_height(&self) -> u64 {
        self.state.lock().unwrap().height
    }

    /// Cache a block in the validator's state.
    #[allow(dead_code)]
    pub fn cache_block(&self, block: Block) {
        self.state.lock().unwrap().cache_block(block);
    }

    /// Process a vote (if not crashed).
    #[allow(dead_code)]
    pub fn process_vote(
        &self,
        vote: Vote,
        validator_pubkeys: &[(Address, VerifyingKey)],
    ) -> Result<(), String> {
        if self.is_crashed() {
            return Ok(()); // Crashed validators ignore messages
        }

        self.state
            .lock()
            .unwrap()
            .add_vote(vote, validator_pubkeys)
            .map_err(|e| format!("{:?}", e))
    }

    /// Try to form QC for a block hash.
    #[allow(dead_code)]
    pub fn try_form_qc(
        &self,
        block_hash: &[u8; 32],
        validator_set: &[Address],
    ) -> Result<Option<QC>, String> {
        if self.is_crashed() {
            return Ok(None); // Crashed validators can't form QCs
        }

        self.state
            .lock()
            .unwrap()
            .try_form_qc(block_hash, validator_set)
            .map_err(|e| format!("{:?}", e))
    }
}

// =============================================================================
// Chaos Controller
// =============================================================================

/// High-level controller for fault injection scenarios.
///
/// Provides convenient API for common chaos patterns:
/// - Network partitions
/// - Latency injection
/// - Message drops
/// - Validator crashes
pub struct ChaosController {
    pub network: Arc<ChaosNetwork>,
    pub validators: Vec<ValidatorHandle>,
}

impl ChaosController {
    /// Create new chaos controller.
    pub fn new(network: Arc<ChaosNetwork>, validators: Vec<ValidatorHandle>) -> Self {
        Self {
            network,
            validators,
        }
    }

    /// Partition network into groups (validators in same group can communicate).
    ///
    /// Example: `inject_partition(vec![vec![0, 1], vec![2, 3, 4]])`
    /// - Validators 0,1 can talk to each other
    /// - Validators 2,3,4 can talk to each other
    /// - But groups cannot talk across partition boundary
    pub fn inject_partition(&self, groups: Vec<Vec<usize>>) -> Result<(), String> {
        // Validate: all validators mentioned exactly once
        let mut seen = HashSet::new();
        for group in &groups {
            for &v in group {
                if v >= self.validators.len() {
                    return Err(format!("Invalid validator ID: {}", v));
                }
                if !seen.insert(v) {
                    return Err(format!("Validator {} appears in multiple groups", v));
                }
            }
        }

        if seen.len() != self.validators.len() {
            return Err(format!(
                "Not all validators included in partition (expected {}, got {})",
                self.validators.len(),
                seen.len()
            ));
        }

        let partition_groups: Vec<HashSet<usize>> = groups
            .into_iter()
            .map(|g| g.into_iter().collect())
            .collect();

        *self.network.partition_groups.lock().unwrap() = partition_groups;

        println!(
            "🌐 Injected network partition: {:?}",
            self.network.partition_groups.lock().unwrap()
        );
        Ok(())
    }

    /// Inject latency for a specific validator (all messages TO this validator delayed).
    pub fn inject_latency(&self, validator: usize, delay: Duration) -> Result<(), String> {
        if validator >= self.validators.len() {
            return Err(format!("Invalid validator ID: {}", validator));
        }

        self.network
            .latency_map
            .lock()
            .unwrap()
            .insert(validator, delay);

        println!(
            "⏱️  Injected {}ms latency for validator {}",
            delay.as_millis(),
            validator
        );
        Ok(())
    }

    /// Inject asymmetric latency (different delays for different validators).
    #[allow(dead_code)]
    pub fn inject_asymmetric_latency(
        &self,
        latency_map: HashMap<usize, Duration>,
    ) -> Result<(), String> {
        for (validator, delay) in latency_map {
            self.inject_latency(validator, delay)?;
        }
        Ok(())
    }

    /// Inject message drop rate for a specific validator (0.0 to 1.0).
    pub fn inject_message_drop(&self, validator: usize, drop_rate: f64) -> Result<(), String> {
        if validator >= self.validators.len() {
            return Err(format!("Invalid validator ID: {}", validator));
        }

        if !(0.0..=1.0).contains(&drop_rate) {
            return Err(format!("Invalid drop rate: {}", drop_rate));
        }

        self.network
            .drop_rate_map
            .lock()
            .unwrap()
            .insert(validator, drop_rate);

        println!(
            "📉 Injected {:.0}% message drop for validator {}",
            drop_rate * 100.0,
            validator
        );
        Ok(())
    }

    /// Crash a validator (stops processing messages, simulates sudden failure).
    pub fn crash_validator(&self, validator: usize) -> Result<(), String> {
        if validator >= self.validators.len() {
            return Err(format!("Validator {} out of range", validator));
        }

        self.validators[validator].crash()?;
        Ok(())
    }

    /// Restart a crashed validator (loads from persistent state).
    pub fn restart_validator(&self, validator: usize) -> Result<(), String> {
        if validator >= self.validators.len() {
            return Err(format!("Validator {} out of range", validator));
        }

        self.validators[validator].restart()?;
        Ok(())
    }

    /// Heal all network faults (remove partitions, latency, drops).
    pub fn heal_network(&self) -> Result<(), String> {
        // Remove partitions (all validators in one group)
        let num_validators = self.validators.len();
        *self.network.partition_groups.lock().unwrap() = vec![(0..num_validators).collect()];

        // Clear latency
        self.network.latency_map.lock().unwrap().clear();

        // Clear drop rates
        self.network.drop_rate_map.lock().unwrap().clear();

        println!("✅ Network healed (all faults removed)");
        Ok(())
    }

    /// Get list of all validator addresses.
    #[allow(dead_code)]
    pub fn validator_addresses(&self) -> Vec<Address> {
        self.validators.iter().map(|v| v.address).collect()
    }

    /// Get list of all validator public keys.
    #[allow(dead_code)]
    pub fn validator_pubkeys(&self) -> Vec<(Address, VerifyingKey)> {
        self.validators
            .iter()
            .map(|v| (v.address, v.verifying_key))
            .collect()
    }

    /// Check if all non-crashed validators have reached a minimum height.
    #[allow(dead_code)]
    pub fn all_reached_height(&self, min_height: u64) -> bool {
        self.validators
            .iter()
            .filter(|v| !v.is_crashed())
            .all(|v| v.committed_height() >= min_height)
    }

    /// Wait for all non-crashed validators to reach a minimum committed height.
    #[allow(dead_code)]
    pub fn wait_for_height(&self, min_height: u64, timeout: Duration) -> Result<(), String> {
        let start = Instant::now();
        while !self.all_reached_height(min_height) {
            if start.elapsed() > timeout {
                return Err(format!(
                    "Timeout waiting for height {} (current: {:?})",
                    min_height,
                    self.validators
                        .iter()
                        .map(|v| v.committed_height())
                        .collect::<Vec<_>>()
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }
}

// =============================================================================
// Test Setup Helpers
// =============================================================================

/// Create deterministic test validators with fixed seeds.
pub fn make_test_validators(count: usize) -> Vec<(Address, SigningKey, VerifyingKey)> {
    (0..count)
        .map(|i| {
            let seed = [i as u8; 32];
            let sk = SigningKey::from_bytes(&seed);
            let pk = sk.verifying_key();
            let addr = address_from_pubkey(&pk);
            (addr, sk, pk)
        })
        .collect()
}

/// Setup a chaos testnet with N validators and deterministic seed.
///
/// Returns (ChaosController, validator_addresses, validator_pubkeys).
pub fn setup_chaos_testnet(
    num_validators: usize,
    seed: u64,
) -> (ChaosController, Vec<Address>, Vec<(Address, VerifyingKey)>) {
    let validators_data = make_test_validators(num_validators);

    let mut validator_handles = Vec::new();
    for (i, (addr, sk, pk)) in validators_data.iter().enumerate() {
        validator_handles.push(ValidatorHandle::new(i, *addr, sk.clone(), *pk));
    }

    let validator_addresses: Vec<Address> = validators_data.iter().map(|(a, _, _)| *a).collect();
    let validator_pubkeys: Vec<(Address, VerifyingKey)> =
        validators_data.iter().map(|(a, _, pk)| (*a, *pk)).collect();

    let network = Arc::new(ChaosNetwork::new(num_validators, seed));
    let controller = ChaosController::new(network, validator_handles);

    (controller, validator_addresses, validator_pubkeys)
}

// =============================================================================
// Basic Sanity Test
// =============================================================================

#[test]
fn test_chaos_network_creation() {
    let network = ChaosNetwork::new(5, 12345);
    assert_eq!(network.num_validators, 5);
    assert_eq!(network.pending_message_count(), 0);
    assert!(!network.has_pending_messages());
}

#[test]
fn test_validator_handle_creation() {
    let validators = make_test_validators(3);
    let (addr, sk, pk) = &validators[0];

    let handle = ValidatorHandle::new(0, *addr, sk.clone(), *pk);
    assert_eq!(handle.id, 0);
    assert_eq!(handle.address, *addr);
    assert!(!handle.is_crashed());
    assert_eq!(handle.committed_height(), 0);
}

#[test]
fn test_chaos_controller_setup() {
    let (controller, addrs, pubkeys) = setup_chaos_testnet(4, 99999);
    assert_eq!(controller.validators.len(), 4);
    assert_eq!(addrs.len(), 4);
    assert_eq!(pubkeys.len(), 4);
}

#[test]
fn test_partition_injection() {
    let (controller, _, _) = setup_chaos_testnet(5, 11111);

    // Partition: 2 vs 3
    controller
        .inject_partition(vec![vec![0, 1], vec![2, 3, 4]])
        .unwrap();

    // Validators 0 and 1 can communicate
    assert!(controller.network.can_communicate(0, 1).unwrap());

    // Validators 2 and 3 can communicate
    assert!(controller.network.can_communicate(2, 3).unwrap());

    // But 0 and 2 cannot (different partitions)
    assert!(!controller.network.can_communicate(0, 2).unwrap());
}

#[test]
fn test_latency_injection() {
    let (controller, _, _) = setup_chaos_testnet(3, 22222);

    controller
        .inject_latency(0, Duration::from_millis(500))
        .unwrap();

    let latency = controller.network.get_latency(0).unwrap();
    assert_eq!(latency, Duration::from_millis(500));
}

#[test]
fn test_validator_crash_and_restart() {
    let (controller, _, _) = setup_chaos_testnet(3, 33333);

    // Crash validator 0
    controller.crash_validator(0).unwrap();
    assert!(controller.validators[0].is_crashed());

    // Restart validator 0
    controller.restart_validator(0).unwrap();
    assert!(!controller.validators[0].is_crashed());
}

#[test]
fn test_heal_network() {
    let (controller, _, _) = setup_chaos_testnet(4, 44444);

    // Inject faults
    controller
        .inject_partition(vec![vec![0, 1], vec![2, 3]])
        .unwrap();
    controller
        .inject_latency(0, Duration::from_millis(1000))
        .unwrap();
    controller.inject_message_drop(2, 0.5).unwrap();

    // Heal
    controller.heal_network().unwrap();

    // Verify all faults removed
    assert!(controller.network.can_communicate(0, 2).unwrap());
    assert_eq!(
        controller.network.get_latency(0).unwrap(),
        Duration::from_millis(0)
    );
    assert_eq!(
        controller
            .network
            .drop_rate_map
            .lock()
            .unwrap()
            .get(&2)
            .copied()
            .unwrap_or(0.0),
        0.0
    );
}
