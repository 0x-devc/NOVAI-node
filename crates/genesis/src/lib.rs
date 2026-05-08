//! Genesis configuration and state generation.
//!
//! This module provides types for parsing and validating genesis.json files,
//! which define the initial state of the blockchain.

use chrono::DateTime;
use ed25519_dalek::VerifyingKey;
use novai_ai_entities::gates::{ApprovalGate, GateType};
use novai_ai_entities::{AiEntity, AutonomyMode, Capabilities};
use novai_codec::{encode_ai_entity_v3, encode_approval_gate_v1};
use novai_consensus_types::Block;
use novai_smt::hash::{empty_hash_at_height, Hash32};
use novai_smt::node::Node;
use novai_smt::smt::{Smt, SmtError, SmtStore};
use novai_state::{
    account_key, ai_entity_key, approval_gate_key, decode_smt_root_v1, encode_account_v1,
    encode_smt_root_v1, smt_key_for_state_key, smt_node_key, AccountStateV1, KvBatch, WriteOp,
    KEY_SMT_ROOT,
};
use novai_types::Address;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Errors that can occur during genesis processing.
#[derive(Debug)]
pub enum GenesisError {
    /// I/O error reading genesis file.
    IoError(std::io::Error),
    /// JSON parsing error.
    ParseError(serde_json::Error),
    /// Genesis validation failed.
    ValidationError(String),
    /// JSON serialization error.
    SerializeError(serde_json::Error),
}

impl From<std::io::Error> for GenesisError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e)
    }
}

impl From<serde_json::Error> for GenesisError {
    fn from(e: serde_json::Error) -> Self {
        Self::ParseError(e)
    }
}

impl std::fmt::Display for GenesisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(e) => write!(f, "I/O error: {e}"),
            Self::ParseError(e) => write!(f, "Parse error: {e}"),
            Self::ValidationError(msg) => write!(f, "Validation error: {msg}"),
            Self::SerializeError(e) => write!(f, "Serialize error: {e}"),
        }
    }
}

impl std::error::Error for GenesisError {}

/// Genesis validator configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisValidator {
    /// Validator public key (64 hex characters = 32 bytes).
    pub pubkey: String,
    /// Initial stake amount (parseable as u64).
    pub initial_stake: String,
    /// Optional validator name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Genesis AI entity configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisAIEntity {
    /// Human-readable name for this entity.
    pub name: String,
    /// Code hash (64 hex characters = 32 bytes).
    pub code_hash: String,
    /// Creator address (64 hex characters = 32 bytes).
    pub creator: String,
    /// Autonomy mode: "advisory", "gated", or "autonomous".
    #[serde(default = "default_autonomy_mode")]
    pub autonomy_mode: String,
    /// Initial balance for this entity.
    #[serde(default)]
    pub initial_balance: String,
    /// Capabilities flags (optional, defaults to match autonomy mode).
    #[serde(default)]
    pub capabilities: Option<GenesisCapabilities>,
}

fn default_autonomy_mode() -> String {
    "advisory".to_string()
}

/// Genesis capabilities configuration.
// Bools are the correct representation for individual capability flags in this config struct
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GenesisCapabilities {
    #[serde(default)]
    pub read_public_chain: bool,
    #[serde(default)]
    pub read_memory_objects: bool,
    #[serde(default)]
    pub emit_proposals: bool,
    #[serde(default)]
    pub request_execution: bool,
    #[serde(default)]
    pub read_nnpx_derived: bool,
    #[serde(default)]
    pub submit_reputation_updates: bool,
}

/// Genesis approval gate configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisApprovalGate {
    /// Gate type: "multisig", "threshold", or "`timelock_only`".
    pub gate_type: String,
    /// Required approvers (list of 64-char hex addresses).
    #[serde(default)]
    pub required_approvers: Vec<String>,
    /// Approval threshold (required signatures).
    pub threshold: u32,
    /// Timelock in blocks before execution.
    pub timelock_blocks: u64,
    /// Expiry in blocks after proposal.
    pub expiry_blocks: u64,
    /// Enable veto capability.
    #[serde(default)]
    pub veto_enabled: bool,
    /// Enable freeze capability.
    #[serde(default)]
    pub freeze_enabled: bool,
}

/// Genesis configuration for blockchain initialization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisConfig {
    /// Chain identifier (must be non-empty).
    pub chain_id: String,
    /// Protocol version (must be >= 1).
    pub protocol_version: u64,
    /// Genesis timestamp (RFC3339 format).
    pub timestamp: String,
    /// Initial validator set (1-100 validators).
    pub validators: Vec<GenesisValidator>,
    /// Initial account balances (address hex string -> amount string).
    #[serde(default)]
    pub accounts: BTreeMap<String, String>,
    /// Initial AI entities.
    #[serde(default)]
    pub ai_entities: Vec<GenesisAIEntity>,
    /// Initial approval gates.
    #[serde(default)]
    pub approval_gates: Vec<GenesisApprovalGate>,
}

impl GenesisConfig {
    /// Parse genesis config from JSON string.
    ///
    /// # Errors
    /// Returns error if JSON is invalid or validation fails.
    pub fn from_json(json: &str) -> Result<Self, GenesisError> {
        let config: Self = serde_json::from_str(json)?;
        config.validate()?;
        Ok(config)
    }

    /// Load genesis config from file.
    ///
    /// # Errors
    /// Returns error if file cannot be read, JSON is invalid, or validation fails.
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, GenesisError> {
        let json = std::fs::read_to_string(path)?;
        Self::from_json(&json)
    }

    /// Serialize to canonical JSON (deterministic ordering).
    ///
    /// # Errors
    /// Returns error if serialization fails.
    pub fn to_canonical_json(&self) -> Result<String, GenesisError> {
        serde_json::to_string_pretty(self).map_err(GenesisError::SerializeError)
    }

    /// Validate genesis configuration.
    ///
    /// # Errors
    /// Returns `ValidationError` if any constraint is violated.
    fn validate(&self) -> Result<(), GenesisError> {
        // chain_id must be non-empty
        if self.chain_id.is_empty() {
            return Err(GenesisError::ValidationError(
                "chain_id must be non-empty".to_string(),
            ));
        }

        // protocol_version must be >= 1
        if self.protocol_version < 1 {
            return Err(GenesisError::ValidationError(
                "protocol_version must be >= 1".to_string(),
            ));
        }

        // timestamp must be valid RFC3339
        DateTime::parse_from_rfc3339(&self.timestamp)
            .map_err(|e| GenesisError::ValidationError(format!("Invalid timestamp: {e}")))?;

        // validators: 1-100
        if self.validators.is_empty() {
            return Err(GenesisError::ValidationError(
                "Must have at least 1 validator".to_string(),
            ));
        }
        if self.validators.len() > 100 {
            return Err(GenesisError::ValidationError(
                "Cannot have more than 100 validators".to_string(),
            ));
        }

        // Validate each validator
        for (idx, validator) in self.validators.iter().enumerate() {
            // pubkey must be 64 hex chars (32 bytes)
            if validator.pubkey.len() != 64 {
                return Err(GenesisError::ValidationError(format!(
                    "Validator {idx} pubkey must be 64 hex characters"
                )));
            }
            hex::decode(&validator.pubkey).map_err(|e| {
                GenesisError::ValidationError(format!("Validator {idx} pubkey invalid hex: {e}"))
            })?;

            // initial_stake must parse as u64
            validator.initial_stake.parse::<u64>().map_err(|e| {
                GenesisError::ValidationError(format!(
                    "Validator {idx} initial_stake must be valid u64: {e}"
                ))
            })?;
        }

        // Validate account amounts are parseable as u64
        for (addr, amount) in &self.accounts {
            amount.parse::<u64>().map_err(|e| {
                GenesisError::ValidationError(format!(
                    "Account {addr} amount must be valid u64: {e}"
                ))
            })?;
        }

        // Validate AI entities
        for (idx, entity) in self.ai_entities.iter().enumerate() {
            // code_hash must be 64 hex chars
            if entity.code_hash.len() != 64 {
                return Err(GenesisError::ValidationError(format!(
                    "AI entity {idx} code_hash must be 64 hex characters"
                )));
            }
            hex::decode(&entity.code_hash).map_err(|e| {
                GenesisError::ValidationError(format!("AI entity {idx} code_hash invalid hex: {e}"))
            })?;

            // creator must be 64 hex chars
            if entity.creator.len() != 64 {
                return Err(GenesisError::ValidationError(format!(
                    "AI entity {idx} creator must be 64 hex characters"
                )));
            }
            hex::decode(&entity.creator).map_err(|e| {
                GenesisError::ValidationError(format!("AI entity {idx} creator invalid hex: {e}"))
            })?;

            // autonomy_mode must be valid
            match entity.autonomy_mode.as_str() {
                "advisory" | "gated" | "autonomous" => {}
                other => {
                    return Err(GenesisError::ValidationError(format!(
                        "AI entity {idx} invalid autonomy_mode: {other}"
                    )));
                }
            }

            // initial_balance must be parseable if provided
            if !entity.initial_balance.is_empty() {
                entity.initial_balance.parse::<u64>().map_err(|e| {
                    GenesisError::ValidationError(format!(
                        "AI entity {idx} initial_balance must be valid u64: {e}"
                    ))
                })?;
            }
        }

        // Validate approval gates
        for (idx, gate) in self.approval_gates.iter().enumerate() {
            // gate_type must be valid
            match gate.gate_type.as_str() {
                "multisig" | "threshold" | "timelock_only" => {}
                other => {
                    return Err(GenesisError::ValidationError(format!(
                        "Approval gate {idx} invalid gate_type: {other}"
                    )));
                }
            }

            // required_approvers must be valid addresses
            for (j, approver) in gate.required_approvers.iter().enumerate() {
                if approver.len() != 64 {
                    return Err(GenesisError::ValidationError(format!(
                        "Approval gate {idx} approver {j} must be 64 hex characters"
                    )));
                }
                hex::decode(approver).map_err(|e| {
                    GenesisError::ValidationError(format!(
                        "Approval gate {idx} approver {j} invalid hex: {e}"
                    ))
                })?;
            }
        }

        Ok(())
    }
}

/// Generated genesis state.
pub struct GenesisState {
    /// Deterministic state root computed from genesis config.
    pub state_root: [u8; 32],
    /// Genesis block (height 0, empty transactions).
    pub genesis_block: Block,
    /// Sorted validator addresses.
    pub validator_set: Vec<Address>,
    /// M-04: Hash of canonical genesis configuration. Validators can compare
    /// this value to verify they initialized from the same genesis file.
    pub genesis_hash: [u8; 32],
}

/// Genesis state generator.
pub struct GenesisGenerator {
    config: GenesisConfig,
}

impl GenesisGenerator {
    /// Create generator from validated config.
    #[must_use]
    pub const fn new(config: GenesisConfig) -> Self {
        Self { config }
    }

    /// Generate deterministic initial state from genesis configuration.
    ///
    /// Same `GenesisConfig` always produces same state root.
    ///
    /// # Errors
    /// Returns error if state initialization fails.
    pub fn generate<K>(&self, db: &mut K) -> Result<GenesisState, GenesisError>
    where
        K: KvBatch,
        K::Error: std::fmt::Debug,
    {
        // 1. Compute validator set (deterministically sorted)
        let validator_set = self.compute_validator_set()?;

        // 2. Build state write operations for account balances
        let mut state_ops = Vec::new();

        // Accounts are in BTreeMap (deterministic order)
        for (addr_hex, balance_str) in &self.config.accounts {
            let addr = Self::parse_address(addr_hex)?;
            let balance = balance_str
                .parse::<u64>()
                .map_err(|e| GenesisError::ValidationError(format!("Invalid balance: {e}")))?;

            let account = AccountStateV1 {
                balance: u128::from(balance),
                nonce: 0,
            };

            state_ops.push(WriteOp::Put(
                account_key(&addr),
                encode_account_v1(&account).to_vec(),
            ));
        }

        // 2b. Write AI entities to state
        for genesis_entity in &self.config.ai_entities {
            let code_hash = Self::parse_hash(&genesis_entity.code_hash)?;
            let creator = Self::parse_address(&genesis_entity.creator)?;

            let autonomy_mode = match genesis_entity.autonomy_mode.as_str() {
                "gated" => AutonomyMode::Gated,
                "autonomous" => AutonomyMode::Autonomous,
                // Default to Advisory for "advisory" and any unrecognized mode
                _ => AutonomyMode::Advisory,
            };

            let capabilities = genesis_entity.capabilities.as_ref().map_or_else(
                || match autonomy_mode {
                    AutonomyMode::Advisory => Capabilities::advisory(),
                    AutonomyMode::Gated | AutonomyMode::Autonomous => Capabilities::gated(),
                },
                |caps| Capabilities {
                    read_public_chain: caps.read_public_chain,
                    read_memory_objects: caps.read_memory_objects,
                    emit_proposals: caps.emit_proposals,
                    request_execution: caps.request_execution,
                    read_nnpx_derived: caps.read_nnpx_derived,
                    submit_reputation_updates: caps.submit_reputation_updates,
                    _reserved: [false; 2],
                },
            );

            // W6-05: Block read_nnpx_derived at genesis registration
            if capabilities.read_nnpx_derived {
                return Err(GenesisError::ValidationError(
                    "AI entity cannot be registered with read_nnpx_derived capability".to_string(),
                ));
            }

            let mut entity = AiEntity::new(code_hash, creator, autonomy_mode, capabilities, 0);

            if !genesis_entity.initial_balance.is_empty() {
                let balance = genesis_entity
                    .initial_balance
                    .parse::<u64>()
                    .map_err(|e| GenesisError::ValidationError(format!("Invalid balance: {e}")))?;
                entity.economic_balance = u128::from(balance);
            }

            let encoded = encode_ai_entity_v3(&entity);
            state_ops.push(WriteOp::Put(ai_entity_key(&entity.id), encoded));
        }

        // 2c. Write approval gates to state
        for genesis_gate in &self.config.approval_gates {
            let gate_type = match genesis_gate.gate_type.as_str() {
                "threshold" => GateType::Threshold,
                "timelock_only" => GateType::TimelockOnly,
                // Default to Multisig for "multisig" and any unrecognized type
                _ => GateType::Multisig,
            };

            let mut approvers = Vec::new();
            for approver_hex in &genesis_gate.required_approvers {
                approvers.push(Self::parse_address(approver_hex)?);
            }

            let gate = ApprovalGate::new(
                gate_type,
                approvers,
                genesis_gate.threshold,
                genesis_gate.timelock_blocks,
                genesis_gate.expiry_blocks,
                genesis_gate.veto_enabled,
                genesis_gate.freeze_enabled,
            )
            .map_err(|e| GenesisError::ValidationError(format!("Invalid gate: {e}")))?;

            let encoded = encode_approval_gate_v1(&gate);
            state_ops.push(WriteOp::Put(approval_gate_key(&gate.gate_id), encoded));
        }

        // 3. Compute SMT root from state operations
        let mut all_ops = state_ops.clone();
        let state_root = append_smt_ops_for_genesis(db, &state_ops, &mut all_ops)?;

        // 4. Apply all operations atomically (state + SMT nodes + root)
        db.apply_batch(&all_ops)
            .map_err(|e| GenesisError::ValidationError(format!("DB write failed: {e:?}")))?;

        // 5. Create genesis block
        let genesis_block = Block {
            height: 0,
            round: 0,
            parent_hash: [0u8; 32],
            state_root,
            txs: vec![],
        };

        // M-04: Compute genesis hash from canonical config for chain identity.
        // Uses state_root (which is already a deterministic blake3 hash of all
        // genesis state) as the genesis chain identity. Validators compare this
        // at startup to verify they initialized from the same genesis file.
        let genesis_hash = state_root;

        Ok(GenesisState {
            state_root,
            genesis_block,
            validator_set,
            genesis_hash,
        })
    }

    /// Compute deterministically sorted validator set.
    fn compute_validator_set(&self) -> Result<Vec<Address>, GenesisError> {
        let mut validators = Vec::new();

        for validator in &self.config.validators {
            let pubkey_bytes = hex::decode(&validator.pubkey)
                .map_err(|e| GenesisError::ValidationError(format!("Invalid pubkey: {e}")))?;

            if pubkey_bytes.len() != 32 {
                return Err(GenesisError::ValidationError(
                    "Validator pubkey must be 32 bytes".to_string(),
                ));
            }

            let mut pubkey_array = [0u8; 32];
            pubkey_array.copy_from_slice(&pubkey_bytes);

            let verifying_key = VerifyingKey::from_bytes(&pubkey_array).map_err(|e| {
                GenesisError::ValidationError(format!("Invalid ed25519 pubkey: {e}"))
            })?;

            let addr = novai_crypto::address_from_pubkey(&verifying_key);
            validators.push(addr);
        }

        // Sort deterministically
        validators.sort_unstable();

        Ok(validators)
    }

    /// Parse hex address string.
    fn parse_address(addr_hex: &str) -> Result<Address, GenesisError> {
        if addr_hex.len() != 64 {
            return Err(GenesisError::ValidationError(
                "Address must be 64 hex characters".to_string(),
            ));
        }

        let bytes = hex::decode(addr_hex)
            .map_err(|e| GenesisError::ValidationError(format!("Invalid address hex: {e}")))?;

        let mut addr = [0u8; 32];
        addr.copy_from_slice(&bytes);
        Ok(addr)
    }

    /// Parse hex hash string to 32-byte array.
    fn parse_hash(hash_hex: &str) -> Result<[u8; 32], GenesisError> {
        if hash_hex.len() != 64 {
            return Err(GenesisError::ValidationError(
                "Hash must be 64 hex characters".to_string(),
            ));
        }

        let bytes = hex::decode(hash_hex)
            .map_err(|e| GenesisError::ValidationError(format!("Invalid hash hex: {e}")))?;

        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes);
        Ok(hash)
    }
}

/// Store adapter for SMT operations during genesis.
/// Buffers writes as `WriteOp::Put`, uses Vec for deterministic ordering.
struct SmtOverlayStore<'a, K: KvBatch> {
    db: &'a K,
    pending: Vec<(Vec<u8>, Vec<u8>)>,
}

impl<'a, K: KvBatch> SmtOverlayStore<'a, K> {
    const fn new(db: &'a K) -> Self {
        Self {
            db,
            pending: Vec::new(),
        }
    }

    fn into_write_ops(mut self) -> Vec<WriteOp> {
        // Sort by key for deterministic ordering
        self.pending.sort_by(|a, b| a.0.cmp(&b.0));

        self.pending
            .into_iter()
            .map(|(k, v)| WriteOp::Put(k, v))
            .collect()
    }

    fn pending_get(&self, key: &[u8]) -> Option<&[u8]> {
        // last-write-wins
        for (k, v) in self.pending.iter().rev() {
            if k.as_slice() == key {
                return Some(v.as_slice());
            }
        }
        None
    }
}

impl<K: KvBatch> SmtStore for SmtOverlayStore<'_, K> {
    type Error = K::Error;

    fn get_node(&self, node_hash: &Hash32) -> Result<Option<[u8; Node::ENCODED_LEN]>, Self::Error> {
        let key = smt_node_key(node_hash);

        // First check buffered writes
        if let Some(v) = self.pending_get(&key) {
            if v.len() != Node::ENCODED_LEN {
                return Ok(None);
            }
            let mut out = [0u8; Node::ENCODED_LEN];
            out.copy_from_slice(v);
            return Ok(Some(out));
        }

        match self.db.get(&key)? {
            None => Ok(None),
            Some(v) => {
                if v.len() != Node::ENCODED_LEN {
                    return Ok(None);
                }
                let mut out = [0u8; Node::ENCODED_LEN];
                out.copy_from_slice(&v);
                Ok(Some(out))
            }
        }
    }

    fn put_node(
        &mut self,
        node_hash: &Hash32,
        node_bytes: &[u8; Node::ENCODED_LEN],
    ) -> Result<(), Self::Error> {
        let key = smt_node_key(node_hash);
        self.pending.push((key, node_bytes.to_vec()));
        Ok(())
    }
}

/// Helper to read SMT root or return empty root for new genesis.
fn read_smt_root_or_default<K: KvBatch>(db: &K) -> Result<Hash32, K::Error> {
    (db.get(KEY_SMT_ROOT)?).map_or_else(
        || Ok(empty_hash_at_height(256)),
        |bytes| decode_smt_root_v1(&bytes).map_err(|_| panic!("Invalid SMT root in database")),
    )
}

/// Compute SMT operations for genesis state operations.
fn append_smt_ops_for_genesis<K: KvBatch>(
    db: &K,
    state_ops: &[WriteOp],
    out_ops: &mut Vec<WriteOp>,
) -> Result<Hash32, GenesisError> {
    let cur_root = read_smt_root_or_default(db)
        .map_err(|_| GenesisError::ValidationError("Failed to read SMT root".to_string()))?;

    // Build SMT updates in overlay store
    let store = SmtOverlayStore::new(db);
    let mut smt = Smt::with_root(store, cur_root);

    for op in state_ops {
        match op {
            WriteOp::Put(k, v) => {
                let sk: Hash32 = smt_key_for_state_key(k);
                smt.update(sk, v).map_err(|e| match e {
                    SmtError::Store(_) => {
                        GenesisError::ValidationError("SMT store error".to_string())
                    }
                    _ => GenesisError::ValidationError("SMT update failed".to_string()),
                })?;
            }
            WriteOp::Delete(k) => {
                let sk: Hash32 = smt_key_for_state_key(k);
                smt.delete(sk).map_err(|e| match e {
                    SmtError::Store(_) => {
                        GenesisError::ValidationError("SMT store error".to_string())
                    }
                    _ => GenesisError::ValidationError("SMT delete failed".to_string()),
                })?;
            }
        }
    }

    let new_root = smt.root();
    let store = smt.into_store();

    // Add SMT node writes
    out_ops.extend(store.into_write_ops());

    // Add root record write
    out_ops.push(WriteOp::Put(
        KEY_SMT_ROOT.to_vec(),
        encode_smt_root_v1(&new_root).to_vec(),
    ));

    Ok(new_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_genesis() {
        let json = r#"{
            "chain_id": "novai-testnet-1",
            "protocol_version": 1,
            "timestamp": "2025-01-01T00:00:00Z",
            "validators": [
                {
                    "pubkey": "0000000000000000000000000000000000000000000000000000000000000001",
                    "initial_stake": "1000000",
                    "name": "validator-1"
                }
            ],
            "accounts": {
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": "500000"
            },
            "ai_entities": []
        }"#;

        let config = GenesisConfig::from_json(json).unwrap();
        assert_eq!(config.chain_id, "novai-testnet-1");
        assert_eq!(config.protocol_version, 1);
        assert_eq!(config.validators.len(), 1);
        assert_eq!(
            config.validators[0].pubkey,
            "0000000000000000000000000000000000000000000000000000000000000001"
        );
        assert_eq!(config.validators[0].initial_stake, "1000000");
        assert_eq!(config.validators[0].name, Some("validator-1".to_string()));
        assert_eq!(config.accounts.len(), 1);
    }

    #[test]
    fn reject_empty_chain_id() {
        let json = r#"{
            "chain_id": "",
            "protocol_version": 1,
            "timestamp": "2025-01-01T00:00:00Z",
            "validators": [
                {
                    "pubkey": "0000000000000000000000000000000000000000000000000000000000000001",
                    "initial_stake": "1000000"
                }
            ]
        }"#;

        let result = GenesisConfig::from_json(json);
        assert!(result.is_err());
        match result.unwrap_err() {
            GenesisError::ValidationError(msg) => {
                assert!(msg.contains("chain_id must be non-empty"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn reject_no_validators() {
        let json = r#"{
            "chain_id": "novai-testnet-1",
            "protocol_version": 1,
            "timestamp": "2025-01-01T00:00:00Z",
            "validators": []
        }"#;

        let result = GenesisConfig::from_json(json);
        assert!(result.is_err());
        match result.unwrap_err() {
            GenesisError::ValidationError(msg) => {
                assert!(msg.contains("Must have at least 1 validator"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn reject_protocol_version_zero() {
        let json = r#"{
            "chain_id": "novai-testnet-1",
            "protocol_version": 0,
            "timestamp": "2025-01-01T00:00:00Z",
            "validators": [
                {
                    "pubkey": "0000000000000000000000000000000000000000000000000000000000000001",
                    "initial_stake": "1000000"
                }
            ]
        }"#;

        let result = GenesisConfig::from_json(json);
        assert!(result.is_err());
        match result.unwrap_err() {
            GenesisError::ValidationError(msg) => {
                assert!(msg.contains("protocol_version must be >= 1"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn reject_invalid_timestamp() {
        let json = r#"{
            "chain_id": "novai-testnet-1",
            "protocol_version": 1,
            "timestamp": "not-a-timestamp",
            "validators": [
                {
                    "pubkey": "0000000000000000000000000000000000000000000000000000000000000001",
                    "initial_stake": "1000000"
                }
            ]
        }"#;

        let result = GenesisConfig::from_json(json);
        assert!(result.is_err());
        match result.unwrap_err() {
            GenesisError::ValidationError(msg) => {
                assert!(msg.contains("Invalid timestamp"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn reject_invalid_pubkey_length() {
        let json = r#"{
            "chain_id": "novai-testnet-1",
            "protocol_version": 1,
            "timestamp": "2025-01-01T00:00:00Z",
            "validators": [
                {
                    "pubkey": "0001",
                    "initial_stake": "1000000"
                }
            ]
        }"#;

        let result = GenesisConfig::from_json(json);
        assert!(result.is_err());
        match result.unwrap_err() {
            GenesisError::ValidationError(msg) => {
                assert!(msg.contains("must be 64 hex characters"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn reject_invalid_stake_amount() {
        let json = r#"{
            "chain_id": "novai-testnet-1",
            "protocol_version": 1,
            "timestamp": "2025-01-01T00:00:00Z",
            "validators": [
                {
                    "pubkey": "0000000000000000000000000000000000000000000000000000000000000001",
                    "initial_stake": "not-a-number"
                }
            ]
        }"#;

        let result = GenesisConfig::from_json(json);
        assert!(result.is_err());
        match result.unwrap_err() {
            GenesisError::ValidationError(msg) => {
                assert!(msg.contains("must be valid u64"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn reject_too_many_validators() {
        let mut validators = Vec::new();
        for i in 0..101 {
            validators.push(format!(
                r#"{{"pubkey": "{i:064x}", "initial_stake": "1000000"}}"#
            ));
        }
        let json = format!(
            r#"{{
                "chain_id": "novai-testnet-1",
                "protocol_version": 1,
                "timestamp": "2025-01-01T00:00:00Z",
                "validators": [{}]
            }}"#,
            validators.join(",")
        );

        let result = GenesisConfig::from_json(&json);
        assert!(result.is_err());
        match result.unwrap_err() {
            GenesisError::ValidationError(msg) => {
                assert!(msg.contains("Cannot have more than 100 validators"));
            }
            _ => panic!("Expected ValidationError"),
        }
    }

    #[test]
    fn to_canonical_json_roundtrip() {
        let json = r#"{
            "chain_id": "novai-testnet-1",
            "protocol_version": 1,
            "timestamp": "2025-01-01T00:00:00Z",
            "validators": [
                {
                    "pubkey": "0000000000000000000000000000000000000000000000000000000000000001",
                    "initial_stake": "1000000"
                }
            ],
            "accounts": {},
            "ai_entities": []
        }"#;

        let config = GenesisConfig::from_json(json).unwrap();
        let canonical = config.to_canonical_json().unwrap();
        let config2 = GenesisConfig::from_json(&canonical).unwrap();
        assert_eq!(config, config2);
    }
}

#[cfg(test)]
mod genesis_generation_tests {
    use super::*;
    use novai_state::MemKv;

    #[test]
    fn test_deterministic_state_root_same_config() {
        let json = r#"{
            "chain_id": "test-chain",
            "protocol_version": 1,
            "timestamp": "2025-01-01T00:00:00Z",
            "validators": [
                {
                    "pubkey": "0000000000000000000000000000000000000000000000000000000000000000",
                    "initial_stake": "1000000"
                }
            ],
            "accounts": {
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": "500000",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb": "300000"
            }
        }"#;

        let config = GenesisConfig::from_json(json).unwrap();
        let generator = GenesisGenerator::new(config);

        // Generate state twice
        let mut db1 = MemKv::new();
        let state1 = generator.generate(&mut db1).unwrap();

        let mut db2 = MemKv::new();
        let state2 = generator.generate(&mut db2).unwrap();

        // State roots must be identical
        assert_eq!(state1.state_root, state2.state_root);
        assert_eq!(
            state1.genesis_block.state_root,
            state2.genesis_block.state_root
        );
        assert_eq!(state1.validator_set, state2.validator_set);
    }

    #[test]
    fn test_golden_genesis_state_root() {
        // Fixed genesis configuration
        let json = r#"{
            "chain_id": "novai-testnet-golden",
            "protocol_version": 1,
            "timestamp": "2025-01-17T00:00:00Z",
            "validators": [
                {
                    "pubkey": "0000000000000000000000000000000000000000000000000000000000000000",
                    "initial_stake": "1000000"
                }
            ],
            "accounts": {
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": "1000000000"
            }
        }"#;

        let config = GenesisConfig::from_json(json).unwrap();
        let generator = GenesisGenerator::new(config);

        let mut db = MemKv::new();
        let state = generator.generate(&mut db).unwrap();

        // Verify deterministic state root (32 bytes)
        assert_eq!(state.state_root.len(), 32);

        // Golden state root - locked to ensure determinism across platforms/versions
        let expected_state_root = [
            0xf7, 0x50, 0x1c, 0xf4, 0x14, 0x61, 0x9c, 0x9a, 0x69, 0xc4, 0x66, 0x5d, 0xdb, 0x50,
            0xf5, 0xd5, 0xef, 0x19, 0x48, 0xc3, 0xa7, 0x3d, 0x50, 0x8d, 0x68, 0x20, 0x9e, 0x7a,
            0x18, 0x51, 0x5d, 0xd7,
        ];

        assert_eq!(
            state.state_root,
            expected_state_root,
            "Genesis state root changed! Expected: {}, Got: {}",
            hex::encode(expected_state_root),
            hex::encode(state.state_root)
        );
    }

    #[test]
    fn test_genesis_block_structure() {
        let json = r#"{
            "chain_id": "test-chain",
            "protocol_version": 1,
            "timestamp": "2025-01-01T00:00:00Z",
            "validators": [
                {
                    "pubkey": "0000000000000000000000000000000000000000000000000000000000000000",
                    "initial_stake": "1000000"
                }
            ],
            "accounts": {}
        }"#;

        let config = GenesisConfig::from_json(json).unwrap();
        let generator = GenesisGenerator::new(config);

        let mut db = MemKv::new();
        let state = generator.generate(&mut db).unwrap();

        // Genesis block properties
        assert_eq!(state.genesis_block.height, 0);
        assert_eq!(state.genesis_block.round, 0);
        assert_eq!(state.genesis_block.parent_hash, [0u8; 32]);
        assert!(state.genesis_block.txs.is_empty());
        assert_eq!(state.genesis_block.state_root, state.state_root);
    }

    #[test]
    fn test_validator_set_deterministic_ordering() {
        let json = r#"{
            "chain_id": "test-chain",
            "protocol_version": 1,
            "timestamp": "2025-01-01T00:00:00Z",
            "validators": [
                {
                    "pubkey": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                    "initial_stake": "1000000"
                },
                {
                    "pubkey": "0000000000000000000000000000000000000000000000000000000000000000",
                    "initial_stake": "1000000"
                },
                {
                    "pubkey": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "initial_stake": "1000000"
                }
            ],
            "accounts": {}
        }"#;

        let config = GenesisConfig::from_json(json).unwrap();
        let generator = GenesisGenerator::new(config);

        let mut db = MemKv::new();
        let state = generator.generate(&mut db).unwrap();

        // Validators should be sorted by address, not by input order
        assert_eq!(state.validator_set.len(), 3);

        // Verify sorted order
        for i in 1..state.validator_set.len() {
            assert!(state.validator_set[i - 1] < state.validator_set[i]);
        }
    }

    #[test]
    fn test_empty_accounts_produces_valid_state() {
        let json = r#"{
            "chain_id": "test-chain",
            "protocol_version": 1,
            "timestamp": "2025-01-01T00:00:00Z",
            "validators": [
                {
                    "pubkey": "0000000000000000000000000000000000000000000000000000000000000000",
                    "initial_stake": "1000000"
                }
            ],
            "accounts": {}
        }"#;

        let config = GenesisConfig::from_json(json).unwrap();
        let generator = GenesisGenerator::new(config);

        let mut db = MemKv::new();
        let state = generator.generate(&mut db).unwrap();

        // Should produce valid state even with no accounts
        assert_eq!(state.state_root.len(), 32);
        assert_eq!(state.validator_set.len(), 1);
    }

    #[test]
    fn test_multiple_accounts_deterministic() {
        let json = r#"{
            "chain_id": "test-chain",
            "protocol_version": 1,
            "timestamp": "2025-01-01T00:00:00Z",
            "validators": [
                {
                    "pubkey": "0000000000000000000000000000000000000000000000000000000000000000",
                    "initial_stake": "1000000"
                }
            ],
            "accounts": {
                "1111111111111111111111111111111111111111111111111111111111111111": "100",
                "2222222222222222222222222222222222222222222222222222222222222222": "200",
                "3333333333333333333333333333333333333333333333333333333333333333": "300"
            }
        }"#;

        let config = GenesisConfig::from_json(json).unwrap();
        let generator = GenesisGenerator::new(config);

        // Generate twice
        let mut db1 = MemKv::new();
        let state1 = generator.generate(&mut db1).unwrap();

        let mut db2 = MemKv::new();
        let state2 = generator.generate(&mut db2).unwrap();

        // Must be identical
        assert_eq!(state1.state_root, state2.state_root);
    }
}
