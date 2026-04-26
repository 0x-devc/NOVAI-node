// SPDX-License-Identifier: MIT OR Apache-2.0
//! Action tiering engine for AI-triggered operations.
//!
//! PURPOSE: Classify AI-triggered actions by security level to determine
//! what approval gates are required before execution.
//!
//! INVARIANTS:
//! - Every ActionType maps to exactly one ActionTier
//! - tier_for_action() is exhaustive and total (no panics)
//! - Tier 0 actions are NEVER executable by AI entities
//!
//! FAILURE MODES:
//! - Unknown action types cannot exist (enum is closed)
//! - Tier misclassification would be a compile-time error (exhaustive match)

/// Security tier for AI-triggered actions.
///
/// Tiers are ordered by security level, with lower values indicating
/// higher security requirements. This ordering is reflected in the
/// `PartialOrd`/`Ord` implementations.
///
/// # Examples
///
/// ```
/// use novai_ai_entities::tiers::ActionTier;
///
/// assert!(ActionTier::Tier0Never < ActionTier::Tier1High);
/// assert!(ActionTier::Tier1High < ActionTier::Tier2Medium);
/// assert!(ActionTier::Tier2Medium < ActionTier::Tier3Low);
/// ```
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ActionTier {
    /// NEVER allowed - consensus-critical operations.
    ///
    /// These actions can never be executed by AI entities regardless of
    /// approval gates or governance. They require human-only governance
    /// processes outside the AI system.
    Tier0Never = 0,

    /// High security - affects core protocol parameters.
    ///
    /// Requires the strongest approval gates: high thresholds, long timelocks,
    /// and potentially validator veto rights.
    Tier1High = 1,

    /// Medium security - affects operational parameters.
    ///
    /// Requires moderate approval gates with reasonable thresholds and
    /// timelocks appropriate for operational changes.
    Tier2Medium = 2,

    /// Low security - operational tuning only.
    ///
    /// Minimal approval requirements. May allow single-signer approval
    /// or short timelocks for routine operational adjustments.
    Tier3Low = 3,
}

impl ActionTier {
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
    /// use novai_ai_entities::tiers::ActionTier;
    ///
    /// assert_eq!(ActionTier::from_byte(0), Some(ActionTier::Tier0Never));
    /// assert_eq!(ActionTier::from_byte(1), Some(ActionTier::Tier1High));
    /// assert_eq!(ActionTier::from_byte(4), None);
    /// ```
    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(ActionTier::Tier0Never),
            1 => Some(ActionTier::Tier1High),
            2 => Some(ActionTier::Tier2Medium),
            3 => Some(ActionTier::Tier3Low),
            _ => None,
        }
    }

    /// Returns true if this tier allows AI execution (with appropriate gates).
    ///
    /// Tier 0 actions are never allowed; all other tiers may be executed
    /// if the appropriate approval gates are satisfied.
    ///
    /// # Examples
    ///
    /// ```
    /// use novai_ai_entities::tiers::ActionTier;
    ///
    /// assert!(!ActionTier::Tier0Never.is_ai_executable());
    /// assert!(ActionTier::Tier1High.is_ai_executable());
    /// assert!(ActionTier::Tier2Medium.is_ai_executable());
    /// assert!(ActionTier::Tier3Low.is_ai_executable());
    /// ```
    #[must_use]
    pub const fn is_ai_executable(self) -> bool {
        !matches!(self, ActionTier::Tier0Never)
    }
}

impl Default for ActionTier {
    /// Default tier is Tier0Never (most restrictive) for safety.
    fn default() -> Self {
        ActionTier::Tier0Never
    }
}

/// Types of actions that AI entities may request.
///
/// Each action type is classified into exactly one security tier.
/// The tier determines what approval gates must be satisfied before
/// the action can be executed.
///
/// # Tier Classification
///
/// | Tier | Actions | Description |
/// |------|---------|-------------|
/// | 0 | ModifyConsensusRule, ModifyStateTransition | Never allowed |
/// | 1 | UpdateBaseFee, UpdateBlockLimit, ActivateModule | High security |
/// | 2 | UpdatePeerScoring, UpdateSpamThreshold, EmitAuditReport | Medium security |
/// | 3 | (Reserved for future operational tuning actions) | Low security |
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionType {
    // === Tier 0: NEVER Allowed ===
    /// Modify consensus rules (block validity, finality conditions).
    /// NEVER allowed via AI execution.
    ModifyConsensusRule = 0,

    /// Modify state transition function.
    /// NEVER allowed via AI execution.
    ModifyStateTransition = 1,

    // === Tier 1: High Security ===
    /// Update base transaction fee parameter.
    UpdateBaseFee = 10,

    /// Update block size/gas limit.
    UpdateBlockLimit = 11,

    /// Activate a new protocol module.
    ActivateModule = 12,

    // === Tier 2: Medium Security ===
    /// Update peer scoring/reputation parameters.
    UpdatePeerScoring = 20,

    /// Update spam detection thresholds.
    UpdateSpamThreshold = 21,

    /// Emit an audit report to the chain.
    EmitAuditReport = 22,
}

impl ActionType {
    /// Encode to canonical byte representation.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        self as u8
    }

    /// Decode from byte, returning None for invalid/unknown values.
    ///
    /// # Examples
    ///
    /// ```
    /// use novai_ai_entities::tiers::ActionType;
    ///
    /// assert_eq!(ActionType::from_byte(0), Some(ActionType::ModifyConsensusRule));
    /// assert_eq!(ActionType::from_byte(10), Some(ActionType::UpdateBaseFee));
    /// assert_eq!(ActionType::from_byte(99), None);
    /// ```
    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(ActionType::ModifyConsensusRule),
            1 => Some(ActionType::ModifyStateTransition),
            10 => Some(ActionType::UpdateBaseFee),
            11 => Some(ActionType::UpdateBlockLimit),
            12 => Some(ActionType::ActivateModule),
            20 => Some(ActionType::UpdatePeerScoring),
            21 => Some(ActionType::UpdateSpamThreshold),
            22 => Some(ActionType::EmitAuditReport),
            _ => None,
        }
    }

    /// Returns all known action types.
    ///
    /// Useful for iteration, testing, and documentation generation.
    #[must_use]
    pub const fn all() -> &'static [ActionType] {
        &[
            ActionType::ModifyConsensusRule,
            ActionType::ModifyStateTransition,
            ActionType::UpdateBaseFee,
            ActionType::UpdateBlockLimit,
            ActionType::ActivateModule,
            ActionType::UpdatePeerScoring,
            ActionType::UpdateSpamThreshold,
            ActionType::EmitAuditReport,
        ]
    }
}

/// Determine the security tier for a given action type.
///
/// This function is the authoritative mapping from action types to tiers.
/// It is exhaustive and will produce a compile error if new ActionType
/// variants are added without updating this function.
///
/// # Examples
///
/// ```
/// use novai_ai_entities::tiers::{tier_for_action, ActionType, ActionTier};
///
/// // Tier 0: Never allowed
/// assert_eq!(tier_for_action(&ActionType::ModifyConsensusRule), ActionTier::Tier0Never);
/// assert_eq!(tier_for_action(&ActionType::ModifyStateTransition), ActionTier::Tier0Never);
///
/// // Tier 1: High security
/// assert_eq!(tier_for_action(&ActionType::UpdateBaseFee), ActionTier::Tier1High);
///
/// // Tier 2: Medium security
/// assert_eq!(tier_for_action(&ActionType::EmitAuditReport), ActionTier::Tier2Medium);
/// ```
#[must_use]
pub const fn tier_for_action(action: &ActionType) -> ActionTier {
    match action {
        // Tier 0: NEVER allowed - consensus-critical
        ActionType::ModifyConsensusRule => ActionTier::Tier0Never,
        ActionType::ModifyStateTransition => ActionTier::Tier0Never,

        // Tier 1: High security - core protocol parameters
        ActionType::UpdateBaseFee => ActionTier::Tier1High,
        ActionType::UpdateBlockLimit => ActionTier::Tier1High,
        ActionType::ActivateModule => ActionTier::Tier1High,

        // Tier 2: Medium security - operational parameters
        ActionType::UpdatePeerScoring => ActionTier::Tier2Medium,
        ActionType::UpdateSpamThreshold => ActionTier::Tier2Medium,
        ActionType::EmitAuditReport => ActionTier::Tier2Medium,
    }
}

/// Check if an action is executable by AI entities.
///
/// Returns true if the action's tier allows AI execution (with gates),
/// false if the action is in Tier 0 (never allowed).
///
/// # Examples
///
/// ```
/// use novai_ai_entities::tiers::{is_ai_executable, ActionType};
///
/// assert!(!is_ai_executable(&ActionType::ModifyConsensusRule));
/// assert!(is_ai_executable(&ActionType::UpdateBaseFee));
/// assert!(is_ai_executable(&ActionType::EmitAuditReport));
/// ```
#[must_use]
pub const fn is_ai_executable(action: &ActionType) -> bool {
    tier_for_action(action).is_ai_executable()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_ordering_is_correct() {
        assert!(ActionTier::Tier0Never < ActionTier::Tier1High);
        assert!(ActionTier::Tier1High < ActionTier::Tier2Medium);
        assert!(ActionTier::Tier2Medium < ActionTier::Tier3Low);
    }

    #[test]
    fn tier_byte_roundtrip() {
        for tier in [
            ActionTier::Tier0Never,
            ActionTier::Tier1High,
            ActionTier::Tier2Medium,
            ActionTier::Tier3Low,
        ] {
            let byte = tier.to_byte();
            let decoded = ActionTier::from_byte(byte);
            assert_eq!(decoded, Some(tier), "Tier {tier:?} roundtrip failed");
        }
    }

    #[test]
    fn tier_from_byte_invalid() {
        assert_eq!(ActionTier::from_byte(4), None);
        assert_eq!(ActionTier::from_byte(255), None);
    }

    #[test]
    fn action_type_byte_roundtrip() {
        for action in ActionType::all() {
            let byte = action.to_byte();
            let decoded = ActionType::from_byte(byte);
            assert_eq!(
                decoded,
                Some(*action),
                "ActionType {action:?} roundtrip failed"
            );
        }
    }

    #[test]
    fn action_type_from_byte_invalid() {
        // Test gaps in the enum values
        assert_eq!(ActionType::from_byte(2), None);
        assert_eq!(ActionType::from_byte(9), None);
        assert_eq!(ActionType::from_byte(13), None);
        assert_eq!(ActionType::from_byte(19), None);
        assert_eq!(ActionType::from_byte(23), None);
        assert_eq!(ActionType::from_byte(255), None);
    }

    #[test]
    fn tier_0_actions_are_never_executable() {
        let tier0_actions = [
            ActionType::ModifyConsensusRule,
            ActionType::ModifyStateTransition,
        ];

        for action in tier0_actions {
            let tier = tier_for_action(&action);
            assert_eq!(
                tier,
                ActionTier::Tier0Never,
                "{action:?} should be Tier0Never"
            );
            assert!(
                !is_ai_executable(&action),
                "{action:?} should not be AI executable"
            );
        }
    }

    #[test]
    fn tier_1_actions_are_high_security() {
        let tier1_actions = [
            ActionType::UpdateBaseFee,
            ActionType::UpdateBlockLimit,
            ActionType::ActivateModule,
        ];

        for action in tier1_actions {
            let tier = tier_for_action(&action);
            assert_eq!(
                tier,
                ActionTier::Tier1High,
                "{action:?} should be Tier1High"
            );
            assert!(
                is_ai_executable(&action),
                "{action:?} should be AI executable"
            );
        }
    }

    #[test]
    fn tier_2_actions_are_medium_security() {
        let tier2_actions = [
            ActionType::UpdatePeerScoring,
            ActionType::UpdateSpamThreshold,
            ActionType::EmitAuditReport,
        ];

        for action in tier2_actions {
            let tier = tier_for_action(&action);
            assert_eq!(
                tier,
                ActionTier::Tier2Medium,
                "{action:?} should be Tier2Medium"
            );
            assert!(
                is_ai_executable(&action),
                "{action:?} should be AI executable"
            );
        }
    }

    #[test]
    fn all_actions_have_tier_mapping() {
        // Exhaustiveness check: every action in all() has a defined tier
        for action in ActionType::all() {
            let tier = tier_for_action(action);
            // Tier should be one of the known values
            assert!(
                matches!(
                    tier,
                    ActionTier::Tier0Never
                        | ActionTier::Tier1High
                        | ActionTier::Tier2Medium
                        | ActionTier::Tier3Low
                ),
                "{action:?} has unknown tier {tier:?}"
            );
        }
    }

    #[test]
    fn action_type_all_returns_all_variants() {
        let all = ActionType::all();
        assert_eq!(all.len(), 8, "Expected 8 action types");

        // Verify all expected actions are present
        assert!(all.contains(&ActionType::ModifyConsensusRule));
        assert!(all.contains(&ActionType::ModifyStateTransition));
        assert!(all.contains(&ActionType::UpdateBaseFee));
        assert!(all.contains(&ActionType::UpdateBlockLimit));
        assert!(all.contains(&ActionType::ActivateModule));
        assert!(all.contains(&ActionType::UpdatePeerScoring));
        assert!(all.contains(&ActionType::UpdateSpamThreshold));
        assert!(all.contains(&ActionType::EmitAuditReport));
    }

    #[test]
    fn default_tier_is_most_restrictive() {
        assert_eq!(ActionTier::default(), ActionTier::Tier0Never);
    }

    #[test]
    fn tier_is_ai_executable_correct() {
        assert!(!ActionTier::Tier0Never.is_ai_executable());
        assert!(ActionTier::Tier1High.is_ai_executable());
        assert!(ActionTier::Tier2Medium.is_ai_executable());
        assert!(ActionTier::Tier3Low.is_ai_executable());
    }
}
