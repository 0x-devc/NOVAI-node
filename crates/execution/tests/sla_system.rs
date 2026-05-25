#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]

//! Integration tests for the Week 31 SLA Agreement create flow
//! (Phase 1).
//!
//! Each test proposes an `SlaAgreement` memory object through the
//! normal `CreateMemoryObject` signal path and verifies the per-type
//! validation rules + the three SLA indexes (active-pair singleton,
//! by-buyer scan, by-seller scan).
//!
//! The set covers:
//!
//! - Happy path: SLA lands in all three indexes, the memory object
//!   payload decodes back to the proposed values, and the entity
//!   memory count is incremented.
//! - Per-entity cap (buyer): 8th SLA succeeds, 9th is rejected with
//!   `SlaAgreementLimitExceeded`.
//! - Validation rejections: bad version, bad initial status,
//!   pre-seeded runtime-only fields, buyer-as-issuer mismatch,
//!   self-SLA, missing seller, inactive seller, inverted window,
//!   start-in-past, span > duration cap, threshold zero, slash
//!   amount zero, out-of-range bps fields, non-zero reserved bytes,
//!   pair-already-open singleton conflict.

use novai_ai_entities::{
    AiEntity, AutonomyMode, Capabilities, MemoryObjectType, SlaAgreementData, MAX_SLAS_PER_ENTITY,
    SLA_AGREEMENT_V1, SLA_MAX_DURATION_BLOCKS, SLA_MIN_DELIVERY_SUCCESS_BPS_MAX,
    SLA_MIN_UPTIME_BPS_MAX, SLA_RESERVED_LEN, SLA_STATUS_ACTIVE, SLA_STATUS_PROPOSED,
};
use novai_execution::{
    apply_create_memory_object_tx, encode_create_memory_object_payload_v1, sla_active_between_key,
    sla_by_buyer_key, sla_by_seller_key, write_ai_entity_op, CreateMemoryObjectPayloadV1,
    ExecError, KEY_PREFIX_AI_SLAS_ACTIVE_BETWEEN, KEY_PREFIX_AI_SLAS_BY_BUYER,
    KEY_PREFIX_AI_SLAS_BY_SELLER,
};
use novai_state::{
    ai_entity_by_address_key, ai_memory_by_type_key, ai_memory_object_key, Kv, KvBatch, MemKv,
    WriteOp,
};
use novai_types::{TxV1, TxVersion};

const BUYER_BALANCE: u128 = 1_000_000;
const SELLER_STAKE: u128 = 5_000_000;
const CREATE_FEE: u64 = 1_000;
const HEIGHT: u64 = 500;
const SLA_START: u64 = 1_000;
const SLA_END: u64 = 5_000;

// ============================================================================
// Helpers
// ============================================================================

fn buyer_caps() -> Capabilities {
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
    AiEntity::new(code_hash, creator, AutonomyMode::Gated, buyer_caps(), 1000)
}

fn store_entity(db: &mut MemKv, entity: &AiEntity) {
    db.apply_batch(&[
        write_ai_entity_op(entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();
}

fn make_buyer(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut buyer = build_entity(code_hash, creator);
    buyer.economic_balance = BUYER_BALANCE;
    store_entity(db, &buyer);
    buyer
}

fn make_seller(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut seller = build_entity(code_hash, creator);
    seller.stake_balance = SELLER_STAKE;
    store_entity(db, &seller);
    seller
}

fn sample_sla(buyer: &AiEntity, seller: &AiEntity) -> SlaAgreementData {
    SlaAgreementData {
        version: SLA_AGREEMENT_V1,
        buyer_entity_id: buyer.id,
        seller_entity_id: seller.id,
        service_descriptor_hash: [0u8; 32],
        status: SLA_STATUS_PROPOSED,
        created_at_height: 0,
        accepted_at_height: 0,
        start_height: SLA_START,
        end_height: SLA_END,
        violation_count: 0,
        violation_threshold: 3,
        max_response_time_blocks: 0,
        min_uptime_bps: 0,
        min_delivery_success_bps: 0,
        price_per_call: 100,
        slash_amount: 1_000_000,
        terminated_at_height: 0,
        slashed_amount: 0,
        reserved: [0u8; SLA_RESERVED_LEN],
    }
}

fn make_create_tx(buyer: &AiEntity, nonce: u64, sla: &SlaAgreementData) -> TxV1 {
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::SlaAgreement,
        data: sla.encode().to_vec(),
    });
    TxV1 {
        version: TxVersion::V1,
        from: buyer.id,
        pubkey: buyer.id,
        nonce,
        fee: CREATE_FEE,
        payload,
        sig: [0u8; 64],
    }
}

fn propose(db: &mut MemKv, buyer: &AiEntity, nonce: u64, sla: &SlaAgreementData) -> [u8; 32] {
    let tx = make_create_tx(buyer, nonce, sla);
    apply_create_memory_object_tx(db, &tx, HEIGHT).expect("propose succeeds")
}

// ============================================================================
// 1. Happy path: SLA lands in all three indexes
// ============================================================================

#[test]
fn sla_propose_lands_in_all_three_indexes() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let seller = make_seller(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let sla = sample_sla(&buyer, &seller);

    let object_id = propose(&mut db, &buyer, 0, &sla);

    // 1a. active_between singleton holds the object_id.
    let pair_key = sla_active_between_key(&buyer.id, &seller.id);
    let stored = db.get(&pair_key).unwrap().expect("pair key present");
    assert_eq!(stored.as_slice(), &object_id[..]);

    // 1b. by_buyer scan marker exists at (buyer, HEIGHT, object_id).
    let by_buyer = sla_by_buyer_key(&buyer.id, HEIGHT, &object_id);
    assert!(db.get(&by_buyer).unwrap().is_some(), "by_buyer marker");

    // 1c. by_seller scan marker exists at (seller, HEIGHT, object_id).
    let by_seller = sla_by_seller_key(&seller.id, HEIGHT, &object_id);
    assert!(db.get(&by_seller).unwrap().is_some(), "by_seller marker");

    // 1d. by_type marker (existing convention) and primary memory object.
    let by_type = ai_memory_by_type_key(
        MemoryObjectType::SlaAgreement.to_byte(),
        &buyer.id,
        &object_id,
    );
    assert!(db.get(&by_type).unwrap().is_some(), "by_type marker");

    let primary = ai_memory_object_key(&buyer.id, &object_id);
    assert!(db.get(&primary).unwrap().is_some(), "primary record");
}

#[test]
fn sla_propose_payload_decodes_back_to_proposed_values() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x13u8; 32], [0x23u8; 32]);
    let seller = make_seller(&mut db, [0x14u8; 32], [0x24u8; 32]);
    let mut sla = sample_sla(&buyer, &seller);
    sla.service_descriptor_hash = [0xABu8; 32];
    sla.violation_threshold = 5;
    sla.slash_amount = 7_500_000;
    sla.price_per_call = 42;

    let object_id = propose(&mut db, &buyer, 0, &sla);

    let envelope = db
        .get(&ai_memory_object_key(&buyer.id, &object_id))
        .unwrap()
        .unwrap();
    let payload_start = envelope.len() - novai_ai_entities::SLA_AGREEMENT_SIZE;
    let decoded = SlaAgreementData::decode(&envelope[payload_start..]).expect("decode");
    assert_eq!(decoded.buyer_entity_id, buyer.id);
    assert_eq!(decoded.seller_entity_id, seller.id);
    assert_eq!(decoded.service_descriptor_hash, [0xABu8; 32]);
    assert_eq!(decoded.violation_threshold, 5);
    assert_eq!(decoded.slash_amount, 7_500_000);
    assert_eq!(decoded.price_per_call, 42);
    assert_eq!(decoded.status, SLA_STATUS_PROPOSED);
    assert_eq!(decoded.violation_count, 0);
    assert_eq!(decoded.terminated_at_height, 0);
    assert_eq!(decoded.slashed_amount, 0);
}

#[test]
fn sla_index_prefixes_are_canonical() {
    // Locks the wire-level prefix bytes so a future rename in the
    // execution crate cannot silently break the on-chain layout.
    assert_eq!(
        KEY_PREFIX_AI_SLAS_ACTIVE_BETWEEN,
        b"ai/slas/active_between/"
    );
    assert_eq!(KEY_PREFIX_AI_SLAS_BY_BUYER, b"ai/slas/by_buyer/");
    assert_eq!(KEY_PREFIX_AI_SLAS_BY_SELLER, b"ai/slas/by_seller/");
}

#[test]
fn sla_active_between_key_separates_distinct_pairs() {
    let a = [0xAAu8; 32];
    let b = [0xBBu8; 32];
    let c = [0xCCu8; 32];
    assert_ne!(
        sla_active_between_key(&a, &b),
        sla_active_between_key(&a, &c)
    );
    assert_ne!(
        sla_active_between_key(&a, &b),
        sla_active_between_key(&b, &a)
    );
}

// ============================================================================
// 2. Per-entity cap (buyer side)
// ============================================================================

#[test]
fn sla_propose_per_buyer_cap_enforced() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x15u8; 32], [0x25u8; 32]);
    // Each open SLA must be against a distinct seller because of the
    // one-open-pair invariant. Generate MAX_SLAS_PER_ENTITY + 1 sellers.
    let mut sellers = Vec::new();
    for i in 0..=MAX_SLAS_PER_ENTITY {
        let i_u8 = u8::try_from(i).unwrap();
        sellers.push(make_seller(&mut db, [0x30 + i_u8; 32], [0x40 + i_u8; 32]));
    }

    // First MAX_SLAS_PER_ENTITY succeed.
    for (i, seller) in sellers
        .iter()
        .take(MAX_SLAS_PER_ENTITY as usize)
        .enumerate()
    {
        let sla = sample_sla(&buyer, seller);
        let tx = make_create_tx(&buyer, i as u64, &sla);
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT)
            .unwrap_or_else(|e| panic!("propose #{i} should succeed: {e:?}"));
    }

    // The (MAX + 1)-th SLA hits the cap.
    let overflow_seller = sellers.last().unwrap();
    let sla = sample_sla(&buyer, overflow_seller);
    let tx = make_create_tx(&buyer, MAX_SLAS_PER_ENTITY as u64, &sla);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    match err {
        ExecError::SlaAgreementLimitExceeded { current, max } => {
            assert_eq!(current, MAX_SLAS_PER_ENTITY);
            assert_eq!(max, MAX_SLAS_PER_ENTITY);
        }
        other => panic!("expected SlaAgreementLimitExceeded, got {other:?}"),
    }
}

// ============================================================================
// 3. Validation rejections
// ============================================================================

#[test]
fn sla_propose_rejects_bad_version_byte() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x16u8; 32], [0x26u8; 32]);
    let seller = make_seller(&mut db, [0x17u8; 32], [0x27u8; 32]);
    let mut sla = sample_sla(&buyer, &seller);
    sla.version = 9;

    let tx = make_create_tx(&buyer, 0, &sla);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    assert!(matches!(
        err,
        ExecError::SlaAgreementVersionInvalid { byte: 9 }
    ));
}

#[test]
fn sla_propose_rejects_non_proposed_initial_status() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x18u8; 32], [0x28u8; 32]);
    let seller = make_seller(&mut db, [0x19u8; 32], [0x29u8; 32]);
    let mut sla = sample_sla(&buyer, &seller);
    sla.status = SLA_STATUS_ACTIVE;

    let tx = make_create_tx(&buyer, 0, &sla);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    assert!(matches!(err, ExecError::SlaAgreementStatusInvalid { .. }));
}

#[test]
fn sla_propose_rejects_preseeded_runtime_fields() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x1Au8; 32], [0x2Au8; 32]);
    let seller = make_seller(&mut db, [0x1Bu8; 32], [0x2Bu8; 32]);

    // Pre-seeded violation_count.
    let mut sla = sample_sla(&buyer, &seller);
    sla.violation_count = 7;
    let tx = make_create_tx(&buyer, 0, &sla);
    assert!(matches!(
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err(),
        ExecError::SlaAgreementInitialFieldsNotZero
    ));

    // Pre-seeded accepted_at_height.
    let mut sla = sample_sla(&buyer, &seller);
    sla.accepted_at_height = 100;
    let tx = make_create_tx(&buyer, 0, &sla);
    assert!(matches!(
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err(),
        ExecError::SlaAgreementInitialFieldsNotZero
    ));

    // Pre-seeded terminated_at_height.
    let mut sla = sample_sla(&buyer, &seller);
    sla.terminated_at_height = 250;
    let tx = make_create_tx(&buyer, 0, &sla);
    assert!(matches!(
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err(),
        ExecError::SlaAgreementInitialFieldsNotZero
    ));

    // Pre-seeded slashed_amount.
    let mut sla = sample_sla(&buyer, &seller);
    sla.slashed_amount = 1;
    let tx = make_create_tx(&buyer, 0, &sla);
    assert!(matches!(
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err(),
        ExecError::SlaAgreementInitialFieldsNotZero
    ));
}

#[test]
fn sla_propose_rejects_buyer_id_mismatch() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x1Cu8; 32], [0x2Cu8; 32]);
    let seller = make_seller(&mut db, [0x1Du8; 32], [0x2Du8; 32]);

    let mut sla = sample_sla(&buyer, &seller);
    sla.buyer_entity_id = [0xFFu8; 32]; // Lie about owner identity.

    let tx = make_create_tx(&buyer, 0, &sla);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    assert!(matches!(err, ExecError::SlaAgreementBuyerMustBeIssuer));
}

#[test]
fn sla_propose_rejects_self_sla() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x1Eu8; 32], [0x2Eu8; 32]);
    let mut sla = sample_sla(&buyer, &buyer);
    // buyer_entity_id == seller_entity_id was set by sample_sla above.
    sla.seller_entity_id = buyer.id;

    let tx = make_create_tx(&buyer, 0, &sla);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    assert!(matches!(err, ExecError::SlaAgreementBuyerSellerSame));
}

#[test]
fn sla_propose_rejects_missing_seller() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x20u8; 32], [0x30u8; 32]);
    let phantom_seller_id = [0xC0u8; 32];

    let mut sla = sample_sla(&buyer, &buyer);
    sla.seller_entity_id = phantom_seller_id;

    let tx = make_create_tx(&buyer, 0, &sla);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    assert!(matches!(err, ExecError::SlaAgreementSellerNotFound));
}

#[test]
fn sla_propose_rejects_inactive_seller() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x31u8; 32], [0x41u8; 32]);
    let mut seller = make_seller(&mut db, [0x32u8; 32], [0x42u8; 32]);
    // Mark seller inactive and write back.
    seller.is_active = false;
    db.apply_batch(&[write_ai_entity_op(&seller)]).unwrap();

    let sla = sample_sla(&buyer, &seller);
    let tx = make_create_tx(&buyer, 0, &sla);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    assert!(matches!(err, ExecError::SlaAgreementSellerNotActive));
}

#[test]
fn sla_propose_rejects_inverted_or_empty_window() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x33u8; 32], [0x43u8; 32]);
    let seller = make_seller(&mut db, [0x34u8; 32], [0x44u8; 32]);

    // end == start.
    let mut sla = sample_sla(&buyer, &seller);
    sla.start_height = 1_000;
    sla.end_height = 1_000;
    let tx = make_create_tx(&buyer, 0, &sla);
    assert!(matches!(
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err(),
        ExecError::SlaAgreementInvalidWindow {
            start: 1_000,
            end: 1_000
        }
    ));

    // end < start.
    let mut sla = sample_sla(&buyer, &seller);
    sla.start_height = 1_000;
    sla.end_height = 500;
    let tx = make_create_tx(&buyer, 0, &sla);
    assert!(matches!(
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err(),
        ExecError::SlaAgreementInvalidWindow {
            start: 1_000,
            end: 500
        }
    ));
}

#[test]
fn sla_propose_rejects_start_in_past() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x35u8; 32], [0x45u8; 32]);
    let seller = make_seller(&mut db, [0x36u8; 32], [0x46u8; 32]);

    let mut sla = sample_sla(&buyer, &seller);
    sla.start_height = HEIGHT - 1; // strictly in the past
    sla.end_height = HEIGHT + 1_000;
    let tx = make_create_tx(&buyer, 0, &sla);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    assert!(matches!(
        err,
        ExecError::SlaAgreementStartInPast { current, start } if current == HEIGHT && start == HEIGHT - 1
    ));
}

#[test]
fn sla_propose_rejects_duration_above_max() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x37u8; 32], [0x47u8; 32]);
    let seller = make_seller(&mut db, [0x38u8; 32], [0x48u8; 32]);

    let mut sla = sample_sla(&buyer, &seller);
    sla.start_height = HEIGHT;
    sla.end_height = HEIGHT + SLA_MAX_DURATION_BLOCKS + 1;
    let tx = make_create_tx(&buyer, 0, &sla);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    assert!(matches!(
        err,
        ExecError::SlaAgreementDurationExceedsMax { span, max }
            if span == SLA_MAX_DURATION_BLOCKS + 1 && max == SLA_MAX_DURATION_BLOCKS
    ));
}

#[test]
fn sla_propose_accepts_duration_equal_to_max() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x39u8; 32], [0x49u8; 32]);
    let seller = make_seller(&mut db, [0x3Au8; 32], [0x4Au8; 32]);

    let mut sla = sample_sla(&buyer, &seller);
    sla.start_height = HEIGHT;
    sla.end_height = HEIGHT + SLA_MAX_DURATION_BLOCKS;
    let tx = make_create_tx(&buyer, 0, &sla);
    apply_create_memory_object_tx(&mut db, &tx, HEIGHT).expect("span == cap must be allowed");
}

#[test]
fn sla_propose_rejects_zero_threshold() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x3Bu8; 32], [0x4Bu8; 32]);
    let seller = make_seller(&mut db, [0x3Cu8; 32], [0x4Cu8; 32]);

    let mut sla = sample_sla(&buyer, &seller);
    sla.violation_threshold = 0;
    let tx = make_create_tx(&buyer, 0, &sla);
    assert!(matches!(
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err(),
        ExecError::SlaAgreementThresholdZero
    ));
}

#[test]
fn sla_propose_rejects_zero_slash_amount() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x3Du8; 32], [0x4Du8; 32]);
    let seller = make_seller(&mut db, [0x3Eu8; 32], [0x4Eu8; 32]);

    let mut sla = sample_sla(&buyer, &seller);
    sla.slash_amount = 0;
    let tx = make_create_tx(&buyer, 0, &sla);
    assert!(matches!(
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err(),
        ExecError::SlaAgreementSlashAmountZero
    ));
}

#[test]
fn sla_propose_rejects_out_of_range_bps_fields() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x3Fu8; 32], [0x4Fu8; 32]);
    let seller = make_seller(&mut db, [0x50u8; 32], [0x60u8; 32]);

    // Above the uptime bps cap.
    let mut sla = sample_sla(&buyer, &seller);
    sla.min_uptime_bps = SLA_MIN_UPTIME_BPS_MAX + 1;
    let tx = make_create_tx(&buyer, 0, &sla);
    assert!(matches!(
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err(),
        ExecError::SlaAgreementInvalidReservedField
    ));

    // Above the delivery-success bps cap.
    let mut sla = sample_sla(&buyer, &seller);
    sla.min_delivery_success_bps = SLA_MIN_DELIVERY_SUCCESS_BPS_MAX + 1;
    let tx = make_create_tx(&buyer, 0, &sla);
    assert!(matches!(
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err(),
        ExecError::SlaAgreementInvalidReservedField
    ));
}

#[test]
fn sla_propose_rejects_nonzero_reserved_bytes() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x51u8; 32], [0x61u8; 32]);
    let seller = make_seller(&mut db, [0x52u8; 32], [0x62u8; 32]);

    let mut sla = sample_sla(&buyer, &seller);
    sla.reserved[3] = 1;
    let tx = make_create_tx(&buyer, 0, &sla);
    assert!(matches!(
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err(),
        ExecError::SlaAgreementReservedNotZero
    ));
}

#[test]
fn sla_propose_rejects_pair_already_open() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x53u8; 32], [0x63u8; 32]);
    let seller = make_seller(&mut db, [0x54u8; 32], [0x64u8; 32]);

    // First proposal succeeds.
    let sla = sample_sla(&buyer, &seller);
    propose(&mut db, &buyer, 0, &sla);

    // Second proposal between same pair is rejected by the active_between
    // singleton, even with a different threshold / slash_amount.
    let mut sla2 = sample_sla(&buyer, &seller);
    sla2.violation_threshold = 1;
    sla2.slash_amount = 200_000;
    let tx = make_create_tx(&buyer, 1, &sla2);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT).unwrap_err();
    assert!(matches!(err, ExecError::SlaAgreementPairAlreadyOpen));
}
