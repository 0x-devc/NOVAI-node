#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

//! Week 14 integration tests for signal commitment transactions.
//!
//! Acceptance Criteria:
//! 1. Tx accepted - Valid signal commitment processed
//! 2. Tx rejected - Invalid issuer rejected
//! 3. Indexed correctly - Queries return correct results
//! 4. Fee deducted - AI entity balance decreases

use novai_ai_entities::{AiEntity, AiSignalType, AutonomyMode, Capabilities};
use novai_execution::{
    apply_signal_commitment_tx, decode_signal_commitment_payload_v1,
    encode_signal_commitment_payload_v1, get_signals_by_height, get_signals_by_issuer,
    get_signals_by_type, read_ai_entity, write_ai_entity_op, ExecError, SignalCommitmentPayloadV1,
    SubscriptionCancelExtraV1, SubscriptionCreateExtraV1,
    SIGNAL_COMMITMENT_PAYLOAD_V1_SUBSCRIPTION_CANCEL_LEN,
    SIGNAL_COMMITMENT_PAYLOAD_V1_SUBSCRIPTION_CREATE_LEN,
};
use novai_state::{ai_entity_by_address_key, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

/// Create a test AI entity with emit_proposals capability.
fn create_test_entity(creator: [u8; 32], balance: u128, nonce: u64) -> AiEntity {
    let code_hash = [0x42u8; 32];
    let mut entity = AiEntity::new(
        code_hash,
        creator,
        AutonomyMode::Gated,
        Capabilities::gated(), // has emit_proposals
        1000,
    );
    entity.economic_balance = balance;
    entity.nonce = nonce;
    entity
}

/// Create a signal commitment payload.
fn create_signal_payload(
    signal_hash: [u8; 32],
    signal_type: AiSignalType,
    issuer: [u8; 32],
) -> Vec<u8> {
    let payload = SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type,
        issuer_entity_id: issuer,
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
        sla_accept: None,
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
    };
    encode_signal_commitment_payload_v1(&payload)
}

/// Create a test transaction.
fn create_test_tx(from: [u8; 32], nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
    TxV1 {
        version: TxVersion::V1,
        from,
        pubkey: from, // Simplified: using same bytes
        nonce,
        fee,
        payload,
        sig: [0u8; 64], // Signature validation not in scope for Week 14
    }
}

// ============================================================================
// Acceptance Criterion 1: Tx accepted - Valid signal commitment processed
// ============================================================================

#[test]
fn valid_signal_commitment_is_accepted() {
    let mut db = MemKv::new();

    // Setup: Create and store AI entity
    let creator = [0x01u8; 32];
    let entity = create_test_entity(creator, 1000, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    // Create signal commitment payload
    let signal_hash = [0xAAu8; 32];
    let payload = create_signal_payload(signal_hash, AiSignalType::Prediction, entity_id);

    // Create transaction
    let tx = create_test_tx(entity_id, 0, 10, payload);

    // Execute
    let result = apply_signal_commitment_tx(&mut db, &tx, 100);
    assert!(result.is_ok(), "Valid signal commitment should be accepted");

    // Verify signal was stored
    let signals = get_signals_by_height(&db, 100).unwrap();
    assert_eq!(
        signals.len(),
        1,
        "One signal should be stored at height 100"
    );
    assert_eq!(signals[0].commitment_hash, signal_hash);
    assert_eq!(signals[0].signal_type, AiSignalType::Prediction);
    assert_eq!(signals[0].issuer, entity_id);
}

// ============================================================================
// Acceptance Criterion 2: Tx rejected - Invalid issuer rejected
// ============================================================================

#[test]
fn nonexistent_issuer_is_rejected() {
    let mut db = MemKv::new();

    // No entity stored - issuer doesn't exist
    let fake_entity_id = [0xDEu8; 32];
    let signal_hash = [0xAAu8; 32];
    let payload = create_signal_payload(signal_hash, AiSignalType::Anomaly, fake_entity_id);

    let tx = create_test_tx(fake_entity_id, 0, 10, payload);

    let result = apply_signal_commitment_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::IssuerNotFound)),
        "Nonexistent issuer should be rejected with IssuerNotFound"
    );
}

#[test]
fn issuer_without_capability_is_rejected() {
    let mut db = MemKv::new();

    // Create entity WITHOUT emit_proposals capability
    let creator = [0x01u8; 32];
    let code_hash = [0x42u8; 32];
    let mut entity = AiEntity::new(
        code_hash,
        creator,
        AutonomyMode::Advisory,
        Capabilities::read_only(), // NO emit_proposals
        1000,
    );
    entity.economic_balance = 1000;
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    let signal_hash = [0xAAu8; 32];
    let payload = create_signal_payload(signal_hash, AiSignalType::RiskScore, entity_id);
    let tx = create_test_tx(entity_id, 0, 10, payload);

    let result = apply_signal_commitment_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::IssuerMissingCapability)),
        "Entity without emit_proposals should be rejected"
    );
}

#[test]
fn issuer_mismatch_is_rejected() {
    let mut db = MemKv::new();

    // Create entity
    let creator = [0x01u8; 32];
    let entity = create_test_entity(creator, 1000, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    // Payload claims different issuer than tx.from
    let different_issuer = [0xFFu8; 32];
    let signal_hash = [0xAAu8; 32];
    let payload = create_signal_payload(signal_hash, AiSignalType::SpamRisk, different_issuer);

    // tx.from = entity_id, but payload claims different_issuer
    let tx = create_test_tx(entity_id, 0, 10, payload);

    let result = apply_signal_commitment_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::IssuerMismatch)),
        "Mismatched issuer should be rejected"
    );
}

#[test]
fn insufficient_balance_is_rejected() {
    let mut db = MemKv::new();

    // Create entity with only 5 balance
    let creator = [0x01u8; 32];
    let entity = create_test_entity(creator, 5, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    let signal_hash = [0xAAu8; 32];
    let payload = create_signal_payload(signal_hash, AiSignalType::AuditReport, entity_id);

    // Fee is 10, but balance is only 5
    let tx = create_test_tx(entity_id, 0, 10, payload);

    let result = apply_signal_commitment_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::InsufficientFunds { .. })),
        "Insufficient balance should be rejected"
    );
}

#[test]
fn wrong_nonce_is_rejected() {
    let mut db = MemKv::new();

    // Create entity with nonce = 5
    let creator = [0x01u8; 32];
    let entity = create_test_entity(creator, 1000, 5);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    let signal_hash = [0xAAu8; 32];
    let payload = create_signal_payload(signal_hash, AiSignalType::CongestionForecast, entity_id);

    // Tx has nonce = 0, but entity has nonce = 5
    let tx = create_test_tx(entity_id, 0, 10, payload);

    let result = apply_signal_commitment_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::NonceMismatch { .. })),
        "Wrong nonce should be rejected"
    );
}

// ============================================================================
// Acceptance Criterion 3: Indexed correctly - Queries return correct results
// ============================================================================

#[test]
fn query_by_height_returns_correct_signals() {
    let mut db = MemKv::new();

    // Create two entities (different issuers at same height)
    let entity1 = create_test_entity([0x01u8; 32], 10000, 0);
    let entity2 = create_test_entity([0x02u8; 32], 10000, 0);
    let id1 = entity1.id;
    let id2 = entity2.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity1),
        WriteOp::Put(ai_entity_by_address_key(&entity1.id), entity1.id.to_vec()),
        write_ai_entity_op(&entity2),
        WriteOp::Put(ai_entity_by_address_key(&entity2.id), entity2.id.to_vec()),
    ])
    .unwrap();

    // Entity 1 submits at height 100
    let payload1 = create_signal_payload([0x01u8; 32], AiSignalType::Prediction, id1);
    let tx1 = create_test_tx(id1, 0, 10, payload1);
    apply_signal_commitment_tx(&mut db, &tx1, 100).unwrap();

    // Entity 2 submits at height 100 (different issuer, same height)
    let payload2 = create_signal_payload([0x02u8; 32], AiSignalType::Prediction, id2);
    let tx2 = create_test_tx(id2, 0, 10, payload2);
    apply_signal_commitment_tx(&mut db, &tx2, 100).unwrap();

    // Entity 1 submits at height 200
    let entity1 = read_ai_entity(&db, &id1).unwrap().unwrap();
    let payload3 = create_signal_payload([0x03u8; 32], AiSignalType::Prediction, id1);
    let tx3 = create_test_tx(id1, entity1.nonce, 10, payload3);
    apply_signal_commitment_tx(&mut db, &tx3, 200).unwrap();

    // Query height 100 - should have 2 signals (from 2 different issuers)
    let signals_100 = get_signals_by_height(&db, 100).unwrap();
    assert_eq!(signals_100.len(), 2, "Height 100 should have 2 signals");

    // Query height 200 - should have 1 signal
    let signals_200 = get_signals_by_height(&db, 200).unwrap();
    assert_eq!(signals_200.len(), 1, "Height 200 should have 1 signal");

    // Query height 300 - should have 0 signals
    let signals_300 = get_signals_by_height(&db, 300).unwrap();
    assert_eq!(signals_300.len(), 0, "Height 300 should have 0 signals");
}

#[test]
fn query_by_issuer_returns_correct_signals() {
    let mut db = MemKv::new();

    // Create two entities
    let entity1 = create_test_entity([0x01u8; 32], 10000, 0);
    let entity2 = create_test_entity([0x02u8; 32], 10000, 0);
    let id1 = entity1.id;
    let id2 = entity2.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity1),
        WriteOp::Put(ai_entity_by_address_key(&entity1.id), entity1.id.to_vec()),
        write_ai_entity_op(&entity2),
        WriteOp::Put(ai_entity_by_address_key(&entity2.id), entity2.id.to_vec()),
    ])
    .unwrap();

    // Entity 1 submits 2 signals
    for i in 0..2 {
        let entity = read_ai_entity(&db, &id1).unwrap().unwrap();
        let signal_hash = [i as u8 + 1; 32];
        let payload = create_signal_payload(signal_hash, AiSignalType::Anomaly, id1);
        let tx = create_test_tx(id1, entity.nonce, 10, payload);
        apply_signal_commitment_tx(&mut db, &tx, 100 + i).unwrap();
    }

    // Entity 2 submits 1 signal
    let entity = read_ai_entity(&db, &id2).unwrap().unwrap();
    let payload = create_signal_payload([0xFFu8; 32], AiSignalType::Optimization, id2);
    let tx = create_test_tx(id2, entity.nonce, 10, payload);
    apply_signal_commitment_tx(&mut db, &tx, 100).unwrap();

    // Query by issuer 1 - should have 2
    let signals_1 = get_signals_by_issuer(&db, &id1, 0, 1000).unwrap();
    assert_eq!(signals_1.len(), 2, "Issuer 1 should have 2 signals");

    // Query by issuer 2 - should have 1
    let signals_2 = get_signals_by_issuer(&db, &id2, 0, 1000).unwrap();
    assert_eq!(signals_2.len(), 1, "Issuer 2 should have 1 signal");
}

#[test]
fn query_by_type_returns_correct_signals() {
    let mut db = MemKv::new();

    let entity = create_test_entity([0x01u8; 32], 10000, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    // Submit signals of different types
    let types = [
        AiSignalType::Anomaly,
        AiSignalType::Prediction,
        AiSignalType::Prediction,
        AiSignalType::RiskScore,
    ];

    for (i, signal_type) in types.iter().enumerate() {
        let entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
        let signal_hash = [i as u8 + 1; 32];
        let payload = create_signal_payload(signal_hash, *signal_type, entity_id);
        let tx = create_test_tx(entity_id, entity.nonce, 10, payload);
        apply_signal_commitment_tx(&mut db, &tx, 100 + i as u64).unwrap();
    }

    // Query by type Prediction - should have 2
    let predictions = get_signals_by_type(&db, AiSignalType::Prediction, 0, 1000).unwrap();
    assert_eq!(predictions.len(), 2, "Should have 2 Prediction signals");

    // Query by type Anomaly - should have 1
    let anomalies = get_signals_by_type(&db, AiSignalType::Anomaly, 0, 1000).unwrap();
    assert_eq!(anomalies.len(), 1, "Should have 1 Anomaly signal");

    // Query by type SpamRisk - should have 0
    let spam = get_signals_by_type(&db, AiSignalType::SpamRisk, 0, 1000).unwrap();
    assert_eq!(spam.len(), 0, "Should have 0 SpamRisk signals");
}

// ============================================================================
// Acceptance Criterion 4: Fee deducted - AI entity balance decreases
// ============================================================================

#[test]
fn fee_is_deducted_from_entity_balance() {
    let mut db = MemKv::new();

    let initial_balance = 1000u128;
    let fee = 50u64;

    let entity = create_test_entity([0x01u8; 32], initial_balance, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    let signal_hash = [0xAAu8; 32];
    let payload = create_signal_payload(signal_hash, AiSignalType::Prediction, entity_id);
    let tx = create_test_tx(entity_id, 0, fee, payload);

    apply_signal_commitment_tx(&mut db, &tx, 100).unwrap();

    // Check balance decreased
    let updated_entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert_eq!(
        updated_entity.economic_balance,
        initial_balance - fee as u128,
        "Balance should decrease by fee amount"
    );
}

#[test]
fn nonce_is_incremented_after_signal() {
    let mut db = MemKv::new();

    let entity = create_test_entity([0x01u8; 32], 1000, 5);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    let signal_hash = [0xAAu8; 32];
    let payload = create_signal_payload(signal_hash, AiSignalType::RiskScore, entity_id);
    let tx = create_test_tx(entity_id, 5, 10, payload);

    apply_signal_commitment_tx(&mut db, &tx, 100).unwrap();

    let updated_entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert_eq!(
        updated_entity.nonce, 6,
        "Nonce should increment from 5 to 6"
    );
}

#[test]
fn last_active_at_is_updated() {
    let mut db = MemKv::new();

    let entity = create_test_entity([0x01u8; 32], 1000, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    let signal_hash = [0xAAu8; 32];
    let payload = create_signal_payload(signal_hash, AiSignalType::Optimization, entity_id);
    let tx = create_test_tx(entity_id, 0, 10, payload);

    let execution_height = 500;
    apply_signal_commitment_tx(&mut db, &tx, execution_height).unwrap();

    let updated_entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert_eq!(
        updated_entity.last_active_at, execution_height,
        "last_active_at should be set to execution height"
    );
}

// ============================================================================
// Phase 3: Subscription codec roundtrip + length validation tests (Feature 9)
// ============================================================================

#[test]
fn subscription_create_payload_roundtrip() {
    let producer = [0xC1u8; 32];
    let issuer = [0xB2u8; 32];
    let signal_hash = [0xA3u8; 32];
    let original = SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type: AiSignalType::SubscriptionCreate,
        issuer_entity_id: issuer,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
        subscription_create: Some(SubscriptionCreateExtraV1 {
            producer_entity_id: producer,
            covered_signal_type: 2, // Prediction
            rate_per_block: 0x0102_0304_0506_0708,
            duration_blocks: 0x1112_1314_1516_1718,
        }),
        subscription_cancel: None,
        payment_request: None,
        service_attestation: None,
        sla_accept: None,
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
    };
    let encoded = encode_signal_commitment_payload_v1(&original);
    assert_eq!(
        encoded.len(),
        SIGNAL_COMMITMENT_PAYLOAD_V1_SUBSCRIPTION_CREATE_LEN
    );
    assert_eq!(encoded.len(), 115);
    let decoded = decode_signal_commitment_payload_v1(&encoded).expect("decode succeeds");
    assert_eq!(decoded, original);
}

#[test]
fn subscription_cancel_payload_roundtrip() {
    let issuer = [0xB2u8; 32];
    let signal_hash = [0xA3u8; 32];
    let sub_id = [0xD4u8; 32];
    let original = SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type: AiSignalType::SubscriptionCancel,
        issuer_entity_id: issuer,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
        subscription_create: None,
        subscription_cancel: Some(SubscriptionCancelExtraV1 {
            subscription_id: sub_id,
        }),
        payment_request: None,
        service_attestation: None,
        sla_accept: None,
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
    };
    let encoded = encode_signal_commitment_payload_v1(&original);
    assert_eq!(
        encoded.len(),
        SIGNAL_COMMITMENT_PAYLOAD_V1_SUBSCRIPTION_CANCEL_LEN
    );
    assert_eq!(encoded.len(), 98);
    let decoded = decode_signal_commitment_payload_v1(&encoded).expect("decode succeeds");
    assert_eq!(decoded, original);
}

#[test]
fn subscription_create_byte_layout_locked() {
    let original = SignalCommitmentPayloadV1 {
        signal_hash: [0u8; 32],
        signal_type: AiSignalType::SubscriptionCreate,
        issuer_entity_id: [0u8; 32],
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
        subscription_create: Some(SubscriptionCreateExtraV1 {
            producer_entity_id: [0xCCu8; 32],
            covered_signal_type: 0x07,
            rate_per_block: 0x0102_0304_0506_0708,
            duration_blocks: 0x1112_1314_1516_1718,
        }),
        subscription_cancel: None,
        payment_request: None,
        service_attestation: None,
        sla_accept: None,
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
    };
    let encoded = encode_signal_commitment_payload_v1(&original);
    // Base header lock (offsets 0..66 already covered by other roundtrip
    // tests). Spot-check the tail offsets to lock the wire format.
    assert_eq!(
        encoded[33], 14,
        "signal_type byte == SubscriptionCreate(14)"
    );
    assert_eq!(
        &encoded[66..98],
        &[0xCCu8; 32],
        "producer_entity_id at 66..98"
    );
    assert_eq!(encoded[98], 0x07, "covered_signal_type at 98");
    assert_eq!(
        &encoded[99..107],
        &0x0102_0304_0506_0708u64.to_be_bytes(),
        "rate_per_block_be at 99..107"
    );
    assert_eq!(
        &encoded[107..115],
        &0x1112_1314_1516_1718u64.to_be_bytes(),
        "duration_blocks_be at 107..115"
    );
}

#[test]
fn subscription_cancel_byte_layout_locked() {
    let original = SignalCommitmentPayloadV1 {
        signal_hash: [0u8; 32],
        signal_type: AiSignalType::SubscriptionCancel,
        issuer_entity_id: [0u8; 32],
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
        subscription_create: None,
        subscription_cancel: Some(SubscriptionCancelExtraV1 {
            subscription_id: [0xEEu8; 32],
        }),
        payment_request: None,
        service_attestation: None,
        sla_accept: None,
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
    };
    let encoded = encode_signal_commitment_payload_v1(&original);
    assert_eq!(
        encoded[33], 15,
        "signal_type byte == SubscriptionCancel(15)"
    );
    assert_eq!(&encoded[66..98], &[0xEEu8; 32], "subscription_id at 66..98");
}

#[test]
fn subscription_create_with_wrong_length_rejected() {
    // Encode a SubscriptionCreate payload then truncate it; decode must reject.
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0u8; 32],
        signal_type: AiSignalType::SubscriptionCreate,
        issuer_entity_id: [0u8; 32],
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
        subscription_create: Some(SubscriptionCreateExtraV1 {
            producer_entity_id: [0u8; 32],
            covered_signal_type: 0,
            rate_per_block: 0,
            duration_blocks: 0,
        }),
        subscription_cancel: None,
        payment_request: None,
        service_attestation: None,
        sla_accept: None,
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
    });
    let mut truncated = payload.clone();
    truncated.truncate(payload.len() - 1);
    assert!(matches!(
        decode_signal_commitment_payload_v1(&truncated),
        Err(ExecError::BadPayloadLength { .. })
    ));
}

#[test]
fn subscription_cancel_with_wrong_length_rejected() {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0u8; 32],
        signal_type: AiSignalType::SubscriptionCancel,
        issuer_entity_id: [0u8; 32],
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
        subscription_create: None,
        subscription_cancel: Some(SubscriptionCancelExtraV1 {
            subscription_id: [0u8; 32],
        }),
        payment_request: None,
        service_attestation: None,
        sla_accept: None,
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
    });
    let mut truncated = payload.clone();
    truncated.truncate(payload.len() - 1);
    assert!(matches!(
        decode_signal_commitment_payload_v1(&truncated),
        Err(ExecError::BadPayloadLength { .. })
    ));
}

#[test]
fn unknown_signal_type_byte_22_rejected_by_decoder() {
    // Build a base-length (66 byte) payload with signal_type byte = 22
    // (one past the current max, ChannelFinalize = 21). The decoder
    // runs from_byte() at offset 33 and must reject with a version-
    // style error (the "max valid signal type" guard).
    let mut payload = vec![0u8; 66];
    payload[0] = 2; // version
    payload[33] = 22; // unknown signal_type
    let result = decode_signal_commitment_payload_v1(&payload);
    assert!(
        matches!(result, Err(ExecError::BadPayloadVersion { .. })),
        "byte 22 must be rejected as unknown signal type, got {result:?}"
    );
}
