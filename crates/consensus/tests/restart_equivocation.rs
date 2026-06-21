//! Gate 9 (persistent vote tracking): restart-race equivocation regression.
//!
//! RED test, added in Phase 1, fails on current HEAD for the right reason: a
//! node votes at view (1, 0), restarts (state recovered from the same db), and
//! then votes again at the SAME view (1, 0) for a CONFLICTING block, because no
//! durable record survives the restart to stop the re-vote. Today the in-memory
//! guard `voted_in_round` is rebuilt empty by `ConsensusState::recover`, so the
//! second self-vote is wrongly permitted: that is the equivocation.
//!
//! This drives `add_vote`, the same engine method the leader self-vote uses
//! (`crates/node/src/consensus_node.rs:1306`). Phase 2 adds a durable
//! `voted_view` high-water mark that `add_vote` consults for this node's own
//! votes, that rides the existing `persist_highest_qc` co-persist, and that
//! `recover` seeds from disk; with those in place this exact test body flips to
//! passing, exactly as the locked-QC regression flips through changes to
//! existing methods. No durable-vote API is referenced here, so the test
//! compiles and runs on current HEAD.

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus::ConsensusState;
use novai_consensus_types::{Block, QC};
use novai_crypto::address_from_pubkey;
use novai_state::MemKv;
use novai_types::Address;

/// Deterministic validators, matching the convention in `recovery.rs`.
fn make_test_validators(count: usize) -> Vec<(Address, SigningKey, VerifyingKey)> {
    (0..count)
        .map(|i| {
            let seed = [i as u8; 32];
            let sk = SigningKey::from_bytes(&seed);
            let pk = sk.verifying_key();
            let addr = address_from_pubkey(&pk);
            (addr, sk, pk)
        })
        .collect()
}

/// Restart-race equivocation: vote at (1, 0), restart, vote again at (1, 0) for
/// a conflicting block. The recovered node MUST refuse the second vote. It does
/// not today (no durable record), so this assertion fails on current HEAD. RED.
#[test]
fn restart_revote_at_same_view_is_refused() {
    let validators = make_test_validators(4);
    let pubkeys: Vec<(Address, VerifyingKey)> =
        validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
    let (v0_addr, v0_key, _) = &validators[0];

    let mut db = MemKv::new();

    // Two conflicting blocks at the SAME view (height 1, round 0), distinct
    // hashes. In production a Byzantine leader's two proposals differ by
    // transaction content; the vote-dedup layer under test only sees distinct
    // block hashes, so distinct parent_hash is enough here.
    let block_a = Block {
        height: 1,
        round: 0,
        parent_hash: [0u8; 32],
        state_root: [0u8; 32],
        txs: vec![],
    };
    let block_b = Block {
        height: 1,
        round: 0,
        parent_hash: [0x11u8; 32],
        state_root: [0u8; 32],
        txs: vec![],
    };
    assert_ne!(
        novai_consensus_types::codec::hash_block_v1(&block_a).unwrap(),
        novai_consensus_types::codec::hash_block_v1(&block_b).unwrap(),
        "the two conflicting blocks must hash differently for this test to be meaningful"
    );

    // This node votes for block A at (1, 0), the real way, then persists every
    // piece of state the engine persists today. None of it records the vote.
    let mut state = ConsensusState::new(*v0_addr);
    let vote_a = state.create_vote(&block_a, v0_key).unwrap();
    state
        .add_vote(vote_a, &pubkeys)
        .expect("first self-vote at (1, 0) must be accepted");
    state.persist_committed_height(&mut db).unwrap();
    state.persist_highest_qc(&mut db).unwrap();

    // RESTART: reconstruct the engine from the same db. `recover` rebuilds
    // `voted_in_round` empty, so the only in-memory trace of the vote is gone.
    let mut recovered = ConsensusState::recover(*v0_addr, &db).unwrap();

    // This node now votes for the CONFLICTING block B at the SAME view (1, 0).
    let vote_b = recovered.create_vote(&block_b, v0_key).unwrap();
    let second = recovered.add_vote(vote_b, &pubkeys);

    // SAFETY: a node that already voted at (1, 0) must never vote again at
    // (1, 0) for a different block. Today nothing durable stops it, so the
    // recovered node accepts the second vote. This assertion fails on current
    // HEAD for exactly that reason: the restart-race equivocation is open.
    assert!(
        second.is_err(),
        "RESTART-RACE EQUIVOCATION: the recovered node re-voted at (1, 0) for a \
         conflicting block; no durable vote record survived the restart to stop it. \
         Got Ok, expected a refusal."
    );
}

/// The height-only-halt trap: a guard keyed by height alone would reject the
/// legitimate (H, R+1) re-proposal after a view change, the misfire that halted
/// the chain when `voted_at_height` existed. The (height, round) key must admit
/// it. After voting at (1, 0) and restarting, the node refuses a re-vote at
/// (1, 0) but still votes at (1, 1).
#[test]
fn higher_round_reproposal_after_restart_still_votes() {
    let validators = make_test_validators(4);
    let pubkeys: Vec<(Address, VerifyingKey)> =
        validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
    let (v0_addr, v0_key, _) = &validators[0];
    let mut db = MemKv::new();

    let block_r0 = Block {
        height: 1,
        round: 0,
        parent_hash: [0u8; 32],
        state_root: [0u8; 32],
        txs: vec![],
    };
    let block_r1 = Block {
        height: 1,
        round: 1,
        parent_hash: [0u8; 32],
        state_root: [0u8; 32],
        txs: vec![],
    };

    let mut state = ConsensusState::new(*v0_addr);
    let vote_r0 = state.create_vote(&block_r0, v0_key).unwrap();
    state.add_vote(vote_r0, &pubkeys).unwrap();
    state.persist_highest_qc(&mut db).unwrap();

    let mut recovered = ConsensusState::recover(*v0_addr, &db).unwrap();
    assert!(
        !recovered.may_vote(1, 0),
        "must refuse a re-vote at the already-voted view (1, 0)"
    );
    assert!(
        recovered.may_vote(1, 1),
        "must admit the higher-round re-proposal at (1, 1) after a view change"
    );

    // End to end: voting for the round-1 re-proposal is accepted, not wedged.
    let vote_r1 = recovered.create_vote(&block_r1, v0_key).unwrap();
    let r = recovered.add_vote(vote_r1, &pubkeys);
    assert!(
        r.is_ok(),
        "post-view-change re-proposal at (1, 1) must be accepted, got {r:?}"
    );
}

/// Genesis / first boot: an absent record loads as None (never voted), and the
/// first vote is allowed.
#[test]
fn genesis_first_vote_allowed_and_absent_record_is_none() {
    let validators = make_test_validators(4);
    let v0 = validators[0].0;
    let db = MemKv::new();

    assert_eq!(
        ConsensusState::load_voted_view(&db).unwrap(),
        None,
        "an absent record must load as None, not error"
    );
    let state = ConsensusState::new(v0);
    assert_eq!(state.voted_view, None);
    assert!(state.may_vote(1, 0), "the first vote at genesis must be allowed");
}

/// The durable mark round-trips through persist and recover, and the recovered
/// predicate refuses every view at or below it while admitting higher ones.
#[test]
fn voted_view_round_trips_through_persist_and_recover() {
    let validators = make_test_validators(4);
    let pubkeys: Vec<(Address, VerifyingKey)> =
        validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
    let (v0, v0_key, _) = &validators[0];
    let mut db = MemKv::new();

    // Vote at (3, 2); set the cursor so add_vote's expected height is 3.
    let mut state = ConsensusState::new(*v0);
    state.committed_height = 2;
    state.height = 2;
    let block = Block {
        height: 3,
        round: 2,
        parent_hash: [0u8; 32],
        state_root: [0u8; 32],
        txs: vec![],
    };
    let vote = state.create_vote(&block, v0_key).unwrap();
    state.add_vote(vote, &pubkeys).unwrap();
    assert_eq!(state.voted_view, Some((3, 2)));

    state.persist_committed_height(&mut db).unwrap();
    state.persist_highest_qc(&mut db).unwrap(); // co-persists voted_view

    let recovered = ConsensusState::recover(*v0, &db).unwrap();
    assert_eq!(
        recovered.voted_view,
        Some((3, 2)),
        "voted_view must survive recovery"
    );
    assert!(!recovered.may_vote(3, 2), "the exact voted view is refused");
    assert!(!recovered.may_vote(3, 1), "a lower round at the same height is refused");
    assert!(!recovered.may_vote(2, 9), "a lower height is refused regardless of round");
    assert!(recovered.may_vote(3, 3), "a higher round at the same height is admitted");
    assert!(recovered.may_vote(4, 0), "a higher height is admitted");
}

/// The restart-race (two votes at one view) is caught by voted_view, NOT by
/// locked_qc: at the first vote the lock is a height behind, so safe_to_extend
/// would admit a conflicting same-height block. Prove voted_view catches what
/// locked_qc misses, and that the two guards are independent.
#[test]
fn composes_with_locked_qc_without_overlap() {
    let validators = make_test_validators(4);
    let pubkeys: Vec<(Address, VerifyingKey)> =
        validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
    let (v0, v0_key, _) = &validators[0];
    let mut db = MemKv::new();

    let block = Block {
        height: 1,
        round: 0,
        parent_hash: [0u8; 32],
        state_root: [0u8; 32],
        txs: vec![],
    };
    let mut state = ConsensusState::new(*v0);
    let vote = state.create_vote(&block, v0_key).unwrap();
    state.add_vote(vote, &pubkeys).unwrap();
    state.persist_highest_qc(&mut db).unwrap();

    let recovered = ConsensusState::recover(*v0, &db).unwrap();
    assert!(
        recovered.locked_qc.is_none(),
        "no locked QC exists at the first vote height, so locked_qc cannot catch this"
    );
    assert!(
        !recovered.may_vote(1, 0),
        "voted_view catches the restart-race that locked_qc misses"
    );
    assert!(
        recovered.may_vote(2, 0),
        "the guards are independent: a strictly higher view still passes voted_view"
    );
}

/// Liveness: the guard never refuses a strictly higher view, so a node can
/// always make progress and never deadlocks itself. Walk a sequence of forward
/// views; each is admitted, then refused once voted, and a higher one always
/// remains available.
#[test]
fn never_deadlocks_monotonic_progress() {
    let validators = make_test_validators(4);
    let v0 = validators[0].0;
    let mut state = ConsensusState::new(v0);

    let sequence = [(1u64, 0u64), (1, 1), (1, 2), (2, 0), (2, 1), (3, 0), (10, 5)];
    for (h, r) in sequence {
        assert!(
            state.may_vote(h, r),
            "legitimate forward view ({h}, {r}) must be admitted"
        );
        state.note_self_vote(h, r).unwrap();
        assert!(
            !state.may_vote(h, r),
            "the just-voted view ({h}, {r}) is refused, no double vote"
        );
    }
    assert!(
        state.may_vote(11, 0),
        "progress is always possible at a strictly higher view"
    );
}

/// No-clear invariant (mirrors the locked-QC `lock_survives_*` tests): a stray
/// reset of voted_view in a round / commit / view path would silently defeat
/// the durable guard and otherwise pass the suite. Drive a strict-height-advance
/// view change and prove voted_view is NOT cleared.
#[test]
fn voted_view_survives_view_change_reset() {
    let validators = make_test_validators(4);
    let db = MemKv::new();
    let mut state = ConsensusState::new(validators[0].0);

    // Pretend this node voted at (1, 0), and that round-scoped state is dirty.
    state.voted_view = Some((1, 0));
    state.voted_in_round.insert(validators[0].0);
    state.round = 5;

    // Adopt QC at height 1 then height 2; the height-2 adoption is a strict
    // height advance, which fires the view-change reset that clears
    // voted_in_round and round. Empty-votes QCs, the commit-path convention.
    let hash = |b: &Block| novai_consensus_types::codec::hash_block_v1(b).unwrap();
    let mk = |height: u64, round: u64, parent: [u8; 32]| Block {
        height,
        round,
        parent_hash: parent,
        state_root: [0u8; 32],
        txs: vec![],
    };
    let b1 = mk(1, 0, [0u8; 32]);
    let h_b1 = hash(&b1);
    let b2 = mk(2, 0, h_b1);
    let h_b2 = hash(&b2);
    let qc1 = QC { height: 1, round: 0, block_hash: h_b1, votes: vec![] };
    let qc2 = QC { height: 2, round: 0, block_hash: h_b2, votes: vec![] };

    state.cache_qc_and_check_commit(qc1, &db).unwrap();
    state.cache_qc_and_check_commit(qc2, &db).unwrap();

    assert_eq!(state.round, 0, "the view-change reset must have fired (round cleared)");
    assert!(
        state.voted_in_round.is_empty(),
        "voted_in_round must be cleared by the reset (proves the reset ran)"
    );
    assert_eq!(
        state.voted_view,
        Some((1, 0)),
        "voted_view must SURVIVE the view-change reset; a stray clear here is the regression"
    );
    assert!(!state.may_vote(1, 0), "voted_view still guards after the reset");
    assert!(state.may_vote(2, 1), "and still admits a strictly higher view");
}
