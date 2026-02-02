//! Week 25: A25.2 Approval Replay Attack Tests.
//!
//! PURPOSE: Test the governance system's resilience to approval replay attacks.
//!
//! ATTACK VECTORS:
//! - Replay approvals from one proposal to another
//! - Resubmit proposals to inherit previous approvals
//! - Switch gates to bypass approval requirements
//! - Exploit TimelockOnly gate to avoid approval collection
//!
//! EXPECTED RESULTS:
//! - Proposal IDs should be deterministic (same content = same ID)
//! - Approvals should be bound to specific proposal instances
//! - Gate requirements should be enforced at execution time
//!
//! FINDINGS DOCUMENTATION:
//! This file documents actual behavior vs expected behavior.
//! Any security gaps discovered will be noted for hardening.

#![allow(clippy::doc_markdown)]

use novai_ai_entities::{AiEntity, ApprovalGate, AutonomyMode, Capabilities, GateType};
use novai_codec::encode_approval_gate_v1;
use novai_execution::{
    apply_governance_execute_tx, apply_governance_submit_tx, encode_execute_proposal_payload_v1,
    encode_submit_proposal_payload_v1, read_proposal, write_ai_entity_op, ExecError,
    ExecuteProposalPayloadV1, SubmitProposalPayloadV1,
};
use novai_governance::{Proposal, ProposalState, ProposalType};
use novai_state::{approval_gate_key, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

// ============================================================================
// TEST HELPERS
// ============================================================================

/// Gate ID for TimelockOnly tests.
fn timelock_gate_id() -> [u8; 32] {
    *blake3::hash(b"NOVAI_TIMELOCK_ONLY_GATE_V1").as_bytes()
}

/// Gate ID for Multisig tests.
fn multisig_gate_id() -> [u8; 32] {
    *blake3::hash(b"NOVAI_MULTISIG_GATE_V1").as_bytes()
}

/// Create a TimelockOnly gate (auto-approve).
fn create_timelock_only_gate(timelock_blocks: u64, expiry_blocks: u64) -> ApprovalGate {
    ApprovalGate {
        gate_id: timelock_gate_id(),
        gate_type: GateType::TimelockOnly,
        required_approvers: Vec::new(),
        threshold: 0,
        timelock_blocks,
        expiry_blocks,
        veto_enabled: false,
        freeze_enabled: false,
    }
}

/// Create a Multisig gate requiring specified approvers.
fn create_multisig_gate(
    approvers: Vec<[u8; 32]>,
    threshold: u32,
    timelock_blocks: u64,
    expiry_blocks: u64,
) -> ApprovalGate {
    ApprovalGate {
        gate_id: multisig_gate_id(),
        gate_type: GateType::Multisig,
        required_approvers: approvers,
        threshold,
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
// Test helper with simple construction; const not possible due to Vec payload.
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

/// Test approver addresses.
fn approver_1() -> [u8; 32] {
    *blake3::hash(b"APPROVER_1").as_bytes()
}

fn approver_2() -> [u8; 32] {
    *blake3::hash(b"APPROVER_2").as_bytes()
}

// ============================================================================
// A25.2.1: PROPOSAL ID DETERMINISM TEST
// ============================================================================

/// Test that proposals with identical content get the same ID.
/// This is intentional design for deduplication but can be an attack vector.
#[test]
fn proposal_id_is_deterministic() {
    println!("=== A25.2.1 PROPOSAL ID DETERMINISM ===");

    let mut db = MemKv::new();

    let entity = create_test_entity(b"determinism_test", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    let gate = create_timelock_only_gate(50, 200);
    store_gate(&mut db, &gate);

    let proposer = entity_id;
    let data = b"set MIN_FEE=100".to_vec();

    // Submit first proposal
    let payload =
        create_submit_payload(ProposalType::ParamChange, timelock_gate_id(), data.clone());
    let tx1 = create_tx(proposer, 0, 100, payload);
    let result1 = apply_governance_submit_tx(&mut db, &tx1, 100);
    assert!(result1.is_ok());
    let proposal_id_1 = result1.unwrap();

    // Compute expected ID
    let expected_id = Proposal::compute_id(
        ProposalType::ParamChange,
        &proposer,
        &timelock_gate_id(),
        &data,
    );

    assert_eq!(
        proposal_id_1, expected_id,
        "Proposal ID should be deterministically computed"
    );
    println!("Proposal ID: {:02x?}", &proposal_id_1[..8]);
    println!("FINDING: Proposal ID is deterministic from content");

    // If we try to submit again with same content, what happens?
    let payload2 = create_submit_payload(ProposalType::ParamChange, timelock_gate_id(), data);
    let tx2 = create_tx(proposer, 1, 100, payload2);
    let result2 = apply_governance_submit_tx(&mut db, &tx2, 101);

    match result2 {
        Ok(id2) => {
            if id2 == proposal_id_1 {
                println!("FINDING: Resubmission overwrites existing proposal");
                println!("SECURITY IMPLICATION: Attacker can reset proposal state");
            } else {
                println!("FINDING: Different submission produces different ID");
            }
        }
        Err(e) => {
            println!("FINDING: Resubmission rejected with: {e:?}");
            println!("SECURITY IMPLICATION: Duplicate proposals prevented");
        }
    }
}

// ============================================================================
// A25.2.2: APPROVAL STATE AFTER RESUBMISSION
// ============================================================================

/// Test what happens to approval state when a proposal is resubmitted.
#[test]
fn attack_resubmit_to_reset_timing() {
    println!("=== A25.2.2 RESUBMISSION TIMING RESET ===");

    let mut db = MemKv::new();

    let entity = create_test_entity(b"resubmit_test", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    let gate = create_timelock_only_gate(50, 200);
    store_gate(&mut db, &gate);

    let data = b"malicious_action".to_vec();

    // Submit proposal at height 100
    let payload =
        create_submit_payload(ProposalType::ParamChange, timelock_gate_id(), data.clone());
    let tx1 = create_tx(entity_id, 0, 100, payload);
    let result = apply_governance_submit_tx(&mut db, &tx1, 100);
    assert!(result.is_ok());
    let proposal_id = result.unwrap();

    // Read proposal - should be approved (TimelockOnly)
    let proposal = read_proposal(&db, &proposal_id).unwrap().unwrap();
    println!("Initial state: {:?}", proposal.state);
    println!("Approved at: {}", proposal.approved_at);
    println!("Executable at: {}", proposal.executable_at);

    let original_executable_at = proposal.executable_at;

    // Now "resubmit" at a later height - does it change approval timing?
    let payload2 = create_submit_payload(ProposalType::ParamChange, timelock_gate_id(), data);
    let tx2 = create_tx(entity_id, 1, 100, payload2);
    let result2 = apply_governance_submit_tx(&mut db, &tx2, 120);

    match result2 {
        Ok(new_id) => {
            let new_proposal = read_proposal(&db, &new_id).unwrap().unwrap();
            if new_id == proposal_id {
                // Same ID - proposal was overwritten
                if new_proposal.executable_at > original_executable_at {
                    println!("WARNING: Resubmission RESET approval timing!");
                    println!("Original executable_at: {original_executable_at}");
                    println!("New executable_at: {}", new_proposal.executable_at);
                    println!("ATTACK VECTOR: Can delay legitimate proposals");
                } else if new_proposal.executable_at == original_executable_at {
                    println!("FINDING: Original timing preserved on resubmission");
                }
            } else {
                println!("FINDING: Different IDs - no overwrite");
            }
        }
        Err(e) => {
            println!("FINDING: Resubmission prevented: {e:?}");
        }
    }
}

// ============================================================================
// A25.2.3: GATE SWITCHING ATTACK
// ============================================================================

/// Test if attacker can submit to a less restrictive gate.
#[test]
fn attack_gate_switching() {
    println!("=== A25.2.3 GATE SWITCHING ATTACK ===");

    let mut db = MemKv::new();

    let entity = create_test_entity(b"gate_switch_test", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    // Setup both gates
    let timelock_gate = create_timelock_only_gate(50, 200);
    store_gate(&mut db, &timelock_gate);

    let multisig_gate = create_multisig_gate(
        vec![approver_1(), approver_2()],
        2, // Need both approvals
        50,
        200,
    );
    store_gate(&mut db, &multisig_gate);

    let sensitive_action = b"modify_consensus_params".to_vec();

    // Legitimate submission to multisig gate (requires 2 approvals)
    let payload_multisig = create_submit_payload(
        ProposalType::ParamChange,
        multisig_gate_id(),
        sensitive_action.clone(),
    );
    let tx_multisig = create_tx(entity_id, 0, 100, payload_multisig);
    let result_multisig = apply_governance_submit_tx(&mut db, &tx_multisig, 100);
    assert!(result_multisig.is_ok());
    let multisig_proposal_id = result_multisig.unwrap();

    // Read the multisig proposal
    let multisig_proposal = read_proposal(&db, &multisig_proposal_id).unwrap().unwrap();
    println!("Multisig proposal state: {:?}", multisig_proposal.state);
    println!(
        "Multisig proposal gate: {:02x?}",
        &multisig_proposal.gate_id[..4]
    );

    // Attacker submits same action to TimelockOnly gate
    let payload_timelock = create_submit_payload(
        ProposalType::ParamChange,
        timelock_gate_id(),
        sensitive_action,
    );
    let tx_timelock = create_tx(entity_id, 1, 100, payload_timelock);
    let result_timelock = apply_governance_submit_tx(&mut db, &tx_timelock, 101);
    assert!(result_timelock.is_ok());
    let timelock_proposal_id = result_timelock.unwrap();

    // Compare proposal IDs
    println!("Multisig proposal ID: {:02x?}", &multisig_proposal_id[..8]);
    println!("Timelock proposal ID: {:02x?}", &timelock_proposal_id[..8]);

    if multisig_proposal_id == timelock_proposal_id {
        println!("FINDING: Gate switching blocked - same ID prevents duplicate");
    } else {
        println!("FINDING: Different gates produce different proposal IDs");
        println!("SECURITY: Gate ID is part of proposal ID computation");

        // But can both exist and be executed?
        let timelock_proposal = read_proposal(&db, &timelock_proposal_id).unwrap().unwrap();
        println!("Timelock proposal state: {:?}", timelock_proposal.state);

        if timelock_proposal.state == ProposalState::Approved {
            println!("WARNING: Same action submitted to multiple gates!");
            println!("TimelockOnly version is auto-approved");
            println!("RECOMMENDATION: Consider action-level uniqueness checks");
        }
    }
}

// ============================================================================
// A25.2.4: TIMELOCKONLY BYPASS OF MULTISIG
// ============================================================================

/// Test if TimelockOnly gates bypass normal approval requirements.
#[test]
fn attack_timelockonly_bypass() {
    println!("=== A25.2.4 TIMELOCKONLY BYPASS ATTACK ===");

    let mut db = MemKv::new();

    let entity = create_test_entity(b"bypass_test", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    let gate = create_timelock_only_gate(50, 200);
    store_gate(&mut db, &gate);

    // Attacker creates a "dangerous" proposal using TimelockOnly gate
    let dangerous_action = b"upgrade_consensus_to_pow".to_vec();
    let payload = create_submit_payload(
        ProposalType::ParamChange,
        timelock_gate_id(),
        dangerous_action,
    );
    let tx = create_tx(entity_id, 0, 100, payload);
    let result = apply_governance_submit_tx(&mut db, &tx, 100);
    assert!(result.is_ok());
    let proposal_id = result.unwrap();

    let proposal = read_proposal(&db, &proposal_id).unwrap().unwrap();
    println!("Dangerous action submitted via TimelockOnly gate");
    println!("State: {:?}", proposal.state);
    println!(
        "Auto-approved: {}",
        proposal.state == ProposalState::Approved
    );

    // Try to execute after timelock
    let exec_payload = create_execute_payload(proposal_id);
    let exec_tx = create_tx(entity_id, 1, 100, exec_payload);
    let result = apply_governance_execute_tx(&mut db, &exec_tx, 151);

    match result {
        Ok(()) => {
            println!("WARNING: Action executed without human approval!");
            println!("FINDING: TimelockOnly gates bypass approval requirements");
            println!("RECOMMENDATION: Restrict which actions can use TimelockOnly");
        }
        Err(e) => {
            println!("FINDING: Execution blocked: {e:?}");
        }
    }
}

// ============================================================================
// A25.2.5: CROSS-PROPOSAL APPROVAL MODEL
// ============================================================================

/// Document current approval model and potential confusion.
#[test]
fn document_approval_model() {
    println!("=== A25.2.5 APPROVAL MODEL DOCUMENTATION ===");

    let mut db = MemKv::new();

    let entity = create_test_entity(b"approval_model_test", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    // Create a multisig gate requiring 2 approvals
    let multisig_gate = create_multisig_gate(vec![approver_1(), approver_2()], 2, 50, 200);
    store_gate(&mut db, &multisig_gate);

    // Create two different proposals to the same gate
    let payload1 = create_submit_payload(
        ProposalType::ParamChange,
        multisig_gate_id(),
        b"action_A".to_vec(),
    );
    let payload2 = create_submit_payload(
        ProposalType::ParamChange,
        multisig_gate_id(),
        b"action_B".to_vec(),
    );

    let tx1 = create_tx(entity_id, 0, 100, payload1);
    let tx2 = create_tx(entity_id, 1, 100, payload2);

    let result1 = apply_governance_submit_tx(&mut db, &tx1, 100);
    let result2 = apply_governance_submit_tx(&mut db, &tx2, 101);

    assert!(result1.is_ok());
    assert!(result2.is_ok());

    let proposal_id_a = result1.unwrap();
    let proposal_id_b = result2.unwrap();

    let proposal_a = read_proposal(&db, &proposal_id_a).unwrap().unwrap();
    let proposal_b = read_proposal(&db, &proposal_id_b).unwrap().unwrap();

    println!(
        "Proposal A: {:02x?}, state: {:?}",
        &proposal_id_a[..8],
        proposal_a.state
    );
    println!(
        "Proposal B: {:02x?}, state: {:?}",
        &proposal_id_b[..8],
        proposal_b.state
    );

    println!();
    println!("CURRENT APPROVAL MODEL:");
    println!("  - Approvals stored as: Vec<Address> in proposal");
    println!("  - No cryptographic binding to proposal ID");
    println!("  - Approval tx type: NOT YET IMPLEMENTED");

    println!();
    println!("SECURITY IMPLICATIONS:");
    println!("  - When approval tx is added, must bind signature to proposal ID");
    println!("  - Must prevent signature replay across proposals");
    println!("  - Should include nonce/timestamp in signed message");
}

// ============================================================================
// A25.2.6: EXPIRED PROPOSAL RESUBMISSION
// ============================================================================

/// Test if an expired proposal can be resubmitted to reset the timer.
#[test]
fn attack_expired_proposal_resubmission() {
    println!("=== A25.2.6 EXPIRED PROPOSAL RESUBMISSION ===");

    let mut db = MemKv::new();

    let entity = create_test_entity(b"expired_resubmit_test", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    // Short expiry gate for testing
    let gate = create_timelock_only_gate(10, 50);
    store_gate(&mut db, &gate);

    let action = b"delayed_action".to_vec();

    // Submit at height 100
    let payload = create_submit_payload(
        ProposalType::ParamChange,
        timelock_gate_id(),
        action.clone(),
    );
    let tx = create_tx(entity_id, 0, 100, payload);
    let result = apply_governance_submit_tx(&mut db, &tx, 100);
    assert!(result.is_ok());
    let proposal_id = result.unwrap();

    let proposal = read_proposal(&db, &proposal_id).unwrap().unwrap();
    println!("Initial submission at height 100");
    println!("Expires at: {}", proposal.expires_at);

    // Let it expire (expiry is 50 blocks, so at height 151 it's expired)
    let exec_payload = create_execute_payload(proposal_id);
    let exec_tx = create_tx(entity_id, 1, 100, exec_payload);
    let expired_result = apply_governance_execute_tx(&mut db, &exec_tx, 160);

    match expired_result {
        Err(ExecError::ProposalExpired) => {
            println!("Confirmed: Proposal expired at height 160");
        }
        other => {
            println!("Unexpected result: {other:?}");
        }
    }

    // Can attacker resubmit to get a fresh expiry?
    let payload2 = create_submit_payload(ProposalType::ParamChange, timelock_gate_id(), action);
    let tx2 = create_tx(entity_id, 2, 100, payload2);
    let result2 = apply_governance_submit_tx(&mut db, &tx2, 170);

    match result2 {
        Ok(new_id) => {
            let new_proposal = read_proposal(&db, &new_id).unwrap().unwrap();
            if new_id == proposal_id {
                println!("FINDING: Resubmission OVERWRITES expired proposal");
                println!("New expires_at: {}", new_proposal.expires_at);
                println!("SECURITY: Attacker can resurrect expired proposals");
            } else {
                println!("FINDING: New submission produces different ID");
                println!("New proposal ID: {:02x?}", &new_id[..8]);
            }
        }
        Err(e) => {
            println!("FINDING: Resubmission blocked: {e:?}");
        }
    }
}

// ============================================================================
// A25.2.7: DOUBLE EXECUTION ATTACK
// ============================================================================

/// Test if an executed proposal can be executed again.
#[test]
fn attack_double_execution() {
    println!("=== A25.2.7 DOUBLE EXECUTION ATTACK ===");

    let mut db = MemKv::new();

    let entity = create_test_entity(b"double_exec_test", 10_000_000_000, true);
    let entity_id = entity.id;
    store_entity(&mut db, &entity);

    let gate = create_timelock_only_gate(10, 200);
    store_gate(&mut db, &gate);

    // Submit and execute
    let payload = create_submit_payload(
        ProposalType::ParamChange,
        timelock_gate_id(),
        b"one_time_action".to_vec(),
    );
    let tx = create_tx(entity_id, 0, 100, payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &tx, 100).unwrap();

    let exec_payload = create_execute_payload(proposal_id);
    let exec_tx = create_tx(entity_id, 1, 100, exec_payload.clone());
    let first_result = apply_governance_execute_tx(&mut db, &exec_tx, 111);
    assert!(first_result.is_ok(), "First execution should succeed");

    let proposal = read_proposal(&db, &proposal_id).unwrap().unwrap();
    println!("After first execution: {:?}", proposal.state);

    // ATTACK: Try to execute again
    let exec_tx2 = create_tx(entity_id, 2, 100, exec_payload);
    let second_result = apply_governance_execute_tx(&mut db, &exec_tx2, 112);

    match second_result {
        Err(ExecError::ProposalNotExecutable) => {
            println!("SECURE: Double execution prevented");
            println!("Error: ProposalNotExecutable (already executed)");
        }
        Err(e) => {
            println!("SECURE: Double execution prevented with: {e:?}");
        }
        Ok(()) => {
            println!("VULNERABILITY: Double execution succeeded!");
            println!("CRITICAL: Proposals can be executed multiple times");
        }
    }
}

// ============================================================================
// SUMMARY
// ============================================================================

#[test]
fn approval_replay_summary() {
    println!("=============================================================");
    println!("           A25.2 APPROVAL REPLAY ATTACK SUMMARY");
    println!("=============================================================");
    println!();
    println!("CURRENT APPROVAL MODEL:");
    println!("  - TimelockOnly gates: Auto-approved at submission");
    println!("  - Multisig/Threshold: Require separate approval tx (NOT IMPL)");
    println!("  - Approvals stored as: Vec<Address> (no signatures)");
    println!();
    println!("ATTACK VECTORS TESTED:");
    println!("  1. Proposal ID determinism - same content = same ID");
    println!("  2. Resubmission to reset timing");
    println!("  3. Gate switching to bypass approval requirements");
    println!("  4. TimelockOnly bypass of normal approval flow");
    println!("  5. Cross-proposal approval model");
    println!("  6. Expired proposal resubmission");
    println!("  7. Double execution attack");
    println!();
    println!("HARDENING RECOMMENDATIONS (when implementing approval tx):");
    println!("  1. Bind approvals cryptographically to proposal ID");
    println!("  2. Include nonce in signed approval message");
    println!("  3. Prevent resubmission of existing proposal IDs");
    println!("  4. Restrict dangerous actions from TimelockOnly gates");
    println!("  5. Consider action-level uniqueness (not just proposal ID)");
    println!("=============================================================");
}
