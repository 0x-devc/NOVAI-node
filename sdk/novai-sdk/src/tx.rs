//! Transaction builders for all 10 NOVAI transaction types.
//!
//! Each builder constructs a fully signed `TxV1` ready for submission.
//! The caller provides the signing key, nonce, fee, and type-specific fields.

use crate::error::Error;
use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_ai_entities::{AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObjectType};
use novai_codec::{encode_tx_v1_signed, txid_v1};
use novai_crypto::{address_from_pubkey, sign_tx_v1};
use novai_types::{Address, TxId, TxV1, TxVersion};

/// Build and sign a transaction from a signing key and payload.
fn build_signed(sk: &SigningKey, nonce: u64, fee: u64, payload: Vec<u8>) -> Result<TxV1, Error> {
    let pk = sk.verifying_key();
    let from = address_from_pubkey(&pk);

    let mut tx = TxV1 {
        version: TxVersion::V1,
        from,
        pubkey: pk.to_bytes(),
        nonce,
        fee,
        payload,
        sig: [0u8; 64],
    };

    sign_tx_v1(sk, &mut tx)?;
    Ok(tx)
}

/// Compute the transaction ID (blake3 hash of unsigned bytes).
///
/// # Errors
///
/// Returns error if encoding fails.
pub fn compute_txid(tx: &TxV1) -> Result<TxId, Error> {
    txid_v1(tx).map_err(Into::into)
}

/// Encode a signed transaction to bytes for RPC submission.
///
/// # Errors
///
/// Returns error if encoding fails.
pub fn encode_signed(tx: &TxV1) -> Result<Vec<u8>, Error> {
    encode_tx_v1_signed(tx).map_err(Into::into)
}

/// Compute the deterministic AI entity ID from code hash and creator address.
///
/// `entity_id = blake3("NOVAI_AI_ENTITY_ID_V1" || code_hash || creator)`
#[must_use]
pub fn compute_entity_id(code_hash: &[u8; 32], creator: &Address) -> [u8; 32] {
    AiEntity::compute_id(code_hash, creator)
}

// ============================================================================
// Type 1: Transfer
// ============================================================================

/// Build a transfer transaction.
///
/// Payload: `[0x01][to:32][amount:8 BE]` (41 bytes)
///
/// # Errors
///
/// Returns error if signing fails.
pub fn transfer(
    sk: &SigningKey,
    nonce: u64,
    fee: u64,
    to: &Address,
    amount: u64,
) -> Result<TxV1, Error> {
    let mut payload = Vec::with_capacity(41);
    payload.push(1);
    payload.extend_from_slice(to);
    payload.extend_from_slice(&amount.to_be_bytes());
    build_signed(sk, nonce, fee, payload)
}

// ============================================================================
// Type 2: Signal Commitment
// ============================================================================

/// Build a signal commitment transaction.
///
/// Payload: `[0x02][signal_hash:32][signal_type:1][issuer_entity_id:32]` (66 bytes)
///
/// # Errors
///
/// Returns error if signing fails.
pub fn signal_commitment(
    sk: &SigningKey,
    nonce: u64,
    fee: u64,
    signal_hash: &[u8; 32],
    signal_type: AiSignalType,
    issuer_entity_id: &[u8; 32],
) -> Result<TxV1, Error> {
    let mut payload = Vec::with_capacity(66);
    payload.push(2);
    payload.extend_from_slice(signal_hash);
    payload.push(signal_type.to_byte());
    payload.extend_from_slice(issuer_entity_id);
    build_signed(sk, nonce, fee, payload)
}

// ============================================================================
// Type 3: Create Memory Object
// ============================================================================

/// Build a create-memory-object transaction.
///
/// Payload: `[0x03][object_type:1][data_len:4 BE][data:N]`
///
/// # Errors
///
/// Returns error if data is too large (> u32::MAX) or signing fails.
pub fn create_memory(
    sk: &SigningKey,
    nonce: u64,
    fee: u64,
    object_type: MemoryObjectType,
    data: &[u8],
) -> Result<TxV1, Error> {
    let data_len = u32::try_from(data.len())
        .map_err(|_| Error::InvalidArgument(format!("data too large: {} bytes", data.len())))?;

    let mut payload = Vec::with_capacity(6 + data.len());
    payload.push(3);
    payload.push(object_type.to_byte());
    payload.extend_from_slice(&data_len.to_be_bytes());
    payload.extend_from_slice(data);
    build_signed(sk, nonce, fee, payload)
}

// ============================================================================
// Type 4: Update Memory Object
// ============================================================================

/// Build an update-memory-object transaction.
///
/// Payload: `[0x04][object_id:32][data_len:4 BE][new_data:N]`
///
/// # Errors
///
/// Returns error if data is too large or signing fails.
pub fn update_memory(
    sk: &SigningKey,
    nonce: u64,
    fee: u64,
    object_id: &[u8; 32],
    data: &[u8],
) -> Result<TxV1, Error> {
    let data_len = u32::try_from(data.len())
        .map_err(|_| Error::InvalidArgument(format!("data too large: {} bytes", data.len())))?;

    let mut payload = Vec::with_capacity(37 + data.len());
    payload.push(4);
    payload.extend_from_slice(object_id);
    payload.extend_from_slice(&data_len.to_be_bytes());
    payload.extend_from_slice(data);
    build_signed(sk, nonce, fee, payload)
}

// ============================================================================
// Type 5: Delete Memory Object
// ============================================================================

/// Build a delete-memory-object transaction.
///
/// Payload: `[0x05][object_id:32]` (33 bytes)
///
/// # Errors
///
/// Returns error if signing fails.
pub fn delete_memory(
    sk: &SigningKey,
    nonce: u64,
    fee: u64,
    object_id: &[u8; 32],
) -> Result<TxV1, Error> {
    let mut payload = Vec::with_capacity(33);
    payload.push(5);
    payload.extend_from_slice(object_id);
    build_signed(sk, nonce, fee, payload)
}

// ============================================================================
// Type 6: Submit Governance Proposal
// ============================================================================

/// Build a submit-governance-proposal transaction.
///
/// Payload: `[0x06][proposal_type:1][gate_id:32][data_len:4 BE][proposal_data:N]`
///
/// # Errors
///
/// Returns error if data is too large or signing fails.
pub fn submit_proposal(
    sk: &SigningKey,
    nonce: u64,
    fee: u64,
    proposal_type: u8,
    gate_id: &[u8; 32],
    proposal_data: &[u8],
) -> Result<TxV1, Error> {
    let data_len = u32::try_from(proposal_data.len()).map_err(|_| {
        Error::InvalidArgument(format!(
            "proposal data too large: {} bytes",
            proposal_data.len()
        ))
    })?;

    let mut payload = Vec::with_capacity(38 + proposal_data.len());
    payload.push(6);
    payload.push(proposal_type);
    payload.extend_from_slice(gate_id);
    payload.extend_from_slice(&data_len.to_be_bytes());
    payload.extend_from_slice(proposal_data);
    build_signed(sk, nonce, fee, payload)
}

// ============================================================================
// Type 7: Execute Governance Proposal
// ============================================================================

/// Build an execute-governance-proposal transaction.
///
/// Payload: `[0x07][proposal_id:32]` (33 bytes)
///
/// # Errors
///
/// Returns error if signing fails.
pub fn execute_proposal(
    sk: &SigningKey,
    nonce: u64,
    fee: u64,
    proposal_id: &[u8; 32],
) -> Result<TxV1, Error> {
    let mut payload = Vec::with_capacity(33);
    payload.push(7);
    payload.extend_from_slice(proposal_id);
    build_signed(sk, nonce, fee, payload)
}

// ============================================================================
// Type 8: Register AI Entity
// ============================================================================

/// Build a register-AI-entity transaction (no entity key).
///
/// Payload: `[0x08][code_hash:32][autonomy:1][capabilities:1][initial_balance:16 BE]` (51 bytes)
///
/// Returns the signed transaction. Use [`compute_entity_id`] to predict the entity ID.
///
/// # Errors
///
/// Returns error if signing fails.
pub fn register_ai_entity(
    sk: &SigningKey,
    nonce: u64,
    fee: u64,
    code_hash: &[u8; 32],
    autonomy: AutonomyMode,
    capabilities: Capabilities,
    initial_balance: u128,
) -> Result<TxV1, Error> {
    let mut payload = Vec::with_capacity(51);
    payload.push(8);
    payload.extend_from_slice(code_hash);
    payload.push(autonomy.to_byte());
    payload.push(capabilities.to_byte());
    payload.extend_from_slice(&initial_balance.to_be_bytes());
    build_signed(sk, nonce, fee, payload)
}

// ============================================================================
// Type 9: Credit AI Entity
// ============================================================================

/// Build a credit-AI-entity transaction.
///
/// Payload: `[0x09][entity_id:32][amount:16 BE]` (49 bytes)
///
/// # Errors
///
/// Returns error if signing fails.
pub fn credit_ai_entity(
    sk: &SigningKey,
    nonce: u64,
    fee: u64,
    entity_id: &[u8; 32],
    amount: u128,
) -> Result<TxV1, Error> {
    let mut payload = Vec::with_capacity(49);
    payload.push(9);
    payload.extend_from_slice(entity_id);
    payload.extend_from_slice(&amount.to_be_bytes());
    build_signed(sk, nonce, fee, payload)
}

// ============================================================================
// Type 10: Register AI Entity with Key
// ============================================================================

/// Build a register-AI-entity-with-key transaction.
///
/// Payload: `[0x0A][code_hash:32][pubkey:32][autonomy:1][capabilities:1][initial_balance:16 BE]` (83 bytes)
///
/// # Errors
///
/// Returns error if signing fails.
#[allow(clippy::too_many_arguments)]
pub fn register_ai_entity_with_key(
    sk: &SigningKey,
    nonce: u64,
    fee: u64,
    code_hash: &[u8; 32],
    entity_pubkey: &VerifyingKey,
    autonomy: AutonomyMode,
    capabilities: Capabilities,
    initial_balance: u128,
) -> Result<TxV1, Error> {
    let mut payload = Vec::with_capacity(83);
    payload.push(10);
    payload.extend_from_slice(code_hash);
    payload.extend_from_slice(entity_pubkey.as_bytes());
    payload.push(autonomy.to_byte());
    payload.push(capabilities.to_byte());
    payload.extend_from_slice(&initial_balance.to_be_bytes());
    build_signed(sk, nonce, fee, payload)
}
