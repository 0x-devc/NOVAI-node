//! Gate F5 Stage 1 RED tests: behind-retention detection and the
//! snapshot-sync state machine.
//!
//! A validator that falls further behind than the fleet's prune horizon
//! cannot recover by block sync, because the blocks it needs no longer exist
//! on any peer. The node has detected that condition since F1
//! (`SyncRequestOutcome::BehindRetention`), but the detection had no state
//! and no action behind it: it logged at ERROR once a minute and probed once
//! a minute, forever. Stage 1 gives the detection a state machine, an
//! evidence rule, and a gauge, so the later stages have something to hang the
//! fetch and install on and so the operator gets an alarm that names the
//! actual condition.
//!
//! What is pinned here:
//! - arming requires ARM_PROBE_FAILURES CONSECUTIVE unserved probes, and a
//!   served probe (commit progress) resets the count, so "consecutive" is
//!   real and not a lifetime tally;
//! - commit progress disarms from every phase, including on the no-gap path,
//!   so a node that catches up can never be left armed;
//! - a strike inside the normal retention band can never arm the machine,
//!   which is the invariant that keeps an ordinary slow peer from ever
//!   looking like an unrecoverable node;
//! - the gauge encoding the monitor's behind_retention alarm reads.
//!
//! RED discipline: this file reads API that does not exist on the tree that
//! precedes the fix, so its RED state is a compile failure, which is a weak
//! RED on its own. The load-bearing evidence is therefore the MUTATION proof
//! recorded at the gate: each feared regression (dropping the consecutive
//! rule, dropping the disarm, moving the disarm after the no-gap return,
//! letting a normal-band strike arm) was applied to the working tree in turn,
//! the named test was proven to FAIL for the stated reason, and the mutation
//! was reverted. The strict-greater-than retention boundary itself (T1.1) is
//! already pinned by `behind_retention_returns_needs_snapshot_outcome` in
//! gate_sync_backoff_green.rs and is deliberately NOT duplicated here.

use ed25519_dalek::SigningKey;
use novai_consensus::PRUNE_RETAIN_BLOCKS;
use novai_consensus_types::{BlockResponse, Vote, QC};
use novai_crypto::address_from_pubkey;
use novai_node::consensus_node::{
    ConsensusNode, SnapshotSyncPhase, SyncRequestOutcome, ARM_PROBE_FAILURES, SYNC_CHUNK_SIZE,
    SYNC_RETRY_MAX_MS,
};
use novai_types::Address;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A domain-separated, validly signed vote (mirrors gate_sync_backoff_green).
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

fn seed_highest_qc(node: &ConsensusNode, signer: &SigningKey, voter: Address, height: u64) {
    let block_hash = [0x42u8; 32];
    let qc = QC {
        height,
        round: 0,
        block_hash,
        votes: vec![signed_vote(signer, voter, height, 0, block_hash)],
    };
    node.state.lock().unwrap().highest_qc = Some(qc);
}

fn empty_response(responder: Address, request_start: u64, request_end: u64) -> BlockResponse {
    BlockResponse {
        responder,
        request_start,
        request_end,
        blocks: vec![],
        qcs: vec![],
    }
}

/// Backdate the last sync attempt so the next behind-retention probe is due.
/// The probe period is SYNC_RETRY_MAX_MS, so one second past it suffices.
fn make_probe_due(node: &ConsensusNode) {
    let mut retry = node.sync_retry.lock().unwrap();
    retry.last_attempt = Some(
        Instant::now()
            .checked_sub(Duration::from_millis(SYNC_RETRY_MAX_MS + 1_000))
            .expect("test host uptime should exceed the backdate window"),
    );
    retry.last_escalation_log = None;
}

/// Drive one full behind-retention probe cycle that comes back unserved.
/// The empty answer echoes the range the node actually asked for, read from
/// the pending slot, so the response correlates the way a real peer's would
/// (`handle_block_response` settles the slot only on a matching
/// `request_start`, and only a settled slot records the strike).
fn one_unserved_probe(node: &ConsensusNode, responder: Address) {
    make_probe_due(node);
    let outcome = node.try_request_missing_blocks();
    assert_eq!(
        outcome,
        SyncRequestOutcome::BehindRetention { probed: true },
        "a due behind-retention cycle must issue the low-rate probe"
    );
    let (start, end) = node
        .pending_sync_request
        .lock()
        .unwrap()
        .as_ref()
        .map(|p| (p.start_height, p.end_height))
        .expect("the probe must have armed the pending slot");
    node.handle_block_response(empty_response(responder, start, end))
        .expect("handling an empty block response must not error");
}

// ---------------------------------------------------------------------------
// T1.2 arming requires consecutive unserved probes
// ---------------------------------------------------------------------------

#[test]
fn arm_threshold_constant_is_pinned() {
    // Two failed probes at the 60 second probe period is a two minute arming
    // delay on a node that has already been unrecoverable for hours, so the
    // cost is nil, and it removes the entire class of "we armed on a single
    // transient answer". Raising this is safe; lowering it to 1 removes the
    // evidence requirement, which is the point of the rule.
    assert_eq!(ARM_PROBE_FAILURES, 2);
}

#[test]
fn a_fresh_node_is_idle() {
    let (node, _sk, _addr) = two_validator_node();
    let m = node.snapshot_sync();
    assert_eq!(m.phase(), SnapshotSyncPhase::Idle);
    assert_eq!(m.unserved_probes(), 0);
    assert_eq!(m.gauge(), 0);
}

#[test]
fn arming_requires_consecutive_unserved_probes() {
    let (node, sk_a, addr_a) = two_validator_node();
    seed_highest_qc(&node, &sk_a, addr_a, PRUNE_RETAIN_BLOCKS + 1);

    // The first beyond-retention cycle enters the arming band and probes.
    // Entering is not evidence: the probe has not been answered yet.
    assert_eq!(
        node.try_request_missing_blocks(),
        SyncRequestOutcome::BehindRetention { probed: true }
    );
    assert_eq!(node.snapshot_sync().phase(), SnapshotSyncPhase::Arming);
    assert_eq!(
        node.snapshot_sync().unserved_probes(),
        0,
        "entering the band is not yet evidence that peers cannot serve"
    );

    // First unserved answer. One is not enough.
    node.handle_block_response(empty_response(addr_a, 1, SYNC_CHUNK_SIZE))
        .expect("handling an empty block response must not error");
    assert_eq!(node.snapshot_sync().unserved_probes(), 1);
    assert_eq!(
        node.snapshot_sync().phase(),
        SnapshotSyncPhase::Arming,
        "one unserved probe must not arm"
    );

    // Second unserved answer reaches the threshold.
    one_unserved_probe(&node, addr_a);
    assert_eq!(node.snapshot_sync().unserved_probes(), ARM_PROBE_FAILURES);
    assert_eq!(
        node.snapshot_sync().phase(),
        SnapshotSyncPhase::Armed,
        "ARM_PROBE_FAILURES consecutive unserved probes arm the machine"
    );
}

#[test]
fn commit_progress_resets_the_probe_count_so_probes_must_be_consecutive() {
    let (node, sk_a, addr_a) = two_validator_node();
    seed_highest_qc(&node, &sk_a, addr_a, PRUNE_RETAIN_BLOCKS + 1);

    // One unserved probe banked.
    assert_eq!(
        node.try_request_missing_blocks(),
        SyncRequestOutcome::BehindRetention { probed: true }
    );
    node.handle_block_response(empty_response(addr_a, 1, SYNC_CHUNK_SIZE))
        .expect("empty response");
    assert_eq!(node.snapshot_sync().unserved_probes(), 1);

    // A probe gets served: committed advances. That is the disarm signal, and
    // it must clear the banked COUNT, not merely the phase.
    //
    // The gap here stays beyond retention on purpose, which is the harder and
    // more realistic case: within the same call the machine disarms on the
    // commit progress and then legitimately re-enters the arming band, because
    // the node is still past the horizon. So the observable proof of the
    // disarm is the cleared count, not the phase. (The phase-level disarm is
    // pinned by the two commit_progress_disarms_from_* tests, where the gap
    // falls back inside the window.)
    node.state.lock().unwrap().committed_height = 1;
    seed_highest_qc(&node, &sk_a, addr_a, PRUNE_RETAIN_BLOCKS + 2);
    node.try_request_missing_blocks();
    assert_eq!(
        node.snapshot_sync().unserved_probes(),
        0,
        "commit progress must clear the banked probe count, or two \
         non-consecutive failures hours apart would arm the machine"
    );

    // A single fresh unserved probe must therefore NOT arm.
    one_unserved_probe(&node, addr_a);
    assert_eq!(node.snapshot_sync().unserved_probes(), 1);
    assert_eq!(
        node.snapshot_sync().phase(),
        SnapshotSyncPhase::Arming,
        "the count restarted, so one probe after progress cannot arm"
    );
}

// ---------------------------------------------------------------------------
// T1.3 commit progress disarms from every phase
// ---------------------------------------------------------------------------

#[test]
fn commit_progress_disarms_from_arming() {
    let (node, sk_a, addr_a) = two_validator_node();
    seed_highest_qc(&node, &sk_a, addr_a, PRUNE_RETAIN_BLOCKS + 1);
    node.try_request_missing_blocks();
    assert_eq!(node.snapshot_sync().phase(), SnapshotSyncPhase::Arming);

    node.state.lock().unwrap().committed_height = 5;
    node.try_request_missing_blocks();
    assert_eq!(node.snapshot_sync().phase(), SnapshotSyncPhase::Idle);
}

#[test]
fn commit_progress_disarms_from_armed() {
    let (node, sk_a, addr_a) = two_validator_node();
    seed_highest_qc(&node, &sk_a, addr_a, PRUNE_RETAIN_BLOCKS + 1);
    assert_eq!(
        node.try_request_missing_blocks(),
        SyncRequestOutcome::BehindRetention { probed: true }
    );
    node.handle_block_response(empty_response(addr_a, 1, SYNC_CHUNK_SIZE))
        .expect("empty response");
    one_unserved_probe(&node, addr_a);
    assert_eq!(node.snapshot_sync().phase(), SnapshotSyncPhase::Armed);

    // Armed is not terminal against reality: if this node commits anything,
    // block sync is working and it must never install a snapshot.
    node.state.lock().unwrap().committed_height = 5;
    node.try_request_missing_blocks();
    assert_eq!(
        node.snapshot_sync().phase(),
        SnapshotSyncPhase::Idle,
        "a node that is committing must never stay armed"
    );
}

#[test]
fn disarm_runs_even_on_the_no_gap_path() {
    // The self-correcting case that matters most: a node right at the
    // retention boundary gets served and catches up completely. The no-gap
    // early return must NOT skip the disarm, or the node would sit armed
    // forever while perfectly healthy.
    let (node, sk_a, addr_a) = two_validator_node();
    seed_highest_qc(&node, &sk_a, addr_a, PRUNE_RETAIN_BLOCKS + 1);
    node.try_request_missing_blocks();
    assert_eq!(node.snapshot_sync().phase(), SnapshotSyncPhase::Arming);

    // Fully caught up: highest_qc == committed, so the function returns NoGap
    // before it ever reaches the retention arithmetic.
    node.state.lock().unwrap().committed_height = PRUNE_RETAIN_BLOCKS + 1;
    assert_eq!(
        node.try_request_missing_blocks(),
        SyncRequestOutcome::NoGap,
        "this test is only meaningful if the call takes the no-gap path"
    );
    assert_eq!(
        node.snapshot_sync().phase(),
        SnapshotSyncPhase::Idle,
        "the disarm must run BEFORE the no-gap early return"
    );
}

#[test]
fn a_normal_band_strike_never_arms_the_machine() {
    // An ordinary slow or empty-handed peer inside the retention window
    // records strikes exactly as before. None of them may advance the
    // snapshot machine, or a transient peer problem would look identical to
    // an unrecoverable node.
    let (node, sk_a, addr_a) = two_validator_node();
    seed_highest_qc(&node, &sk_a, addr_a, 100);

    assert_eq!(
        node.try_request_missing_blocks(),
        SyncRequestOutcome::Requested
    );
    node.handle_block_response(empty_response(addr_a, 1, 100))
        .expect("empty response");

    assert_eq!(
        node.sync_retry.lock().unwrap().strikes,
        1,
        "the F1 strike accounting is unchanged"
    );
    assert_eq!(
        node.snapshot_sync().phase(),
        SnapshotSyncPhase::Idle,
        "a strike inside the retention window must never arm"
    );
    assert_eq!(node.snapshot_sync().unserved_probes(), 0);
}

// ---------------------------------------------------------------------------
// T1.4 the gauge reflects each transition
// ---------------------------------------------------------------------------

#[test]
fn gauge_reflects_each_transition() {
    let (node, sk_a, addr_a) = two_validator_node();
    assert_eq!(node.snapshot_sync().gauge(), 0, "idle");

    seed_highest_qc(&node, &sk_a, addr_a, PRUNE_RETAIN_BLOCKS + 1);
    node.try_request_missing_blocks();
    assert_eq!(node.snapshot_sync().gauge(), 1, "arming");

    node.handle_block_response(empty_response(addr_a, 1, SYNC_CHUNK_SIZE))
        .expect("empty response");
    one_unserved_probe(&node, addr_a);
    assert_eq!(node.snapshot_sync().gauge(), 2, "armed");

    node.state.lock().unwrap().committed_height = 5;
    node.try_request_missing_blocks();
    assert_eq!(node.snapshot_sync().gauge(), 0, "back to idle");
}

#[test]
fn metrics_surface_exposes_sync_mode() {
    // The gauge is the contract the monitor's behind_retention alarm reads.
    // Pinned on the TYPE line, matching gate_commit_gap_metrics.rs.
    let out = novai_node::metrics::MetricsSnapshot {
        committed_height: 1_580_000,
        highest_qc_height: 1_925_939,
        seconds_since_last_commit: 86_400,
        current_round: 0,
        peer_count: 3,
        mempool_size: 0,
        mempool_ready: 0,
        mempool_waiting: 0,
        mempool_gapped: 0,
        mempool_senders: 0,
        mempool_rejects_nonce_too_low: 0,
        mempool_rejects_nonce_too_high: 0,
        mempool_rejects_sender_limit: 0,
        mempool_rejects_fee_too_low: 0,
        mempool_rejects_full: 0,
        view_changes_total: 0,
        block_tx_count: 0,
        total_txs_committed: 0,
        copilot_observations_total: 0,
        anomaly_signals_total: 0,
        anomaly_signals_published: 0,
        anomaly_last_confidence: 0,
        sync_mode: 2,
    }
    .to_prometheus();

    assert!(
        out.contains("# TYPE novai_sync_mode gauge"),
        "the metrics surface must expose the snapshot-sync detection phase; \
         without it a 346,000 block unrecoverable gap and a 30 second commit \
         hiccup look identical to the operator"
    );
    assert!(out.contains("novai_sync_mode 2"));
}
