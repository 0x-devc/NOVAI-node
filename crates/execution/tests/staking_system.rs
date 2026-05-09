#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::cast_sign_loss)]

//! Integration tests for entity staking and bonding (Feature 3).
//!
//! Covers:
//! - StakeDeposit: balance flow, lock setup, redeposit lock refresh, insufficient funds.
//! - StakeWithdraw: lock enforcement, partial withdraw, over-withdraw rejection.
//! - StakeSlash: balance deduction, treasury credit, saturation, atomic rep update,
//!   capability gate, self-slash prohibition.
//! - Regression: zero-stake entity can still purchase signals (no implicit gating).

use novai_ai_entities::{AiEntity, AiSignalType, AutonomyMode, Capabilities};
use novai_execution::{
    apply_signal_commitment_tx, encode_signal_commitment_payload_v1, read_ai_entity,
    write_ai_entity_op, ExecError, SignalCommitmentPayloadV1, StakeDepositExtraV1,
    StakeSlashExtraV1, StakeWithdrawExtraV1, KEY_SLASH_TREASURY, REP_EVENT_DECAY,
    REP_EVENT_STAKE_SLASH, STAKE_LOCK_PERIOD,
};
use novai_state::{ai_entity_by_address_key, decode_fee_pool_v1, Kv, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const STAKER_BALANCE: u128 = 1_000_000;
const STAKE_AMOUNT: u128 = 100_000;
const SIGNAL_FEE: u64 = 1_000;
const DEPOSIT_HEIGHT: u64 = 100;

// ============================================================================
// Helpers
// ============================================================================

fn staker_caps() -> Capabilities {
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

fn slasher_caps() -> Capabilities {
    Capabilities {
        read_public_chain: true,
        read_memory_objects: false,
        emit_proposals: true,
        request_execution: false,
        read_nnpx_derived: false,
        submit_reputation_updates: true,
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

fn make_staker(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut e = build_entity(code_hash, creator, staker_caps());
    e.economic_balance = STAKER_BALANCE;
    store_entity(db, &e);
    e
}

fn make_slasher(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut e = build_entity(code_hash, creator, slasher_caps());
    e.economic_balance = STAKER_BALANCE;
    store_entity(db, &e);
    e
}

fn read_slash_treasury(db: &MemKv) -> u128 {
    db.get(KEY_SLASH_TREASURY)
        .unwrap()
        .map_or(0, |bytes| decode_fee_pool_v1(&bytes).unwrap().balance)
}

fn build_stake_deposit_payload(issuer: [u8; 32], amount: u128) -> Vec<u8> {
    encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xA1u8; 32],
        signal_type: AiSignalType::StakeDeposit,
        issuer_entity_id: issuer,
        reputation: None,
        purchase: None,
        stake_deposit: Some(StakeDepositExtraV1 { amount }),
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
    })
}

fn build_stake_withdraw_payload(issuer: [u8; 32], amount: u128) -> Vec<u8> {
    encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xA2u8; 32],
        signal_type: AiSignalType::StakeWithdraw,
        issuer_entity_id: issuer,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: Some(StakeWithdrawExtraV1 { amount }),
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
    })
}

fn build_stake_slash_payload(
    issuer: [u8; 32],
    target: [u8; 32],
    slash_amount: u128,
    rep_event_type: u8,
    points_delta: i16,
) -> Vec<u8> {
    encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xA3u8; 32],
        signal_type: AiSignalType::StakeSlash,
        issuer_entity_id: issuer,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: Some(StakeSlashExtraV1 {
            target_entity_id: target,
            slash_amount,
            rep_event_type,
            points_delta,
        }),
        composition_check: None,
        proof_submission: None,
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

// ============================================================================
// 1. StakeDeposit
// ============================================================================

#[test]
fn stake_deposit_basic() {
    let mut db = MemKv::new();
    let staker = make_staker(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let payload = build_stake_deposit_payload(staker.id, STAKE_AMOUNT);
    let tx = make_tx(staker.id, 0, SIGNAL_FEE, payload);

    apply_signal_commitment_tx(&mut db, &tx, DEPOSIT_HEIGHT).expect("deposit succeeds");

    let after = read_ai_entity(&db, &staker.id).unwrap().unwrap();
    assert_eq!(after.stake_balance, STAKE_AMOUNT);
    assert_eq!(
        after.economic_balance,
        STAKER_BALANCE - STAKE_AMOUNT - u128::from(SIGNAL_FEE)
    );
}

#[test]
fn stake_deposit_sets_lock_period() {
    let mut db = MemKv::new();
    let staker = make_staker(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let payload = build_stake_deposit_payload(staker.id, STAKE_AMOUNT);
    let tx = make_tx(staker.id, 0, SIGNAL_FEE, payload);

    apply_signal_commitment_tx(&mut db, &tx, DEPOSIT_HEIGHT).expect("deposit succeeds");

    let after = read_ai_entity(&db, &staker.id).unwrap().unwrap();
    assert_eq!(
        after.stake_locked_until,
        DEPOSIT_HEIGHT + STAKE_LOCK_PERIOD,
        "lock should be exactly current_height + STAKE_LOCK_PERIOD"
    );
}

#[test]
fn stake_deposit_insufficient_economic_balance_rejected() {
    let mut db = MemKv::new();
    let mut staker = build_entity([0x11u8; 32], [0x21u8; 32], staker_caps());
    staker.economic_balance = u128::from(SIGNAL_FEE) + 100;
    store_entity(&mut db, &staker);

    let payload = build_stake_deposit_payload(staker.id, STAKE_AMOUNT);
    let tx = make_tx(staker.id, 0, SIGNAL_FEE, payload);

    let err = apply_signal_commitment_tx(&mut db, &tx, DEPOSIT_HEIGHT).expect_err("must fail");
    assert!(
        matches!(err, ExecError::InsufficientEntityBalance { .. }),
        "got {err:?}"
    );

    let after = read_ai_entity(&db, &staker.id).unwrap().unwrap();
    assert_eq!(
        after.stake_balance, 0,
        "stake_balance unchanged on rejection"
    );
}

#[test]
fn stake_deposit_refreshes_lock_on_redeposit() {
    let mut db = MemKv::new();
    let staker = make_staker(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let p1 = build_stake_deposit_payload(staker.id, STAKE_AMOUNT);
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(staker.id, 0, SIGNAL_FEE, p1),
        DEPOSIT_HEIGHT,
    )
    .unwrap();

    let mid = read_ai_entity(&db, &staker.id).unwrap().unwrap();
    assert_eq!(mid.stake_locked_until, DEPOSIT_HEIGHT + STAKE_LOCK_PERIOD);

    let later_height = DEPOSIT_HEIGHT + 10;
    let p2 = build_stake_deposit_payload(staker.id, STAKE_AMOUNT);
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(staker.id, 1, SIGNAL_FEE, p2),
        later_height,
    )
    .unwrap();

    let after = read_ai_entity(&db, &staker.id).unwrap().unwrap();
    assert_eq!(
        after.stake_balance,
        STAKE_AMOUNT * 2,
        "redeposit accumulates"
    );
    assert_eq!(
        after.stake_locked_until,
        later_height + STAKE_LOCK_PERIOD,
        "redeposit refreshes lock to later_height + LOCK"
    );
}

// ============================================================================
// 2. StakeWithdraw
// ============================================================================

#[test]
fn stake_withdraw_after_lock_period() {
    let mut db = MemKv::new();
    let staker = make_staker(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let dep = build_stake_deposit_payload(staker.id, STAKE_AMOUNT);
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(staker.id, 0, SIGNAL_FEE, dep),
        DEPOSIT_HEIGHT,
    )
    .unwrap();

    let unlock_height = DEPOSIT_HEIGHT + STAKE_LOCK_PERIOD;
    let wd = build_stake_withdraw_payload(staker.id, STAKE_AMOUNT);
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(staker.id, 1, SIGNAL_FEE, wd),
        unlock_height,
    )
    .expect("withdraw at unlock_height succeeds");

    let after = read_ai_entity(&db, &staker.id).unwrap().unwrap();
    assert_eq!(after.stake_balance, 0);
    assert_eq!(
        after.economic_balance,
        STAKER_BALANCE - 2 * u128::from(SIGNAL_FEE),
        "two tx fees deducted (deposit + withdraw); principal returned to economic_balance"
    );
}

#[test]
fn stake_withdraw_rejected_before_lock_period() {
    let mut db = MemKv::new();
    let staker = make_staker(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let dep = build_stake_deposit_payload(staker.id, STAKE_AMOUNT);
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(staker.id, 0, SIGNAL_FEE, dep),
        DEPOSIT_HEIGHT,
    )
    .unwrap();

    let too_early = DEPOSIT_HEIGHT + STAKE_LOCK_PERIOD - 1;
    let wd = build_stake_withdraw_payload(staker.id, STAKE_AMOUNT);
    let err =
        apply_signal_commitment_tx(&mut db, &make_tx(staker.id, 1, SIGNAL_FEE, wd), too_early)
            .expect_err("must fail");
    assert!(
        matches!(err, ExecError::StakeStillLocked { .. }),
        "got {err:?}"
    );

    let after = read_ai_entity(&db, &staker.id).unwrap().unwrap();
    assert_eq!(
        after.stake_balance, STAKE_AMOUNT,
        "stake_balance unchanged on rejection"
    );
}

#[test]
fn stake_withdraw_partial_leaves_remaining_unlocked() {
    let mut db = MemKv::new();
    let staker = make_staker(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let dep = build_stake_deposit_payload(staker.id, STAKE_AMOUNT);
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(staker.id, 0, SIGNAL_FEE, dep),
        DEPOSIT_HEIGHT,
    )
    .unwrap();

    let unlock_height = DEPOSIT_HEIGHT + STAKE_LOCK_PERIOD;
    let half = STAKE_AMOUNT / 2;
    let wd = build_stake_withdraw_payload(staker.id, half);
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(staker.id, 1, SIGNAL_FEE, wd),
        unlock_height,
    )
    .unwrap();

    let mid = read_ai_entity(&db, &staker.id).unwrap().unwrap();
    assert_eq!(mid.stake_balance, STAKE_AMOUNT - half);
    let lock_before = mid.stake_locked_until;

    // Second partial withdraw at the same height: remaining stake stays unlocked.
    let wd2 = build_stake_withdraw_payload(staker.id, half);
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(staker.id, 2, SIGNAL_FEE, wd2),
        unlock_height,
    )
    .expect("second partial withdraw also succeeds");

    let after = read_ai_entity(&db, &staker.id).unwrap().unwrap();
    assert_eq!(after.stake_balance, 0);
    assert_eq!(
        after.stake_locked_until, lock_before,
        "lock height NOT refreshed on partial withdraw - remaining stake is unlocked"
    );
}

#[test]
fn stake_withdraw_more_than_balance_rejected() {
    let mut db = MemKv::new();
    let staker = make_staker(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let dep = build_stake_deposit_payload(staker.id, STAKE_AMOUNT);
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(staker.id, 0, SIGNAL_FEE, dep),
        DEPOSIT_HEIGHT,
    )
    .unwrap();

    let unlock_height = DEPOSIT_HEIGHT + STAKE_LOCK_PERIOD;
    let too_much = STAKE_AMOUNT + 1;
    let wd = build_stake_withdraw_payload(staker.id, too_much);
    let err = apply_signal_commitment_tx(
        &mut db,
        &make_tx(staker.id, 1, SIGNAL_FEE, wd),
        unlock_height,
    )
    .expect_err("must fail");
    assert!(
        matches!(err, ExecError::InsufficientStakeBalance { .. }),
        "got {err:?}"
    );
}

// ============================================================================
// 3. StakeSlash
// ============================================================================

#[test]
fn stake_slash_deducts_balance_and_credits_treasury() {
    let mut db = MemKv::new();
    let slasher = make_slasher(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_staker(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let dep = build_stake_deposit_payload(target.id, STAKE_AMOUNT);
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(target.id, 0, SIGNAL_FEE, dep),
        DEPOSIT_HEIGHT,
    )
    .unwrap();

    let slash_amount = STAKE_AMOUNT / 4;
    let slash = build_stake_slash_payload(
        slasher.id,
        target.id,
        slash_amount,
        REP_EVENT_STAKE_SLASH,
        -10,
    );
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(slasher.id, 0, SIGNAL_FEE, slash),
        DEPOSIT_HEIGHT,
    )
    .expect("slash succeeds");

    let target_after = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert_eq!(target_after.stake_balance, STAKE_AMOUNT - slash_amount);
    assert_eq!(read_slash_treasury(&db), slash_amount);
}

#[test]
fn stake_slash_saturates_when_amount_exceeds_balance() {
    let mut db = MemKv::new();
    let slasher = make_slasher(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_staker(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let dep = build_stake_deposit_payload(target.id, STAKE_AMOUNT);
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(target.id, 0, SIGNAL_FEE, dep),
        DEPOSIT_HEIGHT,
    )
    .unwrap();

    let over_slash = STAKE_AMOUNT * 10;
    let slash = build_stake_slash_payload(
        slasher.id,
        target.id,
        over_slash,
        REP_EVENT_STAKE_SLASH,
        -50,
    );
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(slasher.id, 0, SIGNAL_FEE, slash),
        DEPOSIT_HEIGHT,
    )
    .expect("slash succeeds (saturating)");

    let target_after = read_ai_entity(&db, &target.id).unwrap().unwrap();
    assert_eq!(
        target_after.stake_balance, 0,
        "saturating slash zeroes the balance"
    );
    assert_eq!(
        read_slash_treasury(&db),
        STAKE_AMOUNT,
        "treasury credited with actual amount slashed (not the requested over-amount)"
    );
}

#[test]
fn stake_slash_applies_reputation_update_atomically() {
    let mut db = MemKv::new();
    let slasher = make_slasher(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_staker(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let dep = build_stake_deposit_payload(target.id, STAKE_AMOUNT);
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(target.id, 0, SIGNAL_FEE, dep),
        DEPOSIT_HEIGHT,
    )
    .unwrap();

    let target_before = read_ai_entity(&db, &target.id).unwrap().unwrap();
    let pts: i16 = -15;

    let slash = build_stake_slash_payload(
        slasher.id,
        target.id,
        STAKE_AMOUNT / 2,
        REP_EVENT_STAKE_SLASH,
        pts,
    );
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(slasher.id, 0, SIGNAL_FEE, slash),
        DEPOSIT_HEIGHT,
    )
    .unwrap();

    let target_after = read_ai_entity(&db, &target.id).unwrap().unwrap();
    let expected_score =
        (i32::from(target_before.reputation_score) + i32::from(pts)).clamp(0, 100) as u16;
    assert_eq!(target_after.reputation_score, expected_score);
    assert_eq!(
        target_after.reputation_events_count,
        target_before.reputation_events_count + 1
    );
    assert_eq!(target_after.stake_balance, STAKE_AMOUNT / 2);
}

#[test]
fn stake_slash_rejects_issuer_without_capability() {
    let mut db = MemKv::new();
    // Issuer has emit_proposals (gated()) but NOT submit_reputation_updates.
    let mut bad_issuer = build_entity([0x11u8; 32], [0x21u8; 32], Capabilities::gated());
    bad_issuer.economic_balance = STAKER_BALANCE;
    store_entity(&mut db, &bad_issuer);

    let target = make_staker(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let slash = build_stake_slash_payload(bad_issuer.id, target.id, 100, REP_EVENT_DECAY, 0);
    let err = apply_signal_commitment_tx(
        &mut db,
        &make_tx(bad_issuer.id, 0, SIGNAL_FEE, slash),
        DEPOSIT_HEIGHT,
    )
    .expect_err("must fail");
    assert!(
        matches!(err, ExecError::IssuerMissingCapability),
        "got {err:?}"
    );
}

#[test]
fn stake_slash_rejects_self_slash() {
    let mut db = MemKv::new();
    let slasher = make_slasher(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let slash = build_stake_slash_payload(slasher.id, slasher.id, 100, REP_EVENT_STAKE_SLASH, -1);
    let err = apply_signal_commitment_tx(
        &mut db,
        &make_tx(slasher.id, 0, SIGNAL_FEE, slash),
        DEPOSIT_HEIGHT,
    )
    .expect_err("must fail");
    assert!(matches!(err, ExecError::SelfSlash), "got {err:?}");
}

#[test]
fn stake_slash_rejects_invalid_rep_event_type() {
    let mut db = MemKv::new();
    let slasher = make_slasher(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let target = make_staker(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let invalid_event: u8 = 99;
    let slash = build_stake_slash_payload(slasher.id, target.id, 100, invalid_event, 0);
    let err = apply_signal_commitment_tx(
        &mut db,
        &make_tx(slasher.id, 0, SIGNAL_FEE, slash),
        DEPOSIT_HEIGHT,
    )
    .expect_err("must fail");
    assert!(
        matches!(err, ExecError::InvalidReputationEventType { byte: 99 }),
        "got {err:?}"
    );
}

#[test]
fn stake_slash_target_not_found_rejected() {
    let mut db = MemKv::new();
    let slasher = make_slasher(&mut db, [0x11u8; 32], [0x21u8; 32]);

    let bogus_target = [0xDEu8; 32];
    let slash = build_stake_slash_payload(slasher.id, bogus_target, 100, REP_EVENT_STAKE_SLASH, -5);
    let err = apply_signal_commitment_tx(
        &mut db,
        &make_tx(slasher.id, 0, SIGNAL_FEE, slash),
        DEPOSIT_HEIGHT,
    )
    .expect_err("must fail");
    assert!(
        matches!(err, ExecError::TargetEntityNotFound),
        "got {err:?}"
    );
}

// ============================================================================
// 4. Regression: zero-stake entity is unaffected
// ============================================================================

#[test]
fn entity_with_zero_stake_can_still_operate() {
    let mut db = MemKv::new();
    let staker = make_staker(&mut db, [0x11u8; 32], [0x21u8; 32]);

    // Send a base anomaly signal (no stake operations) - should succeed.
    let anomaly = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8; 32],
        signal_type: AiSignalType::Anomaly,
        issuer_entity_id: staker.id,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
    });
    apply_signal_commitment_tx(
        &mut db,
        &make_tx(staker.id, 0, SIGNAL_FEE, anomaly),
        DEPOSIT_HEIGHT,
    )
    .expect("base signal still applies for zero-stake entity");

    let after = read_ai_entity(&db, &staker.id).unwrap().unwrap();
    assert_eq!(after.stake_balance, 0);
    assert_eq!(after.stake_locked_until, 0);
}
