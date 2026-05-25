#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

//! Week 35 Phase 2: validation tests for `validate_oracle_anchor`.
//!
//! Each test exercises one rule of the validator in isolation against a
//! freshly constructed issuer entity and an in-memory KV. The happy paths
//! confirm that the min (1-byte) and max (32-byte) tag boundaries pass and
//! that a well-formed anchor from an entity holding `post_oracle_anchors`
//! is accepted. The validator performs no state mutation; Phase 3 wires it
//! into the signal handler.

use novai_ai_entities::{AiEntity, AutonomyMode, Capabilities};
use novai_execution::{
    encode_oracle_anchor_record_v1, oracle_anchor_by_hash_key, validate_oracle_anchor, ExecError,
    OracleAnchorExtraV1, OracleAnchorRecord, ORACLE_ANCHOR_DATA_TAG_MAX_LEN,
};
use novai_state::{KvBatch, MemKv, WriteOp};

const HEIGHT: u64 = 1000;
const SIGNAL_HASH: [u8; 32] = [0x10; 32];

fn oracle_entity() -> AiEntity {
    AiEntity::new(
        [0x42; 32],
        [0x01; 32],
        AutonomyMode::Advisory,
        Capabilities::oracle(),
        HEIGHT,
    )
}

fn entity_with(caps: Capabilities) -> AiEntity {
    AiEntity::new([0x42; 32], [0x01; 32], AutonomyMode::Advisory, caps, HEIGHT)
}

fn extra(tag: &[u8]) -> OracleAnchorExtraV1 {
    OracleAnchorExtraV1 {
        data_hash: [0xAB; 32],
        external_timestamp: 1_700_000_000,
        source_hash: [0xCD; 32],
        expiry_height: 0,
        data_tag: tag.to_vec(),
    }
}

#[test]
fn happy_path_accepts_valid_anchor() {
    let db = MemKv::new();
    let issuer = oracle_entity();
    assert!(
        validate_oracle_anchor(&db, &issuer, HEIGHT, &SIGNAL_HASH, &extra(b"price/ETH-USD"))
            .is_ok()
    );
}

#[test]
fn min_tag_length_one_accepted() {
    let db = MemKv::new();
    let issuer = oracle_entity();
    assert!(validate_oracle_anchor(&db, &issuer, HEIGHT, &SIGNAL_HASH, &extra(b"x")).is_ok());
}

#[test]
fn max_tag_length_32_accepted() {
    let db = MemKv::new();
    let issuer = oracle_entity();
    let tag = vec![0x5A; ORACLE_ANCHOR_DATA_TAG_MAX_LEN];
    assert!(validate_oracle_anchor(&db, &issuer, HEIGHT, &SIGNAL_HASH, &extra(&tag)).is_ok());
}

#[test]
fn missing_oracle_capability_rejected() {
    // advisory() has emit_proposals but NOT post_oracle_anchors.
    let db = MemKv::new();
    let issuer = entity_with(Capabilities::advisory());
    assert!(matches!(
        validate_oracle_anchor(&db, &issuer, HEIGHT, &SIGNAL_HASH, &extra(b"x")),
        Err(ExecError::IssuerMissingCapability)
    ));
}

#[test]
fn reputation_oracle_without_anchor_capability_rejected() {
    // submit_reputation_updates is a DIFFERENT trust domain; it must not
    // grant anchor posting (distinct capability bits, distinct concerns).
    let db = MemKv::new();
    let issuer = entity_with(Capabilities {
        submit_reputation_updates: true,
        ..Capabilities::default()
    });
    assert!(matches!(
        validate_oracle_anchor(&db, &issuer, HEIGHT, &SIGNAL_HASH, &extra(b"x")),
        Err(ExecError::IssuerMissingCapability)
    ));
}

#[test]
fn inactive_issuer_rejected() {
    let db = MemKv::new();
    let mut issuer = oracle_entity();
    issuer.is_active = false;
    assert!(matches!(
        validate_oracle_anchor(&db, &issuer, HEIGHT, &SIGNAL_HASH, &extra(b"x")),
        Err(ExecError::EntityNotActive)
    ));
}

#[test]
fn zero_data_hash_rejected() {
    let db = MemKv::new();
    let issuer = oracle_entity();
    let mut e = extra(b"x");
    e.data_hash = [0u8; 32];
    assert!(matches!(
        validate_oracle_anchor(&db, &issuer, HEIGHT, &SIGNAL_HASH, &e),
        Err(ExecError::OracleAnchorZeroDataHash)
    ));
}

#[test]
fn zero_timestamp_rejected() {
    let db = MemKv::new();
    let issuer = oracle_entity();
    let mut e = extra(b"x");
    e.external_timestamp = 0;
    assert!(matches!(
        validate_oracle_anchor(&db, &issuer, HEIGHT, &SIGNAL_HASH, &e),
        Err(ExecError::OracleAnchorZeroTimestamp)
    ));
}

#[test]
fn empty_tag_rejected() {
    let db = MemKv::new();
    let issuer = oracle_entity();
    assert!(matches!(
        validate_oracle_anchor(&db, &issuer, HEIGHT, &SIGNAL_HASH, &extra(b"")),
        Err(ExecError::OracleAnchorInvalidTag { len: 0 })
    ));
}

#[test]
fn oversized_tag_rejected() {
    let db = MemKv::new();
    let issuer = oracle_entity();
    let tag = vec![0x5A; ORACLE_ANCHOR_DATA_TAG_MAX_LEN + 1];
    assert!(matches!(
        validate_oracle_anchor(&db, &issuer, HEIGHT, &SIGNAL_HASH, &extra(&tag)),
        Err(ExecError::OracleAnchorInvalidTag { len: 33 })
    ));
}

#[test]
fn duplicate_signal_hash_rejected() {
    let mut db = MemKv::new();
    let issuer = oracle_entity();
    // Pre-write a by-hash record to simulate an already-posted anchor.
    let rec = OracleAnchorRecord {
        issuer_entity_id: issuer.id,
        data_hash: [0xAB; 32],
        external_timestamp: 1_700_000_000,
        source_hash: [0xCD; 32],
        expiry_height: 0,
        anchor_height: HEIGHT,
        data_tag: b"x".to_vec(),
    };
    db.apply_batch(&[WriteOp::Put(
        oracle_anchor_by_hash_key(&SIGNAL_HASH),
        encode_oracle_anchor_record_v1(&rec),
    )])
    .unwrap();
    assert!(matches!(
        validate_oracle_anchor(&db, &issuer, HEIGHT, &SIGNAL_HASH, &extra(b"x")),
        Err(ExecError::OracleAnchorAlreadyExists { signal_hash }) if signal_hash == SIGNAL_HASH
    ));
}

#[test]
fn capability_checked_before_field_rules() {
    // Issuer lacks the capability AND the extra has a zero data_hash; the
    // capability rejection must fire first (capability is checked before
    // the field rules).
    let db = MemKv::new();
    let issuer = entity_with(Capabilities::advisory());
    let mut e = extra(b"x");
    e.data_hash = [0u8; 32];
    assert!(matches!(
        validate_oracle_anchor(&db, &issuer, HEIGHT, &SIGNAL_HASH, &e),
        Err(ExecError::IssuerMissingCapability)
    ));
}

#[test]
fn data_hash_checked_before_timestamp() {
    let db = MemKv::new();
    let issuer = oracle_entity();
    let mut e = extra(b"x");
    e.data_hash = [0u8; 32];
    e.external_timestamp = 0;
    assert!(matches!(
        validate_oracle_anchor(&db, &issuer, HEIGHT, &SIGNAL_HASH, &e),
        Err(ExecError::OracleAnchorZeroDataHash)
    ));
}
