//! novai-execution
//!
//! Week 3 scope: deterministic state transition for account model.
//!
//! Invariants (week 3):
//! - No floats.
//! - No iteration over HashMap/HashSet in consensus-critical ordering.
//! - All arithmetic is checked (overflow/underflow -> error).
//! - Transfer payload is canonical and versioned.
//!
//! Failure modes:
//! - Bad payload encoding -> reject deterministically.
//! - Nonce mismatch -> reject deterministically.
//! - Insufficient funds -> reject deterministically.
//! - Any overflow -> reject deterministically.

use novai_state::{
    account_key, decode_account_v1, decode_fee_pool_v1, decode_smt_root_v1, encode_account_v1,
    encode_fee_pool_v1, encode_smt_root_v1, smt_key_for_state_key, smt_node_key, AccountStateV1,
    FeePoolV1, Kv, KvBatch, StateDecodeError, WriteOp, KEY_FEE_POOL, KEY_SMT_ROOT,
};

use novai_smt::hash::{empty_hash_at_height, Hash32};
use novai_smt::node::Node;
use novai_smt::smt::{Smt, SmtError, SmtStore};
use novai_types::{Address, Nonce, TxV1};

pub const EXECUTION_VERSION: u8 = 1;

/// Transfer payload version.
pub const TRANSFER_PAYLOAD_V1: u8 = 1;

/// Canonical Transfer payload:
/// `[version:1][to:32][amount_be:8]`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPayloadV1 {
    pub to: Address,
    pub amount: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError<E> {
    Db(E),
    Decode(StateDecodeError),
    BadPayloadLength { expected: usize, got: usize },
    BadPayloadVersion { expected: u8, got: u8 },
    NonceMismatch { expected: Nonce, got: Nonce },
    InsufficientFunds { balance: u128, needed: u128 },
    Overflow,
    NonceOverflow,
}

impl<E> From<StateDecodeError> for ExecError<E> {
    fn from(e: StateDecodeError) -> Self {
        Self::Decode(e)
    }
}

/// Deterministically decode a transfer payload from `tx.payload`.
///
/// # Errors
/// Returns error if payload length or version is invalid.
pub fn decode_transfer_payload_v1(payload: &[u8]) -> Result<TransferPayloadV1, ExecError<()>> {
    const LEN: usize = 1 + 32 + 8;
    if payload.len() != LEN {
        return Err(ExecError::BadPayloadLength {
            expected: LEN,
            got: payload.len(),
        });
    }
    let ver = payload[0];
    if ver != TRANSFER_PAYLOAD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: TRANSFER_PAYLOAD_V1,
            got: ver,
        });
    }
    let mut to = [0u8; 32];
    to.copy_from_slice(&payload[1..33]);
    let mut amt = [0u8; 8];
    amt.copy_from_slice(&payload[33..41]);
    Ok(TransferPayloadV1 {
        to,
        amount: u64::from_be_bytes(amt),
    })
}

/// Deterministically encode a transfer payload.
#[must_use]
pub fn encode_transfer_payload_v1(p: &TransferPayloadV1) -> [u8; 1 + 32 + 8] {
    let mut out = [0u8; 1 + 32 + 8];
    out[0] = TRANSFER_PAYLOAD_V1;
    out[1..33].copy_from_slice(&p.to);
    out[33..41].copy_from_slice(&p.amount.to_be_bytes());
    out
}

fn u64_to_u128_checked(x: u64) -> u128 {
    u128::from(x)
}

fn read_account_or_default<K: Kv>(
    db: &K,
    addr: &Address,
) -> Result<AccountStateV1, ExecError<K::Error>> {
    let k = account_key(addr);
    match db.get(&k).map_err(ExecError::Db)? {
        None => Ok(AccountStateV1 {
            balance: 0,
            nonce: 0,
        }),
        Some(bytes) => Ok(decode_account_v1(&bytes)?),
    }
}

fn read_fee_pool_or_default<K: Kv>(db: &K) -> Result<FeePoolV1, ExecError<K::Error>> {
    match db.get(KEY_FEE_POOL).map_err(ExecError::Db)? {
        None => Ok(FeePoolV1 { balance: 0 }),
        Some(bytes) => Ok(decode_fee_pool_v1(&bytes)?),
    }
}

fn read_smt_root_or_default<K: Kv>(db: &K) -> Result<Hash32, ExecError<K::Error>> {
    match db.get(KEY_SMT_ROOT).map_err(ExecError::Db)? {
        None => Ok(empty_hash_at_height(256)),
        Some(bytes) => Ok(decode_smt_root_v1(&bytes)?),
    }
}

/// Store adapter: reads existing nodes from Kv, buffers writes as `WriteOp::Put`.
/// Deterministic: uses Vec + linear search (no `HashMap`).
struct SmtOverlayStore<'a, K: Kv> {
    db: &'a K,
    pending: Vec<(Vec<u8>, Vec<u8>)>, // (db_key, value_bytes)
}

impl<'a, K: Kv> SmtOverlayStore<'a, K> {
    const fn new(db: &'a K) -> Self {
        Self {
            db,
            pending: Vec::new(),
        }
    }

    fn into_write_ops(mut self) -> Vec<WriteOp> {
        // Sort by key for deterministic ordering (consensus-critical).
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

impl<K: Kv> SmtStore for SmtOverlayStore<'_, K> {
    type Error = K::Error;

    fn get_node(&self, node_hash: &Hash32) -> Result<Option<[u8; Node::ENCODED_LEN]>, Self::Error> {
        let key = smt_node_key(node_hash);

        // First check buffered writes.
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

fn append_smt_ops_for_state_ops<K: Kv>(
    db: &K,
    state_ops: &[WriteOp],
    out_ops: &mut Vec<WriteOp>,
) -> Result<Hash32, ExecError<K::Error>> {
    let cur_root = read_smt_root_or_default(db)?;

    // Build SMT updates in an overlay store so we can batch node writes with state writes.
    let store = SmtOverlayStore::new(db);
    let mut smt = Smt::with_root(store, cur_root);

    for op in state_ops {
        match op {
            WriteOp::Put(k, v) => {
                let sk: Hash32 = smt_key_for_state_key(k);
                smt.update(sk, v).map_err(|e| match e {
                    SmtError::Store(err) => ExecError::Db(err),
                    _ => ExecError::Overflow,
                })?;
            }
            WriteOp::Delete(k) => {
                let sk: Hash32 = smt_key_for_state_key(k);
                smt.delete(sk).map_err(|e| match e {
                    SmtError::Store(err) => ExecError::Db(err),
                    _ => ExecError::Overflow,
                })?;
            }
        }
    }

    let new_root = smt.root();
    let store = smt.into_store();

    // Add SMT node writes.
    out_ops.extend(store.into_write_ops());

    // Add root record write.
    out_ops.push(WriteOp::Put(
        KEY_SMT_ROOT.to_vec(),
        encode_smt_root_v1(&new_root).to_vec(),
    ));

    Ok(new_root)
}

/// Apply a single `TxV1` as a `TransferPayloadV1` against the account state machine.
///
/// Rules (Week 3):
/// - Nonce exact match.
/// - Sender balance >= amount + fee.
/// - Checked arithmetic only.
/// - Debit sender by (amount + fee), credit receiver by amount.
/// - Increment sender nonce by 1.
/// - Add fee to `fee_pool`.
///
/// ATOMIC: All state changes are applied in a single batch (all-or-nothing).
///
/// # Errors
/// Returns error if nonce mismatch, insufficient funds, payload decode fails, or DB error.
pub fn apply_tx_v1_transfer<K: KvBatch>(db: &mut K, tx: &TxV1) -> Result<(), ExecError<K::Error>> {
    // Decode payload (deterministic).
    let payload = decode_transfer_payload_v1(&tx.payload).map_err(|e| match e {
        ExecError::BadPayloadLength { expected, got } => {
            ExecError::BadPayloadLength { expected, got }
        }
        ExecError::BadPayloadVersion { expected, got } => {
            ExecError::BadPayloadVersion { expected, got }
        }
        _ => ExecError::Overflow,
    })?;

    // Read current state (no mutations yet)
    let mut from_acct = read_account_or_default(db, &tx.from)?;
    if tx.nonce != from_acct.nonce {
        return Err(ExecError::NonceMismatch {
            expected: from_acct.nonce,
            got: tx.nonce,
        });
    }

    let mut to_acct = read_account_or_default(db, &payload.to)?;
    let mut fee_pool = read_fee_pool_or_default(db)?;

    // Validate and compute new state (all in memory, no writes yet)
    let amount_u128 = u64_to_u128_checked(payload.amount);
    let fee_u128 = u64_to_u128_checked(tx.fee);

    let needed = amount_u128
        .checked_add(fee_u128)
        .ok_or(ExecError::Overflow)?;
    if from_acct.balance < needed {
        return Err(ExecError::InsufficientFunds {
            balance: from_acct.balance,
            needed,
        });
    }

    from_acct.balance = from_acct
        .balance
        .checked_sub(needed)
        .ok_or(ExecError::Overflow)?;
    to_acct.balance = to_acct
        .balance
        .checked_add(amount_u128)
        .ok_or(ExecError::Overflow)?;
    fee_pool.balance = fee_pool
        .balance
        .checked_add(fee_u128)
        .ok_or(ExecError::Overflow)?;

    from_acct.nonce = from_acct
        .nonce
        .checked_add(1)
        .ok_or(ExecError::NonceOverflow)?;

    // Build atomic batch of all state changes.
    let ops = vec![
        WriteOp::Put(
            account_key(&tx.from),
            encode_account_v1(&from_acct).to_vec(),
        ),
        WriteOp::Put(
            account_key(&payload.to),
            encode_account_v1(&to_acct).to_vec(),
        ),
        WriteOp::Put(
            KEY_FEE_POOL.to_vec(),
            encode_fee_pool_v1(&fee_pool).to_vec(),
        ),
    ];

    // Append SMT node writes + smt root write, derived deterministically from the state ops.
    let mut all_ops = ops;
    let state_ops_snapshot = all_ops.clone();
    let _new_root = append_smt_ops_for_state_ops(db, &state_ops_snapshot, &mut all_ops)?;

    // Apply ALL changes atomically (state + SMT nodes + root).
    db.apply_batch(&all_ops).map_err(ExecError::Db)?;

    Ok(())
}

// ============================================================================
// AI STORAGE OPERATIONS (Retrofit Week 3)
// ============================================================================

use novai_ai_entities::AiEntity;
use novai_state::{ai_entity_key, ai_memory_key};

/// AI Entity encoding version.
pub const AI_ENTITY_CODEC_V1: u8 = 1;

/// Encode an `AiEntity` to canonical bytes.
///
/// Format: [version:1][code_hash:32][creator:32][autonomy_mode:1][capabilities:1]
///         `[economic_balance_be:16][nonce_be:8][memory_root:32][params_root:32]`
///         `[registered_at_be:8][last_active_at_be:8]`
///
/// Total: 1 + 32 + 32 + 1 + 1 + 16 + 8 + 32 + 32 + 8 + 8 = 171 bytes
#[must_use]
pub fn encode_ai_entity_v1(entity: &AiEntity) -> [u8; 171] {
    let mut out = [0u8; 171];
    let mut pos = 0;

    out[pos] = AI_ENTITY_CODEC_V1;
    pos += 1;

    out[pos..pos + 32].copy_from_slice(&entity.code_hash);
    pos += 32;

    out[pos..pos + 32].copy_from_slice(&entity.creator);
    pos += 32;

    out[pos] = entity.autonomy_mode.to_byte();
    pos += 1;

    out[pos] = entity.capabilities.to_byte();
    pos += 1;

    out[pos..pos + 16].copy_from_slice(&entity.economic_balance.to_be_bytes());
    pos += 16;

    out[pos..pos + 8].copy_from_slice(&entity.nonce.to_be_bytes());
    pos += 8;

    out[pos..pos + 32].copy_from_slice(&entity.memory_root);
    pos += 32;

    out[pos..pos + 32].copy_from_slice(&entity.params_root);
    pos += 32;

    out[pos..pos + 8].copy_from_slice(&entity.registered_at.to_be_bytes());
    pos += 8;

    out[pos..pos + 8].copy_from_slice(&entity.last_active_at.to_be_bytes());

    out
}

/// Decode an `AiEntity` from canonical bytes.
///
/// # Errors
///
/// Returns error if payload length or version is invalid.
pub fn decode_ai_entity_v1(bytes: &[u8]) -> Result<AiEntity, ExecError<()>> {
    const LEN: usize = 171;
    if bytes.len() != LEN {
        return Err(ExecError::BadPayloadLength {
            expected: LEN,
            got: bytes.len(),
        });
    }

    let mut pos = 0;

    let version = bytes[pos];
    if version != AI_ENTITY_CODEC_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: AI_ENTITY_CODEC_V1,
            got: version,
        });
    }
    pos += 1;

    let mut code_hash = [0u8; 32];
    code_hash.copy_from_slice(&bytes[pos..pos + 32]);
    pos += 32;

    let mut creator = [0u8; 32];
    creator.copy_from_slice(&bytes[pos..pos + 32]);
    pos += 32;

    let autonomy_mode = novai_ai_entities::AutonomyMode::from_byte(bytes[pos]).ok_or(
        ExecError::BadPayloadVersion {
            expected: 0,
            got: bytes[pos],
        },
    )?;
    pos += 1;

    let capabilities = novai_ai_entities::Capabilities::from_byte(bytes[pos]);
    pos += 1;

    let mut bal_bytes = [0u8; 16];
    bal_bytes.copy_from_slice(&bytes[pos..pos + 16]);
    let economic_balance = u128::from_be_bytes(bal_bytes);
    pos += 16;

    let mut nonce_bytes = [0u8; 8];
    nonce_bytes.copy_from_slice(&bytes[pos..pos + 8]);
    let nonce = u64::from_be_bytes(nonce_bytes);
    pos += 8;

    let mut memory_root = [0u8; 32];
    memory_root.copy_from_slice(&bytes[pos..pos + 32]);
    pos += 32;

    let mut params_root = [0u8; 32];
    params_root.copy_from_slice(&bytes[pos..pos + 32]);
    pos += 32;

    let mut reg_bytes = [0u8; 8];
    reg_bytes.copy_from_slice(&bytes[pos..pos + 8]);
    let registered_at = u64::from_be_bytes(reg_bytes);
    pos += 8;

    let mut last_bytes = [0u8; 8];
    last_bytes.copy_from_slice(&bytes[pos..pos + 8]);
    let last_active_at = u64::from_be_bytes(last_bytes);

    // Compute ID from code_hash and creator (deterministic)
    let id = AiEntity::compute_id(&code_hash, &creator);

    Ok(AiEntity {
        id,
        code_hash,
        creator,
        autonomy_mode,
        capabilities,
        economic_balance,
        nonce,
        memory_root,
        params_root,
        registered_at,
        last_active_at,
    })
}

/// Read an AI entity from storage.
///
/// # Errors
///
/// Returns error if DB read fails or stored bytes are malformed.
pub fn read_ai_entity<K: Kv>(
    db: &K,
    entity_id: &[u8; 32],
) -> Result<Option<AiEntity>, ExecError<K::Error>> {
    let key = ai_entity_key(entity_id);
    match db.get(&key).map_err(ExecError::Db)? {
        None => Ok(None),
        Some(bytes) => {
            let entity = decode_ai_entity_v1(&bytes).map_err(|e| match e {
                ExecError::BadPayloadLength { expected, got } => {
                    ExecError::BadPayloadLength { expected, got }
                }
                ExecError::BadPayloadVersion { expected, got } => {
                    ExecError::BadPayloadVersion { expected, got }
                }
                _ => ExecError::Overflow,
            })?;
            Ok(Some(entity))
        }
    }
}

/// Write an AI entity to storage (returns `WriteOp` for batching).
#[must_use]
pub fn write_ai_entity_op(entity: &AiEntity) -> WriteOp {
    let key = ai_entity_key(&entity.id);
    let value = encode_ai_entity_v1(entity).to_vec();
    WriteOp::Put(key, value)
}

/// Read AI memory slot value.
///
/// # Errors
///
/// Returns error if DB read fails.
pub fn read_ai_memory<K: Kv>(
    db: &K,
    entity_id: &[u8; 32],
    slot: &[u8],
) -> Result<Option<Vec<u8>>, ExecError<K::Error>> {
    let key = ai_memory_key(entity_id, slot);
    db.get(&key).map_err(ExecError::Db)
}

/// Create `WriteOp` to write AI memory slot.
#[must_use]
pub fn write_ai_memory_op(entity_id: &[u8; 32], slot: &[u8], value: Vec<u8>) -> WriteOp {
    let key = ai_memory_key(entity_id, slot);
    WriteOp::Put(key, value)
}

/// Create `WriteOp` to delete AI memory slot.
#[must_use]
pub fn delete_ai_memory_op(entity_id: &[u8; 32], slot: &[u8]) -> WriteOp {
    let key = ai_memory_key(entity_id, slot);
    WriteOp::Delete(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use novai_ai_entities::{AutonomyMode, Capabilities};
    use novai_state::{ai_signal_key, KvBatch, MemKv};

    #[test]
    fn ai_entity_write_read_roundtrip() {
        let mut db = MemKv::new();

        // Create an AI entity
        let entity = AiEntity::new(
            [0x42u8; 32], // code_hash
            [0x01u8; 32], // creator
            AutonomyMode::Gated,
            Capabilities::gated(),
            1000, // registered_at
        );

        // Write entity to DB
        let op = write_ai_entity_op(&entity);
        db.apply_batch(&[op]).unwrap();

        // Read it back
        let read_back = read_ai_entity(&db, &entity.id).unwrap();
        assert!(read_back.is_some(), "Entity should exist after write");

        let read_entity = read_back.unwrap();
        assert_eq!(read_entity.id, entity.id);
        assert_eq!(read_entity.code_hash, entity.code_hash);
        assert_eq!(read_entity.creator, entity.creator);
        assert_eq!(read_entity.autonomy_mode, entity.autonomy_mode);
        assert_eq!(
            read_entity.capabilities.to_byte(),
            entity.capabilities.to_byte()
        );
        assert_eq!(read_entity.economic_balance, entity.economic_balance);
        assert_eq!(read_entity.nonce, entity.nonce);
        assert_eq!(read_entity.memory_root, entity.memory_root);
        assert_eq!(read_entity.params_root, entity.params_root);
        assert_eq!(read_entity.registered_at, entity.registered_at);
        assert_eq!(read_entity.last_active_at, entity.last_active_at);
    }

    #[test]
    fn ai_memory_isolation() {
        let mut db = MemKv::new();

        let entity_a = [0xAAu8; 32];
        let entity_b = [0xBBu8; 32];
        let slot = b"config";

        // Write different values for same slot name, different entities
        let op_a = write_ai_memory_op(&entity_a, slot, b"value_for_a".to_vec());
        let op_b = write_ai_memory_op(&entity_b, slot, b"value_for_b".to_vec());
        db.apply_batch(&[op_a, op_b]).unwrap();

        // Read back and verify isolation
        let val_a = read_ai_memory(&db, &entity_a, slot).unwrap();
        let val_b = read_ai_memory(&db, &entity_b, slot).unwrap();

        assert_eq!(val_a, Some(b"value_for_a".to_vec()));
        assert_eq!(val_b, Some(b"value_for_b".to_vec()));

        // Verify they are different
        assert_ne!(val_a, val_b, "Memory must be isolated between entities");
    }

    #[test]
    fn ai_key_ordering() {
        // Signal keys for heights 100, 200 must be lexicographically ordered
        // because we use big-endian encoding for height
        let issuer = [0x01u8; 32];

        let key_100 = ai_signal_key(100, &issuer);
        let key_200 = ai_signal_key(200, &issuer);

        assert!(
            key_100 < key_200,
            "ai_signal_key(100, x) must be < ai_signal_key(200, x) for range scans"
        );

        // Also test edge cases
        let key_0 = ai_signal_key(0, &issuer);
        let key_max = ai_signal_key(u64::MAX, &issuer);

        assert!(key_0 < key_100);
        assert!(key_200 < key_max);

        // Different issuers at same height should not affect height ordering
        let issuer_2 = [0xFFu8; 32];
        let key_100_issuer2 = ai_signal_key(100, &issuer_2);
        let key_200_issuer1 = ai_signal_key(200, &issuer);

        assert!(
            key_100_issuer2 < key_200_issuer1,
            "Height ordering must take precedence over issuer"
        );
    }

    #[test]
    fn ai_entity_not_found_returns_none() {
        let db = MemKv::new();
        let nonexistent = [0xDEu8; 32];

        let result = read_ai_entity(&db, &nonexistent).unwrap();
        assert!(result.is_none(), "Non-existent entity should return None");
    }

    #[test]
    fn ai_memory_not_found_returns_none() {
        let db = MemKv::new();
        let entity = [0xAAu8; 32];

        let result = read_ai_memory(&db, &entity, b"nonexistent").unwrap();
        assert!(
            result.is_none(),
            "Non-existent memory slot should return None"
        );
    }

    #[test]
    fn ai_memory_delete_works() {
        let mut db = MemKv::new();
        let entity = [0xAAu8; 32];
        let slot = b"temp_data";

        // Write
        let write_op = write_ai_memory_op(&entity, slot, b"some_value".to_vec());
        db.apply_batch(&[write_op]).unwrap();

        // Verify exists
        let val = read_ai_memory(&db, &entity, slot).unwrap();
        assert!(val.is_some());

        // Delete
        let delete_op = delete_ai_memory_op(&entity, slot);
        db.apply_batch(&[delete_op]).unwrap();

        // Verify gone
        let val_after = read_ai_memory(&db, &entity, slot).unwrap();
        assert!(val_after.is_none(), "Memory slot should be deleted");
    }
}
