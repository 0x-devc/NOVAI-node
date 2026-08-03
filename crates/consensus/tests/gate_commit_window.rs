//! Commit-window rule (WEDGE-20260718): the consensus frontier must never
//! climb more than COMMIT_WINDOW heights above the committed height.
//!
//! In the 20260718 incident a host resource event froze commits while
//! consensus kept certifying new heights for five days, ending 818,258
//! heights ahead of the committed floor with durably poisoned consensus
//! cursors. Nothing bounded the climb. The rule under test here refuses to
//! propose or vote for any block whose height exceeds committed + window,
//! so a future commit stall parks the fleet a bounded distance above the
//! floor, inside the retention and sync windows, restart recoverable.
//!
//! I test the engine level here: the verify gate, the propose gate, the
//! note_self_vote backstop, exact boundary behavior, the healthy pipeline
//! (the rule must never engage in normal operation), the full stall park
//! and resume story, the wedge-shaped restart, and the legitimately behind
//! node whose window slides as sync commits advance. The node-level entry
//! points are covered in crates/node/tests/gate_commit_window_node.rs.

use ed25519_dalek::{SigningKey, VerifyingKey};
use novai_consensus::ConsensusState;
use novai_consensus_types::codec::{encode_qc_v1, encode_voted_view_v1, hash_block_v1};
use novai_consensus_types::{Block, Vote, QC};
use novai_crypto::address_from_pubkey;
use novai_state::{Kv, MemKv, KEY_COMMITTED_HEIGHT, KEY_HIGHEST_QC, KEY_LOCKED_QC, KEY_VOTED_VIEW};
use novai_types::Address;

/// The spec value from the incident diagnosis, pinned locally so these tests
/// compile and fail on BEHAVIOR against a tree that predates the rule. A
/// separate test asserts the exported constant matches this value.
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

fn make_block(height: u64, parent_hash: [u8; 32]) -> Block {
    Block {
        height,
        round: 0,
        parent_hash,
        state_root: novai_execution::empty_smt_root(),
        txs: vec![],
    }
}

/// A QC certifying `block` at `height`, without signatures. The engine paths
/// under test here never verify QC signatures (that is the node layer's
/// verify_qc_well_formed); what matters is the height and hash binding.
fn make_qc(height: u64, block_hash: [u8; 32]) -> QC {
    QC {
        height,
        round: 0,
        block_hash,
        votes: vec![],
    }
}

/// An unsigned vote literal for add_vote_verified, whose contract is that the
/// caller already verified the signature.
fn make_vote(height: u64, block_hash: [u8; 32], voter: Address) -> Vote {
    Vote {
        height,
        round: 0,
        block_hash,
        voter,
        signature: [0u8; 64],
        ai_signal_commitment: None,
    }
}

/// A state parked with its committed height at 0 and its highest QC at
/// `qc_height`, certifying a block whose hash the caller gets back for
/// building the next proposal.
fn state_with_frontier(our: Address, qc_height: u64) -> (ConsensusState, [u8; 32]) {
    let parent = make_block(qc_height, [0xAA; 32]);
    let parent_hash = hash_block_v1(&parent).expect("hash parent");
    let qc = make_qc(qc_height, parent_hash);
    let mut state = ConsensusState::new(our);
    state.highest_qc = Some(qc.clone());
    state.locked_qc = Some(qc);
    (state, parent_hash)
}

// ---------------------------------------------------------------------------
// The constant itself
// ---------------------------------------------------------------------------

/// The park-inside-retention invariant the recovery story depends on: a
/// frontier parked at committed + COMMIT_WINDOW must sit deep inside every
/// peer's PRUNE_RETAIN_BLOCKS retention window, so a parked or restarted
/// node can always block-range sync back to health. Pinned at compile
/// time: if a future change flips this relation, this test crate stops
/// building.
const _: () = assert!(novai_consensus::COMMIT_WINDOW < novai_consensus::PRUNE_RETAIN_BLOCKS);

#[test]
fn commit_window_constant_matches_spec() {
    assert_eq!(
        novai_consensus::COMMIT_WINDOW,
        W,
        "the exported window must match the incident-diagnosis spec value \
         these tests pin independently"
    );
}

// ---------------------------------------------------------------------------
// The refusal gates, one entry point at a time
// ---------------------------------------------------------------------------

#[test]
fn window_refuses_vote_above_bound() {
    let validators = make_validators(4);
    let (state, parent_hash) = state_with_frontier(validators[0].0, W);
    let candidate = make_block(W + 1, parent_hash);
    let db = MemKv::new();

    let res = state.verify_block(&candidate, &db);
    assert!(
        res.is_err(),
        "verify_block accepted a block at height {} with committed height 0; \
         the commit window rule must refuse to vote more than {W} heights \
         above committed",
        W + 1
    );
    let msg = format!("{res:?}");
    assert!(
        msg.contains("commit window"),
        "the refusal must name the commit window rule, got: {msg}"
    );
}

#[test]
fn window_admits_vote_at_exactly_the_bound() {
    // Reconceived (gate wedge-276272): pin the commit-window PREDICATE, which is the
    // window rule proper. It is inclusive at committed + W and strict beyond it, so
    // the window never costs a height of legitimate progress. Post-fix, actually
    // casting a vote at the bound ALSO requires the node to have EXECUTED the
    // pipeline up to the parent (the execution barrier, the second and tighter
    // bound, pinned in gate_execution_barrier.rs). Here we pin that the window
    // itself never refuses a block at exactly the bound; a synthetic far-ahead vote
    // through verify_block is now the barrier's domain, not the window's.
    let validators = make_validators(4);
    let (state, _parent_hash) = state_with_frontier(validators[0].0, W - 1);
    assert!(
        state.within_commit_window(W),
        "the window admits at exactly committed + W (committed 0)"
    );
    assert!(
        !state.within_commit_window(W + 1),
        "the window refuses strictly above committed + W"
    );
    assert!(
        state.within_commit_window(1) && state.within_commit_window(W - 1),
        "and admits everything at or below the bound"
    );
}

#[test]
fn window_refuses_propose_above_bound() {
    let validators = make_validators(4);
    let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
    // Leader for the view proposing height W + 1 is index (W + 0) % 4 = 0.
    let (mut state, _parent_hash) = state_with_frontier(validators[0].0, W);
    let mut pool = mempool::TxMempool::new(1, 100);
    let db = MemKv::new();

    let res = state.propose_block(&mut pool, &TestNonceProvider, &db, &validator_set);
    assert!(
        res.is_err(),
        "propose_block built a block at height {} with committed height 0; \
         the commit window rule must refuse to propose more than {W} heights \
         above committed",
        W + 1
    );
    let msg = format!("{res:?}");
    assert!(
        msg.contains("CommitWindow"),
        "the propose refusal must be the commit window error, got: {msg}"
    );
    assert_eq!(
        state.last_proposed, None,
        "a window-refused proposal must leave no proposal side effects"
    );
}

#[test]
fn window_backstop_refuses_own_vote_above_bound() {
    let validators = make_validators(4);
    let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
    let pubkeys: Vec<(Address, VerifyingKey)> =
        validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();
    let our = validator_set[0];
    let (mut state, parent_hash) = state_with_frontier(our, W);

    // Expected vote height is max(height, hqc) + 1 = W + 1, above the bound.
    // note_self_vote is the single chokepoint every self vote passes through
    // (add_vote, add_vote_verified, and the node follower path), so refusing
    // here makes "no own vote above committed + window" an engine invariant
    // no caller can bypass.
    let own_vote = make_vote(W + 1, parent_hash, our);
    let res = state.add_vote_verified(own_vote, &pubkeys);
    assert!(
        res.is_err(),
        "the engine recorded this node's own vote at height {} with \
         committed height 0; the commit window backstop must refuse it",
        W + 1
    );
    assert_eq!(
        state.voted_view, None,
        "a window-refused vote must not advance the durable vote mark"
    );

    // A peer's vote at the same height is NOT gated: peer admission control
    // cannot stop a remote quorum and the fleet property comes from every
    // correct node refusing to CAST votes, not from refusing to count them.
    let peer_vote = make_vote(W + 1, parent_hash, validator_set[1]);
    state
        .add_vote_verified(peer_vote, &pubkeys)
        .expect("peer votes stay ungated; only own vote casting is bounded");
}

// ---------------------------------------------------------------------------
// Healthy operation: the rule must never engage
// ---------------------------------------------------------------------------

#[test]
fn window_never_engages_in_healthy_pipeline() {
    let validators = make_validators(4);
    let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();
    let pubkeys: Vec<(Address, VerifyingKey)> =
        validators.iter().map(|(a, _, vk)| (*a, *vk)).collect();

    let mut states: Vec<ConsensusState> = validator_set
        .iter()
        .map(|a| ConsensusState::new(*a))
        .collect();
    let mut pools: Vec<mempool::TxMempool> =
        (0..4).map(|_| mempool::TxMempool::new(1, 100)).collect();
    let mut db = MemKv::new();

    // Forty heights of the real pipeline: propose, verify, vote, form the QC,
    // walk the commit rule, apply commits. The frontier stays two to three
    // heights above committed the whole way, nowhere near the window.
    for h in 1..=40u64 {
        let mut proposed = None;
        for i in 0..4 {
            match states[i].propose_block(&mut pools[i], &TestNonceProvider, &db, &validator_set) {
                Ok(b) => {
                    assert!(proposed.is_none(), "two leaders proposed at height {h}");
                    proposed = Some(b);
                }
                Err(e) => {
                    let msg = format!("{e:?}");
                    assert!(
                        msg.contains("NotLeader"),
                        "healthy propose at height {h} refused for a reason \
                         other than leadership: {msg}"
                    );
                }
            }
        }
        let block = proposed.expect("exactly one leader per height");
        assert_eq!(block.height, h);
        let block_hash = hash_block_v1(&block).expect("hash");

        // gate wedge-276272: persist the block so resolve_parent_state can load the
        // committed tip from the DB (propose and verify now resolve the parent
        // post-state). Empty blocks keep the convention-invariant empty root, so
        // this changes no behavior the window test asserts.
        db.put(
            &novai_state::block_key(block.height),
            &novai_consensus_types::codec::encode_block_v1(&block).expect("encode block"),
        )
        .expect("persist committed-chain block");

        for state in states.iter_mut() {
            state
                .verify_block(&block, &db)
                .unwrap_or_else(|e| panic!("healthy verify refused at height {h}: {e:?}"));
            state.cache_block(block.clone()).expect("cache");
        }
        for &voter in &validator_set {
            let vote = make_vote(h, block_hash, voter);
            for state in states.iter_mut() {
                state
                    .add_vote_verified(vote.clone(), &pubkeys)
                    .unwrap_or_else(|e| panic!("healthy vote refused at height {h}: {e:?}"));
            }
        }
        for state in states.iter_mut() {
            let qc = state
                .try_form_qc(&block_hash, &validator_set)
                .expect("form qc")
                .expect("quorum reached");
            let chain = state
                .cache_qc_and_check_commit(qc, &db)
                .expect("commit walk");
            if !chain.is_empty() {
                state.apply_commits(&chain).expect("apply commits");
            }
            let gap = state.highest_qc.as_ref().map_or(0, |q| q.height) - state.committed_height;
            assert!(
                gap <= 3,
                "healthy pipeline gap reached {gap} at height {h}; the commit \
                 window premise (normal depth 2 to 3) is broken"
            );
        }
    }
    for state in &states {
        assert_eq!(state.committed_height, 38, "3-chain commit lag of 2");
    }
}

// ---------------------------------------------------------------------------
// The incident shape: a commit stall parks the frontier, then resumes
// ---------------------------------------------------------------------------

#[test]
fn window_parks_stalled_frontier_and_resumes_after_commits() {
    // Reconceived (gate wedge-276272). The eliminated stall shape was "the fleet
    // votes on UNRESOLVABLE bodies while the frontier climbs" (the exact 818,258
    // vote-mark poisoning of the incident); the execution barrier now makes voting
    // an unresolvable body impossible by construction. The SURVIVING stall is:
    // commits are frozen (a persist-path freeze) while a node keeps executing and
    // voting on RESOLVABLE bodies, so its durable vote mark climbs. The window
    // BOUNDS that executed-but-uncommitted climb at committed + W and no further,
    // keeping the mark a bounded, restart-recoverable distance above the floor, and
    // the same window admits the resume once commits advance. Pinned via
    // note_self_vote, the engine backstop through which every self vote passes. This
    // is STRONGER than the original: the parked marks are now guaranteed executed
    // (hence recoverable), which is exactly the property the rule protects.
    let mut state = ConsensusState::new(make_validators(4)[0].0);
    let mut db = MemKv::new();

    // The stall: commits frozen at 0, the durable vote mark climbs to exactly the
    // window bound as the node keeps voting resolvable heights.
    for h in 1..=W {
        state.note_self_vote(h, 0).unwrap_or_else(|e| {
            panic!("a self vote at height {h} INSIDE the window must be admitted: {e:?}")
        });
        assert_eq!(state.committed_height, 0, "commits stay frozen through the climb");
    }
    assert_eq!(
        state.voted_view,
        Some((W, 0)),
        "the durable vote mark parks at exactly committed + W"
    );

    // One height past the bound: refused by the window backstop, mark unchanged.
    let refused = format!("{:?}", state.note_self_vote(W + 1, 0));
    assert!(
        refused.contains("CommitWindow"),
        "note_self_vote must refuse a self vote above committed + W: {refused}"
    );
    assert_eq!(
        state.voted_view,
        Some((W, 0)),
        "a window-refused self vote must not advance the durable mark"
    );

    // The parked mark is bounded and restart-recoverable: it persists and reloads
    // without surgery (far inside the retention window).
    state.persist_voted_view(&mut db).expect("persist parked vote mark");
    assert_eq!(
        db.get(KEY_VOTED_VIEW).unwrap(),
        Some(encode_voted_view_v1(W, 0)),
        "the parked vote mark is durable, bounded at committed + W"
    );

    // Resume: as sync commits advance the floor, the window slides and voting
    // resumes at the new frontier. No surgery, no restart, no deadlock with gate 9.
    state.committed_height = W;
    state.note_self_vote(W + 1, 0).expect(
        "once committed advances to W, W+1 is inside the window again and gate 9 admits it",
    );
    assert_eq!(
        state.voted_view,
        Some((W + 1, 0)),
        "the window slid and voting resumed; the two guards do not deadlock"
    );
}

// ---------------------------------------------------------------------------
// Restart shaped like the wedge: high persisted cursors, low committed floor
// ---------------------------------------------------------------------------

#[test]
fn window_parks_wedge_shaped_restart() {
    let validators = make_validators(4);
    let validator_set: Vec<Address> = validators.iter().map(|(a, _, _)| *a).collect();

    // A small-number analog of the incident's durable state: committed floor
    // frozen at 10 while the persisted frontier and vote marks sit hundreds
    // of heights higher.
    let floor = 10u64;
    let frontier = floor + W + 200;
    let mut db = MemKv::new();
    db.put(KEY_COMMITTED_HEIGHT, &floor.to_be_bytes())
        .expect("seed committed height");
    let frontier_qc = make_qc(frontier, [0xCC; 32]);
    let qc_bytes = encode_qc_v1(&frontier_qc).expect("encode qc");
    db.put(KEY_HIGHEST_QC, &qc_bytes).expect("seed highest qc");
    db.put(KEY_LOCKED_QC, &qc_bytes).expect("seed locked qc");
    db.put(KEY_VOTED_VIEW, &encode_voted_view_v1(frontier, 0))
        .expect("seed voted view");

    // The leader for the view proposing frontier + 1 is index
    // (frontier + 0) % 4; recover AS that validator so the propose refusal
    // below is the window, never NotLeader.
    let leader_idx = (frontier as usize) % 4;
    let mut state = ConsensusState::recover(validator_set[leader_idx], &db).expect("recover");
    assert_eq!(state.committed_height, floor);
    assert_eq!(state.highest_qc.as_ref().map(|q| q.height), Some(frontier));

    // The recovered node PARKS: no proposal, no vote, no new durable marks.
    let mut pool = mempool::TxMempool::new(1, 100);
    let res = state.propose_block(&mut pool, &TestNonceProvider, &db, &validator_set);
    let msg = format!("{res:?}");
    assert!(
        res.is_err() && msg.contains("CommitWindow"),
        "a wedge-shaped restart must refuse to keep proposing above the \
         window, got: {msg}"
    );

    let candidate = make_block(frontier + 1, frontier_qc.block_hash);
    let res = state.verify_block(&candidate, &db);
    let msg = format!("{res:?}");
    assert!(
        res.is_err() && msg.contains("commit window"),
        "a wedge-shaped restart must refuse to keep voting above the window, \
         got: {msg}"
    );

    // Gate 9 alone would ADMIT the next view (it only refuses replays at or
    // below the mark); the park comes from the window, an independent gate.
    assert!(
        state.may_vote(frontier + 1, 0),
        "gate 9 is not what parks the climb; the window is"
    );
    let res = state.note_self_vote(frontier + 1, 0);
    assert!(
        format!("{res:?}").contains("CommitWindow"),
        "the self-vote backstop must refuse above the window on the \
         recovered state, got: {res:?}"
    );
    assert_eq!(
        state.voted_view,
        Some((frontier, 0)),
        "the refused vote must not move the recovered durable mark"
    );
}

// ---------------------------------------------------------------------------
// A legitimately behind node: adoption ungated, the window slides with sync
// ---------------------------------------------------------------------------

#[test]
fn window_slides_for_behind_node_as_sync_commits() {
    let validators = make_validators(4);
    let our = validators[0].0;
    let mut state = ConsensusState::new(our);
    let db = MemKv::new();

    // A node far behind adopts the fleet's frontier QC. Adoption must stay
    // ungated: refusing it would strand the node without a sync target.
    let fleet_frontier = W + 500;
    let parent = make_block(fleet_frontier, [0xBB; 32]);
    let parent_hash = hash_block_v1(&parent).expect("hash");
    let frontier_qc = make_qc(fleet_frontier, parent_hash);
    state
        .cache_qc_and_check_commit(frontier_qc, &db)
        .expect_err("commit walk fails while behind; bodies not yet synced");
    assert_eq!(
        state.highest_qc.as_ref().map(|q| q.height),
        Some(fleet_frontier),
        "frontier adoption must not be gated by the commit window"
    );

    // While committed is far behind, the node must not VOTE at the frontier: the
    // commit-window check refuses it before any state resolution.
    let candidate = make_block(fleet_frontier + 1, parent_hash);
    let msg = format!("{:?}", state.verify_block(&candidate, &db));
    assert!(
        msg.contains("commit window"),
        "a behind node must not vote {} heights above its own committed height, got: {msg}",
        fleet_frontier + 1
    );

    // The window is measured from committed and slides forward as sync commits:
    // once committed catches up to within W of the frontier, the same height is
    // inside the window. (Reconceived, gate wedge-276272: actually casting the vote
    // there ALSO requires the pipeline to be executed up to the parent, the
    // execution barrier; sync supplies and executes those bodies as committed
    // advances, so the catching-up node is never wedged. Here we pin that the
    // window itself slides rather than the now-barrier-gated vote.)
    assert!(
        !state.within_commit_window(fleet_frontier + 1),
        "before sync: frontier+1 is above committed(0) + W"
    );
    state.committed_height = 501;
    assert!(
        state.within_commit_window(fleet_frontier + 1),
        "after sync commits to 501: frontier+1 is inside committed(501) + W, the window slid"
    );
}
