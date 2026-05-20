#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::too_many_arguments)]

//! Adversarial tests for Week 32 PaymentChannels (Phase 8).
//!
//! Covers replay defences and multi-channel scenarios that fall
//! outside the per-handler test files:
//!
//! - Cross-channel signature replay: a doubly-signed state for
//!   channel X cannot be submitted against a different channel Y
//!   (the signed bytes bind channel_object_id).
//! - Cross-pair signature replay: a state signed by (A, B) cannot
//!   close a channel between (A, C) because party C's pubkey will
//!   not verify the sig in the sig_b slot.
//! - Multi-channel per (A, B) pair: by design, multiple channels
//!   between the same two participants are allowed simultaneously.
//! - ChannelClose with an unregistered party (pubkey = all-zero):
//!   the chain refuses the close because verify_channel_state_signature
//!   rejects the malformed pubkey rather than panicking.
//! - Stale-state replay across the open / closing / finalize
//!   boundary: a nonce-0 close cannot succeed once the channel has
//!   already moved past nonce 0.

use novai_ai_entities::{
    AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObjectType, PaymentChannelData,
    CHANNEL_DISPUTE_WINDOW_DEFAULT_BLOCKS, PAYMENT_CHANNEL_RESERVED_LEN,
    PAYMENT_CHANNEL_STATUS_PROPOSED, PAYMENT_CHANNEL_V1,
};
use novai_crypto::{sign_channel_state, SigningKey};
use novai_execution::{
    apply_create_memory_object_tx, apply_signal_commitment_tx,
    encode_create_memory_object_payload_v1, encode_signal_commitment_payload_v1,
    get_channels_by_party_a, get_channels_by_party_b, get_payment_channel, write_ai_entity_op,
    ChannelAcceptExtraV1, ChannelCloseExtraV1, CreateMemoryObjectPayloadV1, ExecError,
    SignalCommitmentPayloadV1, CHANNEL_CLOSE_NOT_FINAL, NOVAI_CHANNEL_CHAIN_ID,
};
use novai_state::{ai_entity_by_address_key, Kv, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const PROPOSER_BALANCE: u128 = 10_000_000;
const COUNTERPARTY_BALANCE: u128 = 10_000_000;
const CREATE_FEE: u64 = 1_000;
const ACCEPT_FEE: u64 = 1_000;
const CLOSE_FEE: u64 = 1_000;
const HEIGHT_PROPOSE: u64 = 500;
const HEIGHT_ACCEPT: u64 = 700;
const HEIGHT_CLOSE: u64 = 900;
const DEPOSIT_A: u128 = 200_000;
const DEPOSIT_B: u128 = 150_000;

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
    let mut entity =
        AiEntity::new_with_pubkey(code_hash, creator, AutonomyMode::Gated, caps(), pubkey, 1000);
    entity.economic_balance = balance;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();
    KeyedEntity { entity, sk }
}

/// Make an entity WITHOUT a registered pubkey (legacy V1/V2-style).
fn make_keyless_entity(
    db: &mut MemKv,
    code_hash: [u8; 32],
    creator: [u8; 32],
    balance: u128,
) -> AiEntity {
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

fn make_close_tx(
    submitter: &AiEntity,
    submitter_tx_nonce: u64,
    channel_object_id: [u8; 32],
    party_a_id: [u8; 32],
    state_nonce: u64,
    balance_a: u128,
    balance_b: u128,
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
            is_final: CHANNEL_CLOSE_NOT_FINAL,
            sig_a,
            sig_b,
        }),
        channel_finalize: None,
    });
    TxV1 {
        version: TxVersion::V1,
        from: submitter.id,
        pubkey: submitter.id,
        nonce: submitter_tx_nonce,
        fee: CLOSE_FEE,
        payload,
        sig: [0u8; 64],
    }
}

// ============================================================================
// 1. Cross-channel signature replay
// ============================================================================

#[test]
fn channel_sig_does_not_replay_across_channels() {
    // Open two channels between the same pair, with the same
    // deposits and dispute window. Sign a closing state for channel
    // X, then try to use it against channel Y. The signatures must
    // not verify because the canonical signing bytes bind the
    // distinct channel_object_id values.
    let mut db = MemKv::new();
    let a = make_keyed_entity(&mut db, [0x11u8; 32], [0x21u8; 32], [0x01u8; 32], PROPOSER_BALANCE * 2);
    let b = make_keyed_entity(&mut db, [0x12u8; 32], [0x22u8; 32], [0x02u8; 32], COUNTERPARTY_BALANCE * 2);

    let ch_x = sample_channel(&a.entity, &b.entity, DEPOSIT_A, DEPOSIT_B);
    let oid_x = propose_at(&mut db, &a.entity, 0, &ch_x, HEIGHT_PROPOSE);
    accept_at(&mut db, &b.entity, 0, oid_x, a.entity.id, HEIGHT_ACCEPT);

    let ch_y = sample_channel(&a.entity, &b.entity, DEPOSIT_A, DEPOSIT_B);
    let oid_y = propose_at(&mut db, &a.entity, 1, &ch_y, HEIGHT_PROPOSE + 1);
    accept_at(&mut db, &b.entity, 1, oid_y, a.entity.id, HEIGHT_ACCEPT + 1);
    assert_ne!(oid_x, oid_y, "channels must have distinct ids");

    // Both parties sign a state for channel X.
    let state_nonce: u64 = 3;
    let balance_a = DEPOSIT_A - 10_000;
    let balance_b = DEPOSIT_B + 10_000;
    let sig_a_x = sign_channel_state(
        &a.sk,
        NOVAI_CHANNEL_CHAIN_ID,
        &oid_x,
        &a.entity.id,
        &b.entity.id,
        state_nonce,
        balance_a,
        balance_b,
        false,
    );
    let sig_b_x = sign_channel_state(
        &b.sk,
        NOVAI_CHANNEL_CHAIN_ID,
        &oid_x,
        &a.entity.id,
        &b.entity.id,
        state_nonce,
        balance_a,
        balance_b,
        false,
    );

    // Replay attempt: submit X's signatures against channel Y.
    let tx = make_close_tx(
        &a.entity,
        2,
        oid_y,
        a.entity.id,
        state_nonce,
        balance_a,
        balance_b,
        sig_a_x,
        sig_b_x,
        [0xAAu8; 32],
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).unwrap_err();
    // sig_a is the first sig the handler verifies, so the failure
    // surfaces there.
    assert!(matches!(err, ExecError::ChannelCloseInvalidSignatureA));

    // Channel Y's state is unchanged.
    let (_, ch_y_after) = get_payment_channel(&db, &a.entity.id, &oid_y).unwrap().unwrap();
    assert_eq!(ch_y_after.nonce, 0);
    assert_eq!(ch_y_after.balance_a, DEPOSIT_A);
    assert_eq!(ch_y_after.balance_b, DEPOSIT_B);
}

#[test]
fn channel_sig_does_not_replay_across_pair() {
    // Build two pairs (A, B1) and (A, B2) with one channel each.
    // The (A, B1) state is signed by B1; submitting that sig in B2's
    // slot must fail because B2's pubkey does not verify B1's sig.
    let mut db = MemKv::new();
    let a = make_keyed_entity(&mut db, [0x13u8; 32], [0x23u8; 32], [0x03u8; 32], PROPOSER_BALANCE * 2);
    let b1 = make_keyed_entity(&mut db, [0x14u8; 32], [0x24u8; 32], [0x04u8; 32], COUNTERPARTY_BALANCE);
    let b2 = make_keyed_entity(&mut db, [0x15u8; 32], [0x25u8; 32], [0x05u8; 32], COUNTERPARTY_BALANCE);

    let ch_ab1 = sample_channel(&a.entity, &b1.entity, DEPOSIT_A, DEPOSIT_B);
    let oid_ab1 = propose_at(&mut db, &a.entity, 0, &ch_ab1, HEIGHT_PROPOSE);
    accept_at(&mut db, &b1.entity, 0, oid_ab1, a.entity.id, HEIGHT_ACCEPT);

    let ch_ab2 = sample_channel(&a.entity, &b2.entity, DEPOSIT_A, DEPOSIT_B);
    let oid_ab2 = propose_at(&mut db, &a.entity, 1, &ch_ab2, HEIGHT_PROPOSE + 1);
    accept_at(&mut db, &b2.entity, 0, oid_ab2, a.entity.id, HEIGHT_ACCEPT + 1);

    // A and B1 sign a state for channel (A, B1).
    let nonce: u64 = 1;
    let balance_a = DEPOSIT_A - 5_000;
    let balance_b = DEPOSIT_B + 5_000;
    // Sign for (A, B1) (i.e., the canonical bytes embed b1.entity.id).
    let sig_a = sign_channel_state(
        &a.sk,
        NOVAI_CHANNEL_CHAIN_ID,
        &oid_ab2,
        &a.entity.id,
        &b2.entity.id,
        nonce,
        balance_a,
        balance_b,
        false,
    );
    let sig_b1_for_ab1 = sign_channel_state(
        &b1.sk,
        NOVAI_CHANNEL_CHAIN_ID,
        &oid_ab2,
        &a.entity.id,
        &b1.entity.id,
        nonce,
        balance_a,
        balance_b,
        false,
    );

    // Try to close channel (A, B2) using B1's signature in the
    // sig_b slot. The handler verifies sig_b against b2.pubkey, so
    // a sig signed by B1 against (A, B1) bytes will not verify.
    let tx = make_close_tx(
        &a.entity,
        2,
        oid_ab2,
        a.entity.id,
        nonce,
        balance_a,
        balance_b,
        sig_a,
        sig_b1_for_ab1,
        [0xBBu8; 32],
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).unwrap_err();
    assert!(matches!(err, ExecError::ChannelCloseInvalidSignatureB));
}

// ============================================================================
// 2. Multi-channel between the same pair
// ============================================================================

#[test]
fn multi_channel_between_same_pair_allowed() {
    // The Phase 0 design (pushback P6) explicitly removed the
    // single-active-channel-per-pair constraint that SLAs use.
    // Confirm via three concurrent channels between (A, B), each
    // visible in both by-party scans.
    let mut db = MemKv::new();
    let a = make_keyed_entity(&mut db, [0x16u8; 32], [0x26u8; 32], [0x06u8; 32], PROPOSER_BALANCE * 5);
    let b = make_keyed_entity(&mut db, [0x17u8; 32], [0x27u8; 32], [0x07u8; 32], COUNTERPARTY_BALANCE * 5);

    let ch1 = sample_channel(&a.entity, &b.entity, 30_000, 20_000);
    let oid1 = propose_at(&mut db, &a.entity, 0, &ch1, 100);
    let ch2 = sample_channel(&a.entity, &b.entity, 50_000, 30_000);
    let oid2 = propose_at(&mut db, &a.entity, 1, &ch2, 200);
    let ch3 = sample_channel(&a.entity, &b.entity, 70_000, 40_000);
    let oid3 = propose_at(&mut db, &a.entity, 2, &ch3, 300);

    // All three resolve cleanly.
    assert!(get_payment_channel(&db, &a.entity.id, &oid1).unwrap().is_some());
    assert!(get_payment_channel(&db, &a.entity.id, &oid2).unwrap().is_some());
    assert!(get_payment_channel(&db, &a.entity.id, &oid3).unwrap().is_some());

    // by_party_a lists all three in height order.
    let by_a = get_channels_by_party_a(&db, &a.entity.id, 0, u64::MAX).unwrap();
    assert_eq!(by_a.len(), 3);
    assert_eq!(by_a[0].0.object_id, oid1);
    assert_eq!(by_a[1].0.object_id, oid2);
    assert_eq!(by_a[2].0.object_id, oid3);

    // by_party_b sees the same three from B's side.
    let by_b = get_channels_by_party_b(&db, &b.entity.id, 0, u64::MAX).unwrap();
    assert_eq!(by_b.len(), 3);
    assert_eq!(by_b[0].0.object_id, oid1);
    assert_eq!(by_b[1].0.object_id, oid2);
    assert_eq!(by_b[2].0.object_id, oid3);

    // Sanity: party A's deposit was debited three times.
    let a_after = novai_execution::lookup_ai_entity_by_address(&db, &a.entity.id)
        .unwrap()
        .unwrap();
    let total_locked: u128 = 30_000 + 50_000 + 70_000;
    let total_fees: u128 = u128::from(CREATE_FEE) * 3;
    assert_eq!(
        a_after.economic_balance,
        PROPOSER_BALANCE * 5 - total_locked - total_fees
    );
}

// ============================================================================
// 3. Unregistered-pubkey close attempt
// ============================================================================

#[test]
fn channel_close_against_keyless_counterparty_rejected_at_sig_verify() {
    // A channel can be created against a counterparty that never
    // registered a pubkey (their entity.pubkey == [0; 32]). Such a
    // channel is structurally valid but unclosable: every
    // ChannelClose attempt fails at the sig_b verification because
    // [0; 32] is not a valid Ed25519 public point.
    let mut db = MemKv::new();
    let a = make_keyed_entity(&mut db, [0x18u8; 32], [0x28u8; 32], [0x08u8; 32], PROPOSER_BALANCE);
    let b = make_keyless_entity(&mut db, [0x19u8; 32], [0x29u8; 32], COUNTERPARTY_BALANCE);

    let ch = sample_channel(&a.entity, &b, DEPOSIT_A, DEPOSIT_B);
    let oid = propose_at(&mut db, &a.entity, 0, &ch, HEIGHT_PROPOSE);

    // Manually inject an OPEN status (B never accepts with a real
    // signing flow, so we splice the on-chain state past the
    // accept gate). The point of the test is the close-side sig
    // verification.
    let key = novai_state::ai_memory_object_key(&a.entity.id, &oid);
    let envelope = db.get(&key).unwrap().unwrap();
    let mut patched = envelope.clone();
    let payload_start = envelope.len() - novai_ai_entities::PAYMENT_CHANNEL_SIZE;
    patched[payload_start + 97] = novai_ai_entities::PAYMENT_CHANNEL_STATUS_OPEN;
    patched[payload_start + 146..payload_start + 162].copy_from_slice(&DEPOSIT_B.to_be_bytes());
    db.apply_batch(&[WriteOp::Put(key, patched)]).unwrap();

    // A signs the close state. We do not have B's signing key (B
    // is keyless), so the sig_b slot carries an all-zero placeholder
    // that verify_channel_state_signature must reject as a bad
    // pubkey (B's pubkey is [0; 32], not a valid Ed25519 point).
    let nonce: u64 = 1;
    let balance_a = DEPOSIT_A;
    let balance_b = DEPOSIT_B;
    let sig_a = sign_channel_state(
        &a.sk,
        NOVAI_CHANNEL_CHAIN_ID,
        &oid,
        &a.entity.id,
        &b.id,
        nonce,
        balance_a,
        balance_b,
        false,
    );

    let tx = make_close_tx(
        &a.entity,
        1,
        oid,
        a.entity.id,
        nonce,
        balance_a,
        balance_b,
        sig_a,
        [0u8; 64],
        [0xCCu8; 32],
    );
    let err = apply_signal_commitment_tx(&mut db, &tx, HEIGHT_CLOSE).unwrap_err();
    assert!(matches!(err, ExecError::ChannelCloseInvalidSignatureB));
}

// ============================================================================
// 4. Stale-state replay defence
// ============================================================================

#[test]
fn channel_initial_state_close_rejected_after_state_advances() {
    // Open a channel, submit a unilateral close at nonce 5. A
    // second close attempt at nonce 0 (initial-state path) must
    // fail because the channel's stored nonce is now 5, and the
    // nonce-monotonicity gate's nonce-0 exception only fires when
    // channel.nonce is also 0.
    let mut db = MemKv::new();
    let a = make_keyed_entity(&mut db, [0x1Au8; 32], [0x2Au8; 32], [0x0Au8; 32], PROPOSER_BALANCE);
    let b = make_keyed_entity(&mut db, [0x1Bu8; 32], [0x2Bu8; 32], [0x0Bu8; 32], COUNTERPARTY_BALANCE);
    let ch = sample_channel(&a.entity, &b.entity, DEPOSIT_A, DEPOSIT_B);
    let oid = propose_at(&mut db, &a.entity, 0, &ch, HEIGHT_PROPOSE);
    accept_at(&mut db, &b.entity, 0, oid, a.entity.id, HEIGHT_ACCEPT);

    // First close at nonce 5.
    let nonce_first: u64 = 5;
    let balance_a = DEPOSIT_A - 25_000;
    let balance_b = DEPOSIT_B + 25_000;
    let sig_a = sign_channel_state(
        &a.sk,
        NOVAI_CHANNEL_CHAIN_ID,
        &oid,
        &a.entity.id,
        &b.entity.id,
        nonce_first,
        balance_a,
        balance_b,
        false,
    );
    let sig_b = sign_channel_state(
        &b.sk,
        NOVAI_CHANNEL_CHAIN_ID,
        &oid,
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
        oid,
        a.entity.id,
        nonce_first,
        balance_a,
        balance_b,
        sig_a,
        sig_b,
        [0xDDu8; 32],
    );
    apply_signal_commitment_tx(&mut db, &tx1, HEIGHT_CLOSE).expect("first close");

    // Try nonce 0 (initial-state path) with balances == deposits.
    // Should fail with NonceNotMonotonic because channel.nonce is
    // now 5.
    let zero_sig_a = sign_channel_state(
        &a.sk,
        NOVAI_CHANNEL_CHAIN_ID,
        &oid,
        &a.entity.id,
        &b.entity.id,
        0,
        DEPOSIT_A,
        DEPOSIT_B,
        false,
    );
    let zero_sig_b = sign_channel_state(
        &b.sk,
        NOVAI_CHANNEL_CHAIN_ID,
        &oid,
        &a.entity.id,
        &b.entity.id,
        0,
        DEPOSIT_A,
        DEPOSIT_B,
        false,
    );
    let tx2 = make_close_tx(
        &b.entity,
        1,
        oid,
        a.entity.id,
        0,
        DEPOSIT_A,
        DEPOSIT_B,
        zero_sig_a,
        zero_sig_b,
        [0xEEu8; 32],
    );
    let err = apply_signal_commitment_tx(&mut db, &tx2, HEIGHT_CLOSE + 5).unwrap_err();
    match err {
        ExecError::ChannelCloseNonceNotMonotonic { current, attempted } => {
            assert_eq!(current, nonce_first);
            assert_eq!(attempted, 0);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
