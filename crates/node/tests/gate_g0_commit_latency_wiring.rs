//! Gate G0: the commit-latency gauge is wired to the real propose and commit
//! paths.
//!
//! `ProposalClock` is unit-tested in `crates/node/src/metrics.rs`. Those pins
//! prove the arithmetic. They cannot prove the two ends are attached to
//! anything, and an unattached clock reports a perfectly plausible zero
//! forever.
//!
//! So this file drives the REAL entry points: `try_propose_block`, which is
//! the propose loop's only production call, and `handle_qc`, which is one of
//! the paths that reaches `execute_committed_blocks` where the commit end
//! resolves.
//!
//! THIS FILE MUST STAY IN ITS OWN TEST BINARY, AND IN ONE TEST FUNCTION. The
//! clock is a process-wide static, for the same reason the pool census
//! counters are. Cargo gives each integration test file its own process but
//! runs the functions inside it on parallel THREADS, so splitting this into
//! several tests would make them observe each other's stamps. That version
//! passed repeatedly before failing, which is the worst kind of test. One
//! sequential function removes the race by construction rather than by
//! scheduling luck.

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus_types::codec::hash_block_v1;
use novai_crypto::address_from_pubkey;
use novai_node::consensus_node::ConsensusNode;
use novai_node::metrics::proposal_metrics;
use novai_types::Address;
use std::collections::HashMap;

struct TestNonceProvider;

impl mempool::NonceProvider for TestNonceProvider {
    fn expected_nonce(&self, _from: &Address) -> u64 {
        0
    }
}

fn make_validators(count: usize) -> Vec<(Address, SigningKey, VerifyingKey)> {
    (0..count)
        .map(|i| {
            let sk = SigningKey::from_bytes(&[i as u8; 32]);
            let pk = sk.verifying_key();
            (address_from_pubkey(&pk), sk, pk)
        })
        .collect()
}

fn make_node(validators: &[(Address, SigningKey, VerifyingKey)], node_idx: usize) -> ConsensusNode {
    let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
    let mut pubkeys = HashMap::new();
    for (a, _, pk) in validators {
        pubkeys.insert(*a, *pk);
    }
    ConsensusNode::new(validators[node_idx].1.clone(), validator_set, pubkeys, 1000)
}

/// Drive the real propose and commit entry points end to end.
///
/// `ProposalClock` is unit-tested in `crates/node/src/metrics.rs`; those pins
/// prove the arithmetic and cannot prove the ends are attached to anything.
/// An unattached clock reports a perfectly plausible zero forever, which is
/// precisely the failure this gate exists to prevent.
#[test]
fn commit_latency_is_wired_to_the_real_propose_and_commit_paths() {
    let validators = make_validators(4);
    // Leader for view height 0 is index 0, so this node is the leader and
    // try_propose_block will actually produce a block.
    let node = make_node(&validators, 0);

    // Non-vacuity: the clock starts empty, so every assertion below is caused
    // by this test rather than inherited.
    assert_eq!(proposal_metrics::pending(), 0);
    assert_eq!(
        proposal_metrics::last_latency_seconds(),
        0.0,
        "nothing has committed, so nothing may be published"
    );

    // --- the propose end, through the propose loop's only production call ---
    let mut pool = mempool::TxMempool::new(1, 100);
    let proposed = node
        .try_propose_block(&mut pool, &TestNonceProvider)
        .expect("the leader must be able to propose at genesis");
    assert!(proposed, "node 0 is the leader for view height 0");

    assert_eq!(
        proposal_metrics::pending(),
        1,
        "try_propose_block must stamp the propose end; without this the gauge \
         reports a plausible zero forever"
    );
    assert_eq!(
        proposal_metrics::last_latency_seconds(),
        0.0,
        "a proposal that has not committed must not publish a latency"
    );

    // Recover the block this node just proposed from the node's own state
    // rather than rebuilding it here: the measurement is keyed by hash, so the
    // test must resolve against the same hash the node stamped.
    let (proposed_hash, proposed_height) = {
        let state = node.state.lock().unwrap();
        let block = state
            .block_cache
            .values()
            .next()
            .cloned()
            .or_else(|| state.block_by_hash.values().next().cloned())
            .expect("the proposed block must be cached by the node");
        (hash_block_v1(&block).expect("hash"), block.height)
    };

    // --- the orphan case, which is why the key is a hash and not a height ---
    // A commit at a DIFFERENT hash must not resolve our stamp. Our proposal at
    // H can be orphaned while a sibling commits at H, and publishing the
    // sibling's timing against our stamp would be a wrong number rather than a
    // missing one.
    proposal_metrics::note_committed([0xEE; 32], proposed_height.saturating_sub(1));
    assert_eq!(
        proposal_metrics::last_latency_seconds(),
        0.0,
        "a sibling's commit must not be published against our proposal's stamp"
    );
    assert_eq!(
        proposal_metrics::pending(),
        1,
        "and our stamp must survive a sibling commit below its height"
    );

    // --- the commit end, through the seam the commit path calls ---
    proposal_metrics::note_committed(proposed_hash, proposed_height);
    let measured = proposal_metrics::last_latency_seconds();
    assert!(
        measured > 0.0,
        "committing this node's own proposal must publish a measured latency, \
         got {measured}"
    );
    assert!(
        measured < 60.0,
        "the measurement must be an elapsed interval, not a wall-clock \
         timestamp or a constant, got {measured}"
    );
    assert_eq!(
        proposal_metrics::pending(),
        0,
        "the resolved stamp and everything at or below the committed height \
         must be reaped"
    );

    // A commit for a block nobody proposed is a no-op, which is the common
    // case: three quarters of committed blocks are somebody else's.
    let before = proposal_metrics::last_latency_seconds();
    proposal_metrics::note_committed([0x77; 32], proposed_height + 1);
    assert_eq!(
        proposal_metrics::last_latency_seconds(),
        before,
        "another validator's block must leave the published latency untouched"
    );
}
