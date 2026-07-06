//! Gate F2 RED tests: the block-sync responder has no byte budget.
//!
//! Anchor: HEAD 5b9225a. Spec: docs/gate-syncbudget-fixplan.md section (c)
//! "Responder byte budget (P2 fix)" and its RED-test plan, with line pins
//! re-verified against HEAD in docs/gate-f2-diagnosis.md. T-series
//! mapping: this file is T1 (over-cap starvation input, exists-today
//! failure) and T2 (byte-capped partial advances the requester). The
//! green companions T3 (floor) and T3b (QC bytes in the measurement) live
//! in gate_sync_budget_green.rs, mirroring the F1 split
//! (gate_sync_spin_red.rs + gate_sync_backoff_green.rs).
//!
//! The hole: `build_block_response` (consensus_node.rs:855-910) bounds a
//! response by COUNT only (the SYNC_CHUNK_SIZE clamp at :859-862); the
//! assembly loop (:873-898) does zero byte accounting. A tx-heavy range
//! therefore assembles a response whose encoding exceeds MAX_WIRE_MSG_BYTES
//! (2 MiB, p2p/src/lib.rs:23). `encode_wire_message` rejects it
//! (p2p:120-122), `PeerManager::broadcast` pre-encodes once (p2p:299-300)
//! so NOTHING reaches any peer, the responder logs ERROR and continues
//! (consensus_node.rs:2133-2135), and the requester times out at 5s
//! (main.rs:1644-1651), records a strike (main.rs:1663-1665), backs off,
//! and re-issues the IDENTICAL range from committed+1 forever
//! (consensus_node.rs:2281-2284; strikes reset only on commit progress,
//! :2228-2235, which never comes). F1 made that strand loud and
//! rate-limited; only a responder byte budget makes it progress.
//!
//! RED/GREEN contract: both tests assert the POST-FIX behavior (a
//! wire-encodable response that still serves a useful, QC-paired prefix;
//! for T2, one that the partial-tolerant requester commits and re-chains
//! from). On HEAD each fails at the wire-encode gate with the
//! MessageTooLarge byte count visible, which is the over-cap response the
//! fix removes. Both bodies must flip GREEN under the fixplan design
//! (soft budget = wire cap / 2, encode-and-measure the pair, floor of one
//! pair) with NO edit to this file.
//!
//! Preconditions are fix-agnostic: I compute them from the SEEDED data via
//! the same codec the wire path uses (encode_block_response_v2,
//! consensus_types/codec.rs:709-748), never from build_block_response
//! output, so they hold before and after the fix. The 2 MiB cap literal is
//! mirrored below for those preconditions only (p2p:23 is private); the
//! RED/GREEN gate itself is `encode_wire_message`, which enforces the real
//! constant.

use ed25519_dalek::SigningKey;
use novai_consensus_types::codec::{encode_block_response_v2, encode_block_v1, encode_qc_v1};
use novai_consensus_types::{Block, BlockRequest, BlockResponse, Vote, QC};
use novai_crypto::address_from_pubkey;
use novai_node::consensus_node::{ConsensusNode, SYNC_CHUNK_SIZE};
use novai_p2p::{encode_wire_message, read_wire_message, NetworkMessage};
use novai_state::Kv;
use novai_types::{Address, TxV1, TxVersion};
use std::collections::HashMap;

/// Mirror of the private MAX_WIRE_MSG_BYTES (p2p/src/lib.rs:23), used ONLY
/// for fix-agnostic preconditions on seeded data. Behavior assertions go
/// through `encode_wire_message`, which enforces the real constant.
const HARD_WIRE_CAP: usize = 2 * 1024 * 1024;

/// Every block in these fixtures claims the same state root: the harness
/// registers no commit callback (consensus_node.rs:376), so committed
/// blocks are never executed and the requester's seeded SMT root stays
/// constant, exactly like the empty-block fixtures in sync_test.rs.
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

/// A single-vote QC certifying `block` (quorum is 1 for a 2-validator set:
/// f = (n-1)/3 = 0, quorum = 2f+1 = 1, consensus_node.rs:1147-1148).
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

/// A syntactically valid transaction carrying `payload_len` opaque bytes.
/// Signature and keys are junk: the responder serves blocks opaquely
/// (consensus_node.rs:873-898) and the harness never executes them, so
/// only the ENCODED SIZE matters. Payloads stay under MAX_TX_SIZE (128
/// KiB, types/src/lib.rs:19) so the wire round trip in T2 decodes cleanly.
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

/// A parent-linked chain of `count` blocks from height 1, each carrying
/// `txs_per_block` transactions of `payload_len` bytes.
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

/// Two-validator world with deterministic keys (per gate_sync_spin_red.rs):
/// node A is the responder under test, sk_a signs certifying QCs, addr_b is
/// the requesting peer identity.
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

/// Seed blocks and their QC rows into the responder's DB, the exact rows
/// `build_block_response` reads (block_key / qc_key, per sync_test.rs).
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

/// Encoded payload length of a response, via the same codec the wire path
/// uses. The wire check compares payload + 2 against the cap (p2p:119-122).
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

/// T1 (RED today, GREEN after the F2 fix): the starvation input exists.
///
/// 20 blocks of ~120 KB of txs each assemble into a ~2.4 MB response;
/// encode_wire_message rejects it (MessageTooLarge, p2p:120-122) and the
/// requester starves. After the fix the response must wire-encode and
/// still serve a non-empty contiguous QC-paired prefix from the requested
/// start.
#[test]
fn tx_heavy_range_must_produce_a_wire_encodable_response() {
    let (responder, sk_a, addr_a, addr_b) = two_validator_world();
    let blocks = fat_chain(20, 1, 120_000);
    let qcs: Vec<QC> = blocks
        .iter()
        .map(|b| certifying_qc(&sk_a, addr_a, b))
        .collect();
    seed_responder_db(&responder, &blocks, &qcs);

    // Fix-agnostic precondition on the SEEDED data: this range really is
    // over-cap when served whole (payload + 2 mirrors p2p:119-120).
    let full = payload_len(&full_response(addr_a, &blocks, &qcs));
    assert!(
        full + 2 > HARD_WIRE_CAP,
        "precondition: seeded 20-block range must exceed the wire cap (got {full} bytes)"
    );

    let response = responder.build_block_response(&BlockRequest {
        requester: addr_b,
        start_height: 1,
        end_height: 20,
    });
    let served = response.blocks.len();
    let qc_slots = response.qcs.len();
    let heights: Vec<u64> = response.blocks.iter().map(|b| b.height).collect();
    let contiguous_from_one = heights
        .iter()
        .enumerate()
        .all(|(i, h)| *h == 1 + i as u64);

    let wire = encode_wire_message(&NetworkMessage::BlockResponse(response));
    assert!(
        wire.is_ok(),
        "RESPONDER BYTE HOLE (F2, RED): build_block_response served {served} blocks \
         whose encoding cannot traverse the wire: {:?}. The only bound is the \
         SYNC_CHUNK_SIZE count clamp (consensus_node.rs:859-862); the assembly \
         loop (:873-898) does zero byte accounting, broadcast pre-encodes once \
         (p2p:299-300) so NOTHING reaches any peer, and the requester times out \
         and re-requests the same range forever. The responder must return a \
         byte-budgeted prefix that encodes under MAX_WIRE_MSG_BYTES.",
        wire.err()
    );

    // GREEN contract: a useful, well-formed prefix survived the budget.
    assert!(served > 0, "the budgeted response must not be empty");
    assert!(
        contiguous_from_one,
        "served blocks must be a contiguous prefix from the requested start \
         (got heights {heights:?})"
    );
    assert_eq!(qc_slots, served, "one qcs entry per served block");
}

/// T2 (RED today, GREEN after the F2 fix): a byte-capped partial advances
/// the requester and the chained request continues from the new committed
/// height.
///
/// RED on HEAD: no wire-transmissible response exists for this range at
/// all (encode fails), so the requester can never receive ANY prefix; that
/// is the starvation. GREEN after the fix: the partial round-trips the
/// real codec path (encode_wire_message then read_wire_message), the
/// requester (already partial-tolerant: contiguity check
/// consensus_node.rs:1037-1043, chained request :1274) commits a prefix
/// via the 3-chain rule and re-arms the next request from the NEW
/// committed height + 1 (:2281-2284).
#[test]
fn byte_capped_partial_response_advances_the_requester_and_rechains() {
    let (responder, sk_a, addr_a, addr_b) = two_validator_world();
    let blocks = fat_chain(30, 1, 120_000);
    let qcs: Vec<QC> = blocks
        .iter()
        .map(|b| certifying_qc(&sk_a, addr_a, b))
        .collect();
    seed_responder_db(&responder, &blocks, &qcs);

    let full = payload_len(&full_response(addr_a, &blocks, &qcs));
    assert!(
        full + 2 > HARD_WIRE_CAP,
        "precondition: seeded 30-block range must exceed the wire cap (got {full} bytes)"
    );

    // The requester: fresh node B, committed 0, SMT root seeded to match
    // blocks[0].state_root so C-01 passes (the a2_receiver_fixture idiom,
    // sync_test.rs), gossip already told it the tip is height 30.
    let sk_b = SigningKey::from_bytes(&[2u8; 32]);
    let validator_set = vec![addr_a, addr_b];
    let mut validator_pubkeys = HashMap::new();
    validator_pubkeys.insert(addr_a, sk_a.verifying_key());
    validator_pubkeys.insert(addr_b, sk_b.verifying_key());
    let requester = ConsensusNode::new(sk_b, validator_set, validator_pubkeys, 1000);
    {
        let mut db = requester.db.lock().unwrap();
        db.put(
            novai_state::KEY_SMT_ROOT,
            &novai_state::encode_smt_root_v1(&ROOT),
        )
        .unwrap();
    }
    requester.state.lock().unwrap().highest_qc =
        Some(certifying_qc(&sk_a, addr_a, &blocks[29]));

    // First request arms the pending slot exactly as production does.
    requester.try_request_missing_blocks();
    let armed = requester
        .pending_sync_request
        .lock()
        .unwrap()
        .as_ref()
        .map(|p| (p.start_height, p.end_height));
    assert_eq!(
        armed,
        Some((1, 30)),
        "precondition: a behind requester arms a 1..30 request"
    );

    // The responder serves the range; the partial must survive the REAL
    // wire path (encode + decode), not a hand-built struct.
    let response = responder.build_block_response(&BlockRequest {
        requester: addr_b,
        start_height: 1,
        end_height: 30,
    });
    let wire = match encode_wire_message(&NetworkMessage::BlockResponse(response)) {
        Ok(bytes) => bytes,
        Err(e) => panic!(
            "REQUESTER STARVATION (F2, RED): the responder cannot produce ANY \
             wire-transmissible response for a tx-heavy in-retention range \
             ({e:?}); no partial exists today, so the requester commits \
             nothing, its strikes never reset (consensus_node.rs:2228-2235), \
             and it re-requests 1..30 forever. A byte-budgeted partial prefix \
             must exist and advance the requester."
        ),
    };
    let decoded = match read_wire_message(&mut wire.as_slice()) {
        Ok(NetworkMessage::BlockResponse(resp)) => resp,
        other => panic!("wire round trip must yield a BlockResponse, got {other:?}"),
    };

    requester
        .handle_block_response(decoded)
        .expect("a well-formed partial response must be accepted");

    let committed_after = requester.state.lock().unwrap().committed_height;
    assert!(
        committed_after >= 1,
        "the partial must advance the committable frontier (the 3-chain rule \
         needs QCs through h+2, so the served prefix must cover at least \
         heights 1..=3); committed stayed at {committed_after}"
    );

    let rearmed = requester
        .pending_sync_request
        .lock()
        .unwrap()
        .as_ref()
        .map(|p| (p.start_height, p.end_height));
    let expected_end = std::cmp::min(committed_after + SYNC_CHUNK_SIZE, 30);
    assert_eq!(
        rearmed,
        Some((committed_after + 1, expected_end)),
        "the chained request must continue from the NEW committed height + 1 \
         (consensus_node.rs:1274 into :2281-2284)"
    );
}
