//! Transaction builders for all 11 NOVAI transaction types.
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

// ============================================================================
// Type 11: Entity Upgrade (Week 34)
// ============================================================================

/// Build an entity-upgrade transaction (creator-only, swaps an entity's `code_hash`).
///
/// Payload: `[0x0B][entity_id:32][new_code_hash:32][reason_hash:32]` (97 bytes)
///
/// `reason_hash` may be `None` to omit any off-chain reason commitment (encoded
/// as 32 zero bytes on the wire, matching the Python SDK default).
///
/// The chain enforces creator-only, a per-entity cooldown of
/// `MIN_UPGRADE_INTERVAL_BLOCKS = 1000`, and rejects `new_code_hash` equal to
/// the entity's current code hash (not validated client-side; submission will
/// fail).
///
/// # Errors
///
/// Returns error if signing fails.
pub fn entity_upgrade(
    sk: &SigningKey,
    nonce: u64,
    fee: u64,
    entity_id: &[u8; 32],
    new_code_hash: &[u8; 32],
    reason_hash: Option<&[u8; 32]>,
) -> Result<TxV1, Error> {
    let mut payload = Vec::with_capacity(97);
    payload.push(11);
    payload.extend_from_slice(entity_id);
    payload.extend_from_slice(new_code_hash);
    match reason_hash {
        Some(r) => payload.extend_from_slice(r),
        None => payload.extend_from_slice(&[0u8; 32]),
    }
    build_signed(sk, nonce, fee, payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    /// Golden vector: layout must match the Python
    /// `build_entity_upgrade_payload` byte-for-byte (Week 34, 97 bytes).
    #[test]
    fn entity_upgrade_payload_with_reason() {
        let sk = test_signing_key();
        let entity_id = [0xAAu8; 32];
        let new_code_hash = [0xBBu8; 32];
        let reason_hash = [0xCCu8; 32];

        let tx = entity_upgrade(
            &sk,
            0,
            5_000,
            &entity_id,
            &new_code_hash,
            Some(&reason_hash),
        )
        .expect("entity_upgrade should build");

        assert_eq!(tx.payload.len(), 97, "EntityUpgrade payload is 97 bytes");
        assert_eq!(tx.payload[0], 0x0B, "tx type byte is 11");
        assert_eq!(&tx.payload[1..33], &entity_id[..]);
        assert_eq!(&tx.payload[33..65], &new_code_hash[..]);
        assert_eq!(&tx.payload[65..97], &reason_hash[..]);
    }

    /// `None` reason must encode as 32 zero bytes (parity with the Python
    /// default branch in `build_entity_upgrade_payload`).
    #[test]
    fn entity_upgrade_payload_default_reason_is_zero() {
        let sk = test_signing_key();
        let entity_id = [0x11u8; 32];
        let new_code_hash = [0x22u8; 32];

        let tx = entity_upgrade(&sk, 0, 5_000, &entity_id, &new_code_hash, None)
            .expect("entity_upgrade should build");

        assert_eq!(tx.payload.len(), 97);
        assert_eq!(tx.payload[0], 0x0B);
        assert_eq!(&tx.payload[1..33], &entity_id[..]);
        assert_eq!(&tx.payload[33..65], &new_code_hash[..]);
        assert_eq!(&tx.payload[65..97], &[0u8; 32][..]);
    }
}
