#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

//! Integration tests for Feature 9 Signal Subscriptions.
//!
//! Covers:
//! - Happy path: subscriber locks funds, creates a Subscription memory object,
//!   producer is unaffected at create time.
//! - Cancellation: settlement of accrued payment with 2 percent marketplace fee,
//!   5 percent cancel fee paid 100 percent to producer with no marketplace cut,
//!   refund of remainder to subscriber, in-place rewrite of the memory object
//!   with `is_active = false`.
//! - Validation: insufficient balance, missing producer, self-subscription,
//!   sub-MIN_SUBSCRIPTION_DURATION duration, MAX_SUBSCRIPTIONS_PER_ENTITY cap.
//! - Authorization: only the subscriber may cancel.
//! - Idempotence: a second cancel on the same record is rejected.
//! - End-of-window: cancellation at or after `end_height` settles the full
//!   duration and leaves no refund (and no cancel fee).
//! - Treasury: marketplace fee accrues to KEY_MARKETPLACE_TREASURY.
//! - Regression: unrelated signal types continue to work alongside the
//!   subscription codec.

use novai_ai_entities::{
    AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObjectType, SubscriptionData,
    MAX_SUBSCRIPTIONS_PER_ENTITY,
};
use novai_execution::{
    apply_signal_commitment_tx, encode_signal_commitment_payload_v1,
    get_memory_objects_by_entity_and_type, read_ai_entity, write_ai_entity_op, ExecError,
    SignalCommitmentPayloadV1, SubscriptionCancelExtraV1, SubscriptionCreateExtraV1,
    BPS_DENOMINATOR, KEY_MARKETPLACE_TREASURY, MARKETPLACE_FEE_BPS, MIN_SUBSCRIPTION_DURATION,
    SUBSCRIPTION_CANCEL_FEE_BPS,
};
use novai_state::{ai_entity_by_address_key, decode_fee_pool_v1, Kv, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const SUBSCRIBER_BALANCE: u128 = 10_000_000;
const PRODUCER_BALANCE: u128 = 0;
const SIGNAL_FEE: u64 = 1_000;
const CREATE_HEIGHT: u64 = 1_000;
const RATE_PER_BLOCK: u64 = 10;
const DURATION_BLOCKS: u64 = 10_000;

// ============================================================================
// Helpers
// ============================================================================

fn subscription_caps() -> Capabilities {
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

fn build_entity(code_hash: [u8; 32], creator: [u8; 32], caps: Capabilities) -> AiEntity {
    AiEntity::new(code_hash, creator, AutonomyMode::Gated, caps, 1000)
}

fn store_entity(db: &mut MemKv, entity: &AiEntity) {
    db.apply_batch(&[
        write_ai_entity_op(entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();
}

fn make_subscriber(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut sub = build_entity(code_hash, creator, subscription_caps());
    sub.economic_balance = SUBSCRIBER_BALANCE;
    store_entity(db, &sub);
    sub
}

fn make_producer(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut prod = build_entity(code_hash, creator, subscription_caps());
    prod.economic_balance = PRODUCER_BALANCE;
    store_entity(db, &prod);
    prod
}

fn build_create_payload(
    subscriber: [u8; 32],
    producer: [u8; 32],
    rate: u64,
    duration: u64,
) -> Vec<u8> {
    encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xC1u8; 32],
        signal_type: AiSignalType::SubscriptionCreate,
        issuer_entity_id: subscriber,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
        subscription_create: Some(SubscriptionCreateExtraV1 {
            producer_entity_id: producer,
            covered_signal_type: AiSignalType::Prediction.to_byte(),
            rate_per_block: rate,
            duration_blocks: duration,
        }),
        subscription_cancel: None,
        payment_request: None,
        service_attestation: None,
    })
}

fn build_cancel_payload(subscriber: [u8; 32], subscription_id: [u8; 32]) -> Vec<u8> {
    encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xC2u8; 32],
        signal_type: AiSignalType::SubscriptionCancel,
        issuer_entity_id: subscriber,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
        subscription_create: None,
        subscription_cancel: Some(SubscriptionCancelExtraV1 { subscription_id }),
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

fn read_treasury(db: &MemKv) -> u128 {
    db.get(KEY_MARKETPLACE_TREASURY)
        .unwrap()
        .map_or(0, |bytes| decode_fee_pool_v1(&bytes).unwrap().balance)
}

fn single_subscription_id(db: &MemKv, subscriber: &[u8; 32]) -> [u8; 32] {
    let objs = get_memory_objects_by_entity_and_type(
        db,
        subscriber,
        MemoryObjectType::Subscription.to_byte(),
    )
    .expect("scan ok");
    assert_eq!(objs.len(), 1, "expected exactly one Subscription object");
    objs[0].object_id
}

fn read_subscription(db: &MemKv, subscriber: &[u8; 32], id: &[u8; 32]) -> SubscriptionData {
    let obj = novai_execution::read_memory_object(db, subscriber, id)
        .expect("read ok")
        .expect("memory object exists");
    SubscriptionData::decode(&obj.data).expect("decode SubscriptionData")
}

// ============================================================================
// 1. Create happy path
// ============================================================================

#[test]
fn subscription_create_basic() {
    let mut db = MemKv::new();
    let sub = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let prod = make_producer(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let payload = build_create_payload(sub.id, prod.id, RATE_PER_BLOCK, DURATION_BLOCKS);
    let tx = make_tx(sub.id, 0, SIGNAL_FEE, payload);

    apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT).expect("create succeeds");

    let sub_id = single_subscription_id(&db, &sub.id);
    let record = read_subscription(&db, &sub.id, &sub_id);
    assert_eq!(record.subscriber_entity_id, sub.id);
    assert_eq!(record.producer_entity_id, prod.id);
    assert_eq!(record.rate_per_block, RATE_PER_BLOCK);
    assert_eq!(record.start_height, CREATE_HEIGHT);
    assert_eq!(record.end_height, CREATE_HEIGHT + DURATION_BLOCKS);
    assert_eq!(record.last_settled_height, CREATE_HEIGHT);
    assert_eq!(
        record.total_locked,
        u128::from(RATE_PER_BLOCK) * u128::from(DURATION_BLOCKS)
    );
    assert!(record.is_active);
}

#[test]
fn subscription_create_locks_full_total_from_subscriber() {
    let mut db = MemKv::new();
    let sub = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let prod = make_producer(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let total = u128::from(RATE_PER_BLOCK) * u128::from(DURATION_BLOCKS);
    let payload = build_create_payload(sub.id, prod.id, RATE_PER_BLOCK, DURATION_BLOCKS);
    let tx = make_tx(sub.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT).expect("create succeeds");

    let sub_after = read_ai_entity(&db, &sub.id).unwrap().unwrap();
    let prod_after = read_ai_entity(&db, &prod.id).unwrap().unwrap();
    assert_eq!(
        sub_after.economic_balance,
        SUBSCRIBER_BALANCE - total - u128::from(SIGNAL_FEE),
        "subscriber debited by total_locked + signal fee"
    );
    assert_eq!(
        prod_after.economic_balance, PRODUCER_BALANCE,
        "producer is NOT credited at create time"
    );
}

// ============================================================================
// 2. Create-side rejections
// ============================================================================

#[test]
fn subscription_create_insufficient_balance_rejected() {
    let mut db = MemKv::new();
    // Subscriber only has 1_000 base units; total_locked would be 100_000.
    let mut sub = build_entity([0x11u8; 32], [0x21u8; 32], subscription_caps());
    sub.economic_balance = 1_000;
    store_entity(&mut db, &sub);
    let prod = make_producer(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let payload = build_create_payload(sub.id, prod.id, RATE_PER_BLOCK, DURATION_BLOCKS);
    let tx = make_tx(sub.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::SubscriptionInsufficientBalance { .. }),
        "expected SubscriptionInsufficientBalance, got {err:?}"
    );
}

#[test]
fn subscription_create_producer_not_found_rejected() {
    let mut db = MemKv::new();
    let sub = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let fake_producer = [0xAFu8; 32];
    let payload = build_create_payload(sub.id, fake_producer, RATE_PER_BLOCK, DURATION_BLOCKS);
    let tx = make_tx(sub.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::SubscriptionProducerNotFound),
        "got {err:?}"
    );
}

#[test]
fn subscription_create_producer_inactive_rejected() {
    let mut db = MemKv::new();
    let sub = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let mut prod = build_entity([0x12u8; 32], [0x22u8; 32], subscription_caps());
    prod.is_active = false;
    store_entity(&mut db, &prod);

    let payload = build_create_payload(sub.id, prod.id, RATE_PER_BLOCK, DURATION_BLOCKS);
    let tx = make_tx(sub.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::SubscriptionProducerNotActive),
        "got {err:?}"
    );
}

#[test]
fn subscription_create_self_subscription_rejected() {
    let mut db = MemKv::new();
    let sub = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let payload = build_create_payload(sub.id, sub.id, RATE_PER_BLOCK, DURATION_BLOCKS);
    let tx = make_tx(sub.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::SubscriptionSelfReferential),
        "got {err:?}"
    );
}

#[test]
fn subscription_create_duration_below_min_rejected() {
    let mut db = MemKv::new();
    let sub = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let prod = make_producer(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let too_short = MIN_SUBSCRIPTION_DURATION - 1;
    let payload = build_create_payload(sub.id, prod.id, RATE_PER_BLOCK, too_short);
    let tx = make_tx(sub.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::SubscriptionDurationTooShort {
                required: MIN_SUBSCRIPTION_DURATION,
                given,
            } if given == too_short
        ),
        "got {err:?}"
    );
}

#[test]
fn subscription_create_end_height_overflow_rejected() {
    // u64::MAX * u64::MAX fits in u128 (just under u128::MAX), so the
    // total_locked multiplication itself cannot overflow with v1's
    // u64-bounded fields; SubscriptionRateOverflow is forward-compat
    // cover for widening either operand. The path that DOES surface in
    // v1 is `end_height = current_height + duration_blocks` overflowing
    // u64. This test pins that behaviour: a duration of u64::MAX from any
    // non-zero current_height returns the generic ExecError::Overflow.
    let mut db = MemKv::new();
    let mut sub = build_entity([0x11u8; 32], [0x21u8; 32], subscription_caps());
    sub.economic_balance = u128::MAX;
    store_entity(&mut db, &sub);
    let prod = make_producer(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let payload = build_create_payload(sub.id, prod.id, 1, u64::MAX);
    let tx = make_tx(sub.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::Overflow),
        "expected Overflow from end_height arithmetic, got {err:?}"
    );
}

#[test]
fn subscription_create_max_per_entity_enforced() {
    let mut db = MemKv::new();
    let sub = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let prod = make_producer(&mut db, [0x12u8; 32], [0x22u8; 32]);

    // Create exactly MAX_SUBSCRIPTIONS_PER_ENTITY records. Each carries a
    // distinct rate so MemoryObject::compute_id (which folds in the data
    // hash) yields a different object_id and the by-type index gets a
    // distinct entry every iteration.
    for i in 0..MAX_SUBSCRIPTIONS_PER_ENTITY {
        let rate = RATE_PER_BLOCK + u64::from(i);
        let payload = build_create_payload(sub.id, prod.id, rate, DURATION_BLOCKS);
        let tx = make_tx(sub.id, u64::from(i), SIGNAL_FEE, payload);
        apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT + u64::from(i))
            .unwrap_or_else(|e| panic!("create #{i} should succeed, got {e:?}"));
    }

    // The next attempt must hit the cap.
    let payload = build_create_payload(
        sub.id,
        prod.id,
        RATE_PER_BLOCK + u64::from(MAX_SUBSCRIPTIONS_PER_ENTITY),
        DURATION_BLOCKS,
    );
    let tx = make_tx(
        sub.id,
        u64::from(MAX_SUBSCRIPTIONS_PER_ENTITY),
        SIGNAL_FEE,
        payload,
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT + 1_000_000).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::SubscriptionLimitExceeded {
                current,
                max: MAX_SUBSCRIPTIONS_PER_ENTITY,
            } if current == MAX_SUBSCRIPTIONS_PER_ENTITY
        ),
        "got {err:?}"
    );
}

// ============================================================================
// 3. Cancel happy path
// ============================================================================

#[test]
fn subscription_cancel_settles_accrued_to_producer() {
    let mut db = MemKv::new();
    let sub = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let prod = make_producer(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let payload = build_create_payload(sub.id, prod.id, RATE_PER_BLOCK, DURATION_BLOCKS);
    let tx = make_tx(sub.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT).expect("create succeeds");

    let sub_id = single_subscription_id(&db, &sub.id);
    // Cancel after 1_000 of the 10_000 blocks have elapsed.
    let cancel_height = CREATE_HEIGHT + 1_000;
    let cancel_payload = build_cancel_payload(sub.id, sub_id);
    let cancel_tx = make_tx(sub.id, 1, SIGNAL_FEE, cancel_payload);
    apply_signal_commitment_tx(&mut db, &cancel_tx, cancel_height).expect("cancel succeeds");

    let prod_after = read_ai_entity(&db, &prod.id).unwrap().unwrap();
    let accrued_gross = u128::from(1_000u64) * u128::from(RATE_PER_BLOCK);
    let accrued_fee = accrued_gross * MARKETPLACE_FEE_BPS / BPS_DENOMINATOR;
    let accrued_net = accrued_gross - accrued_fee;
    let total_locked = u128::from(RATE_PER_BLOCK) * u128::from(DURATION_BLOCKS);
    let remaining = total_locked - accrued_gross;
    let cancel_fee = remaining * SUBSCRIPTION_CANCEL_FEE_BPS / BPS_DENOMINATOR;
    assert_eq!(
        prod_after.economic_balance,
        PRODUCER_BALANCE + accrued_net + cancel_fee,
        "producer credited with accrued_net + cancel_fee"
    );
}

#[test]
fn subscription_cancel_refunds_subscriber() {
    let mut db = MemKv::new();
    let sub = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let prod = make_producer(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let payload = build_create_payload(sub.id, prod.id, RATE_PER_BLOCK, DURATION_BLOCKS);
    let tx = make_tx(sub.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT).expect("create succeeds");

    let sub_id = single_subscription_id(&db, &sub.id);
    let cancel_height = CREATE_HEIGHT + 1_000;
    let cancel_payload = build_cancel_payload(sub.id, sub_id);
    let cancel_tx = make_tx(sub.id, 1, SIGNAL_FEE, cancel_payload);
    apply_signal_commitment_tx(&mut db, &cancel_tx, cancel_height).expect("cancel succeeds");

    let sub_after = read_ai_entity(&db, &sub.id).unwrap().unwrap();
    let total_locked = u128::from(RATE_PER_BLOCK) * u128::from(DURATION_BLOCKS);
    let accrued_gross = u128::from(1_000u64) * u128::from(RATE_PER_BLOCK);
    let remaining = total_locked - accrued_gross;
    let cancel_fee = remaining * SUBSCRIPTION_CANCEL_FEE_BPS / BPS_DENOMINATOR;
    let refund = remaining - cancel_fee;
    assert_eq!(
        sub_after.economic_balance,
        SUBSCRIBER_BALANCE - total_locked - 2 * u128::from(SIGNAL_FEE) + refund,
        "subscriber refunded remaining minus cancel fee"
    );
}

#[test]
fn subscription_cancel_marks_record_inactive_in_place() {
    let mut db = MemKv::new();
    let sub = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let prod = make_producer(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let payload = build_create_payload(sub.id, prod.id, RATE_PER_BLOCK, DURATION_BLOCKS);
    let tx = make_tx(sub.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT).expect("create");

    let sub_id = single_subscription_id(&db, &sub.id);
    let cancel_height = CREATE_HEIGHT + 1_000;
    let cancel_tx = make_tx(sub.id, 1, SIGNAL_FEE, build_cancel_payload(sub.id, sub_id));
    apply_signal_commitment_tx(&mut db, &cancel_tx, cancel_height).expect("cancel");

    let after = read_subscription(&db, &sub.id, &sub_id);
    assert!(!after.is_active, "is_active flipped to false");
    assert_eq!(after.last_settled_height, cancel_height);
    // The record stays under the same id (cancellation is in-place).
    assert_eq!(single_subscription_id(&db, &sub.id), sub_id);
}

#[test]
fn subscription_marketplace_fee_credited_to_treasury() {
    let mut db = MemKv::new();
    let sub = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let prod = make_producer(&mut db, [0x12u8; 32], [0x22u8; 32]);

    assert_eq!(read_treasury(&db), 0, "treasury starts empty");

    let payload = build_create_payload(sub.id, prod.id, RATE_PER_BLOCK, DURATION_BLOCKS);
    let tx = make_tx(sub.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT).expect("create");

    let sub_id = single_subscription_id(&db, &sub.id);
    let cancel_height = CREATE_HEIGHT + 5_000;
    let cancel_tx = make_tx(sub.id, 1, SIGNAL_FEE, build_cancel_payload(sub.id, sub_id));
    apply_signal_commitment_tx(&mut db, &cancel_tx, cancel_height).expect("cancel");

    let accrued_gross = u128::from(5_000u64) * u128::from(RATE_PER_BLOCK);
    let accrued_fee = accrued_gross * MARKETPLACE_FEE_BPS / BPS_DENOMINATOR;
    assert_eq!(
        read_treasury(&db),
        accrued_fee,
        "treasury holds the 2pct accrued fee"
    );
}

// ============================================================================
// 4. Cancel-side rejections
// ============================================================================

#[test]
fn subscription_cancel_only_by_subscriber_rejected() {
    let mut db = MemKv::new();
    let sub = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let prod = make_producer(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let other = make_subscriber(&mut db, [0x13u8; 32], [0x23u8; 32]);

    let payload = build_create_payload(sub.id, prod.id, RATE_PER_BLOCK, DURATION_BLOCKS);
    let tx = make_tx(sub.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT).expect("create");

    let sub_id = single_subscription_id(&db, &sub.id);
    // `other` tries to cancel sub's subscription. The record is keyed under
    // sub.id, so `other` queries an empty namespace and the handler reports
    // SubscriptionNotFound (the generic ownership-failure surface).
    let cancel_payload = build_cancel_payload(other.id, sub_id);
    let cancel_tx = make_tx(other.id, 0, SIGNAL_FEE, cancel_payload);
    let err = apply_signal_commitment_tx(&mut db, &cancel_tx, CREATE_HEIGHT + 100).unwrap_err();
    assert!(
        matches!(err, ExecError::SubscriptionNotFound),
        "got {err:?}"
    );
}

#[test]
fn subscription_cancel_already_cancelled_rejected() {
    let mut db = MemKv::new();
    let sub = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let prod = make_producer(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let payload = build_create_payload(sub.id, prod.id, RATE_PER_BLOCK, DURATION_BLOCKS);
    let tx = make_tx(sub.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT).expect("create");

    let sub_id = single_subscription_id(&db, &sub.id);
    let cancel_payload = build_cancel_payload(sub.id, sub_id);
    let cancel_tx = make_tx(sub.id, 1, SIGNAL_FEE, cancel_payload.clone());
    apply_signal_commitment_tx(&mut db, &cancel_tx, CREATE_HEIGHT + 100).expect("first cancel");

    let cancel_tx2 = make_tx(sub.id, 2, SIGNAL_FEE, cancel_payload);
    let err = apply_signal_commitment_tx(&mut db, &cancel_tx2, CREATE_HEIGHT + 200).unwrap_err();
    assert!(
        matches!(err, ExecError::SubscriptionNotActive),
        "got {err:?}"
    );
}

#[test]
fn subscription_cancel_at_or_after_end_height_full_settle() {
    let mut db = MemKv::new();
    let sub = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let prod = make_producer(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let payload = build_create_payload(sub.id, prod.id, RATE_PER_BLOCK, DURATION_BLOCKS);
    let tx = make_tx(sub.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT).expect("create");

    let sub_id = single_subscription_id(&db, &sub.id);
    let cancel_payload = build_cancel_payload(sub.id, sub_id);
    let cancel_tx = make_tx(sub.id, 1, SIGNAL_FEE, cancel_payload);
    let very_late = CREATE_HEIGHT + DURATION_BLOCKS + 5_000;
    apply_signal_commitment_tx(&mut db, &cancel_tx, very_late).expect("cancel succeeds");

    let total_locked = u128::from(RATE_PER_BLOCK) * u128::from(DURATION_BLOCKS);
    let accrued_fee = total_locked * MARKETPLACE_FEE_BPS / BPS_DENOMINATOR;
    let accrued_net = total_locked - accrued_fee;

    let prod_after = read_ai_entity(&db, &prod.id).unwrap().unwrap();
    let sub_after = read_ai_entity(&db, &sub.id).unwrap().unwrap();
    // No remaining funds: cancel_fee = 0, refund = 0. Producer gets the full
    // accrued_net (with the 2pct marketplace cut already deducted).
    assert_eq!(prod_after.economic_balance, PRODUCER_BALANCE + accrued_net);
    assert_eq!(
        sub_after.economic_balance,
        SUBSCRIBER_BALANCE - total_locked - 2 * u128::from(SIGNAL_FEE),
        "subscriber gets no refund when cancelling after end_height"
    );
    let after = read_subscription(&db, &sub.id, &sub_id);
    assert_eq!(after.last_settled_height, after.end_height);
    assert!(!after.is_active);
}

#[test]
fn subscription_cancel_unknown_id_rejected() {
    let mut db = MemKv::new();
    let sub = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let bogus = [0xEEu8; 32];
    let cancel_payload = build_cancel_payload(sub.id, bogus);
    let cancel_tx = make_tx(sub.id, 0, SIGNAL_FEE, cancel_payload);
    let err = apply_signal_commitment_tx(&mut db, &cancel_tx, CREATE_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::SubscriptionNotFound),
        "got {err:?}"
    );
}

// ============================================================================
// 5. Regression: unrelated signals still work
// ============================================================================

#[test]
fn non_subscription_signals_still_work() {
    // Anomaly is the simplest base-tail signal type; if the codec / handler
    // dispatch were broken by the Phase 4 additions this would be the first
    // test to fail.
    let mut db = MemKv::new();
    let issuer = make_subscriber(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xA1u8; 32],
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
    let tx = make_tx(issuer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, CREATE_HEIGHT)
        .expect("Anomaly signal still works after subscription codec additions");
}
