#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::similar_names)]
#![allow(clippy::too_many_lines)]

//! Integration tests for the Week 31 SLA lifecycle (Phase 4):
//! `UpdateMemoryObject` immutability, `DeleteMemoryObject` teardown
//! by state, and the lazy `StakeWithdraw` collateral check.

use novai_ai_entities::{
    AiEntity, AiSignalType, AutonomyMode, Capabilities, MemoryObjectType, SlaAgreementData,
    SLA_AGREEMENT_V1, SLA_RESERVED_LEN, SLA_STATUS_PROPOSED,
};
use novai_execution::{
    apply_create_memory_object_tx, apply_delete_memory_object_tx, apply_signal_commitment_tx,
    apply_update_memory_object_tx, encode_create_memory_object_payload_v1,
    encode_delete_memory_object_payload_v1, encode_signal_commitment_payload_v1,
    encode_update_memory_object_payload_v1, payment_by_hash_key, payment_by_payee_key,
    payment_by_payer_key, sla_active_between_key, sla_by_buyer_key, sla_by_seller_key,
    write_ai_entity_op, CreateMemoryObjectPayloadV1, DeleteMemoryObjectPayloadV1, ExecError,
    PaymentRecord, ServiceAttestationExtraV1, SignalCommitmentPayloadV1, SlaAcceptExtraV1,
    StakeWithdrawExtraV1, UpdateMemoryObjectPayloadV1, PAYMENT_ATTESTATION_STATUS_FAILED,
    PAYMENT_ATTESTATION_STATUS_NONE,
};
use novai_state::{ai_entity_by_address_key, Kv, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const BUYER_BALANCE: u128 = 50_000_000;
const SELLER_STAKE: u128 = 5_000_000;
const SELLER_BALANCE: u128 = 50_000_000;
const FEE: u64 = 1_000;
const HEIGHT_PROPOSE: u64 = 100;
const HEIGHT_ACCEPT: u64 = 200;
const SLA_START: u64 = 500;
const SLA_END: u64 = 5_000;
const SLASH_AMOUNT: u128 = 1_000_000;
const STAKE_UNLOCK_HEIGHT: u64 = 1_100; // > HEIGHT_PROPOSE + STAKE_LOCK_PERIOD

fn caps() -> Capabilities {
    Capabilities {
        read_public_chain: true,
        read_memory_objects: true,
        emit_proposals: true,
        request_execution: true,
        read_nnpx_derived: false,
        submit_reputation_updates: false,
        _reserved: [false; 2],
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

fn make_buyer(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32]) -> AiEntity {
    let mut buyer = build_entity(code_hash, creator);
    buyer.economic_balance = BUYER_BALANCE;
    store_entity(db, &buyer);
    buyer
}

fn make_seller(db: &mut MemKv, code_hash: [u8; 32], creator: [u8; 32], stake: u128) -> AiEntity {
    let mut seller = build_entity(code_hash, creator);
    seller.stake_balance = stake;
    seller.economic_balance = SELLER_BALANCE;
    // Pre-set stake_locked_until to a height the test will run past so
    // the WithdrawStakeStillLocked gate does not eclipse the collateral
    // gate we are exercising in Phase 4.
    seller.stake_locked_until = 0;
    store_entity(db, &seller);
    seller
}

fn sample_sla(buyer: &AiEntity, seller: &AiEntity, slash: u128) -> SlaAgreementData {
    SlaAgreementData {
        version: SLA_AGREEMENT_V1,
        buyer_entity_id: buyer.id,
        seller_entity_id: seller.id,
        service_descriptor_hash: [0u8; 32],
        status: SLA_STATUS_PROPOSED,
        created_at_height: 0,
        accepted_at_height: 0,
        start_height: SLA_START,
        end_height: SLA_END,
        violation_count: 0,
        violation_threshold: 3,
        max_response_time_blocks: 0,
        min_uptime_bps: 0,
        min_delivery_success_bps: 0,
        price_per_call: 100,
        slash_amount: slash,
        terminated_at_height: 0,
        slashed_amount: 0,
        reserved: [0u8; SLA_RESERVED_LEN],
    }
}

fn propose_sla(db: &mut MemKv, buyer: &AiEntity, nonce: u64, sla: &SlaAgreementData) -> [u8; 32] {
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::SlaAgreement,
        data: sla.encode().to_vec(),
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: buyer.id,
        pubkey: buyer.id,
        nonce,
        fee: FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_create_memory_object_tx(db, &tx, HEIGHT_PROPOSE).expect("propose succeeds")
}

fn accept_sla(
    db: &mut MemKv,
    seller: &AiEntity,
    nonce: u64,
    sla_object_id: [u8; 32],
    buyer_id: [u8; 32],
) {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xE0u8 ^ nonce as u8; 32],
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
            sla_object_id,
            buyer_entity_id: buyer_id,
        }),
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: seller.id,
        pubkey: seller.id,
        nonce,
        fee: FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_signal_commitment_tx(db, &tx, HEIGHT_ACCEPT).expect("accept succeeds");
}

fn make_update_tx(buyer: &AiEntity, nonce: u64, object_id: [u8; 32], new_data: Vec<u8>) -> TxV1 {
    let payload = encode_update_memory_object_payload_v1(&UpdateMemoryObjectPayloadV1 {
        object_id,
        new_data,
    });
    TxV1 {
        version: TxVersion::V1,
        from: buyer.id,
        pubkey: buyer.id,
        nonce,
        fee: FEE,
        payload,
        sig: [0u8; 64],
    }
}

fn make_delete_tx(buyer: &AiEntity, nonce: u64, object_id: [u8; 32]) -> TxV1 {
    let payload =
        encode_delete_memory_object_payload_v1(&DeleteMemoryObjectPayloadV1 { object_id });
    TxV1 {
        version: TxVersion::V1,
        from: buyer.id,
        pubkey: buyer.id,
        nonce,
        fee: FEE,
        payload: payload.to_vec(),
        sig: [0u8; 64],
    }
}

fn make_withdraw_tx(seller: &AiEntity, nonce: u64, amount: u128) -> TxV1 {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xD0u8 ^ nonce as u8; 32],
        signal_type: AiSignalType::StakeWithdraw,
        issuer_entity_id: seller.id,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: Some(StakeWithdrawExtraV1 { amount }),
        stake_slash: None,
        composition_check: None,
        proof_submission: None,
        subscription_create: None,
        subscription_cancel: None,
        payment_request: None,
        service_attestation: None,
        sla_accept: None,
    });
    TxV1 {
        version: TxVersion::V1,
        from: seller.id,
        pubkey: seller.id,
        nonce,
        fee: FEE,
        payload,
        sig: [0u8; 64],
    }
}

fn seed_payment(
    db: &mut MemKv,
    payer: &AiEntity,
    payee: &AiEntity,
    signal_hash: [u8; 32],
    payment_height: u64,
) {
    let record = PaymentRecord {
        payer: payer.id,
        payee: payee.id,
        amount: 1_000,
        service_descriptor_hash: [0u8; 32],
        request_hash: [0xFFu8; 32],
        payment_height,
        max_block_height: payment_height + 100,
        attested_status: PAYMENT_ATTESTATION_STATUS_NONE,
        attested_height: 0,
    };
    let bytes = novai_execution::encode_payment_record_v1(&record);
    db.apply_batch(&[
        WriteOp::Put(payment_by_hash_key(&signal_hash), bytes.to_vec()),
        WriteOp::Put(
            payment_by_payer_key(&payer.id, payment_height, &signal_hash),
            Vec::new(),
        ),
        WriteOp::Put(
            payment_by_payee_key(&payee.id, payment_height, &signal_hash),
            Vec::new(),
        ),
    ])
    .unwrap();
}

fn attest_failed(
    db: &mut MemKv,
    payer: &AiEntity,
    payee: &AiEntity,
    payment_signal_hash: [u8; 32],
    nonce: u64,
    height: u64,
) {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8 ^ nonce as u8; 32],
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
        service_attestation: Some(ServiceAttestationExtraV1 {
            payment_signal_hash,
            payee_entity_id: payee.id,
            status: PAYMENT_ATTESTATION_STATUS_FAILED,
        }),
        sla_accept: None,
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: payer.id,
        pubkey: payer.id,
        nonce,
        fee: FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_signal_commitment_tx(db, &tx, height).expect("attest succeeds");
}

// ============================================================================
// 1. Update-side immutability
// ============================================================================

#[test]
fn sla_update_rejected_with_immutable_on_update() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x11u8; 32], [0x21u8; 32]);
    let seller = make_seller(&mut db, [0x12u8; 32], [0x22u8; 32], SELLER_STAKE);
    let sla = sample_sla(&buyer, &seller, SLASH_AMOUNT);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);

    // Buyer attempts to bump the threshold via UpdateMemoryObject.
    let mut mutated = sla;
    mutated.violation_threshold = 10;
    let tx = make_update_tx(&buyer, 1, sla_id, mutated.encode().to_vec());
    let err = apply_update_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE + 50).unwrap_err();
    assert!(matches!(err, ExecError::SlaAgreementImmutableOnUpdate));
}

#[test]
fn sla_update_rejected_even_with_identical_payload() {
    // Defense: an update with NO field changes is still rejected.
    // Mirrors DelegationGrantNotUpdatable's contract: SLAs go through
    // SlaAccept signal / DeleteMemoryObject / auto-slash, never
    // UpdateMemoryObject.
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x13u8; 32], [0x23u8; 32]);
    let seller = make_seller(&mut db, [0x14u8; 32], [0x24u8; 32], SELLER_STAKE);
    let sla = sample_sla(&buyer, &seller, SLASH_AMOUNT);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);

    let tx = make_update_tx(&buyer, 1, sla_id, sla.encode().to_vec());
    let err = apply_update_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE + 50).unwrap_err();
    assert!(matches!(err, ExecError::SlaAgreementImmutableOnUpdate));
}

// ============================================================================
// 2. Delete teardown by state
// ============================================================================

#[test]
fn sla_delete_proposed_tears_down_all_three_indexes() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x15u8; 32], [0x25u8; 32]);
    let seller = make_seller(&mut db, [0x16u8; 32], [0x26u8; 32], SELLER_STAKE);
    let sla = sample_sla(&buyer, &seller, SLASH_AMOUNT);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);

    let tx = make_delete_tx(&buyer, 1, sla_id);
    apply_delete_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE + 10).expect("delete succeeds");

    // All three SLA indexes are torn down.
    let pair_key = sla_active_between_key(&buyer.id, &seller.id);
    assert!(db.get(&pair_key).unwrap().is_none(), "active_between gone");
    let by_buyer = sla_by_buyer_key(&buyer.id, HEIGHT_PROPOSE, &sla_id);
    assert!(db.get(&by_buyer).unwrap().is_none(), "by_buyer gone");
    let by_seller = sla_by_seller_key(&seller.id, HEIGHT_PROPOSE, &sla_id);
    assert!(db.get(&by_seller).unwrap().is_none(), "by_seller gone");
}

#[test]
fn sla_delete_active_in_window_rejected() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x17u8; 32], [0x27u8; 32]);
    let seller = make_seller(&mut db, [0x18u8; 32], [0x28u8; 32], SELLER_STAKE);
    let sla = sample_sla(&buyer, &seller, SLASH_AMOUNT);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    // Inside the violation window: delete must be rejected.
    let tx = make_delete_tx(&buyer, 1, sla_id);
    let err = apply_delete_memory_object_tx(&mut db, &tx, SLA_START + 100).unwrap_err();
    assert!(matches!(err, ExecError::SlaAgreementDeleteWhileActive));

    // Indexes are unchanged.
    let pair_key = sla_active_between_key(&buyer.id, &seller.id);
    assert!(
        db.get(&pair_key).unwrap().is_some(),
        "active_between intact"
    );
}

#[test]
fn sla_delete_active_after_end_height_allowed() {
    // Past `end_height`: the agreement is effectively closed. Delete
    // is allowed and tears down all three indexes.
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x19u8; 32], [0x29u8; 32]);
    let seller = make_seller(&mut db, [0x1Au8; 32], [0x2Au8; 32], SELLER_STAKE);
    let sla = sample_sla(&buyer, &seller, SLASH_AMOUNT);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    let tx = make_delete_tx(&buyer, 1, sla_id);
    apply_delete_memory_object_tx(&mut db, &tx, SLA_END + 1).expect("delete after expiry");

    let pair_key = sla_active_between_key(&buyer.id, &seller.id);
    assert!(db.get(&pair_key).unwrap().is_none());
}

#[test]
fn sla_delete_after_violation_does_not_touch_active_between() {
    // After auto-slash the active_between key is already gone; the
    // delete path must NOT push a redundant Delete op (or if it does,
    // it must not surface a "missing key" error). It must still
    // tear down by_buyer and by_seller.
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x1Bu8; 32], [0x2Bu8; 32]);
    let seller = make_seller(&mut db, [0x1Cu8; 32], [0x2Cu8; 32], SELLER_STAKE);
    let mut sla = sample_sla(&buyer, &seller, SLASH_AMOUNT);
    sla.violation_threshold = 1;
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    let signal_hash = [0xC1u8; 32];
    seed_payment(&mut db, &buyer, &seller, signal_hash, 600);
    attest_failed(&mut db, &buyer, &seller, signal_hash, 1, 700);

    // active_between was deleted by the slash.
    let pair_key = sla_active_between_key(&buyer.id, &seller.id);
    assert!(
        db.get(&pair_key).unwrap().is_none(),
        "auto-slash already deleted"
    );

    // Audit cleanup delete: must succeed past breach.
    let tx = make_delete_tx(&buyer, 2, sla_id);
    apply_delete_memory_object_tx(&mut db, &tx, 800).expect("post-violation delete succeeds");

    // by_buyer + by_seller both gone.
    let by_buyer = sla_by_buyer_key(&buyer.id, HEIGHT_PROPOSE, &sla_id);
    assert!(db.get(&by_buyer).unwrap().is_none());
    let by_seller = sla_by_seller_key(&seller.id, HEIGHT_PROPOSE, &sla_id);
    assert!(db.get(&by_seller).unwrap().is_none());
}

#[test]
fn sla_delete_proposed_frees_slot_for_new_sla_with_same_pair() {
    // Phase 1 enforced one-open-SLA-per-pair via the active_between
    // singleton. After delete, a new proposal between the same pair
    // must succeed because the singleton is gone.
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x1Du8; 32], [0x2Du8; 32]);
    let seller = make_seller(&mut db, [0x1Eu8; 32], [0x2Eu8; 32], SELLER_STAKE);
    let sla = sample_sla(&buyer, &seller, SLASH_AMOUNT);
    let sla_id_1 = propose_sla(&mut db, &buyer, 0, &sla);

    // Delete the still-Proposed SLA.
    let delete_tx = make_delete_tx(&buyer, 1, sla_id_1);
    apply_delete_memory_object_tx(&mut db, &delete_tx, HEIGHT_PROPOSE + 5).unwrap();

    // Same (buyer, seller) pair, different terms. Must succeed.
    let mut sla_2 = sample_sla(&buyer, &seller, SLASH_AMOUNT * 2);
    sla_2.violation_threshold = 5;
    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::SlaAgreement,
        data: sla_2.encode().to_vec(),
    });
    let tx = TxV1 {
        version: TxVersion::V1,
        from: buyer.id,
        pubkey: buyer.id,
        nonce: 2,
        fee: FEE,
        payload,
        sig: [0u8; 64],
    };
    apply_create_memory_object_tx(&mut db, &tx, HEIGHT_PROPOSE + 10).expect("new SLA same pair");
}

// ============================================================================
// 3. StakeWithdraw collateral gate (Q1 Option B)
// ============================================================================

#[test]
fn stake_withdraw_under_committed_collateral_rejected() {
    // Stake-locked-until is 0 (set by make_seller), so the
    // StakeStillLocked gate is inert and the collateral gate is the
    // one we exercise. Seller has stake_balance = 2 * slash_amount;
    // an active SLA commits slash_amount; therefore withdraw can
    // pull at most slash_amount, and anything more is rejected.
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x31u8; 32], [0x41u8; 32]);
    let seller = make_seller(&mut db, [0x32u8; 32], [0x42u8; 32], SLASH_AMOUNT * 2);
    let sla = sample_sla(&buyer, &seller, SLASH_AMOUNT);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    // Try to withdraw MORE than the slack: slack = stake - committed =
    // 2*SLASH - SLASH = SLASH. Asking for SLASH + 1 must fail.
    let tx = make_withdraw_tx(&seller, 1, SLASH_AMOUNT + 1);
    let err = apply_signal_commitment_tx(&mut db, &tx, STAKE_UNLOCK_HEIGHT).unwrap_err();
    assert!(matches!(
        err,
        ExecError::StakeWithdrawWouldUnderfundSlaCollateral {
            required,
            available_after_withdraw
        }
        if required == SLASH_AMOUNT && available_after_withdraw == SLASH_AMOUNT - 1
    ));
}

#[test]
fn stake_withdraw_exactly_at_slack_allowed() {
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x33u8; 32], [0x43u8; 32]);
    let seller = make_seller(&mut db, [0x34u8; 32], [0x44u8; 32], SLASH_AMOUNT * 2);
    let sla = sample_sla(&buyer, &seller, SLASH_AMOUNT);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    // Withdraw exactly the slack: stake_balance after = committed.
    let tx = make_withdraw_tx(&seller, 1, SLASH_AMOUNT);
    apply_signal_commitment_tx(&mut db, &tx, STAKE_UNLOCK_HEIGHT).expect("withdrawal at slack");
}

#[test]
fn stake_withdraw_with_no_active_slas_unconstrained() {
    // No active SLAs as seller: collateral = 0. Withdrawal can drain
    // the full stake_balance.
    let mut db = MemKv::new();
    let _buyer = make_buyer(&mut db, [0x35u8; 32], [0x45u8; 32]);
    let seller = make_seller(&mut db, [0x36u8; 32], [0x46u8; 32], SELLER_STAKE);

    let tx = make_withdraw_tx(&seller, 0, SELLER_STAKE);
    apply_signal_commitment_tx(&mut db, &tx, STAKE_UNLOCK_HEIGHT).expect("unconstrained withdraw");
}

#[test]
fn stake_withdraw_with_expired_sla_unconstrained() {
    // Active-status SLA but `current_height > end_height`: the
    // committed-collateral sum skips it, so the withdrawal is
    // unconstrained.
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x37u8; 32], [0x47u8; 32]);
    let seller = make_seller(&mut db, [0x38u8; 32], [0x48u8; 32], SLASH_AMOUNT * 2);
    let sla = sample_sla(&buyer, &seller, SLASH_AMOUNT);
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    // Withdraw past SLA_END: the active SLA is expired, no longer
    // contributes to committed collateral.
    let tx = make_withdraw_tx(&seller, 1, SLASH_AMOUNT * 2);
    apply_signal_commitment_tx(&mut db, &tx, SLA_END + 1).expect("expired SLA does not commit");
}

#[test]
fn stake_withdraw_after_auto_slash_unconstrained() {
    // After auto-slash the SLA is in SLA_STATUS_VIOLATED. The
    // collateral sum filter is "Active AND in-window"; Violated
    // does not count even if still inside the original window.
    let mut db = MemKv::new();
    let buyer = make_buyer(&mut db, [0x39u8; 32], [0x49u8; 32]);
    let seller = make_seller(&mut db, [0x3Au8; 32], [0x4Au8; 32], SLASH_AMOUNT * 2);
    let mut sla = sample_sla(&buyer, &seller, SLASH_AMOUNT);
    sla.violation_threshold = 1;
    let sla_id = propose_sla(&mut db, &buyer, 0, &sla);
    accept_sla(&mut db, &seller, 0, sla_id, buyer.id);

    let signal_hash = [0xD1u8; 32];
    seed_payment(&mut db, &buyer, &seller, signal_hash, 600);
    attest_failed(&mut db, &buyer, &seller, signal_hash, 1, 700);

    // Seller has stake_balance = 2*SLASH - SLASH = SLASH after slash.
    // Withdraw the full remaining; no committed collateral remains.
    let tx = make_withdraw_tx(&seller, 1, SLASH_AMOUNT);
    apply_signal_commitment_tx(&mut db, &tx, STAKE_UNLOCK_HEIGHT)
        .expect("post-slash withdraw unconstrained");
}

#[test]
fn stake_withdraw_sums_across_multiple_active_slas() {
    // Two distinct buyers, both have active SLAs against the same
    // seller. Total committed collateral = sum of slash_amounts.
    // Withdraw beyond the slack must be rejected.
    let mut db = MemKv::new();
    let buyer1 = make_buyer(&mut db, [0x3Bu8; 32], [0x4Bu8; 32]);
    let buyer2 = make_buyer(&mut db, [0x3Cu8; 32], [0x4Cu8; 32]);
    let seller = make_seller(&mut db, [0x3Du8; 32], [0x4Du8; 32], SLASH_AMOUNT * 5);

    // SLA #1: buyer1 -> seller, slash = SLASH_AMOUNT.
    let sla1 = sample_sla(&buyer1, &seller, SLASH_AMOUNT);
    let id1 = propose_sla(&mut db, &buyer1, 0, &sla1);
    accept_sla(&mut db, &seller, 0, id1, buyer1.id);

    // SLA #2: buyer2 -> seller, slash = 2 * SLASH_AMOUNT.
    let sla2 = sample_sla(&buyer2, &seller, SLASH_AMOUNT * 2);
    let id2 = propose_sla(&mut db, &buyer2, 0, &sla2);
    accept_sla(&mut db, &seller, 1, id2, buyer2.id);

    // Total committed = 3 * SLASH_AMOUNT. Stake = 5 * SLASH_AMOUNT.
    // Slack = 2 * SLASH_AMOUNT. Withdrawing 2 * SLASH_AMOUNT + 1 must fail.
    let tx = make_withdraw_tx(&seller, 2, SLASH_AMOUNT * 2 + 1);
    let err = apply_signal_commitment_tx(&mut db, &tx, STAKE_UNLOCK_HEIGHT).unwrap_err();
    assert!(matches!(
        err,
        ExecError::StakeWithdrawWouldUnderfundSlaCollateral { required, .. }
            if required == SLASH_AMOUNT * 3
    ));

    // Withdrawing exactly 2 * SLASH_AMOUNT (the slack) is allowed.
    let tx_ok = make_withdraw_tx(&seller, 2, SLASH_AMOUNT * 2);
    apply_signal_commitment_tx(&mut db, &tx_ok, STAKE_UNLOCK_HEIGHT).expect("withdraw at slack");
}
