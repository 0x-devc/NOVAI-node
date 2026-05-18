#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]

//! Integration tests for the `PROOF_TYPE_GROTH16_REGISTERED` dispatch path
//! (Week 30, Batch 2).
//!
//! The flow under test:
//!
//! 1. An entity registers a Groth16 VK on-chain via `CreateMemoryObject`,
//!    receiving a 32-byte memory object id (the "registry handle").
//! 2. The same (or another) entity issues a `ProofSubmission` signal whose
//!    `proof_type` is `PROOF_TYPE_GROTH16_REGISTERED` and whose v2
//!    `vk_bytes` field is exactly the 32-byte registry handle.
//! 3. The handler resolves owner via `vk_registry_by_id_key`, loads the
//!    `VkRegistration`, validates `proof_type` and `code_hash` binding,
//!    dispatches to `Groth16Verifier::verify_proof` with the stored
//!    compressed VK, and on success writes a `VerificationRecord` and
//!    applies `+3` reputation to the issuer.
//!
//! Covered scenarios:
//!
//! - Happy path: registered VK is used to verify a proof end-to-end; the
//!   `VerificationRecord` carries `proof_type = PROOF_TYPE_GROTH16_REGISTERED`
//!   so audit consumers can tell the proof was VK-registry-routed.
//! - Decoder: `vk_len != 32` rejected with `RegisteredVkBadIdLength`.
//! - Dispatch: unknown registry id rejected with `VkRegistrationNotFound`.
//! - Dispatch: `code_hash` mismatch between proof and stored registration
//!   rejected with `VkRegistrationCodeHashMismatch`.
//! - Lifecycle: after `DeleteMemoryObject` on the registration, further
//!   submissions for the same handle fail with `VkRegistrationNotFound`.

use ark_bn254::{Bn254, Fr};
use ark_groth16::Groth16;
use ark_relations::{
    lc,
    r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError, Variable},
};
use ark_serialize::CanonicalSerialize;
use ark_std::rand::{rngs::StdRng, SeedableRng};

use novai_ai_entities::{
    AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObject, MemoryObjectType,
    VerificationRecordData, VkRegistrationData, VK_REGISTRATION_VERSION,
};
use novai_execution::{
    apply_create_memory_object_tx, apply_delete_memory_object_tx, apply_signal_commitment_tx,
    encode_create_memory_object_payload_v1, encode_delete_memory_object_payload_v1,
    encode_signal_commitment_payload_v1, get_memory_objects_by_entity_and_type, read_ai_entity,
    write_ai_entity_op, CreateMemoryObjectPayloadV1, DeleteMemoryObjectPayloadV1, ExecError,
    ProofSubmissionExtraV1, SignalCommitmentPayloadV1, PROOF_TYPE_GROTH16,
    PROOF_TYPE_GROTH16_REGISTERED,
};
use novai_state::{ai_entity_by_address_key, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const BALANCE: u128 = 1_000_000;
const FEE: u64 = 1_000;
const REGISTER_HEIGHT: u64 = 500;
const SUBMIT_HEIGHT: u64 = 600;
const SAMPLE_CODE_HASH: [u8; 32] = [0xC0u8; 32];
const OTHER_CODE_HASH: [u8; 32] = [0xC1u8; 32];
const SAMPLE_COMPUTATION_HASH: [u8; 32] = [0xC2u8; 32];

// ============================================================================
// Groth16 setup (sum circuit) — matches verification_system.rs
// ============================================================================

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

fn split_public_inputs_64(bytes: &[u8; 64]) -> [Fr; 4] {
    let mut out = [Fr::from(0u64); 4];
    for (i, slot) in out.iter_mut().enumerate() {
        let mut chunk = [0u8; 16];
        chunk.copy_from_slice(&bytes[i * 16..(i + 1) * 16]);
        *slot = Fr::from(u128::from_be_bytes(chunk));
    }
    out
}

/// Generate a valid Groth16 (vk, proof) pair for the SumCircuit bound to
/// the supplied `(code_hash, computation_hash)` public inputs.
fn gen_valid_groth16(
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

// ============================================================================
// Helpers
// ============================================================================

fn caps() -> Capabilities {
    Capabilities {
        read_public_chain: true,
        read_memory_objects: true,
        emit_proposals: true,
        request_execution: false,
        read_nnpx_derived: false,
        submit_reputation_updates: false,
        _reserved: [false; 2],
    }
}

fn build_entity(code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    AiEntity::new(code_hash, creator, AutonomyMode::Gated, caps(), 1)
}

fn store_entity(db: &mut MemKv, entity: &AiEntity) {
    db.apply_batch(&[
        write_ai_entity_op(entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();
}

fn make_entity(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut e = build_entity(code_hash, creator);
    e.economic_balance = BALANCE;
    e.reputation_score = 50;
    store_entity(db, &e);
    e
}

fn register_vk(
    db: &mut MemKv,
    publisher: &AiEntity,
    nonce: u64,
    code_hash: [u8; 32],
    vk_bytes: Vec<u8>,
) -> [u8; 32] {
    let reg = VkRegistrationData {
        version: VK_REGISTRATION_VERSION,
        proof_type: PROOF_TYPE_GROTH16,
        code_hash,
        label: b"sum-v1".to_vec(),
        vk_bytes,
    };
    let tx = TxV1 {
        version: TxVersion::V1,
        from: publisher.id,
        pubkey: publisher.id,
        nonce,
        fee: FEE,
        payload: encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
            object_type: MemoryObjectType::VkRegistration,
            data: reg.encode(),
        }),
        sig: [0u8; 64],
    };
    apply_create_memory_object_tx(db, &tx, REGISTER_HEIGHT).expect("register succeeds")
}

fn submit_registered_proof(
    db: &mut MemKv,
    issuer: &AiEntity,
    nonce: u64,
    registry_id: [u8; 32],
    code_hash: [u8; 32],
    computation_hash: [u8; 32],
    proof_bytes: Vec<u8>,
) -> Result<(), ExecError<<MemKv as novai_state::Kv>::Error>> {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xD0u8; 32],
        signal_type: AiSignalType::ProofSubmission,
        issuer_entity_id: issuer.id,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: Some(ProofSubmissionExtraV1 {
            proof_type: PROOF_TYPE_GROTH16_REGISTERED,
            code_hash,
            computation_hash,
            vk_bytes: registry_id.to_vec(),
            proof_bytes,
        }),
        subscription_create: None,
        subscription_cancel: None,
        payment_request: None,
        service_attestation: None,
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: issuer.id,
        pubkey: issuer.id,
        nonce,
        fee: FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_signal_commitment_tx(db, &tx, SUBMIT_HEIGHT)
}

fn submit_with_custom_vk_bytes(
    db: &mut MemKv,
    issuer: &AiEntity,
    nonce: u64,
    vk_bytes: Vec<u8>,
    code_hash: [u8; 32],
    computation_hash: [u8; 32],
    proof_bytes: Vec<u8>,
) -> Result<(), ExecError<<MemKv as novai_state::Kv>::Error>> {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xD0u8; 32],
        signal_type: AiSignalType::ProofSubmission,
        issuer_entity_id: issuer.id,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: Some(ProofSubmissionExtraV1 {
            proof_type: PROOF_TYPE_GROTH16_REGISTERED,
            code_hash,
            computation_hash,
            vk_bytes,
            proof_bytes,
        }),
        subscription_create: None,
        subscription_cancel: None,
        payment_request: None,
        service_attestation: None,
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: issuer.id,
        pubkey: issuer.id,
        nonce,
        fee: FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_signal_commitment_tx(db, &tx, SUBMIT_HEIGHT)
}

fn delete_registration(
    db: &mut MemKv,
    publisher: &AiEntity,
    nonce: u64,
    object_id: [u8; 32],
) -> Result<(), ExecError<<MemKv as novai_state::Kv>::Error>> {
    let payload =
        encode_delete_memory_object_payload_v1(&DeleteMemoryObjectPayloadV1 { object_id });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: publisher.id,
        pubkey: publisher.id,
        nonce,
        fee: FEE,
        payload: payload.to_vec(),
        sig: [0u8; 64],
    };
    apply_delete_memory_object_tx(db, &tx, SUBMIT_HEIGHT + 1)
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
// 1. Happy path: end-to-end via registered VK
// ============================================================================

#[test]
fn registered_vk_proof_submission_end_to_end() {
    let mut db = MemKv::new();
    let publisher = make_entity(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let (vk, proof) = gen_valid_groth16(0, SAMPLE_CODE_HASH, SAMPLE_COMPUTATION_HASH);

    let registry_id = register_vk(&mut db, &publisher, 0, SAMPLE_CODE_HASH, vk);
    // After registration the publisher's nonce is 1; submit the proof with
    // nonce=1.
    submit_registered_proof(
        &mut db,
        &publisher,
        1,
        registry_id,
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
        proof,
    )
    .expect("registered-VK proof verifies end-to-end");

    let records = read_verification_records(&db, &publisher.id);
    assert_eq!(records.len(), 1, "exactly one VerificationRecord written");
    let decoded = VerificationRecordData::decode(&records[0].data).unwrap();
    // proof_type recorded as GROTH16_REGISTERED so audit consumers can see
    // this was VK-registry-routed.
    assert_eq!(decoded.proof_type, PROOF_TYPE_GROTH16_REGISTERED);
    assert_eq!(decoded.code_hash, SAMPLE_CODE_HASH);
    assert_eq!(decoded.computation_hash, SAMPLE_COMPUTATION_HASH);
    assert_eq!(decoded.height, SUBMIT_HEIGHT);

    let entity = read_ai_entity(&db, &publisher.id).unwrap().unwrap();
    assert_eq!(
        entity.reputation_score, 53,
        "successful proof should bump reputation by +3 (50 -> 53)"
    );
}

#[test]
fn registered_vk_proof_submission_cross_entity() {
    // Publisher registers a VK; a different issuer submits a proof
    // referencing it. The dispatch must resolve owner via the global
    // by-id index without needing the submitter to be the publisher.
    let mut db = MemKv::new();
    let publisher = make_entity(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let issuer = make_entity(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let (vk, proof) = gen_valid_groth16(0, SAMPLE_CODE_HASH, SAMPLE_COMPUTATION_HASH);

    let registry_id = register_vk(&mut db, &publisher, 0, SAMPLE_CODE_HASH, vk);
    submit_registered_proof(
        &mut db,
        &issuer,
        0,
        registry_id,
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
        proof,
    )
    .expect("cross-entity registered-VK proof verifies");

    // VerificationRecord is owned by the issuer, not the VK publisher.
    let issuer_records = read_verification_records(&db, &issuer.id);
    assert_eq!(issuer_records.len(), 1);
    let publisher_records = read_verification_records(&db, &publisher.id);
    assert_eq!(
        publisher_records.len(),
        0,
        "publisher does not get a VerificationRecord for someone else's proof"
    );
}

// ============================================================================
// 2. Decoder rejection: vk_bytes is not 32 bytes
// ============================================================================

#[test]
fn registered_vk_decoder_rejects_wrong_id_length() {
    let mut db = MemKv::new();
    let issuer = make_entity(&mut db, [0x13u8; 32], [0x23u8; 32]);

    // 30 bytes instead of 32 — short by 2.
    let err = submit_with_custom_vk_bytes(
        &mut db,
        &issuer,
        0,
        vec![0u8; 30],
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
        Vec::new(),
    )
    .expect_err("short vk_bytes must be rejected at decode time");
    assert!(
        matches!(err, ExecError::RegisteredVkBadIdLength { actual: 30 }),
        "got {err:?}"
    );
}

#[test]
fn registered_vk_decoder_rejects_oversize_id_length() {
    let mut db = MemKv::new();
    let issuer = make_entity(&mut db, [0x14u8; 32], [0x24u8; 32]);

    // 64 bytes — too long for a registry id, well below the inline VK cap.
    let err = submit_with_custom_vk_bytes(
        &mut db,
        &issuer,
        0,
        vec![0u8; 64],
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
        Vec::new(),
    )
    .expect_err("long vk_bytes must be rejected at decode time");
    assert!(
        matches!(err, ExecError::RegisteredVkBadIdLength { actual: 64 }),
        "got {err:?}"
    );
}

// ============================================================================
// 3. Dispatch rejection: unknown registry id
// ============================================================================

#[test]
fn registered_vk_dispatch_rejects_unknown_id() {
    let mut db = MemKv::new();
    let issuer = make_entity(&mut db, [0x15u8; 32], [0x25u8; 32]);

    let bogus_id = [0xEEu8; 32];
    let err = submit_with_custom_vk_bytes(
        &mut db,
        &issuer,
        0,
        bogus_id.to_vec(),
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
        Vec::new(),
    )
    .expect_err("unknown registry id must be rejected");
    assert!(
        matches!(err, ExecError::VkRegistrationNotFound { id } if id == bogus_id),
        "got {err:?}"
    );
}

// ============================================================================
// 4. Dispatch rejection: code_hash binding mismatch
// ============================================================================

#[test]
fn registered_vk_dispatch_rejects_code_hash_mismatch() {
    let mut db = MemKv::new();
    let publisher = make_entity(&mut db, [0x16u8; 32], [0x26u8; 32]);
    let (vk, proof) = gen_valid_groth16(0, SAMPLE_CODE_HASH, SAMPLE_COMPUTATION_HASH);

    // Register the VK bound to SAMPLE_CODE_HASH.
    let registry_id = register_vk(&mut db, &publisher, 0, SAMPLE_CODE_HASH, vk);

    // Submit a proof claiming OTHER_CODE_HASH — the binding check rejects.
    let err = submit_registered_proof(
        &mut db,
        &publisher,
        1,
        registry_id,
        OTHER_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
        proof,
    )
    .expect_err("code_hash mismatch must be rejected");
    assert!(
        matches!(err, ExecError::VkRegistrationCodeHashMismatch),
        "got {err:?}"
    );

    // No record was written.
    let records = read_verification_records(&db, &publisher.id);
    assert!(records.is_empty());
}

// ============================================================================
// 5. Lifecycle: delete then submit → NotFound
// ============================================================================

#[test]
fn registered_vk_delete_then_submit_fails() {
    let mut db = MemKv::new();
    let publisher = make_entity(&mut db, [0x17u8; 32], [0x27u8; 32]);
    let (vk, proof) = gen_valid_groth16(0, SAMPLE_CODE_HASH, SAMPLE_COMPUTATION_HASH);

    let registry_id = register_vk(&mut db, &publisher, 0, SAMPLE_CODE_HASH, vk);
    delete_registration(&mut db, &publisher, 1, registry_id).expect("delete succeeds");

    // After delete the by-id index entry is gone; the submission resolves to NotFound.
    let err = submit_registered_proof(
        &mut db,
        &publisher,
        2,
        registry_id,
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
        proof,
    )
    .expect_err("submission after delete must fail");
    assert!(
        matches!(err, ExecError::VkRegistrationNotFound { id } if id == registry_id),
        "got {err:?}"
    );
}

// ============================================================================
// 6. Negative path: tampered proof against a registered VK still fails
// ============================================================================

#[test]
fn registered_vk_tampered_proof_rejected() {
    let mut db = MemKv::new();
    let publisher = make_entity(&mut db, [0x18u8; 32], [0x28u8; 32]);
    let (vk, mut proof) = gen_valid_groth16(0, SAMPLE_CODE_HASH, SAMPLE_COMPUTATION_HASH);

    let registry_id = register_vk(&mut db, &publisher, 0, SAMPLE_CODE_HASH, vk);

    // Flip a bit in the proof so pairing verification fails. Resolution
    // and binding checks still succeed; only the verifier rejects.
    proof[0] ^= 0x01;
    let err = submit_registered_proof(
        &mut db,
        &publisher,
        1,
        registry_id,
        SAMPLE_CODE_HASH,
        SAMPLE_COMPUTATION_HASH,
        proof,
    )
    .expect_err("tampered proof must fail verification");
    assert!(
        matches!(err, ExecError::ProofVerificationFailed),
        "got {err:?}"
    );
}
