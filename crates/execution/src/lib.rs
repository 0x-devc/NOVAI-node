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
/// [version:1][to:32][amount_be:8]
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
        ExecError::Decode(e)
    }
}

/// Deterministically decode a transfer payload from `tx.payload`.
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
pub fn encode_transfer_payload_v1(p: &TransferPayloadV1) -> [u8; 1 + 32 + 8] {
    let mut out = [0u8; 1 + 32 + 8];
    out[0] = TRANSFER_PAYLOAD_V1;
    out[1..33].copy_from_slice(&p.to);
    out[33..41].copy_from_slice(&p.amount.to_be_bytes());
    out
}

fn u64_to_u128_checked(x: u64) -> Result<u128, ExecError<()>> {
    Ok(u128::from(x))
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

/// Store adapter: reads existing nodes from Kv, buffers writes as WriteOp::Put.
/// Deterministic: uses Vec + linear search (no HashMap).
struct SmtOverlayStore<'a, K: Kv> {
    db: &'a K,
    pending: Vec<(Vec<u8>, Vec<u8>)>, // (db_key, value_bytes)
}

impl<'a, K: Kv> SmtOverlayStore<'a, K> {
    fn new(db: &'a K) -> Self {
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

impl<'a, K: Kv> SmtStore for SmtOverlayStore<'a, K> {
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

/// Apply a single TxV1 as a TransferPayloadV1 against the account state machine.
///
/// Rules (Week 3):
/// - Nonce exact match.
/// - Sender balance >= amount + fee.
/// - Checked arithmetic only.
/// - Debit sender by (amount + fee), credit receiver by amount.
/// - Increment sender nonce by 1.
/// - Add fee to fee_pool.
///
/// ATOMIC: All state changes are applied in a single batch (all-or-nothing).
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
    let amount_u128 = u64_to_u128_checked(payload.amount).map_err(|_| ExecError::Overflow)?;
    let fee_u128 = u64_to_u128_checked(tx.fee).map_err(|_| ExecError::Overflow)?;

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