//! Week 24 Deliverable D24.5: Rollback Test
//!
//! This test demonstrates the complete rollback workflow:
//!
//!   activate → problem → rollback → deactivate
//!
//! ACCEPTANCE CRITERIA (D24.5):
//! 1. Module can be activated via governance proposal
//! 2. Active module can emit signals (normal operation)
//! 3. Problem detection triggers anomaly signal
//! 4. Rollback proposal can be submitted
//! 5. After timelock, rollback executes
//! 6. Module `is_active` = false after rollback
//!
//! This is the final verification that the AI module governance
//! lifecycle works end-to-end on testnet.

use novai_ai_entities::{
    AiEntity, AiSignalType, ApprovalGate, AutonomyMode, Capabilities, GateType,
};
use novai_codec::encode_approval_gate_v1;
use novai_execution::{
    apply_governance_execute_tx, apply_governance_submit_tx, apply_signal_commitment_tx,
    encode_execute_proposal_payload_v1, encode_signal_commitment_payload_v1,
    encode_submit_proposal_payload_v1, get_signals_by_type, read_ai_entity, write_ai_entity_op,
    ExecuteProposalPayloadV1, SignalCommitmentPayloadV1, SubmitProposalPayloadV1,
};
use novai_governance::ProposalType;
use novai_state::{ai_entity_by_address_key, approval_gate_key, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

// ============================================================================
// D24.5 ROLLBACK WORKFLOW TEST
// ============================================================================

/// D24.5: Complete rollback workflow demonstration
///
/// This test proves that the NOVAI protocol can:
/// 1. Activate AI modules through governance
/// 2. Detect problems via anomaly signals
/// 3. Roll back problematic modules through governance
/// 4. Deactivate modules to protect the network
#[test]
// Test covers the full activate→problem→rollback→deactivate lifecycle in a single flow.
#[allow(clippy::too_many_lines)]
fn d24_5_rollback_workflow_activate_problem_rollback_deactivate() {
    println!("\n=== D24.5 ROLLBACK WORKFLOW TEST ===\n");

    let mut db = MemKv::new();

    // ========================================================================
    // SETUP: Create third-party AI module and governance gate
    // ========================================================================

    println!("Step 0: Setup - Create third-party module and governance gate");

    // Create a third-party AI module (e.g., "PredictionBot v1.0")
    // Third-party modules start INACTIVE until approved via governance
    let module_code_hash = *blake3::hash(b"PREDICTION_BOT_V1").as_bytes();
    let module_creator = *blake3::hash(b"THIRD_PARTY_DEV").as_bytes();

    let mut module = AiEntity::new(
        module_code_hash,
        module_creator,
        AutonomyMode::Gated,
        Capabilities::gated(),
        0, // registered at genesis
    );
    module.economic_balance = 10_000_000_000; // 10B units for fees
    module.is_active = false; // CRITICAL: Third-party starts inactive
    let module_id = module.id;

    db.apply_batch(&[
        write_ai_entity_op(&module),
        WriteOp::Put(ai_entity_by_address_key(&module.id), module.id.to_vec()),
    ])
    .unwrap();

    // Create governance gate (TimelockOnly for testnet simplicity)
    let gate_id = *blake3::hash(b"TESTNET_GOVERNANCE_GATE").as_bytes();
    let gate = ApprovalGate {
        gate_id,
        gate_type: GateType::TimelockOnly,
        required_approvers: Vec::new(),
        threshold: 0,        // No approvers needed for TimelockOnly
        timelock_blocks: 10, // 10 blocks before execution
        expiry_blocks: 1000, // Proposals expire after 1000 blocks
        veto_enabled: false,
        freeze_enabled: false,
    };
    let gate_key = approval_gate_key(&gate.gate_id);
    let gate_value = encode_approval_gate_v1(&gate);
    db.apply_batch(&[WriteOp::Put(gate_key, gate_value)])
        .unwrap();

    // Governance account (could be multisig in production)
    let governance_account = *blake3::hash(b"GOVERNANCE_COUNCIL").as_bytes();

    // Verify: Module starts INACTIVE
    let loaded = read_ai_entity(&db, &module_id).unwrap().unwrap();
    assert!(!loaded.is_active, "SETUP CHECK: Module must start inactive");
    println!("  - Module registered: is_active = {}", loaded.is_active);
    println!(
        "  - Gate registered: timelock = {} blocks",
        gate.timelock_blocks
    );

    // ========================================================================
    // STEP 1: ACTIVATE - Submit and execute ModuleActivation proposal
    // ========================================================================

    println!("\nStep 1: ACTIVATE - Submit ModuleActivation proposal");

    let activate_payload = SubmitProposalPayloadV1 {
        proposal_type: ProposalType::ModuleActivation,
        gate_id,
        proposal_data: module_id.to_vec(),
    };
    let submit_tx = TxV1 {
        version: TxVersion::V1,
        from: governance_account,
        pubkey: governance_account,
        nonce: 0,
        fee: 100,
        payload: encode_submit_proposal_payload_v1(&activate_payload),
        sig: [0u8; 64],
    };

    let activation_proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 100).unwrap();
    println!("  - Proposal submitted at height 100");
    println!(
        "  - Proposal ID: {:02x}{:02x}...",
        activation_proposal_id[0], activation_proposal_id[1]
    );

    // Wait for timelock (10 blocks) then execute
    let execute_payload = ExecuteProposalPayloadV1 {
        proposal_id: activation_proposal_id,
    };
    let execute_tx = TxV1 {
        version: TxVersion::V1,
        from: governance_account,
        pubkey: governance_account,
        nonce: 0,
        fee: 100,
        payload: encode_execute_proposal_payload_v1(&execute_payload).to_vec(),
        sig: [0u8; 64],
    };

    apply_governance_execute_tx(&mut db, &execute_tx, 111).unwrap();
    println!("  - Proposal executed at height 111 (after 10 block timelock)");

    // Verify: Module is now ACTIVE
    let loaded = read_ai_entity(&db, &module_id).unwrap().unwrap();
    assert!(
        loaded.is_active,
        "STEP 1 CHECK: Module must be active after activation"
    );
    println!("  - Module activated: is_active = {}", loaded.is_active);

    // ========================================================================
    // STEP 2: NORMAL OPERATION - Module emits prediction signals
    // ========================================================================

    println!("\nStep 2: NORMAL OPERATION - Module emits prediction signals");

    // Module emits some normal prediction signals
    for i in 0..3 {
        let prediction_hash = blake3::hash(format!("prediction:market:day_{i}").as_bytes()).into();
        let signal_payload = SignalCommitmentPayloadV1 {
            signal_hash: prediction_hash,
            signal_type: AiSignalType::Prediction,
            issuer_entity_id: module_id,
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
        };
        let signal_tx = TxV1 {
            version: TxVersion::V1,
            from: module_id,
            pubkey: module_id,
            nonce: i,
            fee: 100,
            payload: encode_signal_commitment_payload_v1(&signal_payload),
            sig: [0u8; 64],
        };
        apply_signal_commitment_tx(&mut db, &signal_tx, 200 + i).unwrap();
    }
    println!("  - Emitted 3 prediction signals (heights 200-202)");

    // Verify signals were stored
    let predictions = get_signals_by_type(&db, AiSignalType::Prediction, 0, 500).unwrap();
    assert_eq!(predictions.len(), 3, "Should have 3 prediction signals");
    println!(
        "  - Verified: {} prediction signals in state",
        predictions.len()
    );

    // ========================================================================
    // STEP 3: PROBLEM DETECTED - Module emits anomaly signal
    // ========================================================================

    println!("\nStep 3: PROBLEM DETECTED - Module emits anomaly signal");

    // Something goes wrong - module detects anomalous behavior
    // (e.g., predictions are consistently wrong, or suspicious patterns)
    let anomaly_hash =
        blake3::hash(b"ANOMALY:prediction_accuracy_below_threshold:severity=HIGH").into();
    let anomaly_payload = SignalCommitmentPayloadV1 {
        signal_hash: anomaly_hash,
        signal_type: AiSignalType::Anomaly,
        issuer_entity_id: module_id,
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
    };
    let anomaly_tx = TxV1 {
        version: TxVersion::V1,
        from: module_id,
        pubkey: module_id,
        nonce: 3, // After the 3 predictions
        fee: 100,
        payload: encode_signal_commitment_payload_v1(&anomaly_payload),
        sig: [0u8; 64],
    };
    apply_signal_commitment_tx(&mut db, &anomaly_tx, 300).unwrap();
    println!("  - Anomaly signal emitted at height 300");
    println!("  - Anomaly: prediction_accuracy_below_threshold (HIGH severity)");

    // Verify anomaly was recorded
    let anomalies = get_signals_by_type(&db, AiSignalType::Anomaly, 0, 500).unwrap();
    assert_eq!(anomalies.len(), 1, "Should have 1 anomaly signal");
    println!("  - Verified: {} anomaly signal in state", anomalies.len());

    // ========================================================================
    // STEP 4: ROLLBACK - Submit ModuleRollback proposal
    // ========================================================================

    println!("\nStep 4: ROLLBACK - Submit ModuleRollback proposal");

    // Governance council responds to the anomaly by submitting rollback
    let rollback_payload = SubmitProposalPayloadV1 {
        proposal_type: ProposalType::ModuleRollback,
        gate_id,
        proposal_data: module_id.to_vec(),
    };
    let rollback_submit_tx = TxV1 {
        version: TxVersion::V1,
        from: governance_account,
        pubkey: governance_account,
        nonce: 0,
        fee: 100,
        payload: encode_submit_proposal_payload_v1(&rollback_payload),
        sig: [0u8; 64],
    };

    let rollback_proposal_id =
        apply_governance_submit_tx(&mut db, &rollback_submit_tx, 350).unwrap();
    println!("  - Rollback proposal submitted at height 350");
    println!(
        "  - Proposal ID: {:02x}{:02x}...",
        rollback_proposal_id[0], rollback_proposal_id[1]
    );

    // Module is still active during timelock (giving time for review)
    let loaded = read_ai_entity(&db, &module_id).unwrap().unwrap();
    assert!(
        loaded.is_active,
        "Module should still be active during timelock"
    );
    println!("  - Module still active during timelock review period");

    // ========================================================================
    // STEP 5: EXECUTE ROLLBACK - After timelock, deactivate module
    // ========================================================================

    println!("\nStep 5: EXECUTE ROLLBACK - Deactivate module after timelock");

    let rollback_execute_payload = ExecuteProposalPayloadV1 {
        proposal_id: rollback_proposal_id,
    };
    let rollback_execute_tx = TxV1 {
        version: TxVersion::V1,
        from: governance_account,
        pubkey: governance_account,
        nonce: 0,
        fee: 100,
        payload: encode_execute_proposal_payload_v1(&rollback_execute_payload).to_vec(),
        sig: [0u8; 64],
    };

    // Execute at height 361 (350 + 10 block timelock + 1)
    apply_governance_execute_tx(&mut db, &rollback_execute_tx, 361).unwrap();
    println!("  - Rollback executed at height 361");

    // ========================================================================
    // STEP 6: VERIFY - Module is now DEACTIVATED
    // ========================================================================

    println!("\nStep 6: VERIFY - Module is deactivated");

    let final_state = read_ai_entity(&db, &module_id).unwrap().unwrap();

    // THE CRITICAL ASSERTION: Module must be inactive after rollback
    assert!(
        !final_state.is_active,
        "D24.5 FINAL CHECK: Module MUST be inactive after rollback"
    );

    println!("  - Module is_active = {}", final_state.is_active);
    println!("  - Module successfully deactivated!");

    // ========================================================================
    // SUMMARY
    // ========================================================================

    println!("\n=== D24.5 ROLLBACK WORKFLOW COMPLETE ===");
    println!("  1. Module started INACTIVE (third-party default)");
    println!("  2. Activated via governance proposal");
    println!("  3. Operated normally (3 prediction signals)");
    println!("  4. Problem detected (anomaly signal)");
    println!("  5. Rollback proposal submitted");
    println!("  6. After timelock, rollback executed");
    println!("  7. Module DEACTIVATED - network protected");
    println!("\nD24.5 ACCEPTANCE CRITERIA: PASSED\n");
}
