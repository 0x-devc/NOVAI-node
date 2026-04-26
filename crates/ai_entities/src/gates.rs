// SPDX-License-Identifier: MIT OR Apache-2.0
//! Approval gate types for AI-triggered action execution.
//!
//! PURPOSE: Define the approval gate framework that governs how AI entities
//! can request execution of actions above Tier 0.
//!
//! INVARIANTS:
//! - Gate IDs are deterministically computed from gate parameters
//! - Gates must pass validation before being stored or used
//! - Threshold <= number of required approvers
//! - Expiry blocks > timelock blocks
//!
//! FAILURE MODES:
//! - Invalid gate parameters are rejected at validation time
//! - Duplicate approvers cause validation failure
//! - Zero threshold for Multisig/Threshold gates is rejected

use blake3::Hasher;
use novai_types::Address;

/// Domain separator for gate ID computation.
const GATE_ID_DOMAIN: &[u8] = b"NOVAI_APPROVAL_GATE_ID_V1";

/// Maximum number of approvers per gate (DoS prevention).
pub const MAX_APPROVERS: usize = 256;

/// Unique identifier for an approval gate.
///
/// Computed deterministically from gate parameters using:
/// `blake3(GATE_ID_DOMAIN || gate_type || threshold || sorted_approvers || timelock || expiry || flags)`
///
/// This makes gate IDs content-addressable and collision-resistant.
pub type GateId = [u8; 32];

/// Type of approval mechanism for a gate.
///
/// Determines how approvals are collected and verified before
/// an action can be executed.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GateType {
    /// Requires exactly threshold signatures (N-of-M multisig).
    ///
    /// All threshold signatures must come from the required_approvers list.
    /// Commonly used for high-value operations requiring multiple parties.
    Multisig = 0,

    /// Requires at least threshold signatures.
    ///
    /// More flexible than Multisig - any subset of required_approvers
    /// meeting the threshold count can approve.
    Threshold = 1,

    /// Auto-approved after timelock period expires.
    ///
    /// No signatures required. The action is automatically approved
    /// once the timelock duration has passed. Useful for time-delayed
    /// operations that need a veto window.
    TimelockOnly = 2,
}

impl GateType {
    /// Encode to canonical byte representation.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        self as u8
    }

    /// Decode from byte, returning None for invalid values.
    ///
    /// # Examples
    ///
    /// ```
    /// use novai_ai_entities::gates::GateType;
    ///
    /// assert_eq!(GateType::from_byte(0), Some(GateType::Multisig));
    /// assert_eq!(GateType::from_byte(1), Some(GateType::Threshold));
    /// assert_eq!(GateType::from_byte(2), Some(GateType::TimelockOnly));
    /// assert_eq!(GateType::from_byte(3), None);
    /// ```
    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(GateType::Multisig),
            1 => Some(GateType::Threshold),
            2 => Some(GateType::TimelockOnly),
            _ => None,
        }
    }

    /// Returns true if this gate type requires signature collection.
    ///
    /// # Examples
    ///
    /// ```
    /// use novai_ai_entities::gates::GateType;
    ///
    /// assert!(GateType::Multisig.requires_signatures());
    /// assert!(GateType::Threshold.requires_signatures());
    /// assert!(!GateType::TimelockOnly.requires_signatures());
    /// ```
    #[must_use]
    pub const fn requires_signatures(self) -> bool {
        matches!(self, GateType::Multisig | GateType::Threshold)
    }
}

impl Default for GateType {
    /// Default gate type is Multisig (most secure).
    fn default() -> Self {
        GateType::Multisig
    }
}

/// Validation errors for approval gates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateValidationError {
    /// Threshold exceeds the number of required approvers.
    ThresholdExceedsApprovers {
        threshold: u32,
        approver_count: usize,
    },

    /// Expiry blocks must be greater than timelock blocks.
    ExpiryBeforeTimelock {
        timelock_blocks: u64,
        expiry_blocks: u64,
    },

    /// Duplicate address in required_approvers list.
    DuplicateApprover { address: Address },

    /// Zero threshold for Multisig or Threshold gate type.
    ZeroThreshold,

    /// Too many approvers (exceeds MAX_APPROVERS).
    TooManyApprovers { count: usize, max: usize },

    /// TimelockOnly gate should not have approvers.
    TimelockOnlyWithApprovers { count: usize },
}

impl std::fmt::Display for GateValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateValidationError::ThresholdExceedsApprovers {
                threshold,
                approver_count,
            } => {
                write!(
                    f,
                    "threshold {threshold} exceeds approver count {approver_count}"
                )
            }
            GateValidationError::ExpiryBeforeTimelock {
                timelock_blocks,
                expiry_blocks,
            } => {
                write!(
                    f,
                    "expiry_blocks {expiry_blocks} must be greater than timelock_blocks {timelock_blocks}"
                )
            }
            GateValidationError::DuplicateApprover { address } => {
                write!(f, "duplicate approver address: {:02x?}", &address[..4])
            }
            GateValidationError::ZeroThreshold => {
                write!(f, "threshold must be > 0 for Multisig/Threshold gates")
            }
            GateValidationError::TooManyApprovers { count, max } => {
                write!(f, "too many approvers: {count} exceeds max {max}")
            }
            GateValidationError::TimelockOnlyWithApprovers { count } => {
                write!(
                    f,
                    "TimelockOnly gate should have 0 approvers, found {count}"
                )
            }
        }
    }
}

impl std::error::Error for GateValidationError {}

/// An approval gate that governs AI action execution.
///
/// Gates define the approval requirements that must be satisfied before
/// an AI entity can execute an action. Each gate specifies:
/// - Who can approve (required_approvers)
/// - How many approvals are needed (threshold)
/// - How long to wait after approval (timelock_blocks)
/// - When the proposal expires (expiry_blocks)
///
/// # Gate ID Computation
///
/// The gate_id is computed deterministically from all parameters:
/// ```text
/// gate_id = blake3(
///     GATE_ID_DOMAIN ||
///     gate_type ||
///     threshold ||
///     sorted_approvers ||
///     timelock_blocks ||
///     expiry_blocks ||
///     flags
/// )
/// ```
///
/// This makes gates content-addressable: identical parameters produce
/// identical gate IDs.
///
/// # Examples
///
/// ```
/// use novai_ai_entities::gates::{ApprovalGate, GateType};
///
/// // Create a 2-of-3 multisig gate
/// let approvers = vec![[0x01u8; 32], [0x02u8; 32], [0x03u8; 32]];
/// let gate = ApprovalGate::new(
///     GateType::Multisig,
///     approvers,
///     2,              // threshold
///     100,            // timelock_blocks
///     1000,           // expiry_blocks
///     false,          // veto_enabled
///     false,          // freeze_enabled
/// ).expect("valid gate");
///
/// assert_eq!(gate.threshold, 2);
/// assert_eq!(gate.required_approvers.len(), 3);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalGate {
    /// Unique gate identifier (deterministically computed).
    pub gate_id: GateId,

    /// Type of approval mechanism.
    pub gate_type: GateType,

    /// Addresses allowed to approve proposals through this gate.
    /// For Multisig/Threshold gates, these are the valid signers.
    /// For TimelockOnly gates, this should be empty.
    pub required_approvers: Vec<Address>,

    /// Number of required approvals.
    /// - For Multisig: exactly this many signatures required
    /// - For Threshold: at least this many signatures required
    /// - For TimelockOnly: ignored (should be 0)
    pub threshold: u32,

    /// Blocks to wait after threshold met before execution.
    /// Set to 0 for immediate execution after threshold.
    pub timelock_blocks: u64,

    /// Proposal expires after this many blocks from submission.
    /// Must be greater than timelock_blocks.
    pub expiry_blocks: u64,

    /// Can validators veto approved proposals?
    /// If true, validators can block execution even after approval.
    pub veto_enabled: bool,

    /// Can this gate be frozen/paused?
    /// If true, governance can temporarily disable the gate.
    pub freeze_enabled: bool,
}

impl ApprovalGate {
    /// Create a new approval gate with validation.
    ///
    /// # Arguments
    ///
    /// * `gate_type` - Type of approval mechanism
    /// * `required_approvers` - Addresses that can approve (will be sorted)
    /// * `threshold` - Number of required approvals
    /// * `timelock_blocks` - Blocks to wait after approval
    /// * `expiry_blocks` - Blocks until proposal expires
    /// * `veto_enabled` - Whether validators can veto
    /// * `freeze_enabled` - Whether gate can be frozen
    ///
    /// # Errors
    ///
    /// Returns `GateValidationError` if parameters are invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// use novai_ai_entities::gates::{ApprovalGate, GateType, GateValidationError};
    ///
    /// // Valid 2-of-3 multisig
    /// let result = ApprovalGate::new(
    ///     GateType::Multisig,
    ///     vec![[0x01u8; 32], [0x02u8; 32], [0x03u8; 32]],
    ///     2, 100, 1000, false, false,
    /// );
    /// assert!(result.is_ok());
    ///
    /// // Invalid: threshold > approvers
    /// let result = ApprovalGate::new(
    ///     GateType::Threshold,
    ///     vec![[0x01u8; 32]],
    ///     5, 100, 1000, false, false,
    /// );
    /// assert!(matches!(result, Err(GateValidationError::ThresholdExceedsApprovers { .. })));
    /// ```
    pub fn new(
        gate_type: GateType,
        required_approvers: Vec<Address>,
        threshold: u32,
        timelock_blocks: u64,
        expiry_blocks: u64,
        veto_enabled: bool,
        freeze_enabled: bool,
    ) -> Result<Self, GateValidationError> {
        // Sort approvers for canonical ordering
        let mut sorted_approvers = required_approvers;
        sorted_approvers.sort();

        // Pre-compute gate_id (validation may still fail after this)
        let gate_id = Self::compute_gate_id(
            gate_type,
            &sorted_approvers,
            threshold,
            timelock_blocks,
            expiry_blocks,
            veto_enabled,
            freeze_enabled,
        );

        let gate = Self {
            gate_id,
            gate_type,
            required_approvers: sorted_approvers,
            threshold,
            timelock_blocks,
            expiry_blocks,
            veto_enabled,
            freeze_enabled,
        };

        gate.validate()?;
        Ok(gate)
    }

    /// Compute the canonical gate ID from parameters.
    ///
    /// Uses domain-separated blake3 hashing over canonical encoding.
    fn compute_gate_id(
        gate_type: GateType,
        sorted_approvers: &[Address],
        threshold: u32,
        timelock_blocks: u64,
        expiry_blocks: u64,
        veto_enabled: bool,
        freeze_enabled: bool,
    ) -> GateId {
        let mut hasher = Hasher::new();

        // Domain separation
        hasher.update(GATE_ID_DOMAIN);

        // Gate type
        hasher.update(&[gate_type.to_byte()]);

        // Threshold (big-endian)
        hasher.update(&threshold.to_be_bytes());

        // Sorted approvers with count prefix
        let approver_count = sorted_approvers.len() as u32;
        hasher.update(&approver_count.to_be_bytes());
        for approver in sorted_approvers {
            hasher.update(approver);
        }

        // Timelock and expiry (big-endian)
        hasher.update(&timelock_blocks.to_be_bytes());
        hasher.update(&expiry_blocks.to_be_bytes());

        // Flags
        let flags = Self::pack_flags(veto_enabled, freeze_enabled);
        hasher.update(&[flags]);

        *hasher.finalize().as_bytes()
    }

    /// Pack boolean flags into a single byte.
    #[must_use]
    const fn pack_flags(veto_enabled: bool, freeze_enabled: bool) -> u8 {
        let mut flags = 0u8;
        if veto_enabled {
            flags |= 0x01;
        }
        if freeze_enabled {
            flags |= 0x02;
        }
        flags
    }

    /// Unpack flags byte into boolean values.
    #[must_use]
    pub const fn unpack_flags(flags: u8) -> (bool, bool) {
        let veto_enabled = (flags & 0x01) != 0;
        let freeze_enabled = (flags & 0x02) != 0;
        (veto_enabled, freeze_enabled)
    }

    /// Validate gate parameters.
    ///
    /// # Errors
    ///
    /// Returns `GateValidationError` if any constraint is violated.
    pub fn validate(&self) -> Result<(), GateValidationError> {
        // 1. Check approver count limit
        if self.required_approvers.len() > MAX_APPROVERS {
            return Err(GateValidationError::TooManyApprovers {
                count: self.required_approvers.len(),
                max: MAX_APPROVERS,
            });
        }

        // 2. Type-specific validation
        match self.gate_type {
            GateType::Multisig | GateType::Threshold => {
                // Threshold must be > 0
                if self.threshold == 0 {
                    return Err(GateValidationError::ZeroThreshold);
                }

                // Threshold <= approvers
                if self.threshold as usize > self.required_approvers.len() {
                    return Err(GateValidationError::ThresholdExceedsApprovers {
                        threshold: self.threshold,
                        approver_count: self.required_approvers.len(),
                    });
                }
            }
            GateType::TimelockOnly => {
                // TimelockOnly should not have approvers
                if !self.required_approvers.is_empty() {
                    return Err(GateValidationError::TimelockOnlyWithApprovers {
                        count: self.required_approvers.len(),
                    });
                }
            }
        }

        // 3. Expiry must be greater than timelock
        if self.expiry_blocks <= self.timelock_blocks {
            return Err(GateValidationError::ExpiryBeforeTimelock {
                timelock_blocks: self.timelock_blocks,
                expiry_blocks: self.expiry_blocks,
            });
        }

        // 4. No duplicate approvers (list is sorted, so check adjacent)
        for i in 1..self.required_approvers.len() {
            if self.required_approvers[i] == self.required_approvers[i - 1] {
                return Err(GateValidationError::DuplicateApprover {
                    address: self.required_approvers[i],
                });
            }
        }

        Ok(())
    }

    /// Returns the flags byte for this gate.
    #[must_use]
    pub const fn flags(&self) -> u8 {
        Self::pack_flags(self.veto_enabled, self.freeze_enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_approvers(count: usize) -> Vec<Address> {
        (0..count)
            .map(|i| {
                let mut addr = [0u8; 32];
                addr[0] = i as u8;
                addr[1] = (i >> 8) as u8;
                addr
            })
            .collect()
    }

    #[test]
    fn gate_type_byte_roundtrip() {
        for gt in [
            GateType::Multisig,
            GateType::Threshold,
            GateType::TimelockOnly,
        ] {
            let byte = gt.to_byte();
            let decoded = GateType::from_byte(byte);
            assert_eq!(decoded, Some(gt), "GateType {gt:?} roundtrip failed");
        }
    }

    #[test]
    fn gate_type_from_byte_invalid() {
        assert_eq!(GateType::from_byte(3), None);
        assert_eq!(GateType::from_byte(255), None);
    }

    #[test]
    fn gate_type_requires_signatures() {
        assert!(GateType::Multisig.requires_signatures());
        assert!(GateType::Threshold.requires_signatures());
        assert!(!GateType::TimelockOnly.requires_signatures());
    }

    #[test]
    fn gate_type_default_is_multisig() {
        assert_eq!(GateType::default(), GateType::Multisig);
    }

    #[test]
    fn valid_multisig_gate() {
        let approvers = test_approvers(3);
        let gate = ApprovalGate::new(GateType::Multisig, approvers, 2, 100, 1000, false, false)
            .expect("should be valid");

        assert_eq!(gate.gate_type, GateType::Multisig);
        assert_eq!(gate.threshold, 2);
        assert_eq!(gate.required_approvers.len(), 3);
        assert_eq!(gate.timelock_blocks, 100);
        assert_eq!(gate.expiry_blocks, 1000);
        assert!(!gate.veto_enabled);
        assert!(!gate.freeze_enabled);
    }

    #[test]
    fn valid_threshold_gate() {
        let approvers = test_approvers(5);
        let gate = ApprovalGate::new(GateType::Threshold, approvers, 3, 50, 500, true, true)
            .expect("should be valid");

        assert_eq!(gate.gate_type, GateType::Threshold);
        assert_eq!(gate.threshold, 3);
        assert!(gate.veto_enabled);
        assert!(gate.freeze_enabled);
    }

    #[test]
    fn valid_timelock_only_gate() {
        let gate = ApprovalGate::new(
            GateType::TimelockOnly,
            vec![], // No approvers
            0,      // No threshold
            100,
            1000,
            false,
            false,
        )
        .expect("should be valid");

        assert_eq!(gate.gate_type, GateType::TimelockOnly);
        assert!(gate.required_approvers.is_empty());
    }

    #[test]
    fn threshold_exceeds_approvers_rejected() {
        let approvers = test_approvers(2);
        let result = ApprovalGate::new(
            GateType::Multisig,
            approvers,
            5, // threshold > approvers
            100,
            1000,
            false,
            false,
        );

        assert!(matches!(
            result,
            Err(GateValidationError::ThresholdExceedsApprovers { .. })
        ));
    }

    #[test]
    fn zero_threshold_rejected() {
        let approvers = test_approvers(3);
        let result = ApprovalGate::new(
            GateType::Multisig,
            approvers,
            0, // zero threshold
            100,
            1000,
            false,
            false,
        );

        assert!(matches!(result, Err(GateValidationError::ZeroThreshold)));
    }

    #[test]
    fn expiry_before_timelock_rejected() {
        let approvers = test_approvers(3);
        let result = ApprovalGate::new(
            GateType::Threshold,
            approvers,
            2,
            1000, // timelock
            500,  // expiry < timelock
            false,
            false,
        );

        assert!(matches!(
            result,
            Err(GateValidationError::ExpiryBeforeTimelock { .. })
        ));
    }

    #[test]
    fn expiry_equals_timelock_rejected() {
        let approvers = test_approvers(3);
        let result = ApprovalGate::new(
            GateType::Threshold,
            approvers,
            2,
            100, // timelock
            100, // expiry == timelock
            false,
            false,
        );

        assert!(matches!(
            result,
            Err(GateValidationError::ExpiryBeforeTimelock { .. })
        ));
    }

    #[test]
    fn duplicate_approver_rejected() {
        let mut approvers = test_approvers(3);
        approvers.push(approvers[0]); // Add duplicate

        let result = ApprovalGate::new(GateType::Multisig, approvers, 2, 100, 1000, false, false);

        assert!(matches!(
            result,
            Err(GateValidationError::DuplicateApprover { .. })
        ));
    }

    #[test]
    fn too_many_approvers_rejected() {
        let approvers = test_approvers(MAX_APPROVERS + 1);
        let result = ApprovalGate::new(GateType::Threshold, approvers, 1, 100, 1000, false, false);

        assert!(matches!(
            result,
            Err(GateValidationError::TooManyApprovers { .. })
        ));
    }

    #[test]
    fn timelock_only_with_approvers_rejected() {
        let approvers = test_approvers(2);
        let result = ApprovalGate::new(
            GateType::TimelockOnly,
            approvers, // Should be empty
            0,
            100,
            1000,
            false,
            false,
        );

        assert!(matches!(
            result,
            Err(GateValidationError::TimelockOnlyWithApprovers { .. })
        ));
    }

    #[test]
    fn gate_id_is_deterministic() {
        let approvers = test_approvers(3);

        let gate1 = ApprovalGate::new(
            GateType::Multisig,
            approvers.clone(),
            2,
            100,
            1000,
            false,
            false,
        )
        .unwrap();

        let gate2 =
            ApprovalGate::new(GateType::Multisig, approvers, 2, 100, 1000, false, false).unwrap();

        assert_eq!(gate1.gate_id, gate2.gate_id);
    }

    #[test]
    fn gate_id_changes_with_parameters() {
        let approvers = test_approvers(3);

        let gate1 = ApprovalGate::new(
            GateType::Multisig,
            approvers.clone(),
            2,
            100,
            1000,
            false,
            false,
        )
        .unwrap();

        // Different threshold
        let gate2 = ApprovalGate::new(
            GateType::Multisig,
            approvers.clone(),
            3, // Changed
            100,
            1000,
            false,
            false,
        )
        .unwrap();

        assert_ne!(
            gate1.gate_id, gate2.gate_id,
            "Different threshold should produce different ID"
        );

        // Different gate type
        let gate3 = ApprovalGate::new(
            GateType::Threshold, // Changed
            approvers.clone(),
            2,
            100,
            1000,
            false,
            false,
        )
        .unwrap();

        assert_ne!(
            gate1.gate_id, gate3.gate_id,
            "Different type should produce different ID"
        );

        // Different flags
        let gate4 = ApprovalGate::new(
            GateType::Multisig,
            approvers,
            2,
            100,
            1000,
            true, // Changed
            false,
        )
        .unwrap();

        assert_ne!(
            gate1.gate_id, gate4.gate_id,
            "Different flags should produce different ID"
        );
    }

    #[test]
    fn approvers_are_sorted() {
        // Create approvers in reverse order
        let mut approvers = test_approvers(3);
        approvers.reverse();
        let original_first = approvers[0];
        let original_last = approvers[2];

        let gate =
            ApprovalGate::new(GateType::Multisig, approvers, 2, 100, 1000, false, false).unwrap();

        // After sorting, order should be reversed
        assert_eq!(gate.required_approvers[0], original_last);
        assert_eq!(gate.required_approvers[2], original_first);
    }

    #[test]
    fn flags_pack_unpack() {
        for veto in [false, true] {
            for freeze in [false, true] {
                let packed = ApprovalGate::pack_flags(veto, freeze);
                let (unpacked_veto, unpacked_freeze) = ApprovalGate::unpack_flags(packed);
                assert_eq!(veto, unpacked_veto);
                assert_eq!(freeze, unpacked_freeze);
            }
        }
    }

    #[test]
    fn flags_method_returns_correct_value() {
        let gate = ApprovalGate::new(
            GateType::Multisig,
            test_approvers(3),
            2,
            100,
            1000,
            true, // veto
            true, // freeze
        )
        .unwrap();

        assert_eq!(gate.flags(), 0x03); // Both bits set

        let gate2 = ApprovalGate::new(
            GateType::Multisig,
            test_approvers(3),
            2,
            100,
            1000,
            true,  // veto
            false, // no freeze
        )
        .unwrap();

        assert_eq!(gate2.flags(), 0x01); // Only veto bit
    }

    #[test]
    fn max_approvers_allowed() {
        let approvers = test_approvers(MAX_APPROVERS);
        let result = ApprovalGate::new(GateType::Threshold, approvers, 1, 100, 1000, false, false);

        assert!(result.is_ok(), "MAX_APPROVERS should be allowed");
    }
}
