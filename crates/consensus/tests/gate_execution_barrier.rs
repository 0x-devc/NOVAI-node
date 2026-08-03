//! Gate wedge-276272: the execution barrier as a first-class, pinned invariant.
//!
//! The fix adds a second, tighter bound on voting alongside the commit window: a
//! node cannot vote (or propose) a block whose parent it has not executed, because
//! verify_block now compares the header against the POST-execution root computed
//! over the resolved parent state, and an unresolvable parent is refused. This is
//! the fix's defining liveness property. It is pinned here so a future refactor
//! cannot silently weaken "do not vote what you have not executed", and so the
//! one-laggard fleet tolerance (the correct stall-over-fork trade at n=4/quorum-3)
//! is a guarded invariant rather than an emergent one.

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus::ConsensusState;
use novai_consensus_types::codec::encode_vote_v1_unsigned;
use novai_consensus_types::{block_hash, Block, Vote, QC};
use novai_crypto::{address_from_pubkey, sign_bytes};
use novai_execution::empty_smt_root;
use novai_state::MemKv;
use novai_types::Address;

/// A domain-separated, validly signed vote.
fn signed_vote(sk: &SigningKey, voter: Address, height: u64, bh: [u8; 32]) -> Vote {
    let unsigned = Vote {
        height,
        round: 0,
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

#[test]
fn barrier_refuses_vote_when_parent_unexecuted() {
    let sk = SigningKey::from_bytes(&[1u8; 32]);
    let addr = address_from_pubkey(&sk.verifying_key());
    let mut state = ConsensusState::new(addr);
    let db = MemKv::new();

    // The frontier QC ties a height-2 candidate to a height-1 parent, but the
    // parent's BODY is never cached, so post-state(1) is unresolvable.
    let parent1 = Block {
        height: 1,
        round: 0,
        parent_hash: [0u8; 32],
        state_root: empty_smt_root(),
        txs: vec![],
    };
    let p1hash = block_hash(&parent1);
    state.highest_qc = Some(QC {
        height: 1,
        round: 0,
        block_hash: p1hash,
        votes: vec![],
    });
    state.locked_qc = state.highest_qc.clone();

    let candidate = Block {
        height: 2,
        round: 0,
        parent_hash: p1hash,
        state_root: empty_smt_root(),
        txs: vec![],
    };

    // The height-2 block is well within the commit window (committed 0), so the
    // refusal is the EXECUTION BARRIER (unresolvable parent), not the window.
    let err = format!("{:?}", state.verify_block(&candidate, &db));
    assert!(
        !err.contains("commit window") && err.to_lowercase().contains("unresolvable"),
        "a node must refuse to vote a block whose parent it has not executed: {err}"
    );

    // Once the parent body is available (executed and cached), the same vote is
    // admitted: the barrier gates unexecuted parents only, never wedges a node that
    // has kept up.
    state.cache_block(parent1).unwrap();
    state
        .verify_block(&candidate, &db)
        .expect("with the parent executed and resolvable, the vote is admitted");
}

#[test]
fn quorum_tolerates_one_laggard_not_two() {
    let validators: Vec<(Address, SigningKey)> = (0..4u8)
        .map(|i| {
            let sk = SigningKey::from_bytes(&[i; 32]);
            (address_from_pubkey(&sk.verifying_key()), sk)
        })
        .collect();
    let pubkeys: Vec<(Address, VerifyingKey)> = validators
        .iter()
        .map(|(a, sk)| (*a, sk.verifying_key()))
        .collect();
    let quorum = 2 * ((4 - 1) / 3) + 1;
    assert_eq!(quorum, 3, "n=4 => quorum 3");

    let bh = [0x42u8; 32];
    let vote = |i: usize| signed_vote(&validators[i].1, validators[i].0, 1, bh);

    // Three of four vote (one node barred by the execution barrier): certifies. The
    // fleet tolerates exactly one laggard.
    let qc3 = QC {
        height: 1,
        round: 0,
        block_hash: bh,
        votes: vec![vote(0), vote(1), vote(2)],
    };
    ConsensusState::verify_qc_well_formed(&qc3, &pubkeys, quorum)
        .expect("three of four certify: the fleet tolerates one execution-barrier laggard");

    // Two of four vote (two laggards barred): below quorum, no certificate. The
    // fleet STALLS until one catches up, which is the correct fail-safe direction
    // (stall, never fork) the fix deliberately chooses over voting unexecuted state.
    let qc2 = QC {
        height: 1,
        round: 0,
        block_hash: bh,
        votes: vec![vote(0), vote(1)],
    };
    assert!(
        ConsensusState::verify_qc_well_formed(&qc2, &pubkeys, quorum).is_err(),
        "two of four cannot certify: two simultaneous laggards stall certification (never fork)"
    );
}
