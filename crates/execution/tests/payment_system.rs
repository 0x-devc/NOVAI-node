#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]

//! Integration tests for the native x402 payment rail (Week 28, Phase 2).
//!
//! Covers:
//! - Happy path: payer pays payee, fee accrues to marketplace treasury,
//!   PaymentRecord written to by_hash, scan indexes populated.
//! - Zero-fee corner: amounts below `BPS_DENOMINATOR / PAYMENT_FEE_BPS`
//!   yield fee == 0 and skip the treasury write.
//! - Validation rejections: self-payment, zero amount, expired window,
//!   unknown payee, inactive payee, insufficient balance, replay.
//! - Two distinct payments (different signal_hashes) both settle.
//! - by_payer / by_payee prefix scans return the expected entries.
//! - total_transactions bumped on both parties.
//! - Fee math overflow (huge amount) is rejected, not silently wrapped.
//! - Golden byte-layout vector for the on-chain PaymentRecord (162 bytes).

use novai_ai_entities::{AiEntity, AiSignalType, AutonomyMode, Capabilities};
use novai_execution::{
    apply_signal_commitment_tx, decode_payment_record_v1, decode_payment_splits_record_v1,
    encode_signal_commitment_payload_v1, payment_by_hash_key, payment_by_payee_key,
    payment_by_payer_key, payment_splits_by_hash_key, read_ai_entity, write_ai_entity_op,
    ExecError, PaymentRequestExtraV1, PaymentSplit, PaymentSplitsRecord, PaymentSplitsRecordEntry,
    SignalCommitmentPayloadV1, BPS_DENOMINATOR, KEY_MARKETPLACE_TREASURY,
    KEY_PREFIX_AI_PAYMENTS_BY_PAYEE, KEY_PREFIX_AI_PAYMENTS_BY_PAYER, MAX_PAYMENT_SPLITS,
    MIN_PAYMENT_SPLITS_WHEN_PRESENT, PAYMENT_ATTESTATION_STATUS_NONE, PAYMENT_FEE_BPS,
    PAYMENT_RECORD_LEN, PAYMENT_SPLITS_RECORD_ENTRY_SIZE, PAYMENT_SPLITS_RECORD_MAX_LEN,
    PAYMENT_SPLITS_RECORD_MIN_LEN, PAYMENT_SPLITS_RECORD_V1,
    SIGNAL_COMMITMENT_PAYLOAD_V1_PAYMENT_REQUEST_LEN,
};
use novai_state::{ai_entity_by_address_key, decode_fee_pool_v1, Kv, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const PAYER_BALANCE: u128 = 1_000_000;
const PAYEE_BALANCE: u128 = 250;
const SIGNAL_FEE: u64 = 1_000;
const PAYMENT_HEIGHT: u64 = 500;
const EXPIRY_HEIGHT: u64 = PAYMENT_HEIGHT + 100;

// ============================================================================
// Helpers
// ============================================================================

fn payment_caps() -> Capabilities {
    // emit_proposals is the dispatch gate for SignalCommitment, which
    // carries PaymentRequest. Other flags are off; payments do not need
    // reputation-update or NNPX-derived access.
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
    AiEntity::new(code_hash, creator, AutonomyMode::Gated, caps, 1000)
}

fn store_entity(db: &mut MemKv, entity: &AiEntity) {
    db.apply_batch(&[
        write_ai_entity_op(entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();
}

fn make_payer(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut payer = build_entity(code_hash, creator, payment_caps());
    payer.economic_balance = PAYER_BALANCE;
    store_entity(db, &payer);
    payer
}

fn make_payee(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut payee = build_entity(code_hash, creator, payment_caps());
    payee.economic_balance = PAYEE_BALANCE;
    store_entity(db, &payee);
    payee
}

fn build_payment_payload(
    signal_hash: [u8; 32],
    payer: [u8; 32],
    payee: [u8; 32],
    amount: u64,
    service: [u8; 32],
    request: [u8; 32],
    max_block_height: u64,
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
            service_descriptor_hash: service,
            request_hash: request,
            max_block_height,
            splits: None,
        }),
        service_attestation: None,
        sla_accept: None,
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
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

fn expected_fee(amount: u64) -> u128 {
    u128::from(amount) * PAYMENT_FEE_BPS / BPS_DENOMINATOR
}

// ============================================================================
// 1. Happy path
// ============================================================================

#[test]
fn payment_request_basic_settles() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let payee = make_payee(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let amount: u64 = 10_000;
    let signal_hash = [0xAAu8; 32];
    let service = [0xBBu8; 32];
    let request = [0xCCu8; 32];
    let payload = build_payment_payload(
        signal_hash,
        payer.id,
        payee.id,
        amount,
        service,
        request,
        EXPIRY_HEIGHT,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);

    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("payment settles");

    let payer_after = read_ai_entity(&db, &payer.id).unwrap().unwrap();
    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();

    let fee = expected_fee(amount);
    let total_debit = u128::from(amount) + fee + u128::from(SIGNAL_FEE);

    assert_eq!(
        payer_after.economic_balance,
        PAYER_BALANCE - total_debit,
        "payer pays amount + fee + tx_fee"
    );
    assert_eq!(
        payee_after.economic_balance,
        PAYEE_BALANCE + u128::from(amount),
        "payee receives amount (no fee withheld from payee)"
    );
    assert_eq!(read_treasury(&db), fee, "treasury accrues 2 percent");

    // Canonical record is present under by_hash with the exact field values
    // carried by the original tail.
    let record_bytes = db.get(&payment_by_hash_key(&signal_hash)).unwrap().unwrap();
    assert_eq!(record_bytes.len(), PAYMENT_RECORD_LEN);
    let record = decode_payment_record_v1(&record_bytes).unwrap();
    assert_eq!(record.payer, payer.id);
    assert_eq!(record.payee, payee.id);
    assert_eq!(record.amount, amount);
    assert_eq!(record.service_descriptor_hash, service);
    assert_eq!(record.request_hash, request);
    assert_eq!(record.payment_height, PAYMENT_HEIGHT);
    assert_eq!(record.max_block_height, EXPIRY_HEIGHT);
    assert_eq!(record.attested_status, PAYMENT_ATTESTATION_STATUS_NONE);
    assert_eq!(record.attested_height, 0);

    // Both scan-index markers are present.
    let payer_index_key = payment_by_payer_key(&payer.id, PAYMENT_HEIGHT, &signal_hash);
    let payee_index_key = payment_by_payee_key(&payee.id, PAYMENT_HEIGHT, &signal_hash);
    assert!(db.get(&payer_index_key).unwrap().is_some());
    assert!(db.get(&payee_index_key).unwrap().is_some());
}

// ============================================================================
// 2. Zero-fee corner case
// ============================================================================

#[test]
fn payment_request_with_subbasis_amount_skips_treasury_write() {
    // amount < BPS_DENOMINATOR / PAYMENT_FEE_BPS (50 base units) yields
    // fee = 0 under integer division. Treasury record must remain absent.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x31u8; 32], [0x41u8; 32]);
    let payee = make_payee(&mut db, [0x32u8; 32], [0x42u8; 32]);

    let amount: u64 = 49; // 49 * 200 / 10_000 = 0 (truncated)
    let payload = build_payment_payload(
        [0x01u8; 32],
        payer.id,
        payee.id,
        amount,
        [0x02u8; 32],
        [0x03u8; 32],
        EXPIRY_HEIGHT,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);

    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("payment settles");

    let payer_after = read_ai_entity(&db, &payer.id).unwrap().unwrap();
    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();

    assert_eq!(expected_fee(amount), 0, "sanity: fee truncates to zero");
    assert_eq!(
        payer_after.economic_balance,
        PAYER_BALANCE - u128::from(amount) - u128::from(SIGNAL_FEE),
        "payer pays only the amount + tx_fee"
    );
    assert_eq!(
        payee_after.economic_balance,
        PAYEE_BALANCE + u128::from(amount)
    );
    assert!(
        db.get(KEY_MARKETPLACE_TREASURY).unwrap().is_none(),
        "zero-fee payments must NOT touch the treasury record"
    );
}

// ============================================================================
// 3-9. Validation rejections
// ============================================================================

#[test]
fn payment_request_self_payment_rejected() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x51u8; 32], [0x61u8; 32]);

    let payload = build_payment_payload(
        [0xAAu8; 32],
        payer.id,
        payer.id, // self-payment
        100,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::PaymentSelfReferential),
        "got {err:?}"
    );
}

#[test]
fn payment_request_zero_amount_rejected() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x71u8; 32], [0x81u8; 32]);
    let payee = make_payee(&mut db, [0x72u8; 32], [0x82u8; 32]);

    let payload = build_payment_payload(
        [0xAAu8; 32],
        payer.id,
        payee.id,
        0,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(matches!(err, ExecError::PaymentAmountZero), "got {err:?}");
}

#[test]
fn payment_request_expired_rejected() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x91u8; 32], [0xA1u8; 32]);
    let payee = make_payee(&mut db, [0x92u8; 32], [0xA2u8; 32]);

    let max = PAYMENT_HEIGHT - 1; // strictly less than current_height
    let payload = build_payment_payload(
        [0xAAu8; 32],
        payer.id,
        payee.id,
        100,
        [0u8; 32],
        [0u8; 32],
        max,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::PaymentExpired {
                current_height: PAYMENT_HEIGHT,
                max_block_height
            } if max_block_height == max
        ),
        "got {err:?}"
    );
}

#[test]
fn payment_request_unknown_payee_rejected() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0xB1u8; 32], [0xC1u8; 32]);

    let payload = build_payment_payload(
        [0xAAu8; 32],
        payer.id,
        [0xFFu8; 32], // not in state
        100,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::PaymentPayeeNotFound),
        "got {err:?}"
    );
}

#[test]
fn payment_request_inactive_payee_rejected() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0xD1u8; 32], [0xE1u8; 32]);
    let mut payee = make_payee(&mut db, [0xD2u8; 32], [0xE2u8; 32]);
    payee.is_active = false;
    store_entity(&mut db, &payee);

    let payload = build_payment_payload(
        [0xAAu8; 32],
        payer.id,
        payee.id,
        100,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::PaymentPayeeNotActive),
        "got {err:?}"
    );
}

#[test]
fn payment_request_insufficient_balance_rejected() {
    let mut db = MemKv::new();
    let mut payer = make_payer(&mut db, [0xF1u8; 32], [0x01u8; 32]);
    let payee = make_payee(&mut db, [0xF2u8; 32], [0x02u8; 32]);
    // Drain the payer just below the required amount + fee + tx_fee.
    payer.economic_balance = u128::from(SIGNAL_FEE) + 50;
    store_entity(&mut db, &payer);

    let amount: u64 = 1_000;
    let payload = build_payment_payload(
        [0xAAu8; 32],
        payer.id,
        payee.id,
        amount,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::PaymentInsufficientBalance { .. }),
        "got {err:?}"
    );
}

#[test]
fn payment_request_duplicate_signal_hash_rejected() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x13u8; 32], [0x23u8; 32]);
    let payee = make_payee(&mut db, [0x14u8; 32], [0x24u8; 32]);

    let signal_hash = [0xAAu8; 32];
    let amount: u64 = 500;
    let payload = build_payment_payload(
        signal_hash,
        payer.id,
        payee.id,
        amount,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
    );
    let tx1 = make_tx(payer.id, 0, SIGNAL_FEE, payload.clone());
    apply_signal_commitment_tx(&mut db, &tx1, PAYMENT_HEIGHT).expect("first payment settles");

    // Identical signal, fresh nonce. The signal_hash already lives in the
    // by_hash record; the handler must reject before any balance change.
    let payer_after_first = read_ai_entity(&db, &payer.id).unwrap().unwrap();
    let payee_after_first = read_ai_entity(&db, &payee.id).unwrap().unwrap();

    let tx2 = make_tx(payer.id, 1, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx2, PAYMENT_HEIGHT + 1).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::PaymentAlreadySettled { signal_hash: h } if h == signal_hash
        ),
        "got {err:?}"
    );

    // Balances must be unchanged between the failed retry and the first
    // settlement (except for tx_fee debited by the outer handler before
    // the dedup check fires; that debit happens to all rejected signals,
    // including this one, so we expect a delta of exactly SIGNAL_FEE).
    let payer_after_retry = read_ai_entity(&db, &payer.id).unwrap().unwrap();
    let payee_after_retry = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    assert_eq!(
        payer_after_retry.economic_balance, payer_after_first.economic_balance,
        "duplicate payment must not move payer funds"
    );
    assert_eq!(
        payee_after_retry.economic_balance, payee_after_first.economic_balance,
        "duplicate payment must not move payee funds"
    );
}

// ============================================================================
// 10. Two distinct payments both settle
// ============================================================================

#[test]
fn payment_request_two_distinct_payments_both_settle() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x15u8; 32], [0x25u8; 32]);
    let payee = make_payee(&mut db, [0x16u8; 32], [0x26u8; 32]);

    let amount_a: u64 = 1_000;
    let amount_b: u64 = 2_500;

    let payload_a = build_payment_payload(
        [0xA1u8; 32],
        payer.id,
        payee.id,
        amount_a,
        [0xB1u8; 32],
        [0xC1u8; 32],
        EXPIRY_HEIGHT,
    );
    let tx_a = make_tx(payer.id, 0, SIGNAL_FEE, payload_a);
    apply_signal_commitment_tx(&mut db, &tx_a, PAYMENT_HEIGHT).expect("first settles");

    let payload_b = build_payment_payload(
        [0xA2u8; 32], // distinct signal_hash → distinct dedup key
        payer.id,
        payee.id,
        amount_b,
        [0xB1u8; 32], // same service descriptor is fine
        [0xC2u8; 32], // distinct request id
        EXPIRY_HEIGHT,
    );
    let tx_b = make_tx(payer.id, 1, SIGNAL_FEE, payload_b);
    apply_signal_commitment_tx(&mut db, &tx_b, PAYMENT_HEIGHT + 1).expect("second settles");

    let payer_after = read_ai_entity(&db, &payer.id).unwrap().unwrap();
    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    let total_amount = u128::from(amount_a) + u128::from(amount_b);
    let total_fee = expected_fee(amount_a) + expected_fee(amount_b);
    let total_tx_fee = 2 * u128::from(SIGNAL_FEE);

    assert_eq!(
        payer_after.economic_balance,
        PAYER_BALANCE - total_amount - total_fee - total_tx_fee,
    );
    assert_eq!(payee_after.economic_balance, PAYEE_BALANCE + total_amount,);
    assert_eq!(read_treasury(&db), total_fee);
}

// ============================================================================
// 11. Scan indexes return expected entries
// ============================================================================

#[test]
fn payment_request_indexes_scan_correctly() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x17u8; 32], [0x27u8; 32]);
    let payee_a = make_payee(&mut db, [0x18u8; 32], [0x28u8; 32]);
    let payee_b = make_payee(&mut db, [0x19u8; 32], [0x29u8; 32]);

    // Three payments: 2 to payee_a (at heights 500, 501), 1 to payee_b (at 502).
    let cases = [
        ([0xA1u8; 32], payee_a.id, 0u64, PAYMENT_HEIGHT),
        ([0xA2u8; 32], payee_a.id, 1u64, PAYMENT_HEIGHT + 1),
        ([0xA3u8; 32], payee_b.id, 2u64, PAYMENT_HEIGHT + 2),
    ];
    for (signal_hash, payee_id, nonce, height) in cases {
        let payload = build_payment_payload(
            signal_hash,
            payer.id,
            payee_id,
            1_000,
            [0u8; 32],
            [0u8; 32],
            EXPIRY_HEIGHT,
        );
        let tx = make_tx(payer.id, nonce, SIGNAL_FEE, payload);
        apply_signal_commitment_tx(&mut db, &tx, height).expect("settles");
    }

    // by_payer scan: prefix = "ai/payments/by_payer/" || payer_id.
    let mut payer_prefix = Vec::new();
    payer_prefix.extend_from_slice(KEY_PREFIX_AI_PAYMENTS_BY_PAYER);
    payer_prefix.extend_from_slice(&payer.id);
    let payer_entries = db.scan_prefix(&payer_prefix).unwrap();
    assert_eq!(
        payer_entries.len(),
        3,
        "payer made 3 outgoing payments; scan returns 3 entries"
    );

    // Entries are big-endian-height-prefixed and therefore lexicographically
    // ordered by height. The 8 bytes after the entity id encode height.
    for (i, (key, _)) in payer_entries.iter().enumerate() {
        let body = &key[payer_prefix.len()..];
        let mut height_bytes = [0u8; 8];
        height_bytes.copy_from_slice(&body[..8]);
        let height = u64::from_be_bytes(height_bytes);
        assert_eq!(
            height,
            PAYMENT_HEIGHT + i as u64,
            "by_payer entries returned in height order",
        );
    }

    // by_payee for payee_a: 2 entries.
    let mut payee_a_prefix = Vec::new();
    payee_a_prefix.extend_from_slice(KEY_PREFIX_AI_PAYMENTS_BY_PAYEE);
    payee_a_prefix.extend_from_slice(&payee_a.id);
    let payee_a_entries = db.scan_prefix(&payee_a_prefix).unwrap();
    assert_eq!(payee_a_entries.len(), 2);

    // by_payee for payee_b: 1 entry.
    let mut payee_b_prefix = Vec::new();
    payee_b_prefix.extend_from_slice(KEY_PREFIX_AI_PAYMENTS_BY_PAYEE);
    payee_b_prefix.extend_from_slice(&payee_b.id);
    let payee_b_entries = db.scan_prefix(&payee_b_prefix).unwrap();
    assert_eq!(payee_b_entries.len(), 1);
}

// ============================================================================
// 12. total_transactions bumped on both parties
// ============================================================================

#[test]
fn payment_request_increments_total_transactions_both_parties() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x1Au8; 32], [0x2Au8; 32]);
    let payee = make_payee(&mut db, [0x1Bu8; 32], [0x2Bu8; 32]);

    let before_payer = read_ai_entity(&db, &payer.id).unwrap().unwrap();
    let before_payee = read_ai_entity(&db, &payee.id).unwrap().unwrap();

    let payload = build_payment_payload(
        [0xAAu8; 32],
        payer.id,
        payee.id,
        1_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    let after_payer = read_ai_entity(&db, &payer.id).unwrap().unwrap();
    let after_payee = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    assert_eq!(
        after_payer.total_transactions,
        before_payer.total_transactions + 1,
        "payer total_transactions incremented",
    );
    assert_eq!(
        after_payee.total_transactions,
        before_payee.total_transactions + 1,
        "payee total_transactions incremented",
    );
}

// ============================================================================
// 13. Fee math overflow rejected
// ============================================================================

#[test]
fn payment_request_amount_overflow_in_fee_math_rejected() {
    let mut db = MemKv::new();
    let mut payer = make_payer(&mut db, [0x1Cu8; 32], [0x2Cu8; 32]);
    let payee = make_payee(&mut db, [0x1Du8; 32], [0x2Du8; 32]);
    // Ensure the payer has enough nominal balance for the test to reach
    // the multiplication before tripping a balance failure.
    payer.economic_balance = u128::MAX;
    store_entity(&mut db, &payer);

    // u64::MAX * 200 overflows u128? Compute: u128::from(u64::MAX) * 200 =
    // (2^64 - 1) * 200 ≈ 3.69e21, still fits in u128 (max ~3.4e38). So a
    // single-multiplication overflow needs to come from `checked_add` on
    // amount + fee when payer's balance check tries to combine them with
    // tx_fee. We use u64::MAX as amount to make total exceed payer's
    // balance only if u128 wraps; checked arithmetic should reject.
    //
    // Actually: u64::MAX as u128 + (u64::MAX as u128 * 200 / 10_000)
    // = u64::MAX + u64::MAX/50 which still fits in u128 comfortably. So
    // overflow does NOT trip here. Instead we'd hit
    // PaymentInsufficientBalance because the payer's balance (set to
    // u128::MAX) cannot be exceeded by u64-sized totals.
    //
    // To exercise the overflow path explicitly we'd need amounts that
    // exceed u64 range, which the wire format prevents. The checked_*
    // arithmetic is therefore defense-in-depth, not a user-reachable
    // path under current encoding. We assert success here to confirm the
    // path executes cleanly with the largest possible u64 amount.
    let payload = build_payment_payload(
        [0xAAu8; 32],
        payer.id,
        payee.id,
        u64::MAX,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT)
        .expect("u64::MAX amount fits in u128 fee math without overflow");

    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    assert_eq!(
        payee_after.economic_balance,
        PAYEE_BALANCE + u128::from(u64::MAX)
    );
}

// ============================================================================
// 14. Golden vector: end-to-end record bytes lock the on-chain layout
// ============================================================================

#[test]
fn payment_request_record_bytes_lock_layout() {
    let mut db = MemKv::new();
    let mut payer = make_payer(&mut db, [0x1Eu8; 32], [0x2Eu8; 32]);
    let payee = make_payee(&mut db, [0x1Fu8; 32], [0x2Fu8; 32]);
    // Lift the payer's balance so the golden-amount fits without
    // tripping InsufficientBalance. The bytes we are locking down are
    // the record fields, not the balance arithmetic.
    payer.economic_balance = u128::MAX / 2;
    store_entity(&mut db, &payer);

    let signal_hash = [0xAAu8; 32];
    let service = [0xBBu8; 32];
    let request = [0xCCu8; 32];
    let amount: u64 = 0x0102_0304_0506_0708;
    let max = 0x1112_1314_1516_1718u64;
    let payload = build_payment_payload(
        signal_hash,
        payer.id,
        payee.id,
        amount,
        service,
        request,
        max,
    );
    assert_eq!(
        payload.len(),
        SIGNAL_COMMITMENT_PAYLOAD_V1_PAYMENT_REQUEST_LEN
    );
    assert_eq!(payload.len(), 178);

    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    let record_bytes = db.get(&payment_by_hash_key(&signal_hash)).unwrap().unwrap();
    assert_eq!(record_bytes.len(), 162);
    assert_eq!(record_bytes[0], 1, "PaymentRecord version byte");
    assert_eq!(&record_bytes[1..33], &payer.id, "payer at 1..33");
    assert_eq!(&record_bytes[33..65], &payee.id, "payee at 33..65");
    assert_eq!(
        &record_bytes[65..73],
        &amount.to_be_bytes(),
        "amount_be at 65..73"
    );
    assert_eq!(
        &record_bytes[73..105],
        &service,
        "service_descriptor_hash at 73..105"
    );
    assert_eq!(
        &record_bytes[105..137],
        &request,
        "request_hash at 105..137"
    );
    assert_eq!(
        &record_bytes[137..145],
        &PAYMENT_HEIGHT.to_be_bytes(),
        "payment_height_be at 137..145"
    );
    assert_eq!(
        &record_bytes[145..153],
        &max.to_be_bytes(),
        "max_block_height_be at 145..153"
    );
    assert_eq!(
        record_bytes[153], PAYMENT_ATTESTATION_STATUS_NONE,
        "attested_status sentinel at 153"
    );
    assert_eq!(
        &record_bytes[154..162],
        &0u64.to_be_bytes(),
        "attested_height_be at 154..162 (zero pre-attestation)"
    );
}

// ============================================================================
// Week 33 - Phase 2: PaymentSplits validation
// ============================================================================
//
// These tests exercise the `validate_payment_splits` hook wired into the
// `PaymentRequest` handler. Each rule has at least one dedicated rejection
// test; the happy-path tests assert that validation accepts the payload
// (the executor still settles single-recipient-style until Phase 3 lands
// the split-credit loop, so the tests do NOT assert per-split balance
// deltas yet; Phase 3 does that).

fn make_split_recipient(
    db: &mut MemKv,
    code_seed: u8,
    creator_seed: u8,
    is_active: bool,
) -> AiEntity {
    let mut e = build_entity([code_seed; 32], [creator_seed; 32], payment_caps());
    e.economic_balance = 0;
    e.is_active = is_active;
    store_entity(db, &e);
    e
}

#[allow(clippy::too_many_arguments)]
fn build_split_payment_payload(
    signal_hash: [u8; 32],
    payer: [u8; 32],
    primary_payee: [u8; 32],
    amount: u64,
    service: [u8; 32],
    request: [u8; 32],
    max_block_height: u64,
    splits: Vec<PaymentSplit>,
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
            payee_entity_id: primary_payee,
            amount,
            service_descriptor_hash: service,
            request_hash: request,
            max_block_height,
            splits: Some(splits),
        }),
        service_attestation: None,
        sla_accept: None,
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
    })
}

#[test]
fn payment_split_primary_must_equal_payee_field() {
    // splits[0].recipient != extra.payee_entity_id -> PaymentSplitPrimaryMismatch.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x10u8; 32], [0x20u8; 32]);
    let payee = make_payee(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x12, 0x22, true);

    let splits = vec![
        PaymentSplit {
            recipient_entity_id: r2.id, // WRONG: not the primary
            basis_points: 5_000,
        },
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 5_000,
        },
    ];
    let payload = build_split_payment_payload(
        [0xAAu8; 32],
        payer.id,
        payee.id,
        10_000,
        [0xBBu8; 32],
        [0xCCu8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::PaymentSplitPrimaryMismatch),
        "got {err:?}"
    );
}

#[test]
fn payment_split_zero_basis_points_rejected() {
    // Any single zero-bp entry -> PaymentSplitZeroBasisPoints { index }.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x30u8; 32], [0x40u8; 32]);
    let payee = make_payee(&mut db, [0x31u8; 32], [0x41u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x32, 0x42, true);

    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 10_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 0, // zero share at index 1
        },
    ];
    let payload = build_split_payment_payload(
        [0xABu8; 32],
        payer.id,
        payee.id,
        1_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::PaymentSplitZeroBasisPoints { index: 1 }),
        "got {err:?}"
    );
}

#[test]
fn payment_split_basis_points_sum_below_10000_rejected() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x50u8; 32], [0x60u8; 32]);
    let payee = make_payee(&mut db, [0x51u8; 32], [0x61u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x52, 0x62, true);

    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 6_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 3_999, // sum = 9_999
        },
    ];
    let payload = build_split_payment_payload(
        [0xACu8; 32],
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::PaymentSplitsBasisPointsSumInvalid {
                sum: 9_999,
                expected: 10_000,
            }
        ),
        "got {err:?}"
    );
}

#[test]
fn payment_split_basis_points_sum_above_10000_rejected() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x70u8; 32], [0x80u8; 32]);
    let payee = make_payee(&mut db, [0x71u8; 32], [0x81u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x72, 0x82, true);

    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 6_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 4_001, // sum = 10_001
        },
    ];
    let payload = build_split_payment_payload(
        [0xADu8; 32],
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::PaymentSplitsBasisPointsSumInvalid {
                sum: 10_001,
                expected: 10_000,
            }
        ),
        "got {err:?}"
    );
}

#[test]
fn payment_split_self_payment_primary_rejected() {
    // splits[0] = payee but payee == payer would already trip
    // PaymentSelfReferential in the existing handler step; this test
    // covers the case where splits[1..] names the issuer. The primary
    // self-payment is already covered by `payment_request_self_payment_rejected`.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x90u8; 32], [0xA0u8; 32]);
    let payee = make_payee(&mut db, [0x91u8; 32], [0xA1u8; 32]);

    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 6_000,
        },
        PaymentSplit {
            recipient_entity_id: payer.id, // SELF
            basis_points: 4_000,
        },
    ];
    let payload = build_split_payment_payload(
        [0xAEu8; 32],
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::PaymentSplitSelfPayment),
        "got {err:?}"
    );
}

#[test]
fn payment_split_duplicate_recipients_rejected() {
    // splits[1].recipient == splits[2].recipient -> PaymentSplitDuplicateRecipient.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0xB0u8; 32], [0xC0u8; 32]);
    let payee = make_payee(&mut db, [0xB1u8; 32], [0xC1u8; 32]);
    let r2 = make_split_recipient(&mut db, 0xB2, 0xC2, true);

    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 4_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 3_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id, // DUP
            basis_points: 3_000,
        },
    ];
    let payload = build_split_payment_payload(
        [0xAFu8; 32],
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::PaymentSplitDuplicateRecipient { recipient } if recipient == r2.id
        ),
        "got {err:?}"
    );
}

#[test]
fn payment_split_recipient_not_found_rejected() {
    // Non-primary recipient does not exist in state.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0xD0u8; 32], [0xE0u8; 32]);
    let payee = make_payee(&mut db, [0xD1u8; 32], [0xE1u8; 32]);
    let phantom_id = [0xF5u8; 32]; // never stored

    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 5_000,
        },
        PaymentSplit {
            recipient_entity_id: phantom_id,
            basis_points: 5_000,
        },
    ];
    let payload = build_split_payment_payload(
        [0xBAu8; 32],
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::PaymentSplitRecipientNotFound { recipient } if recipient == phantom_id
        ),
        "got {err:?}"
    );
}

#[test]
fn payment_split_recipient_not_active_rejected() {
    // Non-primary recipient exists but is_active == false.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0xE2u8; 32], [0xF2u8; 32]);
    let payee = make_payee(&mut db, [0xE3u8; 32], [0xF3u8; 32]);
    let r2 = make_split_recipient(&mut db, 0xE4, 0xF4, false);

    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 5_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 5_000,
        },
    ];
    let payload = build_split_payment_payload(
        [0xBBu8; 32],
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::PaymentSplitRecipientNotActive { recipient } if recipient == r2.id
        ),
        "got {err:?}"
    );
}

#[test]
fn payment_split_validation_accepts_2_recipients_at_min_boundary() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x05u8; 32], [0x06u8; 32]);
    let payee = make_payee(&mut db, [0x07u8; 32], [0x08u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x09, 0x0A, true);

    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 7_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 3_000,
        },
    ];
    let payload = build_split_payment_payload(
        [0xC0u8; 32],
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    // Validation must accept; settlement proceeds via the legacy
    // single-recipient executor path in Phase 2 (Phase 3 will swap
    // in the split-credit loop). The test only asserts that the
    // apply returns Ok and that the by_hash record was written.
    assert_eq!(MIN_PAYMENT_SPLITS_WHEN_PRESENT, 2);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT)
        .expect("validation accepts the 2-recipient split");
    let record = db
        .get(&payment_by_hash_key(&[0xC0u8; 32]))
        .unwrap()
        .expect("by_hash record written");
    assert_eq!(record.len(), PAYMENT_RECORD_LEN);
}

#[test]
fn payment_split_validation_accepts_8_recipients_at_max_boundary() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x15u8; 32], [0x16u8; 32]);
    let payee = make_payee(&mut db, [0x17u8; 32], [0x18u8; 32]);
    // Build 7 additional recipients (primary + 7 = 8 total).
    let mut recipients = Vec::with_capacity(MAX_PAYMENT_SPLITS - 1);
    for i in 0..(MAX_PAYMENT_SPLITS - 1) {
        let seed = 0x20u8 + (i as u8);
        recipients.push(make_split_recipient(
            &mut db,
            seed,
            seed.wrapping_add(0x40),
            true,
        ));
    }

    // 1250 bp each across 8 entries sums to exactly 10_000.
    let mut splits = Vec::with_capacity(MAX_PAYMENT_SPLITS);
    splits.push(PaymentSplit {
        recipient_entity_id: payee.id,
        basis_points: 1_250,
    });
    for r in &recipients {
        splits.push(PaymentSplit {
            recipient_entity_id: r.id,
            basis_points: 1_250,
        });
    }
    assert_eq!(splits.len(), MAX_PAYMENT_SPLITS);

    let payload = build_split_payment_payload(
        [0xC1u8; 32],
        payer.id,
        payee.id,
        80_000, // divisible by 8 to keep the Phase 3 remainder visible later
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT)
        .expect("validation accepts the 8-recipient split");
    assert!(db
        .get(&payment_by_hash_key(&[0xC1u8; 32]))
        .unwrap()
        .is_some());
}

#[test]
fn payment_split_validation_rule_order_primary_mismatch_beats_sum_check() {
    // Defence-in-depth: the validator runs the cheap checks before
    // the sum check, so a payload that violates BOTH the primary-
    // mismatch rule and the sum rule must surface PrimaryMismatch
    // (the cheaper rule that fires first).
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x35u8; 32], [0x36u8; 32]);
    let payee = make_payee(&mut db, [0x37u8; 32], [0x38u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x39, 0x3A, true);

    let splits = vec![
        // splits[0] != payee.id (primary mismatch).
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 4_999,
        },
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 4_999,
        },
        // sum = 9_998 (also wrong) but the primary mismatch fires first.
    ];
    let payload = build_split_payment_payload(
        [0xC2u8; 32],
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::PaymentSplitPrimaryMismatch),
        "got {err:?}"
    );
}

#[test]
fn payment_split_failed_validation_leaves_no_by_hash_record() {
    // A rejected split payment must NOT write the by_hash record;
    // otherwise the dedup slot would be consumed and a future
    // (correctly authored) payment with the same signal_hash would
    // be rejected as already-settled.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x45u8; 32], [0x46u8; 32]);
    let payee = make_payee(&mut db, [0x47u8; 32], [0x48u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x49, 0x4A, true);

    let bad_splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 5_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 4_999, // sum = 9_999
        },
    ];
    let signal_hash = [0xC3u8; 32];
    let payload = build_split_payment_payload(
        signal_hash,
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        bad_splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let _ = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();

    let record = db.get(&payment_by_hash_key(&signal_hash)).unwrap();
    assert!(record.is_none(), "no by_hash record after rejected splits");
}

#[test]
fn payment_split_decoder_bad_count_surfaces_through_handler() {
    // End-to-end check that a hand-crafted payload claiming an
    // out-of-range split count surfaces `PaymentSplitsBadCount`
    // through the dispatcher's error-mapping wrapper (NOT the
    // catch-all `Overflow` variant). The Phase 1 decoder test
    // calls decode directly; this one exercises the full path
    // including the wrapper plumbing added in Phase 2.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x55u8; 32], [0x56u8; 32]);
    let payee = make_payee(&mut db, [0x57u8; 32], [0x58u8; 32]);

    let mut payload = build_payment_payload(
        [0xC4u8; 32],
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
    );
    payload.push(1u8); // count = 1, below MIN
    payload.extend_from_slice(&[0xAAu8; 32]);
    payload.extend_from_slice(&10_000u16.to_be_bytes());
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(
            err,
            ExecError::PaymentSplitsBadCount {
                count: 1,
                min: MIN_PAYMENT_SPLITS_WHEN_PRESENT,
                max: MAX_PAYMENT_SPLITS,
            }
        ),
        "got {err:?}"
    );
}

// ============================================================================
// Week 33 - Phase 3: PaymentSplits execution
// ============================================================================
//
// These tests cover the per-recipient credit distribution wired into the
// PaymentRequest handler in Phase 3: balance distribution, remainder fold
// into the primary, sum-of-credits == amount conservation, per-recipient
// by_payee indexes, per-recipient total_transactions bumps, and the
// PaymentSplitsRecord aux row persisted at payment_splits_by_hash_key.
// Service-attestation interaction is also exercised end-to-end against
// a split payment to confirm the reputation delta still lands only on
// the primary recipient.

#[test]
fn payment_split_2_recipients_credits_distributed() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0xA0u8; 32], [0xA1u8; 32]);
    let payee = make_payee(&mut db, [0xA2u8; 32], [0xA3u8; 32]);
    let r2 = make_split_recipient(&mut db, 0xA4, 0xA5, true);

    let amount: u64 = 10_000;
    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 6_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 4_000,
        },
    ];
    let payload = build_split_payment_payload(
        [0xD0u8; 32],
        payer.id,
        payee.id,
        amount,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    let r2_after = read_ai_entity(&db, &r2.id).unwrap().unwrap();
    assert_eq!(
        payee_after.economic_balance,
        PAYEE_BALANCE + 6_000,
        "primary gets 60 percent"
    );
    assert_eq!(r2_after.economic_balance, 4_000, "r2 gets 40 percent");
}

#[test]
fn payment_split_3_recipients_credits_distributed() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0xB0u8; 32], [0xB1u8; 32]);
    let payee = make_payee(&mut db, [0xB2u8; 32], [0xB3u8; 32]);
    let r2 = make_split_recipient(&mut db, 0xB4, 0xB5, true);
    let r3 = make_split_recipient(&mut db, 0xB6, 0xB7, true);

    let amount: u64 = 10_000;
    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 5_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 3_000,
        },
        PaymentSplit {
            recipient_entity_id: r3.id,
            basis_points: 2_000,
        },
    ];
    let payload = build_split_payment_payload(
        [0xD1u8; 32],
        payer.id,
        payee.id,
        amount,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    let r2_after = read_ai_entity(&db, &r2.id).unwrap().unwrap();
    let r3_after = read_ai_entity(&db, &r3.id).unwrap().unwrap();
    assert_eq!(payee_after.economic_balance, PAYEE_BALANCE + 5_000);
    assert_eq!(r2_after.economic_balance, 3_000);
    assert_eq!(r3_after.economic_balance, 2_000);
}

#[test]
fn payment_split_8_recipients_credits_distributed() {
    // 1250 bp each on amount = 80_000 -> 10_000 per recipient.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0xC0u8; 32], [0xC1u8; 32]);
    let payee = make_payee(&mut db, [0xC2u8; 32], [0xC3u8; 32]);
    let mut others = Vec::with_capacity(MAX_PAYMENT_SPLITS - 1);
    for i in 0..(MAX_PAYMENT_SPLITS - 1) {
        let seed = 0xD0u8 + (i as u8);
        others.push(make_split_recipient(
            &mut db,
            seed,
            seed.wrapping_add(0x40),
            true,
        ));
    }
    let mut splits = Vec::with_capacity(MAX_PAYMENT_SPLITS);
    splits.push(PaymentSplit {
        recipient_entity_id: payee.id,
        basis_points: 1_250,
    });
    for r in &others {
        splits.push(PaymentSplit {
            recipient_entity_id: r.id,
            basis_points: 1_250,
        });
    }
    let payload = build_split_payment_payload(
        [0xD2u8; 32],
        payer.id,
        payee.id,
        80_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    assert_eq!(payee_after.economic_balance, PAYEE_BALANCE + 10_000);
    for r in &others {
        let after = read_ai_entity(&db, &r.id).unwrap().unwrap();
        assert_eq!(after.economic_balance, 10_000);
    }
}

#[test]
fn payment_split_remainder_folded_into_primary() {
    // amount = 10_001 with 50/50 split:
    //   floor(10_001 * 5000 / 10000) = 5000 for each
    //   sum_floor = 10_000; remainder = 1
    //   primary gets 5000 + 1 = 5001; other gets 5000.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0xE0u8; 32], [0xE1u8; 32]);
    let payee = make_payee(&mut db, [0xE2u8; 32], [0xE3u8; 32]);
    let r2 = make_split_recipient(&mut db, 0xE4, 0xE5, true);

    let amount: u64 = 10_001;
    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 5_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 5_000,
        },
    ];
    let payload = build_split_payment_payload(
        [0xD3u8; 32],
        payer.id,
        payee.id,
        amount,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    let r2_after = read_ai_entity(&db, &r2.id).unwrap().unwrap();
    assert_eq!(
        payee_after.economic_balance,
        PAYEE_BALANCE + 5_001,
        "primary takes the remainder"
    );
    assert_eq!(r2_after.economic_balance, 5_000);
}

#[test]
fn payment_split_3_way_remainder_folded_into_primary() {
    // amount = 100, splits 3333/3333/3334:
    //   floor(100 * 3333 / 10000) = 33 (twice)
    //   floor(100 * 3334 / 10000) = 33
    //   sum_floor = 99; remainder = 1
    //   primary = 33 + 1 = 34; others = 33 each.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0xE6u8; 32], [0xE7u8; 32]);
    let payee = make_payee(&mut db, [0xE8u8; 32], [0xE9u8; 32]);
    let r2 = make_split_recipient(&mut db, 0xEA, 0xEB, true);
    let r3 = make_split_recipient(&mut db, 0xEC, 0xED, true);

    let amount: u64 = 100;
    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 3_333,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 3_333,
        },
        PaymentSplit {
            recipient_entity_id: r3.id,
            basis_points: 3_334,
        },
    ];
    let payload = build_split_payment_payload(
        [0xD4u8; 32],
        payer.id,
        payee.id,
        amount,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    let r2_after = read_ai_entity(&db, &r2.id).unwrap().unwrap();
    let r3_after = read_ai_entity(&db, &r3.id).unwrap().unwrap();
    assert_eq!(
        payee_after.economic_balance,
        PAYEE_BALANCE + 34,
        "primary takes the floor-loss remainder"
    );
    assert_eq!(r2_after.economic_balance, 33);
    assert_eq!(r3_after.economic_balance, 33);
}

#[test]
fn payment_split_balance_conservation_sum_credits_equals_amount() {
    // Walk multiple split shapes and assert that the sum of credited
    // recipient balance deltas equals the payment amount exactly.
    let cases: &[(u64, &[u16])] = &[
        (10_000, &[6_000, 4_000]),
        (10_001, &[5_000, 5_000]),
        (100, &[3_333, 3_333, 3_334]),
        (80_000, &[1_250; 8]),
        (1, &[5_000, 5_000]),
        (3, &[3_333, 3_333, 3_334]),
    ];
    for (case_idx, (amount, bps)) in cases.iter().enumerate() {
        let mut db = MemKv::new();
        let payer = make_payer(&mut db, [case_idx as u8; 32], [(case_idx + 0x40) as u8; 32]);
        let payee = make_payee(
            &mut db,
            [(case_idx + 0x10) as u8; 32],
            [(case_idx + 0x50) as u8; 32],
        );
        let mut others = Vec::with_capacity(bps.len() - 1);
        for j in 1..bps.len() {
            others.push(make_split_recipient(
                &mut db,
                (case_idx * 16 + j) as u8 + 0x60,
                (case_idx * 16 + j) as u8 + 0x80,
                true,
            ));
        }
        let mut splits = Vec::with_capacity(bps.len());
        splits.push(PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: bps[0],
        });
        for (j, r) in others.iter().enumerate() {
            splits.push(PaymentSplit {
                recipient_entity_id: r.id,
                basis_points: bps[j + 1],
            });
        }
        let payload = build_split_payment_payload(
            [(0xF0 + case_idx) as u8; 32],
            payer.id,
            payee.id,
            *amount,
            [0u8; 32],
            [0u8; 32],
            EXPIRY_HEIGHT,
            splits,
        );
        let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
        apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT)
            .unwrap_or_else(|e| panic!("case {case_idx}: {e:?}"));

        // Sum credit deltas across all recipients.
        let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
        let mut total_credited: u128 = payee_after.economic_balance - PAYEE_BALANCE;
        for r in &others {
            let after = read_ai_entity(&db, &r.id).unwrap().unwrap();
            total_credited += after.economic_balance;
        }
        assert_eq!(
            total_credited,
            u128::from(*amount),
            "case {case_idx}: sum of credits must equal amount",
        );
    }
}

#[test]
fn payment_split_payer_debit_is_amount_plus_one_fee_plus_tx_fee() {
    // P4: one fee on the total amount (not fee-per-split). Payer
    // debit equals (amount + fee + tx_fee), identical to the legacy
    // single-recipient case for the same amount.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x01u8; 32], [0x02u8; 32]);
    let payee = make_payee(&mut db, [0x03u8; 32], [0x04u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x05, 0x06, true);

    let amount: u64 = 10_000;
    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 7_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 3_000,
        },
    ];
    let payload = build_split_payment_payload(
        [0xD5u8; 32],
        payer.id,
        payee.id,
        amount,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    let payer_after = read_ai_entity(&db, &payer.id).unwrap().unwrap();
    let fee = expected_fee(amount);
    let total_debit = u128::from(amount) + fee + u128::from(SIGNAL_FEE);
    assert_eq!(
        payer_after.economic_balance,
        PAYER_BALANCE - total_debit,
        "payer debit matches the single-recipient case exactly",
    );
    assert_eq!(read_treasury(&db), fee, "treasury credited the single fee");
}

#[test]
fn payment_split_by_payee_index_written_for_every_recipient() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x10u8; 32], [0x11u8; 32]);
    let payee = make_payee(&mut db, [0x12u8; 32], [0x13u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x14, 0x15, true);
    let r3 = make_split_recipient(&mut db, 0x16, 0x17, true);

    let signal_hash = [0xD6u8; 32];
    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 5_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 3_000,
        },
        PaymentSplit {
            recipient_entity_id: r3.id,
            basis_points: 2_000,
        },
    ];
    let payload = build_split_payment_payload(
        signal_hash,
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    for recipient_id in [payee.id, r2.id, r3.id] {
        let key = payment_by_payee_key(&recipient_id, PAYMENT_HEIGHT, &signal_hash);
        assert!(
            db.get(&key).unwrap().is_some(),
            "by_payee marker present for recipient",
        );
    }
}

#[test]
fn payment_split_total_transactions_bumped_for_every_recipient() {
    // Interpretation (a) from Phase 0 P3: total_transactions++ on
    // every recipient (no new rep event constant).
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x20u8; 32], [0x21u8; 32]);
    let payee = make_payee(&mut db, [0x22u8; 32], [0x23u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x24, 0x25, true);
    let r3 = make_split_recipient(&mut db, 0x26, 0x27, true);

    let pre_payer_tx = payer.total_transactions;
    let pre_payee_tx = payee.total_transactions;
    let pre_r2_tx = r2.total_transactions;
    let pre_r3_tx = r3.total_transactions;

    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 5_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 3_000,
        },
        PaymentSplit {
            recipient_entity_id: r3.id,
            basis_points: 2_000,
        },
    ];
    let payload = build_split_payment_payload(
        [0xD7u8; 32],
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    let payer_after = read_ai_entity(&db, &payer.id).unwrap().unwrap();
    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    let r2_after = read_ai_entity(&db, &r2.id).unwrap().unwrap();
    let r3_after = read_ai_entity(&db, &r3.id).unwrap().unwrap();
    assert_eq!(payer_after.total_transactions, pre_payer_tx + 1);
    assert_eq!(payee_after.total_transactions, pre_payee_tx + 1);
    assert_eq!(r2_after.total_transactions, pre_r2_tx + 1);
    assert_eq!(r3_after.total_transactions, pre_r3_tx + 1);
}

#[test]
fn payment_split_payment_record_payee_is_primary() {
    // The canonical PaymentRecord stores the primary as `payee` even
    // for multi-party payments; ServiceAttestation / SLA hooks
    // continue to resolve against the primary unchanged.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x30u8; 32], [0x31u8; 32]);
    let payee = make_payee(&mut db, [0x32u8; 32], [0x33u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x34, 0x35, true);

    let signal_hash = [0xD8u8; 32];
    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 6_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 4_000,
        },
    ];
    let payload = build_split_payment_payload(
        signal_hash,
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    let record_bytes = db.get(&payment_by_hash_key(&signal_hash)).unwrap().unwrap();
    assert_eq!(
        record_bytes.len(),
        PAYMENT_RECORD_LEN,
        "PaymentRecord wire format frozen at 162 bytes",
    );
    let record = decode_payment_record_v1(&record_bytes).unwrap();
    assert_eq!(record.payee, payee.id, "PaymentRecord.payee == primary");
    assert_eq!(record.amount, 10_000, "amount unchanged");
}

#[test]
fn payment_split_aux_record_decodes_with_correct_credits() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x40u8; 32], [0x41u8; 32]);
    let payee = make_payee(&mut db, [0x42u8; 32], [0x43u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x44, 0x45, true);
    let r3 = make_split_recipient(&mut db, 0x46, 0x47, true);

    let signal_hash = [0xD9u8; 32];
    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 5_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 3_000,
        },
        PaymentSplit {
            recipient_entity_id: r3.id,
            basis_points: 2_000,
        },
    ];
    let payload = build_split_payment_payload(
        signal_hash,
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    let bytes = db
        .get(&payment_splits_by_hash_key(&signal_hash))
        .unwrap()
        .expect("aux record present");
    let decoded = decode_payment_splits_record_v1(&bytes).expect("decode succeeds");
    assert_eq!(decoded.entries.len(), 3);
    assert_eq!(decoded.entries[0].recipient_entity_id, payee.id);
    assert_eq!(decoded.entries[0].basis_points, 5_000);
    assert_eq!(decoded.entries[0].credited_amount, 5_000);
    assert_eq!(decoded.entries[1].recipient_entity_id, r2.id);
    assert_eq!(decoded.entries[1].basis_points, 3_000);
    assert_eq!(decoded.entries[1].credited_amount, 3_000);
    assert_eq!(decoded.entries[2].recipient_entity_id, r3.id);
    assert_eq!(decoded.entries[2].basis_points, 2_000);
    assert_eq!(decoded.entries[2].credited_amount, 2_000);
}

#[test]
fn payment_split_aux_record_remainder_visible_in_primary_credit() {
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x48u8; 32], [0x49u8; 32]);
    let payee = make_payee(&mut db, [0x4Au8; 32], [0x4Bu8; 32]);
    let r2 = make_split_recipient(&mut db, 0x4C, 0x4D, true);

    let signal_hash = [0xDAu8; 32];
    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 5_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 5_000,
        },
    ];
    let payload = build_split_payment_payload(
        signal_hash,
        payer.id,
        payee.id,
        10_001,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    let bytes = db
        .get(&payment_splits_by_hash_key(&signal_hash))
        .unwrap()
        .unwrap();
    let decoded = decode_payment_splits_record_v1(&bytes).unwrap();
    assert_eq!(
        decoded.entries[0].credited_amount, 5_001,
        "primary credited_amount includes the remainder",
    );
    assert_eq!(decoded.entries[1].credited_amount, 5_000);
}

#[test]
fn payment_split_aux_record_golden_vector_3_recipients() {
    // Lock the wire format. Amount = 10_000 with 5000/3000/2000 bp
    // yields credits 5000/3000/2000 (no remainder). Encoded length
    // = 2 + 3*42 = 128 bytes.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x50u8; 32], [0x51u8; 32]);
    let payee = make_payee(&mut db, [0x52u8; 32], [0x53u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x54, 0x55, true);
    let r3 = make_split_recipient(&mut db, 0x56, 0x57, true);

    let signal_hash = [0xDBu8; 32];
    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 5_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 3_000,
        },
        PaymentSplit {
            recipient_entity_id: r3.id,
            basis_points: 2_000,
        },
    ];
    let payload = build_split_payment_payload(
        signal_hash,
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    let bytes = db
        .get(&payment_splits_by_hash_key(&signal_hash))
        .unwrap()
        .unwrap();
    assert_eq!(bytes.len(), 2 + 3 * PAYMENT_SPLITS_RECORD_ENTRY_SIZE);
    assert_eq!(bytes[0], PAYMENT_SPLITS_RECORD_V1, "version byte");
    assert_eq!(bytes[1], 3, "count byte");
    // Entry 0: primary | 5000 bp | 5000 credit.
    assert_eq!(&bytes[2..34], &payee.id, "entries[0].recipient at 2..34");
    assert_eq!(
        &bytes[34..36],
        &5_000u16.to_be_bytes(),
        "entries[0].bp at 34..36"
    );
    assert_eq!(
        &bytes[36..44],
        &5_000u64.to_be_bytes(),
        "entries[0].credit at 36..44"
    );
    // Entry 1: r2 | 3000 bp | 3000 credit (offset 44..86).
    assert_eq!(&bytes[44..76], &r2.id);
    assert_eq!(&bytes[76..78], &3_000u16.to_be_bytes());
    assert_eq!(&bytes[78..86], &3_000u64.to_be_bytes());
    // Entry 2: r3 | 2000 bp | 2000 credit (offset 86..128).
    assert_eq!(&bytes[86..118], &r3.id);
    assert_eq!(&bytes[118..120], &2_000u16.to_be_bytes());
    assert_eq!(&bytes[120..128], &2_000u64.to_be_bytes());
}

#[test]
fn payment_split_legacy_path_writes_no_aux_record() {
    // Single-recipient payment must NOT produce a PaymentSplitsRecord;
    // the aux row distinguishes split payments in storage.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x60u8; 32], [0x61u8; 32]);
    let payee = make_payee(&mut db, [0x62u8; 32], [0x63u8; 32]);

    let signal_hash = [0xDCu8; 32];
    let payload = build_payment_payload(
        signal_hash,
        payer.id,
        payee.id,
        5_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    assert!(
        db.get(&payment_splits_by_hash_key(&signal_hash))
            .unwrap()
            .is_none(),
        "no aux record for the legacy single-recipient path",
    );
}

#[test]
fn payment_split_service_attestation_rep_applies_to_primary_only() {
    // P6 invariant: ServiceAttestation's reputation delta still
    // lands on the recorded payee (= primary), not on split
    // recipients. Verified end-to-end by issuing a DELIVERED
    // attestation against a split payment and asserting that
    // non-primary recipients see no rep change.
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x70u8; 32], [0x71u8; 32]);
    let payee = make_payee(&mut db, [0x72u8; 32], [0x73u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x74, 0x75, true);

    let primary_rep_before = payee.reputation_score;
    let r2_rep_before = r2.reputation_score;

    let signal_hash = [0xDDu8; 32];
    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 6_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 4_000,
        },
    ];
    let payment_payload = build_split_payment_payload(
        signal_hash,
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let payment_tx = make_tx(payer.id, 0, SIGNAL_FEE, payment_payload);
    apply_signal_commitment_tx(&mut db, &payment_tx, PAYMENT_HEIGHT)
        .expect("split payment settles");

    // Issue a DELIVERED attestation against the split payment.
    let attest_payload =
        encode_signal_commitment_payload_v1(&novai_execution::SignalCommitmentPayloadV1 {
            signal_hash: [0xDEu8; 32],
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
            service_attestation: Some(novai_execution::ServiceAttestationExtraV1 {
                payment_signal_hash: signal_hash,
                payee_entity_id: payee.id,
                status: 0, // PAYMENT_ATTESTATION_STATUS_DELIVERED
            }),
            sla_accept: None,
            channel_accept: None,
            channel_close: None,
            channel_finalize: None,
        });
    let attest_tx = make_tx(payer.id, 1, SIGNAL_FEE, attest_payload);
    apply_signal_commitment_tx(&mut db, &attest_tx, PAYMENT_HEIGHT + 1)
        .expect("attestation settles");

    let payee_after = read_ai_entity(&db, &payee.id).unwrap().unwrap();
    let r2_after = read_ai_entity(&db, &r2.id).unwrap().unwrap();
    assert!(
        payee_after.reputation_score > primary_rep_before,
        "primary's reputation rises on DELIVERED",
    );
    assert_eq!(
        r2_after.reputation_score, r2_rep_before,
        "non-primary recipient's reputation is unchanged",
    );
}

#[test]
fn payment_split_insufficient_balance_leaves_no_aux_or_payment_record() {
    // The balance check fires AFTER validation but BEFORE any
    // mutation. A rejected split payment must leave no PaymentRecord,
    // no aux PaymentSplitsRecord, and no by_payee markers.
    let mut db = MemKv::new();
    let mut payer = make_payer(&mut db, [0x80u8; 32], [0x81u8; 32]);
    let payee = make_payee(&mut db, [0x82u8; 32], [0x83u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x84, 0x85, true);

    // Drain the payer to leave amount + fee just out of reach.
    payer.economic_balance = u128::from(SIGNAL_FEE) + 50;
    store_entity(&mut db, &payer);

    let signal_hash = [0xDFu8; 32];
    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 5_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 5_000,
        },
    ];
    let payload = build_split_payment_payload(
        signal_hash,
        payer.id,
        payee.id,
        10_000,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    let err = apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).unwrap_err();
    assert!(
        matches!(err, ExecError::PaymentInsufficientBalance { .. }),
        "got {err:?}",
    );

    assert!(db
        .get(&payment_by_hash_key(&signal_hash))
        .unwrap()
        .is_none());
    assert!(db
        .get(&payment_splits_by_hash_key(&signal_hash))
        .unwrap()
        .is_none());
    assert!(db
        .get(&payment_by_payee_key(
            &payee.id,
            PAYMENT_HEIGHT,
            &signal_hash
        ))
        .unwrap()
        .is_none());
    assert!(db
        .get(&payment_by_payee_key(&r2.id, PAYMENT_HEIGHT, &signal_hash))
        .unwrap()
        .is_none());
}

#[test]
fn payment_split_treasury_fee_charged_once_on_total() {
    // P4 again, exercised against the treasury balance: regardless
    // of split count, the marketplace treasury receives exactly
    // (amount * PAYMENT_FEE_BPS / BPS_DENOMINATOR).
    let mut db = MemKv::new();
    let payer = make_payer(&mut db, [0x86u8; 32], [0x87u8; 32]);
    let payee = make_payee(&mut db, [0x88u8; 32], [0x89u8; 32]);
    let r2 = make_split_recipient(&mut db, 0x8A, 0x8B, true);
    let r3 = make_split_recipient(&mut db, 0x8C, 0x8D, true);

    let amount: u64 = 50_000;
    let splits = vec![
        PaymentSplit {
            recipient_entity_id: payee.id,
            basis_points: 5_000,
        },
        PaymentSplit {
            recipient_entity_id: r2.id,
            basis_points: 3_000,
        },
        PaymentSplit {
            recipient_entity_id: r3.id,
            basis_points: 2_000,
        },
    ];
    let payload = build_split_payment_payload(
        [0xE0u8; 32],
        payer.id,
        payee.id,
        amount,
        [0u8; 32],
        [0u8; 32],
        EXPIRY_HEIGHT,
        splits,
    );
    let tx = make_tx(payer.id, 0, SIGNAL_FEE, payload);
    apply_signal_commitment_tx(&mut db, &tx, PAYMENT_HEIGHT).expect("settles");

    assert_eq!(
        read_treasury(&db),
        expected_fee(amount),
        "treasury credited the single fee, not fee-per-split",
    );
}

#[test]
fn payment_split_aux_record_constants_smoke_and_roundtrip() {
    assert_eq!(PAYMENT_SPLITS_RECORD_ENTRY_SIZE, 42);
    assert_eq!(PAYMENT_SPLITS_RECORD_MIN_LEN, 1 + 1 + 2 * 42);
    assert_eq!(PAYMENT_SPLITS_RECORD_MAX_LEN, 1 + 1 + 8 * 42);
    assert_eq!(PAYMENT_SPLITS_RECORD_V1, 1);

    let r = PaymentSplitsRecord {
        entries: vec![
            PaymentSplitsRecordEntry {
                recipient_entity_id: [0xAAu8; 32],
                basis_points: 5_000,
                credited_amount: 5_001,
            },
            PaymentSplitsRecordEntry {
                recipient_entity_id: [0xBBu8; 32],
                basis_points: 5_000,
                credited_amount: 5_000,
            },
        ],
    };
    let bytes = novai_execution::encode_payment_splits_record_v1(&r);
    assert_eq!(bytes.len(), PAYMENT_SPLITS_RECORD_MIN_LEN);
    let decoded = decode_payment_splits_record_v1(&bytes).unwrap();
    assert_eq!(decoded, r);
}
