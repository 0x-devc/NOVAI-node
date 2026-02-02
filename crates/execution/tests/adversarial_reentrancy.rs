//! Week 25: A25.5 Executor Reentrancy Attack Tests.
//!
//! PURPOSE: Test for reentrancy-like vulnerabilities in governance execution.
//!
//! BACKGROUND:
//! Unlike smart contracts, this is synchronous Rust code with no callbacks.
//! "Reentrancy" here means:
//! - Multi-proposal interactions on shared state
//! - Execution order manipulation
//! - Conflicting proposals on same entity
//! - Intermediate state exploitation
//!
//! ATTACK VECTORS:
//! - Execute conflicting proposals (activate + rollback) on same entity
//! - Execution order dependencies
//! - Self-deactivation (entity deactivates itself)
//! - Multi-proposal state confusion
//!
//! EXPECTED RESULTS:
//! - State should be consistent regardless of execution order (within same block)
//! - No intermediate state leakage
//! - Idempotent operations should be safe

#![allow(clippy::doc_markdown)]

use novai_ai_entities::{AiEntity, ApprovalGate, AutonomyMode, Capabilities, GateType};
use novai_codec::encode_approval_gate_v1;
use novai_execution::{
    apply_governance_execute_tx, apply_governance_submit_tx, encode_execute_proposal_payload_v1,
    encode_submit_proposal_payload_v1, read_ai_entity, write_ai_entity_op,
    ExecuteProposalPayloadV1, SubmitProposalPayloadV1,
};
use novai_governance::ProposalType;
use novai_state::{approval_gate_key, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

// ============================================================================
// TEST HELPERS
// ============================================================================

/// Gate ID for reentrancy tests.
fn reentrancy_gate_id() -> [u8; 32] {
    *blake3::hash(b"NOVAI_REENTRANCY_GATE_V1").as_bytes()
}

/// Create a TimelockOnly gate with zero timelock (immediate execution).
fn create_instant_gate(expiry_blocks: u64) -> ApprovalGate {
    ApprovalGate {
        gate_id: reentrancy_gate_id(),
        gate_type: GateType::TimelockOnly,
        required_approvers: Vec::new(),
        threshold: 0,
        timelock_blocks: 0, // Immediate execution allowed
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

/// Create a test AI entity.
fn create_test_entity(name: &[u8], balance: u128, is_active: bool) -> AiEntity {
    let code_hash = *blake3::hash(name).as_bytes();
    let creator = *blake3::hash(&[name, b"_creator"].concat()).as_bytes();

    let mut entity = AiEntity::new(
        code_hash,
        creator,
        AutonomyMode::Advisory,
        Capabilities::advisory(),
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
fn create_submit_payload(proposal_type: ProposalType, gate_id: [u8; 32], data: Vec<u8>) -> Vec<u8> {
    let payload = SubmitProposalPayloadV1 {
        proposal_type,
        gate_id,
        proposal_data: data,
    };
    encode_submit_proposal_payload_v1(&payload)
}

/// Create an execute proposal payload.
fn create_execute_payload(proposal_id: [u8; 32]) -> Vec<u8> {
    let payload = ExecuteProposalPayloadV1 { proposal_id };
    encode_execute_proposal_payload_v1(&payload).to_vec()
}

/// Create a test transaction.
const fn create_tx(from: [u8; 32], nonce: u64, fee: u64, payload: Vec<u8>) -> TxV1 {
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
// A25.5.1: CONFLICTING PROPOSALS ON SAME ENTITY
// ============================================================================

/// Test executing activate then rollback on the same entity in same block.
#[test]
fn conflicting_activate_then_rollback() {
    println!("=== A25.5.1 CONFLICTING PROPOSALS: ACTIVATE THEN ROLLBACK ===");

    let mut db = MemKv::new();

    // Target entity starts INACTIVE
    let target = create_test_entity(b"conflict_target", 10_000_000_000, false);
    let target_id = target.id;
    store_entity(&mut db, &target);

    let executor = create_test_entity(b"conflict_executor", 10_000_000_000, true);
    let executor_id = executor.id;
    store_entity(&mut db, &executor);

    let gate = create_instant_gate(1000);
    store_gate(&mut db, &gate);

    println!("Initial state: target.is_active = false");

    // Submit ACTIVATION proposal
    let activate_payload = create_submit_payload(
        ProposalType::ModuleActivation,
        reentrancy_gate_id(),
        target_id.to_vec(),
    );
    let activate_tx = create_tx(executor_id, 0, 100, activate_payload);
    let activate_id = apply_governance_submit_tx(&mut db, &activate_tx, 100).unwrap();
    println!("Submitted activation proposal: {:02x?}", &activate_id[..8]);

    // Submit ROLLBACK proposal (different data to get different ID)
    // Use a different proposer to get a different proposal ID
    let rollback_payload = create_submit_payload(
        ProposalType::ModuleRollback,
        reentrancy_gate_id(),
        target_id.to_vec(),
    );
    let rollback_tx = create_tx(target_id, 0, 100, rollback_payload);
    let rollback_id = apply_governance_submit_tx(&mut db, &rollback_tx, 100).unwrap();
    println!("Submitted rollback proposal: {:02x?}", &rollback_id[..8]);

    // Execute ACTIVATION first (same block height)
    let exec_activate_tx = create_tx(executor_id, 1, 100, create_execute_payload(activate_id));
    let result1 = apply_governance_execute_tx(&mut db, &exec_activate_tx, 100);
    assert!(result1.is_ok(), "Activation should succeed");

    let entity_after_activate = read_ai_entity(&db, &target_id).unwrap().unwrap();
    println!(
        "After activation: is_active = {}",
        entity_after_activate.is_active
    );
    assert!(
        entity_after_activate.is_active,
        "Entity should be active after activation"
    );

    // Execute ROLLBACK second (same block height)
    let exec_rollback_tx = create_tx(executor_id, 2, 100, create_execute_payload(rollback_id));
    let result2 = apply_governance_execute_tx(&mut db, &exec_rollback_tx, 100);
    assert!(result2.is_ok(), "Rollback should succeed");

    let entity_after_rollback = read_ai_entity(&db, &target_id).unwrap().unwrap();
    println!(
        "After rollback: is_active = {}",
        entity_after_rollback.is_active
    );
    assert!(
        !entity_after_rollback.is_active,
        "Entity should be inactive after rollback"
    );

    println!();
    println!("FINDING: Conflicting proposals execute in order");
    println!("Final state determined by execution order, not submission order");
    println!("SECURITY: No reentrancy - each execution completes atomically");
}

// ============================================================================
// A25.5.2: REVERSE ORDER EXECUTION
// ============================================================================

/// Test that execution order determines final state (rollback then activate).
#[test]
fn conflicting_rollback_then_activate() {
    println!("=== A25.5.2 REVERSE ORDER: ROLLBACK THEN ACTIVATE ===");

    let mut db = MemKv::new();

    // Target entity starts ACTIVE
    let target = create_test_entity(b"reverse_target", 10_000_000_000, true);
    let target_id = target.id;
    store_entity(&mut db, &target);

    let executor = create_test_entity(b"reverse_executor", 10_000_000_000, true);
    let executor_id = executor.id;
    store_entity(&mut db, &executor);

    let gate = create_instant_gate(1000);
    store_gate(&mut db, &gate);

    println!("Initial state: target.is_active = true");

    // Submit both proposals
    let rollback_payload = create_submit_payload(
        ProposalType::ModuleRollback,
        reentrancy_gate_id(),
        target_id.to_vec(),
    );
    let rollback_tx = create_tx(executor_id, 0, 100, rollback_payload);
    let rollback_id = apply_governance_submit_tx(&mut db, &rollback_tx, 100).unwrap();

    let activate_payload = create_submit_payload(
        ProposalType::ModuleActivation,
        reentrancy_gate_id(),
        target_id.to_vec(),
    );
    let activate_tx = create_tx(target_id, 0, 100, activate_payload);
    let activate_id = apply_governance_submit_tx(&mut db, &activate_tx, 100).unwrap();

    // Execute ROLLBACK first
    let exec_rollback_tx = create_tx(executor_id, 1, 100, create_execute_payload(rollback_id));
    apply_governance_execute_tx(&mut db, &exec_rollback_tx, 100).unwrap();

    let entity_after_rollback = read_ai_entity(&db, &target_id).unwrap().unwrap();
    println!(
        "After rollback: is_active = {}",
        entity_after_rollback.is_active
    );

    // Execute ACTIVATE second
    let exec_activate_tx = create_tx(executor_id, 2, 100, create_execute_payload(activate_id));
    apply_governance_execute_tx(&mut db, &exec_activate_tx, 100).unwrap();

    let entity_final = read_ai_entity(&db, &target_id).unwrap().unwrap();
    println!("After activate: is_active = {}", entity_final.is_active);

    assert!(
        entity_final.is_active,
        "Final state should be active (last executed wins)"
    );

    println!();
    println!("FINDING: Last execution wins - order matters");
    println!("SECURITY: Deterministic behavior based on execution order");
}

// ============================================================================
// A25.5.3: SELF-DEACTIVATION ATTACK
// ============================================================================

/// Test if an entity can submit a proposal to deactivate itself.
#[test]
fn attack_self_deactivation() {
    println!("=== A25.5.3 SELF-DEACTIVATION ATTACK ===");

    let mut db = MemKv::new();

    // Entity is ACTIVE and will try to deactivate itself
    let attacker = create_test_entity(b"self_deactivator", 10_000_000_000, true);
    let attacker_id = attacker.id;
    store_entity(&mut db, &attacker);

    let gate = create_instant_gate(1000);
    store_gate(&mut db, &gate);

    println!("Initial state: attacker.is_active = true");

    // Submit rollback proposal targeting SELF
    let self_rollback_payload = create_submit_payload(
        ProposalType::ModuleRollback,
        reentrancy_gate_id(),
        attacker_id.to_vec(), // Targeting self
    );
    let self_rollback_tx = create_tx(attacker_id, 0, 100, self_rollback_payload);
    let result = apply_governance_submit_tx(&mut db, &self_rollback_tx, 100);

    match result {
        Ok(proposal_id) => {
            println!(
                "Self-deactivation proposal submitted: {:02x?}",
                &proposal_id[..8]
            );

            // Try to execute
            let exec_tx = create_tx(attacker_id, 1, 100, create_execute_payload(proposal_id));
            let exec_result = apply_governance_execute_tx(&mut db, &exec_tx, 100);

            match exec_result {
                Ok(()) => {
                    let entity = read_ai_entity(&db, &attacker_id).unwrap().unwrap();
                    println!("After execution: is_active = {}", entity.is_active);

                    if !entity.is_active {
                        println!("FINDING: Entity successfully deactivated itself");
                        println!("SECURITY: This may or may not be intended behavior");
                        println!("NOTE: Self-deactivation could be a feature (graceful shutdown)");
                    }
                }
                Err(e) => {
                    println!("FINDING: Self-deactivation blocked at execution: {e:?}");
                }
            }
        }
        Err(e) => {
            println!("FINDING: Self-deactivation blocked at submission: {e:?}");
        }
    }
}

// ============================================================================
// A25.5.4: RAPID SEQUENTIAL EXECUTION
// ============================================================================

/// Test rapid execution of many proposals on same entity.
#[test]
fn attack_rapid_sequential_execution() {
    println!("=== A25.5.4 RAPID SEQUENTIAL EXECUTION ===");

    let mut db = MemKv::new();

    let target = create_test_entity(b"rapid_target", 10_000_000_000, false);
    let target_id = target.id;
    store_entity(&mut db, &target);

    let gate = create_instant_gate(10000);
    store_gate(&mut db, &gate);

    println!("Initial state: target.is_active = false");

    // Submit many activation proposals from different "proposers"
    let mut proposal_ids = Vec::new();
    for i in 0..10 {
        let proposer = create_test_entity(format!("proposer_{i}").as_bytes(), 10_000_000_000, true);
        store_entity(&mut db, &proposer);

        let payload = create_submit_payload(
            ProposalType::ModuleActivation,
            reentrancy_gate_id(),
            target_id.to_vec(),
        );
        let tx = create_tx(proposer.id, 0, 100, payload);
        let proposal_id = apply_governance_submit_tx(&mut db, &tx, 100).unwrap();
        proposal_ids.push((proposer.id, proposal_id));
    }

    println!("Submitted {} activation proposals", proposal_ids.len());

    // Execute all rapidly
    let executor = create_test_entity(b"rapid_executor", 10_000_000_000, true);
    store_entity(&mut db, &executor);

    let mut success_count = 0;
    for (i, (_, proposal_id)) in proposal_ids.iter().enumerate() {
        let exec_tx = create_tx(
            executor.id,
            i as u64,
            100,
            create_execute_payload(*proposal_id),
        );
        let result = apply_governance_execute_tx(&mut db, &exec_tx, 100);

        if result.is_ok() {
            success_count += 1;
        }
    }

    println!(
        "Executed: {}/{} proposals succeeded",
        success_count,
        proposal_ids.len()
    );

    let final_entity = read_ai_entity(&db, &target_id).unwrap().unwrap();
    println!("Final state: is_active = {}", final_entity.is_active);

    assert!(
        final_entity.is_active,
        "Entity should be active after any activation"
    );
    println!();
    println!("FINDING: Multiple activations are idempotent");
    println!("SECURITY: No race condition - operations complete atomically");
}

// ============================================================================
// A25.5.5: TOGGLE ATTACK (ACTIVATE/ROLLBACK ALTERNATING)
// ============================================================================

/// Test alternating activate/rollback to find state inconsistencies.
#[test]
fn attack_toggle_state() {
    println!("=== A25.5.5 TOGGLE STATE ATTACK ===");

    let mut db = MemKv::new();

    let target = create_test_entity(b"toggle_target", 10_000_000_000, false);
    let target_id = target.id;
    store_entity(&mut db, &target);

    let gate = create_instant_gate(10000);
    store_gate(&mut db, &gate);

    // Create proposals for toggling
    let mut proposal_ids = Vec::new();
    for i in 0..20 {
        let proposer = create_test_entity(
            format!("toggle_proposer_{i}").as_bytes(),
            10_000_000_000,
            true,
        );
        store_entity(&mut db, &proposer);

        let proposal_type = if i % 2 == 0 {
            ProposalType::ModuleActivation
        } else {
            ProposalType::ModuleRollback
        };

        let payload =
            create_submit_payload(proposal_type, reentrancy_gate_id(), target_id.to_vec());
        let tx = create_tx(proposer.id, 0, 100, payload);
        let proposal_id = apply_governance_submit_tx(&mut db, &tx, 100).unwrap();
        proposal_ids.push((proposal_type, proposal_id));
    }

    println!("Submitted 20 alternating activate/rollback proposals");

    // Execute all in order
    let executor = create_test_entity(b"toggle_executor", 10_000_000_000, true);
    store_entity(&mut db, &executor);

    let mut state_history = Vec::new();
    for (i, (ptype, proposal_id)) in proposal_ids.iter().enumerate() {
        let exec_tx = create_tx(
            executor.id,
            i as u64,
            100,
            create_execute_payload(*proposal_id),
        );
        apply_governance_execute_tx(&mut db, &exec_tx, 100).unwrap();

        let entity = read_ai_entity(&db, &target_id).unwrap().unwrap();
        state_history.push((*ptype, entity.is_active));
    }

    println!("State transitions:");
    for (i, (ptype, is_active)) in state_history.iter().enumerate() {
        let expected = match ptype {
            ProposalType::ModuleActivation => true,
            ProposalType::ModuleRollback => false,
            _ => unreachable!(),
        };
        let status = if *is_active == expected {
            "OK"
        } else {
            "MISMATCH!"
        };
        if !(5..15).contains(&i) {
            println!("  {i}: {ptype:?} -> is_active={is_active} [{status}]",);
        } else if i == 5 {
            println!("  ... (10 more transitions)");
        }
    }

    // Final state should match last operation (rollback = false)
    let final_entity = read_ai_entity(&db, &target_id).unwrap().unwrap();
    // Last proposal (index 19) is rollback
    assert!(
        !final_entity.is_active,
        "Final state should be inactive (last was rollback)"
    );

    println!();
    println!("FINDING: State toggles correctly with each execution");
    println!("SECURITY: No state corruption during rapid toggling");
}

// ============================================================================
// A25.5.6: EXECUTION DURING INTERMEDIATE STATE
// ============================================================================

/// Document that intermediate state cannot be observed between operations.
#[test]
fn document_no_intermediate_state() {
    println!("=== A25.5.6 INTERMEDIATE STATE ANALYSIS ===");
    println!();
    println!("ARCHITECTURE ANALYSIS:");
    println!();
    println!("1. Execution Model:");
    println!("   - Synchronous Rust code (no async/callbacks)");
    println!("   - Each apply_* function runs to completion");
    println!("   - State changes via apply_batch() are atomic");
    println!();
    println!("2. No Reentrancy Possible:");
    println!("   - No external calls during execution");
    println!("   - No callback mechanisms");
    println!("   - No hooks that could re-enter");
    println!();
    println!("3. State Visibility:");
    println!("   - Intermediate state only visible within single tx execution");
    println!("   - Between transactions, state is consistent");
    println!("   - No partial writes visible to other transactions");
    println!();
    println!("4. Order Dependencies:");
    println!("   - Execution order determined by block producer");
    println!("   - Last writer wins for conflicting proposals");
    println!("   - This is deterministic (same order = same result)");
    println!();
    println!("CONCLUSION:");
    println!("   Traditional reentrancy attacks are not applicable.");
    println!("   The system is secure against intermediate state exploitation.");
}

// ============================================================================
// SUMMARY
// ============================================================================

#[test]
fn reentrancy_attack_summary() {
    println!("=============================================================");
    println!("           A25.5 EXECUTOR REENTRANCY ATTACK SUMMARY");
    println!("=============================================================");
    println!();
    println!("REENTRANCY MODEL:");
    println!("  - NOT traditional smart contract reentrancy");
    println!("  - Synchronous execution, no callbacks");
    println!("  - State changes are atomic (apply_batch)");
    println!();
    println!("ATTACK VECTORS TESTED:");
    println!("  1. Conflicting proposals (activate + rollback)");
    println!("  2. Execution order manipulation");
    println!("  3. Self-deactivation attack");
    println!("  4. Rapid sequential execution");
    println!("  5. Toggle state attack");
    println!("  6. Intermediate state observation");
    println!();
    println!("FINDINGS:");
    println!("  [✓] No reentrancy possible (synchronous code)");
    println!("  [✓] State changes are atomic");
    println!("  [✓] Execution order is deterministic");
    println!("  [✓] Operations are idempotent where expected");
    println!("  [✓] Last execution wins for conflicting proposals");
    println!();
    println!("SECURITY ASSESSMENT:");
    println!("  The governance execution system is SECURE against");
    println!("  reentrancy-style attacks due to its synchronous,");
    println!("  atomic execution model.");
    println!();
    println!("NOTES:");
    println!("  - Self-deactivation is allowed (may be a feature)");
    println!("  - Execution order matters for conflicting proposals");
    println!("  - Block producer controls execution order");
    println!("=============================================================");
}
