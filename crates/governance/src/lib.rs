//! Governance types for NOVAI protocol.
//!
//! PURPOSE: Define proposal types, lifecycle states, and governance primitives
//! for transparent, rule-based upgrades including AI autonomy changes.
//!
//! INVARIANTS:
//! - All proposal IDs are deterministically computed from content
//! - State transitions follow strict lifecycle rules
//! - Timelock enforcement is height-based (deterministic)
//! - No automatic execution - all proposals require explicit execute call
//!
//! FAILURE MODES:
//! - Invalid state transitions are rejected
//! - Expired proposals cannot be executed
//! - Early execution (before timelock) is rejected

pub mod codec;

pub use codec::{
    decode_audit_log_v1, decode_proposal_v1, encode_audit_log_v1, encode_proposal_v1, CodecError,
    AUDIT_LOG_CODEC_V1, PROPOSAL_CODEC_V1,
};

use blake3::Hasher;
use novai_types::Address;

// ============================================================================
// GOVERNANCE CONFIGURATION (D19.4)
// ============================================================================

/// Configuration for governance timelock and expiry policies.
///
/// Different proposal types have different risk profiles and thus different
/// timelock requirements. Emergency proposals have shorter timelocks but are
/// limited in scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceConfig {
    /// Default timelock for standard proposals (e.g., `ParamChange`, `ModuleRollback`).
    /// Number of blocks between approval and executability.
    pub default_timelock_blocks: u64,

    /// Extended timelock for high-risk proposals (`ModuleActivation`, `PolicyChange`).
    /// These require more review time due to their impact on AI behavior.
    pub high_risk_timelock_blocks: u64,

    /// Shortened timelock for emergency proposals (`EmergencyFreeze`).
    /// Must be long enough for review but short enough to act quickly.
    pub emergency_timelock_blocks: u64,

    /// Default expiry: blocks after submission before proposal expires.
    /// Prevents stale proposals from lingering indefinitely.
    pub default_expiry_blocks: u64,
}

impl GovernanceConfig {
    /// Create a new governance configuration.
    #[must_use]
    pub const fn new(
        default_timelock_blocks: u64,
        high_risk_timelock_blocks: u64,
        emergency_timelock_blocks: u64,
        default_expiry_blocks: u64,
    ) -> Self {
        Self {
            default_timelock_blocks,
            high_risk_timelock_blocks,
            emergency_timelock_blocks,
            default_expiry_blocks,
        }
    }

    /// Get the timelock for a specific proposal type.
    ///
    /// - Emergency proposals: `emergency_timelock_blocks`
    /// - High-risk proposals: `high_risk_timelock_blocks`
    /// - Standard proposals: `default_timelock_blocks`
    #[must_use]
    pub const fn timelock_for_proposal_type(&self, proposal_type: ProposalType) -> u64 {
        if proposal_type.is_emergency() {
            self.emergency_timelock_blocks
        } else if proposal_type.is_high_risk() {
            self.high_risk_timelock_blocks
        } else {
            self.default_timelock_blocks
        }
    }
}

impl Default for GovernanceConfig {
    /// Default configuration with reasonable values for a production network.
    ///
    /// - Default timelock: 1000 blocks (~2.7 hours at 10s blocks)
    /// - High-risk timelock: 5000 blocks (~13.9 hours at 10s blocks)
    /// - Emergency timelock: 100 blocks (~16.7 minutes at 10s blocks)
    /// - Default expiry: 50000 blocks (~5.8 days at 10s blocks)
    fn default() -> Self {
        Self {
            default_timelock_blocks: 1000,
            high_risk_timelock_blocks: 5000,
            emergency_timelock_blocks: 100,
            default_expiry_blocks: 50000,
        }
    }
}

/// Get the timelock blocks for a proposal type given a config.
///
/// Convenience function that delegates to `GovernanceConfig::timelock_for_proposal_type`.
#[must_use]
pub const fn get_timelock_for_proposal_type(
    proposal_type: ProposalType,
    config: &GovernanceConfig,
) -> u64 {
    config.timelock_for_proposal_type(proposal_type)
}

// ============================================================================
// AUDIT LOG TYPES (D19.5)
// ============================================================================

/// Action recorded in the governance audit log.
///
/// Each state transition in the proposal lifecycle is logged for transparency.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditAction {
    /// Proposal was submitted.
    Submitted = 0,

    /// Proposal was approved (reached threshold).
    Approved = 1,

    /// Proposal was executed.
    Executed = 2,

    /// Proposal was rejected.
    Rejected = 3,

    /// Proposal expired without execution.
    Expired = 4,
}

impl AuditAction {
    /// Encode to canonical byte representation.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        self as u8
    }

    /// Decode from byte, returning None for invalid values.
    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Submitted),
            1 => Some(Self::Approved),
            2 => Some(Self::Executed),
            3 => Some(Self::Rejected),
            4 => Some(Self::Expired),
            _ => None,
        }
    }
}

/// An entry in the governance audit log.
///
/// Records a significant action taken on a proposal, including who triggered it
/// and when. Used for transparency and debugging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditLogEntry {
    /// The proposal this action relates to.
    pub proposal_id: ProposalId,

    /// The action that occurred.
    pub action: AuditAction,

    /// Block height when the action occurred.
    pub block_height: u64,

    /// Address that triggered the action, if applicable.
    /// - `Some(addr)` for user-initiated actions (submit, approve, execute, reject)
    /// - `None` for system-initiated actions (automatic expiry)
    pub actor: Option<Address>,

    /// Optional serialized context/details.
    /// Format depends on action type.
    pub details: Vec<u8>,
}

impl AuditLogEntry {
    /// Create a new audit log entry.
    #[must_use]
    pub const fn new(
        proposal_id: ProposalId,
        action: AuditAction,
        block_height: u64,
        actor: Option<Address>,
        details: Vec<u8>,
    ) -> Self {
        Self {
            proposal_id,
            action,
            block_height,
            actor,
            details,
        }
    }

    /// Create an entry for a submitted proposal.
    #[must_use]
    pub const fn submitted(proposal_id: ProposalId, block_height: u64, submitter: Address) -> Self {
        Self::new(
            proposal_id,
            AuditAction::Submitted,
            block_height,
            Some(submitter),
            Vec::new(),
        )
    }

    /// Create an entry for an approved proposal.
    #[must_use]
    pub const fn approved(proposal_id: ProposalId, block_height: u64, approver: Address) -> Self {
        Self::new(
            proposal_id,
            AuditAction::Approved,
            block_height,
            Some(approver),
            Vec::new(),
        )
    }

    /// Create an entry for an executed proposal.
    #[must_use]
    pub const fn executed(proposal_id: ProposalId, block_height: u64, executor: Address) -> Self {
        Self::new(
            proposal_id,
            AuditAction::Executed,
            block_height,
            Some(executor),
            Vec::new(),
        )
    }

    /// Create an entry for a rejected proposal.
    #[must_use]
    pub const fn rejected(proposal_id: ProposalId, block_height: u64, rejector: Address) -> Self {
        Self::new(
            proposal_id,
            AuditAction::Rejected,
            block_height,
            Some(rejector),
            Vec::new(),
        )
    }

    /// Create an entry for an expired proposal (system-initiated, no actor).
    #[must_use]
    pub const fn expired(proposal_id: ProposalId, block_height: u64) -> Self {
        Self::new(
            proposal_id,
            AuditAction::Expired,
            block_height,
            None,
            Vec::new(),
        )
    }
}

/// Domain separator for proposal ID computation.
const PROPOSAL_ID_DOMAIN: &[u8] = b"NOVAI_PROPOSAL_ID_V1";

/// Domain separator for approval digest computation.
/// Used to cryptographically bind an approver to a specific proposal.
const APPROVAL_DIGEST_DOMAIN: &[u8] = b"NOVAI_APPROVAL_V1";

/// Unique identifier for a governance proposal.
pub type ProposalId = [u8; 32];

/// Type of governance proposal.
///
/// Each type has different requirements and effects:
/// - `ParamChange`: Modify protocol parameters (fees, limits, etc.)
/// - `ModuleActivation`: Activate a registered AI module
/// - `ModuleRollback`: Rollback to a previous module version
/// - `PolicyChange`: Modify Tier 2 policies
/// - `EmergencyFreeze`: Halt AI execution immediately
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProposalType {
    /// Change protocol parameters (`MIN_FEE`, `BLOCK_SIZE_LIMIT`, etc.).
    ParamChange = 0,

    /// Activate a registered AI module (requires module manifest).
    ModuleActivation = 1,

    /// Rollback to a previous module version.
    ModuleRollback = 2,

    /// Change Tier 2 policies (AI execution rules).
    PolicyChange = 3,

    /// Emergency halt of AI execution (fast-track, shorter timelock).
    EmergencyFreeze = 4,
}

impl ProposalType {
    /// Encode to canonical byte representation.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        self as u8
    }

    /// Decode from byte, returning None for invalid values.
    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::ParamChange),
            1 => Some(Self::ModuleActivation),
            2 => Some(Self::ModuleRollback),
            3 => Some(Self::PolicyChange),
            4 => Some(Self::EmergencyFreeze),
            _ => None,
        }
    }

    /// Returns true if this proposal type is considered high-risk
    /// and requires longer timelocks.
    #[must_use]
    pub const fn is_high_risk(self) -> bool {
        matches!(self, Self::ModuleActivation | Self::PolicyChange)
    }

    /// Returns true if this is an emergency proposal with shortened timelock.
    #[must_use]
    pub const fn is_emergency(self) -> bool {
        matches!(self, Self::EmergencyFreeze)
    }
}

/// Lifecycle state of a governance proposal.
///
/// State machine transitions:
/// ```text
/// Submitted -> Approved -> Executable -> Executed
///     |           |            |
///     v           v            v
///  Rejected    Rejected     Expired
/// ```
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProposalState {
    /// Proposal exists, awaiting approvals from gate.
    Submitted = 0,

    /// Sufficient approvals received, timelock countdown started.
    Approved = 1,

    /// Timelock elapsed, proposal can be executed.
    Executable = 2,

    /// Proposal has been executed successfully.
    Executed = 3,

    /// Proposal expired before execution.
    Expired = 4,

    /// Proposal was explicitly rejected or vetoed.
    Rejected = 5,
}

impl ProposalState {
    /// Encode to canonical byte representation.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        self as u8
    }

    /// Decode from byte, returning None for invalid values.
    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Submitted),
            1 => Some(Self::Approved),
            2 => Some(Self::Executable),
            3 => Some(Self::Executed),
            4 => Some(Self::Expired),
            5 => Some(Self::Rejected),
            _ => None,
        }
    }

    /// Returns true if this is a terminal state (no further transitions).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Executed | Self::Expired | Self::Rejected)
    }

    /// Returns true if the proposal can still be approved.
    #[must_use]
    pub const fn can_approve(self) -> bool {
        matches!(self, Self::Submitted)
    }

    /// Returns true if the proposal can be executed.
    #[must_use]
    pub const fn can_execute(self) -> bool {
        matches!(self, Self::Executable)
    }
}

/// A governance proposal.
///
/// Proposals go through a lifecycle: Submitted -> Approved -> Executable -> Executed.
/// Approvals are collected through the associated gate.
/// Execution is blocked until timelock elapses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// Unique proposal identifier (blake3 hash of content).
    pub id: ProposalId,

    /// Type of proposal.
    pub proposal_type: ProposalType,

    /// Encoded proposal data (interpretation depends on `proposal_type`).
    pub proposal_data: Vec<u8>,

    /// Address that submitted the proposal.
    pub proposer: Address,

    /// Gate ID that must approve this proposal.
    pub gate_id: [u8; 32],

    /// Current lifecycle state.
    pub state: ProposalState,

    /// Block height when proposal was submitted.
    pub submitted_at: u64,

    /// Block height when proposal was approved (0 if not approved).
    pub approved_at: u64,

    /// Block height when proposal becomes executable (0 if not approved).
    /// Computed as: `approved_at + gate.timelock_blocks`
    pub executable_at: u64,

    /// Block height when proposal expires (cannot execute after this).
    /// Computed as: `submitted_at + gate.expiry_blocks`
    pub expires_at: u64,

    /// Block height when proposal was executed (0 if not executed).
    pub executed_at: u64,

    /// Addresses that have approved this proposal.
    pub approvals: Vec<Address>,
}

impl Proposal {
    /// Compute proposal ID from proposal content.
    ///
    /// Uses domain-separated blake3 hashing:
    /// `blake3(DOMAIN || type || proposer || gate_id || data_len || data)`
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // proposal_data will never exceed 4GB
    pub fn compute_id(
        proposal_type: ProposalType,
        proposer: &Address,
        gate_id: &[u8; 32],
        proposal_data: &[u8],
    ) -> ProposalId {
        let mut hasher = Hasher::new();
        hasher.update(PROPOSAL_ID_DOMAIN);
        hasher.update(&[proposal_type.to_byte()]);
        hasher.update(proposer);
        hasher.update(gate_id);
        hasher.update(&(proposal_data.len() as u32).to_be_bytes());
        hasher.update(proposal_data);
        *hasher.finalize().as_bytes()
    }

    /// Create a new proposal in Submitted state.
    ///
    /// # Arguments
    /// * `proposal_type` - Type of proposal
    /// * `proposal_data` - Encoded proposal content
    /// * `proposer` - Address submitting the proposal
    /// * `gate_id` - Gate that must approve
    /// * `submitted_at` - Current block height
    /// * `expiry_blocks` - Blocks until proposal expires
    #[must_use]
    pub fn new(
        proposal_type: ProposalType,
        proposal_data: Vec<u8>,
        proposer: Address,
        gate_id: [u8; 32],
        submitted_at: u64,
        expiry_blocks: u64,
    ) -> Self {
        let id = Self::compute_id(proposal_type, &proposer, &gate_id, &proposal_data);

        Self {
            id,
            proposal_type,
            proposal_data,
            proposer,
            gate_id,
            state: ProposalState::Submitted,
            submitted_at,
            approved_at: 0,
            executable_at: 0,
            expires_at: submitted_at.saturating_add(expiry_blocks),
            executed_at: 0,
            approvals: Vec::new(),
        }
    }

    /// Check if the proposal has expired at the given height.
    #[must_use]
    pub const fn is_expired(&self, current_height: u64) -> bool {
        current_height >= self.expires_at
    }

    /// Check if the proposal is executable at the given height.
    ///
    /// Returns true only if:
    /// 1. State is Approved (not yet Executable, needs state update) or Executable
    /// 2. Current height >= `executable_at` (timelock elapsed)
    /// 3. Not expired
    #[must_use]
    pub const fn can_execute_at(&self, current_height: u64) -> bool {
        if self.is_expired(current_height) {
            return false;
        }

        match self.state {
            ProposalState::Approved => current_height >= self.executable_at,
            ProposalState::Executable => true,
            _ => false,
        }
    }

    /// Add an approval to this proposal.
    ///
    /// Does not check if approver is valid - caller must verify against gate.
    pub fn add_approval(&mut self, approver: Address) {
        if !self.approvals.contains(&approver) {
            self.approvals.push(approver);
        }
    }

    /// Check if proposal has reached threshold approvals.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // Vec::len() is not const stable
    pub fn has_threshold(&self, threshold: u32) -> bool {
        self.approvals.len() >= threshold as usize
    }

    /// Transition to Approved state.
    ///
    /// # Errors
    /// Returns error if not in Submitted state.
    pub fn approve(
        &mut self,
        current_height: u64,
        timelock_blocks: u64,
    ) -> Result<(), ProposalError> {
        if self.state != ProposalState::Submitted {
            return Err(ProposalError::InvalidStateTransition {
                from: self.state,
                to: ProposalState::Approved,
            });
        }

        self.state = ProposalState::Approved;
        self.approved_at = current_height;
        self.executable_at = current_height.saturating_add(timelock_blocks);
        Ok(())
    }

    /// Transition to Executable state (after timelock elapsed).
    ///
    /// # Errors
    /// Returns error if not in Approved state or timelock not elapsed.
    pub fn make_executable(&mut self, current_height: u64) -> Result<(), ProposalError> {
        if self.state != ProposalState::Approved {
            return Err(ProposalError::InvalidStateTransition {
                from: self.state,
                to: ProposalState::Executable,
            });
        }

        if current_height < self.executable_at {
            return Err(ProposalError::TimelockNotElapsed {
                current: current_height,
                required: self.executable_at,
            });
        }

        self.state = ProposalState::Executable;
        Ok(())
    }

    /// Transition to Executed state.
    ///
    /// # Errors
    /// Returns error if not executable or expired.
    #[allow(clippy::missing_const_for_fn)] // uses control flow and mutation
    pub fn execute(&mut self, current_height: u64) -> Result<(), ProposalError> {
        if self.is_expired(current_height) {
            return Err(ProposalError::Expired {
                expires_at: self.expires_at,
                current: current_height,
            });
        }

        if !self.can_execute_at(current_height) {
            return Err(ProposalError::NotExecutable { state: self.state });
        }

        self.state = ProposalState::Executed;
        self.executed_at = current_height;
        Ok(())
    }

    /// Transition to Rejected state.
    ///
    /// # Errors
    /// Returns error if already in terminal state.
    #[allow(clippy::missing_const_for_fn)] // uses control flow
    pub fn reject(&mut self) -> Result<(), ProposalError> {
        if self.state.is_terminal() {
            return Err(ProposalError::InvalidStateTransition {
                from: self.state,
                to: ProposalState::Rejected,
            });
        }

        self.state = ProposalState::Rejected;
        Ok(())
    }

    /// Transition to Expired state.
    ///
    /// # Errors
    /// Returns error if already in terminal state.
    #[allow(clippy::missing_const_for_fn)] // uses control flow
    pub fn expire(&mut self) -> Result<(), ProposalError> {
        if self.state.is_terminal() {
            return Err(ProposalError::InvalidStateTransition {
                from: self.state,
                to: ProposalState::Expired,
            });
        }

        self.state = ProposalState::Expired;
        Ok(())
    }
}

/// Compute a domain-separated approval digest binding an approver to a specific proposal.
///
/// Digest = blake3(APPROVAL_DIGEST_DOMAIN || approver || proposal_id)
///
/// Future approval transactions MUST sign this digest to cryptographically bind
/// the approval to the exact proposal being approved, preventing cross-proposal
/// replay attacks (audit finding W5-04).
#[must_use]
pub fn compute_approval_digest(approver: &Address, proposal_id: &ProposalId) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.update(APPROVAL_DIGEST_DOMAIN);
    hasher.update(approver);
    hasher.update(proposal_id);
    *hasher.finalize().as_bytes()
}

/// Errors that can occur during proposal operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposalError {
    /// Invalid state transition attempted.
    InvalidStateTransition {
        from: ProposalState,
        to: ProposalState,
    },

    /// Timelock has not elapsed yet.
    TimelockNotElapsed { current: u64, required: u64 },

    /// Proposal has expired.
    Expired { expires_at: u64, current: u64 },

    /// Proposal is not in executable state.
    NotExecutable { state: ProposalState },

    /// Proposer is not authorized.
    UnauthorizedProposer,

    /// Gate not found.
    GateNotFound { gate_id: [u8; 32] },

    /// Invalid approval (approver not in gate).
    InvalidApprover { approver: Address },

    /// Proposal not found.
    ProposalNotFound { id: ProposalId },
}

impl std::fmt::Display for ProposalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidStateTransition { from, to } => {
                write!(f, "invalid transition from {from:?} to {to:?}")
            }
            Self::TimelockNotElapsed { current, required } => {
                write!(
                    f,
                    "timelock not elapsed: current {current} < required {required}"
                )
            }
            Self::Expired {
                expires_at,
                current,
            } => {
                write!(f, "proposal expired at {expires_at} (current: {current})")
            }
            Self::NotExecutable { state } => {
                write!(f, "proposal not executable in state {state:?}")
            }
            Self::UnauthorizedProposer => write!(f, "proposer not authorized"),
            Self::GateNotFound { gate_id } => {
                write!(f, "gate not found: {:02x?}", &gate_id[..4])
            }
            Self::InvalidApprover { approver } => {
                write!(f, "invalid approver: {:02x?}", &approver[..4])
            }
            Self::ProposalNotFound { id } => {
                write!(f, "proposal not found: {:02x?}", &id[..4])
            }
        }
    }
}

impl std::error::Error for ProposalError {}

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
    fn proposal_type_byte_roundtrip() {
        for pt in [
            ProposalType::ParamChange,
            ProposalType::ModuleActivation,
            ProposalType::ModuleRollback,
            ProposalType::PolicyChange,
            ProposalType::EmergencyFreeze,
        ] {
            let byte = pt.to_byte();
            let decoded = ProposalType::from_byte(byte);
            assert_eq!(decoded, Some(pt), "ProposalType {pt:?} roundtrip failed");
        }
    }

    #[test]
    fn proposal_type_invalid_byte_returns_none() {
        assert_eq!(ProposalType::from_byte(5), None);
        assert_eq!(ProposalType::from_byte(255), None);
    }

    #[test]
    fn proposal_type_risk_classification() {
        assert!(!ProposalType::ParamChange.is_high_risk());
        assert!(ProposalType::ModuleActivation.is_high_risk());
        assert!(!ProposalType::ModuleRollback.is_high_risk());
        assert!(ProposalType::PolicyChange.is_high_risk());
        assert!(!ProposalType::EmergencyFreeze.is_high_risk());
        assert!(ProposalType::EmergencyFreeze.is_emergency());
    }

    #[test]
    fn proposal_state_byte_roundtrip() {
        for ps in [
            ProposalState::Submitted,
            ProposalState::Approved,
            ProposalState::Executable,
            ProposalState::Executed,
            ProposalState::Expired,
            ProposalState::Rejected,
        ] {
            let byte = ps.to_byte();
            let decoded = ProposalState::from_byte(byte);
            assert_eq!(decoded, Some(ps), "ProposalState {ps:?} roundtrip failed");
        }
    }

    #[test]
    fn proposal_state_terminal_check() {
        assert!(!ProposalState::Submitted.is_terminal());
        assert!(!ProposalState::Approved.is_terminal());
        assert!(!ProposalState::Executable.is_terminal());
        assert!(ProposalState::Executed.is_terminal());
        assert!(ProposalState::Expired.is_terminal());
        assert!(ProposalState::Rejected.is_terminal());
    }

    #[test]
    fn proposal_id_is_deterministic() {
        let data = b"set MIN_FEE=100".to_vec();
        let proposer = test_proposer();
        let gate_id = test_gate_id();

        let id1 = Proposal::compute_id(ProposalType::ParamChange, &proposer, &gate_id, &data);
        let id2 = Proposal::compute_id(ProposalType::ParamChange, &proposer, &gate_id, &data);

        assert_eq!(id1, id2, "Proposal ID must be deterministic");
    }

    #[test]
    fn proposal_id_changes_with_content() {
        let proposer = test_proposer();
        let gate_id = test_gate_id();

        let id1 = Proposal::compute_id(ProposalType::ParamChange, &proposer, &gate_id, b"data1");
        let id2 = Proposal::compute_id(ProposalType::ParamChange, &proposer, &gate_id, b"data2");

        assert_ne!(id1, id2, "Different data must produce different ID");
    }

    #[test]
    fn proposal_lifecycle_happy_path() {
        let mut proposal = Proposal::new(
            ProposalType::ParamChange,
            b"MIN_FEE=100".to_vec(),
            test_proposer(),
            test_gate_id(),
            100, // submitted_at
            500, // expiry_blocks
        );

        assert_eq!(proposal.state, ProposalState::Submitted);
        assert_eq!(proposal.submitted_at, 100);
        assert_eq!(proposal.expires_at, 600);

        // Add approvals
        proposal.add_approval([0xAAu8; 32]);
        proposal.add_approval([0xBBu8; 32]);
        assert_eq!(proposal.approvals.len(), 2);

        // Approve (timelock = 50 blocks)
        proposal.approve(150, 50).unwrap();
        assert_eq!(proposal.state, ProposalState::Approved);
        assert_eq!(proposal.approved_at, 150);
        assert_eq!(proposal.executable_at, 200);

        // Cannot execute before timelock
        assert!(!proposal.can_execute_at(199));
        assert!(proposal.can_execute_at(200));

        // Make executable
        proposal.make_executable(200).unwrap();
        assert_eq!(proposal.state, ProposalState::Executable);

        // Execute
        proposal.execute(201).unwrap();
        assert_eq!(proposal.state, ProposalState::Executed);
        assert_eq!(proposal.executed_at, 201);
    }

    #[test]
    fn proposal_expiry_blocks_execution() {
        let mut proposal = Proposal::new(
            ProposalType::ParamChange,
            b"test".to_vec(),
            test_proposer(),
            test_gate_id(),
            100,
            50, // Short expiry
        );

        // Approve quickly
        proposal.approve(110, 10).unwrap();
        proposal.make_executable(120).unwrap();

        // Try to execute after expiry
        assert!(proposal.is_expired(151));
        let result = proposal.execute(151);
        assert!(matches!(result, Err(ProposalError::Expired { .. })));
    }

    #[test]
    fn proposal_timelock_enforcement() {
        let mut proposal = Proposal::new(
            ProposalType::ParamChange,
            b"test".to_vec(),
            test_proposer(),
            test_gate_id(),
            100,
            1000,
        );

        proposal.approve(150, 100).unwrap();

        // Try to make executable too early
        let result = proposal.make_executable(200);
        assert!(matches!(
            result,
            Err(ProposalError::TimelockNotElapsed { .. })
        ));

        // Should work at exact timelock
        proposal.make_executable(250).unwrap();
        assert_eq!(proposal.state, ProposalState::Executable);
    }

    #[test]
    fn proposal_reject_works() {
        let mut proposal = Proposal::new(
            ProposalType::ParamChange,
            b"test".to_vec(),
            test_proposer(),
            test_gate_id(),
            100,
            1000,
        );

        proposal.reject().unwrap();
        assert_eq!(proposal.state, ProposalState::Rejected);

        // Cannot reject again
        let result = proposal.reject();
        assert!(matches!(
            result,
            Err(ProposalError::InvalidStateTransition { .. })
        ));
    }

    #[test]
    fn proposal_expire_works() {
        let mut proposal = Proposal::new(
            ProposalType::ParamChange,
            b"test".to_vec(),
            test_proposer(),
            test_gate_id(),
            100,
            1000,
        );

        proposal.expire().unwrap();
        assert_eq!(proposal.state, ProposalState::Expired);
    }

    #[test]
    fn duplicate_approval_ignored() {
        let mut proposal = Proposal::new(
            ProposalType::ParamChange,
            b"test".to_vec(),
            test_proposer(),
            test_gate_id(),
            100,
            1000,
        );

        let approver = [0xAAu8; 32];
        proposal.add_approval(approver);
        proposal.add_approval(approver); // Duplicate

        assert_eq!(proposal.approvals.len(), 1);
    }

    #[test]
    fn threshold_check_works() {
        let mut proposal = Proposal::new(
            ProposalType::ParamChange,
            b"test".to_vec(),
            test_proposer(),
            test_gate_id(),
            100,
            1000,
        );

        assert!(!proposal.has_threshold(2));

        proposal.add_approval([0xAAu8; 32]);
        assert!(!proposal.has_threshold(2));

        proposal.add_approval([0xBBu8; 32]);
        assert!(proposal.has_threshold(2));
    }

    // ========================================================================
    // GOVERNANCE CONFIG TESTS (D19.4)
    // ========================================================================

    #[test]
    fn governance_config_default_values() {
        let config = GovernanceConfig::default();

        assert_eq!(config.default_timelock_blocks, 1000);
        assert_eq!(config.high_risk_timelock_blocks, 5000);
        assert_eq!(config.emergency_timelock_blocks, 100);
        assert_eq!(config.default_expiry_blocks, 50000);
    }

    #[test]
    fn governance_config_timelock_for_proposal_type() {
        let config = GovernanceConfig::new(100, 500, 10, 10000);

        // Standard proposals get default timelock
        assert_eq!(
            config.timelock_for_proposal_type(ProposalType::ParamChange),
            100
        );
        assert_eq!(
            config.timelock_for_proposal_type(ProposalType::ModuleRollback),
            100
        );

        // High-risk proposals get extended timelock
        assert_eq!(
            config.timelock_for_proposal_type(ProposalType::ModuleActivation),
            500
        );
        assert_eq!(
            config.timelock_for_proposal_type(ProposalType::PolicyChange),
            500
        );

        // Emergency proposals get shortened timelock
        assert_eq!(
            config.timelock_for_proposal_type(ProposalType::EmergencyFreeze),
            10
        );
    }

    #[test]
    fn get_timelock_helper_function() {
        let config = GovernanceConfig::new(200, 1000, 50, 20000);

        assert_eq!(
            get_timelock_for_proposal_type(ProposalType::ParamChange, &config),
            200
        );
        assert_eq!(
            get_timelock_for_proposal_type(ProposalType::ModuleActivation, &config),
            1000
        );
        assert_eq!(
            get_timelock_for_proposal_type(ProposalType::EmergencyFreeze, &config),
            50
        );
    }

    // ========================================================================
    // AUDIT ACTION TESTS (D19.5)
    // ========================================================================

    #[test]
    fn audit_action_byte_roundtrip() {
        for action in [
            AuditAction::Submitted,
            AuditAction::Approved,
            AuditAction::Executed,
            AuditAction::Rejected,
            AuditAction::Expired,
        ] {
            let byte = action.to_byte();
            let decoded = AuditAction::from_byte(byte);
            assert_eq!(
                decoded,
                Some(action),
                "AuditAction {action:?} roundtrip failed"
            );
        }
    }

    #[test]
    fn audit_action_invalid_byte_returns_none() {
        assert_eq!(AuditAction::from_byte(5), None);
        assert_eq!(AuditAction::from_byte(255), None);
    }

    #[test]
    fn audit_log_entry_factory_methods() {
        let proposal_id = [0x11u8; 32];
        let actor = [0x01u8; 32];

        let submitted = AuditLogEntry::submitted(proposal_id, 100, actor);
        assert_eq!(submitted.action, AuditAction::Submitted);
        assert_eq!(submitted.block_height, 100);
        assert_eq!(submitted.actor, Some(actor));

        let approved = AuditLogEntry::approved(proposal_id, 200, actor);
        assert_eq!(approved.action, AuditAction::Approved);

        let executed = AuditLogEntry::executed(proposal_id, 300, actor);
        assert_eq!(executed.action, AuditAction::Executed);

        let rejected = AuditLogEntry::rejected(proposal_id, 400, actor);
        assert_eq!(rejected.action, AuditAction::Rejected);

        let expired = AuditLogEntry::expired(proposal_id, 500);
        assert_eq!(expired.action, AuditAction::Expired);
        assert_eq!(expired.actor, None); // System-initiated, no actor
    }

    // ========================================================================
    // APPROVAL DIGEST TESTS (W5-04 hardening)
    // ========================================================================

    #[test]
    fn approval_digest_is_deterministic() {
        let approver = [0xAAu8; 32];
        let proposal_id = [0xBBu8; 32];
        let d1 = compute_approval_digest(&approver, &proposal_id);
        let d2 = compute_approval_digest(&approver, &proposal_id);
        assert_eq!(d1, d2);
    }

    #[test]
    fn approval_digest_changes_with_proposal_id() {
        let approver = [0xAAu8; 32];
        let proposal_a = [0x01u8; 32];
        let proposal_b = [0x02u8; 32];
        let da = compute_approval_digest(&approver, &proposal_a);
        let db = compute_approval_digest(&approver, &proposal_b);
        assert_ne!(
            da, db,
            "different proposal_id must produce different digest"
        );
    }

    #[test]
    fn approval_digest_changes_with_approver() {
        let proposal_id = [0xBBu8; 32];
        let approver_a = [0x01u8; 32];
        let approver_b = [0x02u8; 32];
        let da = compute_approval_digest(&approver_a, &proposal_id);
        let db = compute_approval_digest(&approver_b, &proposal_id);
        assert_ne!(da, db, "different approver must produce different digest");
    }
}
