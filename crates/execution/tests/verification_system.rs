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
    ProofSubmissionExtraV1, SignalCommitmentPayloadV1, PROOF_TYPE_GROTH16, PROOF_TYPE_PLONK,
    PROOF_TYPE_STUB, SIGNAL_COMMITMENT_PAYLOAD_V1_PROOF_LEN,
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
    build_proof_submission_payload_v2(
        issuer,
        proof_type,
        code_hash,
        computation_hash,
        Vec::new(),
        Vec::new(),
    )
}

fn build_proof_submission_payload_v2(
    issuer: [u8; 32],
    proof_type: u8,
    code_hash: [u8; 32],
    computation_hash: [u8; 32],
    vk_bytes: Vec<u8>,
    proof_bytes: Vec<u8>,
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
            vk_bytes,
            proof_bytes,
        }),
        subscription_create: None,
        subscription_cancel: None,
        payment_request: None,
        service_attestation: None,
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

    // PROOF_TYPE_PLONK (= 2) is reserved but above PROOF_TYPE_MAX
    // (PROOF_TYPE_GROTH16 = 1 is now activated).
    let payload = build_proof_submission_payload(
        issuer.id,
        PROOF_TYPE_PLONK,
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
    );
    let err =
        apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, payload), HEIGHT)
            .expect_err("unsupported proof_type must reject");
    assert!(
        matches!(err, ExecError::UnsupportedProofType { proof_type: 2 }),
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
    assert!(StubZkVerifier::verify_proof(&[], &[], &[], 0, &code_hash));
    assert!(StubZkVerifier::verify_proof(
        b"any", b"vk", b"inputs", 0, &code_hash
    ));
    assert!(StubZkVerifier::verify_proof(
        &[0u8; 1024],
        &[0u8; 256],
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
        payment_request: None,
        service_attestation: None,
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

#[test]
fn golden_vector_proof_payload_v2_groth16() {
    let issuer = [0x22u8; 32];
    let code_hash = [0x55u8; 32];
    let computation_hash = [0x77u8; 32];
    // Arbitrary placeholder vk/proof bytes — the encoder does not validate
    // cryptographic structure, only the wire layout.
    let vk_bytes: Vec<u8> = (0u8..7).collect(); // 7 bytes
    let proof_bytes: Vec<u8> = (0xF0u8..=0xF3).collect(); // 4 bytes

    let payload = build_proof_submission_payload_v2(
        issuer,
        PROOF_TYPE_GROTH16,
        code_hash,
        computation_hash,
        vk_bytes.clone(),
        proof_bytes.clone(),
    );

    // Total = 131 (v1 prefix) + 4 (vk_len) + 7 (vk) + 4 (proof_len) + 4 (proof) = 150
    assert_eq!(payload.len(), 150);

    // Frozen v1 prefix — must remain bit-for-bit identical to the v1 golden
    // vector so existing observers can parse it without branching.
    assert_eq!(payload[0], 2, "version byte");
    assert_eq!(&payload[1..33], &[0xC4u8; 32], "signal_hash at 1..33");
    assert_eq!(
        payload[33],
        AiSignalType::ProofSubmission.to_byte(),
        "signal_type byte at 33"
    );
    assert_eq!(&payload[34..66], &issuer, "issuer_entity_id at 34..66");
    assert_eq!(payload[66], PROOF_TYPE_GROTH16, "proof_type at 66");
    assert_eq!(&payload[67..99], &code_hash, "code_hash at 67..99");
    assert_eq!(
        &payload[99..131],
        &computation_hash,
        "computation_hash at 99..131"
    );

    // v2 tail: vk_len_be:4 | vk_bytes | proof_len_be:4 | proof_bytes.
    assert_eq!(
        &payload[131..135],
        &7u32.to_be_bytes(),
        "vk_len (big-endian u32) at 131..135"
    );
    assert_eq!(
        &payload[135..142],
        vk_bytes.as_slice(),
        "vk_bytes at 135..142"
    );
    assert_eq!(
        &payload[142..146],
        &4u32.to_be_bytes(),
        "proof_len (big-endian u32) at 142..146"
    );
    assert_eq!(
        &payload[146..150],
        proof_bytes.as_slice(),
        "proof_bytes at 146..150"
    );
}

#[test]
fn proof_payload_v2_roundtrip() {
    use novai_execution::decode_signal_commitment_payload_v1;

    let issuer = [0x33u8; 32];
    let code_hash = [0x11u8; 32];
    let computation_hash = [0x22u8; 32];
    let vk_bytes: Vec<u8> = (0u8..200).collect();
    let proof_bytes: Vec<u8> = (0u8..128).collect();

    let payload = build_proof_submission_payload_v2(
        issuer,
        PROOF_TYPE_GROTH16,
        code_hash,
        computation_hash,
        vk_bytes.clone(),
        proof_bytes.clone(),
    );

    let decoded = decode_signal_commitment_payload_v1(&payload).expect("v2 roundtrip");
    let extra = decoded.proof_submission.expect("proof_submission present");
    assert_eq!(extra.proof_type, PROOF_TYPE_GROTH16);
    assert_eq!(extra.code_hash, code_hash);
    assert_eq!(extra.computation_hash, computation_hash);
    assert_eq!(extra.vk_bytes, vk_bytes);
    assert_eq!(extra.proof_bytes, proof_bytes);
}

#[test]
fn proof_payload_v2_oversized_vk_rejected() {
    use novai_execution::{decode_signal_commitment_payload_v1, PROOF_SUBMISSION_MAX_VK_BYTES};

    let payload = build_proof_submission_payload_v2(
        [0u8; 32],
        PROOF_TYPE_GROTH16,
        [0u8; 32],
        [0u8; 32],
        vec![0u8; PROOF_SUBMISSION_MAX_VK_BYTES + 1],
        Vec::new(),
    );
    let err = decode_signal_commitment_payload_v1(&payload).expect_err("oversized vk must reject");
    assert!(
        matches!(err, ExecError::VerifyingKeyTooLarge { actual, max }
            if actual == PROOF_SUBMISSION_MAX_VK_BYTES + 1
            && max == PROOF_SUBMISSION_MAX_VK_BYTES),
        "got {err:?}"
    );
}

#[test]
fn proof_payload_v2_oversized_proof_rejected() {
    use novai_execution::{decode_signal_commitment_payload_v1, PROOF_SUBMISSION_MAX_PROOF_BYTES};

    let payload = build_proof_submission_payload_v2(
        [0u8; 32],
        PROOF_TYPE_GROTH16,
        [0u8; 32],
        [0u8; 32],
        Vec::new(),
        vec![0u8; PROOF_SUBMISSION_MAX_PROOF_BYTES + 1],
    );
    let err =
        decode_signal_commitment_payload_v1(&payload).expect_err("oversized proof must reject");
    assert!(
        matches!(err, ExecError::ProofBytesTooLarge { actual, max }
            if actual == PROOF_SUBMISSION_MAX_PROOF_BYTES + 1
            && max == PROOF_SUBMISSION_MAX_PROOF_BYTES),
        "got {err:?}"
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

// ============================================================================
// 9. End-to-end Groth16 verification (Phase 5)
// ============================================================================
//
// Drives real BN254 Groth16 proofs through the full ProofSubmission signal
// pipeline: trusted setup -> prove -> serialize -> build v2 payload ->
// apply_signal_commitment_tx -> assert verifier outcome + state effects.
// The circuit is a trivial 4-public-input sum circuit that matches the
// verifier's hi/lo public-input mapping (see Groth16Verifier doc); it
// proves nothing useful, but exercises every code path on the on-chain
// side.

use ark_bn254::{Bn254, Fr};
use ark_groth16::Groth16;
use ark_relations::{
    lc,
    r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError, Variable},
};
use ark_serialize::CanonicalSerialize;
use ark_std::rand::{rngs::StdRng, SeedableRng};

/// Trivial sum circuit (4 public inputs, 1 witness): `w * 1 = c0 + c1 + c2 + c3`.
/// Mirrors the unit-test circuit in `crates/crypto/src/zk.rs` so the
/// integration suite exercises the same constraint shape.
struct SumCircuit {
    public_inputs: [Option<Fr>; 4],
    witness: Option<Fr>,
}

impl ConstraintSynthesizer<Fr> for SumCircuit {
    fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> Result<(), SynthesisError> {
        let c0 = cs.new_input_variable(|| {
            self.public_inputs[0].ok_or(SynthesisError::AssignmentMissing)
        })?;
        let c1 = cs.new_input_variable(|| {
            self.public_inputs[1].ok_or(SynthesisError::AssignmentMissing)
        })?;
        let c2 = cs.new_input_variable(|| {
            self.public_inputs[2].ok_or(SynthesisError::AssignmentMissing)
        })?;
        let c3 = cs.new_input_variable(|| {
            self.public_inputs[3].ok_or(SynthesisError::AssignmentMissing)
        })?;
        let w =
            cs.new_witness_variable(|| self.witness.ok_or(SynthesisError::AssignmentMissing))?;
        cs.enforce_constraint(lc!() + w, lc!() + Variable::One, lc!() + c0 + c1 + c2 + c3)?;
        Ok(())
    }
}

/// Split a 64-byte buffer (`code_hash || computation_hash`) into 4 BN254
/// scalars the same way `Groth16Verifier` does internally — big-endian
/// 16-byte halves lifted into `Fr` via `u128::from_be_bytes`.
fn split_public_inputs_64(bytes: &[u8; 64]) -> [Fr; 4] {
    let mut out = [Fr::from(0u64); 4];
    for (i, slot) in out.iter_mut().enumerate() {
        let mut chunk = [0u8; 16];
        chunk.copy_from_slice(&bytes[i * 16..(i + 1) * 16]);
        *slot = Fr::from(u128::from_be_bytes(chunk));
    }
    out
}

/// Generate a fresh valid `(vk_bytes, proof_bytes)` triple bound to the
/// supplied `(code_hash, computation_hash)`. Seeded so test re-runs produce
/// identical bytes — useful when diffing failures.
fn gen_valid_groth16_proof(
    seed: u64,
    code_hash: [u8; 32],
    computation_hash: [u8; 32],
) -> (Vec<u8>, Vec<u8>) {
    let mut rng = StdRng::seed_from_u64(seed);
    let setup_circuit = SumCircuit {
        public_inputs: [None; 4],
        witness: None,
    };
    let pk = Groth16::<Bn254>::generate_random_parameters_with_reduction(setup_circuit, &mut rng)
        .expect("trusted setup");

    let mut public_inputs_bytes = [0u8; 64];
    public_inputs_bytes[..32].copy_from_slice(&code_hash);
    public_inputs_bytes[32..].copy_from_slice(&computation_hash);
    let fr_inputs = split_public_inputs_64(&public_inputs_bytes);
    let witness = fr_inputs[0] + fr_inputs[1] + fr_inputs[2] + fr_inputs[3];

    let prove_circuit = SumCircuit {
        public_inputs: fr_inputs.map(Some),
        witness: Some(witness),
    };
    let proof = Groth16::<Bn254>::create_random_proof_with_reduction(prove_circuit, &pk, &mut rng)
        .expect("prove");

    let mut vk_bytes = Vec::new();
    pk.vk
        .serialize_compressed(&mut vk_bytes)
        .expect("vk serialize");
    let mut proof_bytes = Vec::new();
    proof
        .serialize_compressed(&mut proof_bytes)
        .expect("proof serialize");

    (vk_bytes, proof_bytes)
}

const G_CODE_HASH: [u8; 32] = [0xC0u8; 32];
const G_COMP_HASH: [u8; 32] = [0xC1u8; 32];

#[test]
fn groth16_valid_proof_accepted() {
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let (vk, proof) = gen_valid_groth16_proof(0, G_CODE_HASH, G_COMP_HASH);
    let payload = build_proof_submission_payload_v2(
        issuer.id,
        PROOF_TYPE_GROTH16,
        G_CODE_HASH,
        G_COMP_HASH,
        vk,
        proof,
    );
    apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, payload), HEIGHT)
        .expect("valid Groth16 proof must be accepted");

    let records = read_verification_records(&db, &issuer.id);
    assert_eq!(records.len(), 1, "valid proof must create a record");
}

#[test]
fn groth16_tampered_proof_rejected() {
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let (vk, mut proof) = gen_valid_groth16_proof(0, G_CODE_HASH, G_COMP_HASH);
    proof[0] ^= 0x01; // flip one bit
    let payload = build_proof_submission_payload_v2(
        issuer.id,
        PROOF_TYPE_GROTH16,
        G_CODE_HASH,
        G_COMP_HASH,
        vk,
        proof,
    );
    let err =
        apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, payload), HEIGHT)
            .expect_err("tampered proof must reject");
    assert!(
        matches!(err, ExecError::ProofVerificationFailed),
        "got {err:?}"
    );

    let records = read_verification_records(&db, &issuer.id);
    assert!(records.is_empty(), "rejection must not create a record");
}

#[test]
fn groth16_invalid_proof_rejected() {
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    // Valid VK but garbage proof bytes (not a serializable Proof).
    let (vk, proof) = gen_valid_groth16_proof(0, G_CODE_HASH, G_COMP_HASH);
    let mut bad_proof = proof;
    bad_proof.fill(0xFF);
    let payload = build_proof_submission_payload_v2(
        issuer.id,
        PROOF_TYPE_GROTH16,
        G_CODE_HASH,
        G_COMP_HASH,
        vk,
        bad_proof,
    );
    let err =
        apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, payload), HEIGHT)
            .expect_err("invalid proof must reject");
    assert!(
        matches!(err, ExecError::ProofVerificationFailed),
        "got {err:?}"
    );
}

#[test]
fn groth16_wrong_vk_rejected() {
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let (_, proof) = gen_valid_groth16_proof(0, G_CODE_HASH, G_COMP_HASH);
    // Different seed -> different trusted setup -> different VK.
    let (other_vk, _) = gen_valid_groth16_proof(1, G_CODE_HASH, G_COMP_HASH);
    let payload = build_proof_submission_payload_v2(
        issuer.id,
        PROOF_TYPE_GROTH16,
        G_CODE_HASH,
        G_COMP_HASH,
        other_vk,
        proof,
    );
    let err =
        apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, payload), HEIGHT)
            .expect_err("wrong VK must reject");
    assert!(
        matches!(err, ExecError::ProofVerificationFailed),
        "got {err:?}"
    );
}

#[test]
fn groth16_wrong_public_inputs_rejected() {
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let (vk, proof) = gen_valid_groth16_proof(0, G_CODE_HASH, G_COMP_HASH);
    // Proof was bound to G_COMP_HASH; submit with a different one — the
    // verifier reconstructs public_inputs from the on-chain
    // (code_hash || computation_hash), so this must reject.
    let mut wrong_comp = G_COMP_HASH;
    wrong_comp[0] ^= 0xFF;
    let payload = build_proof_submission_payload_v2(
        issuer.id,
        PROOF_TYPE_GROTH16,
        G_CODE_HASH,
        wrong_comp,
        vk,
        proof,
    );
    let err =
        apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, payload), HEIGHT)
            .expect_err("wrong public inputs must reject");
    assert!(
        matches!(err, ExecError::ProofVerificationFailed),
        "got {err:?}"
    );
}

#[test]
fn groth16_verification_record_created_on_success() {
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let (vk, proof) = gen_valid_groth16_proof(0, G_CODE_HASH, G_COMP_HASH);
    let expected_proof_hash = *blake3::hash(&proof).as_bytes();
    let payload = build_proof_submission_payload_v2(
        issuer.id,
        PROOF_TYPE_GROTH16,
        G_CODE_HASH,
        G_COMP_HASH,
        vk,
        proof,
    );
    apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, payload), HEIGHT)
        .expect("valid Groth16 proof must be accepted");

    let records = read_verification_records(&db, &issuer.id);
    assert_eq!(records.len(), 1);
    let decoded = VerificationRecordData::decode(&records[0].data).expect("decode record");
    assert_eq!(decoded.proof_type, PROOF_TYPE_GROTH16);
    assert_eq!(decoded.code_hash, G_CODE_HASH);
    assert_eq!(decoded.computation_hash, G_COMP_HASH);
    assert_eq!(
        decoded.proof_hash, expected_proof_hash,
        "proof_hash must be blake3 of the real proof bytes (not the empty slice)"
    );
    assert_eq!(decoded.height, HEIGHT);
}

#[test]
fn groth16_reputation_updated_on_success() {
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let rep_before = issuer.reputation_score;
    let events_before = issuer.reputation_events_count;

    let (vk, proof) = gen_valid_groth16_proof(0, G_CODE_HASH, G_COMP_HASH);
    let payload = build_proof_submission_payload_v2(
        issuer.id,
        PROOF_TYPE_GROTH16,
        G_CODE_HASH,
        G_COMP_HASH,
        vk,
        proof,
    );
    apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, payload), HEIGHT)
        .expect("valid Groth16 proof must be accepted");

    let after = read_ai_entity(&db, &issuer.id).unwrap().unwrap();
    assert_eq!(after.reputation_score, rep_before + 3);
    assert_eq!(after.reputation_events_count, events_before + 1);
}

#[test]
fn groth16_proof_type_0_still_works() {
    // Regression: the stub (PROOF_TYPE_STUB = 0) path must keep its v1
    // wire layout and always-accept semantics after Groth16 is activated.
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let payload = build_proof_submission_payload(
        issuer.id,
        PROOF_TYPE_STUB,
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
    );
    assert_eq!(
        payload.len(),
        SIGNAL_COMMITMENT_PAYLOAD_V1_PROOF_LEN,
        "stub path keeps 131-byte v1 layout"
    );
    apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, payload), HEIGHT)
        .expect("stub path still succeeds");

    let records = read_verification_records(&db, &issuer.id);
    assert_eq!(records.len(), 1);
    let decoded = VerificationRecordData::decode(&records[0].data).unwrap();
    assert_eq!(decoded.proof_type, PROOF_TYPE_STUB);
    // Stub's "proof_bytes" is empty, so proof_hash is blake3 of the empty slice.
    assert_eq!(decoded.proof_hash, *blake3::hash(&[]).as_bytes());
}

#[test]
fn groth16_proof_type_2_still_rejected() {
    // PLONK (proof_type = 2) remains above PROOF_TYPE_MAX after the
    // Groth16 activation, so the decoder must still reject it. Mirrors
    // proof_submission_unsupported_type_rejected; kept separately as a
    // forward-looking guard against another premature PROOF_TYPE_MAX bump.
    let mut db = MemKv::new();
    let issuer = make_issuer(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let payload = build_proof_submission_payload(
        issuer.id,
        PROOF_TYPE_PLONK,
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
    );
    let err =
        apply_signal_commitment_tx(&mut db, &make_tx(issuer.id, 0, SIGNAL_FEE, payload), HEIGHT)
            .expect_err("PLONK must still reject");
    assert!(
        matches!(err, ExecError::UnsupportedProofType { proof_type: 2 }),
        "got {err:?}"
    );
}
