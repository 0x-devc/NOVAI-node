#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::too_many_arguments)]

//! Integration tests for the Week 32 `PaymentChannel` scan helpers
//! (Phase 6).
//!
//! Exercises `get_payment_channel`, `get_channels_by_party_a`, and
//! `get_channels_by_party_b` against real state populated through
//! the create / accept / close handlers. Verifies index-driven
//! resolution, height-window filtering, type-mismatch handling, and
//! the by-party-B value embedding of the memory-object owner.

use novai_ai_entities::{
    AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObjectType, PaymentChannelData,
    CHANNEL_DISPUTE_WINDOW_DEFAULT_BLOCKS, PAYMENT_CHANNEL_RESERVED_LEN,
    PAYMENT_CHANNEL_STATUS_OPEN, PAYMENT_CHANNEL_STATUS_PROPOSED, PAYMENT_CHANNEL_V1,
};
use novai_execution::{
    apply_create_memory_object_tx, apply_signal_commitment_tx,
    encode_create_memory_object_payload_v1, encode_signal_commitment_payload_v1,
    get_channels_by_party_a, get_channels_by_party_b, get_payment_channel, write_ai_entity_op,
    ChannelAcceptExtraV1, CreateMemoryObjectPayloadV1, SignalCommitmentPayloadV1,
};
use novai_state::{ai_entity_by_address_key, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const PROPOSER_BALANCE: u128 = 10_000_000;
const COUNTERPARTY_BALANCE: u128 = 10_000_000;
const CREATE_FEE: u64 = 1_000;
const ACCEPT_FEE: u64 = 1_000;
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

fn make_entity(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32], balance: u128) -> AiEntity {
    let mut e = AiEntity::new(code_hash, creator, AutonomyMode::Gated, caps(), 1000);
    e.economic_balance = balance;
    db.apply_batch(&[
        write_ai_entity_op(&e),
        WriteOp::Put(ai_entity_by_address_key(&e.id), e.id.to_vec()),
    ])
    .unwrap();
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

fn propose_at(
    db: &mut MemKv,
    party_a: &AiEntity,
    nonce: u64,
    channel: &PaymentChannelData,
    height: u64,
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
    apply_create_memory_object_tx(db, &tx, height).expect("propose succeeds")
}

fn accept_at(
    db: &mut MemKv,
    party_b: &AiEntity,
    nonce: u64,
    channel_object_id: [u8; 32],
    party_a_id: [u8; 32],
    height: u64,
) {
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
    let tx = TxV1 {
        version: TxVersion::V1,
        from: party_b.id,
        pubkey: party_b.id,
        nonce,
        fee: ACCEPT_FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_signal_commitment_tx(db, &tx, height).expect("accept succeeds");
}

// ============================================================================
// 1. get_payment_channel
// ============================================================================

#[test]
fn get_payment_channel_resolves_proposed_record() {
    let mut db = MemKv::new();
    let a = make_entity(&mut db, [0x11u8; 32], [0x21u8; 32], PROPOSER_BALANCE);
    let b = make_entity(&mut db, [0x12u8; 32], [0x22u8; 32], COUNTERPARTY_BALANCE);
    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let object_id = propose_at(&mut db, &a, 0, &channel, 100);

    let (obj, data) = get_payment_channel(&db, &a.id, &object_id)
        .unwrap()
        .expect("channel present");
    assert_eq!(obj.object_id, object_id);
    assert_eq!(obj.owner_entity, a.id);
    assert_eq!(obj.object_type, MemoryObjectType::PaymentChannel);
    assert_eq!(data.party_a_entity_id, a.id);
    assert_eq!(data.party_b_entity_id, b.id);
    assert_eq!(data.deposit_a, DEFAULT_DEPOSIT_A);
    assert_eq!(data.deposit_b, DEFAULT_DEPOSIT_B);
    assert_eq!(data.status, PAYMENT_CHANNEL_STATUS_PROPOSED);
}

#[test]
fn get_payment_channel_returns_open_status_after_accept() {
    let mut db = MemKv::new();
    let a = make_entity(&mut db, [0x13u8; 32], [0x23u8; 32], PROPOSER_BALANCE);
    let b = make_entity(&mut db, [0x14u8; 32], [0x24u8; 32], COUNTERPARTY_BALANCE);
    let channel = sample_channel(&a, &b, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let object_id = propose_at(&mut db, &a, 0, &channel, 100);
    accept_at(&mut db, &b, 0, object_id, a.id, 200);

    let (_, data) = get_payment_channel(&db, &a.id, &object_id)
        .unwrap()
        .expect("channel present");
    assert_eq!(data.status, PAYMENT_CHANNEL_STATUS_OPEN);
    assert_eq!(data.balance_b, DEFAULT_DEPOSIT_B);
    assert_eq!(data.accepted_at_height, 200);
}

#[test]
fn get_payment_channel_returns_none_when_missing() {
    let mut db = MemKv::new();
    let a = make_entity(&mut db, [0x15u8; 32], [0x25u8; 32], PROPOSER_BALANCE);
    let bogus = [0xCCu8; 32];
    assert!(get_payment_channel(&db, &a.id, &bogus).unwrap().is_none());
}

#[test]
fn get_payment_channel_returns_none_for_wrong_type() {
    // ChainSummary memory object owned by `a`. get_payment_channel
    // resolves the envelope but rejects the type mismatch and
    // returns None.
    let mut db = MemKv::new();
    let a = make_entity(&mut db, [0x16u8; 32], [0x26u8; 32], PROPOSER_BALANCE);
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::ChainSummary,
        data: vec![0u8; 32],
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: a.id,
        pubkey: a.id,
        nonce: 0,
        fee: CREATE_FEE,
        payload,
        sig: [0u8; 64],
    };
    let object_id = apply_create_memory_object_tx(&mut db, &tx, 100).unwrap();
    assert!(get_payment_channel(&db, &a.id, &object_id)
        .unwrap()
        .is_none());
}

// ============================================================================
// 2. get_channels_by_party_a
// ============================================================================

#[test]
fn get_channels_by_party_a_returns_owned_channels_in_height_order() {
    let mut db = MemKv::new();
    let a = make_entity(&mut db, [0x17u8; 32], [0x27u8; 32], PROPOSER_BALANCE * 2);
    let b1 = make_entity(&mut db, [0x18u8; 32], [0x28u8; 32], COUNTERPARTY_BALANCE);
    let b2 = make_entity(&mut db, [0x19u8; 32], [0x29u8; 32], COUNTERPARTY_BALANCE);

    let ch1 = sample_channel(&a, &b1, 50_000, 30_000);
    let oid1 = propose_at(&mut db, &a, 0, &ch1, 100);
    let ch2 = sample_channel(&a, &b2, 80_000, 20_000);
    let oid2 = propose_at(&mut db, &a, 1, &ch2, 250);

    let results = get_channels_by_party_a(&db, &a.id, 0, u64::MAX).unwrap();
    assert_eq!(results.len(), 2);
    // Ordered ascending by created_at_height (BE key prefix scan).
    assert_eq!(results[0].0.object_id, oid1);
    assert_eq!(results[0].0.created_at, 100);
    assert_eq!(results[1].0.object_id, oid2);
    assert_eq!(results[1].0.created_at, 250);
    assert_eq!(results[0].1.party_b_entity_id, b1.id);
    assert_eq!(results[1].1.party_b_entity_id, b2.id);
}

#[test]
fn get_channels_by_party_a_filters_by_height_window() {
    let mut db = MemKv::new();
    let a = make_entity(&mut db, [0x1Au8; 32], [0x2Au8; 32], PROPOSER_BALANCE * 3);
    let b = make_entity(&mut db, [0x1Bu8; 32], [0x2Bu8; 32], COUNTERPARTY_BALANCE);

    let heights = [100u64, 250, 400, 800];
    for (i, h) in heights.iter().enumerate() {
        let ch = sample_channel(&a, &b, 10_000, 5_000);
        let _ = propose_at(&mut db, &a, i as u64, &ch, *h);
    }

    // Mid-range window: [200, 600] should capture heights 250 and 400.
    let mid = get_channels_by_party_a(&db, &a.id, 200, 600).unwrap();
    assert_eq!(mid.len(), 2);
    assert_eq!(mid[0].0.created_at, 250);
    assert_eq!(mid[1].0.created_at, 400);

    // Tight bracket: [100, 100] catches the first only.
    let single = get_channels_by_party_a(&db, &a.id, 100, 100).unwrap();
    assert_eq!(single.len(), 1);
    assert_eq!(single[0].0.created_at, 100);

    // Empty window above all heights.
    let none = get_channels_by_party_a(&db, &a.id, 1000, 2000).unwrap();
    assert!(none.is_empty());
}

#[test]
fn get_channels_by_party_a_returns_empty_for_unknown_entity() {
    let mut db = MemKv::new();
    let a = make_entity(&mut db, [0x1Cu8; 32], [0x2Cu8; 32], PROPOSER_BALANCE);
    let b = make_entity(&mut db, [0x1Du8; 32], [0x2Du8; 32], COUNTERPARTY_BALANCE);
    let ch = sample_channel(&a, &b, 50_000, 30_000);
    let _ = propose_at(&mut db, &a, 0, &ch, 100);

    let unknown = [0xFFu8; 32];
    let results = get_channels_by_party_a(&db, &unknown, 0, u64::MAX).unwrap();
    assert!(results.is_empty());
}

// ============================================================================
// 3. get_channels_by_party_b
// ============================================================================

#[test]
fn get_channels_by_party_b_resolves_via_embedded_owner() {
    // The by_party_b index value embeds the 32-byte party_a id, so
    // resolution should be O(1) per match without an expensive
    // by-type scan. Two different proposers point at the same
    // counterparty B; B's scan should return both.
    let mut db = MemKv::new();
    let a1 = make_entity(&mut db, [0x1Eu8; 32], [0x2Eu8; 32], PROPOSER_BALANCE);
    let a2 = make_entity(&mut db, [0x1Fu8; 32], [0x2Fu8; 32], PROPOSER_BALANCE);
    let b = make_entity(&mut db, [0x30u8; 32], [0x40u8; 32], COUNTERPARTY_BALANCE);

    let ch1 = sample_channel(&a1, &b, 30_000, 20_000);
    let oid1 = propose_at(&mut db, &a1, 0, &ch1, 100);
    let ch2 = sample_channel(&a2, &b, 40_000, 25_000);
    let oid2 = propose_at(&mut db, &a2, 0, &ch2, 250);

    let results = get_channels_by_party_b(&db, &b.id, 0, u64::MAX).unwrap();
    assert_eq!(results.len(), 2);
    // Height-ordered; first is from a1 at height 100, second from a2 at 250.
    assert_eq!(results[0].0.object_id, oid1);
    assert_eq!(results[0].0.owner_entity, a1.id);
    assert_eq!(results[0].1.party_a_entity_id, a1.id);
    assert_eq!(results[1].0.object_id, oid2);
    assert_eq!(results[1].0.owner_entity, a2.id);
    assert_eq!(results[1].1.party_a_entity_id, a2.id);
    // Both list B as party_b.
    assert_eq!(results[0].1.party_b_entity_id, b.id);
    assert_eq!(results[1].1.party_b_entity_id, b.id);
}

#[test]
fn get_channels_by_party_b_filters_by_height_window() {
    let mut db = MemKv::new();
    let a = make_entity(&mut db, [0x31u8; 32], [0x41u8; 32], PROPOSER_BALANCE * 3);
    let b = make_entity(&mut db, [0x32u8; 32], [0x42u8; 32], COUNTERPARTY_BALANCE);

    for (i, h) in [150u64, 300, 600].iter().enumerate() {
        let ch = sample_channel(&a, &b, 20_000, 10_000);
        let _ = propose_at(&mut db, &a, i as u64, &ch, *h);
    }

    let mid = get_channels_by_party_b(&db, &b.id, 200, 500).unwrap();
    assert_eq!(mid.len(), 1);
    assert_eq!(mid[0].0.created_at, 300);

    let full = get_channels_by_party_b(&db, &b.id, 0, u64::MAX).unwrap();
    assert_eq!(full.len(), 3);
}

#[test]
fn get_channels_by_party_b_returns_empty_for_unknown_entity() {
    let mut db = MemKv::new();
    let a = make_entity(&mut db, [0x33u8; 32], [0x43u8; 32], PROPOSER_BALANCE);
    let b = make_entity(&mut db, [0x34u8; 32], [0x44u8; 32], COUNTERPARTY_BALANCE);
    let ch = sample_channel(&a, &b, 10_000, 5_000);
    let _ = propose_at(&mut db, &a, 0, &ch, 100);

    let unknown = [0xEEu8; 32];
    let results = get_channels_by_party_b(&db, &unknown, 0, u64::MAX).unwrap();
    assert!(results.is_empty());
}

// ============================================================================
// 4. Cross-helper consistency
// ============================================================================

#[test]
fn channel_appears_in_both_party_lists() {
    let mut db = MemKv::new();
    let a = make_entity(&mut db, [0x35u8; 32], [0x45u8; 32], PROPOSER_BALANCE);
    let b = make_entity(&mut db, [0x36u8; 32], [0x46u8; 32], COUNTERPARTY_BALANCE);
    let ch = sample_channel(&a, &b, 60_000, 40_000);
    let object_id = propose_at(&mut db, &a, 0, &ch, 500);

    let by_a = get_channels_by_party_a(&db, &a.id, 0, u64::MAX).unwrap();
    let by_b = get_channels_by_party_b(&db, &b.id, 0, u64::MAX).unwrap();
    assert_eq!(by_a.len(), 1);
    assert_eq!(by_b.len(), 1);
    assert_eq!(by_a[0].0.object_id, object_id);
    assert_eq!(by_b[0].0.object_id, object_id);
    // Both views resolve to the same primary record.
    assert_eq!(by_a[0].0.owner_entity, by_b[0].0.owner_entity);
    assert_eq!(by_a[0].1, by_b[0].1);
}
