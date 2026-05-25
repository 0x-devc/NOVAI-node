#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::too_many_arguments)]

//! Week 35 Phase 5: adversarial tests for the `OracleAnchor` signal,
//! driven through the public `apply_signal_commitment_tx` entry point.
//!
//! Covers the abuse and edge cases: posting without the capability,
//! a zero data_hash, posting after deactivation, the determinism edge of
//! an arbitrarily large external timestamp (no on-chain wall-clock bound),
//! a zero timestamp, high-frequency posting (no rate limit in v1), and the
//! global cross-entity replay guard.

use novai_ai_entities::AiSignalType;
use novai_ai_entities::{AiEntity, AutonomyMode, Capabilities};
use novai_execution::{
    apply_signal_commitment_tx, decode_oracle_anchor_record_v1, decode_oracle_anchor_summary_v1,
    encode_signal_commitment_payload_v1, oracle_anchor_by_hash_key, oracle_anchor_summary_key,
    write_ai_entity_op, ExecError, OracleAnchorExtraV1, SignalCommitmentPayloadV1,
};
use novai_state::{ai_entity_by_address_key, Kv, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

const BALANCE: u128 = 10_000_000;
const FEE: u64 = 1_000;
const HEIGHT: u64 = 1000;
const TS: u64 = 1_700_000_000;

fn setup(db: &mut MemKv, code: u8, caps: Capabilities) -> AiEntity {
    let mut e = AiEntity::new([code; 32], [0x01; 32], AutonomyMode::Advisory, caps, HEIGHT);
    e.economic_balance = BALANCE;
    db.apply_batch(&[
        write_ai_entity_op(&e),
        WriteOp::Put(ai_entity_by_address_key(&e.id), e.id.to_vec()),
    ])
    .unwrap();
    e
}

fn anchor_tx(
    issuer: [u8; 32],
    nonce: u64,
    signal_hash: [u8; 32],
    data_hash: [u8; 32],
    external_timestamp: u64,
    tag: &[u8],
) -> TxV1 {
    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
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
            external_timestamp,
            source_hash: [0u8; 32],
            expiry_height: 0,
            data_tag: tag.to_vec(),
        }),
    });
    TxV1 {
        version: TxVersion::V1,
        from: issuer,
        pubkey: issuer,
        nonce,
        fee: FEE,
        payload,
        sig: [0u8; 64],
    }
}

#[test]
fn entity_without_oracle_capability_cannot_post() {
    let mut db = MemKv::new();
    // advisory() carries emit_proposals (passes the generic signal gate)
    // but NOT post_oracle_anchors.
    let e = setup(&mut db, 0x42, Capabilities::advisory());
    let sh = [0x10u8; 32];
    let res = apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(e.id, 0, sh, [0xAB; 32], TS, b"x"),
        HEIGHT,
    );
    assert!(matches!(res, Err(ExecError::IssuerMissingCapability)));
    assert!(db.get(&oracle_anchor_by_hash_key(&sh)).unwrap().is_none());
}

#[test]
fn zero_data_hash_rejected() {
    let mut db = MemKv::new();
    let e = setup(&mut db, 0x42, Capabilities::oracle());
    let sh = [0x10u8; 32];
    let res = apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(e.id, 0, sh, [0u8; 32], TS, b"x"),
        HEIGHT,
    );
    assert!(matches!(res, Err(ExecError::OracleAnchorZeroDataHash)));
    assert!(db.get(&oracle_anchor_by_hash_key(&sh)).unwrap().is_none());
}

#[test]
fn post_after_deactivation_rejected() {
    let mut db = MemKv::new();
    let mut e = setup(&mut db, 0x42, Capabilities::oracle());
    // Deactivate the entity (as ModuleRollback / kill would).
    e.is_active = false;
    db.apply_batch(&[write_ai_entity_op(&e)]).unwrap();
    let sh = [0x10u8; 32];
    let res = apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(e.id, 0, sh, [0xAB; 32], TS, b"x"),
        HEIGHT,
    );
    assert!(matches!(res, Err(ExecError::EntityNotActive)));
    assert!(db.get(&oracle_anchor_by_hash_key(&sh)).unwrap().is_none());
}

#[test]
fn far_future_external_timestamp_accepted_verbatim() {
    // The chain has no deterministic wall-clock, so it cannot (and must
    // not) reject a "future" external timestamp. Any non-zero value is
    // accepted and stored verbatim; freshness is a consumer concern.
    let mut db = MemKv::new();
    let e = setup(&mut db, 0x42, Capabilities::oracle());
    let sh = [0x10u8; 32];
    apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(e.id, 0, sh, [0xAB; 32], u64::MAX, b"price/ETH-USD"),
        HEIGHT,
    )
    .expect("far-future timestamp accepted");
    let rec =
        decode_oracle_anchor_record_v1(&db.get(&oracle_anchor_by_hash_key(&sh)).unwrap().unwrap())
            .unwrap();
    assert_eq!(rec.external_timestamp, u64::MAX);
    assert_eq!(rec.anchor_height, HEIGHT);
}

#[test]
fn zero_external_timestamp_rejected() {
    let mut db = MemKv::new();
    let e = setup(&mut db, 0x42, Capabilities::oracle());
    let sh = [0x10u8; 32];
    let res = apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(e.id, 0, sh, [0xAB; 32], 0, b"x"),
        HEIGHT,
    );
    assert!(matches!(res, Err(ExecError::OracleAnchorZeroTimestamp)));
    assert!(db.get(&oracle_anchor_by_hash_key(&sh)).unwrap().is_none());
}

#[test]
fn many_rapid_anchors_all_succeed_no_rate_limit() {
    // v1 has no per-entity rate limit: the per-post fee is the spam
    // control. A high-frequency oracle posts on consecutive heights and
    // every post lands.
    let mut db = MemKv::new();
    let e = setup(&mut db, 0x42, Capabilities::oracle());
    let n: u64 = 10;
    for i in 0..n {
        let mut sh = [0u8; 32];
        sh[0..8].copy_from_slice(&i.to_be_bytes());
        apply_signal_commitment_tx(
            &mut db,
            &anchor_tx(e.id, i, sh, [0xAB; 32], TS + i, b"price/ETH-USD"),
            HEIGHT + i,
        )
        .expect("rapid post lands");
    }
    let summary = decode_oracle_anchor_summary_v1(
        &db.get(&oracle_anchor_summary_key(&e.id)).unwrap().unwrap(),
    )
    .unwrap();
    assert_eq!(summary.anchor_count, u32::try_from(n).unwrap());
    assert_eq!(summary.last_anchor_height, HEIGHT + n - 1);
}

#[test]
fn replay_guard_is_global_across_entities() {
    // The by-hash record is a GLOBAL dedup namespace keyed on signal_hash.
    // Once entity A posts under signal_hash H, no entity (not even a
    // different one crafting a different-content payload) can post under H.
    let mut db = MemKv::new();
    let a = setup(&mut db, 0x42, Capabilities::oracle());
    let b = setup(&mut db, 0x43, Capabilities::oracle());
    let h = [0x10u8; 32];
    apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(a.id, 0, h, [0xAB; 32], TS, b"price/ETH-USD"),
        HEIGHT,
    )
    .expect("A posts");

    // B reuses H with entirely different content; rejected by the guard.
    let res = apply_signal_commitment_tx(
        &mut db,
        &anchor_tx(b.id, 0, h, [0x99; 32], TS + 1, b"api/weather"),
        HEIGHT + 1,
    );
    assert!(matches!(
        res,
        Err(ExecError::OracleAnchorAlreadyExists { signal_hash }) if signal_hash == h
    ));

    // A's original record is intact (B's attempt mutated nothing).
    let rec =
        decode_oracle_anchor_record_v1(&db.get(&oracle_anchor_by_hash_key(&h)).unwrap().unwrap())
            .unwrap();
    assert_eq!(rec.issuer_entity_id, a.id);
    assert_eq!(rec.data_hash, [0xAB; 32]);
    assert_eq!(rec.data_tag, b"price/ETH-USD");
}
