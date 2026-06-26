//! Gate 9: the follower self-vote site (handle_proposal) durably persists the
//! vote high-water mark when it votes, and refuses a re-vote after a restart.
//!
//! This exercises the node-level FOLLOWER path, not only the engine add_vote
//! path. The follower is the primary equivocation exposure because it broadcasts
//! its individual vote. The synced persist happens inside the locked block in
//! handle_proposal, before the lock is dropped and the vote is broadcast, so the
//! durable write strictly precedes the network send by program order.
//!
//! Ordering note: with no peers wired in a unit test the broadcast is a no-op,
//! so I cannot capture the network send to assert the write-then-send order
//! directly. I prove the load-bearing consequence instead: after handle_proposal
//! returns, voted_view is on disk (the persist ran), and a recover from that db
//! refuses the re-vote. The strict ordering is guaranteed by code structure
//! (persist inside the locked block, broadcast after the lock drop).

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus::ConsensusState;
use novai_consensus_types::codec::{encode_proposal_v1_unsigned, encode_voted_view_v1};
use novai_consensus_types::{Block, Proposal, SignedProposal, QC};
use novai_crypto::{address_from_pubkey, sign_bytes};
use novai_node::consensus_node::ConsensusNode;
use novai_state::{Kv, KEY_VOTED_VIEW};
use novai_types::Address;
use std::collections::HashMap;

fn make_validators(count: usize) -> Vec<(Address, SigningKey, VerifyingKey)> {
    (0..count)
        .map(|i| {
            let sk = SigningKey::from_bytes(&[i as u8; 32]);
            let pk = sk.verifying_key();
            (address_from_pubkey(&pk), sk, pk)
        })
        .collect()
}

#[test]
fn follower_persists_voted_view_when_voting_and_refuses_after_restart() {
    let validators = make_validators(4);
    let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
    let mut validator_pubkeys = HashMap::new();
    for (a, _, pk) in &validators {
        validator_pubkeys.insert(*a, *pk);
    }

    // The node under test is validator[1], a follower. The leader for (1, 0) is
    // validator_set[(0 + 0) % 4] = validator[0], so the proposal comes from it.
    let node = ConsensusNode::new(
        validators[1].1.clone(),
        validator_set.clone(),
        validator_pubkeys,
        1000,
    );

    let block1 = Block {
        height: 1,
        round: 0,
        parent_hash: [0u8; 32],
        // F3: the empty-DB genesis root is now the canonical empty SMT root, so
        // the leader's genesis proposal must carry it to pass verify_block.
        state_root: novai_execution::empty_smt_root(),
        txs: vec![],
    };
    let genesis_qc = QC {
        height: 0,
        round: 0,
        block_hash: [0u8; 32],
        votes: vec![],
    };
    let proposal = Proposal {
        block: block1,
        justify_qc: genesis_qc,
    };
    let unsigned = encode_proposal_v1_unsigned(&proposal).expect("encode proposal");
    let signature = sign_bytes(&validators[0].1, &unsigned);
    let signed = SignedProposal {
        proposer: validator_set[0],
        proposal,
        signature,
    };

    node.handle_proposal(signed)
        .expect("follower must vote on the leader's valid genesis proposal");

    // The follower durably persisted voted_view as part of voting (the
    // persist-before-broadcast write).
    {
        let db = node.db.lock().unwrap();
        assert_eq!(
            db.get(KEY_VOTED_VIEW).unwrap(),
            Some(encode_voted_view_v1(1, 0)),
            "the follower must durably persist voted_view when it votes"
        );
    }
    {
        let state = node.state.lock().unwrap();
        assert_eq!(state.voted_view, Some((1, 0)));
    }

    // Durable across a restart: recovering from the same db refuses a re-vote at
    // (1, 0) but still admits (1, 1), the legitimate higher-round re-proposal.
    {
        let db = node.db.lock().unwrap();
        let recovered = ConsensusState::recover(validator_set[1], &*db).unwrap();
        assert_eq!(recovered.voted_view, Some((1, 0)));
        assert!(
            !recovered.may_vote(1, 0),
            "the restarted follower must refuse a re-vote at the already-voted view"
        );
        assert!(
            recovered.may_vote(1, 1),
            "the restarted follower must still vote at the higher-round re-proposal"
        );
    }
}
