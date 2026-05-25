#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]

//! End-to-end smoke tests for the Week 31 SLA pipeline (Phase 5).
//!
//! Each test drives the full lifecycle through the public handler /
//! query surface that the RPC and CLI sit on top of:
//!
//!   buyer -> CreateMemoryObject (propose)
//!   seller -> SlaAccept signal
//!   buyer -> PaymentRequest + ServiceAttestation Failed
//!   ... repeated until violation_threshold ...
//!   runtime -> auto-slash + Violated terminal state
//!   buyer -> DeleteMemoryObject (audit cleanup)
//!
//! These tests are intentionally heavier than the per-phase suites
//! and validate that the query helpers (`get_slas_by_buyer`,
//! `get_slas_by_seller`, `get_active_sla_between`) return the same
//! shape an off-chain client would see after each transition.

use novai_ai_entities::{
    AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObjectType, SlaAgreementData,
    SLA_AGREEMENT_V1, SLA_RESERVED_LEN, SLA_STATUS_ACTIVE, SLA_STATUS_PROPOSED,
    SLA_STATUS_VIOLATED,
};
use novai_execution::{
    apply_create_memory_object_tx, apply_delete_memory_object_tx, apply_signal_commitment_tx,
    encode_create_memory_object_payload_v1, encode_delete_memory_object_payload_v1,
    encode_payment_record_v1, encode_signal_commitment_payload_v1, get_active_sla_between,
    get_sla_agreement, get_slas_by_buyer, get_slas_by_seller, payment_by_hash_key,
    payment_by_payee_key, payment_by_payer_key, write_ai_entity_op, CreateMemoryObjectPayloadV1,
    DeleteMemoryObjectPayloadV1, PaymentRecord, ServiceAttestationExtraV1,
    SignalCommitmentPayloadV1, SlaAcceptExtraV1, PAYMENT_ATTESTATION_STATUS_FAILED,
    PAYMENT_ATTESTATION_STATUS_NONE,
};
use novai_state::{ai_entity_by_address_key, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const BUYER_BALANCE: u128 = 50_000_000;
const SELLER_STAKE: u128 = 5_000_000;
const SELLER_BALANCE: u128 = 50_000_000;
const FEE: u64 = 1_000;
const HEIGHT_PROPOSE: u64 = 100;
const HEIGHT_ACCEPT: u64 = 200;
const SLA_START: u64 = 500;
const SLA_END: u64 = 5_000;
const SLASH_AMOUNT: u128 = 1_000_000;

fn caps() -> Capabilities {
    Capabilities {
        read_public_chain: true,
        read_memory_objects: true,
        emit_proposals: true,
        request_execution: true,
        read_nnpx_derived: false,
        submit_reputation_updates: false,
        post_oracle_anchors: false,
        _reserved: [false; 1],
    }
}

fn build_entity(code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    AiEntity::new(code_hash, creator, AutonomyMode::Gated, caps(), 1000)
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
    seller.economic_balance = SELLER_BALANCE;
    store_entity(db, &seller);
    seller
}

fn sample_sla(buyer: &AiEntity, seller: &AiEntity, threshold: u32) -> SlaAgreementData {
    SlaAgreementData {
        version: SLA_AGREEMENT_V1,
        buyer_entity_id: buyer.id,
        seller_entity_id: seller.id,
        service_descriptor_hash: [0xCDu8; 32],
        status: SLA_STATUS_PROPOSED,
        created_at_height: 0,
        accepted_at_height: 0,
        start_height: SLA_START,
        end_height: SLA_END,
        violation_count: 0,
        violation_threshold: threshold,
        max_response_time_blocks: 0,
        min_uptime_bps: 0,
        min_delivery_success_bps: 0,
        price_per_call: 100,
        slash_amount: SLASH_AMOUNT,
        terminated_at_height: 0,
        slashed_amount: 0,
        reserved: [0u8; SLA_RESERVED_LEN],
    }
}

fn propose(db: &mut MemKv, buyer: &AiEntity, nonce: u64, sla: &SlaAgreementData) -> [u8; 32] {
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::SlaAgreement,
        data: sla.encode().to_vec(),
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: buyer.id,
        pubkey: buyer.id,
        nonce,
        fee: FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_create_memory_object_tx(db, &tx, HEIGHT_PROPOSE).expect("propose")
}

fn accept(db: &mut MemKv, seller: &AiEntity, nonce: u64, sla_id: [u8; 32], buyer_id: [u8; 32]) {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xE0u8 ^ nonce as u8; 32],
        signal_type: AiSignalType::SlaAccept,
        issuer_entity_id: seller.id,
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
        sla_accept: Some(SlaAcceptExtraV1 {
            sla_object_id: sla_id,
            buyer_entity_id: buyer_id,
        }),
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
        oracle_anchor: None,
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: seller.id,
        pubkey: seller.id,
        nonce,
        fee: FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_signal_commitment_tx(db, &tx, HEIGHT_ACCEPT).expect("accept");
}

fn seed_payment(
    db: &mut MemKv,
    payer: &AiEntity,
    payee: &AiEntity,
    signal_hash: [u8; 32],
    height: u64,
) {
    let record = PaymentRecord {
        payer: payer.id,
        payee: payee.id,
        amount: 1_000,
        service_descriptor_hash: [0u8; 32],
        request_hash: [0xFFu8; 32],
        payment_height: height,
        max_block_height: height + 100,
        attested_status: PAYMENT_ATTESTATION_STATUS_NONE,
        attested_height: 0,
    };
    let bytes = encode_payment_record_v1(&record);
    db.apply_batch(&[
        WriteOp::Put(payment_by_hash_key(&signal_hash), bytes.to_vec()),
        WriteOp::Put(
            payment_by_payer_key(&payer.id, height, &signal_hash),
            Vec::new(),
        ),
        WriteOp::Put(
            payment_by_payee_key(&payee.id, height, &signal_hash),
            Vec::new(),
        ),
    ])
    .unwrap();
}

fn attest_failed(
    db: &mut MemKv,
    payer: &AiEntity,
    payee: &AiEntity,
    payment_signal_hash: [u8; 32],
    nonce: u64,
    height: u64,
) {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xA0u8 ^ nonce as u8; 32],
        signal_type: AiSignalType::ServiceAttestation,
        issuer_entity_id: payer.id,
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
            payee_entity_id: payee.id,
            status: PAYMENT_ATTESTATION_STATUS_FAILED,
        }),
        sla_accept: None,
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
        oracle_anchor: None,
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: payer.id,
        pubkey: payer.id,
        nonce,
        fee: FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_signal_commitment_tx(db, &tx, height).expect("attest");
}

fn delete_sla(db: &mut MemKv, buyer: &AiEntity, nonce: u64, sla_id: [u8; 32], height: u64) {
    let payload =
        encode_delete_memory_object_payload_v1(&DeleteMemoryObjectPayloadV1 { object_id: sla_id });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: buyer.id,
        pubkey: buyer.id,
        nonce,
        fee: FEE,
        payload: payload.to_vec(),
        sig: [0u8; 64],
    };
    apply_delete_memory_object_tx(db, &tx, height).expect("delete");
}

// ============================================================================
// Full lifecycle: propose -> accept -> 3xFAILED -> auto-slash -> delete
// ============================================================================

#[test]
fn sla_full_lifecycle_propose_accept_breach_delete() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let seller = make_seller(&mut db, [0x12u8; 32], [0x22u8; 32]);

    // 1) PROPOSE
    let sla = sample_sla(&buyer, &seller, 3);
    let sla_id = propose(&mut db, &buyer, 0, &sla);

    // After propose: status=Proposed, no slash, singleton present.
    let (_obj, fetched) = get_sla_agreement(&db, &buyer.id, &sla_id).unwrap().unwrap();
    assert_eq!(fetched.status, SLA_STATUS_PROPOSED);
    assert_eq!(fetched.violation_count, 0);
    let (_obj_via_pair, fetched_via_pair) = get_active_sla_between(&db, &buyer.id, &seller.id)
        .unwrap()
        .unwrap();
    assert_eq!(fetched_via_pair.violation_threshold, 3);

    // 2) ACCEPT
    accept(&mut db, &seller, 0, sla_id, buyer.id);
    let (_, post_accept) = get_sla_agreement(&db, &buyer.id, &sla_id).unwrap().unwrap();
    assert_eq!(post_accept.status, SLA_STATUS_ACTIVE);
    assert_eq!(post_accept.accepted_at_height, HEIGHT_ACCEPT);

    // 3) Two failed attestations: below threshold.
    for i in 0..2u64 {
        let hash = [0x30u8 ^ i as u8; 32];
        seed_payment(&mut db, &buyer, &seller, hash, 600 + i);
        attest_failed(&mut db, &buyer, &seller, hash, i + 1, 700 + i);
    }
    let (_, after_two) = get_sla_agreement(&db, &buyer.id, &sla_id).unwrap().unwrap();
    assert_eq!(after_two.status, SLA_STATUS_ACTIVE);
    assert_eq!(after_two.violation_count, 2);
    assert_eq!(after_two.slashed_amount, 0);

    // 4) Third failed attestation: threshold breach. Auto-slash fires.
    let breach_hash = [0x40u8; 32];
    seed_payment(&mut db, &buyer, &seller, breach_hash, 800);
    attest_failed(&mut db, &buyer, &seller, breach_hash, 3, 900);

    let (_, breached) = get_sla_agreement(&db, &buyer.id, &sla_id).unwrap().unwrap();
    assert_eq!(breached.status, SLA_STATUS_VIOLATED);
    assert_eq!(breached.violation_count, 3);
    assert_eq!(breached.slashed_amount, SLASH_AMOUNT);
    assert_eq!(breached.terminated_at_height, 900);

    // active_between singleton was torn down on breach.
    assert!(get_active_sla_between(&db, &buyer.id, &seller.id)
        .unwrap()
        .is_none());

    // 5) Audit cleanup delete by the buyer. Buyer's nonce: 1 (after
    // propose) + 3 (one per attestation) = 4 going into the delete.
    delete_sla(&mut db, &buyer, 4, sla_id, 1_000);
    assert!(get_sla_agreement(&db, &buyer.id, &sla_id)
        .unwrap()
        .is_none());
}

// ============================================================================
// Query helpers across mixed states
// ============================================================================

#[test]
fn list_by_buyer_and_seller_see_the_same_proposed_sla() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x13u8; 32], [0x23u8; 32]);
    let seller = make_seller(&mut db, [0x14u8; 32], [0x24u8; 32]);
    let sla = sample_sla(&buyer, &seller, 5);
    let sla_id = propose(&mut db, &buyer, 0, &sla);

    let by_buyer = get_slas_by_buyer(&db, &buyer.id, 0, HEIGHT_PROPOSE * 2).unwrap();
    let by_seller = get_slas_by_seller(&db, &seller.id, 0, HEIGHT_PROPOSE * 2).unwrap();
    assert_eq!(by_buyer.len(), 1);
    assert_eq!(by_seller.len(), 1);
    assert_eq!(by_buyer[0].0.object_id, sla_id);
    assert_eq!(by_seller[0].0.object_id, sla_id);
    assert_eq!(by_buyer[0].1.violation_threshold, 5);
    assert_eq!(by_seller[0].1.violation_threshold, 5);
}

#[test]
fn list_by_buyer_height_window_filters_correctly() {
    // Two SLAs created in different blocks. Window queries return
    // only the matching one.
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x15u8; 32], [0x25u8; 32]);
    let seller1 = make_seller(&mut db, [0x16u8; 32], [0x26u8; 32]);
    let seller2 = make_seller(&mut db, [0x17u8; 32], [0x27u8; 32]);

    // First SLA at HEIGHT_PROPOSE = 100.
    let sla1 = sample_sla(&buyer, &seller1, 3);
    let id1 = propose(&mut db, &buyer, 0, &sla1);

    // Second SLA at a much later height. We simulate by re-using
    // propose_sla's HEIGHT_PROPOSE; in real usage the buyer would
    // submit at the actual chain head. Both end up at the same
    // height in this test, but their object_ids differ because the
    // seller is different (different memory-object IDs). Filter only
    // selects entries inside the window.
    let sla2 = sample_sla(&buyer, &seller2, 5);
    let id2 = propose(&mut db, &buyer, 1, &sla2);

    // Tight window around HEIGHT_PROPOSE: both match.
    let in_window = get_slas_by_buyer(&db, &buyer.id, HEIGHT_PROPOSE, HEIGHT_PROPOSE).unwrap();
    let ids: Vec<[u8; 32]> = in_window.iter().map(|(o, _)| o.object_id).collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&id1));
    assert!(ids.contains(&id2));

    // Window strictly before HEIGHT_PROPOSE: no matches.
    let too_early = get_slas_by_buyer(&db, &buyer.id, 0, HEIGHT_PROPOSE - 1).unwrap();
    assert!(too_early.is_empty());
}

#[test]
fn get_active_sla_returns_none_after_violation() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x18u8; 32], [0x28u8; 32]);
    let seller = make_seller(&mut db, [0x19u8; 32], [0x29u8; 32]);
    let sla = sample_sla(&buyer, &seller, 1);
    let sla_id = propose(&mut db, &buyer, 0, &sla);
    accept(&mut db, &seller, 0, sla_id, buyer.id);

    // Breach.
    let hash = [0x50u8; 32];
    seed_payment(&mut db, &buyer, &seller, hash, 600);
    attest_failed(&mut db, &buyer, &seller, hash, 1, 700);

    assert!(get_active_sla_between(&db, &buyer.id, &seller.id)
        .unwrap()
        .is_none());

    // But the (buyer, sla_id) point query still resolves the Violated
    // record for audit consumers.
    let (_, violated) = get_sla_agreement(&db, &buyer.id, &sla_id).unwrap().unwrap();
    assert_eq!(violated.status, SLA_STATUS_VIOLATED);
}
