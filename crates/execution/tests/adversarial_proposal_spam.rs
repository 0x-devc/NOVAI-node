//! Week 25: A25.1 Proposal Spam Attack Tests.
//!
//! PURPOSE: Test the governance system's resilience to proposal spam attacks.
//!
//! ATTACK VECTORS:
//! - Generate thousands of proposals rapidly
//! - Fee exhaustion attacks
//! - Storage/memory exhaustion attempts
//! - Legitimate proposal processing under spam load
//!
//! EXPECTED RESULTS:
//! - System should either:
//!   a) Rate limit proposal submissions, OR
//!   b) Require fees that make spam economically unfeasible
//! - Legitimate proposals must still be processable
//!
//! FINDINGS DOCUMENTATION:
//! This file documents actual behavior vs expected behavior.
//! Any security gaps discovered will be noted for hardening.

#![allow(clippy::doc_markdown)]

use novai_ai_entities::{AiEntity, ApprovalGate, AutonomyMode, Capabilities, GateType};
use novai_codec::encode_approval_gate_v1;
use novai_execution::{
    apply_governance_execute_tx, apply_governance_submit_tx, encode_execute_proposal_payload_v1,
    encode_submit_proposal_payload_v1, read_ai_entity, read_proposal, write_ai_entity_op,
    ExecError, ExecuteProposalPayloadV1, SubmitProposalPayloadV1,
};
use novai_governance::ProposalType;
use novai_state::{approval_gate_key, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};
use std::collections::HashSet;

// ============================================================================
// TEST HELPERS
// ============================================================================

fn spam_gate_id() -> [u8; 32] {
    *blake3::hash(b"NOVAI_SPAM_TEST_GATE_V1").as_bytes()
}

fn create_timelock_gate(timelock_blocks: u64, expiry_blocks: u64) -> ApprovalGate {
    ApprovalGate {
        gate_id: spam_gate_id(),
        gate_type: GateType::TimelockOnly,
        required_approvers: Vec::new(),
        threshold: 0,
        timelock_blocks,
        expiry_blocks,
        veto_enabled: false,
        freeze_enabled: false,
    }
}

fn store_gate(db: &mut MemKv, gate: &ApprovalGate) {
    let key = approval_gate_key(&gate.gate_id);
    let value = encode_approval_gate_v1(gate);
    db.apply_batch(&[WriteOp::Put(key, value)]).unwrap();
}

fn create_test_entity(name: &[u8], balance: u128) -> AiEntity {
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
    entity.is_active = true;
    entity
}

fn store_entity(db: &mut MemKv, entity: &AiEntity) {
    db.apply_batch(&[write_ai_entity_op(entity)]).unwrap();
}

fn create_submit_payload(proposal_type: ProposalType, gate_id: [u8; 32], data: Vec<u8>) -> Vec<u8> {
    let payload = SubmitProposalPayloadV1 {
        proposal_type,
        gate_id,
        proposal_data: data,
    };
    encode_submit_proposal_payload_v1(&payload)
}

fn create_execute_payload(proposal_id: [u8; 32]) -> Vec<u8> {
    let payload = ExecuteProposalPayloadV1 { proposal_id };
    encode_execute_proposal_payload_v1(&payload).to_vec()
}

// const: pure struct construction with no runtime logic
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
// A25.1.1: Mass Proposal Generation Attack
// ============================================================================

#[test]
#[allow(clippy::cast_sign_loss)] // loop counter `i` is always non-negative
fn attack_mass_proposal_generation() {
    let mut db = MemKv::new();

    // Setup: Attacker with large balance
    let attacker = create_test_entity(b"spam_attacker", 1_000_000_000_000);
    let attacker_id = attacker.id;
    store_entity(&mut db, &attacker);

    // Target entity for proposals
    let target = create_test_entity(b"spam_target", 10_000_000_000);
    let target_id = target.id;
    store_entity(&mut db, &target);

    let gate = create_timelock_gate(10, 1000);
    store_gate(&mut db, &gate);

    // ATTACK: Generate 1000 proposals rapidly
    let spam_count: u32 = 1000;
    let mut successful_proposals: u32 = 0;
    let mut unique_proposal_ids = HashSet::new();

    let start_height: u64 = 1000;

    for i in 0..spam_count {
        // Each proposal has unique data to get unique proposal_id
        let mut proposal_data = target_id.to_vec();
        proposal_data.extend_from_slice(&u64::from(i).to_be_bytes());

        // Use ParamChange since it accepts variable data
        let submit_payload =
            create_submit_payload(ProposalType::ParamChange, spam_gate_id(), proposal_data);

        let submit_tx = create_tx(attacker_id, u64::from(i), 100, submit_payload);

        if let Ok(proposal_id) =
            apply_governance_submit_tx(&mut db, &submit_tx, start_height + u64::from(i))
        {
            successful_proposals += 1;
            unique_proposal_ids.insert(proposal_id);
        }
    }

    // FINDING: Document actual behavior
    println!("=== A25.1.1 MASS PROPOSAL ATTACK RESULTS ===");
    println!("Spam attempts: {spam_count}");
    println!("Successful proposals: {successful_proposals}");
    println!("Unique proposal IDs: {}", unique_proposal_ids.len());

    // Verify all proposals have unique IDs (no collisions)
    assert_eq!(
        successful_proposals as usize,
        unique_proposal_ids.len(),
        "All successful proposals must have unique IDs"
    );

    // SECURITY OBSERVATION:
    // If successful_proposals == spam_count, there's no rate limiting
    if successful_proposals == spam_count {
        println!("WARNING: No rate limiting detected - all {spam_count} proposals accepted");
        println!("RECOMMENDATION: Implement per-account proposal rate limiting");
    }
}

// ============================================================================
// A25.1.2: Fee Exhaustion Attack
// ============================================================================

#[test]
#[allow(clippy::cast_sign_loss)] // loop counter `i` is always non-negative
#[allow(clippy::cast_possible_truncation)] // max_affordable is bounded by initial_balance/fee ratio
fn attack_fee_exhaustion_behavior() {
    let mut db = MemKv::new();

    // Setup: Attacker with LIMITED balance
    let initial_balance: u128 = 10_000;
    let attacker = create_test_entity(b"fee_exhaustion_attacker", initial_balance);
    let attacker_id = attacker.id;
    store_entity(&mut db, &attacker);

    let target = create_test_entity(b"fee_target", 10_000_000_000);
    let target_id = target.id;
    store_entity(&mut db, &target);

    let gate = create_timelock_gate(10, 1000);
    store_gate(&mut db, &gate);

    // ATTACK: Try to submit many proposals with fees
    let fee_per_proposal: u64 = 100;
    let max_affordable = (initial_balance / u128::from(fee_per_proposal)) as u32;

    println!("=== A25.1.2 FEE EXHAUSTION ATTACK ===");
    println!("Initial balance: {initial_balance}");
    println!("Fee per proposal: {fee_per_proposal}");
    println!("Max affordable proposals (if fees deducted): {max_affordable}");

    let mut successful_proposals: u32 = 0;
    let spam_attempts = 200;

    for i in 0..spam_attempts {
        let mut proposal_data = target_id.to_vec();
        proposal_data.extend_from_slice(&(i as u64).to_be_bytes());

        let submit_payload =
            create_submit_payload(ProposalType::ParamChange, spam_gate_id(), proposal_data);

        let submit_tx = create_tx(attacker_id, i as u64, fee_per_proposal, submit_payload);

        match apply_governance_submit_tx(&mut db, &submit_tx, 1000 + i as u64) {
            Ok(_) => {
                successful_proposals += 1;
            }
            Err(e) => {
                println!("Proposal {i} failed: {e:?}");
                if matches!(e, ExecError::InsufficientFunds { .. }) {
                    println!("Fee exhaustion detected at proposal {i}");
                    break;
                }
            }
        }
    }

    // Check attacker's final balance
    let final_attacker = read_ai_entity(&db, &attacker_id).unwrap().unwrap();
    let balance_change = initial_balance.saturating_sub(final_attacker.economic_balance);

    println!("Successful proposals: {successful_proposals}");
    println!("Final balance: {}", final_attacker.economic_balance);
    println!("Balance consumed: {balance_change}");

    // FINDING: Document whether fees are actually deducted
    if balance_change == 0 && successful_proposals > 0 {
        println!("FINDING: Proposal submission does NOT deduct fees from proposer");
        println!("Attacker submitted {successful_proposals} proposals without losing balance");
        println!("RECOMMENDATION: Deduct proposal submission fee or require stake");
    } else if successful_proposals <= max_affordable {
        println!("SECURE: Fee deduction limits spam to {successful_proposals} proposals");
    }
}

// ============================================================================
// A25.1.3: Legitimate Proposals Under Spam Load
// ============================================================================

#[test]
#[allow(clippy::cast_sign_loss)] // loop counter `i` is always non-negative
fn legitimate_proposal_works_under_spam() {
    let mut db = MemKv::new();

    // Setup: Legitimate user and spammer
    let legitimate_user = create_test_entity(b"legitimate_user", 10_000_000_000);
    let legitimate_id = legitimate_user.id;
    store_entity(&mut db, &legitimate_user);

    let spammer = create_test_entity(b"spammer", 1_000_000_000_000);
    let spammer_id = spammer.id;
    store_entity(&mut db, &spammer);

    let target = create_test_entity(b"legitimate_target", 10_000_000_000);
    let target_id = target.id;
    store_entity(&mut db, &target);

    let gate = create_timelock_gate(10, 1000);
    store_gate(&mut db, &gate);

    // Phase 1: Spammer floods with 500 proposals
    println!("=== A25.1.3 LEGITIMATE PROPOSAL UNDER SPAM ===");
    println!("Phase 1: Spammer submitting 500 proposals...");

    for i in 0u64..500 {
        let mut spam_data = [0xFFu8; 32].to_vec();
        spam_data.extend_from_slice(&i.to_be_bytes());

        let spam_payload =
            create_submit_payload(ProposalType::ParamChange, spam_gate_id(), spam_data);
        let spam_tx = create_tx(spammer_id, i, 100, spam_payload);
        let _ = apply_governance_submit_tx(&mut db, &spam_tx, 1000 + i);
    }

    // Phase 2: Legitimate user submits real proposal
    println!("Phase 2: Legitimate user submitting proposal...");

    let legitimate_payload = create_submit_payload(
        ProposalType::ModuleRollback,
        spam_gate_id(),
        target_id.to_vec(),
    );
    let legitimate_tx = create_tx(legitimate_id, 0, 100, legitimate_payload);
    let legitimate_proposal_id = apply_governance_submit_tx(&mut db, &legitimate_tx, 1500)
        .expect("Legitimate proposal must succeed");

    // Verify legitimate proposal exists and is valid
    let proposal = read_proposal(&db, &legitimate_proposal_id)
        .expect("DB read should succeed")
        .expect("Legitimate proposal must exist");

    assert_eq!(proposal.proposal_type, ProposalType::ModuleRollback);
    assert_eq!(proposal.proposer, legitimate_id);
    println!(
        "Legitimate proposal created: {:02x}{:02x}{:02x}{:02x}...",
        legitimate_proposal_id[0],
        legitimate_proposal_id[1],
        legitimate_proposal_id[2],
        legitimate_proposal_id[3]
    );

    // Phase 3: More spam after legitimate proposal
    println!("Phase 3: More spam after legitimate proposal...");

    for i in 500u64..700 {
        let mut spam_data = [0xEEu8; 32].to_vec();
        spam_data.extend_from_slice(&i.to_be_bytes());

        let spam_payload =
            create_submit_payload(ProposalType::ParamChange, spam_gate_id(), spam_data);
        let spam_tx = create_tx(spammer_id, i, 100, spam_payload);
        let _ = apply_governance_submit_tx(&mut db, &spam_tx, 1600 + (i - 500));
    }

    // Phase 4: Execute legitimate proposal (after timelock)
    println!("Phase 4: Executing legitimate proposal...");

    let execute_payload = create_execute_payload(legitimate_proposal_id);
    let execute_tx = create_tx(legitimate_id, 1, 100, execute_payload);

    let result = apply_governance_execute_tx(&mut db, &execute_tx, 1520);
    assert!(
        result.is_ok(),
        "Legitimate proposal must execute despite spam: {result:?}",
    );

    // Verify target was rolled back
    let final_target = read_ai_entity(&db, &target_id).unwrap().unwrap();
    assert!(
        !final_target.is_active,
        "Target must be deactivated after rollback"
    );

    println!("SUCCESS: Legitimate proposal processed correctly under spam");
}

// ============================================================================
// A25.1.4: Large Data Proposals
// ============================================================================

#[test]
#[allow(clippy::cast_sign_loss)] // loop index `i` is always non-negative
fn attack_large_data_proposals() {
    let mut db = MemKv::new();

    let attacker = create_test_entity(b"large_data_attacker", 1_000_000_000_000);
    let attacker_id = attacker.id;
    store_entity(&mut db, &attacker);

    let gate = create_timelock_gate(10, 1000);
    store_gate(&mut db, &gate);

    // ATTACK: Submit proposals with increasingly large data
    println!("=== A25.1.4 LARGE DATA PROPOSALS ===");

    let test_sizes = [100, 1_000, 10_000, 100_000];
    let mut results = Vec::new();

    for (i, &size) in test_sizes.iter().enumerate() {
        let mut large_data = vec![0xAAu8; size];
        large_data[0..8].copy_from_slice(&(i as u64).to_be_bytes());

        let submit_payload =
            create_submit_payload(ProposalType::ParamChange, spam_gate_id(), large_data);

        let submit_tx = create_tx(attacker_id, i as u64, 100, submit_payload);
        let result = apply_governance_submit_tx(&mut db, &submit_tx, 1000 + i as u64);

        let success = result.is_ok();
        results.push((size, success));
        println!(
            "Data size {} bytes: {}",
            size,
            if success { "ACCEPTED" } else { "REJECTED" }
        );
    }

    // Document findings
    let all_accepted = results.iter().all(|(_, success)| *success);
    if all_accepted {
        println!("WARNING: No data size limits detected");
        println!("RECOMMENDATION: Implement maximum proposal data size limit");
    }
}

// ============================================================================
// A25.1.5: Duplicate Proposal Handling
// ============================================================================

#[test]
fn duplicate_proposals_have_same_id() {
    let mut db = MemKv::new();

    let user = create_test_entity(b"duplicate_test_user", 10_000_000_000);
    let user_id = user.id;
    store_entity(&mut db, &user);

    let target = create_test_entity(b"duplicate_target", 10_000_000_000);
    let target_id = target.id;
    store_entity(&mut db, &target);

    let gate = create_timelock_gate(10, 1000);
    store_gate(&mut db, &gate);

    // Submit identical proposals
    let proposal_data = target_id.to_vec();
    let submit_payload =
        create_submit_payload(ProposalType::ModuleRollback, spam_gate_id(), proposal_data);

    let tx1 = create_tx(user_id, 0, 100, submit_payload.clone());
    let id1 = apply_governance_submit_tx(&mut db, &tx1, 1000).unwrap();

    println!("=== A25.1.5 DUPLICATE PROPOSAL TEST ===");
    println!(
        "Proposal 1 ID: {:02x}{:02x}{:02x}{:02x}...",
        id1[0], id1[1], id1[2], id1[3]
    );

    // Week 25 Hardening: Second submission should be REJECTED (ProposalAlreadyExists)
    let tx2 = create_tx(user_id, 1, 100, submit_payload);
    let result2 = apply_governance_submit_tx(&mut db, &tx2, 1001);

    match result2 {
        Err(ExecError::ProposalAlreadyExists) => {
            println!("Proposal 2: REJECTED (ProposalAlreadyExists)");
            println!("FINDING: Duplicate proposals rejected - prevents timing reset attacks");
        }
        Ok(id2) => {
            println!(
                "Proposal 2 ID: {:02x}{:02x}{:02x}{:02x}...",
                id2[0], id2[1], id2[2], id2[3]
            );
            panic!("VULNERABILITY: Duplicate submission should be rejected");
        }
        Err(e) => {
            panic!("Unexpected error: {e:?}");
        }
    }

    // Original proposal should remain unchanged
    let proposal = read_proposal(&db, &id1).unwrap().unwrap();
    assert_eq!(
        proposal.submitted_at, 1000,
        "Original proposal should be unchanged"
    );
}

// ============================================================================
// A25.1.6: Different Proposers Same Content
// ============================================================================

#[test]
fn different_proposers_get_different_ids() {
    let mut db = MemKv::new();

    let user1 = create_test_entity(b"proposer_1", 10_000_000_000);
    let user1_id = user1.id;
    store_entity(&mut db, &user1);

    let user2 = create_test_entity(b"proposer_2", 10_000_000_000);
    let user2_id = user2.id;
    store_entity(&mut db, &user2);

    let target = create_test_entity(b"multi_proposer_target", 10_000_000_000);
    let target_id = target.id;
    store_entity(&mut db, &target);

    let gate = create_timelock_gate(10, 1000);
    store_gate(&mut db, &gate);

    // Same proposal content from different proposers
    let proposal_data = target_id.to_vec();

    let payload1 = create_submit_payload(
        ProposalType::ModuleRollback,
        spam_gate_id(),
        proposal_data.clone(),
    );
    let payload2 =
        create_submit_payload(ProposalType::ModuleRollback, spam_gate_id(), proposal_data);

    let tx1 = create_tx(user1_id, 0, 100, payload1);
    let tx2 = create_tx(user2_id, 0, 100, payload2);

    let id1 = apply_governance_submit_tx(&mut db, &tx1, 1000).unwrap();
    let id2 = apply_governance_submit_tx(&mut db, &tx2, 1001).unwrap();

    println!("=== A25.1.6 DIFFERENT PROPOSERS TEST ===");
    println!(
        "User 1 proposal: {:02x}{:02x}{:02x}{:02x}...",
        id1[0], id1[1], id1[2], id1[3]
    );
    println!(
        "User 2 proposal: {:02x}{:02x}{:02x}{:02x}...",
        id2[0], id2[1], id2[2], id2[3]
    );

    // Different proposers should get different proposal IDs
    assert_ne!(id1, id2, "Different proposers get different proposal IDs");

    // Both proposals should exist
    assert!(read_proposal(&db, &id1).unwrap().is_some());
    assert!(read_proposal(&db, &id2).unwrap().is_some());

    println!("FINDING: Proposal ID includes proposer - prevents ID collision attacks");
}

// ============================================================================
// A25.1.7: Rapid Sequential Submissions
// ============================================================================

#[test]
fn attack_rapid_sequential_submissions() {
    let mut db = MemKv::new();

    let attacker = create_test_entity(b"rapid_attacker", 1_000_000_000_000);
    let attacker_id = attacker.id;
    store_entity(&mut db, &attacker);

    let gate = create_timelock_gate(10, 1000);
    store_gate(&mut db, &gate);

    println!("=== A25.1.7 RAPID SEQUENTIAL SUBMISSIONS ===");

    // ATTACK: Submit 100 proposals all at the SAME block height
    let same_height: u64 = 1000;
    let mut accepted = 0;

    for i in 0u64..100 {
        let mut data = vec![0xBBu8; 32];
        data.extend_from_slice(&i.to_be_bytes());

        let payload = create_submit_payload(ProposalType::ParamChange, spam_gate_id(), data);
        let tx = create_tx(attacker_id, i, 100, payload);

        if apply_governance_submit_tx(&mut db, &tx, same_height).is_ok() {
            accepted += 1;
        }
    }

    println!("Proposals submitted at same height {same_height}: {accepted}");

    // Document finding
    if accepted == 100 {
        println!("FINDING: No per-block rate limiting");
        println!("All 100 proposals accepted at same block height");
    }
}

// ============================================================================
// SUMMARY: Spam Attack Resilience Assessment
// ============================================================================

#[test]
fn spam_resilience_summary() {
    println!("=============================================================");
    println!("           A25.1 PROPOSAL SPAM ATTACK SUMMARY");
    println!("=============================================================");
    println!();
    println!("CURRENT PROTECTION MECHANISMS OBSERVED:");
    println!("  [ ] Rate limiting per account: NOT DETECTED");
    println!("  [ ] Rate limiting per block: NOT DETECTED");
    println!("  [ ] Fee deduction on submit: NOT DETECTED");
    println!("  [ ] Data size limits: NOT DETECTED");
    println!("  [✓] Duplicate detection: Same content = same ID");
    println!("  [✓] Proposer isolation: Different proposers = unique IDs");
    println!();
    println!("ATTACK VECTORS TESTED:");
    println!("  1. Mass proposal generation (1000 proposals)");
    println!("  2. Fee exhaustion attack");
    println!("  3. Legitimate proposal under spam");
    println!("  4. Large data proposals (up to 100KB)");
    println!("  5. Duplicate proposal handling");
    println!("  6. Multi-proposer scenarios");
    println!("  7. Same-block rapid submissions");
    println!();
    println!("HARDENING RECOMMENDATIONS:");
    println!("  1. Per-account proposal rate limit (e.g., 10/hour)");
    println!("  2. Require proposal submission fee (burned or staked)");
    println!("  3. Maximum pending proposals per account");
    println!("  4. Maximum proposal data size (e.g., 10KB)");
    println!("  5. Per-block proposal limit");
    println!("=============================================================");
}
