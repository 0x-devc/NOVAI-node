//! Week 25: A25.3 Timelock Bypass Attack Tests.
//!
//! PURPOSE: Attempt to execute proposals before timelock has elapsed.
//! All attacks in this file MUST FAIL for the system to be secure.
//!
//! ATTACK VECTORS:
//! - Execute at height before timelock elapsed
//! - Transaction ordering tricks (submit + execute same block)
//! - Height manipulation attempts
//!
//! EXPECTED RESULTS:
//! - All timelock bypass attempts are rejected with `ProposalNotExecutable`
//! - Execution at exact timelock height succeeds
//! - Execution after expiry fails with `ProposalExpired`

#![allow(clippy::doc_markdown)]

use novai_ai_entities::{AiEntity, ApprovalGate, AutonomyMode, Capabilities, GateType};
use novai_codec::encode_approval_gate_v1;
use novai_execution::{
    apply_governance_execute_tx, apply_governance_submit_tx, encode_execute_proposal_payload_v1,
    encode_submit_proposal_payload_v1, read_proposal, write_ai_entity_op, ExecError,
    ExecuteProposalPayloadV1, SubmitProposalPayloadV1,
};
use novai_governance::{ProposalState, ProposalType};
use novai_state::{approval_gate_key, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

// ============================================================================
// TEST HELPERS
// ============================================================================

/// Well-known gate ID for adversarial tests.
fn adversarial_gate_id() -> [u8; 32] {
    *blake3::hash(b"NOVAI_ADVERSARIAL_GATE_V1").as_bytes()
}

/// Create a TimelockOnly gate with specified parameters.
fn create_timelock_gate(timelock_blocks: u64, expiry_blocks: u64) -> ApprovalGate {
    ApprovalGate {
        gate_id: adversarial_gate_id(),
        gate_type: GateType::TimelockOnly,
        required_approvers: Vec::new(),
        threshold: 0,
        timelock_blocks,
        expiry_blocks,
        veto_enabled: false,
        freeze_enabled: false,
    }
}

/// Store a gate in the database.
fn store_gate(db: &mut MemKv, gate: &ApprovalGate) {
    let key = approval_gate_key(&gate.gate_id);
    let value = encode_approval_gate_v1(gate);
    db.apply_batch(&[WriteOp::Put(key, value)]).unwrap();
}

/// Create a test AI entity with specified balance.
fn create_test_entity(name: &[u8], balance: u128, is_active: bool) -> AiEntity {
    let code_hash = *blake3::hash(name).as_bytes();
    let creator = *blake3::hash(&[name, b"_creator"].concat()).as_bytes();

    let mut entity = AiEntity::new(
        code_hash,
        creator,
        AutonomyMode::Gated,
        Capabilities::gated(),
        0,
    );
    entity.economic_balance = balance;
    entity.is_active = is_active;
    entity
}

/// Store an AI entity in the database.
fn store_entity(db: &mut MemKv, entity: &AiEntity) {
    db.apply_batch(&[write_ai_entity_op(entity)]).unwrap();
}

/// Create a submit proposal payload.
fn create_submit_payload(
    proposal_type: ProposalType,
    gate_id: [u8; 32],
    entity_id: [u8; 32],
) -> Vec<u8> {
    let payload = SubmitProposalPayloadV1 {
        proposal_type,
        gate_id,
        proposal_data: entity_id.to_vec(),
    };
    encode_submit_proposal_payload_v1(&payload)
}

/// Create an execute proposal payload.
fn create_execute_payload(proposal_id: [u8; 32]) -> Vec<u8> {
    let payload = ExecuteProposalPayloadV1 { proposal_id };
    encode_execute_proposal_payload_v1(&payload).to_vec()
}

/// Create a test transaction.
// Test helper that constructs a struct from args; Vec param prevents const.
#[allow(clippy::missing_const_for_fn)]
fn create_tx(from: [u8; 32], nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
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

// ============================================================================
// A25.3.1: Execute BEFORE Timelock (MUST FAIL)
// ============================================================================

#[test]
fn attack_execute_before_timelock_rejected() {
    let mut db = MemKv::new();

    // Setup: Entity and gate with 100-block timelock
    let entity = create_test_entity(b"victim_module", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    let gate = create_timelock_gate(100, 1000); // 100 block timelock
    store_gate(&mut db, &gate);

    // Submit proposal at height 500
    let submit_payload = create_submit_payload(
        ProposalType::ModuleRollback,
        adversarial_gate_id(),
        entity_id,
    );
    let submit_tx = create_tx(entity_id, 0, 100, submit_payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 500).unwrap();

    // ATTACK: Try to execute at various heights before timelock (500 + 100 = 600)
    let execute_payload = create_execute_payload(proposal_id);
    let execute_tx = create_tx(entity_id, 1, 100, execute_payload);

    // All these heights are BEFORE timelock should elapse
    for attack_height in [500, 501, 550, 580, 599] {
        let result = apply_governance_execute_tx(&mut db, &execute_tx, attack_height);
        assert!(
            matches!(result, Err(ExecError::ProposalNotExecutable)),
            "SECURITY VIOLATION: Execution succeeded at height {attack_height} (before timelock 600)",
        );
    }

    // Verify proposal state unchanged (still Approved, not Executed)
    let proposal = read_proposal(&db, &proposal_id).unwrap().unwrap();
    assert_eq!(
        proposal.state,
        ProposalState::Approved,
        "Proposal state should remain Approved after failed attacks"
    );
}

#[test]
fn attack_execute_one_block_before_timelock_rejected() {
    let mut db = MemKv::new();

    let entity = create_test_entity(b"edge_case_module", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    let gate = create_timelock_gate(50, 500);
    store_gate(&mut db, &gate);

    // Submit at height 1000, executable at 1050
    let submit_payload = create_submit_payload(
        ProposalType::ModuleRollback,
        adversarial_gate_id(),
        entity_id,
    );
    let submit_tx = create_tx(entity_id, 0, 100, submit_payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 1000).unwrap();

    // ATTACK: Execute at height 1049 (one block before timelock)
    let execute_payload = create_execute_payload(proposal_id);
    let execute_tx = create_tx(entity_id, 1, 100, execute_payload);

    let result = apply_governance_execute_tx(&mut db, &execute_tx, 1049);
    assert!(
        matches!(result, Err(ExecError::ProposalNotExecutable)),
        "SECURITY VIOLATION: Execution succeeded at height 1049 (1 block before timelock)"
    );
}

// ============================================================================
// A25.3.2: Execute EXACTLY at Timelock (MUST SUCCEED)
// ============================================================================

#[test]
fn execute_at_exact_timelock_succeeds() {
    let mut db = MemKv::new();

    let entity = create_test_entity(b"exact_timelock_module", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    let gate = create_timelock_gate(100, 1000);
    store_gate(&mut db, &gate);

    // Submit at height 500, executable at 600
    let submit_payload = create_submit_payload(
        ProposalType::ModuleRollback,
        adversarial_gate_id(),
        entity_id,
    );
    let submit_tx = create_tx(entity_id, 0, 100, submit_payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 500).unwrap();

    // Execute at EXACTLY height 600
    let execute_payload = create_execute_payload(proposal_id);
    let execute_tx = create_tx(entity_id, 1, 100, execute_payload);

    let result = apply_governance_execute_tx(&mut db, &execute_tx, 600);
    assert!(
        result.is_ok(),
        "Execution at exact timelock height should succeed: {result:?}",
    );

    // Verify proposal is now executed
    let proposal = read_proposal(&db, &proposal_id).unwrap().unwrap();
    assert_eq!(proposal.state, ProposalState::Executed);
}

// ============================================================================
// A25.3.3: Execute AFTER Timelock but BEFORE Expiry (MUST SUCCEED)
// ============================================================================

#[test]
fn execute_after_timelock_before_expiry_succeeds() {
    let mut db = MemKv::new();

    let entity = create_test_entity(b"normal_execution_module", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    let gate = create_timelock_gate(100, 1000); // Expires at submit + 1000
    store_gate(&mut db, &gate);

    // Submit at height 500, executable at 600, expires at 1500
    let submit_payload = create_submit_payload(
        ProposalType::ModuleRollback,
        adversarial_gate_id(),
        entity_id,
    );
    let submit_tx = create_tx(entity_id, 0, 100, submit_payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 500).unwrap();

    // Execute at height 800 (after timelock, before expiry)
    let execute_payload = create_execute_payload(proposal_id);
    let execute_tx = create_tx(entity_id, 1, 100, execute_payload);

    let result = apply_governance_execute_tx(&mut db, &execute_tx, 800);
    assert!(
        result.is_ok(),
        "Execution after timelock should succeed: {result:?}",
    );
}

// ============================================================================
// A25.3.4: Execute AFTER Expiry (MUST FAIL)
// ============================================================================

#[test]
fn attack_execute_after_expiry_rejected() {
    let mut db = MemKv::new();

    let entity = create_test_entity(b"expired_module", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    let gate = create_timelock_gate(10, 100); // Short expiry for testing
    store_gate(&mut db, &gate);

    // Submit at height 500, executable at 510, expires at 600
    let submit_payload = create_submit_payload(
        ProposalType::ModuleRollback,
        adversarial_gate_id(),
        entity_id,
    );
    let submit_tx = create_tx(entity_id, 0, 100, submit_payload);

    for attack_height in [600, 601, 700, 1000] {
        // Fresh DB state for each test
        let mut db2 = MemKv::new();
        store_entity(&mut db2, &entity);
        store_gate(&mut db2, &gate);
        let proposal_id2 = apply_governance_submit_tx(&mut db2, &submit_tx, 500).unwrap();
        let execute_payload2 = create_execute_payload(proposal_id2);
        let execute_tx2 = create_tx(entity_id, 1, 100, execute_payload2);

        let result = apply_governance_execute_tx(&mut db2, &execute_tx2, attack_height);
        assert!(
            matches!(result, Err(ExecError::ProposalExpired)),
            "SECURITY VIOLATION: Execution succeeded at height {attack_height} (after expiry 600)",
        );
    }
}

#[test]
fn attack_execute_at_exact_expiry_rejected() {
    let mut db = MemKv::new();

    let entity = create_test_entity(b"exact_expiry_module", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    let gate = create_timelock_gate(10, 100);
    store_gate(&mut db, &gate);

    // Submit at height 500, expires at 600 (500 + 100)
    let submit_payload = create_submit_payload(
        ProposalType::ModuleRollback,
        adversarial_gate_id(),
        entity_id,
    );
    let submit_tx = create_tx(entity_id, 0, 100, submit_payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 500).unwrap();

    // ATTACK: Execute at EXACTLY the expiry height
    let execute_payload = create_execute_payload(proposal_id);
    let execute_tx = create_tx(entity_id, 1, 100, execute_payload);

    let result = apply_governance_execute_tx(&mut db, &execute_tx, 600);
    assert!(
        matches!(result, Err(ExecError::ProposalExpired)),
        "SECURITY VIOLATION: Execution succeeded at exact expiry height"
    );
}

// ============================================================================
// A25.3.5: Same-Block Submit + Execute (MUST FAIL if timelock > 0)
// ============================================================================

#[test]
fn attack_same_block_submit_execute_rejected() {
    let mut db = MemKv::new();

    let entity = create_test_entity(b"same_block_module", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    // Gate with non-zero timelock
    let gate = create_timelock_gate(1, 1000); // Even 1 block timelock
    store_gate(&mut db, &gate);

    // Submit at height 500
    let submit_payload = create_submit_payload(
        ProposalType::ModuleRollback,
        adversarial_gate_id(),
        entity_id,
    );
    let submit_tx = create_tx(entity_id, 0, 100, submit_payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 500).unwrap();

    // ATTACK: Try to execute in the SAME block (height 500)
    let execute_payload = create_execute_payload(proposal_id);
    let execute_tx = create_tx(entity_id, 1, 100, execute_payload);

    let result = apply_governance_execute_tx(&mut db, &execute_tx, 500);
    assert!(
        matches!(result, Err(ExecError::ProposalNotExecutable)),
        "SECURITY VIOLATION: Same-block submit+execute succeeded with timelock > 0"
    );
}

#[test]
fn same_block_submit_execute_with_zero_timelock_succeeds() {
    let mut db = MemKv::new();

    let entity = create_test_entity(b"zero_timelock_module", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    // Gate with ZERO timelock (immediate execution allowed)
    let gate = create_timelock_gate(0, 1000);
    store_gate(&mut db, &gate);

    // Submit at height 500
    let submit_payload = create_submit_payload(
        ProposalType::ModuleRollback,
        adversarial_gate_id(),
        entity_id,
    );
    let submit_tx = create_tx(entity_id, 0, 100, submit_payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 500).unwrap();

    // Execute in the SAME block (should succeed with zero timelock)
    let execute_payload = create_execute_payload(proposal_id);
    let execute_tx = create_tx(entity_id, 1, 100, execute_payload);

    let result = apply_governance_execute_tx(&mut db, &execute_tx, 500);
    assert!(
        result.is_ok(),
        "Zero-timelock same-block execution should succeed: {result:?}",
    );
}

// ============================================================================
// A25.3.6: Height Underflow/Overflow Attacks
// ============================================================================

#[test]
fn attack_height_overflow_handled_gracefully() {
    let mut db = MemKv::new();

    let entity = create_test_entity(b"overflow_module", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    let gate = create_timelock_gate(100, 1000);
    store_gate(&mut db, &gate);

    // Submit at very high height (near u64::MAX)
    let high_height = u64::MAX - 500;
    let submit_payload = create_submit_payload(
        ProposalType::ModuleRollback,
        adversarial_gate_id(),
        entity_id,
    );
    let submit_tx = create_tx(entity_id, 0, 100, submit_payload);

    // This should use saturating_add internally, so executable_at won't overflow
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, high_height).unwrap();

    // Try to execute - should be handled gracefully
    let execute_payload = create_execute_payload(proposal_id);
    let execute_tx = create_tx(entity_id, 1, 100, execute_payload);

    // Even at u64::MAX, timelock logic should not panic or behave unexpectedly
    let result = apply_governance_execute_tx(&mut db, &execute_tx, u64::MAX);
    // Result depends on whether saturating_add capped the executable_at
    // Either succeeds (if capped at MAX) or fails (if timelock not elapsed)
    // Key point: NO PANIC - test passes if we reach this point
    assert!(
        result.is_ok() || matches!(result, Err(ExecError::ProposalExpired)),
        "Height overflow should be handled gracefully, got: {result:?}",
    );
}

// ============================================================================
// A25.3.7: Multiple Rapid Execution Attempts
// ============================================================================

#[test]
fn attack_rapid_execution_attempts_all_rejected() {
    let mut db = MemKv::new();

    let entity = create_test_entity(b"rapid_attack_module", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    let gate = create_timelock_gate(100, 1000);
    store_gate(&mut db, &gate);

    // Submit at height 500
    let submit_payload = create_submit_payload(
        ProposalType::ModuleRollback,
        adversarial_gate_id(),
        entity_id,
    );
    let submit_tx = create_tx(entity_id, 0, 100, submit_payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 500).unwrap();

    let execute_payload = create_execute_payload(proposal_id);
    let execute_tx = create_tx(entity_id, 1, 100, execute_payload);

    // ATTACK: Rapid-fire execution attempts before timelock
    let mut rejection_count = 0;
    for attempt in 0..100 {
        let attack_height = 500 + attempt; // Heights 500-599 (all before timelock)
        let result = apply_governance_execute_tx(&mut db, &execute_tx, attack_height);
        if matches!(result, Err(ExecError::ProposalNotExecutable)) {
            rejection_count += 1;
        }
    }

    assert_eq!(
        rejection_count, 100,
        "All 100 rapid execution attempts should be rejected"
    );
}

// ============================================================================
// SUMMARY TEST: Comprehensive Timelock Boundary Test
// ============================================================================

#[test]
fn comprehensive_timelock_boundary_test() {
    let mut db = MemKv::new();

    let entity = create_test_entity(b"boundary_test_module", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    // Gate: timelock=50, expiry=200
    let gate = create_timelock_gate(50, 200);
    store_gate(&mut db, &gate);

    // Submit at height 1000
    // executable_at = 1000 + 50 = 1050
    // expires_at = 1000 + 200 = 1200
    let submit_payload = create_submit_payload(
        ProposalType::ModuleRollback,
        adversarial_gate_id(),
        entity_id,
    );
    let submit_tx = create_tx(entity_id, 0, 100, submit_payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 1000).unwrap();

    let execute_payload = create_execute_payload(proposal_id);

    // Test matrix:
    // Heights 1000-1049: MUST FAIL (before timelock)
    // Heights 1050-1199: MUST SUCCEED (after timelock, before expiry)
    // Heights 1200+: MUST FAIL (expired)

    // Before timelock (sample points)
    for h in [1000, 1025, 1049] {
        let mut db_copy = db.clone();
        let tx = create_tx(entity_id, 1, 100, execute_payload.clone());
        let result = apply_governance_execute_tx(&mut db_copy, &tx, h);
        assert!(
            matches!(result, Err(ExecError::ProposalNotExecutable)),
            "Height {h}: Expected ProposalNotExecutable, got {result:?}",
        );
    }

    // At and after timelock, before expiry (sample points)
    for h in [1050, 1100, 1150, 1199] {
        let mut db_copy = db.clone();
        let tx = create_tx(entity_id, 1, 100, execute_payload.clone());
        let result = apply_governance_execute_tx(&mut db_copy, &tx, h);
        assert!(result.is_ok(), "Height {h}: Expected Ok, got {result:?}",);
    }

    // At and after expiry (sample points)
    for h in [1200, 1250, 1500] {
        let mut db_copy = db.clone();
        let tx = create_tx(entity_id, 1, 100, execute_payload.clone());
        let result = apply_governance_execute_tx(&mut db_copy, &tx, h);
        assert!(
            matches!(result, Err(ExecError::ProposalExpired)),
            "Height {h}: Expected ProposalExpired, got {result:?}",
        );
    }
}
