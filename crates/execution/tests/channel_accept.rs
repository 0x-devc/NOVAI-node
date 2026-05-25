#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]

//! Integration tests for the Week 32 `ChannelAccept` signal handler
//! (Phase 3).
//!
//! Each test proposes a `PaymentChannel` via `CreateMemoryObject` and
//! then exercises the `ChannelAccept` signal path:
//!
//! - Happy path: status transitions PROPOSED -> OPEN,
//!   `accepted_at_height` is recorded, `balance_b` is set to
//!   `deposit_b`, party B's `economic_balance` is debited
//!   `deposit_b` (plus the tx fee).
//! - Defensive rejections: channel not found, wrong resolved object
//!   type, double acceptance, wrong issuer (not party B),
//!   insufficient balance.

use novai_ai_entities::{
    AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObjectType, PaymentChannelData,
    CHANNEL_DISPUTE_WINDOW_DEFAULT_BLOCKS, PAYMENT_CHANNEL_RESERVED_LEN, PAYMENT_CHANNEL_SIZE,
    PAYMENT_CHANNEL_STATUS_OPEN, PAYMENT_CHANNEL_STATUS_PROPOSED, PAYMENT_CHANNEL_V1,
};
use novai_execution::{
    apply_create_memory_object_tx, apply_signal_commitment_tx, channel_by_party_a_key,
    channel_by_party_b_key, encode_create_memory_object_payload_v1,
    encode_signal_commitment_payload_v1, write_ai_entity_op, ChannelAcceptExtraV1,
    CreateMemoryObjectPayloadV1, ExecError, SignalCommitmentPayloadV1,
};
use novai_state::{ai_entity_by_address_key, ai_memory_object_key, Kv, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const PROPOSER_BALANCE: u128 = 10_000_000;
const COUNTERPARTY_BALANCE: u128 = 10_000_000;
const CREATE_FEE: u64 = 1_000;
const ACCEPT_FEE: u64 = 1_000;
const HEIGHT_PROPOSE: u64 = 500;
const HEIGHT_ACCEPT: u64 = 700;
const DEFAULT_DEPOSIT_A: u128 = 200_000;
const DEFAULT_DEPOSIT_B: u128 = 150_000;

fn caps() -> Capabilities {
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
    AiEntity::new(code_hash, creator, AutonomyMode::Gated, caps(), 1000)
}

fn store_entity(db: &mut MemKv, entity: &AiEntity) {
    db.apply_batch(&[
        write_ai_entity_op(entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();
}

fn make_proposer(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut e = build_entity(code_hash, creator);
    e.economic_balance = PROPOSER_BALANCE;
    store_entity(db, &e);
    e
}

fn make_counterparty(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut e = build_entity(code_hash, creator);
    e.economic_balance = COUNTERPARTY_BALANCE;
    store_entity(db, &e);
    e
}

fn make_counterparty_with_balance(
    db: &mut MemKv,
    code_hash: [u8; 32],
    creator: [u8; 32],
    balance: u128,
) -> AiEntity {
    let mut e = build_entity(code_hash, creator);
    e.economic_balance = balance;
    store_entity(db, &e);
    e
}

fn sample_channel(
    party_a: &AiEntity,
    party_b: &AiEntity,
    deposit_a: u128,
    deposit_b: u128,
) -> PaymentChannelData {
    PaymentChannelData {
        version: PAYMENT_CHANNEL_V1,
        party_a_entity_id: party_a.id,
        party_b_entity_id: party_b.id,
        sla_object_id: [0u8; 32],
        status: PAYMENT_CHANNEL_STATUS_PROPOSED,
        deposit_a,
        deposit_b,
        balance_a: deposit_a,
        balance_b: 0,
        nonce: 0,
        proposed_at_height: 0,
        accepted_at_height: 0,
        closing_at_height: 0,
        dispute_deadline_height: 0,
        dispute_window_blocks: CHANNEL_DISPUTE_WINDOW_DEFAULT_BLOCKS,
        reserved: [0u8; PAYMENT_CHANNEL_RESERVED_LEN],
    }
}

fn propose(
    db: &mut MemKv,
    party_a: &AiEntity,
    nonce: u64,
    channel: &PaymentChannelData,
) -> [u8; 32] {
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::PaymentChannel,
        data: channel.encode().to_vec(),
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: party_a.id,
        pubkey: party_a.id,
        nonce,
        fee: CREATE_FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_create_memory_object_tx(db, &tx, HEIGHT_PROPOSE).expect("propose succeeds")
}

fn make_accept_tx(
    party_b: &AiEntity,
    nonce: u64,
    channel_object_id: [u8; 32],
    party_a_id: [u8; 32],
) -> TxV1 {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xEFu8; 32],
        signal_type: AiSignalType::ChannelAccept,
        issuer_entity_id: party_b.id,
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
        sla_accept: None,
        channel_accept: Some(ChannelAcceptExtraV1 {
            channel_object_id,
            party_a_entity_id: party_a_id,
        }),
        channel_close: None,
        channel_finalize: None,
        oracle_anchor: None,
    });
    TxV1 {
        version: TxVersion::V1,
        from: party_b.id,
        pubkey: party_b.id,
        nonce,
        fee: ACCEPT_FEE,
        payload,
        sig: [0u8; 64],
    }
}

fn read_channel(db: &MemKv, party_a: &[u8; 32], object_id: &[u8; 32]) -> PaymentChannelData {
    let envelope = db
        .get(&ai_memory_object_key(party_a, object_id))
        .unwrap()
        .expect("channel envelope present");
    let payload_start = envelope.len() - PAYMENT_CHANNEL_SIZE;
    PaymentChannelData::decode(&envelope[payload_start..]).expect("stored bytes decode")
}

// ============================================================================
// 1. Happy path
// ============================================================================

#[test]
fn channel_accept_transitions_to_open_and_escrows_deposit_b() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let b = make_counterparty(&mut db, [0x12u8; 32], [0x22u8; 32]);
    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let object_id = propose(&mut db, &a, 0, &channel);

    let tx = make_accept_tx(&b, 0, object_id, a.id);
    apply_signal_commitment_tx(&mut db, &tx, HEIGHT_ACCEPT).expect("accept succeeds");

    let stored = read_channel(&db, &a.id, &object_id);
    assert_eq!(stored.status, PAYMENT_CHANNEL_STATUS_OPEN);
    assert_eq!(stored.accepted_at_height, HEIGHT_ACCEPT);
    assert_eq!(stored.balance_a, DEFAULT_DEPOSIT_A);
    assert_eq!(stored.balance_b, DEFAULT_DEPOSIT_B);
    assert_eq!(stored.deposit_a, DEFAULT_DEPOSIT_A);
    assert_eq!(stored.deposit_b, DEFAULT_DEPOSIT_B);
    assert_eq!(stored.nonce, 0);

    // Party B's economic_balance is debited deposit_b + accept fee.
    let b_after = novai_execution::lookup_ai_entity_by_address(&db, &b.id)
        .unwrap()
        .unwrap();
    let expected = COUNTERPARTY_BALANCE - DEFAULT_DEPOSIT_B - u128::from(ACCEPT_FEE);
    assert_eq!(b_after.economic_balance, expected);
}

#[test]
fn channel_accept_signal_payload_is_130_bytes() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x13u8; 32], [0x23u8; 32]);
    let b = make_counterparty(&mut db, [0x14u8; 32], [0x24u8; 32]);
    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let object_id = propose(&mut db, &a, 0, &channel);
    let tx = make_accept_tx(&b, 0, object_id, a.id);
    assert_eq!(tx.payload.len(), 130);
}

#[test]
fn channel_accept_leaves_by_party_indexes_in_place() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x15u8; 32], [0x25u8; 32]);
    let b = make_counterparty(&mut db, [0x16u8; 32], [0x26u8; 32]);
    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let object_id = propose(&mut db, &a, 0, &channel);

    let by_a = channel_by_party_a_key(&a.id, HEIGHT_PROPOSE, &object_id);
    let by_b = channel_by_party_b_key(&b.id, HEIGHT_PROPOSE, &object_id);
    assert!(db.get(&by_a).unwrap().is_some(), "pre-accept");
    assert!(db.get(&by_b).unwrap().is_some(), "pre-accept");

    let tx = make_accept_tx(&b, 0, object_id, a.id);
    apply_signal_commitment_tx(&mut db, &tx, HEIGHT_ACCEPT).expect("accept succeeds");

    // Both index entries still present after acceptance.
    assert!(db.get(&by_a).unwrap().is_some(), "post-accept");
    let by_b_val = db.get(&by_b).unwrap().unwrap();
    assert_eq!(by_b_val, a.id.to_vec());
}

// ============================================================================
// 2. Defensive rejections
// ============================================================================

#[test]
fn channel_accept_rejects_when_channel_not_found() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x17u8; 32], [0x27u8; 32]);
    let b = make_counterparty(&mut db, [0x18u8; 32], [0x28u8; 32]);

    let bogus_id = [0xAAu8; 32];
    let tx = make_accept_tx(&b, 0, bogus_id, a.id);
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_ACCEPT).unwrap_err();
    assert!(matches!(err, ExecError::ChannelAcceptNotFound));
}

#[test]
fn channel_accept_rejects_when_resolved_object_is_wrong_type() {
    // Build a non-PaymentChannel memory object (DelegationGrant) and
    // try to accept it via ChannelAccept. The handler must reject
    // with ChannelAcceptObjectTypeMismatch carrying the resolved
    // type byte.
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x19u8; 32], [0x29u8; 32]);
    let b = make_counterparty(&mut db, [0x1Au8; 32], [0x2Au8; 32]);

    // Create a stand-in memory object owned by `a` with a different
    // type. The simplest stand-in: a ChainSummary (type 0) with
    // minimal data.
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::ChainSummary,
        data: vec![0u8; 32],
    });
    let tx_create = TxV1 {
        version: TxVersion::V1,
        from: a.id,
        pubkey: a.id,
        nonce: 0,
        fee: CREATE_FEE,
        payload,
        sig: [0u8; 64],
    };
    let object_id =
        apply_create_memory_object_tx(&mut db, &tx_create, HEIGHT_PROPOSE).expect("create");

    let tx = make_accept_tx(&b, 0, object_id, a.id);
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_ACCEPT).unwrap_err();
    match err {
        ExecError::ChannelAcceptObjectTypeMismatch { found } => {
            assert_eq!(found, MemoryObjectType::ChainSummary.to_byte());
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn channel_accept_rejects_double_acceptance() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x1Bu8; 32], [0x2Bu8; 32]);
    let b = make_counterparty(&mut db, [0x1Cu8; 32], [0x2Cu8; 32]);
    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let object_id = propose(&mut db, &a, 0, &channel);

    let tx1 = make_accept_tx(&b, 0, object_id, a.id);
    apply_signal_commitment_tx(&mut db, &tx1, HEIGHT_ACCEPT).expect("first accept");

    let tx2 = make_accept_tx(&b, 1, object_id, a.id);
    let err = apply_signal_commitment_tx(&mut db, &tx2, HEIGHT_ACCEPT + 1).unwrap_err();
    match err {
        ExecError::ChannelAcceptNotProposed { status } => {
            assert_eq!(status, PAYMENT_CHANNEL_STATUS_OPEN);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn channel_accept_rejects_wrong_counterparty() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x1Du8; 32], [0x2Du8; 32]);
    let b = make_counterparty(&mut db, [0x1Eu8; 32], [0x2Eu8; 32]);
    let intruder = make_counterparty(&mut db, [0x1Fu8; 32], [0x2Fu8; 32]);
    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let object_id = propose(&mut db, &a, 0, &channel);

    // intruder tries to accept the channel meant for b.
    let tx = make_accept_tx(&intruder, 0, object_id, a.id);
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_ACCEPT).unwrap_err();
    assert!(matches!(err, ExecError::ChannelAcceptCounterpartyMismatch));
}

#[test]
fn channel_accept_rejects_insufficient_balance() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x30u8; 32], [0x40u8; 32]);
    // Party B has enough for the fee but not enough for deposit_b.
    let underfunded = u128::from(ACCEPT_FEE) + DEFAULT_DEPOSIT_B - 1;
    let b = make_counterparty_with_balance(&mut db, [0x31u8; 32], [0x41u8; 32], underfunded);
    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let object_id = propose(&mut db, &a, 0, &channel);

    let tx = make_accept_tx(&b, 0, object_id, a.id);
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_ACCEPT).unwrap_err();
    match err {
        ExecError::ChannelAcceptInsufficientBalance {
            required,
            available,
        } => {
            assert_eq!(required, DEFAULT_DEPOSIT_B);
            // After fee debit upstream, available == DEFAULT_DEPOSIT_B - 1.
            assert_eq!(available, DEFAULT_DEPOSIT_B - 1);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn channel_accept_atomic_on_failure() {
    // Sanity: a rejected accept must NOT debit party B's balance,
    // mutate the channel status, or otherwise leave partial state.
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x32u8; 32], [0x42u8; 32]);
    let b = make_counterparty(&mut db, [0x33u8; 32], [0x43u8; 32]);
    let intruder = make_counterparty(&mut db, [0x34u8; 32], [0x44u8; 32]);
    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let object_id = propose(&mut db, &a, 0, &channel);

    let stored_before = read_channel(&db, &a.id, &object_id);
    let intruder_before = novai_execution::lookup_ai_entity_by_address(&db, &intruder.id)
        .unwrap()
        .unwrap();

    let tx = make_accept_tx(&intruder, 0, object_id, a.id);
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_ACCEPT).unwrap_err();
    assert!(matches!(err, ExecError::ChannelAcceptCounterpartyMismatch));

    let stored_after = read_channel(&db, &a.id, &object_id);
    assert_eq!(stored_after, stored_before);
    let intruder_after = novai_execution::lookup_ai_entity_by_address(&db, &intruder.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        intruder_after.economic_balance,
        intruder_before.economic_balance
    );
    assert_eq!(intruder_after.nonce, intruder_before.nonce);
}
