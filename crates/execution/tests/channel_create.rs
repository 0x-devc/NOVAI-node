#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]

//! Integration tests for the Week 32 `PaymentChannel`
//! `CREATE_MEMORY_OBJECT`, `DELETE_MEMORY_OBJECT`, and
//! `UPDATE_MEMORY_OBJECT` handlers (Phase 2).
//!
//! Each test proposes a `PaymentChannel` via the create-memory-object
//! transaction path and exercises one rule of the validator (or one
//! lifecycle path of the delete handler):
//!
//! - Happy path: deposit_a debited, two index entries written, the
//!   primary record carries the canonical PROPOSED status and
//!   initial balances.
//! - Defensive rejections covering every numbered validation rule
//!   in `validate_payment_channel_payload`.
//! - Delete-while-PROPOSED refunds party A and tears down both
//!   indexes; delete-while-OPEN / -CLOSING is rejected.
//! - Update memory object against type 15 is unconditionally
//!   rejected.

use novai_ai_entities::{
    AiEntity, AutonomyMode, Capabilities, MemoryObjectType, PaymentChannelData,
    CHANNEL_DISPUTE_WINDOW_DEFAULT_BLOCKS, CHANNEL_DISPUTE_WINDOW_MAX_BLOCKS,
    CHANNEL_DISPUTE_WINDOW_MIN_BLOCKS, MAX_PAYMENT_CHANNELS_PER_ENTITY,
    PAYMENT_CHANNEL_RESERVED_LEN, PAYMENT_CHANNEL_SIZE, PAYMENT_CHANNEL_STATUS_CLOSING,
    PAYMENT_CHANNEL_STATUS_OPEN, PAYMENT_CHANNEL_STATUS_PROPOSED, PAYMENT_CHANNEL_V1,
};
use novai_execution::{
    apply_create_memory_object_tx, apply_delete_memory_object_tx, apply_update_memory_object_tx,
    channel_by_party_a_key, channel_by_party_b_key, count_payment_channels_for_entity,
    encode_create_memory_object_payload_v1, encode_delete_memory_object_payload_v1,
    encode_update_memory_object_payload_v1, write_ai_entity_op, CreateMemoryObjectPayloadV1,
    DeleteMemoryObjectPayloadV1, ExecError, UpdateMemoryObjectPayloadV1,
};
use novai_state::{ai_entity_by_address_key, ai_memory_object_key, Kv, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const PROPOSER_BALANCE: u128 = 10_000_000;
const COUNTERPARTY_BALANCE: u128 = 10_000_000;
const CREATE_FEE: u64 = 1_000;
const DELETE_FEE: u64 = 1_000;
const UPDATE_FEE: u64 = 1_000;
const HEIGHT_PROPOSE: u64 = 500;
const HEIGHT_DELETE: u64 = 600;
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

fn propose_tx(party_a: &AiEntity, nonce: u64, channel: &PaymentChannelData) -> TxV1 {
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::PaymentChannel,
        data: channel.encode().to_vec(),
    });
    TxV1 {
        version: TxVersion::V1,
        from: party_a.id,
        pubkey: party_a.id,
        nonce,
        fee: CREATE_FEE,
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
fn channel_propose_happy_path() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let b = make_counterparty(&mut db, [0x12u8; 32], [0x22u8; 32]);

    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let tx = propose_tx(&a, 0, &channel);
    let object_id =
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).expect("propose succeeds");

    // Primary record present and carries PROPOSED + correct fields.
    let stored = read_channel(&db, &a.id, &object_id);
    assert_eq!(stored.status, PAYMENT_CHANNEL_STATUS_PROPOSED);
    assert_eq!(stored.party_a_entity_id, a.id);
    assert_eq!(stored.party_b_entity_id, b.id);
    assert_eq!(stored.deposit_a, DEFAULT_DEPOSIT_A);
    assert_eq!(stored.deposit_b, DEFAULT_DEPOSIT_B);
    assert_eq!(stored.balance_a, DEFAULT_DEPOSIT_A);
    assert_eq!(stored.balance_b, 0);
    assert_eq!(stored.nonce, 0);
    assert_eq!(stored.accepted_at_height, 0);
    assert_eq!(stored.closing_at_height, 0);
    assert_eq!(stored.dispute_deadline_height, 0);
    assert_eq!(
        stored.dispute_window_blocks,
        CHANNEL_DISPUTE_WINDOW_DEFAULT_BLOCKS
    );

    // by_party_a index entry present, value empty.
    let by_a = channel_by_party_a_key(&a.id, HEIGHT_PROPOSE, &object_id);
    assert_eq!(db.get(&by_a).unwrap().unwrap(), Vec::<u8>::new());

    // by_party_b index entry present, value == party_a.id.
    let by_b = channel_by_party_b_key(&b.id, HEIGHT_PROPOSE, &object_id);
    assert_eq!(db.get(&by_b).unwrap().unwrap(), a.id.to_vec());

    // Per-entity channel count reflects both roles.
    assert_eq!(count_payment_channels_for_entity(&db, &a.id).unwrap(), 1);
    assert_eq!(count_payment_channels_for_entity(&db, &b.id).unwrap(), 1);

    // economic_balance: proposer debited deposit_a + fee.
    let a_after = novai_execution::lookup_ai_entity_by_address(&db, &a.id)
        .unwrap()
        .unwrap();
    let expected_balance = PROPOSER_BALANCE - DEFAULT_DEPOSIT_A - u128::from(CREATE_FEE);
    assert_eq!(a_after.economic_balance, expected_balance);

    // Counterparty's balance is untouched until accept (Phase 3).
    let b_after = novai_execution::lookup_ai_entity_by_address(&db, &b.id)
        .unwrap()
        .unwrap();
    assert_eq!(b_after.economic_balance, COUNTERPARTY_BALANCE);
}

// ============================================================================
// 2. Defensive rejections
// ============================================================================

#[test]
fn channel_propose_self_referential_rejected() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x13u8; 32], [0x23u8; 32]);
    // Wire party_b == party_a manually (sample_channel forbids it).
    let mut channel = sample_channel(&a, &a, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    channel.party_b_entity_id = a.id;
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    assert!(matches!(err, ExecError::PaymentChannelSelfReferential));
}

#[test]
fn channel_propose_deposit_a_zero_rejected() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x14u8; 32], [0x24u8; 32]);
    let b = make_counterparty(&mut db, [0x15u8; 32], [0x25u8; 32]);
    let mut channel = sample_channel(&a, &b, 0, DEFAULT_DEPOSIT_B);
    channel.balance_a = 0; // keep invariant balance_a == deposit_a
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    assert!(matches!(err, ExecError::PaymentChannelDepositAZero));
}

#[test]
fn channel_propose_deposit_b_zero_rejected() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x16u8; 32], [0x26u8; 32]);
    let b = make_counterparty(&mut db, [0x17u8; 32], [0x27u8; 32]);
    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, 0);
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    assert!(matches!(err, ExecError::PaymentChannelDepositBZero));
}

#[test]
fn channel_propose_deposit_overflow_rejected() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x18u8; 32], [0x28u8; 32]);
    let b = make_counterparty(&mut db, [0x19u8; 32], [0x29u8; 32]);
    // deposit_a + deposit_b > u128::MAX. balance_a == deposit_a is
    // preserved so the InitialFieldsNotZero gate stays clean.
    let mut channel = sample_channel(&a, &b, u128::MAX, 1);
    channel.balance_a = u128::MAX;
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    assert!(matches!(err, ExecError::PaymentChannelDepositTotalOverflow));
}

#[test]
fn channel_propose_insufficient_balance_rejected() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x1Au8; 32], [0x2Au8; 32]);
    let b = make_counterparty(&mut db, [0x1Bu8; 32], [0x2Bu8; 32]);
    // deposit_a slightly exceeds proposer.economic_balance (after fee).
    let oversized = PROPOSER_BALANCE - u128::from(CREATE_FEE) + 1;
    let channel = sample_channel(&a, &b, oversized, DEFAULT_DEPOSIT_B);
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    match err {
        ExecError::PaymentChannelInsufficientBalanceA {
            required,
            available,
        } => {
            assert_eq!(required, oversized);
            assert_eq!(available, PROPOSER_BALANCE - u128::from(CREATE_FEE));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn channel_propose_dispute_window_too_small_rejected() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x1Cu8; 32], [0x2Cu8; 32]);
    let b = make_counterparty(&mut db, [0x1Du8; 32], [0x2Du8; 32]);
    let mut channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    channel.dispute_window_blocks = CHANNEL_DISPUTE_WINDOW_MIN_BLOCKS - 1;
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    assert!(matches!(
        err,
        ExecError::PaymentChannelDisputeWindowOutOfRange { .. }
    ));
}

#[test]
fn channel_propose_dispute_window_too_large_rejected() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x1Eu8; 32], [0x2Eu8; 32]);
    let b = make_counterparty(&mut db, [0x1Fu8; 32], [0x2Fu8; 32]);
    let mut channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    channel.dispute_window_blocks = CHANNEL_DISPUTE_WINDOW_MAX_BLOCKS + 1;
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    assert!(matches!(
        err,
        ExecError::PaymentChannelDisputeWindowOutOfRange { .. }
    ));
}

#[test]
fn channel_propose_reserved_not_zero_rejected() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x30u8; 32], [0x40u8; 32]);
    let b = make_counterparty(&mut db, [0x31u8; 32], [0x41u8; 32]);
    let mut channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    channel.reserved[0] = 1;
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    assert!(matches!(err, ExecError::PaymentChannelReservedNotZero));
}

#[test]
fn channel_propose_initial_fields_not_zero_rejected_balance_b() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x32u8; 32], [0x42u8; 32]);
    let b = make_counterparty(&mut db, [0x33u8; 32], [0x43u8; 32]);
    let mut channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    channel.balance_b = 1; // pre-seed B's balance — forbidden at create
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    assert!(matches!(err, ExecError::PaymentChannelInitialFieldsNotZero));
}

#[test]
fn channel_propose_initial_fields_not_zero_rejected_nonce() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x34u8; 32], [0x44u8; 32]);
    let b = make_counterparty(&mut db, [0x35u8; 32], [0x45u8; 32]);
    let mut channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    channel.nonce = 1;
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    assert!(matches!(err, ExecError::PaymentChannelInitialFieldsNotZero));
}

#[test]
fn channel_propose_initial_fields_balance_a_must_equal_deposit_a() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x36u8; 32], [0x46u8; 32]);
    let b = make_counterparty(&mut db, [0x37u8; 32], [0x47u8; 32]);
    let mut channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    channel.balance_a = DEFAULT_DEPOSIT_A - 1; // mismatch
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    assert!(matches!(err, ExecError::PaymentChannelInitialFieldsNotZero));
}

#[test]
fn channel_propose_status_not_proposed_rejected() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x38u8; 32], [0x48u8; 32]);
    let b = make_counterparty(&mut db, [0x39u8; 32], [0x49u8; 32]);
    let mut channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    channel.status = PAYMENT_CHANNEL_STATUS_OPEN;
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    assert!(matches!(
        err,
        ExecError::PaymentChannelStatusInvalidAtCreate { byte: 1 }
    ));
}

#[test]
fn channel_propose_party_a_must_be_issuer() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x3Au8; 32], [0x4Au8; 32]);
    let other = make_proposer(&mut db, [0x3Bu8; 32], [0x4Bu8; 32]);
    let b = make_counterparty(&mut db, [0x3Cu8; 32], [0x4Cu8; 32]);

    let channel = sample_channel(&other, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    // tx is signed by `a`, but the payload's party_a_entity_id is `other`.
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    assert!(matches!(err, ExecError::PaymentChannelPartyAMustBeIssuer));
}

#[test]
fn channel_propose_party_b_not_found_rejected() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x3Du8; 32], [0x4Du8; 32]);
    // Construct a sample channel against a non-existent party B id.
    let phantom = AiEntity::new(
        [0xFFu8; 32],
        [0xFEu8; 32],
        AutonomyMode::Gated,
        caps(),
        1000,
    );
    let channel = sample_channel(&a, &phantom, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    assert!(matches!(err, ExecError::PaymentChannelPartyBNotFound));
}

#[test]
fn channel_propose_party_b_not_active_rejected() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x3Eu8; 32], [0x4Eu8; 32]);
    let mut b = build_entity([0x3Fu8; 32], [0x4Fu8; 32]);
    b.economic_balance = COUNTERPARTY_BALANCE;
    b.is_active = false;
    store_entity(&mut db, &b);

    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    assert!(matches!(err, ExecError::PaymentChannelPartyBNotActive));
}

#[test]
fn channel_propose_version_invalid_rejected() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x50u8; 32], [0x60u8; 32]);
    let b = make_counterparty(&mut db, [0x51u8; 32], [0x61u8; 32]);
    let mut channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    channel.version = 99;
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    assert!(matches!(
        err,
        ExecError::PaymentChannelVersionInvalid { byte: 99 }
    ));
}

// ============================================================================
// 3. Per-entity cap
// ============================================================================

#[test]
fn channel_propose_per_entity_cap_exceeded() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x52u8; 32], [0x62u8; 32]);

    // Fill A's by_party_a slots with placeholder entries until the cap
    // is hit. Each slot is just an empty marker keyed by (a.id,
    // proposed_at, object_id); the cap counter does not inspect the
    // payload, just the index population, so we do not need to also
    // write real memory objects.
    let mut ops = Vec::new();
    for i in 0..MAX_PAYMENT_CHANNELS_PER_ENTITY {
        let mut object_id = [0u8; 32];
        object_id[0] = (i & 0xFF) as u8;
        object_id[1] = ((i >> 8) & 0xFF) as u8;
        ops.push(WriteOp::Put(
            channel_by_party_a_key(&a.id, 100, &object_id),
            Vec::new(),
        ));
    }
    db.apply_batch(&ops).unwrap();

    let b = make_counterparty(&mut db, [0x53u8; 32], [0x63u8; 32]);
    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let tx = propose_tx(&a, 0, &channel);
    let err = apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).unwrap_err();
    match err {
        ExecError::PaymentChannelPerEntityCapExceeded { current, max } => {
            assert_eq!(current, MAX_PAYMENT_CHANNELS_PER_ENTITY);
            assert_eq!(max, MAX_PAYMENT_CHANNELS_PER_ENTITY);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

// ============================================================================
// 4. Delete handler
// ============================================================================

fn delete_tx(party_a: &AiEntity, nonce: u64, object_id: [u8; 32]) -> TxV1 {
    let payload =
        encode_delete_memory_object_payload_v1(&DeleteMemoryObjectPayloadV1 { object_id });
    TxV1 {
        version: TxVersion::V1,
        from: party_a.id,
        pubkey: party_a.id,
        nonce,
        fee: DELETE_FEE,
        payload: payload.to_vec(),
        sig: [0u8; 64],
    }
}

#[test]
fn channel_delete_while_proposed_refunds_deposit_and_clears_indexes() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x54u8; 32], [0x64u8; 32]);
    let b = make_counterparty(&mut db, [0x55u8; 32], [0x65u8; 32]);
    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let tx = propose_tx(&a, 0, &channel);
    let object_id =
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).expect("propose succeeds");

    // Sanity: A's balance was debited deposit_a + fee.
    let a_after_propose = novai_execution::lookup_ai_entity_by_address(&db, &a.id)
        .unwrap()
        .unwrap();
    let expected_after_propose = PROPOSER_BALANCE - DEFAULT_DEPOSIT_A - u128::from(CREATE_FEE);
    assert_eq!(a_after_propose.economic_balance, expected_after_propose);

    let tx_delete = delete_tx(&a, 1, object_id);
    apply_delete_memory_object_tx(&mut db, &tx_delete, HEIGHT_DELETE)
        .expect("delete-while-proposed succeeds");

    // Primary record gone.
    assert!(db
        .get(&ai_memory_object_key(&a.id, &object_id))
        .unwrap()
        .is_none());
    // Both index entries gone.
    assert!(db
        .get(&channel_by_party_a_key(&a.id, HEIGHT_PROPOSE, &object_id))
        .unwrap()
        .is_none());
    assert!(db
        .get(&channel_by_party_b_key(&b.id, HEIGHT_PROPOSE, &object_id))
        .unwrap()
        .is_none());

    // deposit_a refunded; delete fee charged.
    let a_after_delete = novai_execution::lookup_ai_entity_by_address(&db, &a.id)
        .unwrap()
        .unwrap();
    let expected_after_delete = expected_after_propose + DEFAULT_DEPOSIT_A - u128::from(DELETE_FEE);
    assert_eq!(a_after_delete.economic_balance, expected_after_delete);
}

#[test]
fn channel_delete_while_open_rejected() {
    // Status is mutated post-create via a direct KV write since the
    // ChannelAccept handler is not wired until Phase 3. The delete
    // handler should still refuse to tear down a channel whose status
    // is anything other than PROPOSED.
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x56u8; 32], [0x66u8; 32]);
    let b = make_counterparty(&mut db, [0x57u8; 32], [0x67u8; 32]);
    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let tx = propose_tx(&a, 0, &channel);
    let object_id =
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).expect("propose succeeds");

    // Patch status to OPEN inside the stored envelope.
    let key = ai_memory_object_key(&a.id, &object_id);
    let mut envelope = db.get(&key).unwrap().unwrap();
    let payload_start = envelope.len() - PAYMENT_CHANNEL_SIZE;
    envelope[payload_start + 97] = PAYMENT_CHANNEL_STATUS_OPEN;
    db.apply_batch(&[WriteOp::Put(key, envelope)]).unwrap();

    let tx_delete = delete_tx(&a, 1, object_id);
    let err = apply_delete_memory_object_tx(&mut db, &tx_delete, HEIGHT_DELETE).unwrap_err();
    assert!(matches!(
        err,
        ExecError::PaymentChannelDeleteWhileActive { status: 1 }
    ));
}

#[test]
fn channel_delete_while_closing_rejected() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x58u8; 32], [0x68u8; 32]);
    let b = make_counterparty(&mut db, [0x59u8; 32], [0x69u8; 32]);
    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let tx = propose_tx(&a, 0, &channel);
    let object_id =
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).expect("propose succeeds");

    let key = ai_memory_object_key(&a.id, &object_id);
    let mut envelope = db.get(&key).unwrap().unwrap();
    let payload_start = envelope.len() - PAYMENT_CHANNEL_SIZE;
    envelope[payload_start + 97] = PAYMENT_CHANNEL_STATUS_CLOSING;
    db.apply_batch(&[WriteOp::Put(key, envelope)]).unwrap();

    let tx_delete = delete_tx(&a, 1, object_id);
    let err = apply_delete_memory_object_tx(&mut db, &tx_delete, HEIGHT_DELETE).unwrap_err();
    assert!(matches!(
        err,
        ExecError::PaymentChannelDeleteWhileActive { status: 2 }
    ));
}

// ============================================================================
// 5. Update handler
// ============================================================================

#[test]
fn channel_update_memory_object_always_rejected() {
    let mut db = MemKv::new();
    let a = make_proposer(&mut db, [0x5Au8; 32], [0x6Au8; 32]);
    let b = make_counterparty(&mut db, [0x5Bu8; 32], [0x6Bu8; 32]);
    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let tx = propose_tx(&a, 0, &channel);
    let object_id =
        apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE).expect("propose succeeds");

    // Craft an update tx targeting the channel; the new payload is the
    // same encoded data so the test is solely exercising the
    // immutability gate.
    let payload = encode_update_memory_object_payload_v1(&UpdateMemoryObjectPayloadV1 {
        object_id,
        new_data: channel.encode().to_vec(),
    });
    let tx_update = TxV1 {
        version: TxVersion::V1,
        from: a.id,
        pubkey: a.id,
        nonce: 1,
        fee: UPDATE_FEE,
        payload,
        sig: [0u8; 64],
    };
    let err = apply_update_memory_object_tx(&mut db, &tx_update, HEIGHT_DELETE).unwrap_err();
    assert!(matches!(err, ExecError::PaymentChannelImmutableOnUpdate));
}
