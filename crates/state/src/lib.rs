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
                    eprint!("0x{:02x}, ", b);
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
}
