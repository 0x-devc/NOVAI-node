//! Gate RED test: silent state-root divergence on the QC-driven catch-up commit path.
//!
//! Reproduces the node2 2026-06-26 class of bug in-process. A node that fell
//! behind commits cached blocks through the QC / 3-chain catch-up path
//! (`handle_qc` -> `cache_qc_and_check_commit` -> `persist_commit_atomic` ->
//! `apply_commits` -> `execute_committed_blocks`) WITHOUT ever comparing its own
//! current SMT root against the committed block's header `state_root`. The vote
//! path (`verify_block`) and the block-response sync path (C-01) both perform
//! that comparison; the QC-driven catch-up path does not.
//!
//! Header semantics (verified in the fix plan): a block's header `state_root` is
//! the PRE-state of that block, i.e. `post-state(N-1)`. So for the first block
//! about to be committed (`to_commit[0]`, height `committed_height + 1`), a
//! correct node has `current_root == to_commit[0].state_root`. The check is a
//! pre-execution comparison against the block's own header.
//!
//! `catchup_commit_halts_on_stateroot_divergence` is RED today: with a drifted
//! local root injected, the catch-up commit advances committed state silently.
//! It flips GREEN once the pre-execution root check halts the commit.
//!
//! `catchup_commit_proceeds_when_stateroot_matches` is the liveness guard and
//! the harness soundness proof: with a matching root the same setup must still
//! commit (a correct-but-behind node must never be false-halted). It passes
//! today and must keep passing after the fix.

use ed25519_dalek::SigningKey;
use novai_consensus_types::{Block, Vote, QC};
use novai_crypto::address_from_pubkey;
use novai_node::consensus_node::ConsensusNode;
use novai_state::Kv;
use novai_types::Address;
use std::collections::HashMap;

/// A domain-separated, validly signed vote (mirrors the helper in sync_test.rs).
fn signed_vote(
    signer: &SigningKey,
    voter: Address,
    height: u64,
    round: u64,
    block_hash: [u8; 32],
) -> Vote {
    let unsigned = Vote {
        height,
        round,
        block_hash,
        voter,
        signature: [0u8; 64],
        ai_signal_commitment: None,
    };
    let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&unsigned);
    let mut to_sign = Vec::new();
    to_sign.extend_from_slice(b"NOVAI_VOTE_V1");
    to_sign.extend_from_slice(&unsigned_bytes);
    let signature = novai_crypto::sign_bytes(signer, &to_sign);
    Vote {
        signature,
        ..unsigned
    }
}

/// A single-vote QC that certifies `block` (quorum is 1 for a 2-validator set:
/// `2 * ((n - 1) / 3) + 1 == 1` for `n == 2`).
fn certifying_qc(signer: &SigningKey, voter: Address, block: &Block) -> QC {
    let block_hash = novai_consensus_types::block_hash(block);
    QC {
        height: block.height,
        round: block.round,
        block_hash,
        votes: vec![signed_vote(signer, voter, block.height, block.round, block_hash)],
    }
}

/// The next block in a chain, parented to `parent`, with no transactions.
fn next_block(parent: &Block, state_root: [u8; 32]) -> Block {
    Block {
        height: parent.height + 1,
        round: 0,
        parent_hash: novai_consensus_types::block_hash(parent),
        state_root,
        txs: vec![],
    }
}

/// Build a 2-validator node under test (validator B) plus the signing key for
/// validator A, which certifies QCs. Deterministic keys, so no randomness.
fn two_validator_node() -> (ConsensusNode, SigningKey, Address) {
    let sk_a = SigningKey::from_bytes(&[1u8; 32]);
    let sk_b = SigningKey::from_bytes(&[2u8; 32]);
    let addr_a = address_from_pubkey(&sk_a.verifying_key());
    let addr_b = address_from_pubkey(&sk_b.verifying_key());
    let validator_set = vec![addr_a, addr_b];
    let mut validator_pubkeys = HashMap::new();
    validator_pubkeys.insert(addr_a, sk_a.verifying_key());
    validator_pubkeys.insert(addr_b, sk_b.verifying_key());
    let node = ConsensusNode::new(sk_b, validator_set, validator_pubkeys, 1000);
    (node, sk_a, addr_a)
}

/// Cache a genesis-rooted 3-block chain (heights 1, 2, 3) whose only correct
/// pre-state that matters is block 1's (`empty_smt_root()`), and return the QC
/// for height 3. Delivering that QC drives the 3-chain rule to commit exactly
/// height 1 (`commit_target = qc_height - 2 = 1`).
fn cache_three_block_chain(node: &ConsensusNode, sk_a: &SigningKey, addr_a: Address) -> QC {
    let empty = novai_execution::empty_smt_root();
    let block1 = Block {
        height: 1,
        round: 0,
        parent_hash: [0u8; 32],
        state_root: empty, // correct pre-state of height 1 (genesis empty root)
        txs: vec![],
    };
    let block2 = next_block(&block1, empty);
    let block3 = next_block(&block2, empty);
    let qc3 = certifying_qc(sk_a, addr_a, &block3);
    {
        let mut state = node.state.lock().unwrap();
        state.cache_block(block1).unwrap();
        state.cache_block(block2).unwrap();
        state.cache_block(block3).unwrap();
    }
    qc3
}

/// RED today, GREEN after the fix.
///
/// The node's persisted `KEY_SMT_ROOT` is drifted to a value that does not match
/// the pre-state that block 1's header declares. Today the catch-up commit path
/// has no root check, so it advances committed state silently. The fix must make
/// the commit HALT (no advance, safety error) instead.
#[test]
fn catchup_commit_halts_on_stateroot_divergence() {
    let (node, sk_a, addr_a) = two_validator_node();
    let qc3 = cache_three_block_chain(&node, &sk_a, addr_a);

    // Inject the divergence: overwrite the local root with a value that is not
    // block 1's declared pre-state. This stands in for node2's already-drifted
    // executed state without reproducing the (separate, still-open) root cause.
    let drift = [0x99u8; 32];
    assert_ne!(
        drift,
        novai_execution::empty_smt_root(),
        "the injected drift root must differ from block 1's pre-state, or the test is vacuous"
    );
    {
        let mut db = node.db.lock().unwrap();
        db.put(
            novai_state::KEY_SMT_ROOT,
            &novai_state::encode_smt_root_v1(&drift),
        )
        .unwrap();
    }

    let committed_before = node.state.lock().unwrap().committed_height;
    assert_eq!(committed_before, 0, "precondition: fresh node at committed_height 0");

    // Drive the QC-driven catch-up commit (the unchecked path node2 was on).
    let result = node.handle_qc(qc3);

    let committed_after = node.state.lock().unwrap().committed_height;

    // Required post-fix behavior: the catch-up commit must detect that its
    // current root does not match the committed block's declared pre-state and
    // HALT, leaving committed_height unchanged.
    assert_eq!(
        committed_after, 0,
        "SILENT DIVERGENCE: catch-up commit advanced committed_height 0 -> {committed_after} \
         while current_root (injected drift) != to_commit[0].state_root (block 1 pre-state). \
         The catch-up commit path must HALT on a pre-state mismatch, not advance. result={result:?}"
    );
    assert!(
        result.is_err(),
        "catch-up commit must return a safety error on a state-root pre-state mismatch; \
         got Ok (silent commit)"
    );
}

/// Liveness guard and harness soundness proof: with a matching root the same
/// setup must still commit height 1. A correct-but-behind node must never be
/// false-halted. Passes today and must keep passing after the fix.
#[test]
fn catchup_commit_proceeds_when_stateroot_matches() {
    let (node, sk_a, addr_a) = two_validator_node();
    let qc3 = cache_three_block_chain(&node, &sk_a, addr_a);

    // No drift: the fresh node has no KEY_SMT_ROOT, which defaults to
    // empty_smt_root() (the F3 canonical empty root), matching block 1's
    // pre-state. This is the genesis catch-up scenario.
    let committed_before = node.state.lock().unwrap().committed_height;
    assert_eq!(committed_before, 0, "precondition: fresh node at committed_height 0");

    let result = node.handle_qc(qc3);

    let committed_after = node.state.lock().unwrap().committed_height;
    assert!(
        result.is_ok(),
        "correct-root catch-up must succeed (no false halt); got {result:?}"
    );
    assert_eq!(
        committed_after, 1,
        "correct-root catch-up must advance committed_height to 1 (3-chain commit of height 1)"
    );
}
