//! Gate F1 GREEN companions: the sync retry gate behaves per the approved
//! design (fixplan section c, requester spin fix).
//!
//! The RED tests live in gate_sync_spin_red.rs and are deliberately NOT
//! edited by the fix; this file adds the assertions that could not compile
//! before the fix existed because they read the new API (SyncRequestOutcome,
//! SyncRetryState, the pure backoff functions).
//!
//! Covered here:
//! - the pure decision schedule min(2s * 2^strikes, 60s), including the
//!   zero-strike no-gate rule, the cap, and shift-overflow saturation;
//! - strike accounting: one strike per failed cycle, broadcast fan-in does
//!   not double-strike, non-matching responses neither settle nor strike;
//! - timeouts strike exactly like served-empty answers (dropped response
//!   and pruned peer engage the same gate);
//! - the deterministic BehindRetention "needs snapshot" outcome for F4,
//!   its strict greater-than boundary against PRUNE_RETAIN_BLOCKS, and the
//!   at-most-one-probe-per-period rule;
//! - commit progress resets the gate so multi-chunk catch-up runs at full
//!   cadence.

use ed25519_dalek::SigningKey;
use novai_consensus::PRUNE_RETAIN_BLOCKS;
use novai_consensus_types::{BlockResponse, Vote, QC};
use novai_crypto::address_from_pubkey;
use novai_node::consensus_node::{
    sync_backoff_ms, sync_retry_due, ConsensusNode, SyncRequestOutcome, SYNC_CHUNK_SIZE,
};
use novai_types::Address;
use std::collections::HashMap;
use std::time::{Duration, Instant};

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

/// Build a 2-validator node under test (validator B) plus the signing key for
/// validator A, which signs seeded QCs. Deterministic keys, so no randomness.
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

/// Seed `highest_qc` at `height` so `try_request_missing_blocks` sees a sync
/// gap. Only the QC height is read on this path; the vote is validly signed
/// anyway so the seeded state is well formed.
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

/// The currently armed sync request range, if any.
fn pending_range(node: &ConsensusNode) -> Option<(u64, u64)> {
    node.pending_sync_request
        .lock()
        .unwrap()
        .as_ref()
        .map(|p| (p.start_height, p.end_height))
}

/// A faithful empty answer to a request that started at `request_start`.
fn empty_response(responder: Address, request_start: u64, request_end: u64) -> BlockResponse {
    BlockResponse {
        responder,
        request_start,
        request_end,
        blocks: vec![],
        qcs: vec![],
    }
}

#[test]
fn backoff_schedule_is_exponential_and_capped() {
    assert_eq!(sync_backoff_ms(0), 0, "zero strikes means no gate");
    assert_eq!(sync_backoff_ms(1), 4_000);
    assert_eq!(sync_backoff_ms(2), 8_000);
    assert_eq!(sync_backoff_ms(3), 16_000);
    assert_eq!(sync_backoff_ms(4), 32_000);
    assert_eq!(sync_backoff_ms(5), 60_000, "min(2s * 2^5, 60s) caps at 60s");
    assert_eq!(sync_backoff_ms(6), 60_000);
    assert_eq!(
        sync_backoff_ms(200),
        60_000,
        "shift overflow must saturate at the cap, never wrap"
    );

    assert!(sync_retry_due(0, None));
    assert!(sync_retry_due(7, None), "the first attempt is never gated");
    assert!(sync_retry_due(0, Some(Duration::ZERO)));
    assert!(!sync_retry_due(1, Some(Duration::from_millis(3_999))));
    assert!(sync_retry_due(1, Some(Duration::from_millis(4_000))));
    assert!(!sync_retry_due(200, Some(Duration::from_millis(59_999))));
    assert!(sync_retry_due(200, Some(Duration::from_millis(60_000))));
}

#[test]
fn matching_empty_response_records_one_strike_per_cycle() {
    let (node, sk_a, addr_a) = two_validator_node();
    seed_highest_qc(&node, &sk_a, addr_a, 10);
    assert_eq!(
        node.try_request_missing_blocks(),
        SyncRequestOutcome::Requested
    );
    assert_eq!(pending_range(&node), Some((1, 10)));

    // First matching empty answer: settles the slot and strikes once.
    let response = empty_response(addr_a, 1, 10);
    node.handle_block_response(response.clone())
        .expect("handling an empty block response must not error");
    assert_eq!(
        pending_range(&node),
        None,
        "a matching empty response settles the pending slot"
    );
    assert_eq!(
        node.sync_retry.lock().unwrap().strikes,
        1,
        "one strike per failed cycle"
    );

    // Broadcast fan-in: further empty answers to the same settled cycle
    // must not strike again.
    node.handle_block_response(response)
        .expect("handling an empty block response must not error");
    assert_eq!(
        node.sync_retry.lock().unwrap().strikes,
        1,
        "later empty answers in the same cycle must not double-strike"
    );
}

#[test]
fn non_matching_response_leaves_pending_and_strikes_untouched() {
    let (node, sk_a, addr_a) = two_validator_node();
    seed_highest_qc(&node, &sk_a, addr_a, 10);
    assert_eq!(
        node.try_request_missing_blocks(),
        SyncRequestOutcome::Requested
    );

    // An empty answer to some OTHER node's broadcast request (different
    // request_start) is not ours to settle.
    let response = empty_response(addr_a, 999, 1_400);
    node.handle_block_response(response)
        .expect("handling an empty block response must not error");
    assert_eq!(
        pending_range(&node),
        Some((1, 10)),
        "a non-matching response must not settle our pending slot"
    );
    assert_eq!(
        node.sync_retry.lock().unwrap().strikes,
        0,
        "a non-matching response must not strike"
    );
}

#[test]
fn timeout_strikes_engage_the_backoff_gate() {
    let (node, sk_a, addr_a) = two_validator_node();
    seed_highest_qc(&node, &sk_a, addr_a, 10);
    assert_eq!(
        node.try_request_missing_blocks(),
        SyncRequestOutcome::Requested
    );

    // The binary main-loop sweep clears the timed-out slot, then reports
    // the timeout; a dropped response is a failed cycle like any other.
    *node.pending_sync_request.lock().unwrap() = None;
    node.on_sync_request_timeout();
    assert_eq!(
        node.sync_retry.lock().unwrap().strikes,
        1,
        "a timeout must record a strike"
    );

    // An immediate re-trigger is gated: min(2s * 2^1, 60s) has not elapsed.
    assert_eq!(
        node.try_request_missing_blocks(),
        SyncRequestOutcome::BackedOff
    );
    assert_eq!(
        pending_range(&node),
        None,
        "no identical re-request may be issued while backed off"
    );
}

#[test]
fn behind_retention_returns_needs_snapshot_outcome() {
    // Boundary pin: a gap of exactly PRUNE_RETAIN_BLOCKS is still (barely)
    // servable and takes the normal request path (strict greater-than).
    let (node_at_edge, sk_edge, addr_edge) = two_validator_node();
    seed_highest_qc(&node_at_edge, &sk_edge, addr_edge, PRUNE_RETAIN_BLOCKS);
    assert_eq!(
        node_at_edge.try_request_missing_blocks(),
        SyncRequestOutcome::Requested,
        "a gap of exactly PRUNE_RETAIN_BLOCKS is not behind retention"
    );

    // One block past the window: no honest peer retains the range, so the
    // outcome is the deterministic needs-snapshot signal for F4.
    let (node, sk_a, addr_a) = two_validator_node();
    seed_highest_qc(&node, &sk_a, addr_a, PRUNE_RETAIN_BLOCKS + 1);
    assert_eq!(
        node.try_request_missing_blocks(),
        SyncRequestOutcome::BehindRetention { probed: true },
        "the first behind-retention call issues the low-rate probe"
    );
    assert_eq!(
        pending_range(&node),
        Some((1, SYNC_CHUNK_SIZE)),
        "the probe arms the pending slot like any request"
    );

    // The fleet answers empty: the slot settles and one strike is recorded.
    node.handle_block_response(empty_response(addr_a, 1, SYNC_CHUNK_SIZE))
        .expect("handling an empty block response must not error");
    assert_eq!(pending_range(&node), None);
    assert_eq!(node.sync_retry.lock().unwrap().strikes, 1);

    // An immediate re-trigger stays deterministic and must NOT probe again
    // within the max-backoff period: the spin is over.
    assert_eq!(
        node.try_request_missing_blocks(),
        SyncRequestOutcome::BehindRetention { probed: false },
        "at most one probe per max-backoff period"
    );
    assert_eq!(
        pending_range(&node),
        None,
        "no new request inside the probe period"
    );
}

#[test]
fn backoff_gates_retries_and_commit_progress_resets() {
    let (node, sk_a, addr_a) = two_validator_node();
    seed_highest_qc(&node, &sk_a, addr_a, 100);

    // Two prior failed cycles with a fresh attempt: the gate refuses.
    {
        let mut retry = node.sync_retry.lock().unwrap();
        retry.strikes = 2;
        retry.strike_committed_height = 0;
        retry.last_attempt = Some(Instant::now());
    }
    assert_eq!(
        node.try_request_missing_blocks(),
        SyncRequestOutcome::BackedOff
    );
    assert_eq!(pending_range(&node), None);

    // Once min(2s * 2^2, 60s) = 8s has elapsed, the retry is due again.
    {
        let mut retry = node.sync_retry.lock().unwrap();
        retry.last_attempt = Some(
            Instant::now()
                .checked_sub(Duration::from_secs(9))
                .expect("test host uptime should exceed the backdate window"),
        );
    }
    assert_eq!(
        node.try_request_missing_blocks(),
        SyncRequestOutcome::Requested,
        "an elapsed backoff window permits the retry"
    );
    assert_eq!(pending_range(&node), Some((1, 100)));

    // Commit progress lifts the gate entirely, whatever the strike count:
    // settle the slot, pile strikes high with a fresh attempt, advance
    // committed past the strike stamp.
    *node.pending_sync_request.lock().unwrap() = None;
    {
        let mut retry = node.sync_retry.lock().unwrap();
        retry.strikes = 6;
        retry.strike_committed_height = 0;
        retry.last_attempt = Some(Instant::now());
    }
    node.state.lock().unwrap().committed_height = 10;
    assert_eq!(
        node.try_request_missing_blocks(),
        SyncRequestOutcome::Requested,
        "commit progress must reset the strikes and lift the gate"
    );
    assert_eq!(
        pending_range(&node),
        Some((11, 100)),
        "catch-up continues from the new committed height at full cadence"
    );
    assert_eq!(
        node.sync_retry.lock().unwrap().strikes,
        0,
        "strikes reset on commit progress"
    );
}
