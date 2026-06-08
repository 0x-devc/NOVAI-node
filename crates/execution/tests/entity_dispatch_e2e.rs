#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

//! End-to-end tests for the `dispatch_tx` → handler path on entity-signed txs.
//!
//! Regression guard for the silent-fail bug where `apply_signal_commitment_tx`
//! and the three memory handlers re-resolved the entity by primary-key lookup
//! using `tx.from`. For entity-signed txs `tx.from` is
//! `address_from_pubkey(entity.pubkey)`, not the canonical
//! `entity.id = compute_id(code_hash, creator)`. Handlers now receive the
//! entity from the dispatcher (resolved via the address→id reverse index) and
//! key all storage on `entity.id`. These tests exercise the full path.

use novai_ai_entities::{AiSignalType, AutonomyMode, Capabilities, MemoryObjectType};
use novai_execution::{
    apply_register_ai_entity_with_key_tx, dispatch_tx, encode_create_memory_object_payload_v1,
    encode_delete_memory_object_payload_v1, encode_register_ai_entity_with_key_payload_v1,
    encode_signal_commitment_payload_v1, encode_update_memory_object_payload_v1,
    get_memory_objects_by_entity, get_signals_by_issuer, read_ai_entity,
    CreateMemoryObjectPayloadV1, DeleteMemoryObjectPayloadV1, ExecError,
    RegisterAiEntityWithKeyPayloadV1, SignalCommitmentPayloadV1, UpdateMemoryObjectPayloadV1,
};
use novai_state::{account_key, encode_account_v1, AccountStateV1, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

// =============================================================================
// HELPERS
// =============================================================================

fn derive_addr(pubkey: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"NOVAI_ADDRESS_V1");
    hasher.update(pubkey);
    *hasher.finalize().as_bytes()
}

fn mk_tx(from: [u8; 32], pubkey: [u8; 32], nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
    TxV1 {
        version: TxVersion::V1,
        from,
        pubkey,
        nonce,
        fee,
        payload,
        sig: [0u8; 64],
    }
}

fn fund_account(db: &mut MemKv, addr: [u8; 32], balance: u128) {
    let acct = AccountStateV1 { balance, nonce: 0 };
    db.apply_batch(&[WriteOp::Put(
        account_key(&addr),
        encode_account_v1(&acct).to_vec(),
    )])
    .unwrap();
}

/// Register an entity (type 10) and return `(entity_id, entity_addr)`.
fn register_entity_with_key(
    db: &mut MemKv,
    creator_pubkey: &[u8; 32],
    code_hash: [u8; 32],
    entity_pubkey: [u8; 32],
    initial_balance: u128,
    fee: u64,
) -> ([u8; 32], [u8; 32]) {
    let creator_addr = derive_addr(creator_pubkey);

    let payload =
        encode_register_ai_entity_with_key_payload_v1(&RegisterAiEntityWithKeyPayloadV1 {
            code_hash,
            pubkey: entity_pubkey,
            autonomy_mode: AutonomyMode::Gated,
            capabilities: Capabilities::gated(),
            initial_balance,
        })
        .to_vec();

    let tx = mk_tx(creator_addr, *creator_pubkey, 0, fee, payload);
    let entity_id = apply_register_ai_entity_with_key_tx(db, &tx, 100).unwrap();
    let entity_addr = derive_addr(&entity_pubkey);

    (entity_id, entity_addr)
}

// =============================================================================
// HAPPY-PATH REGRESSION TESTS
// =============================================================================

#[test]
fn signal_publish_lands_via_dispatcher() {
    let mut db = MemKv::new();
    let creator_pubkey = [0x11u8; 32];
    fund_account(&mut db, derive_addr(&creator_pubkey), 1_000_000);

    let entity_pubkey = [0x22u8; 32];
    let (entity_id, entity_addr) = register_entity_with_key(
        &mut db,
        &creator_pubkey,
        [0x33u8; 32],
        entity_pubkey,
        100_000,
        5_000,
    );

    let signal_payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8; 32],
        signal_type: AiSignalType::Anomaly,
        issuer_entity_id: entity_id,
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
        oracle_anchor: None,
    });

    let signal_tx = mk_tx(entity_addr, entity_pubkey, 0, 1_000, signal_payload);
    dispatch_tx(&mut db, &signal_tx, 200).expect("signal dispatch should succeed");

    let signals = get_signals_by_issuer(&db, &entity_id, 0, 1_000).unwrap();
    assert_eq!(
        signals.len(),
        1,
        "signal must be queryable by canonical entity.id"
    );
    assert_eq!(signals[0].issuer, entity_id);
    assert_eq!(signals[0].commitment_hash, [0xAAu8; 32]);
    assert_eq!(signals[0].signal_type, AiSignalType::Anomaly);

    let entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert_eq!(entity.nonce, 1);
    assert_eq!(entity.economic_balance, 100_000 - 1_000);
    assert_eq!(entity.last_active_at, 200);
}

#[test]
fn memory_create_lands_via_dispatcher() {
    let mut db = MemKv::new();
    let creator_pubkey = [0x11u8; 32];
    fund_account(&mut db, derive_addr(&creator_pubkey), 1_000_000);

    let entity_pubkey = [0x22u8; 32];
    let (entity_id, entity_addr) = register_entity_with_key(
        &mut db,
        &creator_pubkey,
        [0x33u8; 32],
        entity_pubkey,
        100_000,
        5_000,
    );

    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::ChainSummary,
        data: b"summary v1".to_vec(),
    });
    let tx = mk_tx(entity_addr, entity_pubkey, 0, 500, payload);
    dispatch_tx(&mut db, &tx, 201).expect("memory create dispatch should succeed");

    let objects = get_memory_objects_by_entity(&db, &entity_id).unwrap();
    assert_eq!(objects.len(), 1, "object must be queryable by entity.id");
    assert_eq!(objects[0].owner_entity, entity_id);
    assert_eq!(objects[0].data, b"summary v1");
    assert_eq!(objects[0].object_type, MemoryObjectType::ChainSummary);
}

#[test]
fn memory_update_round_trip_via_dispatcher() {
    let mut db = MemKv::new();
    let creator_pubkey = [0x11u8; 32];
    fund_account(&mut db, derive_addr(&creator_pubkey), 1_000_000);

    let entity_pubkey = [0x22u8; 32];
    let (entity_id, entity_addr) = register_entity_with_key(
        &mut db,
        &creator_pubkey,
        [0x33u8; 32],
        entity_pubkey,
        100_000,
        5_000,
    );

    let create_tx = mk_tx(
        entity_addr,
        entity_pubkey,
        0,
        500,
        encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
            object_type: MemoryObjectType::ChainSummary,
            data: b"v1".to_vec(),
        }),
    );
    dispatch_tx(&mut db, &create_tx, 100).unwrap();
    let object_id = get_memory_objects_by_entity(&db, &entity_id).unwrap()[0].object_id;

    let update_tx = mk_tx(
        entity_addr,
        entity_pubkey,
        1,
        500,
        encode_update_memory_object_payload_v1(&UpdateMemoryObjectPayloadV1 {
            object_id,
            new_data: b"v2 updated".to_vec(),
        }),
    );
    dispatch_tx(&mut db, &update_tx, 101).expect("memory update dispatch should succeed");

    let objects = get_memory_objects_by_entity(&db, &entity_id).unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].data, b"v2 updated");
    assert_eq!(objects[0].updated_at, 101);
}

#[test]
fn memory_delete_round_trip_via_dispatcher() {
    let mut db = MemKv::new();
    let creator_pubkey = [0x11u8; 32];
    fund_account(&mut db, derive_addr(&creator_pubkey), 1_000_000);

    let entity_pubkey = [0x22u8; 32];
    let (entity_id, entity_addr) = register_entity_with_key(
        &mut db,
        &creator_pubkey,
        [0x33u8; 32],
        entity_pubkey,
        100_000,
        5_000,
    );

    let create_tx = mk_tx(
        entity_addr,
        entity_pubkey,
        0,
        500,
        encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
            object_type: MemoryObjectType::ChainSummary,
            data: b"to delete".to_vec(),
        }),
    );
    dispatch_tx(&mut db, &create_tx, 100).unwrap();
    let object_id = get_memory_objects_by_entity(&db, &entity_id).unwrap()[0].object_id;

    let delete_tx = mk_tx(
        entity_addr,
        entity_pubkey,
        1,
        500,
        encode_delete_memory_object_payload_v1(&DeleteMemoryObjectPayloadV1 { object_id }).to_vec(),
    );
    dispatch_tx(&mut db, &delete_tx, 101).expect("memory delete dispatch should succeed");

    let objects = get_memory_objects_by_entity(&db, &entity_id).unwrap();
    assert!(objects.is_empty(), "object must be removed");
}

// =============================================================================
// REJECTION TESTS
// =============================================================================

#[test]
fn signal_from_non_entity_address_returns_issuer_not_found() {
    let mut db = MemKv::new();
    let stranger_pubkey = [0x99u8; 32];

    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8; 32],
        signal_type: AiSignalType::Anomaly,
        issuer_entity_id: [0xDEu8; 32],
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
        oracle_anchor: None,
    });

    let tx = mk_tx(
        derive_addr(&stranger_pubkey),
        stranger_pubkey,
        0,
        1_000,
        payload,
    );
    let result = dispatch_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::IssuerNotFound)),
        "got {result:?}"
    );
}

#[test]
fn signal_with_mismatched_payload_issuer_returns_mismatch() {
    let mut db = MemKv::new();
    let creator_pubkey = [0x11u8; 32];
    fund_account(&mut db, derive_addr(&creator_pubkey), 1_000_000);

    let entity_pubkey = [0x22u8; 32];
    let (_entity_id, entity_addr) = register_entity_with_key(
        &mut db,
        &creator_pubkey,
        [0x33u8; 32],
        entity_pubkey,
        100_000,
        5_000,
    );

    let payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8; 32],
        signal_type: AiSignalType::Anomaly,
        issuer_entity_id: [0xCCu8; 32], // wrong issuer claimed
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
        oracle_anchor: None,
    });

    let tx = mk_tx(entity_addr, entity_pubkey, 0, 1_000, payload);
    let result = dispatch_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::IssuerMismatch)),
        "got {result:?}"
    );
}

#[test]
fn memory_from_non_entity_address_returns_issuer_not_found() {
    let mut db = MemKv::new();
    let stranger_pubkey = [0x99u8; 32];

    let payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::ChainSummary,
        data: b"unauthorized".to_vec(),
    });

    let tx = mk_tx(
        derive_addr(&stranger_pubkey),
        stranger_pubkey,
        0,
        500,
        payload,
    );
    let result = dispatch_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::IssuerNotFound)),
        "got {result:?}"
    );
}

// =============================================================================
// β4-A SITE 3/4/5 UNIT TESTS
//
// One thin test per site. Each exercises:
//   1. A failure between the nonce check and the nonce advance.
//   2. entity.nonce staying frozen across that failure.
//   3. A recovery tx at tx.nonce above entity.nonce that pre-fix returns
//      NonceMismatch and post-fix returns Ok with entity.nonce pinned to
//      tx.nonce + 1.
// =============================================================================

#[test]
fn create_memory_failure_does_not_wedge_entity() {
    use novai_ai_entities::MAX_MEMORY_OBJECTS_PER_ENTITY;
    use novai_execution::apply_create_memory_object_tx;
    use novai_state::{ai_memory_count_key, encode_memory_count};

    let mut db = MemKv::new();
    let creator_pubkey = [0x71u8; 32];
    fund_account(&mut db, derive_addr(&creator_pubkey), 1_000_000);

    let entity_pubkey = [0x72u8; 32];
    let (entity_id, entity_addr) = register_entity_with_key(
        &mut db,
        &creator_pubkey,
        [0x73u8; 32],
        entity_pubkey,
        100_000,
        5_000,
    );

    // Pre-seed memory count to MAX. The next create trips
    // MemoryObjectCountExceeded at lib.rs:10563 AFTER the nonce check at
    // lib.rs:10545 passes.
    db.apply_batch(&[WriteOp::Put(
        ai_memory_count_key(&entity_id),
        encode_memory_count(MAX_MEMORY_OBJECTS_PER_ENTITY).to_vec(),
    )])
    .unwrap();

    let bad_payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::ChainSummary,
        data: b"first".to_vec(),
    });
    let bad_tx = mk_tx(entity_addr, entity_pubkey, 0, 500, bad_payload);
    let r0 = apply_create_memory_object_tx(&mut db, &bad_tx, 200);
    assert!(
        matches!(r0, Err(ExecError::MemoryObjectCountExceeded { .. })),
        "first create must fail MemoryObjectCountExceeded, got {r0:?}"
    );
    assert_eq!(read_ai_entity(&db, &entity_id).unwrap().unwrap().nonce, 0);

    // Lower the count so the recovery create at tx.nonce = 1 can succeed.
    db.apply_batch(&[WriteOp::Put(
        ai_memory_count_key(&entity_id),
        encode_memory_count(0).to_vec(),
    )])
    .unwrap();

    let good_payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::ChainSummary,
        data: b"recovery".to_vec(),
    });
    let good_tx = mk_tx(entity_addr, entity_pubkey, 1, 500, good_payload);
    apply_create_memory_object_tx(&mut db, &good_tx, 201)
        .expect("β4-A must admit create at tx.nonce above entity.nonce");

    assert_eq!(read_ai_entity(&db, &entity_id).unwrap().unwrap().nonce, 2);
}

#[test]
fn update_memory_failure_does_not_wedge_entity() {
    use novai_execution::{apply_create_memory_object_tx, apply_update_memory_object_tx};

    let mut db = MemKv::new();
    let creator_pubkey = [0x81u8; 32];
    fund_account(&mut db, derive_addr(&creator_pubkey), 1_000_000);

    let entity_pubkey = [0x82u8; 32];
    let (entity_id, entity_addr) = register_entity_with_key(
        &mut db,
        &creator_pubkey,
        [0x83u8; 32],
        entity_pubkey,
        100_000,
        5_000,
    );

    // Seed a real memory object via the create handler. This advances
    // entity.nonce from 0 to 1 so the rest of the test runs from a known
    // post-create baseline.
    let create_payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::ChainSummary,
        data: b"v1".to_vec(),
    });
    let create_tx = mk_tx(entity_addr, entity_pubkey, 0, 500, create_payload);
    let object_id = apply_create_memory_object_tx(&mut db, &create_tx, 200).unwrap();
    assert_eq!(read_ai_entity(&db, &entity_id).unwrap().unwrap().nonce, 1);

    // Bad update at tx.nonce = 1 targets a nonexistent object_id. The
    // handler passes the nonce check at lib.rs:10753 (1 == 1), then loads
    // None from `read_memory_object` and returns MemoryObjectNotFound at
    // lib.rs:10771 BEFORE the advance at lib.rs:10844. entity.nonce stays 1.
    let bad_update = encode_update_memory_object_payload_v1(&UpdateMemoryObjectPayloadV1 {
        object_id: [0xFFu8; 32],
        new_data: b"never".to_vec(),
    });
    let bad_tx = mk_tx(entity_addr, entity_pubkey, 1, 500, bad_update);
    let r0 = apply_update_memory_object_tx(&mut db, &bad_tx, 201);
    assert!(
        matches!(r0, Err(ExecError::MemoryObjectNotFound)),
        "bad update must fail MemoryObjectNotFound, got {r0:?}"
    );
    assert_eq!(read_ai_entity(&db, &entity_id).unwrap().unwrap().nonce, 1);

    // Recovery update at tx.nonce = 2 targets the real object.
    let good_update = encode_update_memory_object_payload_v1(&UpdateMemoryObjectPayloadV1 {
        object_id,
        new_data: b"v2".to_vec(),
    });
    let good_tx = mk_tx(entity_addr, entity_pubkey, 2, 500, good_update);
    apply_update_memory_object_tx(&mut db, &good_tx, 202)
        .expect("β4-A must admit update at tx.nonce above entity.nonce");

    assert_eq!(read_ai_entity(&db, &entity_id).unwrap().unwrap().nonce, 3);
}

#[test]
fn delete_memory_failure_does_not_wedge_entity() {
    use novai_execution::{apply_create_memory_object_tx, apply_delete_memory_object_tx};

    let mut db = MemKv::new();
    let creator_pubkey = [0x91u8; 32];
    fund_account(&mut db, derive_addr(&creator_pubkey), 1_000_000);

    let entity_pubkey = [0x92u8; 32];
    let (entity_id, entity_addr) = register_entity_with_key(
        &mut db,
        &creator_pubkey,
        [0x93u8; 32],
        entity_pubkey,
        100_000,
        5_000,
    );

    // Seed a real memory object via the create handler. entity.nonce -> 1.
    let create_payload = encode_create_memory_object_payload_v1(&CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::ChainSummary,
        data: b"keep".to_vec(),
    });
    let create_tx = mk_tx(entity_addr, entity_pubkey, 0, 500, create_payload);
    let object_id = apply_create_memory_object_tx(&mut db, &create_tx, 200).unwrap();
    assert_eq!(read_ai_entity(&db, &entity_id).unwrap().unwrap().nonce, 1);

    // Bad delete at tx.nonce = 1 targets a nonexistent object_id. Passes
    // the nonce check at lib.rs:10922, then fails MemoryObjectNotFound at
    // lib.rs:10940 BEFORE the advance at lib.rs:10955. entity.nonce stays 1.
    let bad_delete = encode_delete_memory_object_payload_v1(&DeleteMemoryObjectPayloadV1 {
        object_id: [0xFFu8; 32],
    })
    .to_vec();
    let bad_tx = mk_tx(entity_addr, entity_pubkey, 1, 500, bad_delete);
    let r0 = apply_delete_memory_object_tx(&mut db, &bad_tx, 201);
    assert!(
        matches!(r0, Err(ExecError::MemoryObjectNotFound)),
        "bad delete must fail MemoryObjectNotFound, got {r0:?}"
    );
    assert_eq!(read_ai_entity(&db, &entity_id).unwrap().unwrap().nonce, 1);

    // Recovery delete at tx.nonce = 2 targets the real object.
    let good_delete =
        encode_delete_memory_object_payload_v1(&DeleteMemoryObjectPayloadV1 { object_id }).to_vec();
    let good_tx = mk_tx(entity_addr, entity_pubkey, 2, 500, good_delete);
    apply_delete_memory_object_tx(&mut db, &good_tx, 202)
        .expect("β4-A must admit delete at tx.nonce above entity.nonce");

    assert_eq!(read_ai_entity(&db, &entity_id).unwrap().unwrap().nonce, 3);
}
