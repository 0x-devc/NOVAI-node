#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]

//! Week 34 Phase 5: adversarial / cross-feature tests for EntityUpgrade.
//!
//! The headline claim of Week 34 is that an upgrade swaps an entity's
//! `code_hash` while leaving every id-keyed piece of state untouched, because
//! nothing in the runtime references `code_hash`. These tests prove that by
//! standing up a live payment channel and a live SLA with the real handlers,
//! upgrading a participant, and asserting the channel / SLA is byte-for-byte
//! unaffected. The remaining tests re-confirm the cooldown, creator, and
//! same-hash rejections end to end.

use novai_ai_entities::{
    AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObjectType, PaymentChannelData,
    SlaAgreementData, CHANNEL_DISPUTE_WINDOW_DEFAULT_BLOCKS, PAYMENT_CHANNEL_RESERVED_LEN,
    PAYMENT_CHANNEL_SIZE, PAYMENT_CHANNEL_STATUS_OPEN, PAYMENT_CHANNEL_STATUS_PROPOSED,
    PAYMENT_CHANNEL_V1, SLA_AGREEMENT_SIZE, SLA_AGREEMENT_V1, SLA_RESERVED_LEN, SLA_STATUS_ACTIVE,
    SLA_STATUS_PROPOSED,
};
use novai_execution::{
    apply_create_memory_object_tx, apply_entity_upgrade_tx, apply_signal_commitment_tx,
    encode_create_memory_object_payload_v1, encode_signal_commitment_payload_v1, read_ai_entity,
    read_upgrade_summary, write_ai_entity_op, ChannelAcceptExtraV1, CreateMemoryObjectPayloadV1,
    ExecError, SignalCommitmentPayloadV1, SlaAcceptExtraV1, MIN_FEE_ENTITY_UPGRADE,
};
use novai_state::{
    account_key, ai_entity_by_address_key, ai_memory_object_key, encode_account_v1, AccountStateV1,
    Kv, KvBatch, MemKv, WriteOp,
};
use novai_types::{TxV1, TxVersion};

const ENTITY_BALANCE: u128 = 10_000_000;
const ENTITY_STAKE: u128 = 5_000_000;
const CREATE_FEE: u64 = 1_000;
const ACCEPT_FEE: u64 = 1_000;
const DEPOSIT_A: u128 = 200_000;
const DEPOSIT_B: u128 = 150_000;
const SLA_SLASH: u128 = 1_000_000;

// ============================================================================
// Entity / account helpers
// ============================================================================

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

/// Build, fund, and store an entity. The entity signs from its own id-address
/// (the reverse index maps `id -> id`), matching the channel / SLA test setup.
fn make_entity(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut e = AiEntity::new(code_hash, creator, AutonomyMode::Gated, caps(), 1000);
    e.economic_balance = ENTITY_BALANCE;
    e.stake_balance = ENTITY_STAKE;
    db.apply_batch(&[
        write_ai_entity_op(&e),
        WriteOp::Put(ai_entity_by_address_key(&e.id), e.id.to_vec()),
    ])
    .unwrap();
    e
}

/// Fund a normal account (the entity creator) so it can pay the upgrade fee.
fn seed_account(db: &mut MemKv, addr: &[u8; 32], balance: u128) {
    let acct = AccountStateV1 { balance, nonce: 0 };
    db.apply_batch(&[WriteOp::Put(
        account_key(addr),
        encode_account_v1(&acct).to_vec(),
    )])
    .unwrap();
}

/// Build an EntityUpgrade tx (type 11) submitted by the creator account.
fn upgrade_tx(creator: [u8; 32], nonce: u64, entity_id: [u8; 32], new_code_hash: [u8; 32]) -> TxV1 {
    let mut payload = Vec::with_capacity(97);
    payload.push(11);
    payload.extend_from_slice(&entity_id);
    payload.extend_from_slice(&new_code_hash);
    payload.extend_from_slice(&[0u8; 32]); // reason_hash = none
    TxV1 {
        version: TxVersion::V1,
        from: creator,
        pubkey: creator,
        nonce,
        fee: MIN_FEE_ENTITY_UPGRADE,
        payload,
        sig: [0u8; 64],
    }
}

// ============================================================================
// Channel helpers
// ============================================================================

fn sample_channel(party_a: &AiEntity, party_b: &AiEntity) -> PaymentChannelData {
    PaymentChannelData {
        version: PAYMENT_CHANNEL_V1,
        party_a_entity_id: party_a.id,
        party_b_entity_id: party_b.id,
        sla_object_id: [0u8; 32],
        status: PAYMENT_CHANNEL_STATUS_PROPOSED,
        deposit_a: DEPOSIT_A,
        deposit_b: DEPOSIT_B,
        balance_a: DEPOSIT_A,
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

fn propose_channel(db: &mut MemKv, party_a: &AiEntity, channel: &PaymentChannelData) -> [u8; 32] {
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::PaymentChannel,
        data: channel.encode().to_vec(),
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: party_a.id,
        pubkey: party_a.id,
        nonce: 0,
        fee: CREATE_FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_create_memory_object_tx(db, &tx, 500).expect("channel propose succeeds")
}

fn accept_channel(db: &mut MemKv, party_b: &AiEntity, object_id: [u8; 32], party_a_id: [u8; 32]) {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xE1u8; 32],
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
            channel_object_id: object_id,
            party_a_entity_id: party_a_id,
        }),
        channel_close: None,
        channel_finalize: None,
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: party_b.id,
        pubkey: party_b.id,
        nonce: 0,
        fee: ACCEPT_FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_signal_commitment_tx(db, &tx, 700).expect("channel accept succeeds");
}

fn read_channel(db: &MemKv, party_a: &[u8; 32], object_id: &[u8; 32]) -> PaymentChannelData {
    let envelope = db
        .get(&ai_memory_object_key(party_a, object_id))
        .unwrap()
        .expect("channel envelope present");
    let start = envelope.len() - PAYMENT_CHANNEL_SIZE;
    PaymentChannelData::decode(&envelope[start..]).expect("channel decodes")
}

// ============================================================================
// SLA helpers
// ============================================================================

fn sample_sla(buyer: &AiEntity, seller: &AiEntity) -> SlaAgreementData {
    SlaAgreementData {
        version: SLA_AGREEMENT_V1,
        buyer_entity_id: buyer.id,
        seller_entity_id: seller.id,
        service_descriptor_hash: [0u8; 32],
        status: SLA_STATUS_PROPOSED,
        created_at_height: 0,
        accepted_at_height: 0,
        start_height: 1_000,
        end_height: 5_000,
        violation_count: 0,
        violation_threshold: 3,
        max_response_time_blocks: 0,
        min_uptime_bps: 0,
        min_delivery_success_bps: 0,
        price_per_call: 100,
        slash_amount: SLA_SLASH,
        terminated_at_height: 0,
        slashed_amount: 0,
        reserved: [0u8; SLA_RESERVED_LEN],
    }
}

fn propose_sla(db: &mut MemKv, buyer: &AiEntity, sla: &SlaAgreementData) -> [u8; 32] {
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::SlaAgreement,
        data: sla.encode().to_vec(),
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: buyer.id,
        pubkey: buyer.id,
        nonce: 0,
        fee: CREATE_FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_create_memory_object_tx(db, &tx, 500).expect("sla propose succeeds")
}

fn accept_sla(db: &mut MemKv, seller: &AiEntity, object_id: [u8; 32], buyer_id: [u8; 32]) {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xE2u8; 32],
        signal_type: AiSignalType::SlaAccept,
        issuer_entity_id: seller.id,
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
        sla_accept: Some(SlaAcceptExtraV1 {
            sla_object_id: object_id,
            buyer_entity_id: buyer_id,
        }),
        channel_accept: None,
        channel_close: None,
        channel_finalize: None,
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: seller.id,
        pubkey: seller.id,
        nonce: 0,
        fee: ACCEPT_FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_signal_commitment_tx(db, &tx, 700).expect("sla accept succeeds");
}

fn read_sla(db: &MemKv, buyer_id: &[u8; 32], object_id: &[u8; 32]) -> SlaAgreementData {
    let envelope = db
        .get(&ai_memory_object_key(buyer_id, object_id))
        .unwrap()
        .expect("sla envelope present");
    let start = envelope.len() - SLA_AGREEMENT_SIZE;
    SlaAgreementData::decode(&envelope[start..]).expect("sla decodes")
}

// ============================================================================
// 1. Upgrade during an active payment channel: channel unaffected
// ============================================================================

#[test]
fn upgrade_during_active_channel_leaves_channel_unaffected() {
    let mut db = MemKv::new();
    let creator_a = [0x21u8; 32];
    let a = make_entity(&mut db, [0x11u8; 32], creator_a);
    let b = make_entity(&mut db, [0x12u8; 32], [0x22u8; 32]);

    // Stand up a live (OPEN) channel between A and B.
    let object_id = propose_channel(&mut db, &a, &sample_channel(&a, &b));
    accept_channel(&mut db, &b, object_id, a.id);
    let before = read_channel(&db, &a.id, &object_id);
    assert_eq!(before.status, PAYMENT_CHANNEL_STATUS_OPEN);
    let a_balance_before = read_ai_entity(&db, &a.id)
        .unwrap()
        .unwrap()
        .economic_balance;

    // Upgrade party A's code hash.
    seed_account(&mut db, &creator_a, 1_000_000);
    apply_entity_upgrade_tx(&mut db, &upgrade_tx(creator_a, 0, a.id, [0x99u8; 32]), 800)
        .expect("upgrade succeeds during an open channel");

    // Entity A: id preserved, code_hash swapped, economic_balance untouched
    // (the upgrade fee comes from the creator account, not the entity).
    let a_after = read_ai_entity(&db, &a.id).unwrap().unwrap();
    assert_eq!(a_after.id, a.id);
    assert_eq!(a_after.code_hash, [0x99u8; 32]);
    assert_eq!(a_after.economic_balance, a_balance_before);

    // Channel: byte-identical, still resolvable by the unchanged party_a id.
    let after = read_channel(&db, &a.id, &object_id);
    assert_eq!(after.status, PAYMENT_CHANNEL_STATUS_OPEN);
    assert_eq!(after.party_a_entity_id, a.id);
    assert_eq!(after.party_b_entity_id, b.id);
    assert_eq!(after.deposit_a, before.deposit_a);
    assert_eq!(after.deposit_b, before.deposit_b);
    assert_eq!(after.balance_a, before.balance_a);
    assert_eq!(after.balance_b, before.balance_b);
    assert_eq!(after.nonce, before.nonce);
}

// ============================================================================
// 2. Upgrade during an active SLA: SLA unaffected, seller stake preserved
// ============================================================================

#[test]
fn upgrade_during_active_sla_leaves_sla_unaffected() {
    let mut db = MemKv::new();
    let buyer = make_entity(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let creator_seller = [0x22u8; 32];
    let seller = make_entity(&mut db, [0x12u8; 32], creator_seller);

    // Stand up a live (ACTIVE) SLA owned by the buyer, accepted by the seller.
    let object_id = propose_sla(&mut db, &buyer, &sample_sla(&buyer, &seller));
    accept_sla(&mut db, &seller, object_id, buyer.id);
    let before = read_sla(&db, &buyer.id, &object_id);
    assert_eq!(before.status, SLA_STATUS_ACTIVE);

    // Upgrade the seller (the party whose stake collateralizes the SLA).
    seed_account(&mut db, &creator_seller, 1_000_000);
    apply_entity_upgrade_tx(
        &mut db,
        &upgrade_tx(creator_seller, 0, seller.id, [0x99u8; 32]),
        800,
    )
    .expect("upgrade succeeds during an active SLA");

    // Seller: id preserved, code_hash swapped, stake collateral untouched.
    let seller_after = read_ai_entity(&db, &seller.id).unwrap().unwrap();
    assert_eq!(seller_after.id, seller.id);
    assert_eq!(seller_after.code_hash, [0x99u8; 32]);
    assert_eq!(seller_after.stake_balance, ENTITY_STAKE);

    // SLA: still ACTIVE and byte-identical, resolvable by the unchanged ids.
    let after = read_sla(&db, &buyer.id, &object_id);
    assert_eq!(after.status, SLA_STATUS_ACTIVE);
    assert_eq!(after.buyer_entity_id, buyer.id);
    assert_eq!(after.seller_entity_id, seller.id);
    assert_eq!(after.slash_amount, before.slash_amount);
    assert_eq!(after.violation_count, before.violation_count);
    assert_eq!(after.end_height, before.end_height);
}

// ============================================================================
// 3. Rapid upgrade within the cooldown window is rejected
// ============================================================================

#[test]
fn rapid_upgrade_within_cooldown_rejected() {
    let mut db = MemKv::new();
    let creator = [0x21u8; 32];
    let e = make_entity(&mut db, [0x11u8; 32], creator);
    seed_account(&mut db, &creator, 1_000_000);

    // First upgrade at height 1000 succeeds (no prior summary).
    apply_entity_upgrade_tx(&mut db, &upgrade_tx(creator, 0, e.id, [0x91u8; 32]), 1_000)
        .expect("first upgrade succeeds");

    // Second upgrade only 500 blocks later is inside the 1000-block cooldown.
    let err = apply_entity_upgrade_tx(&mut db, &upgrade_tx(creator, 1, e.id, [0x92u8; 32]), 1_500)
        .unwrap_err();
    assert!(matches!(
        err,
        ExecError::EntityUpgradeCooldownActive {
            last_upgrade_height: 1_000,
            current_height: 1_500,
            next_allowed_height: 2_000,
        }
    ));

    // State stays at the first upgrade.
    let after = read_ai_entity(&db, &e.id).unwrap().unwrap();
    assert_eq!(after.code_hash, [0x91u8; 32]);
    let summary = read_upgrade_summary(&db, &e.id).unwrap().unwrap();
    assert_eq!(summary.upgrade_count, 1);
}

// ============================================================================
// 4. A non-creator cannot upgrade the entity
// ============================================================================

#[test]
fn non_creator_upgrade_rejected() {
    let mut db = MemKv::new();
    let creator = [0x21u8; 32];
    let attacker = [0x99u8; 32];
    let e = make_entity(&mut db, [0x11u8; 32], creator);
    seed_account(&mut db, &attacker, 1_000_000);

    let err = apply_entity_upgrade_tx(&mut db, &upgrade_tx(attacker, 0, e.id, [0x92u8; 32]), 1_000)
        .unwrap_err();
    assert!(matches!(err, ExecError::EntityUpgradeNotCreator));

    // Untouched: original code hash, no summary.
    let after = read_ai_entity(&db, &e.id).unwrap().unwrap();
    assert_eq!(after.code_hash, [0x11u8; 32]);
    assert!(read_upgrade_summary(&db, &e.id).unwrap().is_none());
}

// ============================================================================
// 5. Upgrade to the same code hash is rejected
// ============================================================================

#[test]
fn upgrade_to_same_code_hash_rejected() {
    let mut db = MemKv::new();
    let creator = [0x21u8; 32];
    let e = make_entity(&mut db, [0x11u8; 32], creator);
    seed_account(&mut db, &creator, 1_000_000);

    let err = apply_entity_upgrade_tx(&mut db, &upgrade_tx(creator, 0, e.id, [0x11u8; 32]), 1_000)
        .unwrap_err();
    assert!(matches!(err, ExecError::EntityUpgradeSameCodeHash));

    let after = read_ai_entity(&db, &e.id).unwrap().unwrap();
    assert_eq!(after.code_hash, [0x11u8; 32]);
    assert!(read_upgrade_summary(&db, &e.id).unwrap().is_none());
}
