#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

//! Integration tests for the ZK ProofSubmission signal + VerificationRecord
//! memory object (Phase 5 / Phase 6 of the Verification System work).
//!
//! Covers:
//! - Smoke: a stub-typed proof submission succeeds end-to-end.
//! - VerificationRecord memory object is persisted with correct fields.
//! - Issuer reputation is boosted by +3 on success.
//! - Unsupported proof_type rejected with `UnsupportedProofType`.
//! - Inactive entities cannot submit proofs (gate inherited from common
//!   signal-handler entry).
//! - VerificationRecord roundtrip / fixed 105-byte size sanity at the
//!   integration layer.
//! - Stub verifier always returns true (regression on the trait shape).
//! - Regression: non-proof signals (Anomaly) still flow.
//! - Golden vector: 131-byte ProofSubmission payload with frozen offsets.
//! - Multiple proofs from the same entity all produce distinct records.

use novai_ai_entities::{
    AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObject, MemoryObjectType,
    VerificationRecordData, VERIFICATION_RECORD_SIZE,
};
use novai_crypto::{StubZkVerifier, ZkVerifier};
use novai_execution::{
    apply_signal_commitment_tx, encode_signal_commitment_payload_v1,
    get_memory_objects_by_entity_and_type, read_ai_entity, write_ai_entity_op, ExecError,
    ProofSubmissionExtraV1, SignalCommitmentPayloadV1, PROOF_TYPE_GROTH16, PROOF_TYPE_STUB,
    SIGNAL_COMMITMENT_PAYLOAD_V1_PROOF_LEN,
};
use novai_state::{ai_entity_by_address_key, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const ISSUER_BALANCE: u128 = 1_000_000;
const SIGNAL_FEE: u64 = 1_000;
const HEIGHT: u64 = 100;

const SAMPLE_CODE_HASH: [u8; 32] = [0xA1u8; 32];
const SAMPLE_COMPUTATION_HASH: [u8; 32] = [0xB2u8; 32];

// ============================================================================
// Helpers
// ============================================================================

fn issuer_caps() -> Capabilities {
    // ProofSubmission only requires emit_proposals (the signal-handler
    // entry gate). submit_reputation_updates is intentionally false —
    // the +3 self-reputation event the handler applies does NOT route
    // through that capability.
    Capabilities {
        read_public_chain: true,
        read_memory_objects: false,
        emit_proposals: true,
        request_execution: false,
        read_nnpx_derived: false,
        submit_reputation_updates: false,
        _reserved: [false; 2],
    }
}

fn build_entity(code_hash: [u8; 32], creator: [u8; 32], caps: Capabilities) -> AiEntity {
    AiEntity::new(code_hash, creator, AutonomyMode::Gated, caps, 1)
}

fn store_entity(db: &mut MemKv, entity: &AiEntity) {
    db.apply_batch(&[
        write_ai_entity_op(entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();
}

fn make_issuer(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut e = build_entity(code_hash, creator, issuer_caps());
    e.economic_balance = ISSUER_BALANCE;
    e.reputation_score = 50;
    store_entity(db, &e);
    e
}

fn build_proof_submission_payload(
    issuer: [u8; 32],
    proof_type: u8,
    code_hash: [u8; 32],
    computation_hash: [u8; 32],
) -> Vec<u8> {
    encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xC4u8; 32],
        signal_type: AiSignalType::ProofSubmission,
        issuer_entity_id: issuer,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: Some(ProofSubmissionExtraV1 {
            proof_type,
            code_hash,
            computation_hash,
        }),
        subscription_create: None,
        subscription_cancel: None,
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

fn read_verification_records(db: &MemKv, entity_id: &[u8; 32]) -> Vec<MemoryObject> {
    get_memory_objects_by_entity_and_type(
        db,
        entity_id,
        MemoryObjectType::VerificationRecord.to_byte(),
    )
    .expect("scan VerificationRecord by type")
}

// ============================================================================
// 1. Smoke + record creation + reputation
// ============================================================================

#[test]
fn proof_submission_basic_stub_passes() {
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let payload = build_proof_submission_payload(
        issuer.id,
        PROOF_TYPE_STUB,
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
    );
    apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, payload), HEIGHT)
        .expect("stub-typed proof submission must succeed");
}

#[test]
fn proof_submission_creates_verification_record() {
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let payload = build_proof_submission_payload(
        issuer.id,
        PROOF_TYPE_STUB,
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
    );
    apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, payload), HEIGHT)
        .unwrap();

    let records = read_verification_records(&db, &issuer.id);
    assert_eq!(records.len(), 1, "exactly one VerificationRecord emitted");
    let record = &records[0];
    assert_eq!(record.object_type, MemoryObjectType::VerificationRecord);
    assert_eq!(record.owner_entity, issuer.id);
    assert_eq!(record.created_at, HEIGHT);

    let decoded = VerificationRecordData::decode(&record.data).expect("decode record");
    assert_eq!(decoded.proof_type, PROOF_TYPE_STUB);
    assert_eq!(decoded.code_hash, SAMPLE_CODE_HASH);
    assert_eq!(decoded.computation_hash, SAMPLE_COMPUTATION_HASH);
    assert_eq!(decoded.height, HEIGHT);
    // proof_hash is blake3(empty) in v1 (no proof bytes carried inline yet).
    assert_eq!(decoded.proof_hash, *blake3::hash(&[]).as_bytes());
}

#[test]
fn proof_submission_boosts_reputation() {
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let before = read_ai_entity(&db, &issuer.id).unwrap().unwrap();
    let rep_before = before.reputation_score;
    let events_before = before.reputation_events_count;

    let payload = build_proof_submission_payload(
        issuer.id,
        PROOF_TYPE_STUB,
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
    );
    apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, payload), HEIGHT)
        .unwrap();

    let after = read_ai_entity(&db, &issuer.id).unwrap().unwrap();
    assert_eq!(
        after.reputation_score,
        rep_before + 3,
        "delta +3 applied on verified proof"
    );
    assert_eq!(after.reputation_events_count, events_before + 1);
}

// ============================================================================
// 2. Rejection paths
// ============================================================================

#[test]
fn proof_submission_unsupported_type_rejected() {
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    // PROOF_TYPE_GROTH16 (= 1) is reserved but above PROOF_TYPE_MAX in v1.
    let payload = build_proof_submission_payload(
        issuer.id,
        PROOF_TYPE_GROTH16,
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
    );
    let err =
        apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, payload), HEIGHT)
            .expect_err("unsupported proof_type must reject");
    assert!(
        matches!(err, ExecError::UnsupportedProofType { proof_type: 1 }),
        "got {err:?}"
    );

    // No record should have been created on the rejection path.
    let records = read_verification_records(&db, &issuer.id);
    assert!(records.is_empty(), "rejection must not produce a record");
}

#[test]
fn proof_submission_inactive_entity_rejected() {
    let mut db = MemKv::new();
    // Build an issuer with is_active = false; the common signal-handler
    // entry rejects with EntityNotActive before the ProofSubmission
    // branch ever runs.
    let mut issuer = build_entity([0x11u8; 32], [0x21u8; 32], issuer_caps());
    issuer.economic_balance = ISSUER_BALANCE;
    issuer.is_active = false;
    store_entity(&mut db, &issuer);

    let payload = build_proof_submission_payload(
        issuer.id,
        PROOF_TYPE_STUB,
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
    );
    let err =
        apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, payload), HEIGHT)
            .expect_err("inactive entity must reject");
    assert!(matches!(err, ExecError::EntityNotActive), "got {err:?}");
}

// ============================================================================
// 3. Field correctness
// ============================================================================

#[test]
fn proof_submission_records_correct_height() {
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let custom_height: u64 = 7_777;
    let payload = build_proof_submission_payload(
        issuer.id,
        PROOF_TYPE_STUB,
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
    );
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(issuer.id, 0, SIGNAL_FEE, payload),
        custom_height,
    )
    .unwrap();

    let records = read_verification_records(&db, &issuer.id);
    let record = &records[0];
    assert_eq!(record.created_at, custom_height);
    let decoded = VerificationRecordData::decode(&record.data).unwrap();
    assert_eq!(decoded.height, custom_height);
}

// ============================================================================
// 4. VerificationRecord codec sanity (mirrored from ai_entities unit
//    tests so an integration regression also catches a layout drift).
// ============================================================================

#[test]
fn verification_record_roundtrip() {
    let record = VerificationRecordData {
        proof_type: PROOF_TYPE_STUB,
        code_hash: SAMPLE_CODE_HASH,
        computation_hash: SAMPLE_COMPUTATION_HASH,
        proof_hash: [0xC3u8; 32],
        height: 0xDEAD_BEEF,
    };
    let encoded = record.encode();
    let decoded = VerificationRecordData::decode(&encoded).expect("decode");
    assert_eq!(decoded, record);
}

#[test]
fn verification_record_max_size() {
    let record = VerificationRecordData {
        proof_type: PROOF_TYPE_STUB,
        code_hash: SAMPLE_CODE_HASH,
        computation_hash: SAMPLE_COMPUTATION_HASH,
        proof_hash: [0u8; 32],
        height: 0,
    };
    assert_eq!(record.encode().len(), 105);
    assert_eq!(record.encode().len(), VERIFICATION_RECORD_SIZE);
}

// ============================================================================
// 5. Stub verifier behavior (regression on the trait shape)
// ============================================================================

#[test]
fn stub_verifier_always_passes() {
    let code_hash = [0u8; 32];
    assert!(StubZkVerifier::verify_proof(&[], &[], 0, &code_hash));
    assert!(StubZkVerifier::verify_proof(
        b"any", b"inputs", 0, &code_hash
    ));
    assert!(StubZkVerifier::verify_proof(
        &[0u8; 1024],
        &[0u8; 64],
        255,
        &[0xFFu8; 32]
    ));
}

// ============================================================================
// 6. Regression: non-proof signals still work
// ============================================================================

#[test]
fn non_proof_signals_still_work() {
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let anomaly = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8; 32],
        signal_type: AiSignalType::Anomaly,
        issuer_entity_id: issuer.id,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
        subscription_create: None,
        subscription_cancel: None,
    });
    apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, anomaly), HEIGHT)
        .expect("base anomaly still applies; ProofSubmission doesn't break it");

    // Anomaly does not boost reputation.
    let after = read_ai_entity(&db, &issuer.id).unwrap().unwrap();
    assert_eq!(after.reputation_events_count, 0);

    // Anomaly does not create a VerificationRecord.
    let records = read_verification_records(&db, &issuer.id);
    assert!(records.is_empty());
}

// ============================================================================
// 7. Golden vector
// ============================================================================

#[test]
fn golden_vector_proof_payload_131_bytes() {
    let issuer = [0x22u8; 32];
    let code_hash = [0x55u8; 32];
    let computation_hash = [0x77u8; 32];
    let payload =
        build_proof_submission_payload(issuer, PROOF_TYPE_STUB, code_hash, computation_hash);

    assert_eq!(payload.len(), 131);
    assert_eq!(payload.len(), SIGNAL_COMMITMENT_PAYLOAD_V1_PROOF_LEN);

    // Frozen field offsets — moving any of these is a wire-format break.
    assert_eq!(payload[0], 2, "version byte");
    assert_eq!(&payload[1..33], &[0xC4u8; 32], "signal_hash at 1..33");
    assert_eq!(
        payload[33],
        AiSignalType::ProofSubmission.to_byte(),
        "signal_type byte at 33"
    );
    assert_eq!(payload[33], 13, "ProofSubmission discriminant is 13");
    assert_eq!(&payload[34..66], &issuer, "issuer_entity_id at 34..66");
    assert_eq!(payload[66], PROOF_TYPE_STUB, "proof_type at 66");
    assert_eq!(&payload[67..99], &code_hash, "code_hash at 67..99");
    assert_eq!(
        &payload[99..131],
        &computation_hash,
        "computation_hash at 99..131"
    );
}

// ============================================================================
// 8. Multiple proofs from the same entity
// ============================================================================

#[test]
fn multiple_proofs_same_entity_all_recorded() {
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    for nonce in 0u64..3 {
        // Distinct computation_hash per submission so each VerificationRecord
        // has a distinct object_id (id is a hash over owner+type+height+data).
        let mut computation_hash = SAMPLE_COMPUTATION_HASH;
        computation_hash[0] = nonce as u8;
        let payload = build_proof_submission_payload(
            issuer.id,
            PROOF_TYPE_STUB,
            SAMPLE_CODE_HASH,
            computation_hash,
        );
        apply_signal_commitment_tx(
            &mut db,
            &make_tx(issuer.id, nonce, SIGNAL_FEE, payload),
            HEIGHT + nonce,
        )
        .expect("each proof submission succeeds");
    }

    let records = read_verification_records(&db, &issuer.id);
    assert_eq!(records.len(), 3, "all three proofs recorded (no dedup)");

    let after = read_ai_entity(&db, &issuer.id).unwrap().unwrap();
    assert_eq!(after.reputation_events_count, 3, "+3 each time");
}
