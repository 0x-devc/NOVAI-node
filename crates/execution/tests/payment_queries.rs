#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]

//! Integration tests for `get_payments_by_entity` (Week 28, Phase 4).
//!
//! Exercises the helper that backs the `novai_getPaymentsByEntity`
//! RPC endpoint:
//!
//! - Returns payer's outgoing payments in height-ascending order.
//! - Returns payee's incoming payments.
//! - Filters by the `[start_height, end_height]` window correctly,
//!   including the inclusive boundary semantics.
//! - Returns an empty vec when the entity has no matching payments.
//! - Surfaces the attested fields after a ServiceAttestation.

use novai_ai_entities::{AiEntity, AiSignalType, AutonomyMode, Capabilities};
use novai_execution::{
    apply_signal_commitment_tx, encode_signal_commitment_payload_v1, get_payments_by_entity,
    write_ai_entity_op, PaymentRequestExtraV1, PaymentRole, ServiceAttestationExtraV1,
    SignalCommitmentPayloadV1, PAYMENT_ATTESTATION_STATUS_DELIVERED,
    PAYMENT_ATTESTATION_STATUS_FAILED, PAYMENT_ATTESTATION_STATUS_NONE,
};
use novai_state::{ai_entity_by_address_key, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const PAYER_BALANCE: u128 = 10_000_000;
const PAYEE_BALANCE: u128 = 250;
const SIGNAL_FEE: u64 = 1_000;
const EXPIRY_HEIGHT: u64 = 100_000;
const PAYMENT_AMOUNT: u64 = 1_000;

fn caps() -> Capabilities {
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
    AiEntity::new(code_hash, creator, AutonomyMode::Gated, caps(), 1000)
}

fn store_entity(db: &mut MemKv, entity: &AiEntity) {
    use novai_state::KvBatch;
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

fn make_payee(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut payee = build_entity(code_hash, creator);
    payee.economic_balance = PAYEE_BALANCE;
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

fn mk_tx(from: [u8; 32], nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
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

fn seed_payment(
    db: &mut MemKv,
    payer: &AiEntity,
    payee: &AiEntity,
    signal_hash: [u8; 32],
    nonce: u64,
    height: u64,
) {
    let payload = payment_payload(signal_hash, payer.id, payee.id, PAYMENT_AMOUNT);
    let tx = mk_tx(payer.id, nonce, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(db, &tx, height).expect("payment settles");
}

// ============================================================================
// 1. Outgoing payments returned in ascending height order
// ============================================================================

#[test]
fn payments_by_payer_returned_in_height_order() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let payee_a = make_payee(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let payee_b = make_payee(&mut db, [0x13u8; 32], [0x23u8; 32]);

    // Three payments at heights 100, 200, 300.
    seed_payment(&mut db, &payer, &payee_a, [0xA1u8; 32], 0, 100);
    seed_payment(&mut db, &payer, &payee_b, [0xA2u8; 32], 1, 200);
    seed_payment(&mut db, &payer, &payee_a, [0xA3u8; 32], 2, 300);

    let payments = get_payments_by_entity(&db, &payer.id, PaymentRole::Payer, 0, 1000).unwrap();
    assert_eq!(payments.len(), 3);

    // Height-ascending order falls out of the big-endian-height index
    // layout. This is the key property the RPC depends on.
    assert_eq!(payments[0].payment_height, 100);
    assert_eq!(payments[1].payment_height, 200);
    assert_eq!(payments[2].payment_height, 300);

    // Each record refers back to its respective payee.
    assert_eq!(payments[0].payee, payee_a.id);
    assert_eq!(payments[1].payee, payee_b.id);
    assert_eq!(payments[2].payee, payee_a.id);

    // No attestation yet → sentinel.
    for r in &payments {
        assert_eq!(r.attested_status, PAYMENT_ATTESTATION_STATUS_NONE);
        assert_eq!(r.attested_height, 0);
    }
}

// ============================================================================
// 2. Incoming payments returned per-payee
// ============================================================================

#[test]
fn payments_by_payee_returns_only_matching_incoming() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x14u8; 32], [0x24u8; 32]);
    let payee_a = make_payee(&mut db, [0x15u8; 32], [0x25u8; 32]);
    let payee_b = make_payee(&mut db, [0x16u8; 32], [0x26u8; 32]);

    // Two payments to payee_a, one to payee_b.
    seed_payment(&mut db, &payer, &payee_a, [0xA1u8; 32], 0, 100);
    seed_payment(&mut db, &payer, &payee_a, [0xA2u8; 32], 1, 150);
    seed_payment(&mut db, &payer, &payee_b, [0xA3u8; 32], 2, 200);

    let payments_a = get_payments_by_entity(&db, &payee_a.id, PaymentRole::Payee, 0, 1000).unwrap();
    assert_eq!(payments_a.len(), 2);
    for r in &payments_a {
        assert_eq!(r.payee, payee_a.id);
    }
    assert_eq!(payments_a[0].payment_height, 100);
    assert_eq!(payments_a[1].payment_height, 150);

    let payments_b = get_payments_by_entity(&db, &payee_b.id, PaymentRole::Payee, 0, 1000).unwrap();
    assert_eq!(payments_b.len(), 1);
    assert_eq!(payments_b[0].payee, payee_b.id);
    assert_eq!(payments_b[0].payment_height, 200);
}

// ============================================================================
// 3. Height window is inclusive on both ends
// ============================================================================

#[test]
fn payments_by_entity_height_filter_is_inclusive() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x17u8; 32], [0x27u8; 32]);
    let payee = make_payee(&mut db, [0x18u8; 32], [0x28u8; 32]);

    seed_payment(&mut db, &payer, &payee, [0xA1u8; 32], 0, 100);
    seed_payment(&mut db, &payer, &payee, [0xA2u8; 32], 1, 200);
    seed_payment(&mut db, &payer, &payee, [0xA3u8; 32], 2, 300);
    seed_payment(&mut db, &payer, &payee, [0xA4u8; 32], 3, 400);

    // Window [200, 300] must include both 200 and 300, exclude 100 and 400.
    let in_window = get_payments_by_entity(&db, &payer.id, PaymentRole::Payer, 200, 300).unwrap();
    assert_eq!(in_window.len(), 2);
    assert_eq!(in_window[0].payment_height, 200);
    assert_eq!(in_window[1].payment_height, 300);

    // Tight singleton window: [200, 200] returns exactly the height-200
    // payment.
    let single = get_payments_by_entity(&db, &payer.id, PaymentRole::Payer, 200, 200).unwrap();
    assert_eq!(single.len(), 1);
    assert_eq!(single[0].payment_height, 200);

    // Empty window: [250, 280] catches none of the four heights.
    let empty = get_payments_by_entity(&db, &payer.id, PaymentRole::Payer, 250, 280).unwrap();
    assert!(empty.is_empty());
}

// ============================================================================
// 4. Empty result for unknown entity
// ============================================================================

#[test]
fn payments_by_entity_unknown_id_returns_empty() {
    let db = MemKv::new();
    let stranger = [0xFFu8; 32];
    let payer_view = get_payments_by_entity(&db, &stranger, PaymentRole::Payer, 0, 1000).unwrap();
    let payee_view = get_payments_by_entity(&db, &stranger, PaymentRole::Payee, 0, 1000).unwrap();
    assert!(payer_view.is_empty());
    assert!(payee_view.is_empty());
}

// ============================================================================
// 5. Attested fields surface after a ServiceAttestation
// ============================================================================

#[test]
fn payments_by_entity_returns_attested_status_after_attestation() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x19u8; 32], [0x29u8; 32]);
    let payee = make_payee(&mut db, [0x1Au8; 32], [0x2Au8; 32]);

    let payment_hash = [0xAAu8; 32];
    seed_payment(&mut db, &payer, &payee, payment_hash, 0, 100);

    // Attest as Failed at height 110.
    let tx = mk_tx(
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
    apply_signal_commitment_tx(&mut db, &tx, 110).expect("attestation succeeds");

    let payments = get_payments_by_entity(&db, &payer.id, PaymentRole::Payer, 0, 1000).unwrap();
    assert_eq!(payments.len(), 1);
    let r = &payments[0];
    assert_eq!(r.attested_status, PAYMENT_ATTESTATION_STATUS_FAILED);
    assert_eq!(r.attested_height, 110);
    // Original PaymentRequest fields preserved verbatim.
    assert_eq!(r.payer, payer.id);
    assert_eq!(r.payee, payee.id);
    assert_eq!(r.amount, PAYMENT_AMOUNT);
    assert_eq!(r.payment_height, 100);

    // Same record visible from the payee's side.
    let from_payee = get_payments_by_entity(&db, &payee.id, PaymentRole::Payee, 0, 1000).unwrap();
    assert_eq!(from_payee.len(), 1);
    assert_eq!(
        from_payee[0].attested_status,
        PAYMENT_ATTESTATION_STATUS_FAILED
    );
    assert_eq!(from_payee[0].attested_height, 110);
}

// ============================================================================
// 6. Delivered status round-trips through the query helper
// ============================================================================

#[test]
fn payments_by_entity_reports_delivered_attestation() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x1Bu8; 32], [0x2Bu8; 32]);
    let payee = make_payee(&mut db, [0x1Cu8; 32], [0x2Cu8; 32]);

    let payment_hash = [0xAAu8; 32];
    seed_payment(&mut db, &payer, &payee, payment_hash, 0, 100);

    let tx = mk_tx(
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
    apply_signal_commitment_tx(&mut db, &tx, 105).expect("attestation succeeds");

    let payments = get_payments_by_entity(&db, &payer.id, PaymentRole::Payer, 0, 1000).unwrap();
    assert_eq!(payments.len(), 1);
    assert_eq!(
        payments[0].attested_status,
        PAYMENT_ATTESTATION_STATUS_DELIVERED
    );
    assert_eq!(payments[0].attested_height, 105);
}
