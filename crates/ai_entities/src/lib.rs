//! AI Entity types for NOVAI blockchain.
//!
//! This crate defines first-class protocol primitives for AI entities,
//! including their identity, memory, economic agency, and autonomy levels.
//!
//! # Modules
//!
//! - `signals` - AI advisory signal types and commitments
//! - `gates` - Approval gates for AI action execution
//! - `tiers` - Action tier classification system
//! - `artifacts` - Content-addressed storage for off-chain data (Week 15)
//! - `payload` - Signal payload format for off-chain storage (Week 15)
//! - `verify` - Verification flow for payloads and memory (Week 15)
//! - `memory` - On-chain memory objects for AI entities (Week 21)
//! - `privacy` - NNPX privacy commitment types (Week 22)
//! - `derived_views` - Derived views for AI privacy bridge (Week 23)

use blake3::Hasher;

pub mod artifacts;
pub mod derived_views;
pub mod gates;
pub mod memory;
pub mod payload;
pub mod privacy;
pub mod signals;
pub mod tiers;
pub mod verify;

pub use artifacts::*;
pub use derived_views::*;
pub use gates::*;
pub use memory::*;
use novai_types::Address;
pub use payload::*;
pub use privacy::*;
pub use signals::*;
pub use tiers::*;
pub use verify::*;

/// Unique identifier for an AI entity.
pub type AiEntityId = [u8; 32];

/// Hash of AI module code/weights.
pub type CodeHash = [u8; 32];

/// Domain separator for AI entity ID computation.
const AI_ENTITY_ID_DOMAIN: &[u8] = b"NOVAI_AI_ENTITY_ID_V1";

/// Domain separator for module manifest ID computation.
const MODULE_MANIFEST_ID_DOMAIN: &[u8] = b"NOVAI_MODULE_MANIFEST_V1";

// ============================================================================
// WELL-KNOWN MODULE IDENTIFIERS (Week 24 - Core Observer)
// ============================================================================

/// Well-known code hash for NOVAI Core Observer (testnet reference module).
///
/// Computed as: `blake3("NOVAI_CORE_OBSERVER_V1")`
///
/// The Core Observer is a reference AI module that:
/// - Emits Anomaly signals when detecting unusual chain behavior
/// - Emits CongestionForecast signals with predicted load
/// - Stores ChainSummary and StatisticsSnapshot memory objects
pub const CORE_OBSERVER_CODE_HASH: CodeHash = [
    0x53, 0x1c, 0xc2, 0xd3, 0xeb, 0x0a, 0xc4, 0x7e, 0x92, 0x6f, 0xb5, 0x1e, 0x3a, 0xcc, 0xc1, 0x11,
    0x5d, 0xd5, 0x49, 0x5e, 0x31, 0x76, 0xb1, 0x4b, 0xb0, 0x8d, 0x65, 0x98, 0x4e, 0xc7, 0x0f, 0x3a,
];

/// Protocol genesis creator address (well-known authority for genesis modules).
///
/// Computed as: `blake3("NOVAI_PROTOCOL_GENESIS_V1")`
///
/// Used as the `creator` field for protocol-defined AI modules registered
/// at genesis. This is not a real cryptographic key - it's a deterministic
/// placeholder that marks modules as "created by the protocol itself".
pub const PROTOCOL_CREATOR: Address = [
    0x5e, 0x04, 0x3a, 0x2b, 0x43, 0x63, 0x65, 0x79, 0x20, 0x27, 0xea, 0x58, 0x58, 0x05, 0x7a, 0xf1,
    0x47, 0x9c, 0x06, 0x19, 0xd7, 0xf1, 0xf7, 0x64, 0x63, 0xf5, 0x07, 0xf5, 0xb0, 0x8f, 0xd7, 0x9f,
];

/// Autonomy mode determines how AI proposals are processed.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AutonomyMode {
    /// Advisory mode: AI can only emit proposals, never execute.
    #[default]
    Advisory = 0,
    /// Gated mode (Mode B): proposals go through approval gates.
    Gated = 1,
    /// Autonomous mode (Mode C): reserved for future (requires ZK proofs).
    Autonomous = 2,
}

impl AutonomyMode {
    /// Encode to canonical byte representation.
    pub fn to_byte(self) -> u8 {
        self as u8
    }

    /// Decode from byte, returning None for invalid values.
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(AutonomyMode::Advisory),
            1 => Some(AutonomyMode::Gated),
            2 => Some(AutonomyMode::Autonomous),
            _ => None,
        }
    }
}

/// Capability flags defining what an AI entity is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    /// Can read public chain state (blocks, txs, accounts).
    pub read_public_chain: bool,
    /// Can read L1 memory objects.
    pub read_memory_objects: bool,
    /// Can emit proposal objects.
    pub emit_proposals: bool,
    /// Can request Tier 1/2 action execution (must pass gates).
    pub request_execution: bool,
    /// Can read NNPX derived views (bounded, schema-validated).
    pub read_nnpx_derived: bool,
    /// Can submit ReputationUpdate signals (oracle entities).
    pub submit_reputation_updates: bool,
    /// Reserved for future capabilities.
    pub _reserved: [bool; 2],
}

impl Capabilities {
    /// Encode to canonical 8-bit flags.
    pub fn to_byte(&self) -> u8 {
        let mut flags = 0u8;
        if self.read_public_chain {
            flags |= 1 << 0;
        }
        if self.read_memory_objects {
            flags |= 1 << 1;
        }
        if self.emit_proposals {
            flags |= 1 << 2;
        }
        if self.request_execution {
            flags |= 1 << 3;
        }
        if self.read_nnpx_derived {
            flags |= 1 << 4;
        }
        if self.submit_reputation_updates {
            flags |= 1 << 5;
        }
        flags
    }

    /// Decode from canonical 8-bit flags.
    pub fn from_byte(byte: u8) -> Self {
        Self {
            read_public_chain: (byte & (1 << 0)) != 0,
            read_memory_objects: (byte & (1 << 1)) != 0,
            emit_proposals: (byte & (1 << 2)) != 0,
            request_execution: (byte & (1 << 3)) != 0,
            read_nnpx_derived: (byte & (1 << 4)) != 0,
            submit_reputation_updates: (byte & (1 << 5)) != 0,
            _reserved: [false; 2],
        }
    }

    /// Create minimal read-only capability set.
    pub fn read_only() -> Self {
        Self {
            read_public_chain: true,
            read_memory_objects: true,
            emit_proposals: false,
            request_execution: false,
            read_nnpx_derived: false,
            submit_reputation_updates: false,
            _reserved: [false; 2],
        }
    }

    /// Create advisory capability set (can propose but not execute).
    pub fn advisory() -> Self {
        Self {
            read_public_chain: true,
            read_memory_objects: true,
            emit_proposals: true,
            request_execution: false,
            read_nnpx_derived: false,
            submit_reputation_updates: false,
            _reserved: [false; 2],
        }
    }

    /// Create gated capability set (can request execution via gates).
    pub fn gated() -> Self {
        Self {
            read_public_chain: true,
            read_memory_objects: true,
            emit_proposals: true,
            request_execution: true,
            read_nnpx_derived: false,
            submit_reputation_updates: false,
            _reserved: [false; 2],
        }
    }

    /// Return a capability set with every flag enabled in `self` OR `other`.
    ///
    /// Used to fold an entity's static capabilities together with capabilities
    /// granted via active delegation memory objects when resolving the
    /// effective capabilities for a transaction.
    #[must_use]
    pub fn or(&self, other: &Self) -> Self {
        Self {
            read_public_chain: self.read_public_chain || other.read_public_chain,
            read_memory_objects: self.read_memory_objects || other.read_memory_objects,
            emit_proposals: self.emit_proposals || other.emit_proposals,
            request_execution: self.request_execution || other.request_execution,
            read_nnpx_derived: self.read_nnpx_derived || other.read_nnpx_derived,
            submit_reputation_updates: self.submit_reputation_updates
                || other.submit_reputation_updates,
            _reserved: [
                self._reserved[0] || other._reserved[0],
                self._reserved[1] || other._reserved[1],
            ],
        }
    }
}

impl core::ops::BitOr for Capabilities {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.or(&rhs)
    }
}

/// An AI entity registered on-chain.
///
/// AI entities are first-class protocol primitives with:
/// - Stable identity (derived from code + creator)
/// - Persistent memory (survives across blocks)
/// - Economic agency (owns assets, pays fees)
/// - Defined autonomy level and capabilities
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiEntity {
    /// Unique identifier: blake3(domain || code_hash || creator).
    pub id: AiEntityId,
    /// Hash of the module's code/weights.
    pub code_hash: CodeHash,
    /// Address of the entity that created this AI.
    pub creator: Address,
    /// Current autonomy mode.
    pub autonomy_mode: AutonomyMode,
    /// Granted capabilities.
    pub capabilities: Capabilities,
    /// Balance owned by this AI entity (for paying fees).
    pub economic_balance: u128,
    /// Nonce for AI-initiated transactions (prevents replay).
    pub nonce: u64,
    /// Ed25519 public key for signing transactions.
    /// `[0u8; 32]` means no key assigned (legacy/V1/V2 entities).
    pub pubkey: [u8; 32],
    /// Root hash of persistent memory tree.
    pub memory_root: [u8; 32],
    /// Root hash of learned parameters tree.
    pub params_root: [u8; 32],
    /// Block height when entity was registered.
    pub registered_at: u64,
    /// Block height of last activity.
    pub last_active_at: u64,
    /// Whether this entity is currently active (can execute).
    /// Set via ModuleActivation/ModuleRollback governance proposals.
    pub is_active: bool,
    /// Reputation score in [0, 100]. Defaults to 50 for new entities (neutral).
    pub reputation_score: u16,
    /// Total transactions counted toward reputation (e.g., job completions).
    pub total_transactions: u32,
    /// Number of reputation events applied to this entity.
    pub reputation_events_count: u32,
    /// Amount of stake locked as collateral, in smallest balance unit.
    pub stake_balance: u128,
    /// Block height until which stake cannot be withdrawn (0 = unlocked).
    pub stake_locked_until: u64,
}

/// Default reputation score for newly registered entities (neutral midpoint).
pub const DEFAULT_REPUTATION_SCORE: u16 = 50;

/// Maximum reputation score (clamp ceiling).
pub const MAX_REPUTATION_SCORE: u16 = 100;

impl AiEntity {
    /// Compute the canonical entity ID from code hash and creator.
    pub fn compute_id(code_hash: &CodeHash, creator: &Address) -> AiEntityId {
        let mut hasher = Hasher::new();
        hasher.update(AI_ENTITY_ID_DOMAIN);
        hasher.update(code_hash);
        hasher.update(creator);
        *hasher.finalize().as_bytes()
    }

    /// Create a new AI entity with defaults for most fields.
    pub fn new(
        code_hash: CodeHash,
        creator: Address,
        autonomy_mode: AutonomyMode,
        capabilities: Capabilities,
        registered_at: u64,
    ) -> Self {
        let id = Self::compute_id(&code_hash, &creator);
        Self {
            id,
            code_hash,
            creator,
            autonomy_mode,
            capabilities,
            economic_balance: 0,
            nonce: 0,
            pubkey: [0u8; 32],
            memory_root: [0u8; 32],
            params_root: [0u8; 32],
            registered_at,
            last_active_at: registered_at,
            is_active: true,
            reputation_score: DEFAULT_REPUTATION_SCORE,
            total_transactions: 0,
            reputation_events_count: 0,
            stake_balance: 0,
            stake_locked_until: 0,
        }
    }

    /// Create a new AI entity with an ed25519 public key.
    pub fn new_with_pubkey(
        code_hash: CodeHash,
        creator: Address,
        autonomy_mode: AutonomyMode,
        capabilities: Capabilities,
        pubkey: [u8; 32],
        registered_at: u64,
    ) -> Self {
        let id = Self::compute_id(&code_hash, &creator);
        Self {
            id,
            code_hash,
            creator,
            autonomy_mode,
            capabilities,
            economic_balance: 0,
            nonce: 0,
            pubkey,
            memory_root: [0u8; 32],
            params_root: [0u8; 32],
            registered_at,
            last_active_at: registered_at,
            is_active: true,
            reputation_score: DEFAULT_REPUTATION_SCORE,
            total_transactions: 0,
            reputation_events_count: 0,
            stake_balance: 0,
            stake_locked_until: 0,
        }
    }

    /// Returns true if this entity has an assigned public key (non-zero).
    pub fn has_pubkey(&self) -> bool {
        self.pubkey != [0u8; 32]
    }

    /// Check if entity has a specific capability.
    pub fn has_capability(&self, cap: &str) -> bool {
        match cap {
            "read_public_chain" => self.capabilities.read_public_chain,
            "read_memory_objects" => self.capabilities.read_memory_objects,
            "emit_proposals" => self.capabilities.emit_proposals,
            "request_execution" => self.capabilities.request_execution,
            "read_nnpx_derived" => self.capabilities.read_nnpx_derived,
            "submit_reputation_updates" => self.capabilities.submit_reputation_updates,
            _ => false,
        }
    }
}

/// Module manifest - immutable registration of an AI module version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleManifest {
    /// Unique manifest ID: blake3(all fields).
    pub manifest_id: [u8; 32],
    /// Human-readable name.
    pub name: String,
    /// Semantic version string.
    pub version: String,
    /// Hash of the module code.
    pub code_hash: CodeHash,
    /// Hash of the weights/parameters artifact.
    pub weights_hash: [u8; 32],
    /// Hash of the configuration artifact.
    pub config_hash: [u8; 32],
    /// Creator/author public key.
    pub author: Address,
    /// Requested capabilities.
    pub capabilities: Capabilities,
    /// Autonomy level requested.
    pub autonomy_mode: AutonomyMode,
    /// Declaration of deterministic runtime requirements.
    pub determinism_declaration: DeterminismDeclaration,
}

impl ModuleManifest {
    /// Compute canonical manifest ID.
    pub fn compute_id(&self) -> [u8; 32] {
        let mut hasher = Hasher::new();
        hasher.update(MODULE_MANIFEST_ID_DOMAIN);
        // Hash name with length prefix
        hasher.update(&(self.name.len() as u32).to_le_bytes());
        hasher.update(self.name.as_bytes());
        // Hash version with length prefix
        hasher.update(&(self.version.len() as u32).to_le_bytes());
        hasher.update(self.version.as_bytes());
        // Hash fixed-size fields
        hasher.update(&self.code_hash);
        hasher.update(&self.weights_hash);
        hasher.update(&self.config_hash);
        hasher.update(&self.author);
        hasher.update(&[self.capabilities.to_byte()]);
        hasher.update(&[self.autonomy_mode.to_byte()]);
        // Hash determinism declaration
        hasher.update(&[self.determinism_declaration.no_floats as u8]);
        hasher.update(&[self.determinism_declaration.deterministic_iteration as u8]);
        hasher.update(&[self.determinism_declaration.no_time_dependency as u8]);
        match self.determinism_declaration.fixed_point_precision {
            Some(v) => hasher.update(&[1, v]),
            None => hasher.update(&[0]),
        };
        *hasher.finalize().as_bytes()
    }
}

/// Declaration of deterministic runtime requirements.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeterminismDeclaration {
    /// Uses only fixed-point arithmetic (no floats).
    pub no_floats: bool,
    /// Uses only deterministic iteration (sorted keys).
    pub deterministic_iteration: bool,
    /// No time-based behavior.
    pub no_time_dependency: bool,
    /// Specifies fixed-point precision if applicable.
    pub fixed_point_precision: Option<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_id_is_deterministic() {
        let code_hash = [0x42u8; 32];
        let creator = [0x01u8; 32];

        let id1 = AiEntity::compute_id(&code_hash, &creator);
        let id2 = AiEntity::compute_id(&code_hash, &creator);

        assert_eq!(id1, id2, "Entity ID must be deterministic");
    }

    #[test]
    fn entity_id_changes_with_code() {
        let creator = [0x01u8; 32];

        let id1 = AiEntity::compute_id(&[0x42u8; 32], &creator);
        let id2 = AiEntity::compute_id(&[0x43u8; 32], &creator);

        assert_ne!(id1, id2, "Different code must produce different ID");
    }

    #[test]
    fn entity_id_changes_with_creator() {
        let code_hash = [0x42u8; 32];

        let id1 = AiEntity::compute_id(&code_hash, &[0x01u8; 32]);
        let id2 = AiEntity::compute_id(&code_hash, &[0x02u8; 32]);

        assert_ne!(id1, id2, "Different creator must produce different ID");
    }

    #[test]
    fn autonomy_mode_roundtrip() {
        for mode in [
            AutonomyMode::Advisory,
            AutonomyMode::Gated,
            AutonomyMode::Autonomous,
        ] {
            let byte = mode.to_byte();
            let decoded = AutonomyMode::from_byte(byte).unwrap();
            assert_eq!(mode, decoded);
        }
    }

    #[test]
    fn capabilities_roundtrip() {
        let caps = Capabilities::gated();
        let byte = caps.to_byte();
        let decoded = Capabilities::from_byte(byte);
        assert_eq!(caps.read_public_chain, decoded.read_public_chain);
        assert_eq!(caps.read_memory_objects, decoded.read_memory_objects);
        assert_eq!(caps.emit_proposals, decoded.emit_proposals);
        assert_eq!(caps.request_execution, decoded.request_execution);
        assert_eq!(caps.read_nnpx_derived, decoded.read_nnpx_derived);
        assert_eq!(
            caps.submit_reputation_updates,
            decoded.submit_reputation_updates
        );
    }

    #[test]
    fn capabilities_bit5_is_submit_reputation_updates() {
        let caps = Capabilities {
            submit_reputation_updates: true,
            ..Capabilities::default()
        };
        let byte = caps.to_byte();
        assert_eq!(byte, 1 << 5, "submit_reputation_updates must occupy bit 5");
        let decoded = Capabilities::from_byte(byte);
        assert!(decoded.submit_reputation_updates);
        assert!(!decoded.read_public_chain);
    }

    #[test]
    fn invalid_autonomy_mode_returns_none() {
        assert!(AutonomyMode::from_byte(255).is_none());
        assert!(AutonomyMode::from_byte(3).is_none());
    }

    #[test]
    fn default_autonomy_mode_is_advisory() {
        assert_eq!(AutonomyMode::default(), AutonomyMode::Advisory);
    }

    #[test]
    fn capability_constructors() {
        let read_only = Capabilities::read_only();
        assert!(read_only.read_public_chain);
        assert!(read_only.read_memory_objects);
        assert!(!read_only.emit_proposals);
        assert!(!read_only.request_execution);
        assert!(!read_only.submit_reputation_updates);

        let advisory = Capabilities::advisory();
        assert!(advisory.emit_proposals);
        assert!(!advisory.request_execution);
        assert!(!advisory.submit_reputation_updates);

        let gated = Capabilities::gated();
        assert!(gated.emit_proposals);
        assert!(gated.request_execution);
        assert!(!gated.submit_reputation_updates);
    }

    #[test]
    fn ai_entity_new_computes_id() {
        let code_hash = [0x42u8; 32];
        let creator = [0x01u8; 32];

        let entity = AiEntity::new(
            code_hash,
            creator,
            AutonomyMode::Gated,
            Capabilities::gated(),
            1000,
        );

        let expected_id = AiEntity::compute_id(&code_hash, &creator);
        assert_eq!(entity.id, expected_id);
        assert_eq!(entity.economic_balance, 0);
        assert_eq!(entity.nonce, 0);
        assert_eq!(entity.registered_at, 1000);
        assert_eq!(entity.last_active_at, 1000);
        assert_eq!(entity.reputation_score, DEFAULT_REPUTATION_SCORE);
        assert_eq!(entity.total_transactions, 0);
        assert_eq!(entity.reputation_events_count, 0);
    }

    #[test]
    fn has_capability_works() {
        let entity = AiEntity::new(
            [0x42u8; 32],
            [0x01u8; 32],
            AutonomyMode::Gated,
            Capabilities::gated(),
            1000,
        );

        assert!(entity.has_capability("read_public_chain"));
        assert!(entity.has_capability("emit_proposals"));
        assert!(entity.has_capability("request_execution"));
        assert!(!entity.has_capability("read_nnpx_derived"));
        assert!(!entity.has_capability("unknown_capability"));
    }

    #[test]
    fn module_manifest_compute_id_is_deterministic() {
        let manifest = ModuleManifest {
            manifest_id: [0u8; 32],
            name: "test-module".to_string(),
            version: "1.0.0".to_string(),
            code_hash: [0x42u8; 32],
            weights_hash: [0x43u8; 32],
            config_hash: [0x44u8; 32],
            author: [0x01u8; 32],
            capabilities: Capabilities::advisory(),
            autonomy_mode: AutonomyMode::Advisory,
            determinism_declaration: DeterminismDeclaration::default(),
        };

        let id1 = manifest.compute_id();
        let id2 = manifest.compute_id();

        assert_eq!(id1, id2, "Manifest ID must be deterministic");
    }

    #[test]
    fn capabilities_or_idempotent() {
        let caps = Capabilities::gated();
        let merged = caps.or(&caps);
        assert_eq!(caps.to_byte(), merged.to_byte());
    }

    #[test]
    fn capabilities_or_with_default_is_identity() {
        let caps = Capabilities::advisory();
        let identity = Capabilities::default();
        assert_eq!(caps.to_byte(), caps.or(&identity).to_byte());
        assert_eq!(caps.to_byte(), identity.or(&caps).to_byte());
    }

    #[test]
    fn capabilities_or_combines_disjoint_sets() {
        let read_only = Capabilities::read_only();
        let signal_oracle = Capabilities {
            submit_reputation_updates: true,
            ..Capabilities::default()
        };
        let merged = read_only.or(&signal_oracle);
        assert!(merged.read_public_chain);
        assert!(merged.read_memory_objects);
        assert!(merged.submit_reputation_updates);
        assert!(!merged.emit_proposals);
        assert!(!merged.request_execution);
    }

    #[test]
    fn capabilities_or_preserves_byte_layout() {
        let a = Capabilities::from_byte(0b0000_0011);
        let b = Capabilities::from_byte(0b0010_0100);
        let merged = a.or(&b);
        assert_eq!(merged.to_byte(), 0b0010_0111);
    }

    #[test]
    fn capabilities_bitor_matches_or_method() {
        let a = Capabilities::advisory();
        let b = Capabilities::gated();
        let by_method = a.or(&b);
        let by_op = a | b;
        assert_eq!(by_method.to_byte(), by_op.to_byte());
    }

    #[test]
    fn capabilities_or_merges_reserved_bits() {
        let a = Capabilities {
            _reserved: [true, false],
            ..Capabilities::default()
        };
        let b = Capabilities {
            _reserved: [false, true],
            ..Capabilities::default()
        };
        let merged = a.or(&b);
        assert_eq!(merged._reserved, [true, true]);
    }
}
