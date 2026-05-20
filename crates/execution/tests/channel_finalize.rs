#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::too_many_arguments)]

//! Integration tests for the Week 32 `ChannelFinalize` signal
//! handler (Phase 5).
//!
//! ChannelFinalize is the permissionless teardown signal for a
//! PaymentChannel whose dispute window has expired. Any active AI
//! entity may submit it (the parties have aligned incentives, but
//! third-party finalize means liveness does not depend on either
//! participant staying online).
//!
//! Sub-flows exercised:
//! - Happy path: party A submits at deadline+1, channel deleted,
//!   balances credited per the recorded state.
//! - Permissionless: a third party submits, both participants are
//!   credited, the third party pays only the tx fee.
//! - End-to-end: propose -> accept -> off-chain update -> unilateral
//!   close -> finalize after window expires.
//! - Defensive rejections: not-CLOSING status, before deadline,
//!   not-found, wrong type, idempotency on second finalize.

use novai_ai_entities::{
    AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObjectType, PaymentChannelData,
    CHANNEL_DISPUTE_WINDOW_DEFAULT_BLOCKS, PAYMENT_CHANNEL_RESERVED_LEN,
    PAYMENT_CHANNEL_STATUS_PROPOSED, PAYMENT_CHANNEL_V1,
};
use novai_crypto::{sign_channel_state, SigningKey};
use novai_execution::{
    apply_create_memory_object_tx, apply_signal_commitment_tx, channel_by_party_a_key,
    channel_by_party_b_key, encode_create_memory_object_payload_v1,
    encode_signal_commitment_payload_v1, write_ai_entity_op, ChannelAcceptExtraV1,
    ChannelCloseExtraV1, ChannelFinalizeExtraV1, CreateMemoryObjectPayloadV1, ExecError,
    SignalCommitmentPayloadV1, CHANNEL_CLOSE_NOT_FINAL, NOVAI_CHANNEL_CHAIN_ID,
};
use novai_state::{ai_entity_by_address_key, ai_memory_object_key, Kv, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const PROPOSER_BALANCE: u128 = 10_000_000;
const COUNTERPARTY_BALANCE: u128 = 10_000_000;
const THIRD_PARTY_BALANCE: u128 = 5_000_000;
const CREATE_FEE: u64 = 1_000;
const ACCEPT_FEE: u64 = 1_000;
const CLOSE_FEE: u64 = 1_000;
const FINALIZE_FEE: u64 = 1_000;
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

fn close_unilateral(
    db: &mut MemKv,
    submitter: &KeyedEntity,
    submitter_tx_nonce: u64,
    object_id: [u8; 32],
    party_a: &KeyedEntity,
    party_b: &KeyedEntity,
    state_nonce: u64,
    balance_a: u128,
    balance_b: u128,
    signal_hash: [u8; 32],
) {
    let sig_a = sign_channel_state(
        &party_a.sk,
        NOVAI_CHANNEL_CHAIN_ID,
        &object_id,
        &party_a.entity.id,
        &party_b.entity.id,
        state_nonce,
        balance_a,
        balance_b,
        false,
    );
    let sig_b = sign_channel_state(
        &party_b.sk,
        NOVAI_CHANNEL_CHAIN_ID,
        &object_id,
        &party_a.entity.id,
        &party_b.entity.id,
        state_nonce,
        balance_a,
        balance_b,
        false,
    );
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type: AiSignalType::ChannelClose,
        issuer_entity_id: submitter.entity.id,
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
            channel_object_id: object_id,
            party_a_entity_id: party_a.entity.id,
            nonce: state_nonce,
            balance_a,
            balance_b,
            is_final: CHANNEL_CLOSE_NOT_FINAL,
            sig_a,
            sig_b,
        }),
        channel_finalize: None,
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: submitter.entity.id,
        pubkey: submitter.entity.id,
        nonce: submitter_tx_nonce,
        fee: CLOSE_FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_signal_commitment_tx(db, &tx, HEIGHT_CLOSE).expect("close succeeds");
}

fn make_finalize_tx(
    submitter: &AiEntity,
    nonce: u64,
    channel_object_id: [u8; 32],
    party_a_id: [u8; 32],
    signal_hash: [u8; 32],
) -> TxV1 {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type: AiSignalType::ChannelFinalize,
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
        channel_close: None,
        channel_finalize: Some(ChannelFinalizeExtraV1 {
            channel_object_id,
            party_a_entity_id: party_a_id,
        }),
    });
    TxV1 {
        version: TxVersion::V1,
        from: submitter.id,
        pubkey: submitter.id,
        nonce,
        fee: FINALIZE_FEE,
        payload,
        sig: [0u8; 64],
    }
}

fn entity_balance(db: &MemKv, id: &[u8; 32]) -> u128 {
    novai_execution::lookup_ai_entity_by_address(db, id)
        .unwrap()
        .unwrap()
        .economic_balance
}

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

fn past_deadline_height() -> u64 {
    HEIGHT_CLOSE + u64::from(CHANNEL_DISPUTE_WINDOW_DEFAULT_BLOCKS) + 1
}

// ============================================================================
// 1. Happy path
// ============================================================================

#[test]
fn channel_finalize_by_party_a_credits_both_and_deletes() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x07u8; 32], [0x08u8; 32]);

    // Unilateral close at nonce 3 (A paid B 30k).
    let bal_a = DEFAULT_DEPOSIT_A - 30_000;
    let bal_b = DEFAULT_DEPOSIT_B + 30_000;
    close_unilateral(
        &mut db,
        &a,
        1,
        object_id,
        &a,
        &b,
        3,
        bal_a,
        bal_b,
        [0x01u8; 32],
    );

    let a_before = entity_balance(&db, &a.entity.id);
    let b_before = entity_balance(&db, &b.entity.id);

    // Party A submits finalize after the deadline.
    let tx = make_finalize_tx(&a.entity, 2, object_id, a.entity.id, [0x02u8; 32]);
    apply_signal_commitment_tx(&mut db, &tx, past_deadline_height()).expect("finalize succeeds");

    // Primary record + indexes torn down.
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

    // A: got bal_a back, paid finalize fee.
    // B: got bal_b back (unchanged otherwise).
    let a_after = entity_balance(&db, &a.entity.id);
    let b_after = entity_balance(&db, &b.entity.id);
    assert_eq!(a_after, a_before + bal_a - u128::from(FINALIZE_FEE));
    assert_eq!(b_after, b_before + bal_b);
}

#[test]
fn channel_finalize_by_party_b_credits_both_and_deletes() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x09u8; 32], [0x0Au8; 32]);

    let bal_a = DEFAULT_DEPOSIT_A + 25_000;
    let bal_b = DEFAULT_DEPOSIT_B - 25_000;
    close_unilateral(
        &mut db,
        &a,
        1,
        object_id,
        &a,
        &b,
        7,
        bal_a,
        bal_b,
        [0x03u8; 32],
    );

    let a_before = entity_balance(&db, &a.entity.id);
    let b_before = entity_balance(&db, &b.entity.id);

    let tx = make_finalize_tx(&b.entity, 1, object_id, a.entity.id, [0x04u8; 32]);
    apply_signal_commitment_tx(&mut db, &tx, past_deadline_height()).expect("finalize succeeds");

    let a_after = entity_balance(&db, &a.entity.id);
    let b_after = entity_balance(&db, &b.entity.id);
    assert_eq!(a_after, a_before + bal_a);
    assert_eq!(b_after, b_before + bal_b - u128::from(FINALIZE_FEE));
}

// ============================================================================
// 2. Permissionless finalize by a third party
// ============================================================================

#[test]
fn channel_finalize_by_third_party_credits_participants_only() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x0Bu8; 32], [0x0Cu8; 32]);

    // Unrelated third-party entity. The third party has emit_proposals
    // capability (everyone in `caps()` does) and pays the tx fee.
    let watcher = make_keyed_entity(
        &mut db,
        [0x77u8; 32],
        [0x88u8; 32],
        [0x0Du8; 32],
        THIRD_PARTY_BALANCE,
    );

    let bal_a = DEFAULT_DEPOSIT_A - 10_000;
    let bal_b = DEFAULT_DEPOSIT_B + 10_000;
    close_unilateral(
        &mut db,
        &a,
        1,
        object_id,
        &a,
        &b,
        2,
        bal_a,
        bal_b,
        [0x05u8; 32],
    );

    let a_before = entity_balance(&db, &a.entity.id);
    let b_before = entity_balance(&db, &b.entity.id);
    let watcher_before = entity_balance(&db, &watcher.entity.id);

    let tx = make_finalize_tx(&watcher.entity, 0, object_id, a.entity.id, [0x06u8; 32]);
    apply_signal_commitment_tx(&mut db, &tx, past_deadline_height()).expect("third-party finalize");

    // Participants credited; third party only paid the fee.
    let a_after = entity_balance(&db, &a.entity.id);
    let b_after = entity_balance(&db, &b.entity.id);
    let watcher_after = entity_balance(&db, &watcher.entity.id);
    assert_eq!(a_after, a_before + bal_a);
    assert_eq!(b_after, b_before + bal_b);
    assert_eq!(watcher_after, watcher_before - u128::from(FINALIZE_FEE));
}

// ============================================================================
// 3. End-to-end lifecycle
// ============================================================================

#[test]
fn channel_full_lifecycle_propose_accept_close_finalize() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x0Eu8; 32], [0x0Fu8; 32]);

    // Pre-close: A paid CREATE_FEE + DEFAULT_DEPOSIT_A; B paid
    // ACCEPT_FEE + DEFAULT_DEPOSIT_B.
    let a_pre = entity_balance(&db, &a.entity.id);
    let b_pre = entity_balance(&db, &b.entity.id);
    assert_eq!(
        a_pre,
        PROPOSER_BALANCE - DEFAULT_DEPOSIT_A - u128::from(CREATE_FEE)
    );
    assert_eq!(
        b_pre,
        COUNTERPARTY_BALANCE - DEFAULT_DEPOSIT_B - u128::from(ACCEPT_FEE)
    );

    // Off-chain settled state: A paid B 75_000 over the channel's
    // lifetime. nonce = 12, balances reflect the cumulative shift.
    let final_a = DEFAULT_DEPOSIT_A - 75_000;
    let final_b = DEFAULT_DEPOSIT_B + 75_000;
    close_unilateral(
        &mut db,
        &a,
        1,
        object_id,
        &a,
        &b,
        12,
        final_a,
        final_b,
        [0x07u8; 32],
    );

    // After close: A paid CLOSE_FEE; B paid nothing extra. Funds are
    // still locked in the channel record.
    let a_after_close = entity_balance(&db, &a.entity.id);
    assert_eq!(a_after_close, a_pre - u128::from(CLOSE_FEE));

    // Finalize at deadline+1.
    let tx = make_finalize_tx(&a.entity, 2, object_id, a.entity.id, [0x08u8; 32]);
    apply_signal_commitment_tx(&mut db, &tx, past_deadline_height()).expect("finalize");

    // Final accounting: balances of both parties must equal their
    // pre-channel balance minus (own_deposit - own_share_at_close).
    // A: started with PROPOSER_BALANCE, paid CREATE_FEE +
    // CLOSE_FEE + FINALIZE_FEE, lost (DEFAULT_DEPOSIT_A - final_a) =
    // 75_000.
    let a_final = entity_balance(&db, &a.entity.id);
    let expected_a = PROPOSER_BALANCE
        - 75_000
        - u128::from(CREATE_FEE)
        - u128::from(CLOSE_FEE)
        - u128::from(FINALIZE_FEE);
    assert_eq!(a_final, expected_a);

    // B: started with COUNTERPARTY_BALANCE, paid ACCEPT_FEE, gained
    // (final_b - DEFAULT_DEPOSIT_B) = 75_000.
    let b_final = entity_balance(&db, &b.entity.id);
    let expected_b = COUNTERPARTY_BALANCE + 75_000 - u128::from(ACCEPT_FEE);
    assert_eq!(b_final, expected_b);

    // Conservation: total post-finalize == total pre-channel minus
    // all the on-chain fees that went to the fee pool.
    let total_pre = PROPOSER_BALANCE + COUNTERPARTY_BALANCE;
    let total_post = a_final + b_final;
    let fees_paid = u128::from(CREATE_FEE)
        + u128::from(ACCEPT_FEE)
        + u128::from(CLOSE_FEE)
        + u128::from(FINALIZE_FEE);
    assert_eq!(total_pre - fees_paid, total_post);
}

// ============================================================================
// 4. Defensive rejections
// ============================================================================

#[test]
fn channel_finalize_before_deadline_rejected() {
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x10u8; 32], [0x11u8; 32]);

    let bal_a = DEFAULT_DEPOSIT_A;
    let bal_b = DEFAULT_DEPOSIT_B;
    close_unilateral(
        &mut db,
        &a,
        1,
        object_id,
        &a,
        &b,
        0,
        bal_a,
        bal_b,
        [0x09u8; 32],
    );

    // Try to finalize AT the deadline (current_height ==
    // dispute_deadline_height; rule is current must be strictly
    // greater).
    let at_deadline = HEIGHT_CLOSE + u64::from(CHANNEL_DISPUTE_WINDOW_DEFAULT_BLOCKS);
    let tx = make_finalize_tx(&a.entity, 2, object_id, a.entity.id, [0x0Au8; 32]);
    let err = apply_signal_commitment_tx(&mut db, &tx, at_deadline).unwrap_err();
    assert!(matches!(
        err,
        ExecError::ChannelFinalizeBeforeDeadline { .. }
    ));
}

#[test]
fn channel_finalize_on_open_status_rejected() {
    // Channel was accepted but never closed; status is OPEN.
    let mut db = MemKv::new();
    let (a, _b, object_id) = setup_open_channel(&mut db, [0x12u8; 32], [0x13u8; 32]);

    let tx = make_finalize_tx(&a.entity, 1, object_id, a.entity.id, [0x0Bu8; 32]);
    let err = apply_signal_commitment_tx(&mut db, &tx, past_deadline_height()).unwrap_err();
    match err {
        ExecError::ChannelFinalizeNotClosing { status } => {
            // OPEN = 1
            assert_eq!(status, 1);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn channel_finalize_on_proposed_status_rejected() {
    // Create but DO NOT accept; status is PROPOSED.
    let mut db = MemKv::new();
    let a = make_keyed_entity(
        &mut db,
        [0x14u8; 32],
        [0x24u8; 32],
        [0x15u8; 32],
        PROPOSER_BALANCE,
    );
    let b = make_keyed_entity(
        &mut db,
        [0x16u8; 32],
        [0x26u8; 32],
        [0x17u8; 32],
        COUNTERPARTY_BALANCE,
    );
    let channel = sample_channel(&a.entity, &b.entity, DEFAULT_DEPOSIT_A, DEFAULT_DEPOSIT_B);
    let object_id = propose(&mut db, &a.entity, 0, &channel);

    let tx = make_finalize_tx(&a.entity, 1, object_id, a.entity.id, [0x0Cu8; 32]);
    let err = apply_signal_commitment_tx(&mut db, &tx, past_deadline_height()).unwrap_err();
    match err {
        ExecError::ChannelFinalizeNotClosing { status } => {
            assert_eq!(status, PAYMENT_CHANNEL_STATUS_PROPOSED);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn channel_finalize_not_found_rejected() {
    let mut db = MemKv::new();
    let a = make_keyed_entity(
        &mut db,
        [0x18u8; 32],
        [0x28u8; 32],
        [0x19u8; 32],
        PROPOSER_BALANCE,
    );
    let bogus = [0xDEu8; 32];
    let tx = make_finalize_tx(&a.entity, 0, bogus, a.entity.id, [0x0Du8; 32]);
    let err = apply_signal_commitment_tx(&mut db, &tx, past_deadline_height()).unwrap_err();
    assert!(matches!(err, ExecError::ChannelFinalizeNotFound));
}

#[test]
fn channel_finalize_double_finalize_rejected() {
    // Successful finalize removes the channel record; a second
    // finalize attempt must surface ChannelFinalizeNotFound.
    let mut db = MemKv::new();
    let (a, b, object_id) = setup_open_channel(&mut db, [0x1Au8; 32], [0x1Bu8; 32]);

    let bal_a = DEFAULT_DEPOSIT_A;
    let bal_b = DEFAULT_DEPOSIT_B;
    close_unilateral(
        &mut db,
        &a,
        1,
        object_id,
        &a,
        &b,
        0,
        bal_a,
        bal_b,
        [0x0Eu8; 32],
    );

    let tx = make_finalize_tx(&a.entity, 2, object_id, a.entity.id, [0x0Fu8; 32]);
    apply_signal_commitment_tx(&mut db, &tx, past_deadline_height()).expect("first finalize");

    // Second finalize: channel is gone.
    let tx2 = make_finalize_tx(&a.entity, 3, object_id, a.entity.id, [0x10u8; 32]);
    let err = apply_signal_commitment_tx(&mut db, &tx2, past_deadline_height() + 1).unwrap_err();
    assert!(matches!(err, ExecError::ChannelFinalizeNotFound));
}

#[test]
fn channel_finalize_object_type_mismatch_rejected() {
    // Resolve a non-PaymentChannel memory object (ChainSummary) via
    // the finalize handler. The handler must reject with the
    // type-mismatch variant carrying the resolved type byte.
    let mut db = MemKv::new();
    let a = make_keyed_entity(
        &mut db,
        [0x1Cu8; 32],
        [0x2Cu8; 32],
        [0x1Du8; 32],
        PROPOSER_BALANCE,
    );

    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::ChainSummary,
        data: vec![0u8; 32],
    });
    let tx_create = TxV1 {
        version: TxVersion::V1,
        from: a.entity.id,
        pubkey: a.entity.id,
        nonce: 0,
        fee: CREATE_FEE,
        payload,
        sig: [0u8; 64],
    };
    let object_id =
        apply_create_memory_object_tx(&mut db, &tx_create, HEIGHT_PROPOSE).expect("create");

    let tx = make_finalize_tx(&a.entity, 1, object_id, a.entity.id, [0x11u8; 32]);
    let err = apply_signal_commitment_tx(&mut db, &tx, past_deadline_height()).unwrap_err();
    match err {
        ExecError::ChannelFinalizeObjectTypeMismatch { found } => {
            assert_eq!(found, MemoryObjectType::ChainSummary.to_byte());
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
