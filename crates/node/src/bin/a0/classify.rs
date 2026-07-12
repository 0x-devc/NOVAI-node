//! Key classification for the audit's completeness rule (check A3).
//!
//! Every key found in a copied DB must match exactly one entry below, derived
//! from the write-path analysis of HEAD 9750c79 (F4 diagnosis, execute-gate
//! recon). Any key matching nothing is a hard audit failure, named in the
//! report: fail closed, never guess.
//!
//! SmtCommitted: written in production ONLY via apply_state_ops_with_smt
//! (crates/execution/src/lib.rs:6602-6610); the sole production apply_batch
//! in execution is that helper's own (:6609), and its fifteen call sites
//! (:6720, :6761, :6872, :6896, :7086, :7170, :7214, :9043, :9181, :9295,
//! :9442, :9562, :10720, :10892, :11157) cover every state handler,
//! including the AI kill switch (:7169-7170). These keys are the leaf set the
//! rebuild consumes.
//!
//! Operational: consensus and SMT infrastructure rows, written by
//! persist_commit_atomic (crates/consensus/src/lib.rs:1928-2082), the
//! executed-height put (crates/node/src/main.rs:298-299), the vote fsync
//! (consensus lib persist_voted_view), the receipt-time block stores
//! (consensus_node.rs:1976, :2025), and the SMT node/root writes themselves.
//! Excluded from the rebuild; expected present.
//!
//! DefinedUnwritten: prefixes with constants in novai_state but ZERO
//! production write paths at this HEAD (mark_nullifier_spent,
//! write_derived_view_ops, and read_derived_view_with_audit have no
//! production callers; nnpx_commitment_key and nnpx_encrypted_key have no
//! writers at all). Presence in a copy means an older binary or a manual
//! write put them there; provenance is unknown, so the audit fails closed.

use novai_state::{
    KEY_AI_KILL_SWITCH, KEY_COMMITTED_HEIGHT, KEY_EXECUTED_HEIGHT, KEY_FEE_POOL, KEY_HIGHEST_QC,
    KEY_LOCKED_QC, KEY_PREFIX_ACCOUNTS, KEY_PREFIX_AI_DELEGATIONS_BY_DELEGATE,
    KEY_PREFIX_AI_ENTITIES, KEY_PREFIX_AI_ENTITY_BY_ADDR, KEY_PREFIX_AI_MEMORY,
    KEY_PREFIX_AI_MEMORY_BY_TYPE, KEY_PREFIX_AI_MEMORY_COUNT, KEY_PREFIX_AI_MEMORY_OBJECTS,
    KEY_PREFIX_AI_PARAMS, KEY_PREFIX_AI_SIGNALS, KEY_PREFIX_AI_SIGNALS_BY_ISSUER,
    KEY_PREFIX_AI_SIGNALS_BY_TYPE, KEY_PREFIX_APPROVAL_GATES, KEY_PREFIX_BLOCKS,
    KEY_PREFIX_DERIVED_VIEWS, KEY_PREFIX_GOVERNANCE_LOG, KEY_PREFIX_GOVERNANCE_PROPOSALS,
    KEY_PREFIX_GOVERNANCE_PROPOSALS_BY_STATE, KEY_PREFIX_NNPX, KEY_PREFIX_QCS,
    KEY_PREFIX_SMT_NODE, KEY_SMT_ROOT, KEY_VOTED_VIEW,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Authenticated state: input to the SMT rebuild.
    SmtCommitted,
    /// Consensus / SMT infrastructure: expected, excluded from the rebuild.
    Operational,
    /// Defined in the schema but never written by any production path at
    /// this HEAD: presence fails the audit.
    DefinedUnwritten,
}

/// Classify a key, or None when it matches nothing documented (hard fail).
pub fn classify(key: &[u8]) -> Option<Class> {
    // Exact keys first.
    if key == KEY_SMT_ROOT
        || key == KEY_COMMITTED_HEIGHT
        || key == KEY_EXECUTED_HEIGHT
        || key == KEY_HIGHEST_QC
        || key == KEY_LOCKED_QC
        || key == KEY_VOTED_VIEW
    {
        return Some(Class::Operational);
    }
    if key == KEY_FEE_POOL || key == KEY_AI_KILL_SWITCH {
        return Some(Class::SmtCommitted);
    }

    // Operational prefixes.
    for p in [KEY_PREFIX_SMT_NODE, KEY_PREFIX_BLOCKS, KEY_PREFIX_QCS] {
        if key.starts_with(p) {
            return Some(Class::Operational);
        }
    }

    // SMT-committed prefixes. More specific entries precede their parents
    // where prefixes nest (the ai/signals/ index family).
    for p in [
        KEY_PREFIX_ACCOUNTS,
        KEY_PREFIX_AI_SIGNALS_BY_TYPE,
        KEY_PREFIX_AI_SIGNALS_BY_ISSUER,
        KEY_PREFIX_AI_SIGNALS,
        KEY_PREFIX_AI_ENTITY_BY_ADDR,
        KEY_PREFIX_AI_ENTITIES,
        KEY_PREFIX_AI_MEMORY_OBJECTS,
        KEY_PREFIX_AI_MEMORY_COUNT,
        KEY_PREFIX_AI_MEMORY_BY_TYPE,
        KEY_PREFIX_AI_MEMORY,
        KEY_PREFIX_AI_PARAMS,
        KEY_PREFIX_AI_DELEGATIONS_BY_DELEGATE,
        KEY_PREFIX_APPROVAL_GATES,
        KEY_PREFIX_GOVERNANCE_PROPOSALS_BY_STATE,
        KEY_PREFIX_GOVERNANCE_PROPOSALS,
        KEY_PREFIX_GOVERNANCE_LOG,
    ] {
        if key.starts_with(p) {
            return Some(Class::SmtCommitted);
        }
    }

    // Defined in the schema, unwritten by any production path at this HEAD.
    for p in [KEY_PREFIX_DERIVED_VIEWS, KEY_PREFIX_NNPX] {
        if key.starts_with(p) {
            return Some(Class::DefinedUnwritten);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_keys_classify() {
        assert_eq!(classify(b"smt/root"), Some(Class::Operational));
        assert_eq!(classify(b"fee_pool"), Some(Class::SmtCommitted));
        assert_eq!(classify(b"ai/kill_switch"), Some(Class::SmtCommitted));
        assert_eq!(
            classify(b"consensus/voted_view"),
            Some(Class::Operational)
        );
    }

    #[test]
    fn nested_signal_indexes_classify_before_parent() {
        assert_eq!(
            classify(b"ai/signals/by_type/x"),
            Some(Class::SmtCommitted)
        );
        assert_eq!(classify(b"ai/signals/plain"), Some(Class::SmtCommitted));
    }

    #[test]
    fn unwritten_and_unknown() {
        assert_eq!(
            classify(b"derived_views/abc"),
            Some(Class::DefinedUnwritten)
        );
        assert_eq!(classify(b"nnpx/commitments/x"), Some(Class::DefinedUnwritten));
        assert_eq!(classify(b"wat/unknown"), None);
        assert_eq!(classify(b"ai/undocumented_family/x"), None);
    }
}
