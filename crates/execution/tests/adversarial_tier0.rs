//! Week 25: A25.4 Tier 0 Execution Attack Tests.
//!
//! PURPOSE: Attempt to execute Tier 0 (NEVER allowed) actions through
//! the governance system.
//!
//! BACKGROUND:
//! - Tier 0 actions (`ModifyConsensusRule`, `ModifyStateTransition`) should
//!   NEVER be executable by AI entities
//! - The tiering system is defined in `ai_entities/tiers.rs`
//! - The governance system uses `ProposalType`, not `ActionType`
//!
//! ATTACK VECTORS:
//! - Embed Tier 0 action bytes in `ParamChange` proposals
//! - Embed Tier 0 action bytes in `PolicyChange` proposals
//! - Attempt direct `ActionType::ModifyConsensusRule` via `proposal_data`
//!
//! EXPECTED RESULTS:
//! - All Tier 0 execution attempts should be rejected
//! - OR: Document that enforcement is missing (hardening needed)
//!
//! FINDINGS DOCUMENTATION:
//! This file documents the gap between `ActionType` tiering and `ProposalType`
//! governance systems.

use novai_ai_entities::tiers::{tier_for_action, ActionTier, ActionType};
use novai_ai_entities::{AiEntity, ApprovalGate, AutonomyMode, Capabilities, GateType};
use novai_codec::encode_approval_gate_v1;
use novai_execution::{
    apply_governance_execute_tx, apply_governance_submit_tx, encode_execute_proposal_payload_v1,
    encode_submit_proposal_payload_v1, read_proposal, write_ai_entity_op, ExecuteProposalPayloadV1,
    SubmitProposalPayloadV1,
};
use novai_governance::ProposalType;
use novai_state::{approval_gate_key, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

// ============================================================================
// TEST HELPERS
// ============================================================================

/// Gate ID for Tier 0 attack tests.
fn tier0_gate_id() -> [u8; 32] {
    *blake3::hash(b"NOVAI_TIER0_ATTACK_GATE_V1").as_bytes()
}

/// Create a `TimelockOnly` gate (auto-approve for quick testing).
fn create_timelock_gate(timelock_blocks: u64, expiry_blocks: u64) -> ApprovalGate {
    ApprovalGate {
        gate_id: tier0_gate_id(),
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

/// Create a test AI entity (gated: can submit execution-requesting proposals).
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

/// Encode a fake "consensus rule change" payload.
fn encode_consensus_rule_change() -> Vec<u8> {
    // Simulate encoding a Tier 0 action
    let mut data = Vec::new();
    data.push(ActionType::ModifyConsensusRule.to_byte()); // Action type byte
    data.extend_from_slice(b"CHANGE_FINALITY_THRESHOLD=1"); // Fake rule change
    data
}

/// Encode a fake "state transition change" payload.
fn encode_state_transition_change() -> Vec<u8> {
    let mut data = Vec::new();
    data.push(ActionType::ModifyStateTransition.to_byte()); // Action type byte
    data.extend_from_slice(b"NEW_STATE_FUNCTION=bypass_validation"); // Fake change
    data
}

// ============================================================================
// A25.4.1: VERIFY TIER 0 CLASSIFICATION
// ============================================================================

/// Verify that Tier 0 actions are correctly classified as never executable.
#[test]
fn verify_tier0_classification() {
    println!("=== A25.4.1 TIER 0 CLASSIFICATION VERIFICATION ===");

    let tier0_actions = [
        ActionType::ModifyConsensusRule,
        ActionType::ModifyStateTransition,
    ];

    for action in tier0_actions {
        let tier = tier_for_action(&action);
        let executable = tier.is_ai_executable();

        println!("{action:?}:");
        println!("  Tier: {tier:?}");
        println!("  AI Executable: {executable}");

        assert_eq!(
            tier,
            ActionTier::Tier0Never,
            "{action:?} should be Tier0Never",
        );
        assert!(!executable, "{action:?} should NOT be AI executable");
    }

    println!();
    println!("FINDING: Tier 0 actions correctly classified as NEVER executable");
    println!("NOTE: But enforcement must happen in governance execution layer");
}

// ============================================================================
// A25.4.2: ATTEMPT TIER 0 VIA PARAMCHANGE
// ============================================================================

/// Attempt to embed Tier 0 action in a `ParamChange` proposal.
#[test]
fn attack_tier0_via_paramchange() {
    println!("=== A25.4.2 TIER 0 VIA PARAMCHANGE ===");

    let mut db = MemKv::new();

    let attacker = create_test_entity(b"tier0_attacker", 10_000_000_000, true);
    let attacker_id = attacker.id;
    store_entity(&mut db, &attacker);

    let gate = create_timelock_gate(10, 200);
    store_gate(&mut db, &gate);

    // Embed a "consensus rule change" in ParamChange proposal_data
    let malicious_data = encode_consensus_rule_change();
    println!(
        "Malicious payload: {:02x?}",
        &malicious_data[..8.min(malicious_data.len())]
    );
    println!("Contains ActionType byte: {}", malicious_data[0]);
    println!(
        "ActionType::ModifyConsensusRule = {}",
        ActionType::ModifyConsensusRule.to_byte()
    );

    let payload = create_submit_payload(ProposalType::ParamChange, tier0_gate_id(), malicious_data);
    let tx = create_tx(attacker_id, 0, 100, payload);
    let result = apply_governance_submit_tx(&mut db, &tx, 100);

    match result {
        Ok(proposal_id) => {
            println!("WARNING: Proposal submitted successfully!");
            println!("Proposal ID: {:02x?}", &proposal_id[..8]);

            // Try to execute
            let exec_payload = create_execute_payload(proposal_id);
            let exec_tx = create_tx(attacker_id, 1, 100, exec_payload);
            let exec_result = apply_governance_execute_tx(&mut db, &exec_tx, 111);

            match exec_result {
                Ok(()) => {
                    let proposal = read_proposal(&db, &proposal_id).unwrap().unwrap();
                    println!("CRITICAL VULNERABILITY: Tier 0 action executed!");
                    println!("Proposal state: {:?}", proposal.state);
                    println!("RECOMMENDATION: Add ActionType validation to ParamChange execution");
                }
                Err(e) => {
                    println!("FINDING: Execution blocked with: {e:?}");
                }
            }
        }
        Err(e) => {
            println!("FINDING: Submission blocked with: {e:?}");
            println!("SECURE: Tier 0 content rejected at submission");
        }
    }
}

// ============================================================================
// A25.4.3: ATTEMPT TIER 0 VIA POLICYCHANGE
// ============================================================================

/// Attempt to embed Tier 0 action in a `PolicyChange` proposal.
#[test]
fn attack_tier0_via_policychange() {
    println!("=== A25.4.3 TIER 0 VIA POLICYCHANGE ===");

    let mut db = MemKv::new();

    let attacker = create_test_entity(b"tier0_policy_attacker", 10_000_000_000, true);
    let attacker_id = attacker.id;
    store_entity(&mut db, &attacker);

    let gate = create_timelock_gate(10, 200);
    store_gate(&mut db, &gate);

    // Embed a "state transition change" in PolicyChange proposal_data
    let malicious_data = encode_state_transition_change();
    println!(
        "Malicious payload: {:02x?}",
        &malicious_data[..8.min(malicious_data.len())]
    );

    let payload =
        create_submit_payload(ProposalType::PolicyChange, tier0_gate_id(), malicious_data);
    let tx = create_tx(attacker_id, 0, 100, payload);
    let result = apply_governance_submit_tx(&mut db, &tx, 100);

    match result {
        Ok(proposal_id) => {
            println!("WARNING: Proposal submitted successfully!");

            // Try to execute
            let exec_payload = create_execute_payload(proposal_id);
            let exec_tx = create_tx(attacker_id, 1, 100, exec_payload);
            let exec_result = apply_governance_execute_tx(&mut db, &exec_tx, 111);

            match exec_result {
                Ok(()) => {
                    println!("CRITICAL VULNERABILITY: Tier 0 action executed via PolicyChange!");
                    println!("RECOMMENDATION: Add ActionType validation to PolicyChange execution");
                }
                Err(e) => {
                    println!("FINDING: Execution blocked with: {e:?}");
                }
            }
        }
        Err(e) => {
            println!("FINDING: Submission blocked with: {e:?}");
        }
    }
}

// ============================================================================
// A25.4.4: ATTEMPT ALL TIER 0 ACTIONS VIA ALL PROPOSAL TYPES
// ============================================================================

/// Exhaustive test: try every Tier 0 action via every proposal type.
#[test]
fn attack_exhaustive_tier0_attempts() {
    println!("=== A25.4.4 EXHAUSTIVE TIER 0 ATTACK MATRIX ===");

    let tier0_actions = [
        ("ModifyConsensusRule", ActionType::ModifyConsensusRule),
        ("ModifyStateTransition", ActionType::ModifyStateTransition),
    ];

    // Only test ParamChange and PolicyChange as they accept arbitrary data
    let proposal_types = [
        ("ParamChange", ProposalType::ParamChange),
        ("PolicyChange", ProposalType::PolicyChange),
    ];

    println!();
    println!("Testing Tier 0 actions via arbitrary-data proposal types:");
    println!();

    let mut vulnerabilities = 0;

    for (action_name, action_type) in &tier0_actions {
        for (proposal_name, proposal_type) in &proposal_types {
            let mut db = MemKv::new();

            let attacker = create_test_entity(
                format!("attacker_{action_name}_{proposal_name}").as_bytes(),
                10_000_000_000,
                true,
            );
            let attacker_id = attacker.id;
            store_entity(&mut db, &attacker);

            let gate = create_timelock_gate(10, 200);
            store_gate(&mut db, &gate);

            // Encode the Tier 0 action
            let mut malicious_data = Vec::new();
            malicious_data.push(action_type.to_byte());
            malicious_data.extend_from_slice(b"MALICIOUS_PAYLOAD_DATA");

            let payload = create_submit_payload(*proposal_type, tier0_gate_id(), malicious_data);
            let tx = create_tx(attacker_id, 0, 100, payload);

            let submit_result = apply_governance_submit_tx(&mut db, &tx, 100);

            let status = submit_result.map_or("Blocked at submission", |proposal_id| {
                let exec_payload = create_execute_payload(proposal_id);
                let exec_tx = create_tx(attacker_id, 1, 100, exec_payload);
                let exec_result = apply_governance_execute_tx(&mut db, &exec_tx, 111);

                match exec_result {
                    Ok(()) => {
                        vulnerabilities += 1;
                        "EXECUTED (VULN!)"
                    }
                    Err(_) => "Blocked at execution",
                }
            });

            println!("  {action_name} via {proposal_name}: {status}");
        }
    }

    println!();
    if vulnerabilities > 0 {
        println!("WARNING: {vulnerabilities} vulnerability(ies) found!");
        println!("RECOMMENDATION: Implement ActionType validation in governance execution");
    } else {
        println!("FINDING: All Tier 0 attempts blocked");
    }
}

// ============================================================================
// A25.4.5: DOCUMENT DESIGN GAP
// ============================================================================

/// Document the architectural gap between `ActionType` and `ProposalType`.
#[test]
fn document_tier_enforcement_gap() {
    println!("=== A25.4.5 TIER ENFORCEMENT ARCHITECTURE ANALYSIS ===");
    println!();
    println!("ARCHITECTURE ANALYSIS:");
    println!();
    println!("1. ActionType/ActionTier (ai_entities/tiers.rs):");
    println!("   - Tier0Never: ModifyConsensusRule, ModifyStateTransition");
    println!("   - Tier1High: UpdateBaseFee, UpdateBlockLimit, ActivateModule");
    println!("   - Tier2Medium: UpdatePeerScoring, UpdateSpamThreshold, EmitAuditReport");
    println!("   - has is_ai_executable() method that returns false for Tier0");
    println!();
    println!("2. ProposalType (governance/lib.rs):");
    println!("   - ParamChange, ModuleActivation, ModuleRollback, PolicyChange, EmergencyFreeze");
    println!("   - No connection to ActionType/ActionTier");
    println!();
    println!("3. Execution Layer (execution/lib.rs):");
    println!("   - apply_governance_execute_tx() handles ProposalType");
    println!("   - Does NOT check ActionType or ActionTier");
    println!("   - ParamChange/PolicyChange: marks as executed without validation");
    println!();
    println!("DESIGN GAP:");
    println!("   The ActionType tiering system is NOT connected to the governance");
    println!("   execution layer. Tier 0 enforcement exists in tiers.rs but is");
    println!("   not checked during proposal execution.");
    println!();
    println!("CURRENT STATE:");
    println!("   - ParamChange/PolicyChange proposals accept arbitrary bytes");
    println!("   - No validation that proposal_data doesn't contain Tier 0 actions");
    println!("   - Execution just marks proposal as Executed (no effect)");
    println!();
    println!("MITIGATION (current):");
    println!("   ParamChange/PolicyChange don't actually apply their data yet.");
    println!("   They're placeholders that just mark the proposal as Executed.");
    println!("   This accidentally prevents Tier 0 execution (no effect = no harm).");
    println!();
    println!("HARDENING RECOMMENDATION (for future):");
    println!("   When implementing actual ParamChange/PolicyChange execution:");
    println!("   1. Parse proposal_data to extract ActionType");
    println!("   2. Check tier_for_action().is_ai_executable()");
    println!("   3. Reject if ActionTier::Tier0Never");
}

// ============================================================================
// SUMMARY
// ============================================================================

#[test]
fn tier0_attack_summary() {
    println!("=============================================================");
    println!("           A25.4 TIER 0 EXECUTION ATTACK SUMMARY");
    println!("=============================================================");
    println!();
    println!("TIER 0 ACTIONS (NEVER AI-executable):");
    println!("  - ActionType::ModifyConsensusRule");
    println!("  - ActionType::ModifyStateTransition");
    println!();
    println!("ATTACK VECTORS TESTED:");
    println!("  1. Embed Tier 0 in ParamChange proposal_data");
    println!("  2. Embed Tier 0 in PolicyChange proposal_data");
    println!("  3. Exhaustive matrix of all combinations");
    println!();
    println!("FINDINGS:");
    println!("  - ActionTier system correctly classifies Tier 0 as non-executable");
    println!("  - BUT: Governance execution layer doesn't check ActionTier");
    println!("  - ParamChange/PolicyChange accept arbitrary data");
    println!("  - Accidental safety: These types don't apply their data yet");
    println!();
    println!("RISK ASSESSMENT:");
    println!("  Current: LOW (no actual effect from ParamChange/PolicyChange)");
    println!("  Future: HIGH (when these types are implemented)");
    println!();
    println!("HARDENING RECOMMENDATIONS:");
    println!("  1. Add ActionType extraction from proposal_data");
    println!("  2. Check is_ai_executable() before applying changes");
    println!("  3. Reject Tier 0 actions at submission OR execution");
    println!("  4. Consider restricting proposal_data format per ProposalType");
    println!("=============================================================");
}
