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
use novai_consensus_types::{block_hash, Block, BlockResponse, Vote, QC};
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

/// Establish a committed base at height 1: block1 (empty post-state root)
/// persisted as the committed tip with `KEY_SMT_ROOT` set to match its header,
/// `committed_height` = 1, then cache the pending chain 2,3,4 and return the QC for
/// height 4. Delivering that QC drives the 3-chain rule to commit exactly height 2
/// (`commit_target = qc_height - 2 = 2`), whose PARENT is the committed tip block1.
/// The lag-0 parent-header pre-commit check (gate wedge-276272) then compares
/// block1's post-state header against the local root.
fn committed_at_1_plus_pending(node: &ConsensusNode, sk_a: &SigningKey, addr_a: Address) -> QC {
    let empty = novai_execution::empty_smt_root();
    let block1 = Block {
        height: 1,
        round: 0,
        parent_hash: [0u8; 32],
        state_root: empty, // post-state(1) == committed root (empty chain)
        txs: vec![],
    };
    {
        let mut db = node.db.lock().unwrap();
        db.put(
            &novai_state::block_key(1),
            &novai_consensus_types::codec::encode_block_v1(&block1).unwrap(),
        )
        .unwrap();
        db.put(
            novai_state::KEY_SMT_ROOT,
            &novai_state::encode_smt_root_v1(&empty),
        )
        .unwrap();
    }
    let block2 = next_block(&block1, empty);
    let block3 = next_block(&block2, empty);
    let block4 = next_block(&block3, empty);
    let qc4 = certifying_qc(sk_a, addr_a, &block4);
    {
        let mut state = node.state.lock().unwrap();
        state.committed_height = 1;
        state.cache_block(block1).unwrap();
        state.cache_block(block2).unwrap();
        state.cache_block(block3).unwrap();
        state.cache_block(block4).unwrap();
    }
    qc4
}

/// The catch-up commit must HALT when the local root has diverged from the
/// committed TIP's post-state header (the lag-0 parent-header check, gate
/// wedge-276272). Migrated from the pre-flip form (which compared to_commit[0]'s
/// own header); both assertions, halt-on-divergence and the no-false-halt twin,
/// are preserved.
#[test]
fn catchup_commit_halts_on_stateroot_divergence() {
    let (node, sk_a, addr_a) = two_validator_node();
    let qc4 = committed_at_1_plus_pending(&node, &sk_a, addr_a);

    // Inject the divergence: overwrite the local root so it no longer matches the
    // committed tip header (block1's post-state root).
    let drift = [0x99u8; 32];
    assert_ne!(
        drift,
        novai_execution::empty_smt_root(),
        "the injected drift must differ from the committed tip's post-state root, or it is vacuous"
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
    assert_eq!(committed_before, 1, "precondition: committed at the tip (height 1)");

    let result = node.handle_qc(qc4);
    let committed_after = node.state.lock().unwrap().committed_height;

    assert_eq!(
        committed_after, 1,
        "SILENT DIVERGENCE: the catch-up commit of height 2 advanced committed_height 1 -> \
         {committed_after} while the committed tip header (block1) no longer matches the local \
         root. It must HALT, not advance. result={result:?}"
    );
    assert!(
        result.is_err(),
        "the catch-up commit must return a safety error on a committed-tip divergence; got Ok"
    );
    let msg = format!("{result:?}");
    assert!(
        msg.contains("CONSENSUS SAFETY HALT: catch-up commit state root mismatch"),
        "the halt must be the catch-up commit halt; got: {msg}"
    );
}

/// Liveness guard / no-false-halt: with the local root matching the committed tip
/// header, the same catch-up commit must proceed (committed advances to 2). A
/// correct-but-behind node must never be false-halted.
#[test]
fn catchup_commit_proceeds_when_stateroot_matches() {
    let (node, sk_a, addr_a) = two_validator_node();
    let qc4 = committed_at_1_plus_pending(&node, &sk_a, addr_a);
    // No drift: KEY_SMT_ROOT already equals block1's post-state root (empty).

    let committed_before = node.state.lock().unwrap().committed_height;
    assert_eq!(committed_before, 1, "precondition: committed at the tip (height 1)");

    let result = node.handle_qc(qc4);
    let committed_after = node.state.lock().unwrap().committed_height;

    assert!(
        result.is_ok(),
        "a correct-root catch-up must succeed (no false halt); got {result:?}"
    );
    assert_eq!(
        committed_after, 2,
        "correct-root catch-up must commit height 2 (3-chain commit at commit_target 2)"
    );
}

/// Site 4 guard (gate wedge-276272): the sync C-01 check is a LOCAL self-check
/// under the post-state convention: the committed tip's header must equal the local
/// root. If the local root has drifted from the committed tip header, sync must
/// reject (local divergence), regardless of the incoming block's own root. The
/// incoming block here carries the DRIFTED root, so the old first-synced-block
/// comparison would pass; only the local-tip self-check catches it.
#[test]
fn sync_rejects_when_local_tip_header_diverges_from_local_root() {
    let (node, sk_a, addr_a) = two_validator_node();
    let empty = novai_execution::empty_smt_root();
    let block1 = Block {
        height: 1,
        round: 0,
        parent_hash: [0u8; 32],
        state_root: empty, // committed tip header (post-state)
        txs: vec![],
    };
    let drift = [0x99u8; 32];
    {
        let mut db = node.db.lock().unwrap();
        db.put(
            &novai_state::block_key(1),
            &novai_consensus_types::codec::encode_block_v1(&block1).unwrap(),
        )
        .unwrap();
        // Local root drifted away from the committed tip header.
        db.put(
            novai_state::KEY_SMT_ROOT,
            &novai_state::encode_smt_root_v1(&drift),
        )
        .unwrap();
    }
    {
        let mut state = node.state.lock().unwrap();
        state.committed_height = 1;
        state.cache_block(block1.clone()).unwrap();
    }

    // A sync response at height 2 that connects to the committed tip, whose OWN root
    // equals the drifted local root (so the old blocks[0] comparison would pass).
    let block2 = Block {
        height: 2,
        round: 0,
        parent_hash: block_hash(&block1),
        state_root: drift,
        txs: vec![],
    };
    let response = BlockResponse {
        responder: addr_a,
        request_start: 2,
        request_end: 2,
        blocks: vec![block2.clone()],
        qcs: vec![Some(certifying_qc(&sk_a, addr_a, &block2))],
    };

    let err = node.handle_block_response(response).expect_err(
        "sync must reject when the local committed-tip header diverges from the local root",
    );
    assert!(
        err.contains("local committed-tip header"),
        "the rejection must be the local-tip self-check (site 4), got: {err}"
    );
}
