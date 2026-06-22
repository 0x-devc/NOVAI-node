//! F2 GATE 1: RED tests (FAIL BY DESIGN at HEAD 3474cb5).
//!
//! `handle_proposal`'s hand-rolled `justify_qc` check (consensus_node.rs
//! 1389-1463) verifies each vote signature over the vote's OWN message, but,
//! unlike the canonical `verify_qc_well_formed` (consensus/src/lib.rs:809), it
//! never checks that each vote is bound to the QC it belongs to
//! (`vote.block_hash == qc.block_hash`). A Byzantine LEADER can therefore embed
//! a QC that *claims* to certify block X while carrying a quorum of genuine,
//! distinct, validly-signed votes that were actually cast for a DIFFERENT block
//! Y. The proposal path accepts it and installs it as `locked_qc`/`highest_qc`.
//! The identical QC is rejected on the gossip path (`handle_qc` ->
//! `verify_qc_well_formed`, consensus_node.rs:1890) and the sync path
//! (`handle_block_response`, :1014-1018). `encode_qc_v1` (codec.rs:187) does NOT
//! catch it; it only rejects duplicate voters / oversize.
//!
//! Both tests assert the CORRECT post-fix behavior, so they FAIL at HEAD
//! (documenting the hole) and will PASS once the height>1 branch is routed
//! through `verify_qc_well_formed`. They contain NO fix.
//!
//! Convention mirrors `crates/node/tests/sync_test.rs`
//! ::`test_qc_catchup_via_justify_qc_in_proposal`, the legitimate twin of this
//! scenario. The ONLY differences are (a) the QC's `block_hash` is a value the
//! validators never voted for, and (b) the proposed block's `parent_hash`
//! matches that forged hash so the proposal is otherwise fully valid and the
//! `justify_qc` binding is the sole accept/reject variable.

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus_types::{Block, Proposal, SignedProposal, Vote, QC};
use novai_crypto::{address_from_pubkey, sign_bytes};
use novai_node::consensus_node::ConsensusNode;
use novai_types::Address;
use std::collections::HashMap;

/// Builds a 4-validator node (node-under-test = validator 3, a follower) and a
/// height-2 `SignedProposal` from validator 1 (the leader for height 2) whose
/// `justify_qc` is FORGED: it claims to certify `forged_x` at height 1, but its
/// three distinct, validly-signed votes were cast for `real_y` (a different
/// block). Returns `(node, signed_proposal, forged_x, real_y)`.
fn build_forged_scenario() -> (ConsensusNode, SignedProposal, [u8; 32], [u8; 32]) {
    // Deterministic keys (seed = index), matching main.rs / sync_test.rs.
    let validator_keys: Vec<SigningKey> =
        (0u8..4).map(|i| SigningKey::from_bytes(&[i; 32])).collect();
    let validator_set: Vec<Address> = validator_keys
        .iter()
        .map(|sk| address_from_pubkey(&sk.verifying_key()))
        .collect();
    let validator_pubkeys: HashMap<Address, VerifyingKey> = validator_keys
        .iter()
        .map(|sk| {
            let pk = sk.verifying_key();
            (address_from_pubkey(&pk), pk)
        })
        .collect();

    // Node under test: validator 3 (follower; leader for height 2 is validator 1
    // since leader_idx = (height-1 + round) % n = (1 + 0) % 4 = 1).
    let node = ConsensusNode::new(
        validator_keys[3].clone(),
        validator_set.clone(),
        validator_pubkeys,
        1000,
    );

    // real_y = hash of the block the validators ACTUALLY voted for at height 1.
    let real_block1 = Block {
        height: 1,
        round: 0,
        parent_hash: [0u8; 32],
        state_root: [0u8; 32],
        txs: vec![],
    };
    let real_y = novai_consensus_types::block_hash(&real_block1);

    // forged_x = a block_hash the validators NEVER voted for (no such block
    // exists). This is what the attacker's QC falsely claims to certify.
    let forged_x = [0xEEu8; 32];
    assert_ne!(forged_x, real_y, "forged X must differ from the real block Y");

    // Three DISTINCT, validly-signed votes, each signed over real_y (height 1,
    // round 0), exactly as honest validators 0,1,2 would have signed for the
    // real block. The signatures are genuine; only the QC wrapper lies.
    let mut votes = Vec::new();
    for i in 0..3 {
        let unsigned = Vote {
            height: 1,
            round: 0,
            block_hash: real_y, // votes are bound to the REAL block Y
            voter: validator_set[i],
            signature: [0u8; 64],
            ai_signal_commitment: None,
        };
        let unsigned_bytes = novai_consensus_types::codec::encode_vote_v1_unsigned(&unsigned);
        let mut to_sign = Vec::new();
        to_sign.extend_from_slice(b"NOVAI_VOTE_V1");
        to_sign.extend_from_slice(&unsigned_bytes);
        let signature = sign_bytes(&validator_keys[i], &to_sign);
        votes.push(Vote { signature, ..unsigned });
    }

    // The FORGED QC: claims block_hash = forged_x, but carries votes for real_y.
    let forged_justify_qc = QC {
        height: 1,
        round: 0,
        block_hash: forged_x,
        votes,
    };

    // Height-2 proposal. parent_hash = forged_x so that, AFTER the forged QC
    // installs as highest_qc at HEAD, verify_block's parent check passes and the
    // proposal is otherwise fully valid, isolating the justify_qc binding as
    // the sole reason for accept/reject.
    let block2 = Block {
        height: 2,
        round: 0,
        parent_hash: forged_x,
        state_root: [0u8; 32],
        txs: vec![],
    };

    let proposal = Proposal {
        block: block2,
        justify_qc: forged_justify_qc,
    };

    // Sign the proposal with validator 1's key (the leader for height 2).
    // Proposal signatures use NO domain tag (verified at consensus_node.rs:1377).
    let unsigned_bytes = novai_consensus_types::codec::encode_proposal_v1_unsigned(&proposal)
        .expect("encode proposal");
    let signature = sign_bytes(&validator_keys[1], &unsigned_bytes);
    let signed_proposal = SignedProposal {
        proposer: validator_set[1],
        proposal,
        signature,
    };

    (node, signed_proposal, forged_x, real_y)
}

/// TEST 1 (verifier-level). Drives the forged justify_qc through the REAL
/// `handle_proposal` path and asserts the proposal is REJECTED (correct,
/// post-fix). At HEAD the hand-rolled check accepts it, so `handle_proposal`
/// returns `Ok` and this assertion FAILS, proving the hole.
#[test]
fn forged_justify_qc_with_votes_for_different_block_rejected() {
    let (node, signed_proposal, forged_x, real_y) = build_forged_scenario();

    let result = node.handle_proposal(signed_proposal);

    assert!(
        result.is_err(),
        "F2 HOLE: handle_proposal ACCEPTED a forged justify_qc whose votes were \
         cast for a DIFFERENT block (votes bound to Y={:?}, QC claims X={:?}). \
         The canonical verify_qc_well_formed rejects this (consensus/src/lib.rs:809) \
         and handle_qc/sync reject it, but the proposal path does not. \
         Expected Err (post-fix), got {result:?}.",
        &real_y[..4],
        &forged_x[..4],
    );
}

/// TEST 2 (state-level). Drives the same forged justify_qc through
/// `handle_proposal` and asserts the safety anchor `locked_qc` was NOT bound to
/// the uncertified block X (correct, post-fix). At HEAD the forged QC installs,
/// so `locked_qc.block_hash == X` and this assertion FAILS, proving the
/// consequence. Asserts on STATE, independent of the call's return value.
#[test]
fn forged_justify_qc_must_not_corrupt_locked_qc() {
    let (node, signed_proposal, forged_x, real_y) = build_forged_scenario();

    let _ = node.handle_proposal(signed_proposal);

    let locked_hash = node
        .state
        .lock()
        .unwrap()
        .locked_qc
        .as_ref()
        .map(|qc| qc.block_hash);

    assert!(
        locked_hash != Some(forged_x),
        "F2 CONSEQUENCE: a forged justify_qc (votes cast for Y={:?}, QC claims X={:?}) \
         corrupted locked_qc, the safety anchor, to the uncertified block X. \
         Post-fix the proposal must be rejected before install so locked_qc never \
         binds to X. Got locked_qc.block_hash = {locked_hash:?}",
        &real_y[..4],
        &forged_x[..4],
    );
}
