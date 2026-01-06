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
    account_key, decode_account_v1, decode_fee_pool_v1, encode_account_v1, encode_fee_pool_v1,
    AccountStateV1, FeePoolV1, Kv, StateDecodeError, KEY_FEE_POOL,
};
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
    NonceOverflow, // NEW: explicit nonce overflow error
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

fn write_account<K: Kv>(
    db: &mut K,
    addr: &Address,
    a: &AccountStateV1,
) -> Result<(), ExecError<K::Error>> {
    let k = account_key(addr);
    let enc = encode_account_v1(a);
    db.put(&k, &enc).map_err(ExecError::Db)
}

fn read_fee_pool_or_default<K: Kv>(db: &K) -> Result<FeePoolV1, ExecError<K::Error>> {
    match db.get(KEY_FEE_POOL).map_err(ExecError::Db)? {
        None => Ok(FeePoolV1 { balance: 0 }),
        Some(bytes) => Ok(decode_fee_pool_v1(&bytes)?),
    }
}

fn write_fee_pool<K: Kv>(db: &mut K, p: &FeePoolV1) -> Result<(), ExecError<K::Error>> {
    let enc = encode_fee_pool_v1(p);
    db.put(KEY_FEE_POOL, &enc).map_err(ExecError::Db)
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
pub fn apply_tx_v1_transfer<K: Kv>(db: &mut K, tx: &TxV1) -> Result<(), ExecError<K::Error>> {
    // Decode payload (deterministic).
    let payload = decode_transfer_payload_v1(&tx.payload).map_err(|e| match e {
        ExecError::BadPayloadLength { expected, got } => {
            ExecError::BadPayloadLength { expected, got }
        }
        ExecError::BadPayloadVersion { expected, got } => {
            ExecError::BadPayloadVersion { expected, got }
        }
        _ => ExecError::Overflow, // unreachable for decode, but keep total match exhaustive
    })?;

    let mut from_acct = read_account_or_default(db, &tx.from)?;
    if tx.nonce != from_acct.nonce {
        return Err(ExecError::NonceMismatch {
            expected: from_acct.nonce,
            got: tx.nonce,
        });
    }

    let mut to_acct = read_account_or_default(db, &payload.to)?;
    let mut fee_pool = read_fee_pool_or_default(db)?;

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

    write_account(db, &tx.from, &from_acct)?;
    write_account(db, &payload.to, &to_acct)?;
    write_fee_pool(db, &fee_pool)?;

    Ok(())
}
