//! Canonical encoding for governance types.
//!
//! PURPOSE: Provide deterministic, versioned binary encoding for governance
//! types to enable storage and network transmission.
//!
//! INVARIANTS:
//! - All encodings are canonical (one valid encoding per logical value)
//! - All encodings are versioned for forward compatibility
//! - Big-endian byte order for all multi-byte integers
//! - Approvals are sorted by address for deterministic encoding
//!
//! FAILURE MODES:
//! - Invalid version byte returns decode error
//! - Truncated data returns decode error
//! - Invalid enum values return decode error

use crate::{AuditAction, AuditLogEntry, Proposal, ProposalId, ProposalState, ProposalType};
use novai_types::Address;

/// Proposal encoding version.
pub const PROPOSAL_CODEC_V1: u8 = 1;

/// Audit log encoding version.
pub const AUDIT_LOG_CODEC_V1: u8 = 1;

/// Decode error for governance types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// Data too short.
    TooShort { expected: usize, got: usize },
    /// Invalid version byte.
    InvalidVersion { expected: u8, got: u8 },
    /// Invalid proposal type.
    InvalidProposalType { value: u8 },
    /// Invalid proposal state.
    InvalidProposalState { value: u8 },
    /// Invalid audit action.
    InvalidAuditAction { value: u8 },
    /// Data length mismatch.
    LengthMismatch { declared: usize, actual: usize },
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort { expected, got } => {
                write!(f, "data too short: expected {expected}, got {got}")
            }
            Self::InvalidVersion { expected, got } => {
                write!(f, "invalid version: expected {expected}, got {got}")
            }
            Self::InvalidProposalType { value } => {
                write!(f, "invalid proposal type: {value}")
            }
            Self::InvalidProposalState { value } => {
                write!(f, "invalid proposal state: {value}")
            }
            Self::InvalidAuditAction { value } => {
                write!(f, "invalid audit action: {value}")
            }
            Self::LengthMismatch { declared, actual } => {
                write!(f, "length mismatch: declared {declared}, actual {actual}")
            }
        }
    }
}

impl std::error::Error for CodecError {}

/// Encode a proposal to canonical bytes.
///
/// Format (V1):
/// ```text
/// [version:1]                    // 1 byte - PROPOSAL_CODEC_V1
/// [id:32]                        // 32 bytes - proposal ID
/// [proposal_type:1]              // 1 byte - ProposalType
/// [proposer:32]                  // 32 bytes - proposer address
/// [gate_id:32]                   // 32 bytes - gate ID
/// [state:1]                      // 1 byte - ProposalState
/// [submitted_at:8]               // 8 bytes - big-endian u64
/// [approved_at:8]                // 8 bytes - big-endian u64
/// [executable_at:8]              // 8 bytes - big-endian u64
/// [expires_at:8]                 // 8 bytes - big-endian u64
/// [executed_at:8]                // 8 bytes - big-endian u64
/// [approval_count:4]             // 4 bytes - big-endian u32
/// [approvals:32*n]               // 32*n bytes - sorted addresses
/// [data_len:4]                   // 4 bytes - big-endian u32
/// [proposal_data:data_len]       // variable - proposal data
/// ```
///
/// Total fixed: 1 + 32 + 1 + 32 + 32 + 1 + 8*5 + 4 + 4 = 147 bytes + variable
#[must_use]
pub fn encode_proposal_v1(proposal: &Proposal) -> Vec<u8> {
    // Sort approvals for deterministic encoding
    let mut sorted_approvals = proposal.approvals.clone();
    sorted_approvals.sort_unstable();

    let data_len = proposal.proposal_data.len();
    let approval_count = sorted_approvals.len();
    let total_len = 147 + (approval_count * 32) + data_len;

    let mut out = Vec::with_capacity(total_len);

    // Version
    out.push(PROPOSAL_CODEC_V1);

    // ID
    out.extend_from_slice(&proposal.id);

    // Proposal type
    out.push(proposal.proposal_type.to_byte());

    // Proposer
    out.extend_from_slice(&proposal.proposer);

    // Gate ID
    out.extend_from_slice(&proposal.gate_id);

    // State
    out.push(proposal.state.to_byte());

    // Timestamps (big-endian)
    out.extend_from_slice(&proposal.submitted_at.to_be_bytes());
    out.extend_from_slice(&proposal.approved_at.to_be_bytes());
    out.extend_from_slice(&proposal.executable_at.to_be_bytes());
    out.extend_from_slice(&proposal.expires_at.to_be_bytes());
    out.extend_from_slice(&proposal.executed_at.to_be_bytes());

    // Approval count and sorted approvals
    #[allow(clippy::cast_possible_truncation)]
    let approval_count_u32 = approval_count as u32;
    out.extend_from_slice(&approval_count_u32.to_be_bytes());
    for approver in &sorted_approvals {
        out.extend_from_slice(approver);
    }

    // Data length and data
    #[allow(clippy::cast_possible_truncation)]
    let data_len_u32 = data_len as u32;
    out.extend_from_slice(&data_len_u32.to_be_bytes());
    out.extend_from_slice(&proposal.proposal_data);

    out
}

/// Decode a proposal from canonical bytes.
///
/// # Errors
///
/// Returns error if data is malformed, truncated, or has invalid values.
pub fn decode_proposal_v1(bytes: &[u8]) -> Result<Proposal, CodecError> {
    const MIN_LEN: usize = 147; // Fixed portion without approvals or data

    if bytes.len() < MIN_LEN {
        return Err(CodecError::TooShort {
            expected: MIN_LEN,
            got: bytes.len(),
        });
    }

    let mut pos = 0;

    // Version
    let version = bytes[pos];
    if version != PROPOSAL_CODEC_V1 {
        return Err(CodecError::InvalidVersion {
            expected: PROPOSAL_CODEC_V1,
            got: version,
        });
    }
    pos += 1;

    // ID
    let mut id: ProposalId = [0u8; 32];
    id.copy_from_slice(&bytes[pos..pos + 32]);
    pos += 32;

    // Proposal type
    let proposal_type = ProposalType::from_byte(bytes[pos])
        .ok_or(CodecError::InvalidProposalType { value: bytes[pos] })?;
    pos += 1;

    // Proposer
    let mut proposer: Address = [0u8; 32];
    proposer.copy_from_slice(&bytes[pos..pos + 32]);
    pos += 32;

    // Gate ID
    let mut gate_id: [u8; 32] = [0u8; 32];
    gate_id.copy_from_slice(&bytes[pos..pos + 32]);
    pos += 32;

    // State
    let state = ProposalState::from_byte(bytes[pos])
        .ok_or(CodecError::InvalidProposalState { value: bytes[pos] })?;
    pos += 1;

    // Timestamps
    let mut u64_buf = [0u8; 8];

    u64_buf.copy_from_slice(&bytes[pos..pos + 8]);
    let submitted_at = u64::from_be_bytes(u64_buf);
    pos += 8;

    u64_buf.copy_from_slice(&bytes[pos..pos + 8]);
    let approved_at = u64::from_be_bytes(u64_buf);
    pos += 8;

    u64_buf.copy_from_slice(&bytes[pos..pos + 8]);
    let executable_at = u64::from_be_bytes(u64_buf);
    pos += 8;

    u64_buf.copy_from_slice(&bytes[pos..pos + 8]);
    let expires_at = u64::from_be_bytes(u64_buf);
    pos += 8;

    u64_buf.copy_from_slice(&bytes[pos..pos + 8]);
    let executed_at = u64::from_be_bytes(u64_buf);
    pos += 8;

    // Approval count
    let mut u32_buf = [0u8; 4];
    u32_buf.copy_from_slice(&bytes[pos..pos + 4]);
    let approval_count = u32::from_be_bytes(u32_buf) as usize;
    pos += 4;

    // Validate remaining length
    let expected_remaining = (approval_count * 32) + 4; // approvals + data_len field
    if bytes.len() < pos + expected_remaining {
        return Err(CodecError::TooShort {
            expected: pos + expected_remaining,
            got: bytes.len(),
        });
    }

    // Approvals
    let mut approvals = Vec::with_capacity(approval_count);
    for _ in 0..approval_count {
        let mut approver: Address = [0u8; 32];
        approver.copy_from_slice(&bytes[pos..pos + 32]);
        approvals.push(approver);
        pos += 32;
    }

    // Data length
    u32_buf.copy_from_slice(&bytes[pos..pos + 4]);
    let data_len = u32::from_be_bytes(u32_buf) as usize;
    pos += 4;

    // Validate data length
    if bytes.len() < pos + data_len {
        return Err(CodecError::TooShort {
            expected: pos + data_len,
            got: bytes.len(),
        });
    }

    // Check for exact length match
    if bytes.len() != pos + data_len {
        return Err(CodecError::LengthMismatch {
            declared: pos + data_len,
            actual: bytes.len(),
        });
    }

    // Proposal data
    let proposal_data = bytes[pos..pos + data_len].to_vec();

    Ok(Proposal {
        id,
        proposal_type,
        proposal_data,
        proposer,
        gate_id,
        state,
        submitted_at,
        approved_at,
        executable_at,
        expires_at,
        executed_at,
        approvals,
    })
}

// ============================================================================
// AUDIT LOG ENCODING (D19.5)
// ============================================================================

/// Encode an audit log entry to canonical bytes.
///
/// Format (V1):
/// ```text
/// [version:1]                    // 1 byte - AUDIT_LOG_CODEC_V1
/// [proposal_id:32]               // 32 bytes - proposal ID
/// [action:1]                     // 1 byte - AuditAction
/// [block_height:8]               // 8 bytes - big-endian u64
/// [has_actor:1]                  // 1 byte - 0 or 1
/// [actor:32]                     // 32 bytes - actor address (only if has_actor=1)
/// [details_len:4]                // 4 bytes - big-endian u32
/// [details:details_len]          // variable - details bytes
/// ```
///
/// Total fixed (with actor): 1 + 32 + 1 + 8 + 1 + 32 + 4 = 79 bytes + details
/// Total fixed (no actor): 1 + 32 + 1 + 8 + 1 + 4 = 47 bytes + details
#[must_use]
pub fn encode_audit_log_v1(entry: &AuditLogEntry) -> Vec<u8> {
    let has_actor = entry.actor.is_some();
    let actor_len = if has_actor { 32 } else { 0 };
    let total_len = 1 + 32 + 1 + 8 + 1 + actor_len + 4 + entry.details.len();

    let mut out = Vec::with_capacity(total_len);

    // Version
    out.push(AUDIT_LOG_CODEC_V1);

    // Proposal ID
    out.extend_from_slice(&entry.proposal_id);

    // Action
    out.push(entry.action.to_byte());

    // Block height (big-endian)
    out.extend_from_slice(&entry.block_height.to_be_bytes());

    // Actor presence flag and optional actor
    if let Some(actor) = &entry.actor {
        out.push(1);
        out.extend_from_slice(actor);
    } else {
        out.push(0);
    }

    // Details length and data
    #[allow(clippy::cast_possible_truncation)]
    let details_len = entry.details.len() as u32;
    out.extend_from_slice(&details_len.to_be_bytes());
    out.extend_from_slice(&entry.details);

    out
}

/// Decode an audit log entry from canonical bytes.
///
/// # Errors
///
/// Returns error if data is malformed, truncated, or has invalid values.
pub fn decode_audit_log_v1(bytes: &[u8]) -> Result<AuditLogEntry, CodecError> {
    const MIN_LEN_NO_ACTOR: usize = 1 + 32 + 1 + 8 + 1 + 4; // 47 bytes
    const MIN_LEN_WITH_ACTOR: usize = MIN_LEN_NO_ACTOR + 32; // 79 bytes

    if bytes.len() < MIN_LEN_NO_ACTOR {
        return Err(CodecError::TooShort {
            expected: MIN_LEN_NO_ACTOR,
            got: bytes.len(),
        });
    }

    let mut pos = 0;

    // Version
    let version = bytes[pos];
    if version != AUDIT_LOG_CODEC_V1 {
        return Err(CodecError::InvalidVersion {
            expected: AUDIT_LOG_CODEC_V1,
            got: version,
        });
    }
    pos += 1;

    // Proposal ID
    let mut proposal_id: ProposalId = [0u8; 32];
    proposal_id.copy_from_slice(&bytes[pos..pos + 32]);
    pos += 32;

    // Action
    let action = AuditAction::from_byte(bytes[pos])
        .ok_or(CodecError::InvalidAuditAction { value: bytes[pos] })?;
    pos += 1;

    // Block height
    let mut u64_buf = [0u8; 8];
    u64_buf.copy_from_slice(&bytes[pos..pos + 8]);
    let block_height = u64::from_be_bytes(u64_buf);
    pos += 8;

    // Actor presence
    let has_actor = bytes[pos] != 0;
    pos += 1;

    // Check remaining length based on actor presence
    let expected_min = if has_actor {
        MIN_LEN_WITH_ACTOR
    } else {
        MIN_LEN_NO_ACTOR
    };
    if bytes.len() < expected_min {
        return Err(CodecError::TooShort {
            expected: expected_min,
            got: bytes.len(),
        });
    }

    // Actor (if present)
    let actor = if has_actor {
        let mut actor_bytes: Address = [0u8; 32];
        actor_bytes.copy_from_slice(&bytes[pos..pos + 32]);
        pos += 32;
        Some(actor_bytes)
    } else {
        None
    };

    // Details length
    let mut u32_buf = [0u8; 4];
    u32_buf.copy_from_slice(&bytes[pos..pos + 4]);
    let details_len = u32::from_be_bytes(u32_buf) as usize;
    pos += 4;

    // Validate details length
    if bytes.len() < pos + details_len {
        return Err(CodecError::TooShort {
            expected: pos + details_len,
            got: bytes.len(),
        });
    }

    // Check for exact length match
    if bytes.len() != pos + details_len {
        return Err(CodecError::LengthMismatch {
            declared: pos + details_len,
            actual: bytes.len(),
        });
    }

    // Details
    let details = bytes[pos..pos + details_len].to_vec();

    Ok(AuditLogEntry {
        proposal_id,
        action,
        block_height,
        actor,
        details,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_proposer() -> Address {
        [0x01u8; 32]
    }

    fn test_gate_id() -> [u8; 32] {
        [0x42u8; 32]
    }

    #[test]
    fn encode_decode_roundtrip_empty_proposal() {
        let proposal = Proposal::new(
            ProposalType::ParamChange,
            b"MIN_FEE=100".to_vec(),
            test_proposer(),
            test_gate_id(),
            1000,
            500,
        );

        let encoded = encode_proposal_v1(&proposal);
        let decoded = decode_proposal_v1(&encoded).unwrap();

        assert_eq!(decoded.id, proposal.id);
        assert_eq!(decoded.proposal_type, proposal.proposal_type);
        assert_eq!(decoded.proposal_data, proposal.proposal_data);
        assert_eq!(decoded.proposer, proposal.proposer);
        assert_eq!(decoded.gate_id, proposal.gate_id);
        assert_eq!(decoded.state, proposal.state);
        assert_eq!(decoded.submitted_at, proposal.submitted_at);
        assert_eq!(decoded.expires_at, proposal.expires_at);
        assert_eq!(decoded.approvals.len(), 0);
    }

    #[test]
    fn encode_decode_roundtrip_with_approvals() {
        let mut proposal = Proposal::new(
            ProposalType::ModuleActivation,
            b"activate:module_v2".to_vec(),
            test_proposer(),
            test_gate_id(),
            500,
            1000,
        );

        // Add approvals in non-sorted order
        proposal.add_approval([0xCCu8; 32]);
        proposal.add_approval([0xAAu8; 32]);
        proposal.add_approval([0xBBu8; 32]);

        proposal.approve(600, 100).unwrap();

        let encoded = encode_proposal_v1(&proposal);
        let decoded = decode_proposal_v1(&encoded).unwrap();

        assert_eq!(decoded.state, ProposalState::Approved);
        assert_eq!(decoded.approved_at, 600);
        assert_eq!(decoded.executable_at, 700);

        // Approvals should be sorted
        assert_eq!(decoded.approvals.len(), 3);
        assert_eq!(decoded.approvals[0], [0xAAu8; 32]);
        assert_eq!(decoded.approvals[1], [0xBBu8; 32]);
        assert_eq!(decoded.approvals[2], [0xCCu8; 32]);
    }

    #[test]
    fn encode_decode_all_proposal_types() {
        for pt in [
            ProposalType::ParamChange,
            ProposalType::ModuleActivation,
            ProposalType::ModuleRollback,
            ProposalType::PolicyChange,
            ProposalType::EmergencyFreeze,
        ] {
            let proposal = Proposal::new(
                pt,
                b"test".to_vec(),
                test_proposer(),
                test_gate_id(),
                100,
                500,
            );

            let encoded = encode_proposal_v1(&proposal);
            let decoded = decode_proposal_v1(&encoded).unwrap();

            assert_eq!(decoded.proposal_type, pt);
        }
    }

    #[test]
    fn encode_decode_all_states() {
        let mut proposal = Proposal::new(
            ProposalType::ParamChange,
            b"test".to_vec(),
            test_proposer(),
            test_gate_id(),
            100,
            1000,
        );

        // Test Submitted
        let encoded = encode_proposal_v1(&proposal);
        let decoded = decode_proposal_v1(&encoded).unwrap();
        assert_eq!(decoded.state, ProposalState::Submitted);

        // Test Approved
        proposal.approve(150, 50).unwrap();
        let encoded = encode_proposal_v1(&proposal);
        let decoded = decode_proposal_v1(&encoded).unwrap();
        assert_eq!(decoded.state, ProposalState::Approved);

        // Test Executable
        proposal.make_executable(200).unwrap();
        let encoded = encode_proposal_v1(&proposal);
        let decoded = decode_proposal_v1(&encoded).unwrap();
        assert_eq!(decoded.state, ProposalState::Executable);

        // Test Executed
        proposal.execute(250).unwrap();
        let encoded = encode_proposal_v1(&proposal);
        let decoded = decode_proposal_v1(&encoded).unwrap();
        assert_eq!(decoded.state, ProposalState::Executed);
    }

    #[test]
    fn decode_too_short_fails() {
        let result = decode_proposal_v1(&[0x01; 100]);
        assert!(matches!(result, Err(CodecError::TooShort { .. })));
    }

    #[test]
    fn decode_invalid_version_fails() {
        let proposal = Proposal::new(
            ProposalType::ParamChange,
            b"test".to_vec(),
            test_proposer(),
            test_gate_id(),
            100,
            500,
        );

        let mut encoded = encode_proposal_v1(&proposal);
        encoded[0] = 0xFF; // Invalid version

        let result = decode_proposal_v1(&encoded);
        assert!(matches!(
            result,
            Err(CodecError::InvalidVersion {
                expected: 1,
                got: 0xFF
            })
        ));
    }

    #[test]
    fn decode_invalid_proposal_type_fails() {
        let proposal = Proposal::new(
            ProposalType::ParamChange,
            b"test".to_vec(),
            test_proposer(),
            test_gate_id(),
            100,
            500,
        );

        let mut encoded = encode_proposal_v1(&proposal);
        encoded[33] = 0xFF; // Invalid proposal type at offset 33 (after version + id)

        let result = decode_proposal_v1(&encoded);
        assert!(matches!(
            result,
            Err(CodecError::InvalidProposalType { value: 0xFF })
        ));
    }

    #[test]
    fn decode_invalid_state_fails() {
        let proposal = Proposal::new(
            ProposalType::ParamChange,
            b"test".to_vec(),
            test_proposer(),
            test_gate_id(),
            100,
            500,
        );

        let mut encoded = encode_proposal_v1(&proposal);
        // State is at offset: 1 + 32 + 1 + 32 + 32 = 98
        encoded[98] = 0xFF; // Invalid state

        let result = decode_proposal_v1(&encoded);
        assert!(matches!(
            result,
            Err(CodecError::InvalidProposalState { value: 0xFF })
        ));
    }

    #[test]
    fn golden_vector_proposal_encoding() {
        // Create a deterministic proposal for golden vector test
        let proposal = Proposal {
            id: [0x11u8; 32],
            proposal_type: ProposalType::ParamChange,
            proposal_data: b"MIN_FEE=50".to_vec(),
            proposer: [0x01u8; 32],
            gate_id: [0x42u8; 32],
            state: ProposalState::Submitted,
            submitted_at: 1000,
            approved_at: 0,
            executable_at: 0,
            expires_at: 1500,
            executed_at: 0,
            approvals: vec![],
        };

        let encoded = encode_proposal_v1(&proposal);

        // Golden vector - lock the encoding format
        // Format: version(1) + id(32) + type(1) + proposer(32) + gate_id(32) + state(1)
        //         + timestamps(40) + approval_count(4) + data_len(4) + data(10)
        // Total: 157 bytes for this proposal
        assert_eq!(encoded.len(), 157, "Encoded length must be exactly 157");

        // Check version
        assert_eq!(encoded[0], PROPOSAL_CODEC_V1, "Version must be 1");

        // Check ID (bytes 1-32)
        assert_eq!(&encoded[1..33], &[0x11u8; 32], "ID must match");

        // Check proposal type (byte 33)
        assert_eq!(encoded[33], 0, "Type must be ParamChange (0)");

        // Check proposer (bytes 34-65)
        assert_eq!(&encoded[34..66], &[0x01u8; 32], "Proposer must match");

        // Check gate_id (bytes 66-97)
        assert_eq!(&encoded[66..98], &[0x42u8; 32], "Gate ID must match");

        // Check state (byte 98)
        assert_eq!(encoded[98], 0, "State must be Submitted (0)");

        // Check submitted_at (bytes 99-106) = 1000 in big-endian
        assert_eq!(
            &encoded[99..107],
            &1000u64.to_be_bytes(),
            "submitted_at must be 1000"
        );

        // Check expires_at (bytes 123-130) = 1500 in big-endian
        // offset: 99 + 8 + 8 + 8 = 123
        assert_eq!(
            &encoded[123..131],
            &1500u64.to_be_bytes(),
            "expires_at must be 1500"
        );

        // Check approval_count (bytes 139-142) = 0
        // offset: 99 + 40 = 139
        assert_eq!(
            &encoded[139..143],
            &0u32.to_be_bytes(),
            "approval_count must be 0"
        );

        // Check data_len (bytes 143-146) = 10
        assert_eq!(
            &encoded[143..147],
            &10u32.to_be_bytes(),
            "data_len must be 10"
        );

        // Check data (bytes 147-156)
        assert_eq!(
            &encoded[147..157],
            b"MIN_FEE=50",
            "proposal_data must match"
        );

        // Verify roundtrip
        let decoded = decode_proposal_v1(&encoded).unwrap();
        assert_eq!(decoded.id, proposal.id);
        assert_eq!(decoded.proposal_type, proposal.proposal_type);
        assert_eq!(decoded.proposal_data, proposal.proposal_data);
        assert_eq!(decoded.submitted_at, 1000);
        assert_eq!(decoded.expires_at, 1500);
    }

    #[test]
    fn approvals_are_sorted_in_encoding() {
        let mut proposal = Proposal::new(
            ProposalType::ParamChange,
            b"test".to_vec(),
            test_proposer(),
            test_gate_id(),
            100,
            500,
        );

        // Add in reverse order
        proposal.add_approval([0xFFu8; 32]);
        proposal.add_approval([0x00u8; 32]);
        proposal.add_approval([0x88u8; 32]);

        let encoded = encode_proposal_v1(&proposal);
        let decoded = decode_proposal_v1(&encoded).unwrap();

        // Should be sorted
        assert!(decoded.approvals[0] < decoded.approvals[1]);
        assert!(decoded.approvals[1] < decoded.approvals[2]);
    }

    #[test]
    fn encoding_is_deterministic() {
        let proposal = Proposal::new(
            ProposalType::ParamChange,
            b"deterministic".to_vec(),
            test_proposer(),
            test_gate_id(),
            100,
            500,
        );

        let encoded1 = encode_proposal_v1(&proposal);
        let encoded2 = encode_proposal_v1(&proposal);

        assert_eq!(encoded1, encoded2, "Encoding must be deterministic");
    }

    // ========================================================================
    // AUDIT LOG TESTS
    // ========================================================================

    fn test_proposal_id() -> ProposalId {
        [0x11u8; 32]
    }

    #[test]
    fn audit_log_roundtrip_with_actor() {
        use crate::AuditLogEntry;

        let entry = AuditLogEntry::submitted(test_proposal_id(), 1000, test_proposer());

        let encoded = encode_audit_log_v1(&entry);
        let decoded = decode_audit_log_v1(&encoded).unwrap();

        assert_eq!(decoded.proposal_id, entry.proposal_id);
        assert_eq!(decoded.action, entry.action);
        assert_eq!(decoded.block_height, entry.block_height);
        assert_eq!(decoded.actor, entry.actor);
        assert_eq!(decoded.details, entry.details);
    }

    #[test]
    fn audit_log_roundtrip_without_actor() {
        use crate::AuditLogEntry;

        let entry = AuditLogEntry::expired(test_proposal_id(), 5000);

        let encoded = encode_audit_log_v1(&entry);
        let decoded = decode_audit_log_v1(&encoded).unwrap();

        assert_eq!(decoded.proposal_id, entry.proposal_id);
        assert_eq!(decoded.action, crate::AuditAction::Expired);
        assert_eq!(decoded.block_height, 5000);
        assert_eq!(decoded.actor, None);
        assert_eq!(decoded.details.len(), 0);
    }

    #[test]
    fn audit_log_roundtrip_with_details() {
        use crate::{AuditAction, AuditLogEntry};

        let entry = AuditLogEntry::new(
            test_proposal_id(),
            AuditAction::Executed,
            2500,
            Some(test_proposer()),
            b"execution_result:success".to_vec(),
        );

        let encoded = encode_audit_log_v1(&entry);
        let decoded = decode_audit_log_v1(&encoded).unwrap();

        assert_eq!(decoded.details, b"execution_result:success".to_vec());
    }

    #[test]
    fn audit_log_all_actions() {
        use crate::{AuditAction, AuditLogEntry};

        for action in [
            AuditAction::Submitted,
            AuditAction::Approved,
            AuditAction::Executed,
            AuditAction::Rejected,
            AuditAction::Expired,
        ] {
            let entry = AuditLogEntry::new(
                test_proposal_id(),
                action,
                1000,
                Some(test_proposer()),
                Vec::new(),
            );

            let encoded = encode_audit_log_v1(&entry);
            let decoded = decode_audit_log_v1(&encoded).unwrap();

            assert_eq!(decoded.action, action);
        }
    }

    #[test]
    fn audit_log_decode_invalid_version() {
        use crate::AuditLogEntry;

        let entry = AuditLogEntry::submitted(test_proposal_id(), 1000, test_proposer());
        let mut encoded = encode_audit_log_v1(&entry);
        encoded[0] = 0xFF;

        let result = decode_audit_log_v1(&encoded);
        assert!(matches!(
            result,
            Err(CodecError::InvalidVersion {
                expected: 1,
                got: 0xFF
            })
        ));
    }

    #[test]
    fn audit_log_decode_invalid_action() {
        use crate::AuditLogEntry;

        let entry = AuditLogEntry::submitted(test_proposal_id(), 1000, test_proposer());
        let mut encoded = encode_audit_log_v1(&entry);
        // Action is at offset 1 + 32 = 33
        encoded[33] = 0xFF;

        let result = decode_audit_log_v1(&encoded);
        assert!(matches!(
            result,
            Err(CodecError::InvalidAuditAction { value: 0xFF })
        ));
    }

    #[test]
    fn audit_log_decode_too_short() {
        let result = decode_audit_log_v1(&[0x01; 20]);
        assert!(matches!(result, Err(CodecError::TooShort { .. })));
    }

    #[test]
    fn audit_log_golden_vector() {
        use crate::{AuditAction, AuditLogEntry};

        // Create a deterministic entry for golden vector test
        let entry = AuditLogEntry {
            proposal_id: [0x11u8; 32],
            action: AuditAction::Submitted,
            block_height: 1000,
            actor: Some([0x01u8; 32]),
            details: Vec::new(),
        };

        let encoded = encode_audit_log_v1(&entry);

        // Golden vector - lock the encoding format
        // Format: version(1) + id(32) + action(1) + height(8) + has_actor(1) + actor(32) + details_len(4)
        // Total: 79 bytes for this entry (no details)
        assert_eq!(encoded.len(), 79, "Encoded length must be exactly 79");

        // Check version
        assert_eq!(encoded[0], AUDIT_LOG_CODEC_V1, "Version must be 1");

        // Check proposal ID (bytes 1-32)
        assert_eq!(&encoded[1..33], &[0x11u8; 32], "Proposal ID must match");

        // Check action (byte 33)
        assert_eq!(encoded[33], 0, "Action must be Submitted (0)");

        // Check block_height (bytes 34-41) = 1000 in big-endian
        assert_eq!(
            &encoded[34..42],
            &1000u64.to_be_bytes(),
            "block_height must be 1000"
        );

        // Check has_actor (byte 42)
        assert_eq!(encoded[42], 1, "has_actor must be 1");

        // Check actor (bytes 43-74)
        assert_eq!(&encoded[43..75], &[0x01u8; 32], "Actor must match");

        // Check details_len (bytes 75-78) = 0
        assert_eq!(
            &encoded[75..79],
            &0u32.to_be_bytes(),
            "details_len must be 0"
        );

        // Verify roundtrip
        let decoded = decode_audit_log_v1(&encoded).unwrap();
        assert_eq!(decoded.proposal_id, entry.proposal_id);
        assert_eq!(decoded.action, entry.action);
        assert_eq!(decoded.block_height, entry.block_height);
        assert_eq!(decoded.actor, entry.actor);
    }

    #[test]
    fn audit_log_encoding_is_deterministic() {
        use crate::AuditLogEntry;

        let entry = AuditLogEntry::submitted(test_proposal_id(), 1000, test_proposer());

        let encoded1 = encode_audit_log_v1(&entry);
        let encoded2 = encode_audit_log_v1(&entry);

        assert_eq!(encoded1, encoded2, "Encoding must be deterministic");
    }
}
