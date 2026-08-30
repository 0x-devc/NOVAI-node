//! Gate G0: the PRODUCTION commit path publishes the commit latency.
//!
//! `gate_g0_commit_latency_wiring.rs` proves the propose end stamps and that
//! the resolve seam behaves. It cannot prove the production commit path calls
//! that seam, because it calls the seam itself: deleting the call inside
//! `execute_committed_blocks` leaves it green. That is exactly the kind of
//! pass this gate exists to distrust.
//!
//! This file drives a real single-validator chain to a real commit and
//! asserts the latency appears WITHOUT ever touching the seam.
//!
//! SEPARATE TEST BINARY, ON PURPOSE. The clock is a process-wide static, so
//! this drive and the stamp test would see each other's stamps if they shared
//! a process. Cargo gives each integration test file its own process, which is
//! the isolation this needs, and one test function per file is what keeps it.

use ed25519_dalek::SigningKey;
use novai_consensus_types::codec::{encode_vote_v1_unsigned, hash_block_v1};
use novai_consensus_types::{Block, Vote, QC};
use novai_crypto::{address_from_pubkey, sign_bytes};
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

/// A domain-separated, validly signed vote from the sole validator.
fn solo_vote(sk: &SigningKey, voter: Address, block: &Block) -> Vote {
    let bh = hash_block_v1(block).expect("hash");
    let unsigned = Vote {
        height: block.height,
        round: block.round,
        block_hash: bh,
        voter,
        signature: [0u8; 64],
        ai_signal_commitment: None,
    };
    let bytes = encode_vote_v1_unsigned(&unsigned);
    let mut to_sign = Vec::new();
    to_sign.extend_from_slice(b"NOVAI_VOTE_V1");
    to_sign.extend_from_slice(&bytes);
    Vote {
        signature: sign_bytes(sk, &to_sign),
        ..unsigned
    }
}

/// The single-vote QC that certifies `block`. Quorum is 1 at n=1.
fn solo_qc(sk: &SigningKey, voter: Address, block: &Block) -> QC {
    QC {
        height: block.height,
        round: block.round,
        block_hash: hash_block_v1(block).expect("hash"),
        votes: vec![solo_vote(sk, voter, block)],
    }
}

/// The block this node cached at `height`, whatever the real proposer built.
fn cached_block_at(node: &ConsensusNode, height: u64) -> Block {
    let state = node.state.lock().unwrap();
    state
        .block_by_hash
        .values()
        .chain(state.block_cache.values())
        .find(|b| b.height == height)
        .map(|b| (**b).clone())
        .unwrap_or_else(|| panic!("no cached block at height {height}"))
}

#[test]
fn a_real_commit_publishes_the_latency_without_the_test_touching_the_seam() {
    // n = 1, so this node is the leader at every height and quorum is 1.
    let sk = SigningKey::from_bytes(&[7u8; 32]);
    let pk = sk.verifying_key();
    let addr = address_from_pubkey(&pk);
    let mut pubkeys = HashMap::new();
    pubkeys.insert(addr, pk);
    let node = ConsensusNode::new(sk.clone(), vec![addr], pubkeys, 1000);
    let mut pool = mempool::TxMempool::new(1, 1000);

    // Commit of H needs a QC at H+2 under the three-chain rule, so drive three
    // heights. Every proposal goes through the REAL try_propose_block, which
    // is where the stamp lives.
    let mut blocks = Vec::new();
    for expected_height in 1..=3u64 {
        let proposed = node
            .try_propose_block(&mut pool, &TestNonceProvider)
            .unwrap_or_else(|e| panic!("propose at {expected_height}: {e}"));
        assert!(proposed, "n=1 is always the leader at height {expected_height}");
        let block = cached_block_at(&node, expected_height);
        node.handle_qc(solo_qc(&sk, addr, &block))
            .unwrap_or_else(|e| panic!("adopt qc {expected_height}: {e}"));
        blocks.push(block);
    }

    let committed = node.state.lock().unwrap().committed_height;
    assert!(
        committed >= 1,
        "harness soundness: the drive must actually reach a commit, got \
         committed_height {committed}. Without this the latency assertion \
         below would be vacuous."
    );

    // The seam was never called by this test. Any latency published here came
    // through execute_committed_blocks on the production commit path.
    let measured = proposal_metrics::last_latency_seconds();
    assert!(
        measured > 0.0,
        "a real commit of this node's own proposal must publish a latency; \
         got {measured}, which is what an unwired commit end looks like"
    );
    assert!(
        measured < 60.0,
        "the value must be an elapsed interval, not a timestamp: {measured}"
    );

    // The reap must retire exactly the committed height and below, and not a
    // height more. Three heights were proposed and stamped; committing H
    // retires the stamps at or below H and must leave the ones above it alive,
    // because those can still commit and are still measurable. An off-by-one
    // in the reap height silently drops the next measurement instead of
    // producing a wrong one, which is the kind of defect that shows up as a
    // gauge that is merely quiet.
    let still_pending = proposal_metrics::pending() as usize;
    let expected_pending = blocks.iter().filter(|b| b.height > committed).count();
    assert_eq!(
        still_pending, expected_pending,
        "after committing height {committed}, exactly the stamps above it must \
         remain: expected {expected_pending}, got {still_pending}"
    );
    assert!(
        expected_pending > 0,
        "harness soundness: the drive must leave at least one stamp above the \
         committed height, or the reap boundary is untested"
    );
}
