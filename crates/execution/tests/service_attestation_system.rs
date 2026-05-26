#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]

//! Integration tests for ServiceAttestation (signal type 17, Week 28
//! Phase 3).
//!
//! Each test seeds two entities, processes a PaymentRequest to lay down
//! the canonical PaymentRecord, then exercises ServiceAttestation:
//!
//! - Delivered status applies +REP_DELTA_PAYMENT_DELIVERED to the payee.
//! - Failed status applies REP_DELTA_PAYMENT_FAILED to the payee.
//! - Both deltas are clamped to [0, MAX_REPUTATION_SCORE].
//! - Only the payer may attest; non-payer issuers are rejected.
//! - Unknown payment_signal_hash is rejected with PaymentNotFound.
//! - Payee mismatch (tampered tail) is rejected.
//! - Invalid status byte is rejected (defense-in-depth; the decoder
//!   catches it first, but we exercise the path explicitly).
//! - A second attestation against the same record is rejected.
//! - Attestation does NOT move economic balances.
//! - The PaymentRecord is rewritten in place with the new status and
//!   attested_height.

use novai_ai_entities::{AiEntity, AiSignalType, AutonomyMode, Capabilities, MAX_REPUTATION_SCORE};
use novai_execution::{
    apply_signal_commitment_tx, decode_payment_record_v1, encode_signal_commitment_payload_v1,
    payment_by_hash_key, read_ai_entity, write_ai_entity_op, ExecError, PaymentRequestExtraV1,
    ServiceAttestationExtraV1, SignalCommitmentPayloadV1, PAYMENT_ATTESTATION_STATUS_DELIVERED,
    PAYMENT_ATTESTATION_STATUS_FAILED, PAYMENT_ATTESTATION_STATUS_NONE,
    REP_DELTA_PAYMENT_DELIVERED, REP_DELTA_PAYMENT_FAILED,
};
use novai_state::{ai_entity_by_address_key, Kv, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const PAYER_BALANCE: u128 = 1_000_000;
const PAYEE_BALANCE: u128 = 250;
const SIGNAL_FEE: u64 = 1_000;
const PAYMENT_HEIGHT: u64 = 500;
const ATTEST_HEIGHT: u64 = 510;
const EXPIRY_HEIGHT: u64 = 1000;
const PAYMENT_AMOUNT: u64 = 1_000;

// ============================================================================
// Helpers
// ============================================================================

fn payment_caps() -> Capabilities {
    Capabilities {
        read_public_chain: true,
        read_memory_objects: false,
        emit_proposals: true,
        request_execution: false,
        read_nnpx_derived: false,
        submit_reputation_updates: false,
        post_oracle_anchors: false,
        _reserved: [false; 1],
    }
}

fn build_entity(code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    AiEntity::new(
        code_hash,
        creator,
        AutonomyMode::Gated,
        payment_caps(),
        1000,
    )
}

fn store_entity(db: &mut MemKv, entity: &AiEntity) {
    db.apply_batch(&[
        write_ai_entity_op(entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();
}

fn make_payer(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut payer = build_entity(code_hash, creator);
    payer.economic_balance = PAYER_BALANCE;
    store_entity(db, &payer);
    payer
}

fn make_payee(
    db: &mut MemKv,
    code_hash: [u8; 32],
    creator: [u8; 32],
    reputation_score: u16,
) -> AiEntity {
    let mut payee = build_entity(code_hash, creator);
    payee.economic_balance = PAYEE_BALANCE;
    payee.reputation_score = reputation_score;
    store_entity(db, &payee);
    payee
}

fn payment_payload(
    signal_hash: [u8; 32],
    payer: [u8; 32],
    payee: [u8; 32],
    amount: u64,
) -> Vec<u8> {
    encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type: AiSignalType::PaymentRequest,
        issuer_entity_id: payer,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
        subscription_create: None,
        subscription_cancel: None,
        payment_request: Some(PaymentRequestExtraV1 {
            payee_entity_id: payee,
            amount,
            service_descriptor_hash: [0xBBu8; 32],
            request_hash: [0xCCu8; 32],
            max_block_height: EXPIRY_HEIGHT,
            splits: None,
            condition: None,
        }),
        service_attestation: None,
        sla_accept: None,
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
        oracle_anchor: None,
    })
}

fn attestation_payload(
    signal_hash: [u8; 32],
    issuer: [u8; 32],
    payment_signal_hash: [u8; 32],
    payee: [u8; 32],
    status: u8,
) -> Vec<u8> {
    encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type: AiSignalType::ServiceAttestation,
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
        service_attestation: Some(ServiceAttestationExtraV1 {
            payment_signal_hash,
            payee_entity_id: payee,
            status,
        }),
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

/// Drive a fresh payment + return (payer, payee, payment_signal_hash).
fn settle_a_payment(
    db: &mut MemKv,
    payer: &AiEntity,
    payee: &AiEntity,
    payment_signal_hash: [u8; 32],
    nonce: u64,
) {
    let payload = payment_payload(payment_signal_hash, payer.id, payee.id, PAYMENT_AMOUNT);
    let tx = make_tx(payer.id, nonce, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(db, &tx, PAYMENT_HEIGHT).expect("payment settles");
}

// ============================================================================
// 1. Delivered status applies +1 to payee reputation
// ============================================================================

#[test]
fn service_attestation_delivered_increases_payee_reputation() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let payee = make_payee(&mut db, [0x12u8; 32], [0x22u8; 32], 50);

    let payment_hash = [0xAAu8; 32];
    settle_a_payment(&mut db, &payer, &payee, payment_hash, 0);

    let payee_before = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    assert_eq!(payee_before.reputation_score, 50);
    let events_before = payee_before.reputation_events_count;

    let tx = make_tx(
        payer.id,
        1,
        SIGNAL_FEE,
        attestation_payload(
            [0xDDu8; 32],
            payer.id,
            payment_hash,
            payee.id,
            PAYMENT_ATTESTATION_STATUS_DELIVERED,
        ),
    );
    apply_signal_commitment_tx(&mut db, &tx, ATTEST_HEIGHT).expect("attestation succeeds");

    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    assert_eq!(
        payee_after.reputation_score,
        50 + u16::try_from(REP_DELTA_PAYMENT_DELIVERED).unwrap(),
    );
    assert_eq!(payee_after.reputation_events_count, events_before + 1);
}

// ============================================================================
// 2. Failed status applies -3 to payee reputation
// ============================================================================

#[test]
fn service_attestation_failed_decreases_payee_reputation() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x13u8; 32], [0x23u8; 32]);
    let payee = make_payee(&mut db, [0x14u8; 32], [0x24u8; 32], 50);

    let payment_hash = [0xAAu8; 32];
    settle_a_payment(&mut db, &payer, &payee, payment_hash, 0);

    let tx = make_tx(
        payer.id,
        1,
        SIGNAL_FEE,
        attestation_payload(
            [0xDDu8; 32],
            payer.id,
            payment_hash,
            payee.id,
            PAYMENT_ATTESTATION_STATUS_FAILED,
        ),
    );
    apply_signal_commitment_tx(&mut db, &tx, ATTEST_HEIGHT).expect("attestation succeeds");

    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    // 50 + (-3) = 47.
    let expected: i32 = 50 + REP_DELTA_PAYMENT_FAILED;
    assert_eq!(
        payee_after.reputation_score,
        u16::try_from(expected).unwrap()
    );
}

// ============================================================================
// 3. Delivered at MAX score clamps (does not overflow MAX_REPUTATION_SCORE)
// ============================================================================

#[test]
fn service_attestation_delivered_clamps_at_max() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x15u8; 32], [0x25u8; 32]);
    let payee = make_payee(&mut db, [0x16u8; 32], [0x26u8; 32], MAX_REPUTATION_SCORE);

    let payment_hash = [0xAAu8; 32];
    settle_a_payment(&mut db, &payer, &payee, payment_hash, 0);

    let tx = make_tx(
        payer.id,
        1,
        SIGNAL_FEE,
        attestation_payload(
            [0xDDu8; 32],
            payer.id,
            payment_hash,
            payee.id,
            PAYMENT_ATTESTATION_STATUS_DELIVERED,
        ),
    );
    apply_signal_commitment_tx(&mut db, &tx, ATTEST_HEIGHT).expect("attestation succeeds");

    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    assert_eq!(
        payee_after.reputation_score, MAX_REPUTATION_SCORE,
        "score must clamp at MAX, not exceed it",
    );
}

// ============================================================================
// 4. Failed at low score clamps at 0 (does not underflow)
// ============================================================================

#[test]
fn service_attestation_failed_clamps_at_zero() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x17u8; 32], [0x27u8; 32]);
    // Start payee at score = 2; -3 should clamp at 0 (not wrap).
    let payee = make_payee(&mut db, [0x18u8; 32], [0x28u8; 32], 2);

    let payment_hash = [0xAAu8; 32];
    settle_a_payment(&mut db, &payer, &payee, payment_hash, 0);

    let tx = make_tx(
        payer.id,
        1,
        SIGNAL_FEE,
        attestation_payload(
            [0xDDu8; 32],
            payer.id,
            payment_hash,
            payee.id,
            PAYMENT_ATTESTATION_STATUS_FAILED,
        ),
    );
    apply_signal_commitment_tx(&mut db, &tx, ATTEST_HEIGHT).expect("attestation succeeds");

    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    assert_eq!(payee_after.reputation_score, 0, "score must floor at 0");
}

// ============================================================================
// 5. Only the payer may attest
// ============================================================================

#[test]
fn service_attestation_by_non_payer_rejected() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x19u8; 32], [0x29u8; 32]);
    let payee = make_payee(&mut db, [0x1Au8; 32], [0x2Au8; 32], 50);
    let third_party = make_payer(&mut db, [0x1Bu8; 32], [0x2Bu8; 32]);

    let payment_hash = [0xAAu8; 32];
    settle_a_payment(&mut db, &payer, &payee, payment_hash, 0);

    // third_party (not the payer) submits an attestation referencing
    // the payer-payee payment. The handler must reject because
    // record.payer != issuer.
    let tx = make_tx(
        third_party.id,
        0,
        SIGNAL_FEE,
        attestation_payload(
            [0xDDu8; 32],
            third_party.id,
            payment_hash,
            payee.id,
            PAYMENT_ATTESTATION_STATUS_DELIVERED,
        ),
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, ATTEST_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::ServiceAttestationNotPayer),
        "got {err:?}",
    );

    // Payee reputation untouched.
    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    assert_eq!(payee_after.reputation_score, 50);
}

// ============================================================================
// 6. Unknown payment_signal_hash is rejected
// ============================================================================

#[test]
fn service_attestation_unknown_payment_rejected() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x1Cu8; 32], [0x2Cu8; 32]);
    let payee = make_payee(&mut db, [0x1Du8; 32], [0x2Du8; 32], 50);
    // No PaymentRequest is settled; the by_hash record does not exist.

    let tx = make_tx(
        payer.id,
        0,
        SIGNAL_FEE,
        attestation_payload(
            [0xDDu8; 32],
            payer.id,
            [0xEEu8; 32], // arbitrary, non-existent payment hash
            payee.id,
            PAYMENT_ATTESTATION_STATUS_DELIVERED,
        ),
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, ATTEST_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::ServiceAttestationPaymentNotFound),
        "got {err:?}",
    );
}

// ============================================================================
// 7. Payee mismatch (tampered tail) is rejected
// ============================================================================

#[test]
fn service_attestation_payee_mismatch_rejected() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x1Eu8; 32], [0x2Eu8; 32]);
    let real_payee = make_payee(&mut db, [0x1Fu8; 32], [0x2Fu8; 32], 50);
    let fake_payee = make_payee(&mut db, [0x30u8; 32], [0x31u8; 32], 50);

    let payment_hash = [0xAAu8; 32];
    settle_a_payment(&mut db, &payer, &real_payee, payment_hash, 0);

    // Tail names fake_payee but the record on by_hash holds real_payee.
    let tx = make_tx(
        payer.id,
        1,
        SIGNAL_FEE,
        attestation_payload(
            [0xDDu8; 32],
            payer.id,
            payment_hash,
            fake_payee.id, // mismatch
            PAYMENT_ATTESTATION_STATUS_DELIVERED,
        ),
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, ATTEST_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::ServiceAttestationPayeeMismatch),
        "got {err:?}",
    );

    // Neither the real nor the fake payee's reputation should be touched.
    let real_after = read_ai_entity(&db, &real_payee.id).unwrap().unwrap();
    let fake_after = read_ai_entity(&db, &fake_payee.id).unwrap().unwrap();
    assert_eq!(real_after.reputation_score, 50);
    assert_eq!(fake_after.reputation_score, 50);
}

// ============================================================================
// 8. Invalid status byte is rejected
// ============================================================================

#[test]
fn service_attestation_invalid_status_rejected() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x32u8; 32], [0x33u8; 32]);
    let payee = make_payee(&mut db, [0x34u8; 32], [0x35u8; 32], 50);

    let payment_hash = [0xAAu8; 32];
    settle_a_payment(&mut db, &payer, &payee, payment_hash, 0);

    // status = 99 is above PAYMENT_ATTESTATION_STATUS_MAX. The decoder
    // catches it first and surfaces ServiceAttestationInvalidStatus.
    let tx = make_tx(
        payer.id,
        1,
        SIGNAL_FEE,
        attestation_payload([0xDDu8; 32], payer.id, payment_hash, payee.id, 99),
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, ATTEST_HEIGHT).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::ServiceAttestationInvalidStatus { status: 99 }
        ),
        "got {err:?}",
    );
}

// ============================================================================
// 9. Double attestation is rejected
// ============================================================================

#[test]
fn service_attestation_already_attested_rejected() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x36u8; 32], [0x37u8; 32]);
    let payee = make_payee(&mut db, [0x38u8; 32], [0x39u8; 32], 50);

    let payment_hash = [0xAAu8; 32];
    settle_a_payment(&mut db, &payer, &payee, payment_hash, 0);

    // First attestation: delivered.
    let tx1 = make_tx(
        payer.id,
        1,
        SIGNAL_FEE,
        attestation_payload(
            [0xD1u8; 32],
            payer.id,
            payment_hash,
            payee.id,
            PAYMENT_ATTESTATION_STATUS_DELIVERED,
        ),
    );
    apply_signal_commitment_tx(&mut db, &tx1, ATTEST_HEIGHT).expect("first attestation succeeds");

    let payee_after_first = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    let rep_after_first = payee_after_first.reputation_score;

    // Second attestation with a different signal_hash but referencing
    // the same payment must be rejected.
    let tx2 = make_tx(
        payer.id,
        2,
        SIGNAL_FEE,
        attestation_payload(
            [0xD2u8; 32],
            payer.id,
            payment_hash,
            payee.id,
            PAYMENT_ATTESTATION_STATUS_FAILED,
        ),
    );
    let err = apply_signal_commitment_tx(&mut db, &tx2, ATTEST_HEIGHT + 1).unwrap_err();
    assert!(
        matches!(err, ExecError::ServiceAttestationAlreadyAttested),
        "got {err:?}",
    );

    // Payee reputation must reflect the first attestation only.
    let payee_after_second = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    assert_eq!(payee_after_second.reputation_score, rep_after_first);
}

// ============================================================================
// 10. Attestation does NOT move economic balances
// ============================================================================

#[test]
fn service_attestation_does_not_move_funds() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x3Au8; 32], [0x3Bu8; 32]);
    let payee = make_payee(&mut db, [0x3Cu8; 32], [0x3Du8; 32], 50);

    let payment_hash = [0xAAu8; 32];
    settle_a_payment(&mut db, &payer, &payee, payment_hash, 0);

    let payer_before = read_ai_entity(&db, &payer.id).unwrap().unwrap();
    let payee_before = read_ai_entity(&db, &payee.id).unwrap().unwrap();

    let tx = make_tx(
        payer.id,
        1,
        SIGNAL_FEE,
        attestation_payload(
            [0xDDu8; 32],
            payer.id,
            payment_hash,
            payee.id,
            PAYMENT_ATTESTATION_STATUS_DELIVERED,
        ),
    );
    apply_signal_commitment_tx(&mut db, &tx, ATTEST_HEIGHT).expect("attestation succeeds");

    let payer_after = read_ai_entity(&db, &payer.id).unwrap().unwrap();
    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    assert_eq!(
        payer_after.economic_balance,
        payer_before.economic_balance - u128::from(SIGNAL_FEE),
        "payer pays only the tx fee for the attestation; no value transfer",
    );
    assert_eq!(
        payee_after.economic_balance, payee_before.economic_balance,
        "payee balance is unchanged by attestation",
    );
}

// ============================================================================
// 11. PaymentRecord is rewritten in place with attestation status + height
// ============================================================================

#[test]
fn service_attestation_updates_record_in_place() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x3Eu8; 32], [0x3Fu8; 32]);
    let payee = make_payee(&mut db, [0x40u8; 32], [0x41u8; 32], 50);

    let payment_hash = [0xAAu8; 32];
    settle_a_payment(&mut db, &payer, &payee, payment_hash, 0);

    // Pre-attestation: record carries sentinel status and zero height.
    let before_bytes = db
        .get(&payment_by_hash_key(&payment_hash))
        .unwrap()
        .unwrap();
    let before = decode_payment_record_v1(&before_bytes).unwrap();
    assert_eq!(before.attested_status, PAYMENT_ATTESTATION_STATUS_NONE);
    assert_eq!(before.attested_height, 0);

    let tx = make_tx(
        payer.id,
        1,
        SIGNAL_FEE,
        attestation_payload(
            [0xDDu8; 32],
            payer.id,
            payment_hash,
            payee.id,
            PAYMENT_ATTESTATION_STATUS_FAILED,
        ),
    );
    apply_signal_commitment_tx(&mut db, &tx, ATTEST_HEIGHT).expect("attestation succeeds");

    // Post-attestation: same key, updated bytes.
    let after_bytes = db
        .get(&payment_by_hash_key(&payment_hash))
        .unwrap()
        .unwrap();
    let after = decode_payment_record_v1(&after_bytes).unwrap();
    assert_eq!(after.attested_status, PAYMENT_ATTESTATION_STATUS_FAILED);
    assert_eq!(after.attested_height, ATTEST_HEIGHT);

    // Every other field is preserved verbatim from the original record.
    assert_eq!(after.payer, before.payer);
    assert_eq!(after.payee, before.payee);
    assert_eq!(after.amount, before.amount);
    assert_eq!(
        after.service_descriptor_hash,
        before.service_descriptor_hash
    );
    assert_eq!(after.request_hash, before.request_hash);
    assert_eq!(after.payment_height, before.payment_height);
    assert_eq!(after.max_block_height, before.max_block_height);
}
