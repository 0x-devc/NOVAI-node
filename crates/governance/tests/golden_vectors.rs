//! Golden vector tests for governance encoding stability.
//!
//! Run with `UPDATE_VECTORS=1` to regenerate vectors:
//! ```text
//! UPDATE_VECTORS=1 cargo test -p novai-governance --test golden_vectors
//! ```

use novai_governance::{
    decode_audit_log_v1, decode_proposal_v1, encode_audit_log_v1, encode_proposal_v1, AuditAction,
    AuditLogEntry, Proposal, ProposalState, ProposalType,
};
use std::fs;
use std::path::PathBuf;

fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors")
}

fn should_update_vectors() -> bool {
    std::env::var("UPDATE_VECTORS").is_ok()
}

fn write_or_compare(path: &PathBuf, actual: &[u8], name: &str) {
    if should_update_vectors() {
        fs::create_dir_all(vectors_dir()).expect("failed to create vectors dir");
        fs::write(path, actual).expect("failed to write golden vector");
        println!("Updated golden vector: {}", path.display());
        println!("Vector length: {} bytes", actual.len());
    } else {
        let expected = fs::read(path).unwrap_or_else(|_| {
            panic!(
                "Golden vector missing: {}. Run with UPDATE_VECTORS=1",
                path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "{name} encoding drifted from golden vector!"
        );
    }
}

// ============================================================================
// PROPOSAL GOLDEN VECTORS
// ============================================================================

/// Standard proposal for golden vector testing (`ParamChange`, no approvals).
fn golden_proposal_param_change() -> Proposal {
    Proposal {
        id: [0x11u8; 32],
        proposal_type: ProposalType::ParamChange,
        proposal_data: b"MIN_FEE=100".to_vec(),
        proposer: [0x01u8; 32],
        gate_id: [0x42u8; 32],
        state: ProposalState::Submitted,
        submitted_at: 1000,
        approved_at: 0,
        executable_at: 0,
        expires_at: 51000,
        executed_at: 0,
        approvals: vec![],
    }
}

/// Proposal with approvals for golden vector testing.
fn golden_proposal_with_approvals() -> Proposal {
    Proposal {
        id: [0x22u8; 32],
        proposal_type: ProposalType::ModuleActivation,
        proposal_data: b"activate:module_v2".to_vec(),
        proposer: [0x01u8; 32],
        gate_id: [0x42u8; 32],
        state: ProposalState::Approved,
        submitted_at: 500,
        approved_at: 600,
        executable_at: 5600, // 600 + 5000 high-risk timelock
        expires_at: 50500,
        executed_at: 0,
        approvals: vec![[0xAAu8; 32], [0xBBu8; 32], [0xCCu8; 32]],
    }
}

/// Emergency freeze proposal for golden vector testing.
fn golden_proposal_emergency() -> Proposal {
    Proposal {
        id: [0x33u8; 32],
        proposal_type: ProposalType::EmergencyFreeze,
        proposal_data: b"freeze:entity_0x44".to_vec(),
        proposer: [0x01u8; 32],
        gate_id: [0x55u8; 32],
        state: ProposalState::Executable,
        submitted_at: 100,
        approved_at: 110,
        executable_at: 210, // 110 + 100 emergency timelock
        expires_at: 10100,
        executed_at: 0,
        approvals: vec![[0xEEu8; 32], [0xFFu8; 32]],
    }
}

#[test]
fn golden_proposal_v1_param_change() {
    let proposal = golden_proposal_param_change();
    let bytes = encode_proposal_v1(&proposal);
    let path = vectors_dir().join("proposal_v1_param_change.bin");

    write_or_compare(&path, &bytes, "Proposal (ParamChange)");

    // Verify roundtrip
    let decoded = decode_proposal_v1(&bytes).expect("decode failed");
    assert_eq!(proposal.id, decoded.id);
    assert_eq!(proposal.proposal_type, decoded.proposal_type);
    assert_eq!(proposal.proposal_data, decoded.proposal_data);
    assert_eq!(proposal.state, decoded.state);
    assert_eq!(proposal.submitted_at, decoded.submitted_at);
    assert_eq!(proposal.expires_at, decoded.expires_at);
}

#[test]
fn golden_proposal_v1_with_approvals() {
    let proposal = golden_proposal_with_approvals();
    let bytes = encode_proposal_v1(&proposal);
    let path = vectors_dir().join("proposal_v1_with_approvals.bin");

    write_or_compare(&path, &bytes, "Proposal (with approvals)");

    // Verify roundtrip
    let decoded = decode_proposal_v1(&bytes).expect("decode failed");
    assert_eq!(proposal.id, decoded.id);
    assert_eq!(proposal.proposal_type, decoded.proposal_type);
    assert_eq!(proposal.state, decoded.state);
    assert_eq!(proposal.approved_at, decoded.approved_at);
    assert_eq!(proposal.executable_at, decoded.executable_at);
    // Approvals should be sorted in encoding
    assert_eq!(decoded.approvals.len(), 3);
}

#[test]
fn golden_proposal_v1_emergency() {
    let proposal = golden_proposal_emergency();
    let bytes = encode_proposal_v1(&proposal);
    let path = vectors_dir().join("proposal_v1_emergency.bin");

    write_or_compare(&path, &bytes, "Proposal (EmergencyFreeze)");

    // Verify roundtrip
    let decoded = decode_proposal_v1(&bytes).expect("decode failed");
    assert_eq!(proposal.id, decoded.id);
    assert_eq!(proposal.proposal_type, decoded.proposal_type);
    assert_eq!(proposal.state, decoded.state);
}

#[test]
fn golden_proposal_all_types() {
    // Test all proposal types encode/decode correctly
    for (pt, name) in [
        (ProposalType::ParamChange, "param_change"),
        (ProposalType::ModuleActivation, "module_activation"),
        (ProposalType::ModuleRollback, "module_rollback"),
        (ProposalType::PolicyChange, "policy_change"),
        (ProposalType::EmergencyFreeze, "emergency_freeze"),
    ] {
        let proposal = Proposal {
            id: [pt.to_byte(); 32],
            proposal_type: pt,
            proposal_data: format!("type:{name}").into_bytes(),
            proposer: [0x01u8; 32],
            gate_id: [0x42u8; 32],
            state: ProposalState::Submitted,
            submitted_at: 100,
            approved_at: 0,
            executable_at: 0,
            expires_at: 50100,
            executed_at: 0,
            approvals: vec![],
        };

        let bytes = encode_proposal_v1(&proposal);
        let decoded = decode_proposal_v1(&bytes).expect("decode failed");
        assert_eq!(decoded.proposal_type, pt, "Type {name} roundtrip failed");
    }
}

// ============================================================================
// AUDIT LOG GOLDEN VECTORS
// ============================================================================

/// Standard audit log entry for golden vector testing (Submitted, with actor).
#[allow(clippy::missing_const_for_fn)] // Vec::new() is not const-stable
fn golden_audit_submitted() -> AuditLogEntry {
    AuditLogEntry {
        proposal_id: [0x11u8; 32],
        action: AuditAction::Submitted,
        block_height: 1000,
        actor: Some([0x01u8; 32]),
        details: Vec::new(),
    }
}

/// Audit log entry without actor (Expired).
#[allow(clippy::missing_const_for_fn)] // Vec::new() is not const-stable
fn golden_audit_expired() -> AuditLogEntry {
    AuditLogEntry {
        proposal_id: [0x22u8; 32],
        action: AuditAction::Expired,
        block_height: 51000,
        actor: None,
        details: Vec::new(),
    }
}

/// Audit log entry with details.
fn golden_audit_with_details() -> AuditLogEntry {
    AuditLogEntry {
        proposal_id: [0x33u8; 32],
        action: AuditAction::Executed,
        block_height: 5700,
        actor: Some([0x44u8; 32]),
        details: b"result:success,gas_used:12345".to_vec(),
    }
}

#[test]
fn golden_audit_log_v1_submitted() {
    let entry = golden_audit_submitted();
    let bytes = encode_audit_log_v1(&entry);
    let path = vectors_dir().join("audit_log_v1_submitted.bin");

    write_or_compare(&path, &bytes, "AuditLog (Submitted)");

    // Verify roundtrip
    let decoded = decode_audit_log_v1(&bytes).expect("decode failed");
    assert_eq!(entry.proposal_id, decoded.proposal_id);
    assert_eq!(entry.action, decoded.action);
    assert_eq!(entry.block_height, decoded.block_height);
    assert_eq!(entry.actor, decoded.actor);
}

#[test]
fn golden_audit_log_v1_expired() {
    let entry = golden_audit_expired();
    let bytes = encode_audit_log_v1(&entry);
    let path = vectors_dir().join("audit_log_v1_expired.bin");

    write_or_compare(&path, &bytes, "AuditLog (Expired, no actor)");

    // Verify roundtrip
    let decoded = decode_audit_log_v1(&bytes).expect("decode failed");
    assert_eq!(entry.proposal_id, decoded.proposal_id);
    assert_eq!(entry.action, decoded.action);
    assert_eq!(entry.actor, None);
}

#[test]
fn golden_audit_log_v1_with_details() {
    let entry = golden_audit_with_details();
    let bytes = encode_audit_log_v1(&entry);
    let path = vectors_dir().join("audit_log_v1_with_details.bin");

    write_or_compare(&path, &bytes, "AuditLog (with details)");

    // Verify roundtrip
    let decoded = decode_audit_log_v1(&bytes).expect("decode failed");
    assert_eq!(entry.details, decoded.details);
}

#[test]
fn golden_audit_all_actions() {
    // Test all audit actions encode/decode correctly
    for (action, name) in [
        (AuditAction::Submitted, "submitted"),
        (AuditAction::Approved, "approved"),
        (AuditAction::Executed, "executed"),
        (AuditAction::Rejected, "rejected"),
        (AuditAction::Expired, "expired"),
    ] {
        let entry = AuditLogEntry {
            proposal_id: [action.to_byte(); 32],
            action,
            block_height: 1000,
            actor: Some([0x01u8; 32]),
            details: Vec::new(),
        };

        let bytes = encode_audit_log_v1(&entry);
        let decoded = decode_audit_log_v1(&bytes).expect("decode failed");
        assert_eq!(decoded.action, action, "Action {name} roundtrip failed");
    }
}

// ============================================================================
// DETERMINISM TESTS
// ============================================================================

#[test]
fn proposal_encoding_is_deterministic() {
    let proposal = golden_proposal_with_approvals();

    let bytes1 = encode_proposal_v1(&proposal);
    let bytes2 = encode_proposal_v1(&proposal);

    assert_eq!(bytes1, bytes2, "Proposal encoding must be deterministic");
}

#[test]
fn audit_log_encoding_is_deterministic() {
    let entry = golden_audit_with_details();

    let bytes1 = encode_audit_log_v1(&entry);
    let bytes2 = encode_audit_log_v1(&entry);

    assert_eq!(bytes1, bytes2, "AuditLog encoding must be deterministic");
}

#[test]
fn proposal_approvals_sorted_deterministically() {
    // Create proposal with approvals in different orders
    let mut proposal1 = golden_proposal_param_change();
    proposal1.approvals = vec![[0xCCu8; 32], [0xAAu8; 32], [0xBBu8; 32]];

    let mut proposal2 = golden_proposal_param_change();
    proposal2.approvals = vec![[0xAAu8; 32], [0xBBu8; 32], [0xCCu8; 32]];

    let mut proposal3 = golden_proposal_param_change();
    proposal3.approvals = vec![[0xBBu8; 32], [0xCCu8; 32], [0xAAu8; 32]];

    let bytes1 = encode_proposal_v1(&proposal1);
    let bytes2 = encode_proposal_v1(&proposal2);
    let bytes3 = encode_proposal_v1(&proposal3);

    // All should produce identical bytes (approvals are sorted in encoding)
    assert_eq!(bytes1, bytes2, "Approval order must not affect encoding");
    assert_eq!(bytes2, bytes3, "Approval order must not affect encoding");
}
