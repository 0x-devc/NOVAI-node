#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::too_many_arguments)]

//! Week 35 Phase 3: end-to-end execution tests for the `OracleAnchor`
//! signal handler, driven through the public `apply_signal_commitment_tx`
//! entry point.
//!
//! Covers: the canonical by-hash record and its fields, the by-entity and
//! by-tag scan markers, the per-entity summary, fee debit with neutral
//! reputation, multiple anchors from one entity, multiple entities sharing
//! a tag, the replay guard with no state change on a rejected duplicate,
//! and the capability / field / active-entity rejections.

use novai_ai_entities::AiSignalType;
use novai_ai_entities::{AiEntity, AutonomyMode, Capabilities, DEFAULT_REPUTATION_SCORE};
use novai_execution::{
    apply_signal_commitment_tx, decode_oracle_anchor_record_v1, decode_oracle_anchor_summary_v1,
    encode_signal_commitment_payload_v1, oracle_anchor_by_entity_key, oracle_anchor_by_hash_key,
    oracle_anchor_by_tag_key, oracle_anchor_summary_key, oracle_anchor_tag_hash, read_ai_entity,
    write_ai_entity_op, ExecError, OracleAnchorExtraV1, SignalCommitmentPayloadV1,
};
use novai_state::{ai_entity_by_address_key, Kv, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const BALANCE: u128 = 1_000_000;
const FEE: u64 = 1_000;
const HEIGHT: u64 = 1000;
const TS: u64 = 1_700_000_000;

fn setup_oracle(db: &mut MemKv, code: u8, caps: Capabilities) -> AiEntity {
    let mut e = AiEntity::new([code; 32], [0x01; 32], AutonomyMode::Advisory, caps, HEIGHT);
    e.economic_balance = BALANCE;
    db.apply_batch(&[
        write_ai_entity_op(&e),
        WriteOp::Put(ai_entity_by_address_key(&e.id), e.id.to_vec()),
    ])
    .unwrap();
    e
}

fn build_anchor_payload(
    issuer: [u8; 32],
    signal_hash: [u8; 32],
    data_hash: [u8; 32],
    source_hash: [u8; 32],
    expiry_height: u64,
    tag: &[u8],
) -> Vec<u8> {
    encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type: AiSignalType::OracleAnchor,
        issuer_entity_id: issuer,
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
        channel_finalize: None,
        oracle_anchor: Some(OracleAnchorExtraV1 {
            data_hash,
            external_timestamp: TS,
            source_hash,
            expiry_height,
            data_tag: tag.to_vec(),
        }),
    })
}

fn anchor_tx(
    issuer: [u8; 32],
    nonce: u64,
    signal_hash: [u8; 32],
    data_hash: [u8; 32],
    source_hash: [u8; 32],
    expiry_height: u64,
    tag: &[u8],
) -> TxV1 {
    TxV1 {
        version: TxVersion::V1,
        from: issuer,
        pubkey: issuer,
        nonce,
        fee: FEE,
        payload: build_anchor_payload(
            issuer,
            signal_hash,
            data_hash,
            source_hash,
            expiry_height,
            tag,
        ),
        sig: [0u8; 64],
    }
}

#[test]
fn happy_path_writes_canonical_record_with_all_fields() {
    let mut db = MemKv::new();
    let oracle = setup_oracle(&mut db, 0x42, Capabilities::oracle());
    let sh = [0x10u8; 32];
    let tx = anchor_tx(
        oracle.id,
        0,
        sh,
        [0xAB; 32],
        [0xCD; 32],
        5000,
        b"price/ETH-USD",
    );
    apply_signal_commitment_tx(&mut db, &tx, HEIGHT).expect("post ok");

    let bytes = db
        .get(&oracle_anchor_by_hash_key(&sh))
        .unwrap()
        .expect("record present");
    let rec = decode_oracle_anchor_record_v1(&bytes).expect("decodes");
    assert_eq!(rec.issuer_entity_id, oracle.id);
    assert_eq!(rec.data_hash, [0xAB; 32]);
    assert_eq!(rec.external_timestamp, TS);
    assert_eq!(rec.source_hash, [0xCD; 32]);
    assert_eq!(rec.expiry_height, 5000);
    assert_eq!(rec.anchor_height, HEIGHT);
    assert_eq!(rec.data_tag, b"price/ETH-USD");
}

#[test]
fn anchor_height_is_chain_height_not_external_timestamp() {
    let mut db = MemKv::new();
    let oracle = setup_oracle(&mut db, 0x42, Capabilities::oracle());
    let sh = [0x10u8; 32];
    let tx = anchor_tx(oracle.id, 0, sh, [0xAB; 32], [0u8; 32], 0, b"api/weather");
    apply_signal_commitment_tx(&mut db, &tx, HEIGHT).expect("post ok");
    let rec =
        decode_oracle_anchor_record_v1(&db.get(&oracle_anchor_by_hash_key(&sh)).unwrap().unwrap())
            .unwrap();
    // anchor_height is the deterministic chain height; external_timestamp is
    // opaque oracle metadata and the two are independent.
    assert_eq!(rec.anchor_height, HEIGHT);
    assert_ne!(rec.anchor_height, rec.external_timestamp);
}

#[test]
fn happy_path_writes_by_entity_and_by_tag_markers() {
    let mut db = MemKv::new();
    let oracle = setup_oracle(&mut db, 0x42, Capabilities::oracle());
    let sh = [0x10u8; 32];
    let tag = b"price/ETH-USD";
    let tx = anchor_tx(oracle.id, 0, sh, [0xAB; 32], [0u8; 32], 0, tag);
    apply_signal_commitment_tx(&mut db, &tx, HEIGHT).expect("post ok");

    assert!(db
        .get(&oracle_anchor_by_entity_key(&oracle.id, HEIGHT, &sh))
        .unwrap()
        .is_some());
    let tag_hash = oracle_anchor_tag_hash(tag);
    assert!(db
        .get(&oracle_anchor_by_tag_key(&tag_hash, HEIGHT, &sh))
        .unwrap()
        .is_some());
}

#[test]
fn happy_path_writes_summary() {
    let mut db = MemKv::new();
    let oracle = setup_oracle(&mut db, 0x42, Capabilities::oracle());
    let tx = anchor_tx(oracle.id, 0, [0x10; 32], [0xAB; 32], [0u8; 32], 0, b"x");
    apply_signal_commitment_tx(&mut db, &tx, HEIGHT).expect("post ok");

    let summary = decode_oracle_anchor_summary_v1(
        &db.get(&oracle_anchor_summary_key(&oracle.id))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(summary.anchor_count, 1);
    assert_eq!(summary.last_anchor_height, HEIGHT);
}

#[test]
fn fee_debited_and_reputation_neutral() {
    let mut db = MemKv::new();
    let oracle = setup_oracle(&mut db, 0x42, Capabilities::oracle());
    let tx = anchor_tx(oracle.id, 0, [0x10; 32], [0xAB; 32], [0u8; 32], 0, b"x");
    apply_signal_commitment_tx(&mut db, &tx, HEIGHT).expect("post ok");

    let after = read_ai_entity(&db, &oracle.id).unwrap().unwrap();
    assert_eq!(after.economic_balance, BALANCE - u128::from(FEE));
    assert_eq!(after.reputation_score, DEFAULT_REPUTATION_SCORE); // unchanged
    assert_eq!(after.total_transactions, 1); // activity counted
    assert_eq!(after.nonce, 1); // bumped by the dispatch
}

#[test]
fn multiple_anchors_same_entity_increment_summary() {
    let mut db = MemKv::new();
    let oracle = setup_oracle(&mut db, 0x42, Capabilities::oracle());
    let sh1 = [0x10u8; 32];
    let sh2 = [0x20u8; 32];
    apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(
            oracle.id,
            0,
            sh1,
            [0xAB; 32],
            [0u8; 32],
            0,
            b"price/ETH-USD",
        ),
        HEIGHT,
    )
    .expect("post 1 ok");
    apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(
            oracle.id,
            1,
            sh2,
            [0xCD; 32],
            [0u8; 32],
            0,
            b"price/BTC-USD",
        ),
        HEIGHT + 100,
    )
    .expect("post 2 ok");

    let summary = decode_oracle_anchor_summary_v1(
        &db.get(&oracle_anchor_summary_key(&oracle.id))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(summary.anchor_count, 2);
    assert_eq!(summary.last_anchor_height, HEIGHT + 100);
    // Both by-entity markers present at their respective heights.
    assert!(db
        .get(&oracle_anchor_by_entity_key(&oracle.id, HEIGHT, &sh1))
        .unwrap()
        .is_some());
    assert!(db
        .get(&oracle_anchor_by_entity_key(&oracle.id, HEIGHT + 100, &sh2))
        .unwrap()
        .is_some());
}

#[test]
fn multiple_entities_same_tag_share_tag_index() {
    let mut db = MemKv::new();
    let a = setup_oracle(&mut db, 0x42, Capabilities::oracle());
    let b = setup_oracle(&mut db, 0x43, Capabilities::oracle());
    let tag = b"price/ETH-USD";
    let tag_hash = oracle_anchor_tag_hash(tag);
    let sh_a = [0x10u8; 32];
    let sh_b = [0x20u8; 32];
    apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(a.id, 0, sh_a, [0xAB; 32], [0u8; 32], 0, tag),
        HEIGHT,
    )
    .expect("a ok");
    apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(b.id, 0, sh_b, [0xCD; 32], [0u8; 32], 0, tag),
        HEIGHT,
    )
    .expect("b ok");

    // Both anchors are indexed under the same tag hash, distinct records.
    assert!(db
        .get(&oracle_anchor_by_tag_key(&tag_hash, HEIGHT, &sh_a))
        .unwrap()
        .is_some());
    assert!(db
        .get(&oracle_anchor_by_tag_key(&tag_hash, HEIGHT, &sh_b))
        .unwrap()
        .is_some());
    let ra = decode_oracle_anchor_record_v1(
        &db.get(&oracle_anchor_by_hash_key(&sh_a)).unwrap().unwrap(),
    )
    .unwrap();
    let rb = decode_oracle_anchor_record_v1(
        &db.get(&oracle_anchor_by_hash_key(&sh_b)).unwrap().unwrap(),
    )
    .unwrap();
    assert_eq!(ra.issuer_entity_id, a.id);
    assert_eq!(rb.issuer_entity_id, b.id);
}

#[test]
fn duplicate_signal_hash_rejected_with_no_state_change() {
    let mut db = MemKv::new();
    let oracle = setup_oracle(&mut db, 0x42, Capabilities::oracle());
    let sh = [0x10u8; 32];
    apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(oracle.id, 0, sh, [0xAB; 32], [0u8; 32], 0, b"x"),
        HEIGHT,
    )
    .expect("first ok");
    let after_first = read_ai_entity(&db, &oracle.id).unwrap().unwrap();

    // Second post with the SAME signal_hash (nonce now 1) must be rejected.
    let res = apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(oracle.id, 1, sh, [0x99; 32], [0u8; 32], 0, b"y"),
        HEIGHT + 1,
    );
    assert!(matches!(
        res,
        Err(ExecError::OracleAnchorAlreadyExists { signal_hash }) if signal_hash == sh
    ));

    // No state change from the rejected attempt: the standalone wrapper
    // commits a single batch only on success, so balance/nonce/summary are
    // exactly as they were after the first post, and the record is the
    // original (data_hash 0xAB, not the 0x99 from the rejected attempt).
    let after_second = read_ai_entity(&db, &oracle.id).unwrap().unwrap();
    assert_eq!(after_second.economic_balance, after_first.economic_balance);
    assert_eq!(after_second.nonce, after_first.nonce);
    let rec =
        decode_oracle_anchor_record_v1(&db.get(&oracle_anchor_by_hash_key(&sh)).unwrap().unwrap())
            .unwrap();
    assert_eq!(rec.data_hash, [0xAB; 32]);
    let summary = decode_oracle_anchor_summary_v1(
        &db.get(&oracle_anchor_summary_key(&oracle.id))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(summary.anchor_count, 1);
}

#[test]
fn missing_capability_rejected_no_record() {
    let mut db = MemKv::new();
    // advisory(): emit_proposals (passes the generic signal gate) but no
    // post_oracle_anchors (fails validate_oracle_anchor).
    let oracle = setup_oracle(&mut db, 0x42, Capabilities::advisory());
    let sh = [0x10u8; 32];
    let res = apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(oracle.id, 0, sh, [0xAB; 32], [0u8; 32], 0, b"x"),
        HEIGHT,
    );
    assert!(matches!(res, Err(ExecError::IssuerMissingCapability)));
    assert!(db.get(&oracle_anchor_by_hash_key(&sh)).unwrap().is_none());
}

#[test]
fn zero_data_hash_rejected_no_record() {
    let mut db = MemKv::new();
    let oracle = setup_oracle(&mut db, 0x42, Capabilities::oracle());
    let sh = [0x10u8; 32];
    let res = apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(oracle.id, 0, sh, [0u8; 32], [0u8; 32], 0, b"x"),
        HEIGHT,
    );
    assert!(matches!(res, Err(ExecError::OracleAnchorZeroDataHash)));
    assert!(db.get(&oracle_anchor_by_hash_key(&sh)).unwrap().is_none());
}

#[test]
fn inactive_entity_rejected_by_dispatch_gate() {
    let mut db = MemKv::new();
    let mut oracle = setup_oracle(&mut db, 0x42, Capabilities::oracle());
    oracle.is_active = false;
    db.apply_batch(&[write_ai_entity_op(&oracle)]).unwrap();
    let res = apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(oracle.id, 0, [0x10; 32], [0xAB; 32], [0u8; 32], 0, b"x"),
        HEIGHT,
    );
    assert!(matches!(res, Err(ExecError::EntityNotActive)));
}

#[test]
fn min_and_max_tag_lengths_post_successfully() {
    let mut db = MemKv::new();
    let oracle = setup_oracle(&mut db, 0x42, Capabilities::oracle());
    apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(oracle.id, 0, [0x10; 32], [0xAB; 32], [0u8; 32], 0, b"a"),
        HEIGHT,
    )
    .expect("min tag ok");
    let max_tag = vec![0x5Au8; 32];
    apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(oracle.id, 1, [0x20; 32], [0xCD; 32], [0u8; 32], 0, &max_tag),
        HEIGHT + 1,
    )
    .expect("max tag ok");
    let rec = decode_oracle_anchor_record_v1(
        &db.get(&oracle_anchor_by_hash_key(&[0x20; 32]))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(rec.data_tag, max_tag);
}
