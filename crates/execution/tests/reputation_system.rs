#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

//! Integration tests for the on-chain reputation system.
//!
//! Covers:
//! - Oracle path: `submit_reputation_updates` + ReputationUpdate signal mutates target.
//! - Capability gating: non-oracles rejected.
//! - Score clamping at 0 and 100.
//! - `total_transactions` increments only on `REP_EVENT_JOB_COMPLETED`.
//! - `reputation_events_count` always increments.
//! - Self-update prohibition.
//! - Target-not-found path.
//! - Existing non-reputation signals continue to work (regression).
//! - Inline reputation payload byte-length is 101.

use novai_ai_entities::{
    AiEntity, AiSignalType, AutonomyMode, Capabilities, DEFAULT_REPUTATION_SCORE,
};
use novai_execution::{
    apply_signal_commitment_tx, encode_signal_commitment_payload_v1, read_ai_entity,
    write_ai_entity_op, ExecError, ReputationUpdateExtraV1, SignalCommitmentPayloadV1,
    REP_EVENT_AUTO_RELEASE_PENALTY, REP_EVENT_DECAY, REP_EVENT_DISPUTE_WON_CUSTOMER,
    REP_EVENT_DISPUTE_WON_DELIVERER, REP_EVENT_FRAUD_DETECTED, REP_EVENT_JOB_COMPLETED,
    SIGNAL_COMMITMENT_PAYLOAD_V1_REP_LEN,
};
use novai_state::{ai_entity_by_address_key, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const ORACLE_BALANCE: u128 = 1_000_000;
const SIGNAL_FEE: u64 = 1_000;

/// Build a Capabilities set with all flags relevant for an oracle that
/// emits reputation updates: emit_proposals (precondition for signals) +
/// submit_reputation_updates (the new oracle gate).
fn oracle_caps() -> Capabilities {
    Capabilities {
        read_public_chain: true,
        read_memory_objects: true,
        emit_proposals: true,
        request_execution: false,
        read_nnpx_derived: false,
        submit_reputation_updates: true,
        post_oracle_anchors: false,
        _reserved: [false; 1],
    }
}

fn build_entity(code_hash: [u8; 32], creator: [u8; 32], caps: Capabilities) -> AiEntity {
    AiEntity::new(code_hash, creator, AutonomyMode::Gated, caps, 1000)
}

fn store_entity(db: &mut MemKv, entity: &AiEntity) {
    db.apply_batch(&[
        write_ai_entity_op(entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();
}

fn make_oracle(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut oracle = build_entity(code_hash, creator, oracle_caps());
    oracle.economic_balance = ORACLE_BALANCE;
    store_entity(db, &oracle);
    oracle
}

fn make_target(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let target = build_entity(code_hash, creator, Capabilities::advisory());
    store_entity(db, &target);
    target
}

fn build_reputation_payload(
    issuer: [u8; 32],
    target: [u8; 32],
    event_type: u8,
    points_delta: i16,
) -> Vec<u8> {
    encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8; 32],
        signal_type: AiSignalType::ReputationUpdate,
        issuer_entity_id: issuer,
        reputation: Some(ReputationUpdateExtraV1 {
            target_entity_id: target,
            event_type,
            points_delta,
        }),
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
        oracle_anchor: None,
    })
}

fn make_tx(from: [u8; 32], nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
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

// ============================================================================
// 1. Oracle increments target score
// ============================================================================

#[test]
fn reputation_update_oracle_increments_score() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let payload = build_reputation_payload(oracle.id, target.id, REP_EVENT_JOB_COMPLETED, 5);
    let tx = make_tx(oracle.id, 0, SIGNAL_FEE, payload);

    apply_signal_commitment_tx(&mut db, &tx, 100).expect("oracle update should succeed");

    let updated = read_ai_entity(&db, &target.id)
        .expect("read")
        .expect("target present");
    assert_eq!(updated.reputation_score, DEFAULT_REPUTATION_SCORE + 5);
    assert_eq!(updated.total_transactions, 1);
    assert_eq!(updated.reputation_events_count, 1);
}

// ============================================================================
// 2. Non-oracle rejected
// ============================================================================

#[test]
fn reputation_update_rejected_without_capability() {
    let mut db = MemKv::new();
    // Issuer has emit_proposals via gated() but NOT submit_reputation_updates.
    let mut issuer = build_entity([0x33u8; 32], [0x44u8; 32], Capabilities::gated());
    issuer.economic_balance = ORACLE_BALANCE;
    store_entity(&mut db, &issuer);

    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let payload = build_reputation_payload(issuer.id, target.id, REP_EVENT_JOB_COMPLETED, 5);
    let tx = make_tx(issuer.id, 0, SIGNAL_FEE, payload);

    let err = apply_signal_commitment_tx(&mut db, &tx, 100).expect_err("must fail");
    assert!(
        matches!(err, ExecError::IssuerMissingCapability),
        "got {err:?}"
    );

    // Target unchanged.
    let after = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert_eq!(after.reputation_score, DEFAULT_REPUTATION_SCORE);
    assert_eq!(after.reputation_events_count, 0);
}

// ============================================================================
// 3. emit_proposals alone is not sufficient (regression for cap-gating)
// ============================================================================

#[test]
fn reputation_update_emit_proposals_alone_insufficient() {
    let mut db = MemKv::new();
    // Identical to the rejected_without_capability case but stresses that the
    // caller really does carry emit_proposals and is only missing the new gate.
    let mut issuer = build_entity([0x55u8; 32], [0x66u8; 32], Capabilities::advisory());
    assert!(issuer.capabilities.emit_proposals);
    assert!(!issuer.capabilities.submit_reputation_updates);
    issuer.economic_balance = ORACLE_BALANCE;
    store_entity(&mut db, &issuer);

    let target = make_target(&mut db, [0x77u8; 32], [0x88u8; 32]);
    let payload = build_reputation_payload(issuer.id, target.id, REP_EVENT_JOB_COMPLETED, 5);
    let tx = make_tx(issuer.id, 0, SIGNAL_FEE, payload);

    let err = apply_signal_commitment_tx(&mut db, &tx, 100).expect_err("must fail");
    assert!(matches!(err, ExecError::IssuerMissingCapability));
}

// ============================================================================
// 4. Score clamps at zero
// ============================================================================

#[test]
fn reputation_score_clamps_at_zero() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let mut target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);
    target.reputation_score = 3;
    store_entity(&mut db, &target);

    let payload = build_reputation_payload(oracle.id, target.id, REP_EVENT_FRAUD_DETECTED, -100);
    let tx = make_tx(oracle.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, 100).unwrap();

    let updated = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert_eq!(updated.reputation_score, 0, "must clamp at lower bound");
}

// ============================================================================
// 5. Score clamps at 100
// ============================================================================

#[test]
fn reputation_score_clamps_at_hundred() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let mut target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);
    target.reputation_score = 95;
    store_entity(&mut db, &target);

    let payload = build_reputation_payload(oracle.id, target.id, REP_EVENT_JOB_COMPLETED, 20);
    let tx = make_tx(oracle.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, 100).unwrap();

    let updated = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert_eq!(updated.reputation_score, 100, "must clamp at upper bound");
}

// ============================================================================
// 6. total_transactions increments only on JOB_COMPLETED
// ============================================================================

#[test]
fn total_transactions_increments_on_job_completed() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let payload = build_reputation_payload(oracle.id, target.id, REP_EVENT_JOB_COMPLETED, 5);
    let tx = make_tx(oracle.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, 100).unwrap();

    let updated = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert_eq!(updated.total_transactions, 1);
}

// ============================================================================
// 7. total_transactions unchanged on other events; events_count still increments
// ============================================================================

#[test]
fn total_transactions_unchanged_on_other_events() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);

    // Walk three non-completion events with different deltas.
    let events: [(u8, i16, u64); 4] = [
        (REP_EVENT_DISPUTE_WON_DELIVERER, 2, 0),
        (REP_EVENT_DISPUTE_WON_CUSTOMER, 1, 1),
        (REP_EVENT_AUTO_RELEASE_PENALTY, -2, 2),
        (REP_EVENT_DECAY, -1, 3),
    ];

    for (event_type, delta, nonce) in events {
        let payload = build_reputation_payload(oracle.id, target.id, event_type, delta);
        let tx = make_tx(oracle.id, nonce, SIGNAL_FEE, payload);
        apply_signal_commitment_tx(&mut db, &tx, 100 + nonce).unwrap();
    }

    let updated = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert_eq!(
        updated.total_transactions, 0,
        "non-completion must not bump counter"
    );
    assert_eq!(updated.reputation_events_count, 4);
}

// ============================================================================
// 8. reputation_events_count always increments
// ============================================================================

#[test]
fn reputation_events_count_always_increments() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_target(&mut db, [0x12u8; 32], [0x22u8; 32]);

    for nonce in 0u64..3 {
        let payload = build_reputation_payload(oracle.id, target.id, REP_EVENT_JOB_COMPLETED, 1);
        let tx = make_tx(oracle.id, nonce, SIGNAL_FEE, payload);
        apply_signal_commitment_tx(&mut db, &tx, 100 + nonce).unwrap();
    }

    let updated = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert_eq!(updated.reputation_events_count, 3);
    assert_eq!(updated.total_transactions, 3);
}

// ============================================================================
// 9. Oracle cannot self-update
// ============================================================================

#[test]
fn reputation_self_update_rejected() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let payload = build_reputation_payload(oracle.id, oracle.id, REP_EVENT_JOB_COMPLETED, 5);
    let tx = make_tx(oracle.id, 0, SIGNAL_FEE, payload);

    let err = apply_signal_commitment_tx(&mut db, &tx, 100).expect_err("must fail");
    assert!(
        matches!(err, ExecError::SelfReputationUpdate),
        "got {err:?}"
    );
}

// ============================================================================
// 10. Target not found
// ============================================================================

#[test]
fn reputation_update_target_not_found() {
    let mut db = MemKv::new();
    let oracle = make_oracle(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let bogus_target = [0xDEu8; 32];

    let payload = build_reputation_payload(oracle.id, bogus_target, REP_EVENT_JOB_COMPLETED, 5);
    let tx = make_tx(oracle.id, 0, SIGNAL_FEE, payload);

    let err = apply_signal_commitment_tx(&mut db, &tx, 100).expect_err("must fail");
    assert!(
        matches!(err, ExecError::TargetEntityNotFound),
        "got {err:?}"
    );
}

// ============================================================================
// 11. Non-reputation signals continue to work (regression)
// ============================================================================

#[test]
fn non_reputation_signal_unchanged_behavior() {
    let mut db = MemKv::new();
    // Standard advisory (emit_proposals) entity, NO submit_reputation_updates.
    let mut entity = build_entity([0x33u8; 32], [0x44u8; 32], Capabilities::gated());
    entity.economic_balance = ORACLE_BALANCE;
    store_entity(&mut db, &entity);

    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8; 32],
        signal_type: AiSignalType::Anomaly,
        issuer_entity_id: entity.id,
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
        oracle_anchor: None,
    });
    let tx = make_tx(entity.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, 100).expect("non-reputation must still work");

    // Issuer's reputation is unchanged (no oracle path triggered).
    let after = read_ai_entity(&db, &entity.id).unwrap().unwrap();
    assert_eq!(after.reputation_score, DEFAULT_REPUTATION_SCORE);
    assert_eq!(after.reputation_events_count, 0);
}

// ============================================================================
// 12. ReputationUpdate payload byte-length is exactly 101
// ============================================================================

#[test]
fn reputation_payload_is_101_bytes() {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0x11u8; 32],
        signal_type: AiSignalType::ReputationUpdate,
        issuer_entity_id: [0x22u8; 32],
        reputation: Some(ReputationUpdateExtraV1 {
            target_entity_id: [0x33u8; 32],
            event_type: REP_EVENT_JOB_COMPLETED,
            points_delta: -7,
        }),
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
        oracle_anchor: None,
    });
    assert_eq!(payload.len(), 101);
    assert_eq!(payload.len(), SIGNAL_COMMITMENT_PAYLOAD_V1_REP_LEN);
    // version | hash | type | issuer | target | event | delta_be
    assert_eq!(payload[33], AiSignalType::ReputationUpdate as u8);
    assert_eq!(payload[98], REP_EVENT_JOB_COMPLETED);
    assert_eq!(&payload[99..101], &(-7i16).to_be_bytes());
}
