#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]

//! Capability Enforcement Integration Tests
//!
//! PURPOSE: Verify that AI entities are properly gated by their capability
//! manifest, active status, and the global kill switch.
//!
//! COVERAGE:
//! - Tier 0 action rejection (submit + execute)
//! - Capability checks (emit_proposals, read_memory_objects, request_execution)
//! - Inactive entity rejection
//! - Kill switch enforcement (blocks and unblocks)

use novai_ai_entities::tiers::{tier_for_action, ActionTier, ActionType};
use novai_ai_entities::{
    AiEntity, AiSignalType, ApprovalGate, AutonomyMode, Capabilities, GateType, MemoryObjectType,
};
use novai_codec::encode_approval_gate_v1;
use novai_execution::{
    apply_create_memory_object_tx, apply_governance_execute_tx, apply_governance_submit_tx,
    apply_signal_commitment_tx, encode_create_memory_object_payload_v1,
    encode_execute_proposal_payload_v1, encode_signal_commitment_payload_v1,
    encode_submit_proposal_payload_v1, read_ai_kill_switch, read_proposal, write_ai_entity_op,
    write_ai_kill_switch_op, CreateMemoryObjectPayloadV1, ExecError, ExecuteProposalPayloadV1,
    SignalCommitmentPayloadV1, SubmitProposalPayloadV1,
};
use novai_governance::ProposalType;
use novai_state::{ai_entity_by_address_key, approval_gate_key, KvBatch, MemKv, WriteOp};
use novai_types::{TxV1, TxVersion};

// ============================================================================
// TEST HELPERS
// ============================================================================

/// Gate ID for capability enforcement tests.
fn test_gate_id() -> [u8; 32] {
    *blake3::hash(b"NOVAI_CAPABILITY_ENFORCEMENT_GATE_V1").as_bytes()
}

/// Create a `TimelockOnly` gate (auto-approve for quick testing).
fn create_timelock_gate(timelock_blocks: u64, expiry_blocks: u64) -> ApprovalGate {
    ApprovalGate {
        gate_id: test_gate_id(),
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

/// Create an AI entity with specified capabilities.
fn create_entity(
    name: &[u8],
    balance: u128,
    is_active: bool,
    capabilities: Capabilities,
) -> AiEntity {
    let code_hash = *blake3::hash(name).as_bytes();
    let creator = *blake3::hash(&[name, b"_creator"].concat()).as_bytes();
    let mut entity = AiEntity::new(code_hash, creator, AutonomyMode::Gated, capabilities, 0);
    entity.economic_balance = balance;
    entity.is_active = is_active;
    entity
}

/// Store an AI entity in the database. These tests build txs with
/// `tx.from = entity.id`, so we also write a reverse-index entry mapping that
/// id to itself — mirroring the (`address` → `entity_id`) lookup the inner
/// handlers depend on.
fn store_entity(db: &mut MemKv, entity: &AiEntity) {
    db.apply_batch(&[
        write_ai_entity_op(entity),
        WriteOp::Put(ai_entity_by_address_key(&entity.id), entity.id.to_vec()),
    ])
    .unwrap();
}

/// Create a test transaction.
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

/// Create a signal commitment payload.
fn create_signal_payload(issuer: [u8; 32]) -> Vec<u8> {
    let payload = SignalCommitmentPayloadV1 {
        signal_hash: [0xAAu8; 32],
        signal_type: AiSignalType::Anomaly,
        issuer_entity_id: issuer,
        reputation: None,
        purchase: None,
        stake_deposit: None,
        stake_withdraw: None,
        stake_slash: None,
        composition_check: None,
    };
    encode_signal_commitment_payload_v1(&payload)
}

/// Create a memory object payload.
fn create_memory_payload() -> Vec<u8> {
    let payload = CreateMemoryObjectPayloadV1 {
        object_type: MemoryObjectType::ChainSummary,
        data: vec![0x01, 0x02, 0x03],
    };
    encode_create_memory_object_payload_v1(&payload)
}

/// Create a submit proposal payload.
fn create_submit_payload(proposal_type: ProposalType, data: Vec<u8>) -> Vec<u8> {
    let payload = SubmitProposalPayloadV1 {
        proposal_type,
        gate_id: test_gate_id(),
        proposal_data: data,
    };
    encode_submit_proposal_payload_v1(&payload)
}

/// Create an execute proposal payload.
fn create_execute_payload(proposal_id: [u8; 32]) -> Vec<u8> {
    let payload = ExecuteProposalPayloadV1 { proposal_id };
    encode_execute_proposal_payload_v1(&payload).to_vec()
}

// ============================================================================
// TEST 1: Tier 0 submit rejection
// ============================================================================

#[test]
fn tier0_submit_rejected_even_with_all_capabilities() {
    // Verify the tier classification first
    assert_eq!(
        tier_for_action(&ActionType::ModifyConsensusRule),
        ActionTier::Tier0Never
    );
    assert_eq!(
        tier_for_action(&ActionType::ModifyStateTransition),
        ActionTier::Tier0Never
    );

    let mut db = MemKv::new();

    // Setup: gated entity with all capabilities and a gate
    let entity = create_entity(b"tier0_attacker", 100_000, true, Capabilities::gated());
    store_entity(&mut db, &entity);
    let gate = create_timelock_gate(0, 1000);
    store_gate(&mut db, &gate);

    // Attempt 1: ParamChange with ModifyConsensusRule (byte 0)
    let payload = create_submit_payload(
        ProposalType::ParamChange,
        vec![ActionType::ModifyConsensusRule.to_byte()],
    );
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let result = apply_governance_submit_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::Tier0ActionForbidden)),
        "ParamChange with ModifyConsensusRule should be rejected: {result:?}"
    );

    // Attempt 2: PolicyChange with ModifyStateTransition (byte 1)
    let payload = create_submit_payload(
        ProposalType::PolicyChange,
        vec![ActionType::ModifyStateTransition.to_byte()],
    );
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let result = apply_governance_submit_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::Tier0ActionForbidden)),
        "PolicyChange with ModifyStateTransition should be rejected: {result:?}"
    );
}

// ============================================================================
// TEST 2: Tier 0 execute rejection (defense in depth)
// ============================================================================

#[test]
fn tier0_execute_rejected_even_with_all_capabilities() {
    let mut db = MemKv::new();

    // Setup: entity + gate
    let entity = create_entity(b"tier0_executor", 100_000, true, Capabilities::gated());
    store_entity(&mut db, &entity);
    let gate = create_timelock_gate(0, 1000);
    store_gate(&mut db, &gate);

    // Submit a valid ParamChange (non-Tier-0) proposal first so we can test execution
    let safe_data = vec![ActionType::UpdateBaseFee.to_byte(), 0x01, 0x02];
    let payload = create_submit_payload(ProposalType::ParamChange, safe_data);
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &tx, 100).unwrap();

    // Verify proposal is stored and executable (TimelockOnly with 0 blocks)
    let proposal = read_proposal(&db, &proposal_id).unwrap().unwrap();
    assert!(proposal.can_execute_at(100));

    // Execute the safe proposal — should succeed
    let exec_payload = create_execute_payload(proposal_id);
    let exec_tx = create_test_tx(entity.id, 1, 10, exec_payload);
    let result = apply_governance_execute_tx(&mut db, &exec_tx, 100);
    assert!(
        result.is_ok(),
        "Safe ParamChange execution should succeed: {result:?}"
    );

    // Now submit a new proposal with Tier 0 action — this is blocked at submission
    // But even if it somehow gets through, execute also checks
    let tier0_payload = create_submit_payload(
        ProposalType::ParamChange,
        vec![ActionType::ModifyConsensusRule.to_byte()],
    );
    let tier0_tx = create_test_tx(entity.id, 2, 10, tier0_payload);
    let result = apply_governance_submit_tx(&mut db, &tier0_tx, 100);
    assert!(
        matches!(result, Err(ExecError::Tier0ActionForbidden)),
        "Tier 0 should be blocked at submit: {result:?}"
    );
}

// ============================================================================
// TEST 3: Tier 1 submit stays pending (with approval gate)
// ============================================================================

#[test]
fn tier1_submit_without_approval_stays_pending() {
    let mut db = MemKv::new();

    // Setup: entity with gated capabilities + a gate that requires approval (threshold > 0)
    let entity = create_entity(b"tier1_submitter", 100_000, true, Capabilities::gated());
    store_entity(&mut db, &entity);

    let approver = [0x99u8; 32];
    let gate = ApprovalGate {
        gate_id: test_gate_id(),
        gate_type: GateType::Multisig,
        required_approvers: vec![approver],
        threshold: 1, // Requires 1 approval
        timelock_blocks: 10,
        expiry_blocks: 1000,
        veto_enabled: false,
        freeze_enabled: false,
    };
    store_gate(&mut db, &gate);

    // Submit a Tier 1 action (UpdateBaseFee via ParamChange)
    let data = vec![ActionType::UpdateBaseFee.to_byte(), 0x01];
    let payload = create_submit_payload(ProposalType::ParamChange, data);
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let proposal_id = apply_governance_submit_tx(&mut db, &tx, 100).unwrap();

    // Proposal should exist but NOT be executable (needs approval)
    let proposal = read_proposal(&db, &proposal_id).unwrap().unwrap();
    assert!(
        !proposal.can_execute_at(100),
        "Tier 1 proposal should NOT be auto-executable without approval"
    );
}

// ============================================================================
// TEST 4: Entity without emit_proposals cannot signal
// ============================================================================

#[test]
fn entity_without_emit_proposals_cannot_signal() {
    let mut db = MemKv::new();

    // Setup: entity with read_only capabilities (no emit_proposals)
    let entity = create_entity(b"readonly_entity", 100_000, true, Capabilities::read_only());
    store_entity(&mut db, &entity);

    let payload = create_signal_payload(entity.id);
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let result = apply_signal_commitment_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::IssuerMissingCapability)),
        "Entity without emit_proposals should be rejected from signaling: {result:?}"
    );
}

// ============================================================================
// TEST 5: Entity without read_memory_objects cannot create memory
// ============================================================================

#[test]
fn entity_without_read_memory_cannot_create_memory() {
    let mut db = MemKv::new();

    // Setup: entity with custom capabilities (no read_memory_objects)
    let caps = Capabilities {
        read_public_chain: true,
        read_memory_objects: false,
        emit_proposals: true,
        request_execution: false,
        read_nnpx_derived: false,
        submit_reputation_updates: false,
        _reserved: [false; 2],
    };
    let entity = create_entity(b"no_memory_entity", 100_000, true, caps);
    store_entity(&mut db, &entity);

    let payload = create_memory_payload();
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let result = apply_create_memory_object_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::IssuerMissingCapability)),
        "Entity without read_memory_objects should be rejected from creating memory: {result:?}"
    );
}

// ============================================================================
// TEST 6: Inactive entity cannot signal
// ============================================================================

#[test]
fn inactive_entity_cannot_signal() {
    let mut db = MemKv::new();

    // Setup: inactive entity with full capabilities
    let entity = create_entity(b"inactive_signaler", 100_000, false, Capabilities::gated());
    store_entity(&mut db, &entity);

    let payload = create_signal_payload(entity.id);
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let result = apply_signal_commitment_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::EntityNotActive)),
        "Inactive entity should be rejected from signaling: {result:?}"
    );
}

// ============================================================================
// TEST 7: Inactive entity cannot create memory
// ============================================================================

#[test]
fn inactive_entity_cannot_create_memory() {
    let mut db = MemKv::new();

    // Setup: inactive entity with full capabilities
    let entity = create_entity(b"inactive_memory", 100_000, false, Capabilities::gated());
    store_entity(&mut db, &entity);

    let payload = create_memory_payload();
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let result = apply_create_memory_object_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::EntityNotActive)),
        "Inactive entity should be rejected from creating memory: {result:?}"
    );
}

// ============================================================================
// TEST 8: Kill switch blocks signal commitment
// ============================================================================

#[test]
fn kill_switch_blocks_signal() {
    let mut db = MemKv::new();

    // Setup: active entity with full capabilities
    let entity = create_entity(b"killswitch_signal", 100_000, true, Capabilities::gated());
    store_entity(&mut db, &entity);

    // Activate the kill switch
    db.apply_batch(&[write_ai_kill_switch_op(true)]).unwrap();
    assert!(read_ai_kill_switch(&db).unwrap());

    let payload = create_signal_payload(entity.id);
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let result = apply_signal_commitment_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::AiKillSwitchActive)),
        "Kill switch should block signal commitment: {result:?}"
    );
}

// ============================================================================
// TEST 9: Kill switch blocks memory creation
// ============================================================================

#[test]
fn kill_switch_blocks_memory() {
    let mut db = MemKv::new();

    // Setup: active entity with full capabilities
    let entity = create_entity(b"killswitch_memory", 100_000, true, Capabilities::gated());
    store_entity(&mut db, &entity);

    // Activate the kill switch
    db.apply_batch(&[write_ai_kill_switch_op(true)]).unwrap();

    let payload = create_memory_payload();
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let result = apply_create_memory_object_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::AiKillSwitchActive)),
        "Kill switch should block memory creation: {result:?}"
    );
}

// ============================================================================
// TEST 10: Kill switch blocks governance from AI entities
// ============================================================================

#[test]
fn kill_switch_blocks_governance() {
    let mut db = MemKv::new();

    // Setup: active entity with full capabilities + gate
    let entity = create_entity(b"killswitch_gov", 100_000, true, Capabilities::gated());
    store_entity(&mut db, &entity);
    let gate = create_timelock_gate(0, 1000);
    store_gate(&mut db, &gate);

    // Activate the kill switch
    db.apply_batch(&[write_ai_kill_switch_op(true)]).unwrap();

    let payload = create_submit_payload(
        ProposalType::ParamChange,
        vec![ActionType::UpdateBaseFee.to_byte()],
    );
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let result = apply_governance_submit_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::AiKillSwitchActive)),
        "Kill switch should block AI entity governance submissions: {result:?}"
    );
}

// ============================================================================
// TEST 11: Kill switch off allows signal
// ============================================================================

#[test]
fn kill_switch_off_allows_signal() {
    let mut db = MemKv::new();

    // Setup: active entity with full capabilities
    let entity = create_entity(b"killswitch_off", 100_000, true, Capabilities::gated());
    store_entity(&mut db, &entity);

    // Activate then deactivate the kill switch
    db.apply_batch(&[write_ai_kill_switch_op(true)]).unwrap();
    assert!(read_ai_kill_switch(&db).unwrap());
    db.apply_batch(&[write_ai_kill_switch_op(false)]).unwrap();
    assert!(!read_ai_kill_switch(&db).unwrap());

    // Signal should succeed now
    let payload = create_signal_payload(entity.id);
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let result = apply_signal_commitment_tx(&mut db, &tx, 100);
    assert!(
        result.is_ok(),
        "Signal should succeed when kill switch is off: {result:?}"
    );
}

// ============================================================================
// TEST 12: request_execution check
// ============================================================================

#[test]
fn request_execution_check() {
    let mut db = MemKv::new();

    // Setup: entity with advisory capabilities (emit_proposals=true, request_execution=false)
    let entity = create_entity(b"advisory_entity", 100_000, true, Capabilities::advisory());
    store_entity(&mut db, &entity);
    let gate = create_timelock_gate(0, 1000);
    store_gate(&mut db, &gate);

    // Attempt 1: ParamChange requires request_execution — should fail
    let payload = create_submit_payload(
        ProposalType::ParamChange,
        vec![ActionType::UpdateBaseFee.to_byte()],
    );
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let result = apply_governance_submit_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::IssuerMissingCapability)),
        "Advisory entity should not be able to submit ParamChange: {result:?}"
    );

    // Attempt 2: PolicyChange also requires request_execution — should fail
    let payload = create_submit_payload(
        ProposalType::PolicyChange,
        vec![ActionType::UpdateSpamThreshold.to_byte()],
    );
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let result = apply_governance_submit_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::IssuerMissingCapability)),
        "Advisory entity should not be able to submit PolicyChange: {result:?}"
    );

    // Attempt 3: ModuleActivation also requires request_execution — should fail
    let payload = create_submit_payload(
        ProposalType::ModuleActivation,
        vec![0u8; 32], // entity_id
    );
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let result = apply_governance_submit_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::IssuerMissingCapability)),
        "Advisory entity should not be able to submit ModuleActivation: {result:?}"
    );

    // Attempt 4: EmergencyFreeze also requires request_execution — should fail
    let payload = create_submit_payload(
        ProposalType::EmergencyFreeze,
        vec![0x01], // global kill switch payload
    );
    let tx = create_test_tx(entity.id, 0, 10, payload);
    let result = apply_governance_submit_tx(&mut db, &tx, 100);
    assert!(
        matches!(result, Err(ExecError::IssuerMissingCapability)),
        "Advisory entity should not be able to submit EmergencyFreeze: {result:?}"
    );

    // Now upgrade to gated entity and retry ParamChange — should succeed
    let gated_entity = create_entity(b"gated_entity", 100_000, true, Capabilities::gated());
    store_entity(&mut db, &gated_entity);

    let payload = create_submit_payload(
        ProposalType::ParamChange,
        vec![ActionType::UpdateBaseFee.to_byte()],
    );
    let tx = create_test_tx(gated_entity.id, 0, 10, payload);
    let result = apply_governance_submit_tx(&mut db, &tx, 100);
    assert!(
        result.is_ok(),
        "Gated entity should be able to submit ParamChange: {result:?}"
    );
}
