//! Gate F2 GREEN companions: budget properties the RED file cannot pin.
//!
//! Anchor: HEAD 5b9225a. Spec: docs/gate-syncbudget-fixplan.md section (c)
//! "Responder byte budget (P2 fix)"; line pins re-verified in
//! docs/gate-f2-diagnosis.md. This file mirrors the F1 split
//! (gate_sync_spin_red.rs + gate_sync_backoff_green.rs): the RED file
//! (gate_sync_budget_red.rs, T1 and T2) proves the hole exists on HEAD;
//! this file pins the positive properties of the fixed responder.
//!
//! T-series mapping:
//! - T3 (floor): a single (block, QC) pair over the SOFT budget
//!   (wire cap / 2 = 1 MiB) but UNDER the 2 MiB hard cap is still served,
//!   alone, instead of being dropped or starving the range.
//! - T3b (new, no existing T-ID): QC bytes count against the budget. The
//!   assembly loop pushes the block (consensus_node.rs:875, :879) BEFORE
//!   loading its QC (:885-896), so a fix that measures only block bytes
//!   under-counts by the QC trailer and still emits an over-cap response.
//!   This test exists to fail against exactly that buggy fix.
//! - Under-budget invariance (fixplan section (d): small-range syncs stay
//!   byte-for-byte unchanged). This one PASSES on HEAD; it doubles as the
//!   fixture soundness proof for both files and as the post-fix guard
//!   that the budget never truncates an already-legal response.
//! - Byte-accounting drift pin (step 3 review item 1): the responder's
//!   accounting (RESPONSE_HEADER_BYTES plus block + flag + QC per pair)
//!   must equal the real encode_block_response_v2 output exactly, so a
//!   codec layout change fails loudly instead of miscounting silently.
//!
//! Honesty about HEAD behavior, per the gate instructions: the T3 and T3b
//! bodies FAIL on HEAD, through the SAME over-cap mechanism as T1 (no
//! budget exists, all pairs are packed, encode_wire_message rejects at
//! p2p:120-122). That is why they are companions and not RED tests: they
//! add no new failure signal today, and their distinct value (floor
//! semantics, pair-vs-block measurement) is only verifiable once a budget
//! exists. Trivially-passing variants DO exist (drop the wire-encode gate
//! and assert only that the first pair is present; HEAD serves everything,
//! so they pass vacuously), but such variants cannot verify the budget
//! properties and cannot catch a block-only-measuring fix, so I did not
//! write them. Expected state: this file starts passing when the F2 fix
//! lands, with no edit.
//!
//! Deliberately NOT tested (F3 scope, flagged per the fixplan's frontier
//! paragraph): a single pair whose encoding exceeds the 2 MiB HARD cap
//! (the fixplan's degradation branch serves the block with its QC slot as
//! None, and a block that cannot fit alone is not servable at all until
//! the F3 cap raise). F2 makes no coverage claim there; T3's
//! preconditions pin its floor pair UNDER the hard cap explicitly.

use ed25519_dalek::SigningKey;
use novai_consensus_types::codec::{encode_block_response_v2, encode_block_v1, encode_qc_v1};
use novai_consensus_types::{Block, BlockRequest, BlockResponse, Vote, QC};
use novai_crypto::address_from_pubkey;
use novai_node::consensus_node::{ConsensusNode, RESPONSE_HEADER_BYTES};
use novai_p2p::{encode_wire_message, NetworkMessage};
use novai_state::Kv;
use novai_types::{Address, TxV1, TxVersion};
use std::collections::HashMap;

/// Mirror of the private MAX_WIRE_MSG_BYTES (p2p/src/lib.rs:23), used ONLY
/// for fix-agnostic preconditions on seeded data. Behavior assertions go
/// through `encode_wire_message`, which enforces the real constant.
const HARD_WIRE_CAP: usize = 2 * 1024 * 1024;

/// Constant state root for all fixture blocks (no commit callback in the
/// harness, consensus_node.rs:376, so nothing executes; mirrors
/// sync_test.rs).
const ROOT: [u8; 32] = [0xaa; 32];

/// A domain-separated, validly signed vote (mirrors sync_test.rs).
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

/// A single-vote QC certifying `block` (quorum is 1 for a 2-validator set,
/// consensus_node.rs:1147-1148).
fn certifying_qc(signer: &SigningKey, voter: Address, block: &Block) -> QC {
    let block_hash = novai_consensus_types::block_hash(block);
    QC {
        height: block.height,
        round: block.round,
        block_hash,
        votes: vec![signed_vote(
            signer,
            voter,
            block.height,
            block.round,
            block_hash,
        )],
    }
}

/// A syntactically valid transaction carrying `payload_len` opaque bytes
/// (encoded size is all that matters on the responder path).
fn fat_tx(fill: u8, nonce: u64, payload_len: usize) -> TxV1 {
    TxV1 {
        version: TxVersion::V1,
        from: [fill; 32],
        pubkey: [fill; 32],
        nonce,
        fee: 0,
        payload: vec![fill; payload_len],
        sig: [fill; 64],
    }
}

/// A parent-linked chain of `count` blocks from height 1.
fn fat_chain(count: u64, txs_per_block: usize, payload_len: usize) -> Vec<Block> {
    let mut blocks = Vec::with_capacity(count as usize);
    let mut parent_hash = [0u8; 32];
    for height in 1..=count {
        let txs = (0..txs_per_block)
            .map(|i| {
                fat_tx(
                    (height as u8).wrapping_mul(31).wrapping_add(i as u8) | 1,
                    i as u64,
                    payload_len,
                )
            })
            .collect();
        let block = Block {
            height,
            round: 0,
            parent_hash,
            state_root: ROOT,
            txs,
        };
        parent_hash = novai_consensus_types::block_hash(&block);
        blocks.push(block);
    }
    blocks
}

/// Two-validator world with deterministic keys (per gate_sync_spin_red.rs).
fn two_validator_world() -> (ConsensusNode, SigningKey, Address, Address) {
    let sk_a = SigningKey::from_bytes(&[1u8; 32]);
    let sk_b = SigningKey::from_bytes(&[2u8; 32]);
    let addr_a = address_from_pubkey(&sk_a.verifying_key());
    let addr_b = address_from_pubkey(&sk_b.verifying_key());
    let validator_set = vec![addr_a, addr_b];
    let mut validator_pubkeys = HashMap::new();
    validator_pubkeys.insert(addr_a, sk_a.verifying_key());
    validator_pubkeys.insert(addr_b, sk_b.verifying_key());
    let responder = ConsensusNode::new(sk_a.clone(), validator_set, validator_pubkeys, 1000);
    (responder, sk_a, addr_a, addr_b)
}

/// Seed blocks and their QC rows into the responder's DB (block_key /
/// qc_key, per sync_test.rs).
fn seed_responder_db(node: &ConsensusNode, blocks: &[Block], qcs: &[QC]) {
    assert_eq!(blocks.len(), qcs.len(), "fixture bug: one QC per block");
    let mut db = node.db.lock().unwrap();
    for (block, qc) in blocks.iter().zip(qcs) {
        db.put(
            &novai_state::block_key(block.height),
            &encode_block_v1(block).expect("fixture block must encode"),
        )
        .unwrap();
        db.put(
            &novai_state::qc_key(block.height),
            &encode_qc_v1(qc).expect("fixture QC must encode"),
        )
        .unwrap();
    }
}

/// Encoded payload length via the same codec the wire path uses; the wire
/// check compares payload + 2 against the cap (p2p:119-122).
fn payload_len(resp: &BlockResponse) -> usize {
    encode_block_response_v2(resp)
        .expect("codec accepts any count-legal response; only the wire cap rejects")
        .len()
}

/// The full (unbudgeted) response for a seeded range, built independently
/// of build_block_response so preconditions cannot drift with the fix.
fn full_response(responder: Address, blocks: &[Block], qcs: &[QC]) -> BlockResponse {
    BlockResponse {
        responder,
        request_start: blocks.first().map_or(1, |b| b.height),
        request_end: blocks.last().map_or(1, |b| b.height),
        blocks: blocks.to_vec(),
        qcs: qcs.iter().cloned().map(Some).collect(),
    }
}

/// Under-budget invariance (fixplan section (d)) and fixture soundness.
/// PASSES on HEAD and must keep passing after the fix: a range already
/// under the cap is served in full, QC-paired, and wire-encodes; the
/// budget must never truncate an already-legal response.
#[test]
fn under_budget_range_is_served_in_full_and_unchanged() {
    let (responder, sk_a, addr_a, addr_b) = two_validator_world();
    let blocks = fat_chain(3, 0, 0);
    let qcs: Vec<QC> = blocks
        .iter()
        .map(|b| certifying_qc(&sk_a, addr_a, b))
        .collect();
    seed_responder_db(&responder, &blocks, &qcs);

    let full = payload_len(&full_response(addr_a, &blocks, &qcs));
    assert!(
        full + 2 <= HARD_WIRE_CAP / 2,
        "precondition: this range sits under even the soft budget (got {full})"
    );

    let response = responder.build_block_response(&BlockRequest {
        requester: addr_b,
        start_height: 1,
        end_height: 3,
    });

    assert_eq!(
        response.blocks.len(),
        3,
        "an under-budget range is served in full; the budget must not \
         truncate already-legal responses"
    );
    for (i, block) in response.blocks.iter().enumerate() {
        assert_eq!(block.height, 1 + i as u64, "contiguous from the start");
    }
    assert_eq!(response.qcs.len(), 3, "one qcs entry per served block");
    assert!(
        response.qcs.iter().all(Option::is_some),
        "every seeded QC row is served"
    );
    let wire = encode_wire_message(&NetworkMessage::BlockResponse(response));
    assert!(
        wire.is_ok(),
        "an under-cap response must wire-encode: {:?}",
        wire.err()
    );
}

/// T3, the floor (GREEN companion; on HEAD this fails through the same
/// over-cap mechanism as T1, see the file header).
///
/// A single pair over the SOFT budget but under the HARD cap must still be
/// served, alone: heavy regions advance one pair at a time instead of
/// starving. The beyond-hard-cap pair is F3 scope and absent here; the
/// preconditions pin this floor pair UNDER the hard cap.
#[test]
fn first_pair_over_soft_budget_is_still_served() {
    let (responder, sk_a, addr_a, addr_b) = two_validator_world();
    let blocks = fat_chain(3, 12, 120_000);
    let qcs: Vec<QC> = blocks
        .iter()
        .map(|b| certifying_qc(&sk_a, addr_a, b))
        .collect();
    seed_responder_db(&responder, &blocks, &qcs);

    // Fix-agnostic preconditions on the seeded geometry.
    let one_pair = payload_len(&full_response(addr_a, &blocks[..1], &qcs[..1]));
    assert!(
        one_pair > HARD_WIRE_CAP / 2,
        "precondition: a single pair must exceed the soft budget wire_cap/2 (got {one_pair})"
    );
    assert!(
        one_pair + 2 <= HARD_WIRE_CAP,
        "precondition: the floor pair must fit under the HARD cap (got {one_pair}); \
         beyond-hard-cap pairs are F3 scope, not exercised here"
    );
    let full = payload_len(&full_response(addr_a, &blocks, &qcs));
    assert!(
        full + 2 > HARD_WIRE_CAP,
        "precondition: serving all three pairs must exceed the wire cap (got {full})"
    );

    let response = responder.build_block_response(&BlockRequest {
        requester: addr_b,
        start_height: 1,
        end_height: 3,
    });
    let served = response.blocks.len();
    let first_height = response.blocks.first().map(|b| b.height);
    let first_qc_present = response.qcs.first().map(Option::is_some);

    let wire = encode_wire_message(&NetworkMessage::BlockResponse(response));
    assert!(
        wire.is_ok(),
        "FLOOR (F2 green companion): {served} pairs were packed and the response \
         cannot encode ({:?}). The budget must cut AFTER the first pair: a pair \
         over the soft budget but under the hard cap is served alone (floor of \
         one pair), so heavy regions still advance one block-pair at a time.",
        wire.err()
    );
    assert!(served >= 1, "the floor pair must be served, not dropped");
    assert_eq!(first_height, Some(1), "the floor pair is the requested start");
    assert_eq!(
        first_qc_present,
        Some(true),
        "the floor pair keeps its QC; shedding QCs to fit the budget would \
         leave the prefix uncertified and unable to advance the requester \
         (sync_test.rs sync_rejects_uncertified_block)"
    );
    assert!(
        served < 3,
        "the budget must engage: serving all three pairs cannot encode under \
         the current cap and would exceed any soft budget at or below it"
    );
}

/// T3b, QC bytes count against the budget (GREEN companion; new coverage,
/// no existing T-ID; on HEAD this fails through the same over-cap
/// mechanism as T1, see the file header).
///
/// Geometry chosen to discriminate: block bytes total ~51 KB (far under
/// any budget) while the QC trailers carry ~3.1 MB. A fix that measures
/// only block bytes packs all five pairs and still emits an over-cap
/// response, so THIS test stays failing against exactly that buggy fix;
/// it passes only when the encoded PAIR (block + flag byte + QC) is
/// measured, per the fixplan's encode-and-measure design. The block push
/// currently precedes the QC load (consensus_node.rs:875/:879 vs
/// :885-896), which is the ordering trap this guards.
#[test]
fn qc_bytes_count_against_the_byte_budget() {
    let (responder, _sk_a, addr_a, addr_b) = two_validator_world();
    let blocks = fat_chain(5, 1, 10_000);
    // Vote-stuffed QCs: 4_300 votes x ~146 bytes each (MIN_VOTE_BYTES,
    // consensus_types/codec.rs:71-72), under MAX_VOTES_PER_QC (11_000,
    // codec.rs:65). Voter addresses are DISTINCT (encode_qc_v1 rejects
    // duplicate voters); signatures are junk: the responder serves QC rows
    // opaquely and nothing verifies them on this path.
    let qcs: Vec<QC> = blocks
        .iter()
        .map(|b| {
            let block_hash = novai_consensus_types::block_hash(b);
            let votes: Vec<Vote> = (0..4_300u64)
                .map(|i| {
                    let mut voter = [0x33u8; 32];
                    voter[..8].copy_from_slice(&i.to_be_bytes());
                    Vote {
                        height: b.height,
                        round: 0,
                        block_hash,
                        voter,
                        signature: [0x44; 64],
                        ai_signal_commitment: None,
                    }
                })
                .collect();
            QC {
                height: b.height,
                round: 0,
                block_hash,
                votes,
            }
        })
        .collect();
    seed_responder_db(&responder, &blocks, &qcs);

    // Fix-agnostic preconditions: blocks alone are tiny, pairs are not.
    let blocks_only = payload_len(&BlockResponse {
        responder: addr_a,
        request_start: 1,
        request_end: 5,
        blocks: blocks.clone(),
        qcs: vec![None; 5],
    });
    assert!(
        blocks_only < HARD_WIRE_CAP / 2,
        "precondition: block bytes alone must sit far under the soft budget \
         (got {blocks_only}); only the QC trailers make this range heavy"
    );
    let full = payload_len(&full_response(addr_a, &blocks, &qcs));
    assert!(
        full + 2 > HARD_WIRE_CAP,
        "precondition: serving all five pairs must exceed the wire cap (got {full})"
    );

    let response = responder.build_block_response(&BlockRequest {
        requester: addr_b,
        start_height: 1,
        end_height: 5,
    });
    let served = response.blocks.len();
    let first_height = response.blocks.first().map(|b| b.height);
    let qc_slots = response.qcs.len();
    let all_qcs_present = response.qcs.iter().all(Option::is_some);

    let wire = encode_wire_message(&NetworkMessage::BlockResponse(response));
    assert!(
        wire.is_ok(),
        "QC BYTES NOT BUDGETED (F2 green companion): {served} pairs were packed \
         and the response cannot encode ({:?}). Block bytes total ~{blocks_only} \
         and fit any budget; the QC trailers are what breach the cap. The budget \
         must measure the encoded PAIR (block + flag byte + QC), not the block \
         alone.",
        wire.err()
    );
    assert!(served > 0, "the budgeted response must not be empty");
    assert_eq!(first_height, Some(1), "prefix must start at the requested height");
    assert_eq!(qc_slots, served, "one qcs entry per served block");
    assert!(
        all_qcs_present,
        "every served block keeps its QC; a fix must not shed QCs to fit the \
         budget (uncertified blocks cannot advance the requester)"
    );
}

/// Byte-accounting drift pin (step 3 review item 1). The compile-time
/// assert in consensus_node.rs pins the budget arithmetic; this test
/// closes the remaining loop by proving the accounting FORMULA matches
/// the real encoder: RESPONSE_HEADER_BYTES must equal the true
/// encode_block_response_v2 header cost, and each served pair must cost
/// exactly block_bytes + 1 flag byte + qc_bytes (0 for a None slot). A
/// future codec layout change fails this equality loudly instead of
/// letting the responder miscount silently. The range sits under budget
/// so the full response, with BOTH has_qc arms exercised, is compared.
#[test]
fn byte_accounting_matches_the_real_encoder() {
    let (responder, sk_a, addr_a, addr_b) = two_validator_world();
    let blocks = fat_chain(4, 2, 5_000);
    let qcs: Vec<QC> = blocks
        .iter()
        .map(|b| certifying_qc(&sk_a, addr_a, b))
        .collect();
    // Seed QC rows for heights 1, 2, and 4 only; height 3 is served with
    // a faithful None so the flag-only arm is in the measured response.
    {
        let mut db = responder.db.lock().unwrap();
        for (block, qc) in blocks.iter().zip(&qcs) {
            db.put(
                &novai_state::block_key(block.height),
                &encode_block_v1(block).expect("fixture block must encode"),
            )
            .unwrap();
            if block.height != 3 {
                db.put(
                    &novai_state::qc_key(block.height),
                    &encode_qc_v1(qc).expect("fixture QC must encode"),
                )
                .unwrap();
            }
        }
    }

    let response = responder.build_block_response(&BlockRequest {
        requester: addr_b,
        start_height: 1,
        end_height: 4,
    });
    assert_eq!(response.blocks.len(), 4, "under-budget range is served in full");
    assert!(
        response.qcs[0].is_some() && response.qcs[2].is_none(),
        "both has_qc arms must be present in the measured response"
    );

    let formula: usize = RESPONSE_HEADER_BYTES
        + response
            .blocks
            .iter()
            .zip(&response.qcs)
            .map(|(block, qc)| {
                encode_block_v1(block).expect("served block must encode").len()
                    + 1
                    + qc.as_ref().map_or(0, |qc| {
                        encode_qc_v1(qc).expect("served QC must encode").len()
                    })
            })
            .sum::<usize>();
    let real = encode_block_response_v2(&response)
        .expect("served response must encode")
        .len();
    assert_eq!(
        formula, real,
        "the responder's byte accounting (RESPONSE_HEADER_BYTES + per pair \
         block bytes + has_qc flag byte + QC bytes) must equal the real \
         encoded payload length exactly; if the codec layout changes, this \
         must fail loudly, never miscount silently"
    );
}
