#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

//! Week 34 Phase 2: validation tests for `validate_entity_upgrade`.
//!
//! Each test exercises one rule of the validator in isolation and asserts that
//! a rejection fires with the expected `ExecError` variant. The happy paths
//! confirm that a first upgrade (no summary), an exactly-elapsed cooldown, and
//! a set-or-zero reason hash all pass.

use novai_ai_entities::{AutonomyMode, Capabilities};
use novai_execution::{
    apply_register_ai_entity_tx, encode_register_ai_entity_payload_v1, encode_upgrade_summary_v1,
    entity_upgrade_summary_key, read_ai_entity, validate_entity_upgrade, write_ai_entity_op,
    EntityUpgradePayloadV1, ExecError, RegisterAiEntityPayloadV1, UpgradeSummary,
    MIN_UPGRADE_INTERVAL_BLOCKS,
};
use novai_state::{account_key, encode_account_v1, AccountStateV1, KvBatch, MemKv, WriteOp};
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

/// Fund `creator` and register an entity with the given code hash. Returns the
/// entity id. The creator account is left with `funding - initial - fee`.
fn register(
    db: &mut MemKv,
    creator: [u8; 32],
    code_hash: [u8; 32],
    funding: u128,
    initial: u128,
    fee: u64,
    height: u64,
) -> [u8; 32] {
    seed_account(db, &creator, funding, 0);
    let payload = encode_register_ai_entity_payload_v1(&RegisterAiEntityPayloadV1 {
        code_hash,
        autonomy_mode: AutonomyMode::Gated,
        capabilities: Capabilities::gated(),
        initial_balance: initial,
    })
    .to_vec();
    let tx = create_test_tx(creator, 0, fee, payload);
    apply_register_ai_entity_tx(db, &tx, height).unwrap()
}

fn deactivate(db: &mut MemKv, entity_id: &[u8; 32]) {
    let mut entity = read_ai_entity(db, entity_id).unwrap().unwrap();
    entity.is_active = false;
    db.apply_batch(&[write_ai_entity_op(&entity)]).unwrap();
}

fn seed_summary(db: &mut MemKv, entity_id: &[u8; 32], count: u32, last_height: u64) {
    let summary = UpgradeSummary {
        upgrade_count: count,
        last_upgrade_height: last_height,
    };
    let op = WriteOp::Put(
        entity_upgrade_summary_key(entity_id),
        encode_upgrade_summary_v1(&summary).to_vec(),
    );
    db.apply_batch(&[op]).unwrap();
}

fn payload(entity_id: [u8; 32], new_code_hash: [u8; 32]) -> EntityUpgradePayloadV1 {
    EntityUpgradePayloadV1 {
        entity_id,
        new_code_hash,
        reason_hash: [0u8; 32],
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[test]
fn validate_happy_path_first_upgrade() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 1_000_000, 0, 5_000, 100);

    let result = validate_entity_upgrade(&db, &payload(id, [0x43u8; 32]), &creator, 5_000, 50_000);
    assert!(result.is_ok(), "first upgrade with no summary must pass");
}

#[test]
fn validate_rejects_unknown_entity() {
    let db = MemKv::new();
    let creator = [0x01u8; 32];
    let unknown = [0x99u8; 32];

    let result =
        validate_entity_upgrade(&db, &payload(unknown, [0x43u8; 32]), &creator, 5_000, 100);
    assert!(matches!(
        result,
        Err(ExecError::EntityUpgradeEntityNotFound)
    ));
}

#[test]
fn validate_rejects_non_creator() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let attacker = [0x02u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 1_000_000, 0, 5_000, 100);
    seed_account(&mut db, &attacker, 1_000_000, 0);

    let result = validate_entity_upgrade(&db, &payload(id, [0x43u8; 32]), &attacker, 5_000, 50_000);
    assert!(matches!(result, Err(ExecError::EntityUpgradeNotCreator)));
}

#[test]
fn validate_rejects_inactive_entity() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 1_000_000, 0, 5_000, 100);
    deactivate(&mut db, &id);

    let result = validate_entity_upgrade(&db, &payload(id, [0x43u8; 32]), &creator, 5_000, 50_000);
    assert!(matches!(
        result,
        Err(ExecError::EntityUpgradeEntityNotActive)
    ));
}

#[test]
fn validate_rejects_same_code_hash() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let code_hash = [0x42u8; 32];
    let id = register(&mut db, creator, code_hash, 1_000_000, 0, 5_000, 100);

    let result = validate_entity_upgrade(&db, &payload(id, code_hash), &creator, 5_000, 50_000);
    assert!(matches!(result, Err(ExecError::EntityUpgradeSameCodeHash)));
}

#[test]
fn validate_rejects_cooldown_active() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 1_000_000, 0, 5_000, 100);
    // Last upgrade at height 10_000; current well within the 1000-block window.
    seed_summary(&mut db, &id, 1, 10_000);

    let result = validate_entity_upgrade(&db, &payload(id, [0x43u8; 32]), &creator, 5_000, 10_500);
    assert!(matches!(
        result,
        Err(ExecError::EntityUpgradeCooldownActive {
            last_upgrade_height: 10_000,
            current_height: 10_500,
            next_allowed_height: 11_000,
        })
    ));
}

#[test]
fn validate_cooldown_boundary_exact_allowed() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 1_000_000, 0, 5_000, 100);
    seed_summary(&mut db, &id, 3, 10_000);

    // current == last + MIN_UPGRADE_INTERVAL_BLOCKS is the first allowed height.
    let at = 10_000 + MIN_UPGRADE_INTERVAL_BLOCKS;
    let result = validate_entity_upgrade(&db, &payload(id, [0x43u8; 32]), &creator, 5_000, at);
    assert!(result.is_ok(), "exact cooldown boundary must be allowed");
}

#[test]
fn validate_cooldown_one_before_boundary_rejected() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 1_000_000, 0, 5_000, 100);
    seed_summary(&mut db, &id, 3, 10_000);

    let at = 10_000 + MIN_UPGRADE_INTERVAL_BLOCKS - 1;
    let result = validate_entity_upgrade(&db, &payload(id, [0x43u8; 32]), &creator, 5_000, at);
    assert!(matches!(
        result,
        Err(ExecError::EntityUpgradeCooldownActive {
            next_allowed_height,
            ..
        }) if next_allowed_height == 10_000 + MIN_UPGRADE_INTERVAL_BLOCKS
    ));
}

#[test]
fn validate_rejects_insufficient_fee() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    // Fund 6000, register costs initial 0 + fee 5000 => 1000 left.
    let id = register(&mut db, creator, [0x42u8; 32], 6_000, 0, 5_000, 100);

    // Upgrade fee 5000 exceeds the remaining 1000.
    let result = validate_entity_upgrade(&db, &payload(id, [0x43u8; 32]), &creator, 5_000, 50_000);
    assert!(matches!(
        result,
        Err(ExecError::InsufficientFunds {
            balance: 1_000,
            needed: 5_000,
        })
    ));
}

#[test]
fn validate_first_upgrade_allowed_at_high_height() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 1_000_000, 0, 5_000, 100);

    // No summary row at all => first upgrade, allowed regardless of height.
    let result =
        validate_entity_upgrade(&db, &payload(id, [0x43u8; 32]), &creator, 5_000, 9_000_000);
    assert!(result.is_ok());
}

#[test]
fn validate_reason_hash_zero_and_set_both_pass() {
    let mut db = MemKv::new();
    let creator = [0x01u8; 32];
    let id = register(&mut db, creator, [0x42u8; 32], 1_000_000, 0, 5_000, 100);

    let zero = EntityUpgradePayloadV1 {
        entity_id: id,
        new_code_hash: [0x43u8; 32],
        reason_hash: [0u8; 32],
    };
    let set = EntityUpgradePayloadV1 {
        entity_id: id,
        new_code_hash: [0x43u8; 32],
        reason_hash: [0xAAu8; 32],
    };
    assert!(validate_entity_upgrade(&db, &zero, &creator, 5_000, 50_000).is_ok());
    assert!(validate_entity_upgrade(&db, &set, &creator, 5_000, 50_000).is_ok());
}

#[test]
fn validate_creator_binding_is_per_entity() {
    let mut db = MemKv::new();
    let creator_a = [0x01u8; 32];
    let creator_b = [0x02u8; 32];
    let _id_a = register(&mut db, creator_a, [0x42u8; 32], 1_000_000, 0, 5_000, 100);
    let id_b = register(&mut db, creator_b, [0x52u8; 32], 1_000_000, 0, 5_000, 100);

    // creator_a is a valid creator, but not of entity B.
    let result =
        validate_entity_upgrade(&db, &payload(id_b, [0x53u8; 32]), &creator_a, 5_000, 50_000);
    assert!(matches!(result, Err(ExecError::EntityUpgradeNotCreator)));
}
