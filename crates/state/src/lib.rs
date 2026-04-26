//! novai-state
//!
//! Week 3 scope: state DB abstraction + schema keys (account + fee pool).
//!
//! Invariants (week 3):
//! - Keys are canonical byte strings (no locale / no Unicode normalization issues).
//! - State values are opaque bytes at this layer (execution defines encoding).
//! - No nondeterminism is introduced by this crate.
//!
//! Failure modes:
//! - DB I/O errors must be surfaced; never silently ignored.
//! - Feature-gated RocksDB may be unavailable on some machines (missing toolchain).
//!
//! NOTE: We keep this crate minimal in Week 3; more logic comes in later weeks.

/// Canonical prefix for account records.
pub const KEY_PREFIX_ACCOUNTS: &[u8] = b"accounts/";

/// Canonical key for the fee pool balance record.
pub const KEY_FEE_POOL: &[u8] = b"fee_pool";

/// Canonical key for the current SMT root (32 bytes, versioned encoding).
pub const KEY_SMT_ROOT: &[u8] = b"smt/root";

/// Canonical prefix for SMT node records.
pub const KEY_PREFIX_SMT_NODE: &[u8] = b"smt/node/";

/// Canonical key for committed height (u64 big-endian).
pub const KEY_COMMITTED_HEIGHT: &[u8] = b"consensus/committed_height";

/// Canonical key for executed height (u64 big-endian).
///
/// CRASH-SAFE COMMIT/EXECUTE INVARIANT:
/// - `KEY_COMMITTED_HEIGHT` is written by `persist_commit_atomic` (consensus
///   layer). It marks the highest height whose block + QC are durable on disk.
/// - `KEY_EXECUTED_HEIGHT` is written by the commit callback **after** every
///   tx in those blocks has been dispatched (account state + SMT updated).
/// - On startup, if `executed < committed`, blocks `(executed+1)..=committed`
///   are replayed before the node rejoins consensus. Replay is idempotent —
///   already-applied txs fail nonce/balance checks and are skipped, identical
///   to the original on_commit error path.
///
/// Without this cursor, a crash between `persist_commit_atomic` and the end
/// of execute_committed_blocks leaves account/SMT state behind committed
/// state forever, producing permanent state-root divergence on the next
/// peer's proposal.
pub const KEY_EXECUTED_HEIGHT: &[u8] = b"consensus/executed_height";

/// Canonical prefix for block records by height.
pub const KEY_PREFIX_BLOCKS: &[u8] = b"consensus/blocks/";

/// Canonical prefix for QC records by height.
pub const KEY_PREFIX_QCS: &[u8] = b"consensus/qcs/";

/// Canonical key for the highest QC seen.
pub const KEY_HIGHEST_QC: &[u8] = b"consensus/highest_qc";

// ============================================================================
// AI STORAGE KEY PREFIXES (Retrofit Week 3)
// ============================================================================

/// Canonical prefix for AI entity records.
pub const KEY_PREFIX_AI_ENTITIES: &[u8] = b"ai/entities/";

/// Canonical prefix for AI memory records.
pub const KEY_PREFIX_AI_MEMORY: &[u8] = b"ai/memory/";

/// Canonical prefix for AI parameter records.
pub const KEY_PREFIX_AI_PARAMS: &[u8] = b"ai/params/";

/// Canonical prefix for AI signal records.
pub const KEY_PREFIX_AI_SIGNALS: &[u8] = b"ai/signals/";

/// Canonical prefix for AI signal index by type (Week 14 - D14.4).
pub const KEY_PREFIX_AI_SIGNALS_BY_TYPE: &[u8] = b"ai/signals/by_type/";

/// Canonical prefix for AI signal index by issuer (Week 14 - D14.4).
pub const KEY_PREFIX_AI_SIGNALS_BY_ISSUER: &[u8] = b"ai/signals/by_issuer/";

// ============================================================================
// AI ENTITY REVERSE INDEX (by address → entity_id)
// ============================================================================

/// Canonical prefix for reverse index: AI entity address → entity_id.
/// Key: `ai/entities_by_addr/{address32}` → entity_id (32 bytes)
pub const KEY_PREFIX_AI_ENTITY_BY_ADDR: &[u8] = b"ai/entities_by_addr/";

/// Build the key for reverse-indexing an AI entity by its derived address.
pub fn ai_entity_by_address_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_AI_ENTITY_BY_ADDR.len() + 32);
    k.extend_from_slice(KEY_PREFIX_AI_ENTITY_BY_ADDR);
    k.extend_from_slice(addr);
    k
}

// ============================================================================
// AI MEMORY OBJECT KEY PREFIXES (Week 21)
// ============================================================================

/// Canonical prefix for AI memory object records (Week 21 - D21.3).
/// Key: `ai/memory_objects/{entity_id32}/{object_id32}` → MemoryObject
pub const KEY_PREFIX_AI_MEMORY_OBJECTS: &[u8] = b"ai/memory_objects/";

/// Canonical prefix for AI memory object count per entity (Week 21 - D21.3).
/// Key: `ai/memory_count/{entity_id32}` → u32 count (big-endian)
pub const KEY_PREFIX_AI_MEMORY_COUNT: &[u8] = b"ai/memory_count/";

/// Canonical prefix for AI memory objects indexed by type (Week 21).
/// Key: `ai/memory_by_type/{type_u8}/{entity_id32}/{object_id32}` → empty (presence index)
pub const KEY_PREFIX_AI_MEMORY_BY_TYPE: &[u8] = b"ai/memory_by_type/";

// ============================================================================
// NNPX PRIVACY KEY PREFIXES (Week 22)
// ============================================================================

/// Canonical prefix for NNPX private data (Week 22 - D22.2).
/// All keys with this prefix are routed to the `nnpx` column family.
pub const KEY_PREFIX_NNPX: &[u8] = b"nnpx/";

/// Canonical prefix for NNPX commitment records (Week 22).
/// Key: `nnpx/commitments/{commitment_hash32}` -> PrivatePayloadCommitment
pub const KEY_PREFIX_NNPX_COMMITMENTS: &[u8] = b"nnpx/commitments/";

/// Canonical prefix for NNPX nullifier set (Week 22).
/// Key: `nnpx/nullifiers/{nullifier32}` -> empty (presence indicates spent)
pub const KEY_PREFIX_NNPX_NULLIFIERS: &[u8] = b"nnpx/nullifiers/";

/// Canonical prefix for NNPX encrypted payloads (Week 22).
/// Key: `nnpx/encrypted/{commitment_hash32}` -> encrypted bytes
pub const KEY_PREFIX_NNPX_ENCRYPTED: &[u8] = b"nnpx/encrypted/";

/// RocksDB column family name for private data.
pub const CF_NNPX: &str = "nnpx";

/// RocksDB column family name for public data (default).
pub const CF_DEFAULT: &str = "default";

// ============================================================================
// DERIVED VIEWS KEY PREFIXES (Week 23)
// ============================================================================

/// Canonical prefix for derived view records (Week 23 - D23.5).
/// Key: `derived_views/{view_id32}` -> DerivedView
///
/// Derived views are privacy-safe aggregates that AI entities can read
/// (with `read_nnpx_derived` capability) without accessing raw private data.
pub const KEY_PREFIX_DERIVED_VIEWS: &[u8] = b"derived_views/";

/// Canonical prefix for derived view audit log (Week 23 - D23.5).
/// Key: `derived_views/audit/{entity_id32}/{height_be8}` -> audit record
///
/// Records all AI entity reads of derived views for audit purposes.
pub const KEY_PREFIX_DERIVED_VIEWS_AUDIT: &[u8] = b"derived_views/audit/";

/// Canonical prefix for derived views indexed by schema (Week 23).
/// Key: `derived_views/by_schema/{schema_id_be4}/{view_id32}` -> empty (presence index)
pub const KEY_PREFIX_DERIVED_VIEWS_BY_SCHEMA: &[u8] = b"derived_views/by_schema/";

/// Canonical prefix for derived views indexed by creator (Week 23).
/// Key: `derived_views/by_creator/{creator32}/{view_id32}` -> empty (presence index)
pub const KEY_PREFIX_DERIVED_VIEWS_BY_CREATOR: &[u8] = b"derived_views/by_creator/";

// ============================================================================
// GOVERNANCE STORAGE KEY PREFIXES (Week 19)
// ============================================================================

/// Canonical prefix for governance proposal records.
/// Key: `proposals/{proposal_id32}` → Proposal (encoded via governance codec)
pub const KEY_PREFIX_GOVERNANCE_PROPOSALS: &[u8] = b"governance/proposals/";

/// Canonical prefix for governance audit log records.
/// Key: `governance_log/{proposal_id32}` → AuditLogEntry
pub const KEY_PREFIX_GOVERNANCE_LOG: &[u8] = b"governance/log/";

/// Canonical prefix for governance proposals indexed by state.
/// Key: `proposals_by_state/{state_u8}/{proposal_id32}` → empty (presence is the index)
pub const KEY_PREFIX_GOVERNANCE_PROPOSALS_BY_STATE: &[u8] = b"governance/proposals_by_state/";

/// Canonical prefix for approval gate records.
/// Key: `ai/gates/{gate_id32}` → ApprovalGate (encoded via gate codec)
pub const KEY_PREFIX_APPROVAL_GATES: &[u8] = b"ai/gates/";

/// AI emergency kill switch key.
/// Value: `0u8` = normal operation, `1u8` = all AI entity operations blocked.
pub const KEY_AI_KILL_SWITCH: &[u8] = b"ai/kill_switch";

/// Build the canonical key for an account record: `b"accounts/" ++ addr32`.
///
/// `addr` must be exactly 32 bytes.
pub fn account_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_ACCOUNTS.len() + addr.len());
    k.extend_from_slice(KEY_PREFIX_ACCOUNTS);
    k.extend_from_slice(addr);
    k
}

/// Build canonical SMT node key: `b"smt/node/" ++ node_hash32`.
pub fn smt_node_key(node_hash: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_SMT_NODE.len() + node_hash.len());
    k.extend_from_slice(KEY_PREFIX_SMT_NODE);
    k.extend_from_slice(node_hash);
    k
}

/// Build canonical block key: `b"consensus/blocks/" ++ height_u64_be`.
pub fn block_key(height: u64) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_BLOCKS.len() + 8);
    k.extend_from_slice(KEY_PREFIX_BLOCKS);
    k.extend_from_slice(&height.to_be_bytes());
    k
}

/// Build canonical QC key: `b"consensus/qcs/" ++ height_u64_be`.
pub fn qc_key(height: u64) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_QCS.len() + 8);
    k.extend_from_slice(KEY_PREFIX_QCS);
    k.extend_from_slice(&height.to_be_bytes());
    k
}

// ============================================================================
// AI KEY BUILDER FUNCTIONS (Retrofit Week 3)
// ============================================================================

/// Build canonical key for an AI entity: `b"ai/entities/" ++ entity_id32`.
pub fn ai_entity_key(entity_id: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_AI_ENTITIES.len() + 32);
    k.extend_from_slice(KEY_PREFIX_AI_ENTITIES);
    k.extend_from_slice(entity_id);
    k
}

/// Build canonical key for AI memory slot: `b"ai/memory/" ++ entity_id32 ++ "/" ++ slot`.
pub fn ai_memory_key(entity_id: &[u8; 32], slot: &[u8]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_AI_MEMORY.len() + 32 + 1 + slot.len());
    k.extend_from_slice(KEY_PREFIX_AI_MEMORY);
    k.extend_from_slice(entity_id);
    k.push(b'/');
    k.extend_from_slice(slot);
    k
}

/// Build canonical key for AI params: `b"ai/params/" ++ entity_id32 ++ "/" ++ param_name`.
pub fn ai_params_key(entity_id: &[u8; 32], param_name: &[u8]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_AI_PARAMS.len() + 32 + 1 + param_name.len());
    k.extend_from_slice(KEY_PREFIX_AI_PARAMS);
    k.extend_from_slice(entity_id);
    k.push(b'/');
    k.extend_from_slice(param_name);
    k
}

/// Build canonical key for AI signal: `b"ai/signals/" ++ height_be8 ++ "/" ++ issuer32`.
///
/// Uses big-endian height encoding so lexicographic ordering matches height ordering
/// for efficient range scans.
pub fn ai_signal_key(height: u64, issuer: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_AI_SIGNALS.len() + 8 + 1 + 32);
    k.extend_from_slice(KEY_PREFIX_AI_SIGNALS);
    k.extend_from_slice(&height.to_be_bytes());
    k.push(b'/');
    k.extend_from_slice(issuer);
    k
}

/// Build canonical key for AI signal index by type (Week 14 - D14.4):
/// `b"ai/signals/by_type/" ++ type_u8 ++ "/" ++ height_be8 ++ "/" ++ issuer32`.
///
/// Uses big-endian height so lexicographic ordering matches height ordering.
pub fn ai_signal_by_type_key(signal_type: u8, height: u64, issuer: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_AI_SIGNALS_BY_TYPE.len() + 1 + 1 + 8 + 1 + 32);
    k.extend_from_slice(KEY_PREFIX_AI_SIGNALS_BY_TYPE);
    k.push(signal_type);
    k.push(b'/');
    k.extend_from_slice(&height.to_be_bytes());
    k.push(b'/');
    k.extend_from_slice(issuer);
    k
}

/// Build canonical key for AI signal index by issuer (Week 14 - D14.4):
/// `b"ai/signals/by_issuer/" ++ issuer32 ++ "/" ++ height_be8`.
///
/// Uses big-endian height so lexicographic ordering matches height ordering.
pub fn ai_signal_by_issuer_key(issuer: &[u8; 32], height: u64) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_AI_SIGNALS_BY_ISSUER.len() + 32 + 1 + 8);
    k.extend_from_slice(KEY_PREFIX_AI_SIGNALS_BY_ISSUER);
    k.extend_from_slice(issuer);
    k.push(b'/');
    k.extend_from_slice(&height.to_be_bytes());
    k
}

// ============================================================================
// AI MEMORY OBJECT KEY BUILDER FUNCTIONS (Week 21)
// ============================================================================

/// Build canonical key for an AI memory object (Week 21 - D21.3):
/// `b"ai/memory_objects/" ++ entity_id32 ++ "/" ++ object_id32`.
pub fn ai_memory_object_key(entity_id: &[u8; 32], object_id: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_AI_MEMORY_OBJECTS.len() + 32 + 1 + 32);
    k.extend_from_slice(KEY_PREFIX_AI_MEMORY_OBJECTS);
    k.extend_from_slice(entity_id);
    k.push(b'/');
    k.extend_from_slice(object_id);
    k
}

/// Build canonical key for AI memory object count per entity (Week 21 - D21.3):
/// `b"ai/memory_count/" ++ entity_id32`.
///
/// Value is a u32 count encoded as big-endian 4 bytes.
pub fn ai_memory_count_key(entity_id: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_AI_MEMORY_COUNT.len() + 32);
    k.extend_from_slice(KEY_PREFIX_AI_MEMORY_COUNT);
    k.extend_from_slice(entity_id);
    k
}

/// Build canonical key for AI memory object index by type (Week 21):
/// `b"ai/memory_by_type/" ++ type_u8 ++ "/" ++ entity_id32 ++ "/" ++ object_id32`.
///
/// This is a presence-only index (value is empty) for efficient type queries.
pub fn ai_memory_by_type_key(
    object_type: u8,
    entity_id: &[u8; 32],
    object_id: &[u8; 32],
) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_AI_MEMORY_BY_TYPE.len() + 1 + 1 + 32 + 1 + 32);
    k.extend_from_slice(KEY_PREFIX_AI_MEMORY_BY_TYPE);
    k.push(object_type);
    k.push(b'/');
    k.extend_from_slice(entity_id);
    k.push(b'/');
    k.extend_from_slice(object_id);
    k
}

/// Encode a memory object count as big-endian bytes.
#[must_use]
pub fn encode_memory_count(count: u32) -> [u8; 4] {
    count.to_be_bytes()
}

/// Decode a memory object count from big-endian bytes.
///
/// Returns 0 if bytes are invalid length.
#[must_use]
pub fn decode_memory_count(bytes: &[u8]) -> u32 {
    if bytes.len() != 4 {
        return 0;
    }
    let mut arr = [0u8; 4];
    arr.copy_from_slice(bytes);
    u32::from_be_bytes(arr)
}

// ============================================================================
// GOVERNANCE KEY BUILDER FUNCTIONS (Week 19)
// ============================================================================

/// Build canonical key for a governance proposal:
/// `b"governance/proposals/" ++ proposal_id32`.
pub fn governance_proposal_key(proposal_id: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_GOVERNANCE_PROPOSALS.len() + 32);
    k.extend_from_slice(KEY_PREFIX_GOVERNANCE_PROPOSALS);
    k.extend_from_slice(proposal_id);
    k
}

/// Build canonical key for a governance audit log entry:
/// `b"governance/log/" ++ proposal_id32`.
pub fn governance_log_key(proposal_id: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_GOVERNANCE_LOG.len() + 32);
    k.extend_from_slice(KEY_PREFIX_GOVERNANCE_LOG);
    k.extend_from_slice(proposal_id);
    k
}

/// Build canonical key for governance proposal-by-state index:
/// `b"governance/proposals_by_state/" ++ state_u8 ++ "/" ++ proposal_id32`.
///
/// The state byte comes first to enable efficient range scans of all proposals
/// in a given state.
pub fn governance_proposal_by_state_key(state: u8, proposal_id: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_GOVERNANCE_PROPOSALS_BY_STATE.len() + 1 + 1 + 32);
    k.extend_from_slice(KEY_PREFIX_GOVERNANCE_PROPOSALS_BY_STATE);
    k.push(state);
    k.push(b'/');
    k.extend_from_slice(proposal_id);
    k
}

/// Build canonical key for an approval gate: `b"ai/gates/" ++ gate_id32`.
pub fn approval_gate_key(gate_id: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_APPROVAL_GATES.len() + 32);
    k.extend_from_slice(KEY_PREFIX_APPROVAL_GATES);
    k.extend_from_slice(gate_id);
    k
}

// ============================================================================
// NNPX KEY BUILDER FUNCTIONS (Week 22)
// ============================================================================

/// Build canonical key for an NNPX commitment (Week 22 - D22.2):
/// `b"nnpx/commitments/" ++ commitment_hash32`.
pub fn nnpx_commitment_key(commitment_hash: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_NNPX_COMMITMENTS.len() + 32);
    k.extend_from_slice(KEY_PREFIX_NNPX_COMMITMENTS);
    k.extend_from_slice(commitment_hash);
    k
}

/// Build canonical key for an NNPX nullifier (Week 22 - D22.2):
/// `b"nnpx/nullifiers/" ++ nullifier32`.
pub fn nnpx_nullifier_key(nullifier: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_NNPX_NULLIFIERS.len() + 32);
    k.extend_from_slice(KEY_PREFIX_NNPX_NULLIFIERS);
    k.extend_from_slice(nullifier);
    k
}

/// Build canonical key for an NNPX encrypted payload (Week 22 - D22.2):
/// `b"nnpx/encrypted/" ++ commitment_hash32`.
pub fn nnpx_encrypted_key(commitment_hash: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_NNPX_ENCRYPTED.len() + 32);
    k.extend_from_slice(KEY_PREFIX_NNPX_ENCRYPTED);
    k.extend_from_slice(commitment_hash);
    k
}

/// Check if a key belongs to the NNPX private store.
///
/// Keys starting with `b"nnpx/"` are routed to the private column family.
#[inline]
#[must_use]
pub fn is_nnpx_key(key: &[u8]) -> bool {
    key.starts_with(KEY_PREFIX_NNPX)
}

// ============================================================================
// DERIVED VIEWS KEY BUILDER FUNCTIONS (Week 23)
// ============================================================================

/// Build canonical key for a derived view (Week 23 - D23.5):
/// `b"derived_views/" ++ view_id32`.
pub fn derived_view_key(view_id: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_DERIVED_VIEWS.len() + 32);
    k.extend_from_slice(KEY_PREFIX_DERIVED_VIEWS);
    k.extend_from_slice(view_id);
    k
}

/// Build canonical key for a derived view audit log entry (Week 23 - D23.5):
/// `b"derived_views/audit/" ++ entity_id32 ++ "/" ++ height_be8`.
///
/// Uses big-endian height for lexicographic ordering in range scans.
pub fn derived_view_audit_key(entity_id: &[u8; 32], height: u64) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_DERIVED_VIEWS_AUDIT.len() + 32 + 1 + 8);
    k.extend_from_slice(KEY_PREFIX_DERIVED_VIEWS_AUDIT);
    k.extend_from_slice(entity_id);
    k.push(b'/');
    k.extend_from_slice(&height.to_be_bytes());
    k
}

/// Build canonical key for derived view index by schema (Week 23):
/// `b"derived_views/by_schema/" ++ schema_id_be4 ++ "/" ++ view_id32`.
pub fn derived_view_by_schema_key(schema_id: u32, view_id: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_DERIVED_VIEWS_BY_SCHEMA.len() + 4 + 1 + 32);
    k.extend_from_slice(KEY_PREFIX_DERIVED_VIEWS_BY_SCHEMA);
    k.extend_from_slice(&schema_id.to_be_bytes());
    k.push(b'/');
    k.extend_from_slice(view_id);
    k
}

/// Build canonical key for derived view index by creator (Week 23):
/// `b"derived_views/by_creator/" ++ creator32 ++ "/" ++ view_id32`.
pub fn derived_view_by_creator_key(creator: &[u8; 32], view_id: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::with_capacity(KEY_PREFIX_DERIVED_VIEWS_BY_CREATOR.len() + 32 + 1 + 32);
    k.extend_from_slice(KEY_PREFIX_DERIVED_VIEWS_BY_CREATOR);
    k.extend_from_slice(creator);
    k.push(b'/');
    k.extend_from_slice(view_id);
    k
}

/// Check if a key belongs to the derived views store.
///
/// Keys starting with `b"derived_views/"` are derived view records.
/// AI entities with `read_nnpx_derived` capability can access these.
#[inline]
#[must_use]
pub fn is_derived_view_key(key: &[u8]) -> bool {
    key.starts_with(KEY_PREFIX_DERIVED_VIEWS)
}

/// Canonical mapping from variable-length state DB keys to 32-byte SMT keys.
///
/// # Consensus-critical
///
/// State keys are variable-length (e.g. `b"accounts/<addr32>"`, `b"fee_pool"`),
/// but the Sparse Merkle Tree uses fixed 256-bit keys.
///
/// **Rule:** `smt_key = blake3(state_key_bytes)`
///
/// This function is the single source of truth for that mapping.
/// Do not change its behavior without a network upgrade.
pub fn smt_key_for_state_key(key: &[u8]) -> [u8; 32] {
    blake3::hash(key).into()
}

/// A write operation for atomic batching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOp {
    Put(Vec<u8>, Vec<u8>),
    Delete(Vec<u8>),
}

/// Minimal KV interface for state storage (RocksDB-backed impl later).
#[allow(clippy::type_complexity)]
pub trait Kv {
    type Error;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error>;
    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Self::Error>;
    fn delete(&mut self, key: &[u8]) -> Result<(), Self::Error>;

    /// Scan all keys with given prefix, returning (key, value) pairs.
    ///
    /// Results MUST be ordered lexicographically by key for determinism.
    /// This is required for consensus-safe range queries (Week 14 - D14.5).
    fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, Self::Error>;
}

/// Extended KV trait with atomic batch support.
///
/// Implementations must guarantee that either all operations succeed
/// or none take effect (all-or-nothing semantics).
pub trait KvBatch: Kv {
    /// Apply multiple operations atomically (all-or-nothing).
    fn apply_batch(&mut self, ops: &[WriteOp]) -> Result<(), Self::Error>;
}

pub mod memkv;
pub use memkv::MemKv;

/// Optional RocksDB-backed KV store.
#[cfg(feature = "rocksdb")]
pub mod rocksdb_kv;

#[cfg(feature = "rocksdb")]
pub use rocksdb_kv::RocksKv;

/// State encoding version for AccountStateV1 and FeePoolV1.
pub const STATE_CODEC_V1: u8 = 1;

/// SMT root encoding version.
pub const SMT_ROOT_CODEC_V1: u8 = 1;

/// Encode SMT root as canonical bytes:
/// [version:1][root32]
pub fn encode_smt_root_v1(root: &[u8; 32]) -> [u8; 1 + 32] {
    let mut out = [0u8; 1 + 32];
    out[0] = SMT_ROOT_CODEC_V1;
    out[1..33].copy_from_slice(root);
    out
}

/// Decode SMT root from canonical bytes.
pub fn decode_smt_root_v1(bytes: &[u8]) -> Result<[u8; 32], StateDecodeError> {
    if bytes.len() != 1 + 32 {
        return Err(StateDecodeError::BadLength {
            expected: 33,
            got: bytes.len(),
        });
    }
    if bytes[0] != SMT_ROOT_CODEC_V1 {
        return Err(StateDecodeError::BadVersion {
            expected: SMT_ROOT_CODEC_V1,
            got: bytes[0],
        });
    }
    let mut root = [0u8; 32];
    root.copy_from_slice(&bytes[1..33]);
    Ok(root)
}

/// Account state record (Week 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStateV1 {
    pub balance: u128,
    pub nonce: u64,
}

/// Fee pool state record (Week 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeePoolV1 {
    pub balance: u128,
}

/// Encode AccountStateV1 as canonical bytes:
/// [version:1][balance_be:16][nonce_be:8]
pub fn encode_account_v1(a: &AccountStateV1) -> [u8; 1 + 16 + 8] {
    let mut out = [0u8; 1 + 16 + 8];
    out[0] = STATE_CODEC_V1;
    out[1..17].copy_from_slice(&a.balance.to_be_bytes());
    out[17..25].copy_from_slice(&a.nonce.to_be_bytes());
    out
}

/// Decode AccountStateV1 from canonical bytes.
pub fn decode_account_v1(bytes: &[u8]) -> Result<AccountStateV1, StateDecodeError> {
    if bytes.len() != 1 + 16 + 8 {
        return Err(StateDecodeError::BadLength {
            expected: 25,
            got: bytes.len(),
        });
    }
    if bytes[0] != STATE_CODEC_V1 {
        return Err(StateDecodeError::BadVersion {
            expected: STATE_CODEC_V1,
            got: bytes[0],
        });
    }
    let mut bal = [0u8; 16];
    bal.copy_from_slice(&bytes[1..17]);
    let mut nonce = [0u8; 8];
    nonce.copy_from_slice(&bytes[17..25]);

    Ok(AccountStateV1 {
        balance: u128::from_be_bytes(bal),
        nonce: u64::from_be_bytes(nonce),
    })
}

/// Encode FeePoolV1 as canonical bytes:
/// [version:1][balance_be:16]
pub fn encode_fee_pool_v1(p: &FeePoolV1) -> [u8; 1 + 16] {
    let mut out = [0u8; 1 + 16];
    out[0] = STATE_CODEC_V1;
    out[1..17].copy_from_slice(&p.balance.to_be_bytes());
    out
}

/// Decode FeePoolV1 from canonical bytes.
pub fn decode_fee_pool_v1(bytes: &[u8]) -> Result<FeePoolV1, StateDecodeError> {
    if bytes.len() != 1 + 16 {
        return Err(StateDecodeError::BadLength {
            expected: 17,
            got: bytes.len(),
        });
    }
    if bytes[0] != STATE_CODEC_V1 {
        return Err(StateDecodeError::BadVersion {
            expected: STATE_CODEC_V1,
            got: bytes[0],
        });
    }
    let mut bal = [0u8; 16];
    bal.copy_from_slice(&bytes[1..17]);
    Ok(FeePoolV1 {
        balance: u128::from_be_bytes(bal),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateDecodeError {
    BadLength { expected: usize, got: usize },
    BadVersion { expected: u8, got: u8 },
}
#[cfg(test)]
mod tests {
    use super::*;
    use novai_smt::{MemoryStore, Smt};

    #[test]
    fn test_ai_keys_produce_valid_smt_keys() {
        // Task 4.1: Verify AI entity keys produce valid 32-byte SMT keys
        let entity_id = [42u8; 32];
        let ai_key = ai_entity_key(&entity_id);
        let smt_key = smt_key_for_state_key(&ai_key);

        // Must be exactly 32 bytes
        assert_eq!(smt_key.len(), 32);

        // Should be deterministic
        let smt_key2 = smt_key_for_state_key(&ai_key);
        assert_eq!(smt_key, smt_key2);
    }

    #[test]
    fn test_different_entity_ids_produce_different_keys() {
        // Task 4.1: Collision test - different IDs must produce different SMT keys
        let id1 = [1u8; 32];
        let id2 = [2u8; 32];

        let ai_key1 = ai_entity_key(&id1);
        let ai_key2 = ai_entity_key(&id2);

        let smt_key1 = smt_key_for_state_key(&ai_key1);
        let smt_key2 = smt_key_for_state_key(&ai_key2);

        assert_ne!(
            smt_key1, smt_key2,
            "Different entity IDs must produce different SMT keys"
        );
    }

    #[test]
    fn test_ai_entity_changes_state_root() {
        // Task 4.2: Prove AI entities are included in authenticated state
        let store = MemoryStore::default();
        let mut smt = Smt::new(store);

        // Get root of empty tree
        let root1 = smt.root();

        // Write an AI entity to SMT
        let entity_id = [99u8; 32];
        let ai_key = ai_entity_key(&entity_id);
        let smt_key = smt_key_for_state_key(&ai_key);

        // Simple value representing serialized AI entity
        let ai_value = vec![1, 2, 3, 4];
        let root2 = smt
            .update(smt_key, &ai_value)
            .expect("update should succeed");

        // Roots must be different - proves AI entities affect authenticated state
        assert_ne!(root1, root2, "AI entity insertion must change state root");
    }

    #[test]
    fn test_memory_slot_keys_are_valid() {
        // Task 4.1: Verify memory slot keys also work with SMT
        let entity_id = [77u8; 32];
        let slot1 = b"slot_5";
        let slot2 = b"slot_6";

        let memory_key1 = ai_memory_key(&entity_id, slot1);
        let smt_key1 = smt_key_for_state_key(&memory_key1);

        // Must be exactly 32 bytes
        assert_eq!(smt_key1.len(), 32);

        // Different slots should produce different keys
        let memory_key2 = ai_memory_key(&entity_id, slot2);
        let smt_key2 = smt_key_for_state_key(&memory_key2);
        assert_ne!(smt_key1, smt_key2);
    }
    #[test]
    fn test_golden_ai_inclusive_root() {
        // Task 4.3: Golden vector proving AI entities are in authenticated state
        // This test is deterministic and locks the root computation

        let store = MemoryStore::default();
        let mut smt = Smt::new(store);

        // Step 1: Add one account (user account state)
        let account_addr = [0x11u8; 32];
        let account_key = account_key(&account_addr);
        let smt_account_key = smt_key_for_state_key(&account_key);
        let account_value = b"balance:1000";
        smt.update(smt_account_key, account_value)
            .expect("account insert");

        // Step 2: Add one AI entity
        let entity_id = [0x42u8; 32];
        let ai_key = ai_entity_key(&entity_id);
        let smt_ai_key = smt_key_for_state_key(&ai_key);
        let ai_value = b"entity:advisor:v1";
        smt.update(smt_ai_key, ai_value).expect("ai entity insert");

        // Step 3: Get final root
        let final_root = smt.root();

        // Golden vector (this will be computed on first run, then locked)
        // To regenerate: run test, copy the printed root, paste here
        const EXPECTED_ROOT: [u8; 32] = [
            0x09, 0x49, 0x67, 0xb1, 0x8d, 0x07, 0xc6, 0xa7, 0xb7, 0x74, 0x1f, 0xe0, 0x4d, 0x2a,
            0x2e, 0x54, 0x21, 0xa3, 0xde, 0xb7, 0x55, 0x26, 0x1b, 0x6a, 0x73, 0x83, 0x73, 0xcc,
            0xd4, 0xb7, 0x1a, 0xe8,
        ];

        // On first run, print the actual root so we can lock it
        if final_root != EXPECTED_ROOT {
            eprintln!("GOLDEN VECTOR UPDATE NEEDED:");
            eprintln!("const EXPECTED_ROOT: [u8; 32] = [");
            for chunk in final_root.chunks(8) {
                eprint!("    ");
                for b in chunk {
                    eprint!("0x{b:02x}, ");
                }
                eprintln!();
            }
            eprintln!("];");
            panic!("Golden vector mismatch - see stderr for new value");
        }

        assert_eq!(
            final_root, EXPECTED_ROOT,
            "AI-inclusive root must match golden vector"
        );
    }

    // ========================================================================
    // AI MEMORY OBJECT KEY TESTS (Week 21)
    // ========================================================================

    #[test]
    fn test_memory_object_key_format() {
        let entity_id = [0x42u8; 32];
        let object_id = [0xAAu8; 32];

        let key = ai_memory_object_key(&entity_id, &object_id);

        // Check prefix
        assert!(key.starts_with(KEY_PREFIX_AI_MEMORY_OBJECTS));

        // Check length: prefix + 32 + 1 (/) + 32
        assert_eq!(key.len(), KEY_PREFIX_AI_MEMORY_OBJECTS.len() + 32 + 1 + 32);
    }

    #[test]
    fn test_memory_object_key_uniqueness() {
        let entity1 = [0x01u8; 32];
        let entity2 = [0x02u8; 32];
        let object1 = [0xAAu8; 32];
        let object2 = [0xBBu8; 32];

        let key_e1_o1 = ai_memory_object_key(&entity1, &object1);
        let key_e1_o2 = ai_memory_object_key(&entity1, &object2);
        let key_e2_o1 = ai_memory_object_key(&entity2, &object1);

        assert_ne!(
            key_e1_o1, key_e1_o2,
            "Different objects must have different keys"
        );
        assert_ne!(
            key_e1_o1, key_e2_o1,
            "Different entities must have different keys"
        );
    }

    #[test]
    fn test_memory_count_key_format() {
        let entity_id = [0x42u8; 32];

        let key = ai_memory_count_key(&entity_id);

        // Check prefix
        assert!(key.starts_with(KEY_PREFIX_AI_MEMORY_COUNT));

        // Check length: prefix + 32
        assert_eq!(key.len(), KEY_PREFIX_AI_MEMORY_COUNT.len() + 32);
    }

    #[test]
    fn test_memory_count_encode_decode_roundtrip() {
        for count in [0u32, 1, 50, 100, 1000, u32::MAX] {
            let encoded = encode_memory_count(count);
            let decoded = decode_memory_count(&encoded);
            assert_eq!(count, decoded, "Count {count} roundtrip failed");
        }
    }

    #[test]
    fn test_memory_count_decode_invalid_length() {
        // Too short
        assert_eq!(decode_memory_count(&[1, 2, 3]), 0);
        // Too long
        assert_eq!(decode_memory_count(&[1, 2, 3, 4, 5]), 0);
        // Empty
        assert_eq!(decode_memory_count(&[]), 0);
    }

    #[test]
    fn test_memory_by_type_key_format() {
        let entity_id = [0x42u8; 32];
        let object_id = [0xAAu8; 32];
        let object_type = 2u8; // EmbeddingCommitment

        let key = ai_memory_by_type_key(object_type, &entity_id, &object_id);

        // Check prefix
        assert!(key.starts_with(KEY_PREFIX_AI_MEMORY_BY_TYPE));

        // Check type byte is in correct position
        assert_eq!(key[KEY_PREFIX_AI_MEMORY_BY_TYPE.len()], object_type);

        // Check length: prefix + 1 (type) + 1 (/) + 32 + 1 (/) + 32
        assert_eq!(
            key.len(),
            KEY_PREFIX_AI_MEMORY_BY_TYPE.len() + 1 + 1 + 32 + 1 + 32
        );
    }

    #[test]
    fn test_memory_by_type_key_ordering() {
        let entity_id = [0x42u8; 32];
        let object_id = [0xAAu8; 32];

        // Keys for different types should be ordered by type byte
        let key_type0 = ai_memory_by_type_key(0, &entity_id, &object_id);
        let key_type1 = ai_memory_by_type_key(1, &entity_id, &object_id);
        let key_type4 = ai_memory_by_type_key(4, &entity_id, &object_id);

        assert!(key_type0 < key_type1, "Type 0 key must be < type 1 key");
        assert!(key_type1 < key_type4, "Type 1 key must be < type 4 key");
    }

    #[test]
    fn test_memory_object_keys_produce_valid_smt_keys() {
        let entity_id = [0x42u8; 32];
        let object_id = [0xAAu8; 32];

        let key = ai_memory_object_key(&entity_id, &object_id);
        let smt_key = smt_key_for_state_key(&key);

        // Must be exactly 32 bytes
        assert_eq!(smt_key.len(), 32);

        // Should be deterministic
        let smt_key2 = smt_key_for_state_key(&key);
        assert_eq!(smt_key, smt_key2);
    }

    // ========================================================================
    // NNPX PRIVACY KEY TESTS (Week 22)
    // ========================================================================

    #[test]
    fn test_nnpx_commitment_key_format() {
        let commitment_hash = [0xABu8; 32];

        let key = nnpx_commitment_key(&commitment_hash);

        // Check prefix
        assert!(key.starts_with(KEY_PREFIX_NNPX_COMMITMENTS));

        // Check length: prefix + 32
        assert_eq!(key.len(), KEY_PREFIX_NNPX_COMMITMENTS.len() + 32);

        // Verify it's an NNPX key
        assert!(is_nnpx_key(&key));
    }

    #[test]
    fn test_nnpx_nullifier_key_format() {
        let nullifier = [0xCDu8; 32];

        let key = nnpx_nullifier_key(&nullifier);

        // Check prefix
        assert!(key.starts_with(KEY_PREFIX_NNPX_NULLIFIERS));

        // Check length: prefix + 32
        assert_eq!(key.len(), KEY_PREFIX_NNPX_NULLIFIERS.len() + 32);

        // Verify it's an NNPX key
        assert!(is_nnpx_key(&key));
    }

    #[test]
    fn test_nnpx_encrypted_key_format() {
        let commitment_hash = [0xEFu8; 32];

        let key = nnpx_encrypted_key(&commitment_hash);

        // Check prefix
        assert!(key.starts_with(KEY_PREFIX_NNPX_ENCRYPTED));

        // Check length: prefix + 32
        assert_eq!(key.len(), KEY_PREFIX_NNPX_ENCRYPTED.len() + 32);

        // Verify it's an NNPX key
        assert!(is_nnpx_key(&key));
    }

    #[test]
    fn test_is_nnpx_key() {
        // NNPX keys
        assert!(is_nnpx_key(b"nnpx/"));
        assert!(is_nnpx_key(b"nnpx/commitments/"));
        assert!(is_nnpx_key(b"nnpx/nullifiers/abc"));
        assert!(is_nnpx_key(b"nnpx/encrypted/xyz"));

        // Non-NNPX keys
        assert!(!is_nnpx_key(b"accounts/"));
        assert!(!is_nnpx_key(b"ai/entities/"));
        assert!(!is_nnpx_key(b"consensus/"));
        assert!(!is_nnpx_key(b"governance/"));
        assert!(!is_nnpx_key(b"smt/"));

        // Edge cases
        assert!(!is_nnpx_key(b"nnpx")); // Missing trailing slash
        assert!(!is_nnpx_key(b"NNPX/")); // Case sensitive
        assert!(!is_nnpx_key(b"")); // Empty
    }

    #[test]
    fn test_nnpx_key_uniqueness() {
        let hash1 = [0x01u8; 32];
        let hash2 = [0x02u8; 32];

        let key1 = nnpx_commitment_key(&hash1);
        let key2 = nnpx_commitment_key(&hash2);
        let key3 = nnpx_nullifier_key(&hash1);
        let key4 = nnpx_encrypted_key(&hash1);

        // Different hashes produce different keys
        assert_ne!(key1, key2);

        // Different key types produce different keys even with same hash
        assert_ne!(key1, key3);
        assert_ne!(key1, key4);
        assert_ne!(key3, key4);
    }

    #[test]
    fn test_memkv_nnpx_column_family_isolation() {
        let mut db = MemKv::new();

        // Write to both column families
        let public_key = b"accounts/alice";
        let private_key = b"nnpx/commitments/abc";

        db.put(public_key, b"public_value").unwrap();
        db.put(private_key, b"private_value").unwrap();

        // Read back
        assert_eq!(db.get(public_key).unwrap(), Some(b"public_value".to_vec()));
        assert_eq!(
            db.get(private_key).unwrap(),
            Some(b"private_value".to_vec())
        );

        // Scan prefix should only return keys from the correct CF
        let public_results = db.scan_prefix(b"accounts/").unwrap();
        assert_eq!(public_results.len(), 1);
        assert!(public_results[0].0.starts_with(b"accounts/"));

        let private_results = db.scan_prefix(b"nnpx/").unwrap();
        assert_eq!(private_results.len(), 1);
        assert!(private_results[0].0.starts_with(b"nnpx/"));
    }

    #[test]
    fn test_memkv_batch_with_nnpx_keys() {
        let mut db = MemKv::new();

        // Apply batch with mixed public and private keys
        let ops = vec![
            WriteOp::Put(b"accounts/bob".to_vec(), b"balance:100".to_vec()),
            WriteOp::Put(b"nnpx/nullifiers/null1".to_vec(), b"".to_vec()),
            WriteOp::Put(b"ai/entities/entity1".to_vec(), b"entity_data".to_vec()),
            WriteOp::Put(b"nnpx/commitments/commit1".to_vec(), b"commitment".to_vec()),
        ];

        db.apply_batch(&ops).unwrap();

        // Verify all keys are accessible
        assert!(db.get(b"accounts/bob").unwrap().is_some());
        assert!(db.get(b"nnpx/nullifiers/null1").unwrap().is_some());
        assert!(db.get(b"ai/entities/entity1").unwrap().is_some());
        assert!(db.get(b"nnpx/commitments/commit1").unwrap().is_some());

        // Delete from both CFs
        let delete_ops = vec![
            WriteOp::Delete(b"accounts/bob".to_vec()),
            WriteOp::Delete(b"nnpx/nullifiers/null1".to_vec()),
        ];

        db.apply_batch(&delete_ops).unwrap();

        // Verify deletions
        assert!(db.get(b"accounts/bob").unwrap().is_none());
        assert!(db.get(b"nnpx/nullifiers/null1").unwrap().is_none());

        // Others still exist
        assert!(db.get(b"ai/entities/entity1").unwrap().is_some());
        assert!(db.get(b"nnpx/commitments/commit1").unwrap().is_some());
    }

    // ========================================================================
    // DERIVED VIEWS KEY TESTS (Week 23)
    // ========================================================================

    #[test]
    fn test_derived_view_key_format() {
        let view_id = [0xABu8; 32];

        let key = derived_view_key(&view_id);

        // Check prefix
        assert!(key.starts_with(KEY_PREFIX_DERIVED_VIEWS));

        // Check length: prefix + 32
        assert_eq!(key.len(), KEY_PREFIX_DERIVED_VIEWS.len() + 32);

        // Verify it's a derived view key
        assert!(is_derived_view_key(&key));
    }

    #[test]
    fn test_derived_view_audit_key_format() {
        let entity_id = [0x42u8; 32];
        let height = 12345u64;

        let key = derived_view_audit_key(&entity_id, height);

        // Check prefix
        assert!(key.starts_with(KEY_PREFIX_DERIVED_VIEWS_AUDIT));

        // Check length: prefix + 32 + 1 (/) + 8
        assert_eq!(key.len(), KEY_PREFIX_DERIVED_VIEWS_AUDIT.len() + 32 + 1 + 8);

        // Verify it's a derived view key
        assert!(is_derived_view_key(&key));
    }

    #[test]
    fn test_derived_view_audit_key_ordering() {
        let entity_id = [0x42u8; 32];

        // Keys for different heights must be lexicographically ordered
        let key_100 = derived_view_audit_key(&entity_id, 100);
        let key_200 = derived_view_audit_key(&entity_id, 200);
        let key_max = derived_view_audit_key(&entity_id, u64::MAX);

        assert!(key_100 < key_200, "Height 100 key must be < height 200 key");
        assert!(key_200 < key_max, "Height 200 key must be < max height key");
    }

    #[test]
    fn test_derived_view_by_schema_key_format() {
        let schema_id = 1u32;
        let view_id = [0xCDu8; 32];

        let key = derived_view_by_schema_key(schema_id, &view_id);

        // Check prefix
        assert!(key.starts_with(KEY_PREFIX_DERIVED_VIEWS_BY_SCHEMA));

        // Check length: prefix + 4 + 1 (/) + 32
        assert_eq!(
            key.len(),
            KEY_PREFIX_DERIVED_VIEWS_BY_SCHEMA.len() + 4 + 1 + 32
        );

        // Verify it's a derived view key
        assert!(is_derived_view_key(&key));
    }

    #[test]
    fn test_derived_view_by_schema_key_ordering() {
        let view_id = [0xCDu8; 32];

        // Keys for different schemas must be lexicographically ordered
        let key_schema1 = derived_view_by_schema_key(1, &view_id);
        let key_schema2 = derived_view_by_schema_key(2, &view_id);
        let key_schema3 = derived_view_by_schema_key(3, &view_id);

        assert!(
            key_schema1 < key_schema2,
            "Schema 1 key must be < schema 2 key"
        );
        assert!(
            key_schema2 < key_schema3,
            "Schema 2 key must be < schema 3 key"
        );
    }

    #[test]
    fn test_derived_view_by_creator_key_format() {
        let creator = [0xEFu8; 32];
        let view_id = [0x12u8; 32];

        let key = derived_view_by_creator_key(&creator, &view_id);

        // Check prefix
        assert!(key.starts_with(KEY_PREFIX_DERIVED_VIEWS_BY_CREATOR));

        // Check length: prefix + 32 + 1 (/) + 32
        assert_eq!(
            key.len(),
            KEY_PREFIX_DERIVED_VIEWS_BY_CREATOR.len() + 32 + 1 + 32
        );

        // Verify it's a derived view key
        assert!(is_derived_view_key(&key));
    }

    #[test]
    fn test_is_derived_view_key() {
        // Derived view keys
        assert!(is_derived_view_key(b"derived_views/"));
        assert!(is_derived_view_key(b"derived_views/abc123"));
        assert!(is_derived_view_key(b"derived_views/audit/entity/100"));
        assert!(is_derived_view_key(b"derived_views/by_schema/1/view"));
        assert!(is_derived_view_key(b"derived_views/by_creator/addr/view"));

        // Non-derived-view keys
        assert!(!is_derived_view_key(b"accounts/"));
        assert!(!is_derived_view_key(b"ai/entities/"));
        assert!(!is_derived_view_key(b"nnpx/"));
        assert!(!is_derived_view_key(b"governance/"));

        // Edge cases
        assert!(!is_derived_view_key(b"derived_views")); // Missing trailing slash
        assert!(!is_derived_view_key(b"DERIVED_VIEWS/")); // Case sensitive
        assert!(!is_derived_view_key(b"")); // Empty
    }

    #[test]
    fn test_derived_view_key_uniqueness() {
        let view_id1 = [0x01u8; 32];
        let view_id2 = [0x02u8; 32];
        let creator = [0x03u8; 32];

        let key1 = derived_view_key(&view_id1);
        let key2 = derived_view_key(&view_id2);
        let key3 = derived_view_by_creator_key(&creator, &view_id1);
        let key4 = derived_view_by_schema_key(1, &view_id1);

        // Different view IDs produce different keys
        assert_ne!(key1, key2);

        // Different key types produce different keys even with same view ID
        assert_ne!(key1, key3);
        assert_ne!(key1, key4);
        assert_ne!(key3, key4);
    }

    #[test]
    fn test_derived_view_keys_produce_valid_smt_keys() {
        let view_id = [0xABu8; 32];

        let key = derived_view_key(&view_id);
        let smt_key = smt_key_for_state_key(&key);

        // Must be exactly 32 bytes
        assert_eq!(smt_key.len(), 32);

        // Should be deterministic
        let smt_key2 = smt_key_for_state_key(&key);
        assert_eq!(smt_key, smt_key2);
    }

    #[test]
    fn test_derived_view_not_nnpx_key() {
        // Derived view keys are NOT NNPX keys
        // This is important: AI can read derived_views/ but not nnpx/
        let view_id = [0xABu8; 32];
        let key = derived_view_key(&view_id);

        assert!(is_derived_view_key(&key), "Should be a derived view key");
        assert!(!is_nnpx_key(&key), "Should NOT be an NNPX key");
    }
}
