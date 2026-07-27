//! Commit-window rule (WEDGE-20260718), node-level entry points.
//!
//! The engine-level matrix lives in
//! crates/consensus/tests/gate_commit_window.rs. Here I drive the two REAL
//! production entry points the rule gates:
//!
//! - the follower vote path: handle_proposal, which runs verify_block and
//!   then note_self_vote plus the synced durable-mark persist, and
//! - the leader path: try_propose_block, whose intent check runs before the
//!   mempool is drained and before any self vote is recorded.
//!
//! The load-bearing assertions are about the durable vote mark: a refusal
//! above the window must leave voted_view untouched in memory AND on disk,
//! because bounded marks are exactly what keeps a future commit stall
//! restart recoverable instead of requiring fleet-wide offline surgery.

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus_types::codec::{
    encode_proposal_v1_unsigned, encode_vote_v1_unsigned, encode_voted_view_v1, hash_block_v1,
};
use novai_consensus_types::{Block, Proposal, SignedProposal, Vote, QC};
use novai_crypto::{address_from_pubkey, sign_bytes};
use novai_node::consensus_node::ConsensusNode;
use novai_state::{Kv, KEY_VOTED_VIEW};
use novai_types::Address;
use std::collections::HashMap;

/// The spec value, pinned locally so this file compiles and fails on
/// behavior against a tree that predates the rule.
const W: u64 = 1024;

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

fn make_block(height: u64, parent_hash: [u8; 32]) -> Block {
    Block {
        height,
        round: 0,
        parent_hash,
        state_root: novai_execution::empty_smt_root(),
        txs: vec![],
    }
}

/// A vote at (height, 0) over block_hash, properly signed with the voter's
/// key under the vote domain tag, so verify_qc_well_formed accepts it.
fn signed_vote(
    height: u64,
    block_hash: [u8; 32],
    signer: &(Address, SigningKey, VerifyingKey),
) -> Vote {
    let mut vote = Vote {
        height,
        round: 0,
        block_hash,
        voter: signer.0,
        signature: [0u8; 64],
        ai_signal_commitment: None,
    };
    let unsigned = encode_vote_v1_unsigned(&vote);
    let mut to_sign = Vec::with_capacity(unsigned.len() + 13);
    to_sign.extend_from_slice(b"NOVAI_VOTE_V1");
    to_sign.extend_from_slice(&unsigned);
    vote.signature = sign_bytes(&signer.1, &to_sign);
    vote
}

/// A fully verifiable QC at (height, 0) over block_hash: three distinct
/// in-set voters, every vote bound and signed. Quorum for n=4 is 3.
fn signed_qc(
    height: u64,
    block_hash: [u8; 32],
    validators: &[(Address, SigningKey, VerifyingKey)],
) -> QC {
    QC {
        height,
        round: 0,
        block_hash,
        votes: (0..3)
            .map(|i| signed_vote(height, block_hash, &validators[i]))
            .collect(),
    }
}

/// A signed proposal for `block` justified by `justify_qc`, signed by the
/// expected leader for the block's view (index (height - 1 + 0) % 4).
fn signed_proposal(
    block: Block,
    justify_qc: QC,
    validators: &[(Address, SigningKey, VerifyingKey)],
) -> SignedProposal {
    let leader_idx = ((block.height - 1) as usize) % validators.len();
    let proposal = Proposal { block, justify_qc };
    let unsigned = encode_proposal_v1_unsigned(&proposal).expect("encode proposal");
    let signature = sign_bytes(&validators[leader_idx].1, &unsigned);
    SignedProposal {
        proposer: validators[leader_idx].0,
        proposal,
        signature,
    }
}

#[test]
fn follower_refuses_vote_above_commit_window() {
    let validators = make_validators(4);
    // The leader for a block at W + 1 is index (W + 0) % 4 = 0; the node
    // under test is a follower.
    let node = make_node(&validators, 1);

    // The fleet's frontier arrives as the proposal's justify QC: a fully
    // valid QC at height W over a parent this follower has never executed.
    let parent = make_block(W, [0xAA; 32]);
    let parent_hash = hash_block_v1(&parent).expect("hash parent");
    let justify = signed_qc(W, parent_hash, &validators);
    let candidate = make_block(W + 1, parent_hash);
    let signed = signed_proposal(candidate, justify, &validators);

    let res = node.handle_proposal(signed);

    // The follower must refuse to vote: its committed height is 0 and the
    // block is W + 1 heights above it.
    assert!(
        res.is_err(),
        "the follower voted at height {} with committed height 0; the \
         commit window rule must refuse the vote",
        W + 1
    );
    let msg = res.unwrap_err();
    assert!(
        msg.contains("commit window"),
        "the refusal must name the commit window rule, got: {msg}"
    );

    // The refusal must precede every durable vote effect.
    {
        let state = node.state.lock().unwrap();
        assert_eq!(
            state.voted_view, None,
            "a window-refused vote must not advance the in-memory mark"
        );
        // Frontier adoption stays ungated: the justify QC was verified and
        // adopted before the vote decision, exactly as in live operation.
        assert_eq!(
            state.highest_qc.as_ref().map(|q| q.height),
            Some(W),
            "the follower must still adopt the frontier QC it cannot vote past"
        );
    }
    {
        let db = node.db.lock().unwrap();
        assert_eq!(
            db.get(KEY_VOTED_VIEW).unwrap(),
            None,
            "a window-refused vote must leave no durable vote mark on disk"
        );
    }
}

#[test]
fn follower_votes_at_exactly_the_window_bound() {
    let validators = make_validators(4);
    // The leader for a block at W is index (W - 1) % 4 = 3; validator 1 is
    // again a follower.
    let node = make_node(&validators, 1);

    let parent = make_block(W - 1, [0xAA; 32]);
    let parent_hash = hash_block_v1(&parent).expect("hash parent");
    let justify = signed_qc(W - 1, parent_hash, &validators);
    let candidate = make_block(W, parent_hash);
    let signed = signed_proposal(candidate, justify, &validators);

    node.handle_proposal(signed)
        .expect("a block at exactly committed + window must still be votable");

    {
        let state = node.state.lock().unwrap();
        assert_eq!(state.voted_view, Some((W, 0)));
    }
    {
        let db = node.db.lock().unwrap();
        assert_eq!(
            db.get(KEY_VOTED_VIEW).unwrap(),
            Some(encode_voted_view_v1(W, 0)),
            "the boundary vote is a real vote: durably marked before broadcast"
        );
    }
}

#[test]
fn leader_refuses_to_propose_above_commit_window() {
    let validators = make_validators(4);
    // propose_block selects the leader at view height max(height, hqc) = W,
    // index (W + 0) % 4 = 0, so the node under test IS the leader.
    let node = make_node(&validators, 0);

    let parent = make_block(W, [0xAA; 32]);
    let parent_hash = hash_block_v1(&parent).expect("hash parent");
    {
        let mut state = node.state.lock().unwrap();
        let qc = QC {
            height: W,
            round: 0,
            block_hash: parent_hash,
            votes: vec![],
        };
        state.highest_qc = Some(qc.clone());
        state.locked_qc = Some(qc);
    }

    let mut pool = mempool::TxMempool::new(1, 100);
    let refused = node
        .try_propose_block(&mut pool, &TestNonceProvider)
        .expect("the intent refusal is a quiet skip, never an error");
    assert!(
        !refused,
        "the leader proposed at height {} with committed height 0; the \
         commit window rule must park the proposer",
        W + 1
    );
    // The refusal repeats quietly on the next tick (the parked fleet churns
    // rounds on the 5 ms propose cadence; this must stay a skip, not an
    // error and not a second warning storm).
    let refused_again = node
        .try_propose_block(&mut pool, &TestNonceProvider)
        .expect("the repeated intent refusal is still a quiet skip");
    assert!(!refused_again);

    {
        let state = node.state.lock().unwrap();
        assert_eq!(
            state.voted_view, None,
            "a parked leader must not self-vote; the durable mark stays put"
        );
        assert_eq!(
            state.last_proposed, None,
            "a parked leader must not burn its proposal slot"
        );
    }
    {
        let db = node.db.lock().unwrap();
        assert_eq!(
            db.get(KEY_VOTED_VIEW).unwrap(),
            None,
            "a parked leader must leave no durable vote mark on disk"
        );
    }
}

#[test]
fn leader_proposes_at_exactly_the_window_bound() {
    let validators = make_validators(4);
    // With the frontier at W - 1 the intended height is exactly W, and the
    // leader is index (W - 1 + 0) % 4 = 3.
    let node = make_node(&validators, 3);

    let parent = make_block(W - 1, [0xAA; 32]);
    let parent_hash = hash_block_v1(&parent).expect("hash parent");
    {
        let mut state = node.state.lock().unwrap();
        let qc = QC {
            height: W - 1,
            round: 0,
            block_hash: parent_hash,
            votes: vec![],
        };
        state.highest_qc = Some(qc.clone());
        state.locked_qc = Some(qc);
    }

    let mut pool = mempool::TxMempool::new(1, 100);
    let proposed = node
        .try_propose_block(&mut pool, &TestNonceProvider)
        .expect("proposing at the bound must succeed");
    assert!(
        proposed,
        "a block at exactly committed + window is still proposable; the \
         window must not cost a height of legitimate progress"
    );

    let state = node.state.lock().unwrap();
    assert_eq!(
        state.voted_view,
        Some((W, 0)),
        "the boundary proposal self-vote is a real vote with a durable mark"
    );
}
