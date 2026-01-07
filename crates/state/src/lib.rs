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
pub trait Kv {
    type Error;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error>;
    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), Self::Error>;
    fn delete(&mut self, key: &[u8]) -> Result<(), Self::Error>;
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
