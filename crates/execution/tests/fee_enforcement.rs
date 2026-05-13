#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

//! Integration tests for tiered minimum fee enforcement and fee distribution.
//!
//! Acceptance Criteria:
//! 1. Transactions below minimum fee are rejected at dispatch with FeeBelowMinimum
//! 2. Transactions at or above minimum fee pass dispatch
//! 3. minimum_fee_for_tx returns correct minimum for all known types
//! 4. FeeSchedule::default() returns expected values
//! 5. distribute_fee splits AI transaction fees between fee_pool and ai_treasury
//! 6. distribute_fee routes base transfer fees entirely to fee_pool

use novai_ai_entities::{AiEntity, AutonomyMode, Capabilities};
use novai_execution::{
    dispatch_tx, distribute_fee, encode_credit_ai_entity_payload_v1,
    encode_register_ai_entity_payload_v1, encode_signal_commitment_payload_v1,
    encode_transfer_payload_v1, minimum_fee_for_tx, write_ai_entity_op, CreditAiEntityPayloadV1,
    ExecError, FeeSchedule, RegisterAiEntityPayloadV1, SignalCommitmentPayloadV1,
    TransferPayloadV1, CREATE_MEMORY_OBJECT_PAYLOAD_V1, CREDIT_AI_ENTITY_PAYLOAD_V1,
    DELETE_MEMORY_OBJECT_PAYLOAD_V1, EXECUTE_PROPOSAL_PAYLOAD_V1, KEY_AI_TREASURY,
    KEY_PRIVACY_TREASURY, MIN_FEE_CREDIT_AI_ENTITY, MIN_FEE_GOVERNANCE_EXECUTE,
    MIN_FEE_GOVERNANCE_SUBMIT, MIN_FEE_MEMORY_OBJECT, MIN_FEE_REGISTER_AI_ENTITY,
    MIN_FEE_SIGNAL_COMMITMENT, MIN_FEE_TRANSFER, REGISTER_AI_ENTITY_PAYLOAD_V1,
    SIGNAL_COMMITMENT_PAYLOAD_V1, SUBMIT_PROPOSAL_PAYLOAD_V1, TRANSFER_PAYLOAD_V1,
    UPDATE_MEMORY_OBJECT_PAYLOAD_V1,
};
use novai_state::{
    account_key, ai_entity_by_address_key, decode_fee_pool_v1, encode_account_v1, AccountStateV1,
    KvBatch, MemKv, WriteOp, KEY_FEE_POOL,
};
use novai_types::{TxV1, TxVersion};

// ============================================================================
// HELPERS
// ============================================================================

fn create_test_tx(from: [u8; 32], nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
    TxV1 {
        version: TxVersion::V1,
        from,
        pubkey: from,
        nonce,
        fee,
        payload,
        sig: [0u8; 64],
    }
}

fn seed_account(db: &mut MemKv, addr: &[u8; 32], balance: u128, nonce: u64) {
    let acct = AccountStateV1 { balance, nonce };
    let op = WriteOp::Put(account_key(addr), encode_account_v1(&acct).to_vec());
    db.apply_batch(&[op]).unwrap();
}

fn read_balance_at_key(db: &MemKv, key: &[u8]) -> u128 {
    use novai_state::Kv;
    db.get(key)
        .unwrap()
        .map_or(0, |bytes| decode_fee_pool_v1(&bytes).unwrap().balance)
}

// ============================================================================
// TEST 1: transfer_below_minimum_rejected
// ============================================================================

#[test]
fn transfer_below_minimum_rejected() {
    let mut db = MemKv::new();

    let sender = [0x01u8; 32];
    seed_account(&mut db, &sender, 100_000, 0);

    let payload = encode_transfer_payload_v1(&TransferPayloadV1 {
        to: [0x02u8; 32],
        amount: 100,
    })
    .to_vec();

    // Fee 50 is below minimum of 100
    let tx = create_test_tx(sender, 0, 50, payload);
    let result = dispatch_tx(&mut db, &tx, 100);
    assert!(
        matches!(
            result,
            Err(ExecError::FeeBelowMinimum {
                minimum: 100,
                provided: 50
            })
        ),
        "Transfer with fee below minimum should be rejected: {result:?}"
    );
}

// ============================================================================
// TEST 2: transfer_at_minimum_accepted
// ============================================================================

#[test]
fn transfer_at_minimum_accepted() {
    let mut db = MemKv::new();

    let sender = [0x01u8; 32];
    let receiver = [0x02u8; 32];
    seed_account(&mut db, &sender, 100_000, 0);

    let payload = encode_transfer_payload_v1(&TransferPayloadV1 {
        to: receiver,
        amount: 5000, // Must meet MIN_ACCOUNT_BALANCE for new accounts (M-06)
    })
    .to_vec();

    // Fee exactly at minimum
    let tx = create_test_tx(sender, 0, MIN_FEE_TRANSFER, payload);
    let result = dispatch_tx(&mut db, &tx, 100);
    assert!(
        result.is_ok(),
        "Transfer at minimum fee should pass: {result:?}"
    );
}

// ============================================================================
// TEST 3: transfer_above_minimum_accepted
// ============================================================================

#[test]
fn transfer_above_minimum_accepted() {
    let mut db = MemKv::new();

    let sender = [0x01u8; 32];
    let receiver = [0x02u8; 32];
    seed_account(&mut db, &sender, 100_000, 0);

    let payload = encode_transfer_payload_v1(&TransferPayloadV1 {
        to: receiver,
        amount: 5000, // Must meet MIN_ACCOUNT_BALANCE for new accounts (M-06)
    })
    .to_vec();

    // Fee above minimum
    let tx = create_test_tx(sender, 0, 500, payload);
    let result = dispatch_tx(&mut db, &tx, 100);
    assert!(
        result.is_ok(),
        "Transfer above minimum fee should pass: {result:?}"
    );
}

// ============================================================================
// TEST 4: signal_below_minimum_rejected
// ============================================================================

#[test]
fn signal_below_minimum_rejected() {
    let mut db = MemKv::new();

    // Create an entity for the signal
    let creator = [0x01u8; 32];
    let entity = AiEntity::new(
        [0x42u8; 32],
        creator,
        AutonomyMode::Gated,
        Capabilities::gated(),
        50,
    );
    let entity_id = entity.id;
    let mut funded = entity;
    funded.economic_balance = 100_000;
    db.apply_batch(&[write_ai_entity_op(&funded)]).unwrap();

    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8; 32],
        signal_type: novai_ai_entities::AiSignalType::Prediction,
        issuer_entity_id: entity_id,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
        subscription_create: None,
        subscription_cancel: None,
        payment_request: None,
        service_attestation: None,
    });

    // Fee 500 is below minimum of 1000 for signals
    let tx = create_test_tx(entity_id, 0, 500, payload);
    let result = dispatch_tx(&mut db, &tx, 100);
    assert!(
        matches!(
            result,
            Err(ExecError::FeeBelowMinimum {
                minimum: 1000,
                provided: 500
            })
        ),
        "Signal with fee below minimum should be rejected: {result:?}"
    );
}

// ============================================================================
// TEST 5: signal_at_minimum_accepted
// ============================================================================

#[test]
fn signal_at_minimum_accepted() {
    let mut db = MemKv::new();

    let creator = [0x01u8; 32];
    let entity = AiEntity::new(
        [0x42u8; 32],
        creator,
        AutonomyMode::Gated,
        Capabilities::gated(),
        50,
    );
    let entity_id = entity.id;
    let mut funded = entity;
    funded.economic_balance = 100_000;
    db.apply_batch(&[
        write_ai_entity_op(&funded),
        WriteOp::Put(ai_entity_by_address_key(&entity_id), entity_id.to_vec()),
    ])
    .unwrap();

    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8; 32],
        signal_type: novai_ai_entities::AiSignalType::Prediction,
        issuer_entity_id: entity_id,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
        subscription_create: None,
        subscription_cancel: None,
        payment_request: None,
        service_attestation: None,
    });

    let tx = create_test_tx(entity_id, 0, MIN_FEE_SIGNAL_COMMITMENT, payload);
    let result = dispatch_tx(&mut db, &tx, 100);
    assert!(
        result.is_ok(),
        "Signal at minimum fee should pass: {result:?}"
    );
}

// ============================================================================
// TEST 6: register_entity_below_minimum_rejected
// ============================================================================

#[test]
fn register_entity_below_minimum_rejected() {
    let mut db = MemKv::new();

    let creator = [0x01u8; 32];
    seed_account(&mut db, &creator, 1_000_000, 0);

    let payload = encode_register_ai_entity_payload_v1(&RegisterAiEntityPayloadV1 {
        code_hash: [0x42u8; 32],
        autonomy_mode: AutonomyMode::Gated,
        capabilities: Capabilities::gated(),
        initial_balance: 100,
    })
    .to_vec();

    // Fee 1000 is below minimum of 5000 for registration
    let tx = create_test_tx(creator, 0, 1000, payload);
    let result = dispatch_tx(&mut db, &tx, 100);
    assert!(
        matches!(
            result,
            Err(ExecError::FeeBelowMinimum {
                minimum: 5000,
                provided: 1000
            })
        ),
        "Register with fee below minimum should be rejected: {result:?}"
    );
}

// ============================================================================
// TEST 7: register_entity_at_minimum_accepted
// ============================================================================

#[test]
fn register_entity_at_minimum_accepted() {
    let mut db = MemKv::new();

    let creator = [0x01u8; 32];
    seed_account(&mut db, &creator, 1_000_000, 0);

    let payload = encode_register_ai_entity_payload_v1(&RegisterAiEntityPayloadV1 {
        code_hash: [0x42u8; 32],
        autonomy_mode: AutonomyMode::Gated,
        capabilities: Capabilities::gated(),
        initial_balance: 100,
    })
    .to_vec();

    let tx = create_test_tx(creator, 0, MIN_FEE_REGISTER_AI_ENTITY, payload);
    let result = dispatch_tx(&mut db, &tx, 100);
    assert!(
        result.is_ok(),
        "Register at minimum fee should pass: {result:?}"
    );
}

// ============================================================================
// TEST 8: credit_entity_at_minimum_accepted
// ============================================================================

#[test]
fn credit_entity_at_minimum_accepted() {
    let mut db = MemKv::new();

    // Pre-create entity
    let creator = [0x01u8; 32];
    let entity = AiEntity::new(
        [0x42u8; 32],
        creator,
        AutonomyMode::Gated,
        Capabilities::gated(),
        50,
    );
    let entity_id = entity.id;
    db.apply_batch(&[write_ai_entity_op(&entity)]).unwrap();

    let sender = [0x02u8; 32];
    seed_account(&mut db, &sender, 100_000, 0);

    let payload = encode_credit_ai_entity_payload_v1(&CreditAiEntityPayloadV1 {
        entity_id,
        amount: 500,
    })
    .to_vec();

    let tx = create_test_tx(sender, 0, MIN_FEE_CREDIT_AI_ENTITY, payload);
    let result = dispatch_tx(&mut db, &tx, 100);
    assert!(
        result.is_ok(),
        "Credit at minimum fee should pass: {result:?}"
    );
}

// ============================================================================
// TEST 9: minimum_fee_for_unknown_payload_version
// ============================================================================

#[test]
fn minimum_fee_for_unknown_payload_version() {
    let tx = create_test_tx([0x01u8; 32], 0, 0, vec![0xFF]);
    let result = minimum_fee_for_tx(&tx);
    assert!(
        matches!(
            result,
            Err(ExecError::UnknownPayloadVersion { version: 0xFF })
        ),
        "Unknown payload version should error: {result:?}"
    );

    // Empty payload
    let tx_empty = create_test_tx([0x01u8; 32], 0, 0, vec![]);
    let result_empty = minimum_fee_for_tx(&tx_empty);
    assert!(
        matches!(
            result_empty,
            Err(ExecError::UnknownPayloadVersion { version: 0 })
        ),
        "Empty payload should error: {result_empty:?}"
    );
}

// ============================================================================
// TEST 10: minimum_fee_for_all_known_types
// ============================================================================

#[test]
fn minimum_fee_for_all_known_types() {
    let cases: &[(u8, u64)] = &[
        (TRANSFER_PAYLOAD_V1, MIN_FEE_TRANSFER),
        (SIGNAL_COMMITMENT_PAYLOAD_V1, MIN_FEE_SIGNAL_COMMITMENT),
        (CREATE_MEMORY_OBJECT_PAYLOAD_V1, MIN_FEE_MEMORY_OBJECT),
        (UPDATE_MEMORY_OBJECT_PAYLOAD_V1, MIN_FEE_MEMORY_OBJECT),
        (DELETE_MEMORY_OBJECT_PAYLOAD_V1, MIN_FEE_MEMORY_OBJECT),
        (SUBMIT_PROPOSAL_PAYLOAD_V1, MIN_FEE_GOVERNANCE_SUBMIT),
        (EXECUTE_PROPOSAL_PAYLOAD_V1, MIN_FEE_GOVERNANCE_EXECUTE),
        (REGISTER_AI_ENTITY_PAYLOAD_V1, MIN_FEE_REGISTER_AI_ENTITY),
        (CREDIT_AI_ENTITY_PAYLOAD_V1, MIN_FEE_CREDIT_AI_ENTITY),
    ];

    for &(version, expected_min) in cases {
        // Create a minimal tx with just the version byte as payload
        let tx = create_test_tx([0x01u8; 32], 0, 0, vec![version]);
        let result = minimum_fee_for_tx(&tx).unwrap();
        assert_eq!(
            result, expected_min,
            "Payload version {version} should have minimum fee {expected_min}, got {result}"
        );
    }
}

// ============================================================================
// TEST 11: fee_schedule_default_values
// ============================================================================

#[test]
fn fee_schedule_default_values() {
    let schedule = FeeSchedule::default();
    assert_eq!(schedule.transfer, 100);
    assert_eq!(schedule.signal_commitment, 1_000);
    assert_eq!(schedule.memory_object, 500);
    assert_eq!(schedule.governance_submit, 2_000);
    assert_eq!(schedule.governance_execute, 500);
    assert_eq!(schedule.register_ai_entity, 5_000);
    assert_eq!(schedule.credit_ai_entity, 100);
}

// ============================================================================
// TEST 12: distribute_fee_splits_ai_signal
// ============================================================================

#[test]
fn distribute_fee_splits_ai_signal() {
    let mut db = MemKv::new();

    // Create a tx with signal commitment payload version
    let entity_id = [0x01u8; 32];
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8; 32],
        signal_type: novai_ai_entities::AiSignalType::Prediction,
        issuer_entity_id: entity_id,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
        subscription_create: None,
        subscription_cancel: None,
        payment_request: None,
        service_attestation: None,
    });

    let fee = 1_000u64;
    let tx = create_test_tx(entity_id, 0, fee, payload);

    let ops = distribute_fee(&mut db, &tx, u128::from(fee)).unwrap();

    // Apply the ops
    db.apply_batch(&ops).unwrap();

    // Base portion (100) should go to fee pool
    let fee_pool_balance = read_balance_at_key(&db, KEY_FEE_POOL);
    assert_eq!(
        fee_pool_balance,
        u128::from(MIN_FEE_TRANSFER),
        "Fee pool should receive base portion (MIN_FEE_TRANSFER)"
    );

    // Remainder (900) should go to AI treasury
    let ai_treasury_balance = read_balance_at_key(&db, KEY_AI_TREASURY);
    assert_eq!(
        ai_treasury_balance,
        u128::from(fee) - u128::from(MIN_FEE_TRANSFER),
        "AI treasury should receive remainder"
    );
}

// ============================================================================
// TEST 13: distribute_fee_base_transfer
// ============================================================================

#[test]
fn distribute_fee_base_transfer() {
    let mut db = MemKv::new();

    let payload = encode_transfer_payload_v1(&TransferPayloadV1 {
        to: [0x02u8; 32],
        amount: 100,
    })
    .to_vec();

    let fee = 500u64;
    let tx = create_test_tx([0x01u8; 32], 0, fee, payload);

    let ops = distribute_fee(&mut db, &tx, u128::from(fee)).unwrap();
    db.apply_batch(&ops).unwrap();

    // All should go to fee pool for base transfer
    let fee_pool_balance = read_balance_at_key(&db, KEY_FEE_POOL);
    assert_eq!(
        fee_pool_balance,
        u128::from(fee),
        "Base transfer fee should all go to fee pool"
    );

    // AI treasury should be empty
    let ai_treasury_balance = read_balance_at_key(&db, KEY_AI_TREASURY);
    assert_eq!(
        ai_treasury_balance, 0,
        "AI treasury should be empty for base transfers"
    );

    // Privacy treasury should be empty
    let privacy_treasury_balance = read_balance_at_key(&db, KEY_PRIVACY_TREASURY);
    assert_eq!(
        privacy_treasury_balance, 0,
        "Privacy treasury should be empty"
    );
}
