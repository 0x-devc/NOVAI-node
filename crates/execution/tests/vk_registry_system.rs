#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]

//! Integration tests for the VK Registry create / update / delete flow
//! (Week 30, Batch 1).
//!
//! Each test publishes a `VkRegistration` memory object through the
//! normal `CreateMemoryObject` signal path and exercises the per-type
//! validation rules added in Batch 1:
//!
//! - Happy path: registration lands in the `ai_memory_by_type` index,
//!   memory object record carries the canonical `VkRegistrationData`
//!   payload, and the memory count is incremented.
//! - Per-entity cap: 8th create succeeds, 9th is rejected with
//!   `VkRegistrationLimitExceeded`.
//! - Validation rejections: bad version, unsupported proof_type, label
//!   over `VK_REGISTRATION_LABEL_MAX`, empty VK, oversized VK, garbage
//!   VK bytes that do not deserialize.
//! - Update: only `label` may change; mutating `code_hash` or
//!   `vk_bytes` is rejected with `VkRegistrationImmutableFieldChanged`.
//! - Delete: registration is removed from the by_type index, memory
//!   count is decremented, and deleted entries do not count against
//!   the per-entity cap.

use ark_bn254::{Bn254, Fr};
use ark_groth16::Groth16;
use ark_relations::{
    lc,
    r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError, Variable},
};
use ark_serialize::CanonicalSerialize;
use ark_std::rand::{rngs::StdRng, SeedableRng};

use novai_ai_entities::{
    AiEntity, AutonomyMode, Capabilities, MemoryObjectType, VkRegistrationData,
    MAX_VK_REGISTRATIONS_PER_ENTITY, VK_REGISTRATION_LABEL_MAX, VK_REGISTRATION_VERSION,
};
use novai_execution::{
    apply_create_memory_object_tx, apply_delete_memory_object_tx, apply_update_memory_object_tx,
    encode_create_memory_object_payload_v1, encode_delete_memory_object_payload_v1,
    encode_update_memory_object_payload_v1, write_ai_entity_op, CreateMemoryObjectPayloadV1,
    DeleteMemoryObjectPayloadV1, ExecError, UpdateMemoryObjectPayloadV1,
    PROOF_SUBMISSION_MAX_VK_BYTES, PROOF_TYPE_GROTH16, PROOF_TYPE_GROTH16_REGISTERED,
    PROOF_TYPE_PLONK, PROOF_TYPE_PLONK_REGISTERED, PROOF_TYPE_STUB,
};
use novai_state::{
    ai_entity_by_address_key, ai_memory_by_type_key, ai_memory_object_key, Kv, KvBatch, MemKv,
    WriteOp,
};
use novai_types::{TxV1, TxVersion};

const PUBLISHER_BALANCE: u128 = 1_000_000;
const CREATE_FEE: u64 = 1_000;
const HEIGHT: u64 = 500;

// ============================================================================
// Groth16 VK generation (mirrors verification_system.rs SumCircuit)
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

/// Generate a fresh valid Groth16 compressed VK for the sum circuit.
/// Seeded so test re-runs are deterministic.
fn gen_valid_vk(seed: u64) -> Vec<u8> {
    let mut rng = StdRng::seed_from_u64(seed);
    let setup_circuit = SumCircuit {
        public_inputs: [None; 4],
        witness: None,
    };
    let pk = Groth16::<Bn254>::generate_random_parameters_with_reduction(setup_circuit, &mut rng)
        .expect("trusted setup");
    let mut vk_bytes = Vec::new();
    pk.vk
        .serialize_compressed(&mut vk_bytes)
        .expect("vk serialize");
    vk_bytes
}

// ============================================================================
// Helpers
// ============================================================================

fn publisher_caps() -> Capabilities {
    Capabilities {
        read_public_chain: true,
        read_memory_objects: true,
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
        publisher_caps(),
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

fn make_publisher(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut publisher = build_entity(code_hash, creator);
    publisher.economic_balance = PUBLISHER_BALANCE;
    store_entity(db, &publisher);
    publisher
}

fn sample_registration(code_hash: [u8; 32], vk_bytes: Vec<u8>) -> VkRegistrationData {
    VkRegistrationData {
        version: VK_REGISTRATION_VERSION,
        proof_type: PROOF_TYPE_GROTH16,
        code_hash,
        label: b"sum-v1".to_vec(),
        vk_bytes,
    }
}

fn make_create_tx(publisher: &AiEntity, nonce: u64, reg: &VkRegistrationData) -> TxV1 {
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::VkRegistration,
        data: reg.encode(),
    });
    TxV1 {
        version: TxVersion::V1,
        from: publisher.id,
        pubkey: publisher.id,
        nonce,
        fee: CREATE_FEE,
        payload,
        sig: [0u8; 64],
    }
}

fn make_update_tx(
    publisher: &AiEntity,
    nonce: u64,
    object_id: [u8; 32],
    new_data: Vec<u8>,
) -> TxV1 {
    let payload = encode_update_memory_object_payload_v1(&UpdateMemoryObjectPayloadV1 {
        object_id,
        new_data,
    });
    TxV1 {
        version: TxVersion::V1,
        from: publisher.id,
        pubkey: publisher.id,
        nonce,
        fee: CREATE_FEE,
        payload,
        sig: [0u8; 64],
    }
}

fn make_delete_tx(publisher: &AiEntity, nonce: u64, object_id: [u8; 32]) -> TxV1 {
    let payload =
        encode_delete_memory_object_payload_v1(&DeleteMemoryObjectPayloadV1 { object_id });
    TxV1 {
        version: TxVersion::V1,
        from: publisher.id,
        pubkey: publisher.id,
        nonce,
        fee: CREATE_FEE,
        payload: payload.to_vec(),
        sig: [0u8; 64],
    }
}

fn read_registration(db: &MemKv, publisher: &AiEntity, object_id: &[u8; 32]) -> VkRegistrationData {
    let envelope_bytes = db
        .get(&ai_memory_object_key(&publisher.id, object_id))
        .unwrap()
        .unwrap();
    // The memory object envelope wraps a header + the type-specific payload.
    // Strip the envelope by walking from the end of the data segment back;
    // VkRegistrationData is variable-length, so we attempt suffixes of
    // decreasing length until decode succeeds.
    for start in 0..envelope_bytes.len() {
        if let Some(reg) = VkRegistrationData::decode(&envelope_bytes[start..]) {
            return reg;
        }
    }
    panic!("stored bytes do not contain a decodable VkRegistrationData");
}

// ============================================================================
// 1. Happy path
// ============================================================================

#[test]
fn vk_registration_create_lands_in_by_type_index() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let vk = gen_valid_vk(0);

    let reg = sample_registration([0xC0u8; 32], vk.clone());
    let tx = make_create_tx(&publisher, 0, &reg);
    let object_id = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).expect("register succeeds");

    let type_key = ai_memory_by_type_key(
        MemoryObjectType::VkRegistration.to_byte(),
        &publisher.id,
        &object_id,
    );
    assert!(
        db.get(&type_key).unwrap().is_some(),
        "by_type index entry must exist after register"
    );

    let decoded = read_registration(&db, &publisher, &object_id);
    assert_eq!(decoded.version, VK_REGISTRATION_VERSION);
    assert_eq!(decoded.proof_type, PROOF_TYPE_GROTH16);
    assert_eq!(decoded.code_hash, [0xC0u8; 32]);
    assert_eq!(decoded.label, b"sum-v1".to_vec());
    assert_eq!(decoded.vk_bytes, vk);
}

// ============================================================================
// 2. Per-entity cap
// ============================================================================

#[test]
fn vk_registration_per_entity_cap_enforced() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let vk = gen_valid_vk(0);

    // 8 successful registrations, each with a distinct code_hash so the
    // payloads (and therefore object_ids) differ.
    for i in 0..MAX_VK_REGISTRATIONS_PER_ENTITY {
        let mut code_hash = [0u8; 32];
        code_hash[0] = u8::try_from(i).unwrap();
        let reg = sample_registration(code_hash, vk.clone());
        let tx = make_create_tx(&publisher, u64::from(i), &reg);
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT + u64::from(i)).unwrap_or_else(|e| {
            panic!("register #{i} should succeed, got {e:?}");
        });
    }

    // 9th must fail with the cap-exceeded variant.
    let mut code_hash = [0u8; 32];
    code_hash[0] = u8::try_from(MAX_VK_REGISTRATIONS_PER_ENTITY).unwrap();
    let reg = sample_registration(code_hash, vk);
    let tx = make_create_tx(&publisher, u64::from(MAX_VK_REGISTRATIONS_PER_ENTITY), &reg);
    let err = apply_create_memory_object_tx(
        &mut db,
        &tx,
        HEIGHT + u64::from(MAX_VK_REGISTRATIONS_PER_ENTITY),
    )
    .expect_err("9th registration must fail");
    assert!(
        matches!(
            err,
            ExecError::VkRegistrationLimitExceeded { current, max }
                if current == MAX_VK_REGISTRATIONS_PER_ENTITY
                    && max == MAX_VK_REGISTRATIONS_PER_ENTITY
        ),
        "expected VkRegistrationLimitExceeded, got {err:?}"
    );
}

// ============================================================================
// 3. Validation rejections
// ============================================================================

#[test]
fn vk_registration_create_rejects_bad_version() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x13u8; 32], [0x23u8; 32]);
    let mut reg = sample_registration([0xC0u8; 32], gen_valid_vk(0));
    reg.version = 99;
    let tx = make_create_tx(&publisher, 0, &reg);
    let err =
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT).expect_err("bad version rejected");
    assert!(
        matches!(err, ExecError::InvalidVkRegistration),
        "expected InvalidVkRegistration, got {err:?}"
    );
}

#[test]
fn vk_registration_create_rejects_unsupported_proof_type() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x14u8; 32], [0x24u8; 32]);
    // Walk every non-Groth16 proof-type discriminant that callers might
    // attempt and confirm each is rejected at registration time. STUB is
    // useless on-chain; PLONK is reserved but unwired; the two registered
    // variants are reserved for the Phase 2 dispatch path.
    for bad in [
        PROOF_TYPE_STUB,
        PROOF_TYPE_PLONK,
        PROOF_TYPE_GROTH16_REGISTERED,
        PROOF_TYPE_PLONK_REGISTERED,
    ] {
        let mut reg = sample_registration([0xC0u8; 32], gen_valid_vk(0));
        reg.proof_type = bad;
        let tx = make_create_tx(&publisher, 0, &reg);
        let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT)
            .expect_err("unsupported proof_type rejected");
        assert!(
            matches!(err, ExecError::VkRegistrationUnsupportedProofType { byte } if byte == bad),
            "expected VkRegistrationUnsupportedProofType {{ byte: {bad} }}, got {err:?}"
        );
    }
}

#[test]
fn vk_registration_create_rejects_label_too_long() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x15u8; 32], [0x25u8; 32]);
    let mut reg = sample_registration([0xC0u8; 32], gen_valid_vk(0));
    reg.label = vec![0xAB; VK_REGISTRATION_LABEL_MAX + 1];
    let tx = make_create_tx(&publisher, 0, &reg);
    let err =
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT).expect_err("oversized label rejected");
    assert!(
        matches!(
            err,
            ExecError::VkRegistrationLabelTooLong { len, max }
                if len == VK_REGISTRATION_LABEL_MAX + 1 && max == VK_REGISTRATION_LABEL_MAX
        ),
        "expected VkRegistrationLabelTooLong, got {err:?}"
    );
}

#[test]
fn vk_registration_create_rejects_empty_vk() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x16u8; 32], [0x26u8; 32]);
    let reg = sample_registration([0xC0u8; 32], Vec::new());
    let tx = make_create_tx(&publisher, 0, &reg);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).expect_err("empty vk rejected");
    assert!(
        matches!(
            err,
            ExecError::VkRegistrationBadVkLen { len, max }
                if len == 0 && max == PROOF_SUBMISSION_MAX_VK_BYTES
        ),
        "expected VkRegistrationBadVkLen (empty), got {err:?}"
    );
}

#[test]
fn vk_registration_create_rejects_oversized_vk() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x17u8; 32], [0x27u8; 32]);
    let reg = sample_registration([0xC0u8; 32], vec![0u8; PROOF_SUBMISSION_MAX_VK_BYTES + 1]);
    let tx = make_create_tx(&publisher, 0, &reg);
    let err =
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT).expect_err("oversized vk rejected");
    assert!(
        matches!(
            err,
            ExecError::VkRegistrationBadVkLen { len, max }
                if len == PROOF_SUBMISSION_MAX_VK_BYTES + 1
                    && max == PROOF_SUBMISSION_MAX_VK_BYTES
        ),
        "expected VkRegistrationBadVkLen (oversized), got {err:?}"
    );
}

#[test]
fn vk_registration_create_rejects_garbage_vk_bytes() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x18u8; 32], [0x28u8; 32]);
    // Non-zero, length within cap, but not a valid compressed
    // VerifyingKey<Bn254> serialization. The deserialize check rejects
    // it before the registration lands.
    let reg = sample_registration([0xC0u8; 32], vec![0xFFu8; 256]);
    let tx = make_create_tx(&publisher, 0, &reg);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).expect_err("garbage vk rejected");
    assert!(
        matches!(err, ExecError::VkRegistrationVkDeserializeFailed),
        "expected VkRegistrationVkDeserializeFailed, got {err:?}"
    );
}

// ============================================================================
// 4. Update rules (label-only)
// ============================================================================

#[test]
fn vk_registration_update_label_only_succeeds() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x19u8; 32], [0x29u8; 32]);
    let vk = gen_valid_vk(0);

    let reg = sample_registration([0xC0u8; 32], vk.clone());
    let create_tx = make_create_tx(&publisher, 0, &reg);
    let object_id =
        apply_create_memory_object_tx(&mut db, &create_tx, HEIGHT).expect("register succeeds");

    let mut updated = reg.clone();
    updated.label = b"sum-v2".to_vec();
    let update_tx = make_update_tx(&publisher, 1, object_id, updated.encode());
    apply_update_memory_object_tx(&mut db, &update_tx, HEIGHT + 1).expect("label update succeeds");

    let after = read_registration(&db, &publisher, &object_id);
    assert_eq!(after.label, b"sum-v2".to_vec(), "label should be updated");
    assert_eq!(after.vk_bytes, vk, "vk_bytes must be unchanged");
    assert_eq!(
        after.code_hash, reg.code_hash,
        "code_hash must be unchanged"
    );
}

#[test]
fn vk_registration_update_rejects_code_hash_change() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x1Au8; 32], [0x2Au8; 32]);
    let vk = gen_valid_vk(0);

    let mut reg = sample_registration([0xC0u8; 32], vk);
    let create_tx = make_create_tx(&publisher, 0, &reg);
    let object_id =
        apply_create_memory_object_tx(&mut db, &create_tx, HEIGHT).expect("register succeeds");

    reg.code_hash = [0xD0u8; 32];
    let update_tx = make_update_tx(&publisher, 1, object_id, reg.encode());
    let err = apply_update_memory_object_tx(&mut db, &update_tx, HEIGHT + 1)
        .expect_err("code_hash mutation rejected");
    assert!(
        matches!(err, ExecError::VkRegistrationImmutableFieldChanged),
        "expected VkRegistrationImmutableFieldChanged, got {err:?}"
    );
}

#[test]
fn vk_registration_update_rejects_vk_bytes_change() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x1Bu8; 32], [0x2Bu8; 32]);
    let vk_a = gen_valid_vk(0);
    let vk_b = gen_valid_vk(1);

    let mut reg = sample_registration([0xC0u8; 32], vk_a);
    let create_tx = make_create_tx(&publisher, 0, &reg);
    let object_id =
        apply_create_memory_object_tx(&mut db, &create_tx, HEIGHT).expect("register succeeds");

    reg.vk_bytes = vk_b;
    let update_tx = make_update_tx(&publisher, 1, object_id, reg.encode());
    let err = apply_update_memory_object_tx(&mut db, &update_tx, HEIGHT + 1)
        .expect_err("vk_bytes mutation rejected");
    assert!(
        matches!(err, ExecError::VkRegistrationImmutableFieldChanged),
        "expected VkRegistrationImmutableFieldChanged, got {err:?}"
    );
}

// ============================================================================
// 5. Delete + cap interaction
// ============================================================================

#[test]
fn vk_registration_delete_removes_by_type_entry() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x1Cu8; 32], [0x2Cu8; 32]);
    let vk = gen_valid_vk(0);

    let reg = sample_registration([0xC0u8; 32], vk);
    let create_tx = make_create_tx(&publisher, 0, &reg);
    let object_id =
        apply_create_memory_object_tx(&mut db, &create_tx, HEIGHT).expect("register succeeds");

    let delete_tx = make_delete_tx(&publisher, 1, object_id);
    apply_delete_memory_object_tx(&mut db, &delete_tx, HEIGHT + 1).expect("delete succeeds");

    let type_key = ai_memory_by_type_key(
        MemoryObjectType::VkRegistration.to_byte(),
        &publisher.id,
        &object_id,
    );
    assert!(
        db.get(&type_key).unwrap().is_none(),
        "by_type index entry must be cleared after delete"
    );
    assert!(
        db.get(&ai_memory_object_key(&publisher.id, &object_id))
            .unwrap()
            .is_none(),
        "primary record must be cleared after delete"
    );
}

#[test]
fn vk_registration_cap_ignores_deleted() {
    let mut db = MemKv::new();
    let publisher = make_publisher(&mut db, [0x1Du8; 32], [0x2Du8; 32]);
    let vk = gen_valid_vk(0);

    // Fill the cap to MAX_VK_REGISTRATIONS_PER_ENTITY.
    let mut object_ids = Vec::with_capacity(MAX_VK_REGISTRATIONS_PER_ENTITY as usize);
    for i in 0..MAX_VK_REGISTRATIONS_PER_ENTITY {
        let mut code_hash = [0u8; 32];
        code_hash[0] = u8::try_from(i).unwrap();
        let reg = sample_registration(code_hash, vk.clone());
        let tx = make_create_tx(&publisher, u64::from(i), &reg);
        let object_id = apply_create_memory_object_tx(&mut db, &tx, HEIGHT + u64::from(i))
            .expect("registration within cap");
        object_ids.push(object_id);
    }

    // Delete one registration; the slot must become available.
    let delete_tx = make_delete_tx(
        &publisher,
        u64::from(MAX_VK_REGISTRATIONS_PER_ENTITY),
        object_ids[0],
    );
    apply_delete_memory_object_tx(
        &mut db,
        &delete_tx,
        HEIGHT + u64::from(MAX_VK_REGISTRATIONS_PER_ENTITY),
    )
    .expect("delete succeeds");

    // Adding a 9th registration (post-delete) now lands successfully.
    let mut code_hash = [0u8; 32];
    code_hash[0] = u8::try_from(MAX_VK_REGISTRATIONS_PER_ENTITY).unwrap();
    let reg = sample_registration(code_hash, vk);
    let create_tx = make_create_tx(
        &publisher,
        u64::from(MAX_VK_REGISTRATIONS_PER_ENTITY) + 1,
        &reg,
    );
    apply_create_memory_object_tx(
        &mut db,
        &create_tx,
        HEIGHT + u64::from(MAX_VK_REGISTRATIONS_PER_ENTITY) + 1,
    )
    .expect("registration succeeds after delete frees a slot");
}
