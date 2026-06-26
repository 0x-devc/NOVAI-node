//! F3 GATE 1: RED test (FAILS BY DESIGN at HEAD a0cfbbf6cd15c97a7d2cdb15cf2e6c7aeb3040db).
//!
//! F3 root cause: the consensus layer defaults an absent KEY_SMT_ROOT to
//! [0u8;32] at three sites, while execution and genesis default an absent root
//! to the non-zero empty_hash_at_height(256) (smt/src/hash.rs:28).
//!   1. propose_block: crates/consensus/src/lib.rs:287  ([0u8;32] "Genesis root")
//!   2. verify_block:  crates/consensus/src/lib.rs:438  (the site this test drives)
//!   3. sync C-01:     crates/node/src/consensus_node.rs:942 ([0u8;32] "Genesis state")
//!
//! On a node started WITHOUT --dev-keys the DB is empty (KEY_SMT_ROOT absent),
//! so consensus holds current_root = [0u8;32] and rejects any block carrying the
//! real empty root, returning ConsensusError::InvalidBlock("State root mismatch")
//! at crates/consensus/src/lib.rs:441-444. The sync path reports the same bite as
//! "Sync rejected: state root mismatch" at crates/node/src/consensus_node.rs:945-953.
//! It is masked on the dev-keys testnet only because apply_dev_genesis writes a
//! real KEY_SMT_ROOT first.
//!
//! This test asserts the CORRECT post-fix behavior, so it FAILS at HEAD
//! (documenting the divergence) and will PASS once the three consensus sites
//! default an absent KEY_SMT_ROOT to empty_hash_at_height(256), matching
//! execution and genesis. It contains NO fix.
//!
//! Layer: the [0u8;32] default lives at the consensus-state level, so this drives
//! the real ConsensusState::verify_block against a real, empty novai_state::MemKv,
//! using real types (no mocks). The expected empty root is computed by the real
//! execution function novai_execution::append_smt_ops_for_state_ops, which is the
//! exact value execution and genesis use for an absent root. verify_block runs the
//! identical state-root comparison the sync path (site 3) uses, so a single
//! consensus-level test faithfully reproduces the F3 bite.

use ed25519_dalek::SigningKey;
use novai_consensus::ConsensusState;
use novai_consensus_types::Block;
use novai_crypto::address_from_pubkey;
use novai_state::{Kv, MemKv, WriteOp, KEY_SMT_ROOT};

/// A non-dev-keys node (empty DB) must ACCEPT a height-1 block that carries the
/// canonical empty SMT root, so it can sync from and agree with a real chain.
///
/// At HEAD this FAILS: verify_block defaults an absent KEY_SMT_ROOT to [0u8;32]
/// (consensus/src/lib.rs:438) and rejects the non-zero canonical root with
/// "State root mismatch" (consensus/src/lib.rs:441-444). Post-fix the consensus
/// default becomes empty_hash_at_height(256) and the block is accepted.
#[test]
fn empty_db_accepts_canonical_empty_smt_root() {
    // A deterministic validator address. No genesis is applied: this is the
    // non-dev-keys condition, an empty DB, not the dev-keys path.
    let our_addr = address_from_pubkey(&SigningKey::from_bytes(&[0u8; 32]).verifying_key());

    // The non-dev-keys start: an EMPTY state DB. KEY_SMT_ROOT must be absent.
    let db = MemKv::new();
    assert!(
        db.get(KEY_SMT_ROOT).expect("db read").is_none(),
        "precondition: an empty DB must have KEY_SMT_ROOT absent (the non-dev-keys start)"
    );

    // The canonical empty root, computed by the execution layer exactly as
    // genesis and block execution compute it for an absent root. This call takes
    // &db (read-only) and writes only into the discarded out_ops vec, so it does
    // NOT mutate db.
    let mut discard: Vec<WriteOp> = Vec::new();
    let canonical_empty_root =
        novai_execution::append_smt_ops_for_state_ops(&db, &[], &mut discard)
            .expect("execution computes the canonical empty SMT root for an empty DB");

    // Trap guard 1 (compares zero to zero): the canonical empty root must be
    // non-zero, otherwise accepting it would be vacuous.
    assert_ne!(
        canonical_empty_root, [0u8; 32],
        "the canonical empty root must be non-zero (blake3 of the empty tree at \
         height 256); a zero value would make this test vacuous"
    );

    // Trap guard 2 (DB silently seeded): deriving the root must not have written
    // KEY_SMT_ROOT; the DB stays empty so verify_block still hits its absent-root
    // default, not a stored value.
    assert!(
        db.get(KEY_SMT_ROOT).expect("db read").is_none(),
        "the DB must remain empty after deriving the root: KEY_SMT_ROOT still absent"
    );

    // Fresh consensus state: height 0, no QC. verify_block's expected_height is 1
    // and expected_parent is the genesis [0u8;32], so a height-1, zero-tx block
    // with the genesis parent passes every check before the state-root comparison
    // and reaches exactly that comparison (consensus/src/lib.rs:430-445). With no
    // txs, no signature checks run after it either, so the state-root comparison
    // is the sole accept/reject variable.
    let state = ConsensusState::new(our_addr);

    let block = Block {
        height: 1,
        round: 0,
        parent_hash: [0u8; 32], // genesis parent (correct; not the state root)
        state_root: canonical_empty_root,
        txs: vec![],
    };

    let result = state.verify_block(&block, &db);

    assert!(
        result.is_ok(),
        "F3: verify_block REJECTED a height-1 block carrying the canonical empty \
         SMT root against an empty DB. Consensus defaults an absent KEY_SMT_ROOT \
         to [0u8;32] (consensus/src/lib.rs:438) while execution and genesis use \
         the non-zero empty_hash_at_height(256); the mismatch is the F3 bite that \
         the sync path reports as 'Sync rejected: state root mismatch' \
         (consensus_node.rs:945-953). Post-fix the consensus default must match \
         execution and the block must be accepted. Got {result:?}"
    );
}
