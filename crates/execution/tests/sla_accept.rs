#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]

//! Integration tests for the Week 31 `SlaAccept` signal handler
//! (Phase 2).
//!
//! Each test proposes an `SlaAgreement` via `CreateMemoryObject` and
//! then exercises the `SlaAccept` signal path:
//!
//! - Happy path: status transitions PROPOSED -> ACTIVE,
//!   `accepted_at_height` is recorded, the active-between singleton
//!   stays in place.
//! - Defensive rejections: SLA not found, wrong object type at
//!   resolved id, SLA already accepted, wrong issuer (not the
//!   seller), acceptance after start_height, seller stake below
//!   `slash_amount` (Q2 gate).

use novai_ai_entities::{
    AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObjectType, SlaAgreementData,
    SLA_AGREEMENT_SIZE, SLA_AGREEMENT_V1, SLA_RESERVED_LEN, SLA_STATUS_ACTIVE, SLA_STATUS_PROPOSED,
};
use novai_execution::{
    apply_create_memory_object_tx, apply_signal_commitment_tx,
    encode_create_memory_object_payload_v1, encode_signal_commitment_payload_v1,
    sla_active_between_key, write_ai_entity_op, CreateMemoryObjectPayloadV1, ExecError,
    SignalCommitmentPayloadV1, SlaAcceptExtraV1,
};
use novai_state::{ai_entity_by_address_key, ai_memory_object_key, Kv, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const BUYER_BALANCE: u128 = 1_000_000;
const SELLER_STAKE: u128 = 5_000_000;
const CREATE_FEE: u64 = 1_000;
const ACCEPT_FEE: u64 = 1_000;
const HEIGHT_PROPOSE: u64 = 500;
const HEIGHT_ACCEPT: u64 = 700;
const SLA_START: u64 = 1_000;
const SLA_END: u64 = 5_000;
const DEFAULT_SLASH: u128 = 1_000_000;

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

fn make_seller(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32], stake: u128) -> AiEntity {
    let mut seller = build_entity(code_hash, creator);
    seller.stake_balance = stake;
    // Acceptance pays the tx fee from economic_balance.
    seller.economic_balance = BUYER_BALANCE;
    store_entity(db, &seller);
    seller
}

fn sample_sla(buyer: &AiEntity, seller: &AiEntity, slash_amount: u128) -> SlaAgreementData {
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
        slash_amount,
        terminated_at_height: 0,
        slashed_amount: 0,
        reserved: [0u8; SLA_RESERVED_LEN],
    }
}

fn propose_sla(db: &mut MemKv, buyer: &AiEntity, nonce: u64, sla: &SlaAgreementData) -> [u8; 32] {
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::SlaAgreement,
        data: sla.encode().to_vec(),
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: buyer.id,
        pubkey: buyer.id,
        nonce,
        fee: CREATE_FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_create_memory_object_tx(db, &tx, HEIGHT_PROPOSE).expect("propose succeeds")
}

fn make_accept_tx(
    seller: &AiEntity,
    nonce: u64,
    sla_object_id: [u8; 32],
    buyer_id: [u8; 32],
) -> TxV1 {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xEFu8; 32],
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
            sla_object_id,
            buyer_entity_id: buyer_id,
        }),
    });
    TxV1 {
        version: TxVersion::V1,
        from: seller.id,
        pubkey: seller.id,
        nonce,
        fee: ACCEPT_FEE,
        payload,
        sig: [0u8; 64],
    }
}

fn read_sla(db: &MemKv, buyer_id: &[u8; 32], object_id: &[u8; 32]) -> SlaAgreementData {
    let envelope = db
        .get(&ai_memory_object_key(buyer_id, object_id))
        .unwrap()
        .unwrap();
    let payload_start = envelope.len() - SLA_AGREEMENT_SIZE;
    SlaAgreementData::decode(&envelope[payload_start..]).expect("stored bytes decode")
}

// ============================================================================
// 1. Happy path: PROPOSED -> ACTIVE transition
// ============================================================================

#[test]
fn sla_accept_transitions_status_to_active() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let seller = make_seller(&mut db, [0x12u8; 32], [0x22u8; 32], SELLER_STAKE);
    let sla = sample_sla(&buyer, &seller, DEFAULT_SLASH);
    let object_id = propose_sla(&mut db, &buyer, 0, &sla);

    let tx = make_accept_tx(&seller, 0, object_id, buyer.id);
    apply_signal_commitment_tx(&mut db, &tx, HEIGHT_ACCEPT).expect("accept succeeds");

    let stored = read_sla(&db, &buyer.id, &object_id);
    assert_eq!(stored.status, SLA_STATUS_ACTIVE);
    assert_eq!(stored.accepted_at_height, HEIGHT_ACCEPT);
    // Fields that should NOT change on acceptance:
    assert_eq!(stored.buyer_entity_id, buyer.id);
    assert_eq!(stored.seller_entity_id, seller.id);
    assert_eq!(stored.start_height, SLA_START);
    assert_eq!(stored.end_height, SLA_END);
    assert_eq!(stored.violation_count, 0);
    assert_eq!(stored.violation_threshold, sla.violation_threshold);
    assert_eq!(stored.slash_amount, sla.slash_amount);
    assert_eq!(stored.terminated_at_height, 0);
    assert_eq!(stored.slashed_amount, 0);
}

#[test]
fn sla_accept_leaves_active_between_index_in_place() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x13u8; 32], [0x23u8; 32]);
    let seller = make_seller(&mut db, [0x14u8; 32], [0x24u8; 32], SELLER_STAKE);
    let sla = sample_sla(&buyer, &seller, DEFAULT_SLASH);
    let object_id = propose_sla(&mut db, &buyer, 0, &sla);

    // Singleton present after proposal.
    let pair_key = sla_active_between_key(&buyer.id, &seller.id);
    assert!(db.get(&pair_key).unwrap().is_some(), "pre-accept");

    let tx = make_accept_tx(&seller, 0, object_id, buyer.id);
    apply_signal_commitment_tx(&mut db, &tx, HEIGHT_ACCEPT).expect("accept succeeds");

    // Singleton still present after acceptance: the SLA is still OPEN
    // between this pair (just moved from Proposed to Active).
    let stored = db.get(&pair_key).unwrap().expect("post-accept singleton");
    assert_eq!(stored.as_slice(), &object_id[..]);
}

#[test]
fn sla_accept_signal_payload_is_130_bytes() {
    // Wire-length sanity: 66 (base) + 64 (sla_object_id || buyer) = 130.
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x15u8; 32], [0x25u8; 32]);
    let seller = make_seller(&mut db, [0x16u8; 32], [0x26u8; 32], SELLER_STAKE);
    let sla = sample_sla(&buyer, &seller, DEFAULT_SLASH);
    let object_id = propose_sla(&mut db, &buyer, 0, &sla);

    let tx = make_accept_tx(&seller, 0, object_id, buyer.id);
    assert_eq!(tx.payload.len(), 130);
}

// ============================================================================
// 2. Defensive rejections
// ============================================================================

#[test]
fn sla_accept_rejects_when_sla_not_found() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x17u8; 32], [0x27u8; 32]);
    let seller = make_seller(&mut db, [0x18u8; 32], [0x28u8; 32], SELLER_STAKE);
    // Do NOT propose an SLA. Seller targets a phantom object id.
    let phantom_object_id = [0xC1u8; 32];

    let tx = make_accept_tx(&seller, 0, phantom_object_id, buyer.id);
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_ACCEPT).unwrap_err();
    assert!(matches!(err, ExecError::SlaAcceptNotFound));
}

#[test]
fn sla_accept_rejects_when_resolved_object_is_wrong_type() {
    // Create a different memory object type at a known object_id slot,
    // then point the SlaAccept signal at it. The handler must surface
    // a type mismatch (not a generic decode error).
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x19u8; 32], [0x29u8; 32]);
    let seller = make_seller(&mut db, [0x1Au8; 32], [0x2Au8; 32], SELLER_STAKE);
    // Stand up a fake memory object that's not an SlaAgreement: simplest
    // is to write a raw envelope with ChainSummary at the same key.
    let raw_object_id = [0xC2u8; 32];
    let chain_summary = novai_ai_entities::ChainSummaryData {
        start_height: 0,
        end_height: 1,
        tx_count: 0,
        fee_total: 0,
        avg_block_fullness: 50,
    };
    let mem = novai_ai_entities::MemoryObject {
        object_id: raw_object_id,
        object_type: MemoryObjectType::ChainSummary,
        owner_entity: buyer.id,
        created_at: HEIGHT_PROPOSE,
        updated_at: HEIGHT_PROPOSE,
        data: chain_summary.encode(),
    };
    let bytes = novai_ai_entities::encode_memory_object_v1(&mem);
    db.apply_batch(&[WriteOp::Put(
        ai_memory_object_key(&buyer.id, &raw_object_id),
        bytes,
    )])
    .unwrap();

    let tx = make_accept_tx(&seller, 0, raw_object_id, buyer.id);
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_ACCEPT).unwrap_err();
    assert!(matches!(
        err,
        ExecError::SlaAcceptObjectTypeMismatch { found }
            if found == MemoryObjectType::ChainSummary.to_byte()
    ));
}

#[test]
fn sla_accept_rejects_double_acceptance() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x1Bu8; 32], [0x2Bu8; 32]);
    let seller = make_seller(&mut db, [0x1Cu8; 32], [0x2Cu8; 32], SELLER_STAKE);
    let sla = sample_sla(&buyer, &seller, DEFAULT_SLASH);
    let object_id = propose_sla(&mut db, &buyer, 0, &sla);

    // First acceptance succeeds.
    let tx1 = make_accept_tx(&seller, 0, object_id, buyer.id);
    apply_signal_commitment_tx(&mut db, &tx1, HEIGHT_ACCEPT).expect("first accept succeeds");

    // Second acceptance is rejected; SLA is no longer in PROPOSED state.
    let tx2 = make_accept_tx(&seller, 1, object_id, buyer.id);
    let err = apply_signal_commitment_tx(&mut db, &tx2, HEIGHT_ACCEPT + 1).unwrap_err();
    assert!(matches!(
        err,
        ExecError::SlaAcceptNotProposed { status } if status == SLA_STATUS_ACTIVE
    ));
}

#[test]
fn sla_accept_rejects_wrong_seller() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x1Du8; 32], [0x2Du8; 32]);
    let seller = make_seller(&mut db, [0x1Eu8; 32], [0x2Eu8; 32], SELLER_STAKE);
    let imposter = make_seller(&mut db, [0x1Fu8; 32], [0x2Fu8; 32], SELLER_STAKE);
    let sla = sample_sla(&buyer, &seller, DEFAULT_SLASH);
    let object_id = propose_sla(&mut db, &buyer, 0, &sla);

    // Imposter (different entity) tries to accept the SLA designating
    // `seller`. The signal issuer is the imposter; the SLA's seller_id
    // is `seller`. Should be rejected with SlaAcceptSellerMismatch.
    let tx = make_accept_tx(&imposter, 0, object_id, buyer.id);
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_ACCEPT).unwrap_err();
    assert!(matches!(err, ExecError::SlaAcceptSellerMismatch));
}

#[test]
fn sla_accept_rejects_at_start_height_boundary() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x30u8; 32], [0x40u8; 32]);
    let seller = make_seller(&mut db, [0x31u8; 32], [0x41u8; 32], SELLER_STAKE);
    let sla = sample_sla(&buyer, &seller, DEFAULT_SLASH);
    let object_id = propose_sla(&mut db, &buyer, 0, &sla);

    // Acceptance lands exactly at start_height: rejected. Locks the
    // boundary behavior (the rule is "must land strictly before start").
    let tx = make_accept_tx(&seller, 0, object_id, buyer.id);
    let err = apply_signal_commitment_tx(&mut db, &tx, SLA_START).unwrap_err();
    assert!(matches!(
        err,
        ExecError::SlaAcceptAfterStart { current, start } if current == SLA_START && start == SLA_START
    ));
}

#[test]
fn sla_accept_rejects_strictly_after_start_height() {
    // Separate test because the failing tx leaves the seller's nonce
    // at 0, so a follow-up tx in the same db with nonce=1 would fail
    // at the nonce check instead of the after-start check.
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x36u8; 32], [0x46u8; 32]);
    let seller = make_seller(&mut db, [0x37u8; 32], [0x47u8; 32], SELLER_STAKE);
    let sla = sample_sla(&buyer, &seller, DEFAULT_SLASH);
    let object_id = propose_sla(&mut db, &buyer, 0, &sla);

    let tx = make_accept_tx(&seller, 0, object_id, buyer.id);
    let err = apply_signal_commitment_tx(&mut db, &tx, SLA_START + 100).unwrap_err();
    assert!(matches!(
        err,
        ExecError::SlaAcceptAfterStart { current, start } if current == SLA_START + 100 && start == SLA_START
    ));
}

#[test]
fn sla_accept_rejects_seller_stake_below_slash_amount() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x32u8; 32], [0x42u8; 32]);
    // Seller's stake is BELOW the SLA's declared slash_amount.
    let seller = make_seller(&mut db, [0x33u8; 32], [0x43u8; 32], DEFAULT_SLASH - 1);
    let sla = sample_sla(&buyer, &seller, DEFAULT_SLASH);
    let object_id = propose_sla(&mut db, &buyer, 0, &sla);

    let tx = make_accept_tx(&seller, 0, object_id, buyer.id);
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_ACCEPT).unwrap_err();
    assert!(matches!(
        err,
        ExecError::SlaAcceptInsufficientStake { required, available }
            if required == DEFAULT_SLASH && available == DEFAULT_SLASH - 1
    ));
}

#[test]
fn sla_accept_accepts_seller_stake_equal_to_slash_amount() {
    // Boundary: stake_balance == slash_amount is sufficient (the rule
    // is "stake >= slash_amount").
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x34u8; 32], [0x44u8; 32]);
    let seller = make_seller(&mut db, [0x35u8; 32], [0x45u8; 32], DEFAULT_SLASH);
    let sla = sample_sla(&buyer, &seller, DEFAULT_SLASH);
    let object_id = propose_sla(&mut db, &buyer, 0, &sla);

    let tx = make_accept_tx(&seller, 0, object_id, buyer.id);
    apply_signal_commitment_tx(&mut db, &tx, HEIGHT_ACCEPT)
        .expect("stake == slash_amount is sufficient");
    assert_eq!(
        read_sla(&db, &buyer.id, &object_id).status,
        SLA_STATUS_ACTIVE
    );
}
