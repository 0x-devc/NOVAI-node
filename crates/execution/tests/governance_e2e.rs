#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

//! Week 24 End-to-End Tests for Governance Execution.
//!
//! D24.3 - Full path: observe → detect → signal → commit → RPC → proposal → gate → execute
//! D24.4 - Third-party module registration and activation
//!
//! Acceptance Criteria:
//! 1. Full signal-to-rollback path works end-to-end
//! 2. Third-party module can be registered and activated
//! 3. Timelock is properly enforced
//! 4. Expired proposals are rejected
//! 5. Module activation via proposal works

use novai_ai_entities::{
    AiEntity, AiSignalType, ApprovalGate, AutonomyMode, Capabilities, GateType,
    CORE_OBSERVER_CODE_HASH, PROTOCOL_CREATOR,
};
use novai_codec::encode_approval_gate_v1;
use novai_execution::{
    apply_governance_execute_tx, apply_governance_submit_tx, apply_signal_commitment_tx,
    encode_execute_proposal_payload_v1, encode_signal_commitment_payload_v1,
    encode_submit_proposal_payload_v1, read_ai_entity, read_proposal, write_ai_entity_op,
    ExecError, ExecuteProposalPayloadV1, SignalCommitmentPayloadV1, SubmitProposalPayloadV1,
};
use novai_governance::ProposalType;
use novai_state::{ai_entity_by_address_key, approval_gate_key, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

// ============================================================================
// TEST HELPERS
// ============================================================================

/// Well-known gate ID for testnet timelock gate.
fn testnet_gate_id() -> [u8; 32] {
    *blake3::hash(b"NOVAI_TESTNET_GATE_V1").as_bytes()
}

/// Create a TimelockOnly gate with specified parameters.
fn create_timelock_gate(timelock_blocks: u64, expiry_blocks: u64) -> ApprovalGate {
    ApprovalGate {
        gate_id: testnet_gate_id(),
        gate_type: GateType::TimelockOnly,
        required_approvers: Vec::new(),
        threshold: 0, // No approvers needed for TimelockOnly
        timelock_blocks,
        expiry_blocks,
        veto_enabled: false,
        freeze_enabled: false,
    }
}

/// Write a gate to storage.
fn store_gate(db: &mut MemKv, gate: &ApprovalGate) {
    let key = approval_gate_key(&gate.gate_id);
    let value = encode_approval_gate_v1(gate);
    db.apply_batch(&[WriteOp::Put(key, value)]).unwrap();
}

/// Create the Core Observer entity.
fn create_core_observer(balance: u128, nonce: u64) -> AiEntity {
    let mut entity = AiEntity::new(
        CORE_OBSERVER_CODE_HASH,
        PROTOCOL_CREATOR,
        AutonomyMode::Gated,
        Capabilities::gated(),
        0,
    );
    entity.economic_balance = balance;
    entity.nonce = nonce;
    entity
}

/// Create a third-party module entity (different code hash).
fn create_third_party_module(balance: u128, nonce: u64) -> AiEntity {
    let code_hash = *blake3::hash(b"THIRD_PARTY_MODULE_V1").as_bytes();
    let creator = *blake3::hash(b"THIRD_PARTY_CREATOR").as_bytes();

    let mut entity = AiEntity::new(
        code_hash,
        creator,
        AutonomyMode::Gated,
        Capabilities::gated(),
        0,
    );
    entity.economic_balance = balance;
    entity.nonce = nonce;
    // Third-party modules start inactive until activated via governance
    entity.is_active = false;
    entity
}

/// Create a signal commitment payload.
fn create_signal_payload(
    signal_hash: [u8; 32],
    signal_type: AiSignalType,
    issuer: [u8; 32],
) -> Vec<u8> {
    let payload = SignalCommitmentPayloadV1 {
        signal_hash,
        signal_type,
        issuer_entity_id: issuer,
        reputation: None,
    };
    encode_signal_commitment_payload_v1(&payload)
}

/// Create a submit proposal payload.
fn create_submit_proposal_payload(
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
fn create_execute_proposal_payload(proposal_id: [u8; 32]) -> [u8; 33] {
    let payload = ExecuteProposalPayloadV1 { proposal_id };
    encode_execute_proposal_payload_v1(&payload)
}

/// Create a test transaction.
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
// Test 1: Full path - signal → rollback proposal → execute (D24.3)
// ============================================================================

#[test]
fn full_path_signal_to_rollback() {
    let mut db = MemKv::new();

    // Setup: Create Core Observer and gate
    let entity = create_core_observer(10_000_000_000, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    let gate = create_timelock_gate(10, 1000); // 10 block timelock, 1000 block expiry
    store_gate(&mut db, &gate);

    // Verify entity starts active
    let loaded = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert!(loaded.is_active, "Entity should start active");

    // Step 1: Core Observer detects anomaly and emits signal
    let anomaly_hash = blake3::hash(b"anomaly:critical:immediate_action_required").into();
    let signal_payload = create_signal_payload(anomaly_hash, AiSignalType::Anomaly, entity_id);
    let signal_tx = create_tx(entity_id, 0, 100, signal_payload);
    apply_signal_commitment_tx(&mut db, &signal_tx, 100).unwrap();

    // Step 2: Submit ModuleRollback proposal (referencing the anomaly)
    let rollback_payload =
        create_submit_proposal_payload(ProposalType::ModuleRollback, testnet_gate_id(), entity_id);
    let submit_tx = create_tx(entity_id, 1, 100, rollback_payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 100).unwrap();

    // Verify proposal exists
    let proposal = read_proposal(&db, &proposal_id).unwrap().unwrap();
    assert_eq!(
        proposal.proposal_type,
        ProposalType::ModuleRollback,
        "Proposal type must be ModuleRollback"
    );

    // Step 3: Try to execute before timelock - should fail
    let execute_payload = create_execute_proposal_payload(proposal_id);
    let execute_tx = create_tx(entity_id, 2, 100, execute_payload.to_vec());
    let result = apply_governance_execute_tx(&mut db, &execute_tx, 105); // Only 5 blocks elapsed
    assert!(
        matches!(result, Err(ExecError::ProposalNotExecutable)),
        "Should fail before timelock elapsed"
    );

    // Step 4: Execute after timelock elapsed
    let result = apply_governance_execute_tx(&mut db, &execute_tx, 111); // 11 blocks elapsed (>= 10)
    assert!(result.is_ok(), "Should succeed after timelock");

    // Verify entity is now inactive (rolled back)
    let loaded = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert!(
        !loaded.is_active,
        "Entity should be inactive after rollback"
    );

    // Verify proposal is marked as executed
    let proposal = read_proposal(&db, &proposal_id).unwrap().unwrap();
    assert_eq!(
        proposal.state,
        novai_governance::ProposalState::Executed,
        "Proposal should be Executed"
    );
}

// ============================================================================
// Test 2: Third-party module registration (D24.4)
// ============================================================================

#[test]
fn third_party_module_registration() {
    let mut db = MemKv::new();

    // Setup: Create third-party module (starts inactive)
    let module = create_third_party_module(10_000_000_000, 0);
    let module_id = module.id;
    db.apply_batch(&[
        write_ai_entity_op(&module),
        WriteOp::Put(ai_entity_by_address_key(&module.id), module.id.to_vec()),
    ])
    .unwrap();

    // Create a governance account that can submit proposals
    let governance_account = *blake3::hash(b"GOVERNANCE_MULTISIG").as_bytes();
    let gov_entity = AiEntity::new(
        *blake3::hash(b"GOVERNANCE_MODULE").as_bytes(),
        governance_account,
        AutonomyMode::Gated,
        Capabilities::gated(),
        0,
    );
    db.apply_batch(&[
        write_ai_entity_op(&gov_entity),
        WriteOp::Put(
            ai_entity_by_address_key(&gov_entity.id),
            gov_entity.id.to_vec(),
        ),
    ])
    .unwrap();

    let gate = create_timelock_gate(5, 500);
    store_gate(&mut db, &gate);

    // Verify module starts inactive
    let loaded = read_ai_entity(&db, &module_id).unwrap().unwrap();
    assert!(
        !loaded.is_active,
        "Third-party module should start inactive"
    );

    // Submit ModuleActivation proposal
    let activate_payload = create_submit_proposal_payload(
        ProposalType::ModuleActivation,
        testnet_gate_id(),
        module_id,
    );
    let submit_tx = create_tx(governance_account, 0, 100, activate_payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 100).unwrap();

    // Execute after timelock
    let execute_payload = create_execute_proposal_payload(proposal_id);
    let execute_tx = create_tx(governance_account, 0, 100, execute_payload.to_vec());
    apply_governance_execute_tx(&mut db, &execute_tx, 106).unwrap();

    // Verify module is now active
    let loaded = read_ai_entity(&db, &module_id).unwrap().unwrap();
    assert!(
        loaded.is_active,
        "Third-party module should be active after governance activation"
    );
}

// ============================================================================
// Test 3: Timelock enforcement
// ============================================================================

#[test]
fn timelock_enforcement() {
    let mut db = MemKv::new();

    let entity = create_core_observer(10_000_000_000, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    let gate = create_timelock_gate(100, 1000); // 100 block timelock
    store_gate(&mut db, &gate);

    // Submit proposal at height 500
    let rollback_payload =
        create_submit_proposal_payload(ProposalType::ModuleRollback, testnet_gate_id(), entity_id);
    let submit_tx = create_tx(entity_id, 0, 100, rollback_payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 500).unwrap();

    let execute_payload = create_execute_proposal_payload(proposal_id);
    let execute_tx = create_tx(entity_id, 1, 100, execute_payload.to_vec());

    // Test various heights before timelock
    for height in [500, 550, 580, 599] {
        let result = apply_governance_execute_tx(&mut db, &execute_tx, height);
        assert!(
            matches!(result, Err(ExecError::ProposalNotExecutable)),
            "Should fail at height {height} (before timelock)"
        );
    }

    // Should succeed at exactly timelock height (500 + 100 = 600)
    let result = apply_governance_execute_tx(&mut db, &execute_tx, 600);
    assert!(
        result.is_ok(),
        "Should succeed at exact timelock height 600"
    );
}

// ============================================================================
// Test 4: Expired proposal rejected
// ============================================================================

#[test]
fn expired_proposal_rejected() {
    let mut db = MemKv::new();

    let entity = create_core_observer(10_000_000_000, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    let gate = create_timelock_gate(10, 50); // 10 block timelock, 50 block expiry
    store_gate(&mut db, &gate);

    // Submit proposal at height 100
    let rollback_payload =
        create_submit_proposal_payload(ProposalType::ModuleRollback, testnet_gate_id(), entity_id);
    let submit_tx = create_tx(entity_id, 0, 100, rollback_payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 100).unwrap();

    // Proposal expires at 100 + 50 = 150
    let execute_payload = create_execute_proposal_payload(proposal_id);
    let execute_tx = create_tx(entity_id, 1, 100, execute_payload.to_vec());

    // Execute at height 110 (after timelock, before expiry) - should succeed
    // But first let's test the expiry case

    // Try to execute after expiry (at height 151)
    let result = apply_governance_execute_tx(&mut db, &execute_tx, 151);
    assert!(
        matches!(result, Err(ExecError::ProposalExpired)),
        "Should fail after expiry"
    );

    // Verify proposal state hasn't changed (still can be retried if not expired)
    // In this case it's expired so we just verify the error
}

// ============================================================================
// Test 5: Module activation via proposal
// ============================================================================

#[test]
fn module_activation_via_proposal() {
    let mut db = MemKv::new();

    // Create inactive entity
    let mut entity = create_core_observer(10_000_000_000, 0);
    entity.is_active = false;
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    // Create a separate active governance account to submit the proposal
    let gov_account = *blake3::hash(b"GOVERNANCE_ACTIVATION").as_bytes();
    let gov_entity = AiEntity::new(
        *blake3::hash(b"GOVERNANCE_ACTIVATION_MODULE").as_bytes(),
        gov_account,
        AutonomyMode::Gated,
        Capabilities::gated(),
        0,
    );
    db.apply_batch(&[
        write_ai_entity_op(&gov_entity),
        WriteOp::Put(
            ai_entity_by_address_key(&gov_entity.id),
            gov_entity.id.to_vec(),
        ),
    ])
    .unwrap();

    let gate = create_timelock_gate(5, 500);
    store_gate(&mut db, &gate);

    // Verify starts inactive
    let loaded = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert!(!loaded.is_active);

    // Submit and execute ModuleActivation (from active governance account)
    let activate_payload = create_submit_proposal_payload(
        ProposalType::ModuleActivation,
        testnet_gate_id(),
        entity_id,
    );
    let submit_tx = create_tx(gov_account, 0, 100, activate_payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 100).unwrap();

    let execute_payload = create_execute_proposal_payload(proposal_id);
    let execute_tx = create_tx(gov_account, 1, 100, execute_payload.to_vec());
    apply_governance_execute_tx(&mut db, &execute_tx, 106).unwrap();

    // Verify now active
    let loaded = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert!(loaded.is_active, "Entity should be active after activation");
}

// ============================================================================
// Test 6: Emergency freeze proposal
// ============================================================================

#[test]
fn emergency_freeze_proposal() {
    let mut db = MemKv::new();

    let entity = create_core_observer(10_000_000_000, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    // Emergency gate with shorter timelock
    let gate = create_timelock_gate(2, 100); // Only 2 blocks timelock for emergencies
    store_gate(&mut db, &gate);

    // Verify starts active
    assert!(read_ai_entity(&db, &entity_id).unwrap().unwrap().is_active);

    // Submit EmergencyFreeze proposal
    let freeze_payload =
        create_submit_proposal_payload(ProposalType::EmergencyFreeze, testnet_gate_id(), entity_id);
    let submit_tx = create_tx(entity_id, 0, 100, freeze_payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 100).unwrap();

    // Execute after short timelock
    let execute_payload = create_execute_proposal_payload(proposal_id);
    let execute_tx = create_tx(entity_id, 1, 100, execute_payload.to_vec());
    apply_governance_execute_tx(&mut db, &execute_tx, 103).unwrap();

    // Verify entity is frozen (inactive)
    let loaded = read_ai_entity(&db, &entity_id).unwrap().unwrap();
    assert!(!loaded.is_active, "Entity should be frozen after emergency");
}

// ============================================================================
// Test 7: Proposal not found error
// ============================================================================

#[test]
fn proposal_not_found_error() {
    let mut db = MemKv::new();

    let entity = create_core_observer(10_000_000_000, 0);
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    // Try to execute non-existent proposal
    let fake_proposal_id = [0xDEu8; 32];
    let execute_payload = create_execute_proposal_payload(fake_proposal_id);
    let execute_tx = create_tx(entity.id, 0, 100, execute_payload.to_vec());

    let result = apply_governance_execute_tx(&mut db, &execute_tx, 100);
    assert!(
        matches!(result, Err(ExecError::ProposalNotFound)),
        "Should fail with ProposalNotFound"
    );
}

// ============================================================================
// Test 8: Gate not found error
// ============================================================================

#[test]
fn gate_not_found_error() {
    let mut db = MemKv::new();

    let entity = create_core_observer(10_000_000_000, 0);
    let entity_id = entity.id;
    db.apply_batch(&[
        write_ai_entity_op(&entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();

    // Don't create the gate - try to submit proposal
    let rollback_payload =
        create_submit_proposal_payload(ProposalType::ModuleRollback, testnet_gate_id(), entity_id);
    let submit_tx = create_tx(entity_id, 0, 100, rollback_payload);

    let result = apply_governance_submit_tx(&mut db, &submit_tx, 100);
    assert!(
        matches!(result, Err(ExecError::GateNotFound)),
        "Should fail with GateNotFound"
    );
}

// ============================================================================
// Test 9: Full workflow - module lifecycle
// ============================================================================

#[test]
fn full_module_lifecycle() {
    let mut db = MemKv::new();

    // Setup: Create third-party module (inactive) and gate
    let module = create_third_party_module(10_000_000_000, 0);
    let module_id = module.id;
    db.apply_batch(&[
        write_ai_entity_op(&module),
        WriteOp::Put(ai_entity_by_address_key(&module.id), module.id.to_vec()),
    ])
    .unwrap();

    let gate = create_timelock_gate(5, 500);
    store_gate(&mut db, &gate);

    // Also need the governance account to submit proposals
    let gov_account = *blake3::hash(b"GOVERNANCE").as_bytes();

    // Phase 1: Activate the module
    let activate_payload = create_submit_proposal_payload(
        ProposalType::ModuleActivation,
        testnet_gate_id(),
        module_id,
    );
    let submit_tx = create_tx(gov_account, 0, 100, activate_payload);
    let activate_proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 100).unwrap();

    let execute_payload = create_execute_proposal_payload(activate_proposal_id);
    let execute_tx = create_tx(gov_account, 0, 100, execute_payload.to_vec());
    apply_governance_execute_tx(&mut db, &execute_tx, 106).unwrap();

    assert!(
        read_ai_entity(&db, &module_id).unwrap().unwrap().is_active,
        "Module should be active"
    );

    // Phase 2: Module operates and emits signals
    let signal_hash = blake3::hash(b"prediction:market_trend:bullish").into();
    let signal_payload = create_signal_payload(signal_hash, AiSignalType::Prediction, module_id);
    let signal_tx = create_tx(module_id, 0, 100, signal_payload);
    apply_signal_commitment_tx(&mut db, &signal_tx, 200).unwrap();

    // Phase 3: Rollback the module (e.g., detected issue)
    let rollback_payload =
        create_submit_proposal_payload(ProposalType::ModuleRollback, testnet_gate_id(), module_id);
    let submit_tx = create_tx(gov_account, 0, 100, rollback_payload);
    let rollback_proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 300).unwrap();

    let execute_payload = create_execute_proposal_payload(rollback_proposal_id);
    let execute_tx = create_tx(gov_account, 0, 100, execute_payload.to_vec());
    apply_governance_execute_tx(&mut db, &execute_tx, 306).unwrap();

    assert!(
        !read_ai_entity(&db, &module_id).unwrap().unwrap().is_active,
        "Module should be inactive after rollback"
    );

    // Phase 4: Re-activate after fix
    let reactivate_payload = create_submit_proposal_payload(
        ProposalType::ModuleActivation,
        testnet_gate_id(),
        module_id,
    );
    let submit_tx = create_tx(gov_account, 0, 100, reactivate_payload);
    let reactivate_proposal_id = apply_governance_submit_tx(&mut db, &submit_tx, 400).unwrap();

    let execute_payload = create_execute_proposal_payload(reactivate_proposal_id);
    let execute_tx = create_tx(gov_account, 0, 100, execute_payload.to_vec());
    apply_governance_execute_tx(&mut db, &execute_tx, 406).unwrap();

    assert!(
        read_ai_entity(&db, &module_id).unwrap().unwrap().is_active,
        "Module should be active again"
    );
}
