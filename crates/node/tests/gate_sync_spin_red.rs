//! Gate F1 RED test: the block-sync requester spins on an unservable range.
//!
//! Reproduces the node2 2026-07-03 sync-stall class of bug in-process. Every
//! healthy peer prunes block and QC rows older than PRUNE_RETAIN_BLOCKS
//! (50_000) behind its committed height, so a node that is far behind
//! requests a range no peer can serve and every peer answers with an EMPTY
//! BlockResponse. Today the requester (all sites at commit 11da81a):
//!
//! 1. ignores a matching empty response WITHOUT releasing
//!    `pending_sync_request` (`handle_block_response` returns early for
//!    empty responses, consensus_node.rs:887-893, before the clear at
//!    :895-899), burning the full 5s timeout on an answer that already
//!    arrived, and
//! 2. once the binary main-loop sweep clears the slot after 5s
//!    (main.rs:1638-1653), the 2s trigger (main.rs:1655-1660) calls
//!    `try_request_missing_blocks`, which recomputes the IDENTICAL range
//!    from the unadvanced committed height (consensus_node.rs:2085-2088)
//!    and re-issues it: no backoff, no strike accounting, no distinction
//!    between "peer lacks the range (pruned)" and "response dropped", and
//!    no behind-retention escalation. Forever.
//!
//! Live signature: node2 stuck at height 1 spamming "Sync request timed out
//! ... start_height=1 end_height=500" against a fleet at ~4.36M.
//!
//! The timeout sweep and the 2s re-trigger live inline in the binary loop
//! (main.rs), not on ConsensusNode, so this file replicates the sweep's
//! decision logic (clear the slot once the request is at least 5s old) and
//! backdates `request_time` to make that decision fire without sleeping,
//! per the house idiom (see check_timeout_backs_off_on_repeated_failure in
//! sync_test.rs).
//!
//! `matching_empty_response_releases_pending_slot` is RED today: the empty
//! early-return leaves the slot armed. It flips GREEN when the requester
//! treats a matching empty response as a definitive answer.
//!
//! `unservable_range_is_not_respun_identically_without_backoff` is RED
//! today: four back-to-back trigger cycles issue four identical 1..500
//! requests with zero elapsed wall time. It flips GREEN when retries are
//! gated by backoff or behind-retention escalation. The assertion tolerates
//! one initial request (a first attempt or low-rate probe is legitimate).
//!
//! `first_sync_request_arms_pending_slot` is the harness soundness proof
//! and the post-fix liveness guard: a behind node must still issue its
//! FIRST request for an in-window gap. The fix must throttle retries,
//! never initial sync. It passes today and must keep passing.

use ed25519_dalek::SigningKey;
use novai_consensus_types::{BlockResponse, Vote, QC};
use novai_crypto::address_from_pubkey;
use novai_node::consensus_node::{ConsensusNode, SYNC_CHUNK_SIZE};
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
/// anyway so the seeded state is well formed (quorum is 1 for a 2-validator
/// set).
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

/// Backdate the in-flight request so the sweep's 5s decision fires now,
/// per the house Instant-backdating idiom. No-op when nothing is pending.
fn backdate_pending_request(node: &ConsensusNode, age: Duration) {
    let mut pending = node.pending_sync_request.lock().unwrap();
    if let Some(ref mut request) = *pending {
        request.request_time = Instant::now()
            .checked_sub(age)
            .expect("test host uptime should exceed the backdate window");
    }
}

/// Replica of the binary main-loop sync-timeout sweep ("Sync request timed
/// out", main.rs:1638-1653 at 11da81a): once the pending request is at least
/// 5s old, the slot is cleared. That clear is the ONLY timeout handling in
/// production; logging is omitted here, the decision logic is identical.
fn sweep_pending_sync_timeout(node: &ConsensusNode) {
    let mut pending = node.pending_sync_request.lock().unwrap();
    if let Some(ref request) = *pending {
        if request.request_time.elapsed() >= Duration::from_secs(5) {
            *pending = None;
        }
    }
}

/// RED today, GREEN after the F1 fix.
///
/// A matching empty BlockResponse is a definitive answer: the peer received
/// the request and has none of the blocks. The requester must release the
/// pending slot (and account the failure) when that answer arrives, instead
/// of holding the slot until the 5s timeout as if the peer had never
/// responded. Today the empty early-return leaves the slot armed, so every
/// probe of an unservable range silently costs the full timeout.
#[test]
fn matching_empty_response_releases_pending_slot() {
    let (node, sk_a, addr_a) = two_validator_node();
    // In-window gap: committed 0, tip 10. The empty-response defect is not
    // retention-specific; a peer can lack an in-window range too (partial
    // prune, lagging store), and a dropped-vs-answered distinction matters
    // just as much there.
    seed_highest_qc(&node, &sk_a, addr_a, 10);

    node.try_request_missing_blocks();
    assert_eq!(
        pending_range(&node),
        Some((1, 10)),
        "precondition: a behind node (committed 0, hqc 10) arms a 1..10 request"
    );

    // Every peer answers the broadcast; this one faithfully echoes the
    // requested range and has no blocks (the pruned-peer answer shape).
    let response = BlockResponse {
        responder: addr_a,
        request_start: 1,
        request_end: 10,
        blocks: vec![],
        qcs: vec![],
    };
    node.handle_block_response(response)
        .expect("handling an empty block response must not error");

    assert!(
        pending_range(&node).is_none(),
        "SPIN PRECONDITION: a matching empty BlockResponse arrived (the peer \
         answered: it lacks the range), but pending_sync_request is still \
         armed. The requester treats a definitive answer like silence, burns \
         the full 5s timeout every cycle, and cannot distinguish 'peer lacks \
         range' from 'response dropped'. The requester must release the slot \
         when a matching empty response arrives."
    );
}

/// RED today, GREEN after the F1 fix.
///
/// Incident geometry: committed 0 and a fleet tip far beyond
/// PRUNE_RETAIN_BLOCKS, so heights 1..500 exist on no honest peer and every
/// response is empty (4_000_000 mirrors the live fleet at ~4.36M). Each
/// cycle below is one turn of the production loop: 2s trigger, empty answer
/// from the fleet, 5s timeout sweep. Today that loop re-issues the IDENTICAL
/// range every cycle with no backoff, no strike accounting, and no
/// behind-retention escalation. After the fix at most one initial request
/// (or low-rate probe) may be issued in a zero-elapsed-time window.
#[test]
fn unservable_range_is_not_respun_identically_without_backoff() {
    let (node, sk_a, addr_a) = two_validator_node();
    seed_highest_qc(&node, &sk_a, addr_a, 4_000_000);

    let mut issued: Vec<(u64, u64)> = Vec::new();
    for _cycle in 0..4 {
        let before = pending_range(&node);
        // The 2s periodic trigger (main.rs:1655-1660).
        node.try_request_missing_blocks();
        let after = pending_range(&node);
        // Count only requests newly armed by this cycle's trigger.
        if before.is_none() {
            if let Some(range) = after {
                issued.push(range);
            }
        }
        // Every peer answers the broadcast with a faithful empty response.
        if let Some((start, end)) = after {
            let response = BlockResponse {
                responder: addr_a,
                request_start: start,
                request_end: end,
                blocks: vec![],
                qcs: vec![],
            };
            node.handle_block_response(response)
                .expect("handling an empty block response must not error");
        }
        // The binary's 500ms sweep clears a pending request once it is 5s
        // old; backdate so its decision fires now instead of sleeping.
        backdate_pending_request(&node, Duration::from_secs(6));
        sweep_pending_sync_timeout(&node);
    }

    let identical = issued
        .iter()
        .filter(|range| **range == (1, SYNC_CHUNK_SIZE))
        .count();
    assert!(
        identical <= 1,
        "REQUESTER SPIN: {identical} identical 1..{SYNC_CHUNK_SIZE} requests were issued \
         across 4 back-to-back cycles with zero elapsed wall time and only \
         empty responses (issued log: {issued:?}). A range no peer can serve \
         must not be re-requested verbatim with no backoff, no strike \
         accounting, and no behind-retention escalation; a deterministic \
         'cannot block-sync this range, needs snapshot' outcome must exist \
         for the layer above. This loop is the live node2 stall: 'Sync \
         request timed out ... start_height=1 end_height=500' forever."
    );
}

/// Harness soundness proof and post-fix liveness guard. Passes today and
/// must keep passing after the fix.
///
/// A behind node with an in-window gap must still issue its FIRST sync
/// request immediately. The F1 fix gates RETRIES (backoff, strikes,
/// behind-retention escalation); it must never suppress or delay initial
/// sync on the normal in-window path.
#[test]
fn first_sync_request_arms_pending_slot() {
    let (node, sk_a, addr_a) = two_validator_node();
    seed_highest_qc(&node, &sk_a, addr_a, 10);

    node.try_request_missing_blocks();

    assert_eq!(
        pending_range(&node),
        Some((1, 10)),
        "a behind node (committed 0, hqc 10) must arm its first 1..10 sync \
         request immediately; retry throttling must never suppress the \
         initial request"
    );
}
