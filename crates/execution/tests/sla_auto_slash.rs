#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]

//! Integration tests for the Week 31 SLA auto-slash path
//! (Phase 3).
//!
//! Setup pattern: a buyer proposes an SLA, the seller accepts, then
//! the buyer pays the seller via NAP and attests Failed. Each test
//! asserts the slash math and the resulting state transitions.

use novai_ai_entities::{
    AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObjectType, SlaAgreementData,
    SLA_AGREEMENT_SIZE, SLA_AGREEMENT_V1, SLA_RESERVED_LEN, SLA_STATUS_ACTIVE, SLA_STATUS_PROPOSED,
    SLA_STATUS_VIOLATED,
};
use novai_execution::{
    apply_create_memory_object_tx, apply_signal_commitment_tx, decode_payment_record_v1,
    encode_create_memory_object_payload_v1, encode_payment_record_v1,
    encode_signal_commitment_payload_v1, payment_by_hash_key, payment_by_payee_key,
    payment_by_payer_key, sla_active_between_key, write_ai_entity_op, CreateMemoryObjectPayloadV1,
    ExecError, PaymentRecord, ServiceAttestationExtraV1, SignalCommitmentPayloadV1,
    SlaAcceptExtraV1, KEY_SLASH_TREASURY, PAYMENT_ATTESTATION_STATUS_DELIVERED,
    PAYMENT_ATTESTATION_STATUS_FAILED, PAYMENT_ATTESTATION_STATUS_NONE,
};
use novai_state::{ai_entity_by_address_key, ai_memory_object_key, Kv, KvBatch, MemKv, WriteOp};
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
    seller.economic_balance = SELLER_BALANCE;
    store_entity(db, &seller);
    seller
}

fn sample_sla_with_threshold(
    buyer: &AiEntity,
    seller: &AiEntity,
    threshold: u32,
) -> SlaAgreementData {
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
        fee: FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_create_memory_object_tx(db, &tx, HEIGHT_PROPOSE).expect("propose succeeds")
}

fn accept_sla(
    db: &mut MemKv,
    seller: &AiEntity,
    nonce: u64,
    sla_object_id: [u8; 32],
    buyer_id: [u8; 32],
) {
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
            sla_object_id,
            buyer_entity_id: buyer_id,
        }),
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
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
    apply_signal_commitment_tx(db, &tx, HEIGHT_ACCEPT).expect("accept succeeds");
}

/// Seed a PaymentRecord directly without going through the
/// PaymentRequest signal. Avoids cluttering tests with the full NAP
/// payment flow when the focus is the attestation hook.
fn seed_payment(
    db: &mut MemKv,
    payer: &AiEntity,
    payee: &AiEntity,
    signal_hash: [u8; 32],
    payment_height: u64,
) {
    let record = PaymentRecord {
        payer: payer.id,
        payee: payee.id,
        amount: 1_000,
        service_descriptor_hash: [0u8; 32],
        request_hash: [0xFFu8; 32],
        payment_height,
        max_block_height: payment_height + 100,
        attested_status: PAYMENT_ATTESTATION_STATUS_NONE,
        attested_height: 0,
    };
    let bytes = encode_payment_record_v1(&record);
    db.apply_batch(&[
        WriteOp::Put(payment_by_hash_key(&signal_hash), bytes.to_vec()),
        WriteOp::Put(
            payment_by_payer_key(&payer.id, payment_height, &signal_hash),
            Vec::new(),
        ),
        WriteOp::Put(
            payment_by_payee_key(&payee.id, payment_height, &signal_hash),
            Vec::new(),
        ),
    ])
    .unwrap();
}

fn attest(
    db: &mut MemKv,
    payer: &AiEntity,
    payee: &AiEntity,
    payment_signal_hash: [u8; 32],
    status: u8,
    nonce: u64,
    height: u64,
) -> Result<(), ExecError<<MemKv as Kv>::Error>> {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8 ^ nonce as u8; 32],
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
            status,
        }),
        sla_accept: None,
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
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
    apply_signal_commitment_tx(db, &tx, height)
}

fn read_sla(db: &MemKv, buyer_id: &[u8; 32], object_id: &[u8; 32]) -> SlaAgreementData {
    let envelope = db
        .get(&ai_memory_object_key(buyer_id, object_id))
        .unwrap()
        .unwrap();
    let payload_start = envelope.len() - SLA_AGREEMENT_SIZE;
    SlaAgreementData::decode(&envelope[payload_start..]).expect("stored bytes decode")
}

fn read_treasury(db: &MemKv) -> u128 {
    let bytes = db.get(KEY_SLASH_TREASURY).unwrap();
    bytes.map_or(0, |b| {
        // FeePoolV1 layout: version:1 | balance_be:16. The first byte is
        // the version, the next 16 are the u128 BE.
        assert_eq!(b.len(), 1 + 16);
        assert_eq!(b[0], 1, "FeePoolV1 version");
        let mut buf = [0u8; 16];
        buf.copy_from_slice(&b[1..17]);
        u128::from_be_bytes(buf)
    })
}

fn read_seller_stake(db: &MemKv, seller_id: &[u8; 32]) -> u128 {
    let entity_bytes = db
        .get(&novai_state::ai_entity_key(seller_id))
        .unwrap()
        .unwrap();
    let entity = novai_codec::decode_ai_entity(&entity_bytes).expect("decode entity");
    entity.stake_balance
}

fn read_seller_reputation(db: &MemKv, seller_id: &[u8; 32]) -> u16 {
    let entity_bytes = db
        .get(&novai_state::ai_entity_key(seller_id))
        .unwrap()
        .unwrap();
    let entity = novai_codec::decode_ai_entity(&entity_bytes).expect("decode entity");
    entity.reputation_score
}

// ============================================================================
// 1. Happy path: threshold breach -> slash + status transition
// ============================================================================

#[test]
fn sla_auto_slash_below_threshold_does_not_fire() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let seller = make_seller(&mut db, [0x12u8; 32], [0x22u8; 32], SELLER_STAKE);
    let sla = sample_sla_with_threshold(&buyer, &seller, 3);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    let stake_pre = read_seller_stake(&db, &seller.id);
    let treasury_pre = read_treasury(&db);

    // 2 failures: below the threshold of 3.
    for i in 0..2u64 {
        let signal_hash = [0x10 + i as u8; 32];
        seed_payment(&mut db, &buyer, &seller, signal_hash, 600 + i);
        attest(
            &mut db,
            &buyer,
            &seller,
            signal_hash,
            PAYMENT_ATTESTATION_STATUS_FAILED,
            i + 1,
            700 + i,
        )
        .unwrap();
    }

    let stored = read_sla(&db, &buyer.id, &sla_id);
    assert_eq!(
        stored.status, SLA_STATUS_ACTIVE,
        "stays active under threshold"
    );
    assert_eq!(stored.violation_count, 2);
    assert_eq!(stored.slashed_amount, 0);
    assert_eq!(stored.terminated_at_height, 0);
    assert_eq!(read_seller_stake(&db, &seller.id), stake_pre, "no slash");
    assert_eq!(read_treasury(&db), treasury_pre, "no treasury credit");
}

#[test]
fn sla_auto_slash_at_threshold_fires_once_and_terminates() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x13u8; 32], [0x23u8; 32]);
    let seller = make_seller(&mut db, [0x14u8; 32], [0x24u8; 32], SELLER_STAKE);
    let sla = sample_sla_with_threshold(&buyer, &seller, 3);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    let stake_pre = read_seller_stake(&db, &seller.id);
    let treasury_pre = read_treasury(&db);
    let rep_pre = read_seller_reputation(&db, &seller.id);

    // 3 failures: exactly at threshold.
    for i in 0..3u64 {
        let signal_hash = [0x20 + i as u8; 32];
        seed_payment(&mut db, &buyer, &seller, signal_hash, 600 + i);
        attest(
            &mut db,
            &buyer,
            &seller,
            signal_hash,
            PAYMENT_ATTESTATION_STATUS_FAILED,
            i + 1,
            700 + i,
        )
        .unwrap();
    }

    let stored = read_sla(&db, &buyer.id, &sla_id);
    assert_eq!(stored.status, SLA_STATUS_VIOLATED);
    assert_eq!(stored.violation_count, 3);
    assert_eq!(stored.slashed_amount, SLASH_AMOUNT);
    assert_eq!(stored.terminated_at_height, 702);
    assert_eq!(
        read_seller_stake(&db, &seller.id),
        stake_pre - SLASH_AMOUNT,
        "stake debited by slash_amount"
    );
    assert_eq!(
        read_treasury(&db),
        treasury_pre + SLASH_AMOUNT,
        "treasury credited by slash_amount"
    );

    // Reputation hits: each of the 3 FAILED attestations applies -3,
    // plus the breaching one applies an additional -5. Total: -14.
    // Clamped at 0 if seller started below 14.
    let expected_rep = (i32::from(rep_pre) - 9 - 5).max(0) as u16;
    assert_eq!(read_seller_reputation(&db, &seller.id), expected_rep);

    // Active-between singleton was deleted on breach.
    let pair_key = sla_active_between_key(&buyer.id, &seller.id);
    assert!(
        db.get(&pair_key).unwrap().is_none(),
        "active_between cleared on auto-slash"
    );
}

#[test]
fn sla_auto_slash_does_not_double_fire() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x15u8; 32], [0x25u8; 32]);
    let seller = make_seller(&mut db, [0x16u8; 32], [0x26u8; 32], SELLER_STAKE);
    let sla = sample_sla_with_threshold(&buyer, &seller, 1);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    // First FAILED triggers slash + transition to Violated.
    let signal_hash_1 = [0x31u8; 32];
    seed_payment(&mut db, &buyer, &seller, signal_hash_1, 600);
    attest(
        &mut db,
        &buyer,
        &seller,
        signal_hash_1,
        PAYMENT_ATTESTATION_STATUS_FAILED,
        1,
        700,
    )
    .unwrap();
    let stake_after_first = read_seller_stake(&db, &seller.id);
    let treasury_after_first = read_treasury(&db);
    assert_eq!(stake_after_first, SELLER_STAKE - SLASH_AMOUNT);

    // Second FAILED: SLA is in Violated state; active-between was
    // deleted; auto-slash skips entirely. The standard PaymentFailed
    // reputation delta still fires.
    let signal_hash_2 = [0x32u8; 32];
    seed_payment(&mut db, &buyer, &seller, signal_hash_2, 800);
    attest(
        &mut db,
        &buyer,
        &seller,
        signal_hash_2,
        PAYMENT_ATTESTATION_STATUS_FAILED,
        2,
        900,
    )
    .unwrap();
    assert_eq!(
        read_seller_stake(&db, &seller.id),
        stake_after_first,
        "no double slash"
    );
    assert_eq!(
        read_treasury(&db),
        treasury_after_first,
        "no double treasury credit"
    );
}

// ============================================================================
// 2. Saturating slash when seller stake < slash_amount
// ============================================================================

#[test]
fn sla_auto_slash_saturates_when_stake_below_slash_amount() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x17u8; 32], [0x27u8; 32]);
    // Seller has stake >= slash_amount at acceptance (gate passes),
    // but the test then artificially reduces stake below slash_amount
    // before the threshold-breaching attestation. This simulates a
    // seller that withdrew most of their stake during the SLA window
    // (Phase 4 will gate this; Phase 3 only verifies saturating math).
    let seller = make_seller(&mut db, [0x18u8; 32], [0x28u8; 32], SLASH_AMOUNT);
    let sla = sample_sla_with_threshold(&buyer, &seller, 1);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    // Manually drain the seller's stake to less than slash_amount.
    let mut drained = seller.clone();
    drained.nonce = 1;
    drained.stake_balance = SLASH_AMOUNT / 4;
    db.apply_batch(&[write_ai_entity_op(&drained)]).unwrap();
    let stake_pre = read_seller_stake(&db, &seller.id);
    let treasury_pre = read_treasury(&db);

    let signal_hash = [0x41u8; 32];
    seed_payment(&mut db, &buyer, &seller, signal_hash, 600);
    attest(
        &mut db,
        &buyer,
        &seller,
        signal_hash,
        PAYMENT_ATTESTATION_STATUS_FAILED,
        1,
        700,
    )
    .unwrap();

    let stored = read_sla(&db, &buyer.id, &sla_id);
    assert_eq!(stored.status, SLA_STATUS_VIOLATED);
    assert_eq!(
        stored.slashed_amount, stake_pre,
        "saturating at available stake"
    );
    assert_eq!(read_seller_stake(&db, &seller.id), 0, "fully drained");
    assert_eq!(read_treasury(&db), treasury_pre + stake_pre);
}

#[test]
fn sla_auto_slash_with_zero_stake_only_terminates_no_treasury_credit() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x19u8; 32], [0x29u8; 32]);
    let seller = make_seller(&mut db, [0x1Au8; 32], [0x2Au8; 32], SLASH_AMOUNT);
    let sla = sample_sla_with_threshold(&buyer, &seller, 1);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    // Drain seller stake to exactly 0 before the breach.
    let mut drained = seller.clone();
    drained.nonce = 1;
    drained.stake_balance = 0;
    db.apply_batch(&[write_ai_entity_op(&drained)]).unwrap();

    let treasury_pre = read_treasury(&db);

    let signal_hash = [0x51u8; 32];
    seed_payment(&mut db, &buyer, &seller, signal_hash, 600);
    attest(
        &mut db,
        &buyer,
        &seller,
        signal_hash,
        PAYMENT_ATTESTATION_STATUS_FAILED,
        1,
        700,
    )
    .unwrap();

    let stored = read_sla(&db, &buyer.id, &sla_id);
    assert_eq!(stored.status, SLA_STATUS_VIOLATED);
    assert_eq!(stored.slashed_amount, 0);
    assert_eq!(read_seller_stake(&db, &seller.id), 0);
    assert_eq!(
        read_treasury(&db),
        treasury_pre,
        "no treasury credit on zero slash"
    );
}

// ============================================================================
// 3. Window enforcement
// ============================================================================

#[test]
fn sla_auto_slash_ignores_attestation_before_start_height() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x1Bu8; 32], [0x2Bu8; 32]);
    let seller = make_seller(&mut db, [0x1Cu8; 32], [0x2Cu8; 32], SELLER_STAKE);
    let sla = sample_sla_with_threshold(&buyer, &seller, 1);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    let stake_pre = read_seller_stake(&db, &seller.id);

    // Attest at height SLA_START - 1: outside the window.
    let signal_hash = [0x61u8; 32];
    seed_payment(&mut db, &buyer, &seller, signal_hash, SLA_START - 50);
    attest(
        &mut db,
        &buyer,
        &seller,
        signal_hash,
        PAYMENT_ATTESTATION_STATUS_FAILED,
        1,
        SLA_START - 1,
    )
    .unwrap();

    let stored = read_sla(&db, &buyer.id, &sla_id);
    assert_eq!(
        stored.violation_count, 0,
        "before-start attestation does not count"
    );
    assert_eq!(stored.status, SLA_STATUS_ACTIVE);
    assert_eq!(read_seller_stake(&db, &seller.id), stake_pre);
}

#[test]
fn sla_auto_slash_ignores_attestation_after_end_height() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x1Du8; 32], [0x2Du8; 32]);
    let seller = make_seller(&mut db, [0x1Eu8; 32], [0x2Eu8; 32], SELLER_STAKE);
    let sla = sample_sla_with_threshold(&buyer, &seller, 1);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    let stake_pre = read_seller_stake(&db, &seller.id);

    // Attest at SLA_END + 1: outside the window.
    let signal_hash = [0x71u8; 32];
    seed_payment(&mut db, &buyer, &seller, signal_hash, SLA_END);
    attest(
        &mut db,
        &buyer,
        &seller,
        signal_hash,
        PAYMENT_ATTESTATION_STATUS_FAILED,
        1,
        SLA_END + 1,
    )
    .unwrap();

    let stored = read_sla(&db, &buyer.id, &sla_id);
    assert_eq!(stored.violation_count, 0);
    assert_eq!(stored.status, SLA_STATUS_ACTIVE);
    assert_eq!(read_seller_stake(&db, &seller.id), stake_pre);
}

#[test]
fn sla_auto_slash_skipped_when_status_still_proposed() {
    // Boundary: an SLA that exists in active-between but has not been
    // accepted by the seller yet (status = Proposed). The hook must
    // NOT count a FAILED attestation as a violation.
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x1Fu8; 32], [0x2Fu8; 32]);
    let seller = make_seller(&mut db, [0x30u8; 32], [0x40u8; 32], SELLER_STAKE);
    let sla = sample_sla_with_threshold(&buyer, &seller, 1);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    // No accept.

    let stake_pre = read_seller_stake(&db, &seller.id);

    let signal_hash = [0x81u8; 32];
    seed_payment(&mut db, &buyer, &seller, signal_hash, 600);
    attest(
        &mut db,
        &buyer,
        &seller,
        signal_hash,
        PAYMENT_ATTESTATION_STATUS_FAILED,
        1,
        700,
    )
    .unwrap();

    let stored = read_sla(&db, &buyer.id, &sla_id);
    assert_eq!(stored.status, SLA_STATUS_PROPOSED);
    assert_eq!(stored.violation_count, 0);
    assert_eq!(read_seller_stake(&db, &seller.id), stake_pre);
}

// ============================================================================
// 4. Existing behavior unchanged
// ============================================================================

#[test]
fn sla_no_pair_no_auto_slash_still_applies_payment_failed_delta() {
    // No SLA exists between this (payer, payee). The FAILED
    // attestation must still apply REP_DELTA_PAYMENT_FAILED (-3),
    // matching pre-Week-31 behavior.
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x31u8; 32], [0x41u8; 32]);
    let seller = make_seller(&mut db, [0x32u8; 32], [0x42u8; 32], SELLER_STAKE);

    let rep_pre = read_seller_reputation(&db, &seller.id);
    let stake_pre = read_seller_stake(&db, &seller.id);

    let signal_hash = [0x91u8; 32];
    seed_payment(&mut db, &buyer, &seller, signal_hash, 600);
    attest(
        &mut db,
        &buyer,
        &seller,
        signal_hash,
        PAYMENT_ATTESTATION_STATUS_FAILED,
        0,
        700,
    )
    .unwrap();

    let expected_rep = (i32::from(rep_pre) - 3).max(0) as u16;
    assert_eq!(read_seller_reputation(&db, &seller.id), expected_rep);
    assert_eq!(
        read_seller_stake(&db, &seller.id),
        stake_pre,
        "no slash without SLA"
    );

    let record_bytes = db.get(&payment_by_hash_key(&signal_hash)).unwrap().unwrap();
    let record = decode_payment_record_v1(&record_bytes).unwrap();
    assert_eq!(record.attested_status, PAYMENT_ATTESTATION_STATUS_FAILED);
}

#[test]
fn sla_delivered_attestation_does_not_increment_violation_count() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x33u8; 32], [0x43u8; 32]);
    let seller = make_seller(&mut db, [0x34u8; 32], [0x44u8; 32], SELLER_STAKE);
    let sla = sample_sla_with_threshold(&buyer, &seller, 1);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    // 5 successful deliveries: violation count must stay at 0 and the
    // SLA must remain Active. No memory-object rewrites either: the
    // hook only fires on FAILED.
    for i in 0..5u64 {
        let signal_hash = [0xA0 + i as u8; 32];
        seed_payment(&mut db, &buyer, &seller, signal_hash, 600 + i);
        attest(
            &mut db,
            &buyer,
            &seller,
            signal_hash,
            PAYMENT_ATTESTATION_STATUS_DELIVERED,
            i + 1,
            700 + i,
        )
        .unwrap();
    }

    let stored = read_sla(&db, &buyer.id, &sla_id);
    assert_eq!(stored.status, SLA_STATUS_ACTIVE);
    assert_eq!(stored.violation_count, 0);
}

#[test]
fn sla_auto_slash_writes_envelope_with_correct_updated_at() {
    // Lock the on-chain side effect: the SLA memory object's
    // `updated_at` field is bumped to the breach height. Off-chain
    // consumers rely on this to detect lifecycle transitions.
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x35u8; 32], [0x45u8; 32]);
    let seller = make_seller(&mut db, [0x36u8; 32], [0x46u8; 32], SELLER_STAKE);
    let sla = sample_sla_with_threshold(&buyer, &seller, 1);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    let breach_height = 850u64;
    let signal_hash = [0xB1u8; 32];
    seed_payment(&mut db, &buyer, &seller, signal_hash, 800);
    attest(
        &mut db,
        &buyer,
        &seller,
        signal_hash,
        PAYMENT_ATTESTATION_STATUS_FAILED,
        1,
        breach_height,
    )
    .unwrap();

    // Decode the full MemoryObject envelope to inspect `updated_at`.
    let bytes = db
        .get(&ai_memory_object_key(&buyer.id, &sla_id))
        .unwrap()
        .unwrap();
    let envelope = novai_ai_entities::decode_memory_object_v1(&bytes).expect("decode envelope");
    assert_eq!(envelope.object_type, MemoryObjectType::SlaAgreement);
    assert_eq!(envelope.updated_at, breach_height);
}
