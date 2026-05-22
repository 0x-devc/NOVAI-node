#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

//! Week 34 Phase 3: execution tests for `apply_entity_upgrade_tx`.
//!
//! These tests prove the upgrade mutates only `code_hash` (and `last_active_at`)
//! while preserving the `entity_id` and every other field, writes the history
//! and summary aux rows, charges the fee correctly, and rolls nothing back into
//! state on a rejection.

use novai_ai_entities::{AutonomyMode, Capabilities};
use novai_execution::{
    apply_entity_upgrade_tx, apply_register_ai_entity_tx, decode_upgrade_record_v1, dispatch_tx,
    encode_entity_upgrade_payload_v1, encode_register_ai_entity_payload_v1,
    entity_upgrade_by_entity_key, read_ai_entity, read_upgrade_summary, write_ai_entity_op,
    write_ai_kill_switch_op, EntityUpgradePayloadV1, ExecError, RegisterAiEntityPayloadV1,
    UpgradeRecord, MIN_FEE_ENTITY_UPGRADE,
};
use novai_state::{
    account_key, decode_account_v1, decode_fee_pool_v1, encode_account_v1, AccountStateV1, KvBatch,
    MemKv, WriteOp, KEY_FEE_POOL,
};
use novai_types::{TxV1, TxVersion};

// ============================================================================
// HELPERS
// ============================================================================

fn create_test_tx(from: [u8; 32], nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
    TxV1 {
        version: TxVersion::V1,
        from,
        pubkey: from,
        nonce,
        fee,
        payload,
        sig: [0u8; 64],
    }
}

fn seed_account(db: &mut MemKv, addr: &[u8; 32], balance: u128, nonce: u64) {
    let acct = AccountStateV1 { balance, nonce };
    let op = WriteOp::Put(account_key(addr), encode_account_v1(&acct).to_vec());
    db.apply_batch(&[op]).unwrap();
}

fn read_account(db: &MemKv, addr: &[u8; 32]) -> AccountStateV1 {
    use novai_state::Kv;
    db.get(&account_key(addr)).unwrap().map_or(
        AccountStateV1 {
            balance: 0,
            nonce: 0,
        },
        |b| decode_account_v1(&b).unwrap(),
    )
}

fn read_fee_pool(db: &MemKv) -> u128 {
    use novai_state::Kv;
    db.get(KEY_FEE_POOL)
        .unwrap()
        .map_or(0, |b| decode_fee_pool_v1(&b).unwrap().balance)
}

fn read_record(db: &MemKv, id: &[u8; 32], height: u64) -> Option<UpgradeRecord> {
    use novai_state::Kv;
    db.get(&entity_upgrade_by_entity_key(id, height))
        .unwrap()
        .map(|b| decode_upgrade_record_v1(&b).unwrap())
}

/// Fund `creator` with 1_000_000 and register an entity with `initial` balance.
/// Register fee is the standard 5_000; creator nonce is 1 afterwards.
fn register(db: &mut MemKv, creator: [u8; 32], code_hash: [u8; 32], initial: u128) -> [u8; 32] {
    seed_account(db, &creator, 1_000_000, 0);
    let payload = encode_register_ai_entity_payload_v1(&RegisterAiEntityPayloadV1 {
        code_hash,
        autonomy_mode: AutonomyMode::Gated,
        capabilities: Capabilities::gated(),
        initial_balance: initial,
    })
    .to_vec();
    let tx = create_test_tx(creator, 0, 5_000, payload);
    apply_register_ai_entity_tx(db, &tx, 100).unwrap()
}

fn upgrade_tx(
    creator: [u8; 32],
    nonce: u64,
    entity_id: [u8; 32],
    new_hash: [u8; 32],
    reason: [u8; 32],
) -> TxV1 {
    let payload = encode_entity_upgrade_payload_v1(&EntityUpgradePayloadV1 {
        entity_id,
        new_code_hash: new_hash,
        reason_hash: reason,
    })
    .to_vec();
    create_test_tx(creator, nonce, MIN_FEE_ENTITY_UPGRADE, payload)
}

// ============================================================================
// TESTS
// ============================================================================

#[test]
fn upgrade_updates_code_hash_and_keeps_id() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 0);

    let returned = apply_entity_upgrade_tx(
        &mut db,
        &upgrade_tx(creator, 1, id, [0x43u8; 32], [0u8; 32]),
        50_000,
    )
    .unwrap();

    assert_eq!(returned, id, "upgrade returns the unchanged entity id");
    let entity = read_ai_entity(&db, &id).unwrap().unwrap();
    assert_eq!(entity.id, id, "entity id is preserved");
    assert_eq!(entity.code_hash, [0x43u8; 32], "code_hash is swapped");
    assert_eq!(entity.creator, creator, "creator is preserved");
    assert_eq!(entity.last_active_at, 50_000, "last_active_at bumped");
}

#[test]
fn upgrade_writes_record_and_summary() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 0);

    apply_entity_upgrade_tx(
        &mut db,
        &upgrade_tx(creator, 1, id, [0x43u8; 32], [0xCCu8; 32]),
        50_000,
    )
    .unwrap();

    let summary = read_upgrade_summary(&db, &id).unwrap().unwrap();
    assert_eq!(summary.upgrade_count, 1);
    assert_eq!(summary.last_upgrade_height, 50_000);

    let record = read_record(&db, &id, 50_000).expect("history row at the upgrade height");
    assert_eq!(record.old_code_hash, [0x42u8; 32]);
    assert_eq!(record.new_code_hash, [0x43u8; 32]);
    assert_eq!(record.upgrade_height, 50_000);
    assert_eq!(record.upgrade_count, 1);
    assert_eq!(record.reason_hash, [0xCCu8; 32]);
}

#[test]
fn upgrade_preserves_reputation_and_counts() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 0);

    // Seed reputation-related fields, then upgrade.
    let mut entity = read_ai_entity(&db, &id).unwrap().unwrap();
    entity.reputation_score = 77;
    entity.reputation_events_count = 5;
    entity.total_transactions = 9;
    db.apply_batch(&[write_ai_entity_op(&entity)]).unwrap();

    apply_entity_upgrade_tx(
        &mut db,
        &upgrade_tx(creator, 1, id, [0x43u8; 32], [0u8; 32]),
        50_000,
    )
    .unwrap();

    let after = read_ai_entity(&db, &id).unwrap().unwrap();
    assert_eq!(after.reputation_score, 77);
    assert_eq!(after.reputation_events_count, 5);
    assert_eq!(after.total_transactions, 9);
}

#[test]
fn upgrade_preserves_economic_balance() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 12_345);

    apply_entity_upgrade_tx(
        &mut db,
        &upgrade_tx(creator, 1, id, [0x43u8; 32], [0u8; 32]),
        50_000,
    )
    .unwrap();

    let after = read_ai_entity(&db, &id).unwrap().unwrap();
    assert_eq!(
        after.economic_balance, 12_345,
        "entity balance is untouched by upgrade"
    );
}

#[test]
fn upgrade_preserves_stake() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 0);

    let mut entity = read_ai_entity(&db, &id).unwrap().unwrap();
    entity.stake_balance = 999;
    entity.stake_locked_until = 8_888;
    db.apply_batch(&[write_ai_entity_op(&entity)]).unwrap();

    apply_entity_upgrade_tx(
        &mut db,
        &upgrade_tx(creator, 1, id, [0x43u8; 32], [0u8; 32]),
        50_000,
    )
    .unwrap();

    let after = read_ai_entity(&db, &id).unwrap().unwrap();
    assert_eq!(after.stake_balance, 999);
    assert_eq!(after.stake_locked_until, 8_888);
}

#[test]
fn upgrade_preserves_capabilities_autonomy_pubkey_and_roots() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 0);

    let mut entity = read_ai_entity(&db, &id).unwrap().unwrap();
    entity.pubkey = [0xABu8; 32];
    entity.memory_root = [0x10u8; 32];
    entity.params_root = [0x20u8; 32];
    let caps_before = entity.capabilities;
    let autonomy_before = entity.autonomy_mode;
    db.apply_batch(&[write_ai_entity_op(&entity)]).unwrap();

    apply_entity_upgrade_tx(
        &mut db,
        &upgrade_tx(creator, 1, id, [0x43u8; 32], [0u8; 32]),
        50_000,
    )
    .unwrap();

    let after = read_ai_entity(&db, &id).unwrap().unwrap();
    assert_eq!(after.capabilities, caps_before);
    assert_eq!(after.autonomy_mode, autonomy_before);
    assert_eq!(after.pubkey, [0xABu8; 32]);
    assert_eq!(after.memory_root, [0x10u8; 32]);
    assert_eq!(after.params_root, [0x20u8; 32]);
}

#[test]
fn upgrade_debits_creator_and_credits_fee_pool() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 0);

    // After register: balance 1_000_000 - 5_000 = 995_000, nonce 1, pool 5_000.
    let before = read_account(&db, &creator);
    assert_eq!(before.balance, 995_000);
    assert_eq!(before.nonce, 1);

    apply_entity_upgrade_tx(
        &mut db,
        &upgrade_tx(creator, 1, id, [0x43u8; 32], [0u8; 32]),
        50_000,
    )
    .unwrap();

    let after = read_account(&db, &creator);
    assert_eq!(after.balance, 995_000 - MIN_FEE_ENTITY_UPGRADE as u128);
    assert_eq!(after.nonce, 2);
    assert_eq!(read_fee_pool(&db), 5_000 + MIN_FEE_ENTITY_UPGRADE as u128);
}

#[test]
fn second_upgrade_after_cooldown_increments_count() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 0);

    apply_entity_upgrade_tx(
        &mut db,
        &upgrade_tx(creator, 1, id, [0x43u8; 32], [0u8; 32]),
        50_000,
    )
    .unwrap();
    // Exactly at the cooldown boundary (50_000 + 1000).
    apply_entity_upgrade_tx(
        &mut db,
        &upgrade_tx(creator, 2, id, [0x44u8; 32], [0u8; 32]),
        51_000,
    )
    .unwrap();

    let summary = read_upgrade_summary(&db, &id).unwrap().unwrap();
    assert_eq!(summary.upgrade_count, 2);
    assert_eq!(summary.last_upgrade_height, 51_000);

    let first = read_record(&db, &id, 50_000).unwrap();
    let second = read_record(&db, &id, 51_000).unwrap();
    assert_eq!(first.upgrade_count, 1);
    assert_eq!(first.new_code_hash, [0x43u8; 32]);
    assert_eq!(second.upgrade_count, 2);
    assert_eq!(second.old_code_hash, [0x43u8; 32]);
    assert_eq!(second.new_code_hash, [0x44u8; 32]);

    let entity = read_ai_entity(&db, &id).unwrap().unwrap();
    assert_eq!(entity.code_hash, [0x44u8; 32]);
}

#[test]
fn upgrade_via_dispatch_tx_succeeds() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 0);

    dispatch_tx(
        &mut db,
        &upgrade_tx(creator, 1, id, [0x43u8; 32], [0u8; 32]),
        50_000,
    )
    .unwrap();

    let entity = read_ai_entity(&db, &id).unwrap().unwrap();
    assert_eq!(entity.code_hash, [0x43u8; 32]);
}

#[test]
fn upgrade_via_dispatch_below_min_fee_rejected() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 0);

    let payload = encode_entity_upgrade_payload_v1(&EntityUpgradePayloadV1 {
        entity_id: id,
        new_code_hash: [0x43u8; 32],
        reason_hash: [0u8; 32],
    })
    .to_vec();
    let tx = create_test_tx(creator, 1, 100, payload); // 100 < MIN_FEE_ENTITY_UPGRADE
    let result = dispatch_tx(&mut db, &tx, 50_000);
    assert!(matches!(
        result,
        Err(ExecError::FeeBelowMinimum {
            minimum: 5_000,
            provided: 100
        })
    ));
}

#[test]
fn upgrade_blocked_by_kill_switch() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 0);
    db.apply_batch(&[write_ai_kill_switch_op(true)]).unwrap();

    let result = apply_entity_upgrade_tx(
        &mut db,
        &upgrade_tx(creator, 1, id, [0x43u8; 32], [0u8; 32]),
        50_000,
    );
    assert!(matches!(result, Err(ExecError::AiKillSwitchActive)));

    // No mutation: code_hash unchanged, no summary written.
    let entity = read_ai_entity(&db, &id).unwrap().unwrap();
    assert_eq!(entity.code_hash, [0x42u8; 32]);
    assert!(read_upgrade_summary(&db, &id).unwrap().is_none());
}

#[test]
fn upgrade_nonce_mismatch_rejected() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 0);

    // Creator nonce is 1 after register; submit with nonce 5.
    let result = apply_entity_upgrade_tx(
        &mut db,
        &upgrade_tx(creator, 5, id, [0x43u8; 32], [0u8; 32]),
        50_000,
    );
    assert!(matches!(
        result,
        Err(ExecError::NonceMismatch {
            expected: 1,
            got: 5
        })
    ));
}

#[test]
fn upgrade_same_code_hash_via_handler_no_mutation() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 0);
    let acct_before = read_account(&db, &creator);

    let result = apply_entity_upgrade_tx(
        &mut db,
        &upgrade_tx(creator, 1, id, [0x42u8; 32], [0u8; 32]),
        50_000,
    );
    assert!(matches!(result, Err(ExecError::EntityUpgradeSameCodeHash)));

    assert!(read_upgrade_summary(&db, &id).unwrap().is_none());
    assert!(read_record(&db, &id, 50_000).is_none());
    let acct_after = read_account(&db, &creator);
    assert_eq!(acct_after.balance, acct_before.balance, "no fee charged");
    assert_eq!(acct_after.nonce, acct_before.nonce, "no nonce bump");
}

#[test]
fn upgrade_cooldown_via_handler_no_mutation() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 0);

    apply_entity_upgrade_tx(
        &mut db,
        &upgrade_tx(creator, 1, id, [0x43u8; 32], [0u8; 32]),
        50_000,
    )
    .unwrap();
    let acct_after_first = read_account(&db, &creator);

    // Second upgrade only 10 blocks later: inside the 1000-block window.
    let result = apply_entity_upgrade_tx(
        &mut db,
        &upgrade_tx(creator, 2, id, [0x44u8; 32], [0u8; 32]),
        50_010,
    );
    assert!(matches!(
        result,
        Err(ExecError::EntityUpgradeCooldownActive { .. })
    ));

    // State stays at the first upgrade.
    let entity = read_ai_entity(&db, &id).unwrap().unwrap();
    assert_eq!(entity.code_hash, [0x43u8; 32]);
    let summary = read_upgrade_summary(&db, &id).unwrap().unwrap();
    assert_eq!(summary.upgrade_count, 1);
    assert_eq!(summary.last_upgrade_height, 50_000);
    assert!(read_record(&db, &id, 50_010).is_none());
    let acct_now = read_account(&db, &creator);
    assert_eq!(acct_now.balance, acct_after_first.balance);
    assert_eq!(acct_now.nonce, acct_after_first.nonce);
}
