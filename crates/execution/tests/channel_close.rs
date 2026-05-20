#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::too_many_arguments)]

//! Integration tests for the Week 32 `ChannelClose` signal handler
//! (Phase 4).
//!
//! Sub-flows exercised:
//! - Cooperative settle (is_final = 1): instant credit to both
//!   parties' economic_balance plus channel memory object + index
//!   teardown in the same atomic batch.
//! - Unilateral close at the initial state (nonce = 0): status flips
//!   PROPOSED -> OPEN ... -> CLOSING, deadline set, balances
//!   recorded as deposits.
//! - Unilateral close with a signed mid-channel update (nonce > 0):
//!   same status flip but the recorded state reflects the off-chain
//!   shift.
//! - Dispute inside the window: a strictly larger nonce overrides
//!   the recorded state without resetting the deadline.
//! - Dispute with a stale nonce: rejected.
//! - Close after the deadline: rejected (finalize-only past that
//!   point).
//! - Signature defences: invalid sig_a / sig_b, balance imbalance,
//!   not-a-participant, initial-state mismatch, channel not found,
//!   PROPOSED status close.
//! - Replay defences: signature binds chain_id and is_final flag,
//!   so updates from another chain or a mid-channel snapshot
//!   marked as cooperative-settle fail verification.

use novai_ai_entities::{
    AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObjectType, PaymentChannelData,
    CHANNEL_DISPUTE_WINDOW_DEFAULT_BLOCKS, PAYMENT_CHANNEL_RESERVED_LEN, PAYMENT_CHANNEL_SIZE,
    PAYMENT_CHANNEL_STATUS_CLOSING, PAYMENT_CHANNEL_STATUS_PROPOSED, PAYMENT_CHANNEL_V1,
};
use novai_crypto::{sign_channel_state, SigningKey};
use novai_execution::{
    apply_create_memory_object_tx, apply_signal_commitment_tx, channel_by_party_a_key,
    channel_by_party_b_key, encode_create_memory_object_payload_v1,
    encode_signal_commitment_payload_v1, write_ai_entity_op, ChannelAcceptExtraV1,
    ChannelCloseExtraV1, CreateMemoryObjectPayloadV1, ExecError, SignalCommitmentPayloadV1,
    CHANNEL_CLOSE_IS_FINAL, CHANNEL_CLOSE_NOT_FINAL, NOVAI_CHANNEL_CHAIN_ID,
};
use novai_state::{ai_entity_by_address_key, ai_memory_object_key, Kv, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const PROPOSER_BALANCE: u128 = 10_000_000;
const COUNTERPARTY_BALANCE: u128 = 10_000_000;
const CREATE_FEE: u64 = 1_000;
const ACCEPT_FEE: u64 = 1_000;
const CLOSE_FEE: u64 = 1_000;
const HEIGHT_PROPOSE: u64 = 500;
const HEIGHT_ACCEPT: u64 = 700;
const HEIGHT_CLOSE: u64 = 900;
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
        _reserved: [false; 2],
    }
}

/// Test fixture: an entity paired with the signing key whose
/// `verifying_key()` was registered as the entity's pubkey. The
/// entity's on-chain id is computed from `code_hash || creator`
/// (independent of the pubkey), so signing-key seed and entity id
/// are unrelated; this matches the production design where the
/// signing operator is decoupled from the entity identity.
struct KeyedEntity {
    entity: AiEntity,
    sk: SigningKey,
}

fn make_keyed_entity(
    db: &mut MemKv,
    code_hash: [u8; 32],
    creator: [u8; 32],
    seed: [u8; 32],
    balance: u128,
) -> KeyedEntity {
    let sk = SigningKey::from_bytes(&seed);
    let pubkey = sk.verifying_key().to_bytes();
    let mut entity = AiEntity::new_with_pubkey(
        code_hash,
        creator,
        AutonomyMode::Gated,
        caps(),
        pubkey,
        1000,
    );
    entity.economic_balance = balance;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();
    KeyedEntity { entity, sk }
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

fn accept(
    db: &mut MemKv,
    party_b: &AiEntity,
    nonce: u64,
    channel_object_id: [u8; 32],
    party_a_id: [u8; 32],
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
    apply_signal_commitment_tx(db, &tx, HEIGHT_ACCEPT).expect("accept succeeds");
}

fn make_close_tx(
    submitter: &AiEntity,
    nonce_for_tx: u64,
    channel_object_id: [u8; 32],
    party_a_id: [u8; 32],
    state_nonce: u64,
    balance_a: u128,
    balance_b: u128,
    is_final: u8,
    sig_a: [u8; 64],
    sig_b: [u8; 64],
    signal_hash: [u8; 32],
) -> TxV1 {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type: AiSignalType::ChannelClose,
        issuer_entity_id: submitter.id,
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
        channel_accept: None,
        channel_close: Some(ChannelCloseExtraV1 {
            channel_object_id,
            party_a_entity_id: party_a_id,
            nonce: state_nonce,
            balance_a,
            balance_b,
            is_final,
            sig_a,
            sig_b,
        }),
        channel_finalize: None,
    });
    TxV1 {
        version: TxVersion::V1,
        from: submitter.id,
        pubkey: submitter.id,
        nonce: nonce_for_tx,
        fee: CLOSE_FEE,
        payload,
        sig: [0u8; 64],
    }
}

fn sign_state(
    sk: &SigningKey,
    channel_id: &[u8; 32],
    party_a: &[u8; 32],
    party_b: &[u8; 32],
    state_nonce: u64,
    balance_a: u128,
    balance_b: u128,
    is_final: bool,
) -> [u8; 64] {
    sign_channel_state(
        sk,
        NOVAI_CHANNEL_CHAIN_ID,
        channel_id,
        party_a,
        party_b,
        state_nonce,
        balance_a,
        balance_b,
        is_final,
    )
}

fn read_channel(db: &MemKv, party_a: &[u8; 32], object_id: &[u8; 32]) -> PaymentChannelData {
    let envelope = db
        .get(&ai_memory_object_key(party_a, object_id))
        .unwrap()
        .expect("channel envelope present");
    let payload_start = envelope.len() - PAYMENT_CHANNEL_SIZE;
    PaymentChannelData::decode(&envelope[payload_start..]).expect("stored bytes decode")
}

fn entity_balance(db: &MemKv, id: &[u8; 32]) -> u128 {
    novai_execution::lookup_ai_entity_by_address(db, id)
        .unwrap()
        .unwrap()
        .economic_balance
}

// ============================================================================
// Setup helper: returns an opened channel + the two keyed parties.
// ============================================================================
fn setup_open_channel(
    db: &mut MemKv,
    a_seed: [u8; 32],
    b_seed: [u8; 32],
) -> (KeyedEntity, KeyedEntity, [u8; 32]) {
    let a = make_keyed_entity(db, [0x11u8; 32], [0x21u8; 32], a_seed, PROPOSER_BALANCE);
    let b = make_keyed_entity(db, [0x12u8; 32], [0x22u8; 32], b_seed, COUNTERPARTY_BALANCE);
    let channel = sample_channel(&a.entity, &b.entity, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let object_id = propose(db, &a.entity, 0, &channel);
    accept(db, &b.entity, 0, object_id, a.entity.id);
    (a, b, object_id)
}

// ============================================================================
// 1. Cooperative settle (is_final = 1)
// ============================================================================

#[test]
fn channel_cooperative_settle_credits_and_deletes() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x07u8; 32], [0x08u8; 32]);

    // Off-chain: agree on final state (A pays B 25k after one
    // round-trip). Both parties sign with is_final = true.
    let nonce: u64 = 1;
    let balance_a = DEFAULT_DEPOSIT_A - 25_000;
    let balance_b = DEFAULT_DEPOSIT_B + 25_000;
    let sig_a = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        true,
    );
    let sig_b = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        true,
    );

    // A submits the close.
    let a_before = entity_balance(&db, &a.entity.id);
    let b_before = entity_balance(&db, &b.entity.id);
    let tx = make_close_tx(
        &a.entity,
        1, // A's create nonce was 0, so close uses 1
        object_id,
        a.entity.id,
        nonce,
        balance_a,
        balance_b,
        CHANNEL_CLOSE_IS_FINAL,
        sig_a,
        sig_b,
        [0xAAu8; 32],
    );
    apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).expect("settle succeeds");

    // Primary record + all indexes are gone.
    assert!(db
        .get(&ai_memory_object_key(&a.entity.id, &object_id))
        .unwrap()
        .is_none());
    assert!(db
        .get(&channel_by_party_a_key(
            &a.entity.id,
            HEIGHT_PROPOSE,
            &object_id
        ))
        .unwrap()
        .is_none());
    assert!(db
        .get(&channel_by_party_b_key(
            &b.entity.id,
            HEIGHT_PROPOSE,
            &object_id
        ))
        .unwrap()
        .is_none());

    // Balances credited: A got balance_a back, B got balance_b. A
    // also paid the close fee.
    let a_after = entity_balance(&db, &a.entity.id);
    let b_after = entity_balance(&db, &b.entity.id);
    assert_eq!(a_after, a_before + balance_a - u128::from(CLOSE_FEE));
    assert_eq!(b_after, b_before + balance_b);
}

#[test]
fn channel_cooperative_settle_works_when_b_submits() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x09u8; 32], [0x0Au8; 32]);

    let nonce: u64 = 1;
    let balance_a = DEFAULT_DEPOSIT_A + 10_000;
    let balance_b = DEFAULT_DEPOSIT_B - 10_000;
    let sig_a = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        true,
    );
    let sig_b = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        true,
    );

    let a_before = entity_balance(&db, &a.entity.id);
    let b_before = entity_balance(&db, &b.entity.id);
    // B submits the close (B's accept nonce was 0, so close uses 1).
    let tx = make_close_tx(
        &b.entity,
        1,
        object_id,
        a.entity.id,
        nonce,
        balance_a,
        balance_b,
        CHANNEL_CLOSE_IS_FINAL,
        sig_a,
        sig_b,
        [0xBBu8; 32],
    );
    apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).expect("settle succeeds");

    let a_after = entity_balance(&db, &a.entity.id);
    let b_after = entity_balance(&db, &b.entity.id);
    assert_eq!(a_after, a_before + balance_a);
    assert_eq!(b_after, b_before + balance_b - u128::from(CLOSE_FEE));
}

// ============================================================================
// 2. Unilateral close opens dispute window
// ============================================================================

#[test]
fn channel_unilateral_close_initial_state_flips_to_closing() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x0Bu8; 32], [0x0Cu8; 32]);

    // Nonce-0 initial-state close: balances must equal deposits.
    let sig_a = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        0,
        DEFAULT_DEPOSIT_A,
        DEFAULT_DEPOSIT_B,
        false,
    );
    let sig_b = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        0,
        DEFAULT_DEPOSIT_A,
        DEFAULT_DEPOSIT_B,
        false,
    );

    let tx = make_close_tx(
        &a.entity,
        1,
        object_id,
        a.entity.id,
        0,
        DEFAULT_DEPOSIT_A,
        DEFAULT_DEPOSIT_B,
        CHANNEL_CLOSE_NOT_FINAL,
        sig_a,
        sig_b,
        [0xCCu8; 32],
    );
    apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).expect("close succeeds");

    let stored = read_channel(&db, &a.entity.id, &object_id);
    assert_eq!(stored.status, PAYMENT_CHANNEL_STATUS_CLOSING);
    assert_eq!(stored.closing_at_height, HEIGHT_CLOSE);
    assert_eq!(
        stored.dispute_deadline_height,
        HEIGHT_CLOSE + u64::from(CHANNEL_DISPUTE_WINDOW_DEFAULT_BLOCKS)
    );
    assert_eq!(stored.balance_a, DEFAULT_DEPOSIT_A);
    assert_eq!(stored.balance_b, DEFAULT_DEPOSIT_B);
    assert_eq!(stored.nonce, 0);
}

#[test]
fn channel_unilateral_close_with_signed_update_records_state() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x0Du8; 32], [0x0Eu8; 32]);

    // Off-chain update at nonce 5: A paid B 40k.
    let nonce: u64 = 5;
    let balance_a = DEFAULT_DEPOSIT_A - 40_000;
    let balance_b = DEFAULT_DEPOSIT_B + 40_000;
    let sig_a = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        false,
    );
    let sig_b = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        false,
    );

    let tx = make_close_tx(
        &b.entity,
        1,
        object_id,
        a.entity.id,
        nonce,
        balance_a,
        balance_b,
        CHANNEL_CLOSE_NOT_FINAL,
        sig_a,
        sig_b,
        [0xDDu8; 32],
    );
    apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).expect("close succeeds");

    let stored = read_channel(&db, &a.entity.id, &object_id);
    assert_eq!(stored.status, PAYMENT_CHANNEL_STATUS_CLOSING);
    assert_eq!(stored.nonce, nonce);
    assert_eq!(stored.balance_a, balance_a);
    assert_eq!(stored.balance_b, balance_b);
    assert_eq!(stored.closing_at_height, HEIGHT_CLOSE);
}

// ============================================================================
// 3. Dispute mechanics inside the window
// ============================================================================

#[test]
fn channel_dispute_with_higher_nonce_overrides() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x0Fu8; 32], [0x10u8; 32]);

    // First close: A submits stale state at nonce 5 favouring A.
    let stale_nonce: u64 = 5;
    let stale_a = DEFAULT_DEPOSIT_A + 30_000;
    let stale_b = DEFAULT_DEPOSIT_B - 30_000;
    let sig_a1 = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        stale_nonce,
        stale_a,
        stale_b,
        false,
    );
    let sig_b1 = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        stale_nonce,
        stale_a,
        stale_b,
        false,
    );
    let tx1 = make_close_tx(
        &a.entity,
        1,
        object_id,
        a.entity.id,
        stale_nonce,
        stale_a,
        stale_b,
        CHANNEL_CLOSE_NOT_FINAL,
        sig_a1,
        sig_b1,
        [0x01u8; 32],
    );
    apply_signal_commitment_tx(&mut db, &tx1, HEIGHT_CLOSE).expect("first close");

    // B disputes with a fresher state at nonce 10 favouring B.
    let fresh_nonce: u64 = 10;
    let fresh_a = DEFAULT_DEPOSIT_A - 80_000;
    let fresh_b = DEFAULT_DEPOSIT_B + 80_000;
    let sig_a2 = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        fresh_nonce,
        fresh_a,
        fresh_b,
        false,
    );
    let sig_b2 = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        fresh_nonce,
        fresh_a,
        fresh_b,
        false,
    );
    let tx2 = make_close_tx(
        &b.entity,
        1,
        object_id,
        a.entity.id,
        fresh_nonce,
        fresh_a,
        fresh_b,
        CHANNEL_CLOSE_NOT_FINAL,
        sig_a2,
        sig_b2,
        [0x02u8; 32],
    );
    apply_signal_commitment_tx(&mut db, &tx2, HEIGHT_CLOSE + 10).expect("dispute succeeds");

    let stored = read_channel(&db, &a.entity.id, &object_id);
    assert_eq!(stored.nonce, fresh_nonce);
    assert_eq!(stored.balance_a, fresh_a);
    assert_eq!(stored.balance_b, fresh_b);
    // Dispute deadline preserved (cheater cannot extend the window).
    assert_eq!(
        stored.dispute_deadline_height,
        HEIGHT_CLOSE + u64::from(CHANNEL_DISPUTE_WINDOW_DEFAULT_BLOCKS)
    );
    assert_eq!(stored.closing_at_height, HEIGHT_CLOSE);
}

#[test]
fn channel_dispute_with_lower_nonce_rejected() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x11u8; 32], [0x12u8; 32]);

    // First close at nonce 5.
    let nonce_first: u64 = 5;
    let sig_a1 = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce_first,
        DEFAULT_DEPOSIT_A - 50_000,
        DEFAULT_DEPOSIT_B + 50_000,
        false,
    );
    let sig_b1 = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce_first,
        DEFAULT_DEPOSIT_A - 50_000,
        DEFAULT_DEPOSIT_B + 50_000,
        false,
    );
    let tx1 = make_close_tx(
        &a.entity,
        1,
        object_id,
        a.entity.id,
        nonce_first,
        DEFAULT_DEPOSIT_A - 50_000,
        DEFAULT_DEPOSIT_B + 50_000,
        CHANNEL_CLOSE_NOT_FINAL,
        sig_a1,
        sig_b1,
        [0x03u8; 32],
    );
    apply_signal_commitment_tx(&mut db, &tx1, HEIGHT_CLOSE).expect("first close");

    // Try to dispute with an older nonce 3 (lower than 5).
    let nonce_stale: u64 = 3;
    let bal_a_stale = DEFAULT_DEPOSIT_A - 10_000;
    let bal_b_stale = DEFAULT_DEPOSIT_B + 10_000;
    let sig_a2 = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce_stale,
        bal_a_stale,
        bal_b_stale,
        false,
    );
    let sig_b2 = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce_stale,
        bal_a_stale,
        bal_b_stale,
        false,
    );
    let tx2 = make_close_tx(
        &b.entity,
        1,
        object_id,
        a.entity.id,
        nonce_stale,
        bal_a_stale,
        bal_b_stale,
        CHANNEL_CLOSE_NOT_FINAL,
        sig_a2,
        sig_b2,
        [0x04u8; 32],
    );
    let err = apply_signal_commitment_tx(&mut db, &tx2, HEIGHT_CLOSE + 5).unwrap_err();
    match err {
        ExecError::ChannelCloseNonceNotMonotonic { current, attempted } => {
            assert_eq!(current, nonce_first);
            assert_eq!(attempted, nonce_stale);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn channel_dispute_after_deadline_rejected() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x13u8; 32], [0x14u8; 32]);

    // First close at nonce 1.
    let nonce_first: u64 = 1;
    let balance_a = DEFAULT_DEPOSIT_A - 1000;
    let balance_b = DEFAULT_DEPOSIT_B + 1000;
    let sig_a = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce_first,
        balance_a,
        balance_b,
        false,
    );
    let sig_b = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce_first,
        balance_a,
        balance_b,
        false,
    );
    let tx1 = make_close_tx(
        &a.entity,
        1,
        object_id,
        a.entity.id,
        nonce_first,
        balance_a,
        balance_b,
        CHANNEL_CLOSE_NOT_FINAL,
        sig_a,
        sig_b,
        [0x05u8; 32],
    );
    apply_signal_commitment_tx(&mut db, &tx1, HEIGHT_CLOSE).expect("first close");

    // Try to dispute with nonce 2 AFTER the deadline.
    let nonce_late: u64 = 2;
    let bal_a_late = DEFAULT_DEPOSIT_A - 2000;
    let bal_b_late = DEFAULT_DEPOSIT_B + 2000;
    let sig_a2 = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce_late,
        bal_a_late,
        bal_b_late,
        false,
    );
    let sig_b2 = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce_late,
        bal_a_late,
        bal_b_late,
        false,
    );
    let tx2 = make_close_tx(
        &b.entity,
        1,
        object_id,
        a.entity.id,
        nonce_late,
        bal_a_late,
        bal_b_late,
        CHANNEL_CLOSE_NOT_FINAL,
        sig_a2,
        sig_b2,
        [0x06u8; 32],
    );
    let past_deadline = HEIGHT_CLOSE + u64::from(CHANNEL_DISPUTE_WINDOW_DEFAULT_BLOCKS) + 1;
    let err = apply_signal_commitment_tx(&mut db, &tx2, past_deadline).unwrap_err();
    assert!(matches!(err, ExecError::ChannelCloseAfterDeadline { .. }));
}

// ============================================================================
// 4. Defensive rejections
// ============================================================================

#[test]
fn channel_close_rejects_invalid_sig_a() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x15u8; 32], [0x16u8; 32]);

    let nonce: u64 = 1;
    let balance_a = DEFAULT_DEPOSIT_A;
    let balance_b = DEFAULT_DEPOSIT_B;
    // Forge sig_a by signing with B's key, then place it in the sig_a slot.
    let bogus_sig_a = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        false,
    );
    let sig_b = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        false,
    );
    let tx = make_close_tx(
        &a.entity,
        1,
        object_id,
        a.entity.id,
        nonce,
        balance_a,
        balance_b,
        CHANNEL_CLOSE_NOT_FINAL,
        bogus_sig_a,
        sig_b,
        [0x07u8; 32],
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).unwrap_err();
    assert!(matches!(err, ExecError::ChannelCloseInvalidSignatureA));
}

#[test]
fn channel_close_rejects_invalid_sig_b() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x17u8; 32], [0x18u8; 32]);

    let nonce: u64 = 1;
    let balance_a = DEFAULT_DEPOSIT_A;
    let balance_b = DEFAULT_DEPOSIT_B;
    let sig_a = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        false,
    );
    // sig_b signed with A's key (wrong).
    let bogus_sig_b = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        false,
    );
    let tx = make_close_tx(
        &a.entity,
        1,
        object_id,
        a.entity.id,
        nonce,
        balance_a,
        balance_b,
        CHANNEL_CLOSE_NOT_FINAL,
        sig_a,
        bogus_sig_b,
        [0x08u8; 32],
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).unwrap_err();
    assert!(matches!(err, ExecError::ChannelCloseInvalidSignatureB));
}

#[test]
fn channel_close_rejects_balance_imbalance() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x19u8; 32], [0x1Au8; 32]);

    // balance_a + balance_b != deposit_a + deposit_b. Sign whatever
    // bytes we have anyway so the balance-invariant check is the one
    // that rejects.
    let nonce: u64 = 1;
    let bad_a = DEFAULT_DEPOSIT_A;
    let bad_b = DEFAULT_DEPOSIT_B + 1; // off by one
    let sig_a = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        bad_a,
        bad_b,
        false,
    );
    let sig_b = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        bad_a,
        bad_b,
        false,
    );
    let tx = make_close_tx(
        &a.entity,
        1,
        object_id,
        a.entity.id,
        nonce,
        bad_a,
        bad_b,
        CHANNEL_CLOSE_NOT_FINAL,
        sig_a,
        sig_b,
        [0x09u8; 32],
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).unwrap_err();
    assert!(matches!(
        err,
        ExecError::ChannelCloseBalanceImbalance { .. }
    ));
}

#[test]
fn channel_close_rejects_submitter_not_participant() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x1Bu8; 32], [0x1Cu8; 32]);
    let intruder = make_keyed_entity(
        &mut db,
        [0x1Du8; 32],
        [0x2Du8; 32],
        [0x1Eu8; 32],
        PROPOSER_BALANCE,
    );

    let nonce: u64 = 1;
    let balance_a = DEFAULT_DEPOSIT_A;
    let balance_b = DEFAULT_DEPOSIT_B;
    let sig_a = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        false,
    );
    let sig_b = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        false,
    );
    let tx = make_close_tx(
        &intruder.entity,
        0,
        object_id,
        a.entity.id,
        nonce,
        balance_a,
        balance_b,
        CHANNEL_CLOSE_NOT_FINAL,
        sig_a,
        sig_b,
        [0x0Au8; 32],
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).unwrap_err();
    assert!(matches!(
        err,
        ExecError::ChannelCloseSubmitterNotParticipant
    ));
}

#[test]
fn channel_close_rejects_initial_state_mismatch() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x1Fu8; 32], [0x20u8; 32]);

    // nonce-0 close but balances do NOT match deposits.
    let bad_a = DEFAULT_DEPOSIT_A - 1;
    let bad_b = DEFAULT_DEPOSIT_B + 1;
    let sig_a = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        0,
        bad_a,
        bad_b,
        false,
    );
    let sig_b = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        0,
        bad_a,
        bad_b,
        false,
    );
    let tx = make_close_tx(
        &a.entity,
        1,
        object_id,
        a.entity.id,
        0,
        bad_a,
        bad_b,
        CHANNEL_CLOSE_NOT_FINAL,
        sig_a,
        sig_b,
        [0x0Bu8; 32],
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).unwrap_err();
    assert!(matches!(err, ExecError::ChannelCloseInitialStateMismatch));
}

#[test]
fn channel_close_rejects_proposed_status() {
    // Create but DO NOT accept; the channel stays in PROPOSED.
    let mut db = MemKv::new();
    let a = make_keyed_entity(
        &mut db,
        [0x21u8; 32],
        [0x31u8; 32],
        [0x22u8; 32],
        PROPOSER_BALANCE,
    );
    let b = make_keyed_entity(
        &mut db,
        [0x23u8; 32],
        [0x33u8; 32],
        [0x24u8; 32],
        COUNTERPARTY_BALANCE,
    );
    let channel = sample_channel(&a.entity, &b.entity, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let object_id = propose(&mut db, &a.entity, 0, &channel);

    // Attempt to close immediately at nonce 0.
    let sig_a = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        0,
        DEFAULT_DEPOSIT_A,
        DEFAULT_DEPOSIT_B,
        false,
    );
    let sig_b = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        0,
        DEFAULT_DEPOSIT_A,
        DEFAULT_DEPOSIT_B,
        false,
    );
    let tx = make_close_tx(
        &a.entity,
        1,
        object_id,
        a.entity.id,
        0,
        DEFAULT_DEPOSIT_A,
        DEFAULT_DEPOSIT_B,
        CHANNEL_CLOSE_NOT_FINAL,
        sig_a,
        sig_b,
        [0x0Cu8; 32],
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).unwrap_err();
    match err {
        ExecError::ChannelCloseInvalidStatus { status } => {
            assert_eq!(status, PAYMENT_CHANNEL_STATUS_PROPOSED);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn channel_close_rejects_when_channel_not_found() {
    let mut db = MemKv::new();
    let a = make_keyed_entity(
        &mut db,
        [0x25u8; 32],
        [0x35u8; 32],
        [0x26u8; 32],
        PROPOSER_BALANCE,
    );
    let bogus = [0xAAu8; 32];
    let sig = [0u8; 64];
    let tx = make_close_tx(
        &a.entity,
        0,
        bogus,
        a.entity.id,
        0,
        DEFAULT_DEPOSIT_A,
        DEFAULT_DEPOSIT_B,
        CHANNEL_CLOSE_NOT_FINAL,
        sig,
        sig,
        [0x0Du8; 32],
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).unwrap_err();
    assert!(matches!(err, ExecError::ChannelCloseNotFound));
}

// ============================================================================
// 5. Replay defences
// ============================================================================

#[test]
fn channel_close_signature_binds_chain_id() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x27u8; 32], [0x28u8; 32]);

    // Sign with a different chain_id, then submit (canonical chain
    // id verification path uses NOVAI_CHANNEL_CHAIN_ID).
    let nonce: u64 = 1;
    let balance_a = DEFAULT_DEPOSIT_A;
    let balance_b = DEFAULT_DEPOSIT_B;
    let wrong_chain: u64 = NOVAI_CHANNEL_CHAIN_ID + 999;
    let sig_a = sign_channel_state(
        &a.sk,
        wrong_chain,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        false,
    );
    let sig_b = sign_channel_state(
        &b.sk,
        wrong_chain,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        false,
    );
    let tx = make_close_tx(
        &a.entity,
        1,
        object_id,
        a.entity.id,
        nonce,
        balance_a,
        balance_b,
        CHANNEL_CLOSE_NOT_FINAL,
        sig_a,
        sig_b,
        [0x0Eu8; 32],
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).unwrap_err();
    assert!(matches!(err, ExecError::ChannelCloseInvalidSignatureA));
}

#[test]
fn channel_close_signature_binds_is_final_flag() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x29u8; 32], [0x2Au8; 32]);

    // Parties signed an is_final = false (mid-channel) state. Closer
    // tries to submit with is_final = true (cooperative settle flag)
    // — sig verification fails because the canonical bytes differ.
    let nonce: u64 = 1;
    let balance_a = DEFAULT_DEPOSIT_A - 5000;
    let balance_b = DEFAULT_DEPOSIT_B + 5000;
    let sig_a = sign_state(
        &a.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        false,
    );
    let sig_b = sign_state(
        &b.sk,
        &object_id,
        &a.entity.id,
        &b.entity.id,
        nonce,
        balance_a,
        balance_b,
        false,
    );
    let tx = make_close_tx(
        &a.entity,
        1,
        object_id,
        a.entity.id,
        nonce,
        balance_a,
        balance_b,
        CHANNEL_CLOSE_IS_FINAL,
        sig_a,
        sig_b,
        [0x0Fu8; 32],
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).unwrap_err();
    assert!(matches!(err, ExecError::ChannelCloseInvalidSignatureA));
}
