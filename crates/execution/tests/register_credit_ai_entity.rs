#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

//! Integration tests for RegisterAiEntity and CreditAiEntity transaction types.
//!
//! Acceptance Criteria:
//! 1. Register creates entity with correct ID, fields, and initial balance
//! 2. Duplicate registration is rejected
//! 3. Autonomous mode is rejected
//! 4. Credit increases entity balance
//! 5. Credit to nonexistent/inactive entity is rejected
//! 6. Full lifecycle: register → credit → signal pipeline works end-to-end

use novai_ai_entities::{AiEntity, AiSignalType, AutonomyMode, Capabilities};
use novai_execution::{
    apply_credit_ai_entity_tx, apply_register_ai_entity_tx, apply_signal_commitment_tx,
    dispatch_tx, encode_credit_ai_entity_payload_v1, encode_register_ai_entity_payload_v1,
    encode_signal_commitment_payload_v1, read_ai_entity, write_ai_entity_op,
    CreditAiEntityPayloadV1, ExecError, RegisterAiEntityPayloadV1, SignalCommitmentPayloadV1,
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
    let key = account_key(addr);
    db.get(&key).unwrap().map_or(
        AccountStateV1 {
            balance: 0,
            nonce: 0,
        },
        |bytes| decode_account_v1(&bytes).unwrap(),
    )
}

fn read_fee_pool(db: &MemKv) -> u128 {
    use novai_state::Kv;
    db.get(KEY_FEE_POOL)
        .unwrap()
        .map_or(0, |bytes| decode_fee_pool_v1(&bytes).unwrap().balance)
}

// ============================================================================
// TEST 1: register_entity_success
// ============================================================================

#[test]
fn register_entity_success() {
    let mut db = MemKv::new();

    let creator = [0x01u8; 32];
    let initial_balance = 500u128;
    let fee = 10u64;
    let code_hash = [0x42u8; 32];

    // Fund creator account
    seed_account(&mut db, &creator, 1000, 0);

    let payload = encode_register_ai_entity_payload_v1(&RegisterAiEntityPayloadV1 {
        code_hash,
        autonomy_mode: AutonomyMode::Gated,
        capabilities: Capabilities::gated(),
        initial_balance,
    })
    .to_vec();

    let tx = create_test_tx(creator, 0, fee, payload);
    let entity_id = apply_register_ai_entity_tx(&mut db, &tx, 100).unwrap();

    // Verify entity was created
    let entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert_eq!(entity.code_hash, code_hash);
    assert_eq!(entity.creator, creator);
    assert_eq!(entity.autonomy_mode, AutonomyMode::Gated);
    assert_eq!(entity.economic_balance, initial_balance);
    assert_eq!(entity.registered_at, 100);
    assert!(entity.is_active);

    // Verify creator was debited
    let creator_acct = read_account(&db, &creator);
    assert_eq!(creator_acct.balance, 1000 - initial_balance - fee as u128);
    assert_eq!(creator_acct.nonce, 1);

    // Verify fee pool was credited
    assert_eq!(read_fee_pool(&db), fee as u128);
}

// ============================================================================
// TEST 2: register_entity_deterministic_id
// ============================================================================

#[test]
fn register_entity_deterministic_id() {
    let mut db = MemKv::new();

    let creator = [0x01u8; 32];
    let code_hash = [0x42u8; 32];

    seed_account(&mut db, &creator, 1000, 0);

    let payload = encode_register_ai_entity_payload_v1(&RegisterAiEntityPayloadV1 {
        code_hash,
        autonomy_mode: AutonomyMode::Advisory,
        capabilities: Capabilities::advisory(),
        initial_balance: 100,
    })
    .to_vec();

    let tx = create_test_tx(creator, 0, 10, payload);
    let entity_id = apply_register_ai_entity_tx(&mut db, &tx, 50).unwrap();

    // Verify ID matches compute_id
    let expected_id = AiEntity::compute_id(&code_hash, &creator);
    assert_eq!(entity_id, expected_id);
}

// ============================================================================
// TEST 3: register_entity_duplicate_rejected
// ============================================================================

#[test]
fn register_entity_duplicate_rejected() {
    let mut db = MemKv::new();

    let creator = [0x01u8; 32];
    let code_hash = [0x42u8; 32];

    seed_account(&mut db, &creator, 10_000, 0);

    let payload = encode_register_ai_entity_payload_v1(&RegisterAiEntityPayloadV1 {
        code_hash,
        autonomy_mode: AutonomyMode::Gated,
        capabilities: Capabilities::gated(),
        initial_balance: 100,
    })
    .to_vec();

    // First registration succeeds
    let tx1 = create_test_tx(creator, 0, 10, payload.clone());
    apply_register_ai_entity_tx(&mut db, &tx1, 100).unwrap();

    // Second registration with same code_hash + creator → EntityAlreadyExists
    let tx2 = create_test_tx(creator, 1, 10, payload);
    let result = apply_register_ai_entity_tx(&mut db, &tx2, 101);
    assert!(
        matches!(result, Err(ExecError::EntityAlreadyExists)),
        "Duplicate registration should be rejected"
    );
}

// ============================================================================
// TEST 4: register_entity_autonomous_rejected
// ============================================================================

#[test]
fn register_entity_autonomous_rejected() {
    let mut db = MemKv::new();

    let creator = [0x01u8; 32];
    seed_account(&mut db, &creator, 10_000, 0);

    let payload = encode_register_ai_entity_payload_v1(&RegisterAiEntityPayloadV1 {
        code_hash: [0x42u8; 32],
        autonomy_mode: AutonomyMode::Autonomous,
        capabilities: Capabilities::gated(),
        initial_balance: 100,
    })
    .to_vec();

    let tx = create_test_tx(creator, 0, 10, payload);
    let result = apply_register_ai_entity_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::AutonomousModeReserved)),
        "Autonomous mode should be rejected"
    );
}

// ============================================================================
// TEST 5: register_entity_insufficient_balance
// ============================================================================

#[test]
fn register_entity_insufficient_balance() {
    let mut db = MemKv::new();

    let creator = [0x01u8; 32];
    // Only 50 balance, but initial_balance=100 + fee=10 = 110 needed
    seed_account(&mut db, &creator, 50, 0);

    let payload = encode_register_ai_entity_payload_v1(&RegisterAiEntityPayloadV1 {
        code_hash: [0x42u8; 32],
        autonomy_mode: AutonomyMode::Gated,
        capabilities: Capabilities::gated(),
        initial_balance: 100,
    })
    .to_vec();

    let tx = create_test_tx(creator, 0, 10, payload);
    let result = apply_register_ai_entity_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::InsufficientFunds { .. })),
        "Insufficient balance should be rejected"
    );
}

// ============================================================================
// TEST 6: register_entity_in_smt
// ============================================================================

#[test]
fn register_entity_in_smt() {
    let mut db = MemKv::new();

    let creator = [0x01u8; 32];
    let code_hash = [0x42u8; 32];
    seed_account(&mut db, &creator, 1000, 0);

    let payload = encode_register_ai_entity_payload_v1(&RegisterAiEntityPayloadV1 {
        code_hash,
        autonomy_mode: AutonomyMode::Gated,
        capabilities: Capabilities::gated(),
        initial_balance: 200,
    })
    .to_vec();

    let tx = create_test_tx(creator, 0, 10, payload);
    let entity_id = apply_register_ai_entity_tx(&mut db, &tx, 100).unwrap();

    // Verify entity can be read back with all fields correct
    let entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert_eq!(entity.id, entity_id);
    assert_eq!(entity.code_hash, code_hash);
    assert_eq!(entity.creator, creator);
    assert_eq!(entity.autonomy_mode, AutonomyMode::Gated);
    assert!(entity.capabilities.emit_proposals);
    assert!(entity.capabilities.request_execution);
    assert_eq!(entity.economic_balance, 200);
    assert_eq!(entity.nonce, 0);
    assert_eq!(entity.registered_at, 100);
    assert_eq!(entity.last_active_at, 100);
    assert!(entity.is_active);
}

// ============================================================================
// TEST 7: credit_entity_success
// ============================================================================

#[test]
fn credit_entity_success() {
    let mut db = MemKv::new();

    // Create an entity via direct write (simulating prior registration)
    let creator = [0x01u8; 32];
    let entity = AiEntity::new(
        [0x42u8; 32],
        creator,
        AutonomyMode::Gated,
        Capabilities::gated(),
        50,
    );
    let entity_id = entity.id;
    db.apply_batch(&[write_ai_entity_op(&entity)]).unwrap();

    // Fund the sender
    let sender = [0x02u8; 32];
    seed_account(&mut db, &sender, 1000, 0);

    let credit_amount = 300u128;
    let fee = 5u64;

    let payload = encode_credit_ai_entity_payload_v1(&CreditAiEntityPayloadV1 {
        entity_id,
        amount: credit_amount,
    })
    .to_vec();

    let tx = create_test_tx(sender, 0, fee, payload);
    apply_credit_ai_entity_tx(&mut db, &tx, 100).unwrap();

    // Verify entity balance increased
    let updated_entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert_eq!(updated_entity.economic_balance, credit_amount);

    // Verify sender was debited
    let sender_acct = read_account(&db, &sender);
    assert_eq!(sender_acct.balance, 1000 - credit_amount - fee as u128);
    assert_eq!(sender_acct.nonce, 1);

    // Verify fee pool was credited
    assert_eq!(read_fee_pool(&db), fee as u128);
}

// ============================================================================
// TEST 8: credit_entity_nonexistent_rejected
// ============================================================================

#[test]
fn credit_entity_nonexistent_rejected() {
    let mut db = MemKv::new();

    let sender = [0x02u8; 32];
    seed_account(&mut db, &sender, 1000, 0);

    let fake_entity_id = [0xDEu8; 32];
    let payload = encode_credit_ai_entity_payload_v1(&CreditAiEntityPayloadV1 {
        entity_id: fake_entity_id,
        amount: 100,
    })
    .to_vec();

    let tx = create_test_tx(sender, 0, 10, payload);
    let result = apply_credit_ai_entity_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::EntityNotFound)),
        "Credit to nonexistent entity should be rejected"
    );
}

// ============================================================================
// TEST 9: credit_entity_inactive_rejected
// ============================================================================

#[test]
fn credit_entity_inactive_rejected() {
    let mut db = MemKv::new();

    // Create an inactive entity
    let creator = [0x01u8; 32];
    let mut entity = AiEntity::new(
        [0x42u8; 32],
        creator,
        AutonomyMode::Gated,
        Capabilities::gated(),
        50,
    );
    entity.is_active = false;
    let entity_id = entity.id;
    db.apply_batch(&[write_ai_entity_op(&entity)]).unwrap();

    let sender = [0x02u8; 32];
    seed_account(&mut db, &sender, 1000, 0);

    let payload = encode_credit_ai_entity_payload_v1(&CreditAiEntityPayloadV1 {
        entity_id,
        amount: 100,
    })
    .to_vec();

    let tx = create_test_tx(sender, 0, 10, payload);
    let result = apply_credit_ai_entity_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::EntityNotActive)),
        "Credit to inactive entity should be rejected"
    );
}

// ============================================================================
// TEST 10: credit_entity_overflow_rejected
// ============================================================================

#[test]
fn credit_entity_overflow_rejected() {
    let mut db = MemKv::new();

    // Create entity with near-max balance
    let creator = [0x01u8; 32];
    let mut entity = AiEntity::new(
        [0x42u8; 32],
        creator,
        AutonomyMode::Gated,
        Capabilities::gated(),
        50,
    );
    entity.economic_balance = u128::MAX - 1;
    let entity_id = entity.id;
    db.apply_batch(&[write_ai_entity_op(&entity)]).unwrap();

    let sender = [0x02u8; 32];
    seed_account(&mut db, &sender, u128::MAX, 0);

    let payload = encode_credit_ai_entity_payload_v1(&CreditAiEntityPayloadV1 {
        entity_id,
        amount: 2,
    })
    .to_vec();

    let tx = create_test_tx(sender, 0, 0, payload);
    let result = apply_credit_ai_entity_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::Overflow)),
        "Overflow on entity credit should be rejected"
    );
}

// ============================================================================
// TEST 11: credit_entity_insufficient_sender_balance
// ============================================================================

#[test]
fn credit_entity_insufficient_sender_balance() {
    let mut db = MemKv::new();

    let creator = [0x01u8; 32];
    let entity = AiEntity::new(
        [0x42u8; 32],
        creator,
        AutonomyMode::Gated,
        Capabilities::gated(),
        50,
    );
    let entity_id = entity.id;
    db.apply_batch(&[write_ai_entity_op(&entity)]).unwrap();

    let sender = [0x02u8; 32];
    // Only 50 balance, but amount=100 + fee=10 = 110 needed
    seed_account(&mut db, &sender, 50, 0);

    let payload = encode_credit_ai_entity_payload_v1(&CreditAiEntityPayloadV1 {
        entity_id,
        amount: 100,
    })
    .to_vec();

    let tx = create_test_tx(sender, 0, 10, payload);
    let result = apply_credit_ai_entity_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::InsufficientFunds { .. })),
        "Insufficient sender balance should be rejected"
    );
}

// ============================================================================
// TEST 12: register_then_credit_lifecycle
// ============================================================================

#[test]
fn register_then_credit_lifecycle() {
    let mut db = MemKv::new();

    let creator = [0x01u8; 32];
    let code_hash = [0x42u8; 32];
    seed_account(&mut db, &creator, 10_000, 0);

    // Step 1: Register with initial_balance=100
    let reg_payload = encode_register_ai_entity_payload_v1(&RegisterAiEntityPayloadV1 {
        code_hash,
        autonomy_mode: AutonomyMode::Gated,
        capabilities: Capabilities::gated(),
        initial_balance: 100,
    })
    .to_vec();

    let reg_tx = create_test_tx(creator, 0, 10, reg_payload);
    let entity_id = apply_register_ai_entity_tx(&mut db, &reg_tx, 100).unwrap();

    // Verify initial state
    let entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert_eq!(entity.economic_balance, 100);

    // Step 2: Credit 50 more (creator sends from nonce=1)
    let credit_payload = encode_credit_ai_entity_payload_v1(&CreditAiEntityPayloadV1 {
        entity_id,
        amount: 50,
    })
    .to_vec();

    let credit_tx = create_test_tx(creator, 1, 5, credit_payload);
    apply_credit_ai_entity_tx(&mut db, &credit_tx, 101).unwrap();

    // Verify final entity balance = 100 + 50 = 150
    let entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert_eq!(entity.economic_balance, 150);

    // Verify creator deductions: 100 (initial) + 10 (reg fee) + 50 (credit) + 5 (credit fee) = 165
    let creator_acct = read_account(&db, &creator);
    assert_eq!(creator_acct.balance, 10_000 - 165);
    assert_eq!(creator_acct.nonce, 2);

    // Verify total fees: 10 + 5 = 15
    assert_eq!(read_fee_pool(&db), 15);
}

// ============================================================================
// TEST 13: register_then_signal (end-to-end pipeline)
// ============================================================================

#[test]
fn register_then_signal() {
    let mut db = MemKv::new();

    let creator = [0x01u8; 32];
    let code_hash = [0x42u8; 32];
    seed_account(&mut db, &creator, 10_000, 0);

    // Step 1: Register entity with emit_proposals capability
    let reg_payload = encode_register_ai_entity_payload_v1(&RegisterAiEntityPayloadV1 {
        code_hash,
        autonomy_mode: AutonomyMode::Gated,
        capabilities: Capabilities::gated(),
        initial_balance: 1000,
    })
    .to_vec();

    let reg_tx = create_test_tx(creator, 0, 10, reg_payload);
    let entity_id = apply_register_ai_entity_tx(&mut db, &reg_tx, 100).unwrap();

    // Step 2: Submit signal commitment from the registered entity
    let signal_hash = [0xAAu8; 32];
    let signal_payload = encode_signal_commitment_payload_v1(&SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type: AiSignalType::Prediction,
        issuer_entity_id: entity_id,
    })
    .to_vec();

    // Signal tx is from the entity itself (entity pays fee from its balance)
    let signal_tx = create_test_tx(entity_id, 0, 20, signal_payload);
    let result = apply_signal_commitment_tx(&mut db, &signal_tx, 101);
    assert!(
        result.is_ok(),
        "Signal from registered entity should succeed: {result:?}"
    );

    // Verify entity balance was debited by signal fee
    let entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert_eq!(entity.economic_balance, 1000 - 20);
    assert_eq!(entity.nonce, 1);
    assert_eq!(entity.last_active_at, 101);
}

// ============================================================================
// TEST 14: dispatch_register_entity (payload version 8 through dispatch_tx)
// ============================================================================

#[test]
fn dispatch_register_entity() {
    let mut db = MemKv::new();

    let creator = [0x01u8; 32];
    let code_hash = [0x55u8; 32];
    seed_account(&mut db, &creator, 100_000, 0);

    let payload = encode_register_ai_entity_payload_v1(&RegisterAiEntityPayloadV1 {
        code_hash,
        autonomy_mode: AutonomyMode::Gated,
        capabilities: Capabilities::gated(),
        initial_balance: 200,
    })
    .to_vec();

    // Verify payload version byte is 8
    assert_eq!(payload[0], 8, "Register payload must start with version 8");

    let fee = 5000u64; // MIN_FEE_REGISTER_AI_ENTITY
    let tx = create_test_tx(creator, 0, fee, payload);
    dispatch_tx(&mut db, &tx, 100).unwrap();

    // Verify entity was created via dispatch
    let entity_id = AiEntity::compute_id(&code_hash, &creator);
    let entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert_eq!(entity.code_hash, code_hash);
    assert_eq!(entity.creator, creator);
    assert_eq!(entity.economic_balance, 200);
    assert_eq!(entity.registered_at, 100);

    // Verify creator was debited
    let creator_acct = read_account(&db, &creator);
    assert_eq!(creator_acct.balance, 100_000 - 200 - fee as u128);
    assert_eq!(creator_acct.nonce, 1);
}

// ============================================================================
// TEST 15: dispatch_credit_entity (payload version 9 through dispatch_tx)
// ============================================================================

#[test]
fn dispatch_credit_entity() {
    let mut db = MemKv::new();

    // Pre-create an entity
    let creator = [0x01u8; 32];
    let mut entity = AiEntity::new(
        [0x42u8; 32],
        creator,
        AutonomyMode::Gated,
        Capabilities::gated(),
        50,
    );
    entity.economic_balance = 100;
    let entity_id = entity.id;
    db.apply_batch(&[write_ai_entity_op(&entity)]).unwrap();

    // Fund sender
    let sender = [0x02u8; 32];
    seed_account(&mut db, &sender, 100_000, 0);

    let payload = encode_credit_ai_entity_payload_v1(&CreditAiEntityPayloadV1 {
        entity_id,
        amount: 500,
    })
    .to_vec();

    // Verify payload version byte is 9
    assert_eq!(payload[0], 9, "Credit payload must start with version 9");

    let fee = 100u64; // MIN_FEE_CREDIT_AI_ENTITY
    let tx = create_test_tx(sender, 0, fee, payload);
    dispatch_tx(&mut db, &tx, 200).unwrap();

    // Verify entity balance increased via dispatch
    let updated_entity = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert_eq!(updated_entity.economic_balance, 100 + 500);

    // Verify sender was debited
    let sender_acct = read_account(&db, &sender);
    assert_eq!(sender_acct.balance, 100_000 - 500 - fee as u128);
    assert_eq!(sender_acct.nonce, 1);

    // Verify fee pool
    assert_eq!(read_fee_pool(&db), fee as u128);
}
