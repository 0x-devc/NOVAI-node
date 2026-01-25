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
    BadPayloadLength {
        expected: usize,
        got: usize,
    },
    BadPayloadVersion {
        expected: u8,
        got: u8,
    },
    NonceMismatch {
        expected: Nonce,
        got: Nonce,
    },
    InsufficientFunds {
        balance: u128,
        needed: u128,
    },
    Overflow,
    NonceOverflow,
    // Week 14 - Signal commitment errors (D14.2)
    /// Issuer entity ID not found in state.
    IssuerNotFound,
    /// Issuer entity does not have `emit_proposals` capability.
    IssuerMissingCapability,
    /// Issuer entity ID in payload does not match tx.from.
    IssuerMismatch,
    // Week 21 - Memory object errors (D21.4)
    /// Memory object data exceeds maximum size.
    MemoryObjectTooLarge {
        size: usize,
        max: usize,
    },
    /// Entity has too many memory objects.
    MemoryObjectCountExceeded {
        count: u32,
        max: u32,
    },
    /// Memory object not found.
    MemoryObjectNotFound,
    /// Invalid memory object type byte.
    InvalidMemoryObjectType {
        byte: u8,
    },
    /// Entity does not own the memory object.
    MemoryObjectOwnerMismatch,
    // Week 22 - NNPX privacy errors (D22.4)
    /// AI entity attempted to access NNPX private data.
    NnpxAccessDenied,
    /// Nullifier has already been spent (double-spend attempt).
    NullifierAlreadySpent,
    /// Invalid private payload commitment.
    InvalidPrivateCommitment,
    // Week 23 - Derived view errors (D23.5)
    /// AI entity missing `read_nnpx_derived` capability.
    DerivedViewAccessDenied,
    /// Derived view not found.
    DerivedViewNotFound,
    /// Invalid derived view schema.
    InvalidDerivedViewSchema { schema_id: u32 },
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

// ============================================================================
// SIGNAL COMMITMENT PAYLOAD (Week 14 - D14.1)
// ============================================================================

/// Signal commitment payload version.
pub const SIGNAL_COMMITMENT_PAYLOAD_V1: u8 = 2;

/// Canonical Signal Commitment payload (D14.1):
/// `[version:1][signal_hash:32][signal_type:1][issuer_entity_id:32]`
///
/// Total size: 66 bytes
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalCommitmentPayloadV1 {
    /// Commitment hash of the full signal.
    pub signal_hash: [u8; 32],
    /// Signal type (0-6).
    pub signal_type: novai_ai_entities::AiSignalType,
    /// AI entity ID that issued this signal.
    pub issuer_entity_id: [u8; 32],
}

/// Deterministically encode a signal commitment payload.
#[must_use]
pub fn encode_signal_commitment_payload_v1(p: &SignalCommitmentPayloadV1) -> [u8; 66] {
    let mut out = [0u8; 66];
    out[0] = SIGNAL_COMMITMENT_PAYLOAD_V1;
    out[1..33].copy_from_slice(&p.signal_hash);
    out[33] = p.signal_type.to_byte();
    out[34..66].copy_from_slice(&p.issuer_entity_id);
    out
}

/// Deterministically decode a signal commitment payload from `tx.payload`.
///
/// # Errors
/// Returns error if payload length or version is invalid.
pub fn decode_signal_commitment_payload_v1(
    payload: &[u8],
) -> Result<SignalCommitmentPayloadV1, ExecError<()>> {
    const LEN: usize = 66;
    if payload.len() != LEN {
        return Err(ExecError::BadPayloadLength {
            expected: LEN,
            got: payload.len(),
        });
    }
    let ver = payload[0];
    if ver != SIGNAL_COMMITMENT_PAYLOAD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: SIGNAL_COMMITMENT_PAYLOAD_V1,
            got: ver,
        });
    }

    let mut signal_hash = [0u8; 32];
    signal_hash.copy_from_slice(&payload[1..33]);

    let signal_type = novai_ai_entities::AiSignalType::from_byte(payload[33]).ok_or(
        ExecError::BadPayloadVersion {
            expected: 6, // max valid signal type
            got: payload[33],
        },
    )?;

    let mut issuer_entity_id = [0u8; 32];
    issuer_entity_id.copy_from_slice(&payload[34..66]);

    Ok(SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type,
        issuer_entity_id,
    })
}

// ============================================================================
// MEMORY OBJECT PAYLOADS (Week 21 - D21.4)
// ============================================================================

/// Create memory object payload version.
pub const CREATE_MEMORY_OBJECT_PAYLOAD_V1: u8 = 3;

/// Update memory object payload version.
pub const UPDATE_MEMORY_OBJECT_PAYLOAD_V1: u8 = 4;

/// Delete memory object payload version.
pub const DELETE_MEMORY_OBJECT_PAYLOAD_V1: u8 = 5;

/// Create Memory Object payload (D21.4):
/// `[version:1][object_type:1][data_len_be:4][data:var]`
///
/// Minimum size: 6 bytes (empty data)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateMemoryObjectPayloadV1 {
    /// Type of memory object to create.
    pub object_type: novai_ai_entities::MemoryObjectType,
    /// Initial data for the object.
    pub data: Vec<u8>,
}

/// Deterministically encode a create memory object payload.
#[must_use]
pub fn encode_create_memory_object_payload_v1(p: &CreateMemoryObjectPayloadV1) -> Vec<u8> {
    let mut out = Vec::with_capacity(6 + p.data.len());
    out.push(CREATE_MEMORY_OBJECT_PAYLOAD_V1);
    out.push(p.object_type.to_byte());
    #[allow(clippy::cast_possible_truncation)]
    let data_len = p.data.len() as u32;
    out.extend_from_slice(&data_len.to_be_bytes());
    out.extend_from_slice(&p.data);
    out
}

/// Deterministically decode a create memory object payload.
///
/// # Errors
/// Returns error if payload is malformed.
pub fn decode_create_memory_object_payload_v1(
    payload: &[u8],
) -> Result<CreateMemoryObjectPayloadV1, ExecError<()>> {
    const MIN_LEN: usize = 6; // version + type + data_len
    if payload.len() < MIN_LEN {
        return Err(ExecError::BadPayloadLength {
            expected: MIN_LEN,
            got: payload.len(),
        });
    }

    let ver = payload[0];
    if ver != CREATE_MEMORY_OBJECT_PAYLOAD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: CREATE_MEMORY_OBJECT_PAYLOAD_V1,
            got: ver,
        });
    }

    let object_type = novai_ai_entities::MemoryObjectType::from_byte(payload[1])
        .ok_or(ExecError::InvalidMemoryObjectType { byte: payload[1] })?;

    let mut data_len_bytes = [0u8; 4];
    data_len_bytes.copy_from_slice(&payload[2..6]);
    let data_len = u32::from_be_bytes(data_len_bytes) as usize;

    if payload.len() != MIN_LEN + data_len {
        return Err(ExecError::BadPayloadLength {
            expected: MIN_LEN + data_len,
            got: payload.len(),
        });
    }

    let data = payload[6..].to_vec();

    Ok(CreateMemoryObjectPayloadV1 { object_type, data })
}

/// Update Memory Object payload (D21.4):
/// `[version:1][object_id:32][data_len_be:4][new_data:var]`
///
/// Minimum size: 37 bytes (empty data)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateMemoryObjectPayloadV1 {
    /// ID of the memory object to update.
    pub object_id: [u8; 32],
    /// New data for the object.
    pub new_data: Vec<u8>,
}

/// Deterministically encode an update memory object payload.
#[must_use]
pub fn encode_update_memory_object_payload_v1(p: &UpdateMemoryObjectPayloadV1) -> Vec<u8> {
    let mut out = Vec::with_capacity(37 + p.new_data.len());
    out.push(UPDATE_MEMORY_OBJECT_PAYLOAD_V1);
    out.extend_from_slice(&p.object_id);
    #[allow(clippy::cast_possible_truncation)]
    let data_len = p.new_data.len() as u32;
    out.extend_from_slice(&data_len.to_be_bytes());
    out.extend_from_slice(&p.new_data);
    out
}

/// Deterministically decode an update memory object payload.
///
/// # Errors
/// Returns error if payload is malformed.
pub fn decode_update_memory_object_payload_v1(
    payload: &[u8],
) -> Result<UpdateMemoryObjectPayloadV1, ExecError<()>> {
    const MIN_LEN: usize = 37; // version + object_id + data_len
    if payload.len() < MIN_LEN {
        return Err(ExecError::BadPayloadLength {
            expected: MIN_LEN,
            got: payload.len(),
        });
    }

    let ver = payload[0];
    if ver != UPDATE_MEMORY_OBJECT_PAYLOAD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: UPDATE_MEMORY_OBJECT_PAYLOAD_V1,
            got: ver,
        });
    }

    let mut object_id = [0u8; 32];
    object_id.copy_from_slice(&payload[1..33]);

    let mut data_len_bytes = [0u8; 4];
    data_len_bytes.copy_from_slice(&payload[33..37]);
    let data_len = u32::from_be_bytes(data_len_bytes) as usize;

    if payload.len() != MIN_LEN + data_len {
        return Err(ExecError::BadPayloadLength {
            expected: MIN_LEN + data_len,
            got: payload.len(),
        });
    }

    let new_data = payload[37..].to_vec();

    Ok(UpdateMemoryObjectPayloadV1 {
        object_id,
        new_data,
    })
}

/// Delete Memory Object payload (D21.4):
/// `[version:1][object_id:32]`
///
/// Total size: 33 bytes
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteMemoryObjectPayloadV1 {
    /// ID of the memory object to delete.
    pub object_id: [u8; 32],
}

/// Deterministically encode a delete memory object payload.
#[must_use]
pub fn encode_delete_memory_object_payload_v1(p: &DeleteMemoryObjectPayloadV1) -> [u8; 33] {
    let mut out = [0u8; 33];
    out[0] = DELETE_MEMORY_OBJECT_PAYLOAD_V1;
    out[1..33].copy_from_slice(&p.object_id);
    out
}

/// Deterministically decode a delete memory object payload.
///
/// # Errors
/// Returns error if payload is malformed.
pub fn decode_delete_memory_object_payload_v1(
    payload: &[u8],
) -> Result<DeleteMemoryObjectPayloadV1, ExecError<()>> {
    const LEN: usize = 33;
    if payload.len() != LEN {
        return Err(ExecError::BadPayloadLength {
            expected: LEN,
            got: payload.len(),
        });
    }

    let ver = payload[0];
    if ver != DELETE_MEMORY_OBJECT_PAYLOAD_V1 {
        return Err(ExecError::BadPayloadVersion {
            expected: DELETE_MEMORY_OBJECT_PAYLOAD_V1,
            got: ver,
        });
    }

    let mut object_id = [0u8; 32];
    object_id.copy_from_slice(&payload[1..33]);

    Ok(DeleteMemoryObjectPayloadV1 { object_id })
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

use novai_ai_entities::{AiEntity, SignalCommitment};
use novai_codec::encode_signal_commitment_v1;
use novai_state::{
    ai_entity_key, ai_memory_key, ai_signal_by_issuer_key, ai_signal_by_type_key, ai_signal_key,
};

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

// ============================================================================
// SIGNAL COMMITMENT EXECUTION (Week 14 - D14.2, D14.3, D14.6)
// ============================================================================

/// Apply a `PublishSignalCommitment` transaction.
///
/// # Validation (D14.2)
/// 1. Issuer entity ID must match `tx.from`
/// 2. Issuer must be a registered AI entity
/// 3. Issuer must have `emit_proposals` capability
/// 4. Signal type must be valid (0-6) - checked during payload decode
/// 5. Issuer must have sufficient balance for fee
/// 6. Nonce must be correct
///
/// # State Changes (D14.3, D14.4, D14.6)
/// - Store commitment at `ai_signal_key(height, issuer)`
/// - Create secondary index by type
/// - Create secondary index by issuer
/// - Deduct fee from AI entity balance
/// - Increment AI entity nonce
/// - Update `last_active_at`
///
/// # Errors
/// Returns error if validation fails or DB error occurs.
pub fn apply_signal_commitment_tx<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    current_height: u64,
) -> Result<(), ExecError<K::Error>> {
    // Decode payload (validates signal_type is 0-6)
    let payload = decode_signal_commitment_payload_v1(&tx.payload).map_err(|e| match e {
        ExecError::BadPayloadLength { expected, got } => {
            ExecError::BadPayloadLength { expected, got }
        }
        ExecError::BadPayloadVersion { expected, got } => {
            ExecError::BadPayloadVersion { expected, got }
        }
        _ => ExecError::Overflow,
    })?;

    // D14.2: Validate issuer_entity_id matches tx.from
    if payload.issuer_entity_id != tx.from {
        return Err(ExecError::IssuerMismatch);
    }

    // D14.2: Load and validate AI entity
    let mut entity =
        read_ai_entity(db, &payload.issuer_entity_id)?.ok_or(ExecError::IssuerNotFound)?;

    // D14.2: Validate emit_proposals capability
    if !entity.capabilities.emit_proposals {
        return Err(ExecError::IssuerMissingCapability);
    }

    // D14.2: Validate nonce
    if tx.nonce != entity.nonce {
        return Err(ExecError::NonceMismatch {
            expected: entity.nonce,
            got: tx.nonce,
        });
    }

    // D14.2: Validate sufficient balance for fee
    let fee_u128 = u128::from(tx.fee);
    if entity.economic_balance < fee_u128 {
        return Err(ExecError::InsufficientFunds {
            balance: entity.economic_balance,
            needed: fee_u128,
        });
    }

    // D14.6: Deduct fee from AI entity balance
    entity.economic_balance = entity
        .economic_balance
        .checked_sub(fee_u128)
        .ok_or(ExecError::Overflow)?;

    // D14.6: Increment AI entity nonce
    entity.nonce = entity
        .nonce
        .checked_add(1)
        .ok_or(ExecError::NonceOverflow)?;

    // D14.6: Update last_active_at
    entity.last_active_at = current_height;

    // D14.3: Build SignalCommitment for storage
    let commitment = SignalCommitment {
        commitment_hash: payload.signal_hash,
        signal_type: payload.signal_type,
        height: current_height,
        issuer: payload.issuer_entity_id,
    };
    let commitment_bytes = encode_signal_commitment_v1(&commitment);

    // Build atomic batch of all state changes
    let mut ops = Vec::new();

    // D14.3: Store commitment at primary key
    let primary_key = ai_signal_key(current_height, &payload.issuer_entity_id);
    ops.push(WriteOp::Put(primary_key, commitment_bytes.clone()));

    // D14.4: Secondary index by type
    let type_key = ai_signal_by_type_key(
        payload.signal_type.to_byte(),
        current_height,
        &payload.issuer_entity_id,
    );
    ops.push(WriteOp::Put(type_key, commitment_bytes.clone()));

    // D14.4: Secondary index by issuer
    let issuer_key = ai_signal_by_issuer_key(&payload.issuer_entity_id, current_height);
    ops.push(WriteOp::Put(issuer_key, commitment_bytes));

    // D14.6: Update AI entity
    ops.push(write_ai_entity_op(&entity));

    // Apply all changes atomically
    db.apply_batch(&ops).map_err(ExecError::Db)?;

    Ok(())
}

// ============================================================================
// MEMORY OBJECT EXECUTION (Week 21 - D21.4)
// ============================================================================

use novai_ai_entities::{
    decode_memory_object_v1, encode_memory_object_v1, MemoryObject, MAX_MEMORY_OBJECTS_PER_ENTITY,
    MAX_MEMORY_OBJECT_SIZE,
};
use novai_state::{
    ai_memory_by_type_key, ai_memory_count_key, ai_memory_object_key, decode_memory_count,
    encode_memory_count, KEY_PREFIX_AI_MEMORY_OBJECTS,
};

/// Read memory object count for an entity.
fn read_memory_count<K: Kv>(db: &K, entity_id: &[u8; 32]) -> Result<u32, ExecError<K::Error>> {
    let key = ai_memory_count_key(entity_id);
    Ok(db
        .get(&key)
        .map_err(ExecError::Db)?
        .map_or(0, |bytes| decode_memory_count(&bytes)))
}

/// Read a memory object from storage.
///
/// # Errors
/// Returns error if DB read fails or stored data is malformed.
pub fn read_memory_object<K: Kv>(
    db: &K,
    entity_id: &[u8; 32],
    object_id: &[u8; 32],
) -> Result<Option<MemoryObject>, ExecError<K::Error>> {
    let key = ai_memory_object_key(entity_id, object_id);
    match db.get(&key).map_err(ExecError::Db)? {
        None => Ok(None),
        Some(bytes) => {
            let obj = decode_memory_object_v1(&bytes).map_err(|_| ExecError::Overflow)?;
            Ok(Some(obj))
        }
    }
}

/// Apply a `CreateMemoryObject` transaction.
///
/// # Validation (D21.4)
/// 1. Entity must exist and match tx.from
/// 2. Entity must have `read_memory_objects` capability
/// 3. Data size must not exceed `MAX_MEMORY_OBJECT_SIZE`
/// 4. Entity must not exceed `MAX_MEMORY_OBJECTS_PER_ENTITY`
/// 5. Nonce must be correct
/// 6. Sufficient balance for fee
///
/// # State Changes
/// - Create memory object with computed ID
/// - Store at `ai/memory_objects/{entity}/{object_id}`
/// - Create type index at `ai/memory_by_type/{type}/{entity}/{object_id}`
/// - Increment memory count
/// - Deduct fee, increment nonce, update `last_active_at`
///
/// # Errors
/// Returns error if validation fails or DB error occurs.
pub fn apply_create_memory_object_tx<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    current_height: u64,
) -> Result<[u8; 32], ExecError<K::Error>> {
    // Decode payload
    let payload = decode_create_memory_object_payload_v1(&tx.payload).map_err(|e| match e {
        ExecError::BadPayloadLength { expected, got } => {
            ExecError::BadPayloadLength { expected, got }
        }
        ExecError::BadPayloadVersion { expected, got } => {
            ExecError::BadPayloadVersion { expected, got }
        }
        ExecError::InvalidMemoryObjectType { byte } => ExecError::InvalidMemoryObjectType { byte },
        _ => ExecError::Overflow,
    })?;

    // Validate data size
    if payload.data.len() > MAX_MEMORY_OBJECT_SIZE {
        return Err(ExecError::MemoryObjectTooLarge {
            size: payload.data.len(),
            max: MAX_MEMORY_OBJECT_SIZE,
        });
    }

    // Load and validate AI entity
    let mut entity = read_ai_entity(db, &tx.from)?.ok_or(ExecError::IssuerNotFound)?;

    // Validate capability
    if !entity.capabilities.read_memory_objects {
        return Err(ExecError::IssuerMissingCapability);
    }

    // Validate nonce
    if tx.nonce != entity.nonce {
        return Err(ExecError::NonceMismatch {
            expected: entity.nonce,
            got: tx.nonce,
        });
    }

    // Validate balance
    let fee_u128 = u128::from(tx.fee);
    if entity.economic_balance < fee_u128 {
        return Err(ExecError::InsufficientFunds {
            balance: entity.economic_balance,
            needed: fee_u128,
        });
    }

    // Check memory object count limit
    let current_count = read_memory_count(db, &tx.from)?;
    if current_count >= MAX_MEMORY_OBJECTS_PER_ENTITY {
        return Err(ExecError::MemoryObjectCountExceeded {
            count: current_count,
            max: MAX_MEMORY_OBJECTS_PER_ENTITY,
        });
    }

    // Create memory object
    let memory_object =
        MemoryObject::new(tx.from, payload.object_type, current_height, payload.data);
    let object_id = memory_object.object_id;
    let encoded = encode_memory_object_v1(&memory_object);

    // Update entity state
    entity.economic_balance = entity
        .economic_balance
        .checked_sub(fee_u128)
        .ok_or(ExecError::Overflow)?;
    entity.nonce = entity
        .nonce
        .checked_add(1)
        .ok_or(ExecError::NonceOverflow)?;
    entity.last_active_at = current_height;

    // Build atomic batch
    let mut ops = Vec::new();

    // Store memory object
    let obj_key = ai_memory_object_key(&tx.from, &object_id);
    ops.push(WriteOp::Put(obj_key, encoded));

    // Type index
    let type_key = ai_memory_by_type_key(payload.object_type.to_byte(), &tx.from, &object_id);
    ops.push(WriteOp::Put(type_key, vec![])); // Presence-only index

    // Update count
    let count_key = ai_memory_count_key(&tx.from);
    ops.push(WriteOp::Put(
        count_key,
        encode_memory_count(current_count + 1).to_vec(),
    ));

    // Update entity
    ops.push(write_ai_entity_op(&entity));

    // Apply atomically
    db.apply_batch(&ops).map_err(ExecError::Db)?;

    Ok(object_id)
}

/// Apply an `UpdateMemoryObject` transaction.
///
/// # Validation (D21.4)
/// 1. Entity must exist and match tx.from
/// 2. Memory object must exist and be owned by entity
/// 3. New data size must not exceed `MAX_MEMORY_OBJECT_SIZE`
/// 4. Nonce must be correct
/// 5. Sufficient balance for fee
///
/// # State Changes
/// - Update memory object data and `updated_at`
/// - Deduct fee, increment nonce, update `last_active_at`
///
/// # Errors
/// Returns error if validation fails or DB error occurs.
pub fn apply_update_memory_object_tx<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    current_height: u64,
) -> Result<(), ExecError<K::Error>> {
    // Decode payload
    let payload = decode_update_memory_object_payload_v1(&tx.payload).map_err(|e| match e {
        ExecError::BadPayloadLength { expected, got } => {
            ExecError::BadPayloadLength { expected, got }
        }
        ExecError::BadPayloadVersion { expected, got } => {
            ExecError::BadPayloadVersion { expected, got }
        }
        _ => ExecError::Overflow,
    })?;

    // Validate data size
    if payload.new_data.len() > MAX_MEMORY_OBJECT_SIZE {
        return Err(ExecError::MemoryObjectTooLarge {
            size: payload.new_data.len(),
            max: MAX_MEMORY_OBJECT_SIZE,
        });
    }

    // Load and validate AI entity
    let mut entity = read_ai_entity(db, &tx.from)?.ok_or(ExecError::IssuerNotFound)?;

    // Validate nonce
    if tx.nonce != entity.nonce {
        return Err(ExecError::NonceMismatch {
            expected: entity.nonce,
            got: tx.nonce,
        });
    }

    // Validate balance
    let fee_u128 = u128::from(tx.fee);
    if entity.economic_balance < fee_u128 {
        return Err(ExecError::InsufficientFunds {
            balance: entity.economic_balance,
            needed: fee_u128,
        });
    }

    // Load memory object
    let mut memory_object = read_memory_object(db, &tx.from, &payload.object_id)?
        .ok_or(ExecError::MemoryObjectNotFound)?;

    // Validate ownership
    if memory_object.owner_entity != tx.from {
        return Err(ExecError::MemoryObjectOwnerMismatch);
    }

    // Update memory object
    memory_object.data = payload.new_data;
    memory_object.updated_at = current_height;
    let encoded = encode_memory_object_v1(&memory_object);

    // Update entity state
    entity.economic_balance = entity
        .economic_balance
        .checked_sub(fee_u128)
        .ok_or(ExecError::Overflow)?;
    entity.nonce = entity
        .nonce
        .checked_add(1)
        .ok_or(ExecError::NonceOverflow)?;
    entity.last_active_at = current_height;

    // Build atomic batch
    let mut ops = Vec::new();

    // Update memory object
    let obj_key = ai_memory_object_key(&tx.from, &payload.object_id);
    ops.push(WriteOp::Put(obj_key, encoded));

    // Update entity
    ops.push(write_ai_entity_op(&entity));

    // Apply atomically
    db.apply_batch(&ops).map_err(ExecError::Db)?;

    Ok(())
}

/// Apply a `DeleteMemoryObject` transaction.
///
/// # Validation (D21.4)
/// 1. Entity must exist and match tx.from
/// 2. Memory object must exist and be owned by entity
/// 3. Nonce must be correct
/// 4. Sufficient balance for fee
///
/// # State Changes
/// - Delete memory object
/// - Delete type index
/// - Decrement memory count
/// - Deduct fee, increment nonce, update `last_active_at`
///
/// # Errors
/// Returns error if validation fails or DB error occurs.
pub fn apply_delete_memory_object_tx<K: KvBatch>(
    db: &mut K,
    tx: &TxV1,
    current_height: u64,
) -> Result<(), ExecError<K::Error>> {
    // Decode payload
    let payload = decode_delete_memory_object_payload_v1(&tx.payload).map_err(|e| match e {
        ExecError::BadPayloadLength { expected, got } => {
            ExecError::BadPayloadLength { expected, got }
        }
        ExecError::BadPayloadVersion { expected, got } => {
            ExecError::BadPayloadVersion { expected, got }
        }
        _ => ExecError::Overflow,
    })?;

    // Load and validate AI entity
    let mut entity = read_ai_entity(db, &tx.from)?.ok_or(ExecError::IssuerNotFound)?;

    // Validate nonce
    if tx.nonce != entity.nonce {
        return Err(ExecError::NonceMismatch {
            expected: entity.nonce,
            got: tx.nonce,
        });
    }

    // Validate balance
    let fee_u128 = u128::from(tx.fee);
    if entity.economic_balance < fee_u128 {
        return Err(ExecError::InsufficientFunds {
            balance: entity.economic_balance,
            needed: fee_u128,
        });
    }

    // Load memory object
    let memory_object = read_memory_object(db, &tx.from, &payload.object_id)?
        .ok_or(ExecError::MemoryObjectNotFound)?;

    // Validate ownership
    if memory_object.owner_entity != tx.from {
        return Err(ExecError::MemoryObjectOwnerMismatch);
    }

    // Get current count
    let current_count = read_memory_count(db, &tx.from)?;

    // Update entity state
    entity.economic_balance = entity
        .economic_balance
        .checked_sub(fee_u128)
        .ok_or(ExecError::Overflow)?;
    entity.nonce = entity
        .nonce
        .checked_add(1)
        .ok_or(ExecError::NonceOverflow)?;
    entity.last_active_at = current_height;

    // Build atomic batch
    let mut ops = Vec::new();

    // Delete memory object
    let obj_key = ai_memory_object_key(&tx.from, &payload.object_id);
    ops.push(WriteOp::Delete(obj_key));

    // Delete type index
    let type_key = ai_memory_by_type_key(
        memory_object.object_type.to_byte(),
        &tx.from,
        &payload.object_id,
    );
    ops.push(WriteOp::Delete(type_key));

    // Update count (decrement, but don't go below 0)
    let count_key = ai_memory_count_key(&tx.from);
    ops.push(WriteOp::Put(
        count_key,
        encode_memory_count(current_count.saturating_sub(1)).to_vec(),
    ));

    // Update entity
    ops.push(write_ai_entity_op(&entity));

    // Apply atomically
    db.apply_batch(&ops).map_err(ExecError::Db)?;

    Ok(())
}

/// Query all memory objects for an entity.
///
/// Returns all memory objects owned by the entity.
///
/// # Errors
/// Returns error if DB read fails or stored data is malformed.
pub fn get_memory_objects_by_entity<K: Kv>(
    db: &K,
    entity_id: &[u8; 32],
) -> Result<Vec<MemoryObject>, ExecError<K::Error>> {
    // Build prefix: "ai/memory_objects/" ++ entity_id ++ "/"
    let mut prefix = Vec::with_capacity(KEY_PREFIX_AI_MEMORY_OBJECTS.len() + 32 + 1);
    prefix.extend_from_slice(KEY_PREFIX_AI_MEMORY_OBJECTS);
    prefix.extend_from_slice(entity_id);
    prefix.push(b'/');

    let entries = db.scan_prefix(&prefix).map_err(ExecError::Db)?;

    let mut results = Vec::with_capacity(entries.len());
    for (_key, value) in entries {
        let obj = decode_memory_object_v1(&value).map_err(|_| ExecError::Overflow)?;
        results.push(obj);
    }

    Ok(results)
}

// ============================================================================
// SIGNAL QUERY FUNCTIONS (Week 14 - D14.5)
// ============================================================================

use novai_codec::decode_signal_commitment_v1;
use novai_state::{
    KEY_PREFIX_AI_SIGNALS, KEY_PREFIX_AI_SIGNALS_BY_ISSUER, KEY_PREFIX_AI_SIGNALS_BY_TYPE,
};

/// Query all signal commitments at a specific height (D14.5).
///
/// Returns all signals stored at the given height, ordered by issuer.
///
/// # Errors
/// Returns error if DB read fails or stored data is malformed.
pub fn get_signals_by_height<K: Kv>(
    db: &K,
    height: u64,
) -> Result<Vec<SignalCommitment>, ExecError<K::Error>> {
    // Build prefix: "ai/signals/" ++ height_be8 ++ "/"
    let mut prefix = Vec::with_capacity(KEY_PREFIX_AI_SIGNALS.len() + 8 + 1);
    prefix.extend_from_slice(KEY_PREFIX_AI_SIGNALS);
    prefix.extend_from_slice(&height.to_be_bytes());
    prefix.push(b'/');

    let entries = db.scan_prefix(&prefix).map_err(ExecError::Db)?;

    let mut results = Vec::with_capacity(entries.len());
    for (_key, value) in entries {
        let commitment = decode_signal_commitment_v1(&value).map_err(|_| ExecError::Overflow)?;
        results.push(commitment);
    }

    Ok(results)
}

/// Query signal commitments by issuer in height range [`start_height`, `end_height`] (D14.5).
///
/// Returns all signals from the given issuer within the height range.
///
/// # Errors
/// Returns error if DB read fails or stored data is malformed.
pub fn get_signals_by_issuer<K: Kv>(
    db: &K,
    issuer: &[u8; 32],
    start_height: u64,
    end_height: u64,
) -> Result<Vec<SignalCommitment>, ExecError<K::Error>> {
    // Build prefix: "ai/signals/by_issuer/" ++ issuer32 ++ "/"
    let mut prefix = Vec::with_capacity(KEY_PREFIX_AI_SIGNALS_BY_ISSUER.len() + 32 + 1);
    prefix.extend_from_slice(KEY_PREFIX_AI_SIGNALS_BY_ISSUER);
    prefix.extend_from_slice(issuer);
    prefix.push(b'/');

    let entries = db.scan_prefix(&prefix).map_err(ExecError::Db)?;

    let mut results = Vec::new();
    for (_key, value) in entries {
        let commitment = decode_signal_commitment_v1(&value).map_err(|_| ExecError::Overflow)?;
        // Filter by height range
        if commitment.height >= start_height && commitment.height <= end_height {
            results.push(commitment);
        }
    }

    Ok(results)
}

/// Query signal commitments by type in height range [`start_height`, `end_height`] (D14.5).
///
/// Returns all signals of the given type within the height range.
///
/// # Errors
/// Returns error if DB read fails or stored data is malformed.
pub fn get_signals_by_type<K: Kv>(
    db: &K,
    signal_type: novai_ai_entities::AiSignalType,
    start_height: u64,
    end_height: u64,
) -> Result<Vec<SignalCommitment>, ExecError<K::Error>> {
    // Build prefix: "ai/signals/by_type/" ++ type_u8 ++ "/"
    let mut prefix = Vec::with_capacity(KEY_PREFIX_AI_SIGNALS_BY_TYPE.len() + 1 + 1);
    prefix.extend_from_slice(KEY_PREFIX_AI_SIGNALS_BY_TYPE);
    prefix.push(signal_type.to_byte());
    prefix.push(b'/');

    let entries = db.scan_prefix(&prefix).map_err(ExecError::Db)?;

    let mut results = Vec::new();
    for (_key, value) in entries {
        let commitment = decode_signal_commitment_v1(&value).map_err(|_| ExecError::Overflow)?;
        // Filter by height range
        if commitment.height >= start_height && commitment.height <= end_height {
            results.push(commitment);
        }
    }

    Ok(results)
}

// ============================================================================
// NNPX PRIVACY BOUNDARY ENFORCEMENT (Week 22 - D22.4)
// ============================================================================

use novai_state::{is_nnpx_key, nnpx_nullifier_key};

/// Caller context for privacy boundary checks.
///
/// Used to determine if an operation is initiated by an AI entity
/// (which is blocked from NNPX access) or by a human-controlled account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// Human-controlled account (address).
    Account([u8; 32]),
    /// AI entity (entity ID).
    AiEntity([u8; 32]),
}

/// Validate that the caller has permission to access a key.
///
/// # Privacy Boundary (D22.4)
///
/// AI entities are NEVER allowed to directly access NNPX private data.
/// This is a hard security boundary enforced at the execution layer.
///
/// # Errors
///
/// Returns `NnpxAccessDenied` if:
/// - Key starts with `b"nnpx/"` AND caller is an AI entity
#[inline]
pub fn validate_nnpx_access<E>(key: &[u8], caller: &Caller) -> Result<(), ExecError<E>> {
    if is_nnpx_key(key) {
        if let Caller::AiEntity(_) = caller {
            return Err(ExecError::NnpxAccessDenied);
        }
    }
    Ok(())
}

/// Check if a nullifier has already been spent.
///
/// # Errors
///
/// Returns `NullifierAlreadySpent` if the nullifier exists in state.
pub fn validate_nullifier_unspent<K: Kv>(
    db: &K,
    nullifier: &[u8; 32],
) -> Result<(), ExecError<K::Error>> {
    let key = nnpx_nullifier_key(nullifier);
    if db.get(&key).map_err(ExecError::Db)?.is_some() {
        return Err(ExecError::NullifierAlreadySpent);
    }
    Ok(())
}

/// Mark a nullifier as spent by writing it to the nullifier set.
///
/// Returns a `WriteOp` for inclusion in an atomic batch.
#[must_use]
pub fn mark_nullifier_spent(nullifier: &[u8; 32]) -> WriteOp {
    let key = nnpx_nullifier_key(nullifier);
    // Value is empty - presence in the set is what matters
    WriteOp::Put(key, vec![])
}

/// Validate that an AI entity does NOT have NNPX-derived capability.
///
/// # Privacy Invariant
///
/// AI entities must NEVER be registered with `read_nnpx_derived: true`.
/// This function is used during entity registration to enforce this invariant.
///
/// # Errors
///
/// Returns `NnpxAccessDenied` if the entity has `read_nnpx_derived` capability.
#[inline]
#[allow(clippy::missing_const_for_fn)] // Generic functions cannot be const in stable Rust
pub fn validate_ai_entity_no_nnpx_capability<E>(
    capabilities: &novai_ai_entities::Capabilities,
) -> Result<(), ExecError<E>> {
    if capabilities.read_nnpx_derived {
        return Err(ExecError::NnpxAccessDenied);
    }
    Ok(())
}

/// Check if a key is in the NNPX private store.
///
/// Convenience re-export from `novai_state` for use in execution logic.
#[inline]
#[must_use]
pub fn is_private_key(key: &[u8]) -> bool {
    is_nnpx_key(key)
}

// ============================================================================
// DERIVED VIEW ACCESS CONTROL (Week 23 - D23.5)
// ============================================================================

use novai_ai_entities::{
    decode_derived_view_v1, encode_derived_view_v1, DerivedView, DerivedViewDecodeError,
};
use novai_state::{
    derived_view_audit_key, derived_view_by_creator_key, derived_view_by_schema_key,
    derived_view_key, is_derived_view_key, KEY_PREFIX_DERIVED_VIEWS,
};

/// Validate that an AI entity can read derived views.
///
/// # Access Control (D23.5)
///
/// AI entities can ONLY read derived views if they have the `read_nnpx_derived` capability.
/// This capability grants access to privacy-safe aggregates without exposing raw private data.
///
/// # Arguments
///
/// - `entity`: The AI entity attempting to read
///
/// # Errors
///
/// Returns `DerivedViewAccessDenied` if the entity lacks the `read_nnpx_derived` capability.
#[inline]
#[allow(clippy::missing_const_for_fn)] // Generic functions cannot be const in stable Rust
pub fn validate_derived_view_access<E>(
    entity: &novai_ai_entities::AiEntity,
) -> Result<(), ExecError<E>> {
    if !entity.capabilities.read_nnpx_derived {
        return Err(ExecError::DerivedViewAccessDenied);
    }
    Ok(())
}

/// Read a derived view from storage with access control.
///
/// # Access Control (D23.5)
///
/// 1. Validates the AI entity has `read_nnpx_derived` capability
/// 2. Reads the derived view from storage
/// 3. Creates an audit log entry (returned as `WriteOp`)
///
/// # Arguments
///
/// - `db`: Database handle
/// - `entity`: AI entity performing the read
/// - `view_id`: ID of the derived view to read
/// - `current_height`: Current block height (for audit log)
///
/// # Returns
///
/// On success, returns `(DerivedView, WriteOp)` where `WriteOp` is the audit log entry.
///
/// # Errors
///
/// - `DerivedViewAccessDenied`: Entity lacks capability
/// - `DerivedViewNotFound`: View doesn't exist
/// - `Db`: Database error
pub fn read_derived_view_with_audit<K: Kv>(
    db: &K,
    entity: &novai_ai_entities::AiEntity,
    view_id: &[u8; 32],
    current_height: u64,
) -> Result<(DerivedView, WriteOp), ExecError<K::Error>> {
    // Step 1: Validate capability
    validate_derived_view_access(entity)?;

    // Step 2: Read derived view
    let key = derived_view_key(view_id);
    let bytes = db
        .get(&key)
        .map_err(ExecError::Db)?
        .ok_or(ExecError::DerivedViewNotFound)?;

    let view = decode_derived_view_v1(&bytes).map_err(|e| match e {
        DerivedViewDecodeError::InvalidSchemaId { id } => {
            ExecError::InvalidDerivedViewSchema { schema_id: id }
        }
        _ => ExecError::Overflow, // Map other decode errors
    })?;

    // Step 3: Create audit log entry
    let audit_op = create_derived_view_audit_entry(&entity.id, view_id, current_height);

    Ok((view, audit_op))
}

/// Read a derived view without access control (for internal use).
///
/// Use this only for protocol-level operations that don't require capability checks.
///
/// # Errors
///
/// - `DerivedViewNotFound`: View doesn't exist
/// - `Db`: Database error
pub fn read_derived_view<K: Kv>(
    db: &K,
    view_id: &[u8; 32],
) -> Result<Option<DerivedView>, ExecError<K::Error>> {
    let key = derived_view_key(view_id);
    match db.get(&key).map_err(ExecError::Db)? {
        None => Ok(None),
        Some(bytes) => {
            let view = decode_derived_view_v1(&bytes).map_err(|e| match e {
                DerivedViewDecodeError::InvalidSchemaId { id } => {
                    ExecError::InvalidDerivedViewSchema { schema_id: id }
                }
                _ => ExecError::Overflow,
            })?;
            Ok(Some(view))
        }
    }
}

/// Create a `WriteOp` to store a derived view.
///
/// Also creates index entries for schema and creator lookups.
///
/// # Returns
///
/// Vector of `WriteOps` for atomic batch:
/// 1. Primary view storage
/// 2. Schema index entry
/// 3. Creator index entry
#[must_use]
pub fn write_derived_view_ops(view: &DerivedView) -> Vec<WriteOp> {
    let mut ops = Vec::with_capacity(3);

    // Primary storage
    let primary_key = derived_view_key(&view.view_id);
    let encoded = encode_derived_view_v1(view);
    ops.push(WriteOp::Put(primary_key, encoded));

    // Schema index
    let schema_key = derived_view_by_schema_key(view.schema_id, &view.view_id);
    ops.push(WriteOp::Put(schema_key, vec![])); // Presence-only index

    // Creator index
    let creator_key = derived_view_by_creator_key(&view.creator, &view.view_id);
    ops.push(WriteOp::Put(creator_key, vec![])); // Presence-only index

    ops
}

/// Create an audit log entry for a derived view read.
///
/// # Audit Log Format (D23.5)
///
/// Key: `derived_views/audit/{entity_id}/{height}`
/// Value: `{view_id}` (32 bytes)
///
/// This records that the given AI entity read a derived view at the given height.
#[must_use]
pub fn create_derived_view_audit_entry(
    entity_id: &[u8; 32],
    view_id: &[u8; 32],
    height: u64,
) -> WriteOp {
    let key = derived_view_audit_key(entity_id, height);
    WriteOp::Put(key, view_id.to_vec())
}

/// Query derived views by schema ID.
///
/// Returns all derived views with the given schema.
///
/// # Errors
///
/// Returns error if DB read fails or stored data is malformed.
pub fn get_derived_views_by_schema<K: Kv>(
    db: &K,
    schema_id: u32,
) -> Result<Vec<DerivedView>, ExecError<K::Error>> {
    // Build prefix: "derived_views/by_schema/" ++ schema_id_be4 ++ "/"
    let mut prefix = Vec::with_capacity(
        KEY_PREFIX_DERIVED_VIEWS.len() + "by_schema/".len() + 4 + 1,
    );
    prefix.extend_from_slice(b"derived_views/by_schema/");
    prefix.extend_from_slice(&schema_id.to_be_bytes());
    prefix.push(b'/');

    let entries = db.scan_prefix(&prefix).map_err(ExecError::Db)?;

    let mut results = Vec::with_capacity(entries.len());
    for (key, _value) in entries {
        // Extract view_id from key (last 32 bytes)
        if key.len() >= 32 {
            let mut view_id = [0u8; 32];
            view_id.copy_from_slice(&key[key.len() - 32..]);

            // Read the actual view
            if let Some(view) = read_derived_view(db, &view_id)? {
                results.push(view);
            }
        }
    }

    Ok(results)
}

/// Query derived views by creator.
///
/// Returns all derived views created by the given address/entity.
///
/// # Errors
///
/// Returns error if DB read fails or stored data is malformed.
pub fn get_derived_views_by_creator<K: Kv>(
    db: &K,
    creator: &[u8; 32],
) -> Result<Vec<DerivedView>, ExecError<K::Error>> {
    // Build prefix: "derived_views/by_creator/" ++ creator32 ++ "/"
    let mut prefix = Vec::with_capacity(
        KEY_PREFIX_DERIVED_VIEWS.len() + "by_creator/".len() + 32 + 1,
    );
    prefix.extend_from_slice(b"derived_views/by_creator/");
    prefix.extend_from_slice(creator);
    prefix.push(b'/');

    let entries = db.scan_prefix(&prefix).map_err(ExecError::Db)?;

    let mut results = Vec::with_capacity(entries.len());
    for (key, _value) in entries {
        // Extract view_id from key (last 32 bytes)
        if key.len() >= 32 {
            let mut view_id = [0u8; 32];
            view_id.copy_from_slice(&key[key.len() - 32..]);

            // Read the actual view
            if let Some(view) = read_derived_view(db, &view_id)? {
                results.push(view);
            }
        }
    }

    Ok(results)
}

/// Check if a key is a derived view key.
///
/// Convenience re-export for use in execution logic.
#[inline]
#[must_use]
pub fn is_derived_view(key: &[u8]) -> bool {
    is_derived_view_key(key)
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

    // ========================================================================
    // MEMORY OBJECT PAYLOAD TESTS (Week 21 - D21.4)
    // ========================================================================

    #[test]
    fn create_memory_object_payload_roundtrip() {
        use novai_ai_entities::MemoryObjectType;

        let payload = CreateMemoryObjectPayloadV1 {
            object_type: MemoryObjectType::ChainSummary,
            data: b"test chain summary data".to_vec(),
        };

        let encoded = encode_create_memory_object_payload_v1(&payload);
        let decoded = decode_create_memory_object_payload_v1(&encoded).unwrap();

        assert_eq!(decoded, payload);
    }

    #[test]
    fn create_memory_object_payload_all_types() {
        use novai_ai_entities::MemoryObjectType;

        for obj_type in [
            MemoryObjectType::ChainSummary,
            MemoryObjectType::LabelIndex,
            MemoryObjectType::EmbeddingCommitment,
            MemoryObjectType::AnomalyLog,
            MemoryObjectType::StatisticsSnapshot,
        ] {
            let payload = CreateMemoryObjectPayloadV1 {
                object_type: obj_type,
                data: vec![0xAA, 0xBB, 0xCC],
            };

            let encoded = encode_create_memory_object_payload_v1(&payload);
            let decoded = decode_create_memory_object_payload_v1(&encoded).unwrap();

            assert_eq!(decoded.object_type, obj_type);
            assert_eq!(decoded.data, payload.data);
        }
    }

    #[test]
    fn update_memory_object_payload_roundtrip() {
        let payload = UpdateMemoryObjectPayloadV1 {
            object_id: [0xABu8; 32],
            new_data: b"updated data content".to_vec(),
        };

        let encoded = encode_update_memory_object_payload_v1(&payload);
        let decoded = decode_update_memory_object_payload_v1(&encoded).unwrap();

        assert_eq!(decoded, payload);
    }

    #[test]
    fn delete_memory_object_payload_roundtrip() {
        let payload = DeleteMemoryObjectPayloadV1 {
            object_id: [0xCDu8; 32],
        };

        let encoded = encode_delete_memory_object_payload_v1(&payload);
        let decoded = decode_delete_memory_object_payload_v1(&encoded).unwrap();

        assert_eq!(decoded, payload);
    }

    #[test]
    fn create_memory_object_payload_empty_data() {
        use novai_ai_entities::MemoryObjectType;

        let payload = CreateMemoryObjectPayloadV1 {
            object_type: MemoryObjectType::LabelIndex,
            data: vec![], // Empty data
        };

        let encoded = encode_create_memory_object_payload_v1(&payload);
        assert_eq!(encoded.len(), 6); // version(1) + type(1) + data_len(4)

        let decoded = decode_create_memory_object_payload_v1(&encoded).unwrap();
        assert_eq!(decoded.data.len(), 0);
    }

    // ========================================================================
    // MEMORY OBJECT CRUD EXECUTION TESTS (Week 21 - D21.4)
    // ========================================================================

    /// Helper to create an AI entity with memory capabilities.
    fn setup_test_entity_for_memory(db: &mut MemKv) -> AiEntity {
        let mut entity = AiEntity::new(
            [0x42u8; 32], // code_hash
            [0x01u8; 32], // creator
            AutonomyMode::Gated,
            Capabilities::gated(),
            1000, // registered_at
        );
        entity.economic_balance = 1_000_000u128;

        let op = write_ai_entity_op(&entity);
        db.apply_batch(&[op]).unwrap();

        entity
    }

    /// Helper to build a create memory object tx.
    fn mk_create_memory_tx(
        entity_id: [u8; 32],
        nonce: u64,
        fee: u64,
        object_type: novai_ai_entities::MemoryObjectType,
        data: Vec<u8>,
    ) -> TxV1 {
        use novai_types::TxVersion;

        let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
            object_type,
            data,
        });

        TxV1 {
            version: TxVersion::V1,
            from: entity_id,
            pubkey: entity_id,
            nonce,
            fee,
            payload,
            sig: [0u8; 64],
        }
    }

    /// Helper to build an update memory object tx.
    fn mk_update_memory_tx(
        entity_id: [u8; 32],
        nonce: u64,
        fee: u64,
        object_id: [u8; 32],
        new_data: Vec<u8>,
    ) -> TxV1 {
        use novai_types::TxVersion;

        let payload = encode_update_memory_object_payload_v1(&UpdateMemoryObjectPayloadV1 {
            object_id,
            new_data,
        });

        TxV1 {
            version: TxVersion::V1,
            from: entity_id,
            pubkey: entity_id,
            nonce,
            fee,
            payload,
            sig: [0u8; 64],
        }
    }

    /// Helper to build a delete memory object tx.
    fn mk_delete_memory_tx(entity_id: [u8; 32], nonce: u64, fee: u64, object_id: [u8; 32]) -> TxV1 {
        use novai_types::TxVersion;

        let payload =
            encode_delete_memory_object_payload_v1(&DeleteMemoryObjectPayloadV1 { object_id })
                .to_vec();

        TxV1 {
            version: TxVersion::V1,
            from: entity_id,
            pubkey: entity_id,
            nonce,
            fee,
            payload,
            sig: [0u8; 64],
        }
    }

    #[test]
    fn create_memory_object_success() {
        use novai_ai_entities::MemoryObjectType;

        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);

        let tx = mk_create_memory_tx(
            entity.id,
            0, // nonce
            1, // fee
            MemoryObjectType::ChainSummary,
            b"chain summary data".to_vec(),
        );

        let object_id = apply_create_memory_object_tx(&mut db, &tx, 2000).unwrap();

        // Verify object was created
        let obj = read_memory_object(&db, &entity.id, &object_id)
            .unwrap()
            .expect("Object should exist");
        assert_eq!(obj.object_type, MemoryObjectType::ChainSummary);
        assert_eq!(obj.data, b"chain summary data".to_vec());
        assert_eq!(obj.owner_entity, entity.id);
        assert_eq!(obj.created_at, 2000);
        assert_eq!(obj.updated_at, 2000);

        // Verify count was incremented
        let count = read_memory_count(&db, &entity.id).unwrap();
        assert_eq!(count, 1);

        // Verify entity state was updated
        let updated_entity = read_ai_entity(&db, &entity.id).unwrap().unwrap();
        assert_eq!(updated_entity.nonce, 1);
        assert_eq!(updated_entity.last_active_at, 2000);
    }

    #[test]
    fn create_memory_object_size_limit_enforced() {
        use novai_ai_entities::{MemoryObjectType, MAX_MEMORY_OBJECT_SIZE};

        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);

        // Try to create object exceeding size limit
        let oversized_data = vec![0xAAu8; MAX_MEMORY_OBJECT_SIZE + 1];
        let tx = mk_create_memory_tx(
            entity.id,
            0,
            1,
            MemoryObjectType::ChainSummary,
            oversized_data,
        );

        let result = apply_create_memory_object_tx(&mut db, &tx, 2000);
        assert!(matches!(
            result,
            Err(ExecError::MemoryObjectTooLarge { .. })
        ));
    }

    #[test]
    fn create_memory_object_count_limit_enforced() {
        use novai_ai_entities::{MemoryObjectType, MAX_MEMORY_OBJECTS_PER_ENTITY};
        use novai_state::{ai_memory_count_key, encode_memory_count};

        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);

        // Set count to max - this simulates having max objects already
        let count_key = ai_memory_count_key(&entity.id);
        db.put(
            &count_key,
            &encode_memory_count(MAX_MEMORY_OBJECTS_PER_ENTITY),
        )
        .unwrap();

        let tx = mk_create_memory_tx(
            entity.id,
            0,
            1,
            MemoryObjectType::ChainSummary,
            b"data".to_vec(),
        );

        let result = apply_create_memory_object_tx(&mut db, &tx, 2000);
        assert!(matches!(
            result,
            Err(ExecError::MemoryObjectCountExceeded { .. })
        ));
    }

    #[test]
    fn update_memory_object_success() {
        use novai_ai_entities::MemoryObjectType;

        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);

        // Create an object first
        let create_tx = mk_create_memory_tx(
            entity.id,
            0,
            1,
            MemoryObjectType::StatisticsSnapshot,
            b"initial data".to_vec(),
        );
        let object_id = apply_create_memory_object_tx(&mut db, &create_tx, 2000).unwrap();

        // Update the object
        let update_tx = mk_update_memory_tx(
            entity.id,
            1, // nonce incremented
            1,
            object_id,
            b"updated data".to_vec(),
        );
        apply_update_memory_object_tx(&mut db, &update_tx, 3000).unwrap();

        // Verify update
        let obj = read_memory_object(&db, &entity.id, &object_id)
            .unwrap()
            .expect("Object should exist");
        assert_eq!(obj.data, b"updated data".to_vec());
        assert_eq!(obj.created_at, 2000); // unchanged
        assert_eq!(obj.updated_at, 3000); // updated
    }

    #[test]
    fn update_memory_object_not_found() {
        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);

        // Try to update non-existent object
        let update_tx = mk_update_memory_tx(
            entity.id,
            0,
            1,
            [0xFFu8; 32], // non-existent object ID
            b"data".to_vec(),
        );

        let result = apply_update_memory_object_tx(&mut db, &update_tx, 2000);
        assert!(matches!(result, Err(ExecError::MemoryObjectNotFound)));
    }

    #[test]
    fn delete_memory_object_success() {
        use novai_ai_entities::MemoryObjectType;

        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);

        // Create an object first
        let create_tx = mk_create_memory_tx(
            entity.id,
            0,
            1,
            MemoryObjectType::AnomalyLog,
            b"anomaly data".to_vec(),
        );
        let object_id = apply_create_memory_object_tx(&mut db, &create_tx, 2000).unwrap();

        // Verify it exists
        let count_before = read_memory_count(&db, &entity.id).unwrap();
        assert_eq!(count_before, 1);

        // Delete the object
        let delete_tx = mk_delete_memory_tx(
            entity.id, 1, // nonce incremented
            1, object_id,
        );
        apply_delete_memory_object_tx(&mut db, &delete_tx, 3000).unwrap();

        // Verify deletion
        let obj = read_memory_object(&db, &entity.id, &object_id).unwrap();
        assert!(obj.is_none(), "Object should be deleted");

        // Verify count was decremented
        let count_after = read_memory_count(&db, &entity.id).unwrap();
        assert_eq!(count_after, 0);
    }

    #[test]
    fn delete_memory_object_not_found() {
        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);

        // Try to delete non-existent object
        let delete_tx = mk_delete_memory_tx(
            entity.id,
            0,
            1,
            [0xFFu8; 32], // non-existent object ID
        );

        let result = apply_delete_memory_object_tx(&mut db, &delete_tx, 2000);
        assert!(matches!(result, Err(ExecError::MemoryObjectNotFound)));
    }

    #[test]
    fn memory_object_crud_full_lifecycle() {
        use novai_ai_entities::MemoryObjectType;

        let mut db = MemKv::new();
        let entity = setup_test_entity_for_memory(&mut db);
        let mut nonce = 0u64;

        // 1. Create
        let create_tx = mk_create_memory_tx(
            entity.id,
            nonce,
            1,
            MemoryObjectType::EmbeddingCommitment,
            b"embedding v1".to_vec(),
        );
        nonce += 1;
        let object_id = apply_create_memory_object_tx(&mut db, &create_tx, 1000).unwrap();

        // 2. Read
        let obj = read_memory_object(&db, &entity.id, &object_id)
            .unwrap()
            .unwrap();
        assert_eq!(obj.data, b"embedding v1".to_vec());

        // 3. Update
        let update_tx =
            mk_update_memory_tx(entity.id, nonce, 1, object_id, b"embedding v2".to_vec());
        nonce += 1;
        apply_update_memory_object_tx(&mut db, &update_tx, 2000).unwrap();

        let obj = read_memory_object(&db, &entity.id, &object_id)
            .unwrap()
            .unwrap();
        assert_eq!(obj.data, b"embedding v2".to_vec());

        // 4. Delete
        let delete_tx = mk_delete_memory_tx(entity.id, nonce, 1, object_id);
        apply_delete_memory_object_tx(&mut db, &delete_tx, 3000).unwrap();

        let obj = read_memory_object(&db, &entity.id, &object_id).unwrap();
        assert!(obj.is_none());
    }

    // ========================================================================
    // NNPX PRIVACY SECURITY TESTS (Week 22 - D22.5)
    // ========================================================================

    #[test]
    fn ai_cannot_read_nnpx() {
        // D22.5: AI entity operations are rejected for nnpx/ keys

        let ai_entity_id = [0x42u8; 32];
        let account_addr = [0x01u8; 32];

        let ai_caller = Caller::AiEntity(ai_entity_id);
        let account_caller = Caller::Account(account_addr);

        // AI entity trying to access NNPX keys should be denied
        let nnpx_key = b"nnpx/commitments/abc123";
        let result: Result<(), ExecError<()>> = validate_nnpx_access(nnpx_key, &ai_caller);
        assert!(
            matches!(result, Err(ExecError::NnpxAccessDenied)),
            "AI entity must be denied access to NNPX keys"
        );

        // Human account can access NNPX keys
        let result: Result<(), ExecError<()>> = validate_nnpx_access(nnpx_key, &account_caller);
        assert!(
            result.is_ok(),
            "Human account should be allowed to access NNPX keys"
        );

        // AI entity can access public keys
        let public_key = b"accounts/alice";
        let result: Result<(), ExecError<()>> = validate_nnpx_access(public_key, &ai_caller);
        assert!(result.is_ok(), "AI entity can access public keys");

        // Test various NNPX prefixes
        for key in [
            b"nnpx/".as_slice(),
            b"nnpx/nullifiers/null1",
            b"nnpx/encrypted/payload1",
            b"nnpx/commitments/commit1",
        ] {
            let result: Result<(), ExecError<()>> = validate_nnpx_access(key, &ai_caller);
            assert!(
                matches!(result, Err(ExecError::NnpxAccessDenied)),
                "AI entity must be denied access to all NNPX keys"
            );
        }
    }

    #[test]
    fn commitment_hides_payload() {
        // D22.5: Same payload with different encryption produces different commitments
        use novai_ai_entities::PrivatePayloadCommitment;

        // Simulate same logical payload encrypted with different randomness
        let payload_encrypted_v1 = b"encrypted_with_random_nonce_1";
        let payload_encrypted_v2 = b"encrypted_with_random_nonce_2";

        let hash1 = PrivatePayloadCommitment::compute_commitment_hash(payload_encrypted_v1);
        let hash2 = PrivatePayloadCommitment::compute_commitment_hash(payload_encrypted_v2);

        // Different encrypted payloads must produce different commitments
        // This is the "hiding" property - observer cannot determine original content
        assert_ne!(
            hash1, hash2,
            "Different encryptions must produce different commitments (hiding property)"
        );

        // Same payload must produce same commitment (deterministic)
        let hash1_again = PrivatePayloadCommitment::compute_commitment_hash(payload_encrypted_v1);
        assert_eq!(
            hash1, hash1_again,
            "Same payload must produce same commitment (deterministic)"
        );
    }

    #[test]
    fn nullifier_prevents_reuse() {
        // D22.5: Duplicate nullifiers are detected and rejected
        use novai_ai_entities::PrivatePayloadCommitment;

        let mut db = MemKv::new();

        let spending_secret = [0xABu8; 32];
        let counter = 42u64;

        // Compute nullifier
        let nullifier = PrivatePayloadCommitment::compute_nullifier(&spending_secret, counter);

        // First spend: nullifier is unspent
        let result = validate_nullifier_unspent(&db, &nullifier);
        assert!(result.is_ok(), "First spend should succeed");

        // Mark as spent
        let spend_op = mark_nullifier_spent(&nullifier);
        db.apply_batch(&[spend_op]).unwrap();

        // Second spend: nullifier is already spent (double-spend attempt)
        let result = validate_nullifier_unspent(&db, &nullifier);
        assert!(
            matches!(result, Err(ExecError::NullifierAlreadySpent)),
            "Double-spend must be rejected"
        );

        // Different counter = different nullifier = valid new spend
        let nullifier2 = PrivatePayloadCommitment::compute_nullifier(&spending_secret, counter + 1);
        let result = validate_nullifier_unspent(&db, &nullifier2);
        assert!(
            result.is_ok(),
            "Different nullifier (new spend) should succeed"
        );
    }

    #[test]
    fn ai_entity_cannot_have_nnpx_capability() {
        // D22.4: AI entities must not be registered with read_nnpx_derived capability

        // Valid AI capabilities (no NNPX access)
        let valid_caps = Capabilities::gated();
        assert!(!valid_caps.read_nnpx_derived);
        let result: Result<(), ExecError<()>> = validate_ai_entity_no_nnpx_capability(&valid_caps);
        assert!(result.is_ok(), "AI without NNPX capability should pass");

        // Invalid: AI trying to get NNPX access
        let mut invalid_caps = Capabilities::gated();
        invalid_caps.read_nnpx_derived = true;
        let result: Result<(), ExecError<()>> =
            validate_ai_entity_no_nnpx_capability(&invalid_caps);
        assert!(
            matches!(result, Err(ExecError::NnpxAccessDenied)),
            "AI with NNPX capability must be rejected"
        );
    }

    #[test]
    fn is_private_key_identifies_nnpx_keys() {
        // Verify the is_private_key helper works correctly

        // NNPX keys are private
        assert!(is_private_key(b"nnpx/"));
        assert!(is_private_key(b"nnpx/commitments/abc"));
        assert!(is_private_key(b"nnpx/nullifiers/xyz"));
        assert!(is_private_key(b"nnpx/encrypted/data"));

        // Non-NNPX keys are public
        assert!(!is_private_key(b"accounts/alice"));
        assert!(!is_private_key(b"ai/entities/entity1"));
        assert!(!is_private_key(b"consensus/blocks/100"));
        assert!(!is_private_key(b"governance/proposals/prop1"));
    }

    #[test]
    fn nullifier_storage_key_format() {
        // Verify nullifier keys are stored in the NNPX namespace
        let nullifier = [0xABu8; 32];
        let key = nnpx_nullifier_key(&nullifier);

        // Must be an NNPX key
        assert!(
            is_private_key(&key),
            "Nullifier key must be in NNPX namespace"
        );

        // Must start with correct prefix
        assert!(
            key.starts_with(b"nnpx/nullifiers/"),
            "Nullifier key must have correct prefix"
        );
    }

    #[test]
    fn caller_enum_equality() {
        let ai1 = Caller::AiEntity([0x01u8; 32]);
        let ai2 = Caller::AiEntity([0x02u8; 32]);
        let ai1_copy = Caller::AiEntity([0x01u8; 32]);
        let account = Caller::Account([0x01u8; 32]);

        assert_eq!(ai1, ai1_copy, "Same AI entity should be equal");
        assert_ne!(ai1, ai2, "Different AI entities should not be equal");
        assert_ne!(ai1, account, "AI entity and account should not be equal");
    }

    // ========================================================================
    // DERIVED VIEWS BOUNDARY TESTS (Week 23 - D23.6)
    // ========================================================================

    use novai_ai_entities::{AggregateVolumeData, DerivedSourceType, DerivedView};

    /// Helper: Create an AI entity with derived view read capability.
    fn create_entity_with_derived_capability() -> novai_ai_entities::AiEntity {
        let code_hash = [0xAAu8; 32];
        let creator = [0xBBu8; 32];
        let caps = Capabilities {
            read_nnpx_derived: true,
            read_public_chain: true,
            ..Capabilities::default()
        };

        novai_ai_entities::AiEntity::new(
            code_hash,
            creator,
            AutonomyMode::Advisory,
            caps,
            1000,
        )
    }

    /// Helper: Create an AI entity WITHOUT derived view read capability.
    fn create_entity_without_derived_capability() -> novai_ai_entities::AiEntity {
        let code_hash = [0xCCu8; 32];
        let creator = [0xDDu8; 32];
        let caps = Capabilities {
            read_nnpx_derived: false,
            read_public_chain: true,
            ..Capabilities::default()
        };

        novai_ai_entities::AiEntity::new(
            code_hash,
            creator,
            AutonomyMode::Advisory,
            caps,
            1000,
        )
    }

    /// Helper: Create a test derived view.
    fn create_test_derived_view(creator: [u8; 32]) -> DerivedView {
        let data = AggregateVolumeData {
            start_height: 100,
            end_height: 200,
            total_volume: 1_000_000,
        }
        .encode();

        DerivedView::new(
            DerivedSourceType::ChainAggregate,
            1, // AggregateVolume schema
            1000,
            creator,
            data,
        )
        .expect("Valid derived view")
    }

    #[test]
    fn ai_reads_derived_view_with_capability() {
        // D23.6: AI entity with read_nnpx_derived capability can read derived views
        let mut db = MemKv::new();

        // Create an entity WITH derived view capability
        let entity = create_entity_with_derived_capability();
        assert!(
            entity.capabilities.read_nnpx_derived,
            "Test entity should have capability"
        );

        // Create and store a derived view
        let creator = [0x42u8; 32];
        let view = create_test_derived_view(creator);
        let view_id = view.view_id;

        // Store the view
        let ops = write_derived_view_ops(&view);
        db.apply_batch(&ops).unwrap();

        // Entity with capability can read the view
        let result = read_derived_view_with_audit(&db, &entity, &view_id, 2000);
        assert!(result.is_ok(), "Entity with capability should read view");

        let (read_view, audit_op) = result.unwrap();
        assert_eq!(read_view.view_id, view.view_id);
        assert_eq!(read_view.schema_id, 1);
        assert_eq!(read_view.source_type, DerivedSourceType::ChainAggregate);

        // Verify audit op was created
        match audit_op {
            WriteOp::Put(key, value) => {
                assert!(
                    key.starts_with(b"derived_views/audit/"),
                    "Audit key should have correct prefix"
                );
                assert_eq!(value.len(), 32, "Audit value should be view_id");
            }
            WriteOp::Delete(_) => panic!("Audit should be a Put, not Delete"),
        }
    }

    #[test]
    fn ai_cannot_read_derived_view_without_capability() {
        // D23.6: AI entity WITHOUT read_nnpx_derived capability is denied
        let mut db = MemKv::new();

        // Create an entity WITHOUT derived view capability
        let entity = create_entity_without_derived_capability();
        assert!(
            !entity.capabilities.read_nnpx_derived,
            "Test entity should NOT have capability"
        );

        // Create and store a derived view
        let creator = [0x42u8; 32];
        let view = create_test_derived_view(creator);
        let view_id = view.view_id;

        let ops = write_derived_view_ops(&view);
        db.apply_batch(&ops).unwrap();

        // Entity without capability is denied
        let result = read_derived_view_with_audit(&db, &entity, &view_id, 2000);
        assert!(
            matches!(result, Err(ExecError::DerivedViewAccessDenied)),
            "Entity without capability should be denied"
        );
    }

    #[test]
    fn ai_cannot_read_raw_nnpx_even_with_derived_capability() {
        // D23.6: AI entity with read_nnpx_derived still cannot read raw NNPX data
        let entity = create_entity_with_derived_capability();
        assert!(
            entity.capabilities.read_nnpx_derived,
            "Entity should have derived capability"
        );

        let ai_caller = Caller::AiEntity(entity.id);

        // AI entity is still blocked from raw NNPX keys
        let nnpx_key = b"nnpx/commitments/secret123";
        let result: Result<(), ExecError<()>> = validate_nnpx_access(nnpx_key, &ai_caller);
        assert!(
            matches!(result, Err(ExecError::NnpxAccessDenied)),
            "AI must STILL be blocked from raw NNPX even with derived capability"
        );

        // But derived_views/ keys are NOT nnpx keys
        let derived_key = b"derived_views/view123";
        assert!(!is_private_key(derived_key), "Derived views are not NNPX");
        assert!(is_derived_view(derived_key), "Should be a derived view key");
    }

    #[test]
    fn derived_view_schema_validated() {
        // D23.6: Invalid schema is rejected
        let creator = [0x42u8; 32];

        // Valid schema (AggregateVolume = 1)
        let valid_data = AggregateVolumeData {
            start_height: 0,
            end_height: 100,
            total_volume: 0,
        }
        .encode();

        let view = DerivedView::new(
            DerivedSourceType::ChainAggregate,
            1, // Valid schema
            1000,
            creator,
            valid_data.clone(),
        );
        assert!(view.is_some(), "Valid schema should create view");

        // Invalid schema ID (99 doesn't exist)
        let view = DerivedView::new(
            DerivedSourceType::ChainAggregate,
            99, // Invalid schema
            1000,
            creator,
            valid_data,
        );
        assert!(view.is_none(), "Invalid schema should fail");

        // Invalid data length for schema
        let wrong_length_data = vec![0u8; 10]; // AggregateVolume needs 32 bytes
        let view = DerivedView::new(
            DerivedSourceType::ChainAggregate,
            1,
            1000,
            creator,
            wrong_length_data,
        );
        assert!(view.is_none(), "Wrong data length should fail");
    }

    #[test]
    fn derived_view_not_found_returns_error() {
        let db = MemKv::new();
        let entity = create_entity_with_derived_capability();

        // Try to read non-existent view
        let missing_id = [0xFFu8; 32];
        let result = read_derived_view_with_audit(&db, &entity, &missing_id, 1000);
        assert!(
            matches!(result, Err(ExecError::DerivedViewNotFound)),
            "Missing view should return NotFound error"
        );
    }

    #[test]
    fn derived_view_write_creates_indices() {
        let mut db = MemKv::new();

        let creator = [0x42u8; 32];
        let view = create_test_derived_view(creator);

        // Write the view
        let ops = write_derived_view_ops(&view);
        assert_eq!(ops.len(), 3, "Should create 3 WriteOps (primary + 2 indices)");

        db.apply_batch(&ops).unwrap();

        // Verify primary storage
        let primary_key = derived_view_key(&view.view_id);
        assert!(
            db.get(&primary_key).unwrap().is_some(),
            "Primary storage should exist"
        );

        // Verify schema index
        let schema_key = derived_view_by_schema_key(view.schema_id, &view.view_id);
        assert!(
            db.get(&schema_key).unwrap().is_some(),
            "Schema index should exist"
        );

        // Verify creator index
        let creator_key = derived_view_by_creator_key(&view.creator, &view.view_id);
        assert!(
            db.get(&creator_key).unwrap().is_some(),
            "Creator index should exist"
        );
    }

    #[test]
    fn query_derived_views_by_schema() {
        let mut db = MemKv::new();

        let creator = [0x42u8; 32];

        // Create multiple views with same schema
        let view1 = create_test_derived_view(creator);

        // Create another view with different created_at to get different ID
        let data2 = AggregateVolumeData {
            start_height: 200,
            end_height: 300,
            total_volume: 2_000_000,
        }
        .encode();
        let view2 = DerivedView::new(
            DerivedSourceType::ChainAggregate,
            1, // Same schema
            2000,
            creator,
            data2,
        )
        .unwrap();

        // Store both views
        db.apply_batch(&write_derived_view_ops(&view1)).unwrap();
        db.apply_batch(&write_derived_view_ops(&view2)).unwrap();

        // Query by schema
        let results = get_derived_views_by_schema(&db, 1).unwrap();
        assert_eq!(results.len(), 2, "Should find 2 views with schema 1");

        // Query non-existent schema
        let results = get_derived_views_by_schema(&db, 99).unwrap();
        assert_eq!(results.len(), 0, "Should find 0 views with schema 99");
    }

    #[test]
    fn query_derived_views_by_creator() {
        let mut db = MemKv::new();

        let creator1 = [0x42u8; 32];
        let creator2 = [0x43u8; 32];

        // Create views from different creators
        let view1 = create_test_derived_view(creator1);

        let data2 = AggregateVolumeData {
            start_height: 200,
            end_height: 300,
            total_volume: 2_000_000,
        }
        .encode();
        let view2 = DerivedView::new(
            DerivedSourceType::UserAuthorized,
            1,
            2000,
            creator2, // Different creator
            data2,
        )
        .unwrap();

        // Store both views
        db.apply_batch(&write_derived_view_ops(&view1)).unwrap();
        db.apply_batch(&write_derived_view_ops(&view2)).unwrap();

        // Query by creator1
        let results = get_derived_views_by_creator(&db, &creator1).unwrap();
        assert_eq!(results.len(), 1, "Should find 1 view from creator1");
        assert_eq!(results[0].creator, creator1);

        // Query by creator2
        let results = get_derived_views_by_creator(&db, &creator2).unwrap();
        assert_eq!(results.len(), 1, "Should find 1 view from creator2");
        assert_eq!(results[0].creator, creator2);

        // Query by non-existent creator
        let creator3 = [0x44u8; 32];
        let results = get_derived_views_by_creator(&db, &creator3).unwrap();
        assert_eq!(results.len(), 0, "Should find 0 views from creator3");
    }

    #[test]
    fn validate_derived_view_access_function() {
        // Test the access validation function directly
        let entity_with = create_entity_with_derived_capability();
        let entity_without = create_entity_without_derived_capability();

        let result: Result<(), ExecError<()>> = validate_derived_view_access(&entity_with);
        assert!(result.is_ok(), "Entity with capability should pass");

        let result: Result<(), ExecError<()>> = validate_derived_view_access(&entity_without);
        assert!(
            matches!(result, Err(ExecError::DerivedViewAccessDenied)),
            "Entity without capability should fail"
        );
    }

    #[test]
    fn is_derived_view_function() {
        // Test the is_derived_view helper
        assert!(is_derived_view(b"derived_views/"));
        assert!(is_derived_view(b"derived_views/view123"));
        assert!(is_derived_view(b"derived_views/audit/entity/100"));

        assert!(!is_derived_view(b"nnpx/"));
        assert!(!is_derived_view(b"accounts/"));
        assert!(!is_derived_view(b"ai/entities/"));
    }

    #[test]
    fn all_schemas_are_valid() {
        // Verify all predefined schemas can create views
        let creator = [0x42u8; 32];

        // Schema 1: AggregateVolume
        let data1 = AggregateVolumeData {
            start_height: 0,
            end_height: 100,
            total_volume: 0,
        }
        .encode();
        assert!(DerivedView::new(DerivedSourceType::ChainAggregate, 1, 0, creator, data1).is_some());

        // Schema 2: ActivityCount
        let data2 = novai_ai_entities::ActivityCountData {
            start_height: 0,
            end_height: 100,
            tx_count: 0,
        }
        .encode();
        assert!(DerivedView::new(DerivedSourceType::ChainAggregate, 2, 0, creator, data2).is_some());

        // Schema 3: PoolSize
        let data3 = novai_ai_entities::PoolSizeData {
            snapshot_height: 0,
            pool_size: 0,
        }
        .encode();
        assert!(DerivedView::new(DerivedSourceType::ChainAggregate, 3, 0, creator, data3).is_some());
    }
}
