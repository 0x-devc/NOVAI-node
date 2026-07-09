//! Gate F3 green companions: proposer guard and Phase A emission bound.
//!
//! Anchor: HEAD 2ed6e93. Spec: docs/gate-f3-diagnosis.md sections 4, 7, 11
//! and docs/gate-syncbudget-fixplan.md section (d) (the proposer-size RED
//! sketch, :163-169). RED companions (T7: receive cap, stranded pair,
//! frontier sizing) live in gate_f3_wire_cap_red.rs, mirroring the F2
//! split (gate_sync_budget_red.rs + gate_sync_budget_green.rs).
//!
//! The two step-1 tests:
//! - The proposer-guard window test FAILS on HEAD (red in character): tx
//!   selection packs to MAX_BLOCK_SIZE with no allowance for the envelope
//!   (consensus/src/lib.rs:258), so a proposal whose tx bytes land within
//!   the overhead window below the cap assembles legally and then dies at
//!   encode inside broadcast (p2p:122 via :302 from consensus_node.rs:1560),
//!   after last_proposed is irreversibly set (consensus/src/lib.rs:305).
//!   It flips GREEN with no edit when guard Layer 1 (the wire-derived
//!   selection budget, diagnosis 4.3) lands.
//! - The Phase A emission pin PASSES on HEAD and must KEEP passing after
//!   the fix: the default send path never emits a frame the old binary
//!   would not (diagnosis section 7, the mixed-fleet safety proof). If the
//!   fix breaks this test, the two-phase deploy is unsafe.
//!
//! STEP 2 CONTRACT, ENACTED (amended per the approved diagnosis section
//! 12): the five runtime send-cap tests at the end of this file were
//! specified as a comment contract in step 1, because the
//! --wire-send-cap-bytes surface (locked decision 2) did not exist at
//! 2ed6e93 and a test file must compile on HEAD for the RED file's
//! failures to be demonstrable. Step 2 introduced the surface
//! (ConsensusNode::set_wire_send_cap / wire_send_cap, backed by the ONE
//! PeerManager value the encoder enforces) and these tests now run
//! against it:
//!   1. envelope_backstop_refuses_oversized_before_broadcast (guard
//!      Layer 2, diagnosis 4.3)
//!   2. guard_tracks_runtime_send_cap_phase_b_non_binding (locked
//!      decision 2, diagnosis 11.2)
//!   3. phase_b_send_cap_encodes_the_stranded_pair (the send half of the
//!      stranded-pair fix, diagnosis sections 6 and 12.2)
//!   4. floor_serves_three_full_pairs_at_phase_b (the 12.3 hard
//!      requirement at the codec ceiling)
//!   5. budget_derives_from_runtime_send_cap (the 12.3 soft-budget rule)

use ed25519_dalek::SigningKey;
use mempool::{NonceProvider, TxMempool};
use novai_consensus_types::codec::{encode_block_response_v2, encode_block_v1, encode_qc_v1};
use novai_consensus_types::{Block, BlockRequest, BlockResponse, Proposal, QC, SignedProposal, Vote};
use novai_crypto::{address_from_pubkey, sign_tx_v1};
use novai_node::consensus_node::{
    response_byte_budget, validate_wire_send_cap, ConsensusNode, MAX_PAIR_BYTES,
    RESPONSE_BYTE_BUDGET, RESPONSE_HEADER_BYTES,
};
use novai_p2p::{
    encode_wire_message, encode_wire_message_with_cap, read_wire_message, NetworkMessage,
    MAX_RECV_WIRE_MSG_BYTES, MAX_WIRE_MSG_BYTES,
};
use novai_state::Kv;
use novai_types::{Address, TxV1, TxVersion};
use std::collections::HashMap;

/// The Phase B send cap: the full receive cap (diagnosis 12.2).
const PHASE_B_CAP: u32 = 16 * 1024 * 1024;

/// Mirror of MAX_WIRE_MSG_BYTES (p2p/src/lib.rs:25), for fix-agnostic
/// preconditions; behavior assertions go through the real functions.
const HARD_WIRE_CAP: usize = 2 * 1024 * 1024;

/// Mirror of MAX_BLOCK_SIZE (types/src/lib.rs:22). Same numeric value as
/// the wire cap; kept as a distinct named mirror because the collision of
/// the two constants is exactly what arms the proposer window
/// (diagnosis 4.1).
const BLOCK_CAP: usize = 2 * 1024 * 1024;

/// The proposal envelope overhead for a height-1 proposal, mirrored for
/// window preconditions only (behavior asserts go through
/// try_propose_block): 97-byte SignedProposal wrapper (tag + proposer +
/// signature, codec.rs:269-277) + 85-byte block header (codec.rs:88) +
/// 53-byte genesis justify QC (empty votes; consensus_node.rs:1511-1517,
/// QC header codec.rs:204-213) + 2 wire envelope bytes counted in the
/// length check (p2p:121).
const HEIGHT1_ENVELOPE_OVERHEAD: usize = 97 + 85 + 53 + 2;

/// All fixture senders expect nonce 0.
struct ZeroNonces;
impl NonceProvider for ZeroNonces {
    fn expected_nonce(&self, _from: &Address) -> u64 {
        0
    }
}

/// A properly signed transaction from a deterministic per-sender key
/// (mempool insert verifies pubkey, address, and signature,
/// mempool/src/lib.rs:318-330), carrying `payload_len` opaque bytes.
/// Encoded size is TX_V1_OVERHEAD (149, codec/src/lib.rs:231) + payload.
fn signed_fat_tx(sender_seed: u8, payload_len: usize) -> TxV1 {
    let sk = SigningKey::from_bytes(&[sender_seed; 32]);
    let vk = sk.verifying_key();
    let mut tx = TxV1 {
        version: TxVersion::V1,
        from: address_from_pubkey(&vk),
        pubkey: vk.to_bytes(),
        nonce: 0,
        fee: 1,
        payload: vec![sender_seed; payload_len],
        sig: [0u8; 64],
    };
    sign_tx_v1(&sk, &mut tx).expect("fixture tx must sign");
    tx
}

/// A single-validator world: this node is the leader at every view, so
/// try_propose_block exercises the full assembly + broadcast path
/// deterministically (quorum is 1, the leader self-vote completes it).
fn single_validator_world() -> ConsensusNode {
    let sk = SigningKey::from_bytes(&[9u8; 32]);
    let addr = address_from_pubkey(&sk.verifying_key());
    let mut validator_pubkeys = HashMap::new();
    validator_pubkeys.insert(addr, sk.verifying_key());
    ConsensusNode::new(sk, vec![addr], validator_pubkeys, 1000)
}

/// Guard Layer 1, the selection-budget geometry (fixplan (d) RED sketch;
/// FAILS on HEAD, flips green with the guard, no edit).
///
/// The geometry is chosen to catch a guard that measures only tx bytes and
/// ignores the envelope: every tx here fits the block cap
/// (consensus/src/lib.rs:258 packs all of them on HEAD), but their sum
/// lands INSIDE the envelope-overhead window below the cap
/// (diagnosis 4.1/4.2), so the assembled SignedProposal exceeds the wire
/// cap by less than the overhead. On HEAD: encode fails inside broadcast
/// (p2p:122 via :302), try_propose_block returns Err after last_proposed
/// was already set (consensus/src/lib.rs:305), the round is stalled via
/// AlreadyProposed (:232), and the next leader can rebuild the same
/// oversized block: the repeating liveness failure of diagnosis 4.2.
/// Post-fix: Layer 1 derives the tx budget from the runtime send cap minus
/// the MEASURED envelope (justify QC encoded by the real codec, not
/// estimated; diagnosis 4.3), the unfitting tail returns to the mempool,
/// and the proposal broadcasts.
#[test]
fn window_packed_mempool_must_still_produce_a_broadcastable_proposal() {
    let node = single_validator_world();
    let np = ZeroNonces;
    let mut mp = TxMempool::new(1, 1000);

    // 15 txs at exactly MAX_TX_SIZE encoded (131,072 = 149 + 130,923)
    // plus one at 131,000 encoded: the sum is 2,097,080 bytes, inside the
    // window (over cap - overhead, at or under the block cap). Distinct
    // senders dodge the per-sender fairness cap and pending limit.
    let mut txs: Vec<TxV1> = (0..15)
        .map(|i| signed_fat_tx(100 + i, 131_072 - 149))
        .collect();
    txs.push(signed_fat_tx(115, 131_000 - 149));

    let sum: usize = txs.iter().map(novai_codec::tx_encoded_size).sum();
    assert!(
        sum > HARD_WIRE_CAP - HEIGHT1_ENVELOPE_OVERHEAD,
        "precondition: tx bytes must land inside the envelope window \
         below the cap (sum {sum})"
    );
    assert!(
        sum <= BLOCK_CAP,
        "precondition: every tx must fit the block cap on HEAD so the \
         unguarded selector packs them all (sum {sum})"
    );

    for tx in txs {
        mp.insert(tx, &np).expect("fixture tx must pass admission");
    }
    assert_eq!(mp.len(), 16, "precondition: all 16 txs admitted");

    match node.try_propose_block(&mut mp, &np) {
        Ok(true) => {}
        Ok(false) => panic!(
            "the single validator is always leader; a window-packed mempool \
             must produce a proposal, not skip the round"
        ),
        Err(e) => panic!(
            "PROPOSER WINDOW (F3 guard Layer 1): a window-packed mempool must \
             yield a BROADCASTABLE proposal, got Err({e}). On HEAD tx \
             selection budgets against MAX_BLOCK_SIZE only \
             (consensus/src/lib.rs:258), the envelope adds \
             ~{HEIGHT1_ENVELOPE_OVERHEAD} bytes (diagnosis 4.1), \
             encode_wire_message rejects the SignedProposal (p2p:122 via \
             :302 from consensus_node.rs:1560), nothing reaches any peer, \
             and the round stalls via AlreadyProposed \
             (consensus/src/lib.rs:232, :305). The guard must shrink \
             selection to the runtime send cap minus the measured envelope."
        ),
    }

    // The guard packs a useful prefix and returns the unfitting tail: at
    // least one tx stays out (the window is narrower than the smallest
    // fixture tx) and at least one is packed.
    let remaining = mp.len();
    assert!(
        (1..16).contains(&remaining),
        "the guard must pack a useful prefix and return the unfitting \
         tail to the mempool (got {remaining} of 16 remaining)"
    );
}

/// Phase A emission bound (diagnosis section 7): GREEN on HEAD and it must
/// SURVIVE the fix unchanged. The default send path refuses any frame over
/// 2 MiB and still emits frames at exactly the cap. This is the property
/// that makes receive-first deployment safe: a Phase A binary (receive
/// 16 MiB, send default 2 MiB) emits nothing the old binary would not, so
/// old and Phase A nodes interoperate in any mix. If the fix flips this
/// test, the mixed-fleet safety proof is broken and Phase A must not ship.
#[test]
fn phase_a_send_path_keeps_the_two_mib_emission_bound() {
    // Over the cap by one byte: len = payload + 2 = 2 MiB + 2.
    let over = NetworkMessage::Transaction(vec![0x5a_u8; 2 * 1024 * 1024]);
    assert!(
        encode_wire_message(&over).is_err(),
        "the DEFAULT send cap must refuse frames over 2 MiB, on HEAD and \
         after the fix (Phase A emission bound, diagnosis section 7)"
    );

    // At the cap exactly: len = payload + 2 = 2 MiB. Encodes on both
    // sides of the fix, and the receive path accepts it on both sides
    // (the receive cap only grows).
    let at_cap = NetworkMessage::Transaction(vec![0x5a_u8; 2 * 1024 * 1024 - 2]);
    let wire = encode_wire_message(&at_cap)
        .expect("a frame at exactly the send cap must encode, on HEAD and after the fix");
    assert_eq!(
        wire.len(),
        4 + 2 * 1024 * 1024,
        "wire bytes are the 4-byte length prefix plus the checked length"
    );
    match read_wire_message(&mut wire.as_slice()) {
        Ok(NetworkMessage::Transaction(bytes)) => {
            assert_eq!(bytes.len(), 2 * 1024 * 1024 - 2, "boundary frame round-trips");
        }
        other => panic!("boundary frame must decode as Transaction, got {other:?}"),
    }
}

// =============================================================================
// STEP 2 CONTRACT TESTS, enacted against the runtime send-cap surface.
// =============================================================================

/// An opaque junk-signed tx of exactly `encoded` bytes (149 overhead +
/// payload, codec/src/lib.rs:231). Served/measured only, never verified.
fn opaque_tx(fill: u8, nonce: u64, encoded: usize) -> TxV1 {
    TxV1 {
        version: TxVersion::V1,
        from: [fill; 32],
        pubkey: [fill; 32],
        nonce,
        fee: 0,
        payload: vec![fill; encoded - 149],
        sig: [fill; 64],
    }
}

/// A block whose tx bytes sum to exactly MAX_BLOCK_SIZE (16 txs at the
/// 131,072-byte MAX_TX_SIZE encoding; diagnosis 12.1).
fn max_size_block(height: u64, parent_hash: [u8; 32]) -> Block {
    let txs = (0..16)
        .map(|i| {
            opaque_tx(
                (height as u8).wrapping_mul(31).wrapping_add(i as u8) | 1,
                i as u64,
                131_072,
            )
        })
        .collect();
    Block {
        height,
        round: 0,
        parent_hash,
        state_root: [0xaa; 32],
        txs,
    }
}

/// A QC at the codec ceiling: 11,000 DISTINCT voters (encode_qc_v1
/// rejects duplicates, codec.rs:197-202), each vote at its 178-byte
/// with-signal maximum. Junk-signed: these tests round-trip the wire and
/// the responder serves QC rows opaquely; nothing verifies them here.
fn codec_ceiling_qc(block: &Block) -> QC {
    let block_hash = novai_consensus_types::block_hash(block);
    let votes = (0..11_000u32)
        .map(|i| {
            let mut voter = [0u8; 32];
            voter[..4].copy_from_slice(&i.to_be_bytes());
            Vote {
                height: block.height,
                round: block.round,
                block_hash,
                voter,
                signature: [0xbb; 64],
                ai_signal_commitment: Some([0xcc; 32]),
            }
        })
        .collect();
    QC {
        height: block.height,
        round: block.round,
        block_hash,
        votes,
    }
}

/// A SignedProposal wrapping `block` with a genesis justify QC and junk
/// signature: proposal_wire_len measures, it does not verify.
fn junk_signed_proposal(block: Block) -> SignedProposal {
    SignedProposal {
        proposer: [0xaa; 32],
        proposal: Proposal {
            block,
            justify_qc: QC {
                height: 0,
                round: 0,
                block_hash: [0u8; 32],
                votes: vec![],
            },
        },
        signature: [0xdd; 64],
    }
}

/// The window-packed mempool txs from the guard geometry test, as a
/// fixture for the tracking test: 15 txs at MAX_TX_SIZE encoding plus one
/// at 131,000, summing to 2,097,080 bytes, inside the envelope window
/// below the 2 MiB cap and at or under MAX_BLOCK_SIZE.
fn window_packed_txs() -> Vec<TxV1> {
    let mut txs: Vec<TxV1> = (0..15)
        .map(|i| signed_fat_tx(100 + i, 131_072 - 149))
        .collect();
    txs.push(signed_fat_tx(115, 131_000 - 149));
    txs
}

/// Contract test 1, guard Layer 2 (diagnosis 4.3): an assembled envelope
/// over the runtime send cap is refused BEFORE broadcast, loudly, and the
/// measurement matches the real encoder byte for byte.
#[test]
fn envelope_backstop_refuses_oversized_before_broadcast() {
    let node = single_validator_world();

    // Oversized: a MAX_BLOCK_SIZE block plus wrapper exceeds the 2 MiB
    // default cap by the envelope overhead. Built directly: Layer 1 makes
    // this unreachable through the mempool path.
    let oversized = junk_signed_proposal(max_size_block(1, [0u8; 32]));
    let refused = node.proposal_wire_len(&oversized);
    assert!(
        refused.is_err(),
        "an envelope over the send cap must be refused before broadcast"
    );
    assert!(
        refused.unwrap_err().contains("exceeds the send cap"),
        "the refusal must name the violated invariant"
    );

    // Under-cap: the measurement is the exact checked wire length the
    // encoder compares against the cap (the F2 byte-accounting idiom).
    let small = junk_signed_proposal(Block {
        height: 1,
        round: 0,
        parent_hash: [0u8; 32],
        state_root: [0xaa; 32],
        txs: vec![opaque_tx(1, 0, 1_000)],
    });
    let measured = node
        .proposal_wire_len(&small)
        .expect("an under-cap envelope passes the backstop");
    let wire = encode_wire_message(&NetworkMessage::SignedProposal(small))
        .expect("an under-cap envelope encodes");
    assert_eq!(
        measured,
        wire.len() - 4,
        "the backstop measurement must equal the encoder's checked length \
         (wire bytes minus the 4-byte length prefix)"
    );
}

/// Contract test 2, the deploy-safety property (diagnosis 11.2): the
/// guard tracks the ONE runtime send-cap value the encoder enforces, so
/// a Phase B restart flips both atomically. At the 2 MiB default the
/// guard binds (packs less than the window mempool); at the 16 MiB
/// Phase B cap MAX_BLOCK_SIZE binds first and the guard is non-binding
/// (packs everything), and the resulting proposal still broadcasts,
/// proving the encoder saw the same raised cap.
#[test]
fn guard_tracks_runtime_send_cap_phase_b_non_binding() {
    let np = ZeroNonces;

    // Phase A node: default cap. The guard trims the window tail.
    let node_a = single_validator_world();
    assert_eq!(
        node_a.wire_send_cap(),
        MAX_WIRE_MSG_BYTES,
        "the runtime cap defaults to the 2 MiB constant"
    );
    let mut mp_a = TxMempool::new(1, 1000);
    for tx in window_packed_txs() {
        mp_a.insert(tx, &np).expect("fixture tx must pass admission");
    }
    assert_eq!(
        node_a.try_propose_block(&mut mp_a, &np),
        Ok(true),
        "Phase A: the guarded proposal must broadcast"
    );
    assert!(
        (1..16).contains(&mp_a.len()),
        "Phase A: the guard binds at 2 MiB and returns the unfitting tail \
         (got {} remaining)",
        mp_a.len()
    );

    // Phase B node: the one runtime value raised to 16 MiB. Guard and
    // encoder read it through the same PeerManager storage, so this
    // single set call is the whole Phase B flip.
    let node_b = single_validator_world();
    node_b
        .set_wire_send_cap(PHASE_B_CAP)
        .expect("the Phase B cap is a legal configuration");
    assert_eq!(
        node_b.wire_send_cap(),
        PHASE_B_CAP,
        "the guard reads back the raised runtime value"
    );
    let mut mp_b = TxMempool::new(1, 1000);
    for tx in window_packed_txs() {
        mp_b.insert(tx, &np).expect("fixture tx must pass admission");
    }
    assert_eq!(
        node_b.try_propose_block(&mut mp_b, &np),
        Ok(true),
        "Phase B: the proposal must broadcast under the raised cap, \
         proving the ENCODER saw the same runtime value the guard used"
    );
    assert_eq!(
        mp_b.len(),
        0,
        "Phase B: MAX_BLOCK_SIZE binds first and the guard is non-binding; \
         the full window mempool packs"
    );
}

/// Contract test 3, the send half of the stranded-pair fix (diagnosis
/// sections 6 and 12.2): the floor response the red file proves
/// acceptable on the receive side also ENCODES through the runtime-cap
/// send surface at 16 MiB, while the default send path keeps refusing it
/// (the Phase A emission bound). Plus the startup validation pins.
#[test]
fn phase_b_send_cap_encodes_the_stranded_pair() {
    let block = max_size_block(1, [0u8; 32]);
    let block_hash = novai_consensus_types::block_hash(&block);
    let qc = QC {
        height: 1,
        round: 0,
        block_hash,
        votes: vec![Vote {
            height: 1,
            round: 0,
            block_hash,
            voter: [0x11; 32],
            signature: [0x22; 64],
            ai_signal_commitment: None,
        }],
    };
    let response = BlockResponse {
        responder: [0xaa; 32],
        request_start: 1,
        request_end: 1,
        blocks: vec![block],
        qcs: vec![Some(qc)],
    };
    let payload = encode_block_response_v2(&response)
        .expect("codec accepts the pair")
        .len();
    assert!(
        payload + 2 > MAX_WIRE_MSG_BYTES as usize,
        "precondition: the pair is over the 2 MiB default (got {payload})"
    );
    assert!(
        payload + 2 <= PHASE_B_CAP as usize,
        "precondition: the pair fits the Phase B cap (got {payload})"
    );

    let msg = NetworkMessage::BlockResponse(response);
    assert!(
        encode_wire_message(&msg).is_err(),
        "the DEFAULT send path must keep refusing the over-cap pair \
         (Phase A emission bound)"
    );
    let wire = encode_wire_message_with_cap(&msg, PHASE_B_CAP)
        .expect("the Phase B send cap must encode the previously stranded pair");
    match read_wire_message(&mut wire.as_slice()) {
        Ok(NetworkMessage::BlockResponse(resp)) => {
            assert_eq!(resp.blocks.len(), 1, "the pair round-trips whole");
            assert_eq!(
                novai_consensus_types::block_hash(&resp.blocks[0]),
                block_hash,
                "the stranded block round-trips byte-identically"
            );
        }
        other => panic!("wire round trip must yield a BlockResponse, got {other:?}"),
    }

    // Startup validation (diagnosis 12.3, 12.6): the cap may only live in
    // [2 MiB default, 16 MiB receive cap].
    assert!(validate_wire_send_cap(MAX_WIRE_MSG_BYTES).is_ok());
    assert!(validate_wire_send_cap(MAX_RECV_WIRE_MSG_BYTES).is_ok());
    assert!(
        validate_wire_send_cap(MAX_WIRE_MSG_BYTES - 1).is_err(),
        "a cap below the default loses the deployed budget guarantees"
    );
    assert!(
        validate_wire_send_cap(MAX_RECV_WIRE_MSG_BYTES + 1).is_err(),
        "a cap above the receive cap could partition a mixed fleet"
    );
}

/// Contract test 4, the unconditional frontier guarantee at the codec
/// ceiling (fixplan :122, diagnosis 12.3): with the Phase B cap, the
/// responder serves THREE maximal pairs in one response, floor-exempt
/// from the 8 MiB soft budget, and the response encodes and round-trips.
#[test]
fn floor_serves_three_full_pairs_at_phase_b() {
    // The compile-time 3-pair assert, re-derived as a runtime pin: a
    // codec constant change must fail here loudly.
    assert_eq!(
        MAX_PAIR_BYTES,
        (85 + 2 * 1024 * 1024) + 1 + (53 + 11_000 * 178),
        "MAX_PAIR_BYTES must match the diagnosis 12.1 derivation"
    );
    assert!(
        3 * MAX_PAIR_BYTES + RESPONSE_HEADER_BYTES + 2 <= MAX_RECV_WIRE_MSG_BYTES as usize,
        "three full pairs must fit the receive cap (diagnosis 12.2)"
    );

    let responder = single_validator_world();
    responder
        .set_wire_send_cap(PHASE_B_CAP)
        .expect("the Phase B cap is a legal configuration");

    let b1 = max_size_block(1, [0u8; 32]);
    let b2 = max_size_block(2, novai_consensus_types::block_hash(&b1));
    let b3 = max_size_block(3, novai_consensus_types::block_hash(&b2));
    let blocks = [b1, b2, b3];
    {
        let mut db = responder.db.lock().unwrap();
        for block in &blocks {
            db.put(
                &novai_state::block_key(block.height),
                &encode_block_v1(block).expect("fixture block must encode"),
            )
            .unwrap();
            db.put(
                &novai_state::qc_key(block.height),
                &encode_qc_v1(&codec_ceiling_qc(block)).expect("fixture QC must encode"),
            )
            .unwrap();
        }
    }

    let response = responder.build_block_response(&BlockRequest {
        requester: [0xbb; 32],
        start_height: 1,
        end_height: 3,
    });
    let served: Vec<u64> = response.blocks.iter().map(|b| b.height).collect();
    assert_eq!(
        served,
        vec![1, 2, 3],
        "the 3-pair floor must serve three FULL pairs at Phase B despite \
         the soft budget; anything less re-strands the frontier at the \
         codec ceiling (fixplan :122)"
    );
    assert!(
        response.qcs.iter().all(Option::is_some),
        "every served pair keeps its QC"
    );

    let payload = encode_block_response_v2(&response)
        .expect("codec accepts the 3-pair response")
        .len();
    assert_eq!(
        payload, 12_165_930,
        "the served response must sit at the diagnosis 12.1 3-pair maximum"
    );
    assert!(
        payload > response_byte_budget(PHASE_B_CAP),
        "precondition: the floor exemption was exercised (payload over the \
         8 MiB soft budget)"
    );

    let msg = NetworkMessage::BlockResponse(response);
    let wire = encode_wire_message_with_cap(&msg, PHASE_B_CAP)
        .expect("the 3-pair maximum must encode under the Phase B cap");
    match read_wire_message(&mut wire.as_slice()) {
        Ok(NetworkMessage::BlockResponse(resp)) => {
            assert_eq!(resp.blocks.len(), 3, "all three pairs round-trip");
            assert!(
                resp.qcs
                    .iter()
                    .all(|q| q.as_ref().is_some_and(|qc| qc.votes.len() == 11_000)),
                "the ceiling QCs round-trip with their full vote sets"
            );
        }
        other => panic!("wire round trip must yield a BlockResponse, got {other:?}"),
    }
}

/// Contract test 5, the 12.3 soft-budget rule: half the runtime send cap,
/// byte-identical to deployed F2 at the Phase A default, 8 MiB at
/// Phase B, always strictly inside the send frame.
#[test]
fn budget_derives_from_runtime_send_cap() {
    assert_eq!(
        response_byte_budget(MAX_WIRE_MSG_BYTES),
        1_048_576,
        "Phase A soft budget is the deployed F2 value, byte-identical"
    );
    assert_eq!(
        response_byte_budget(MAX_WIRE_MSG_BYTES),
        RESPONSE_BYTE_BUDGET,
        "the runtime rule at the default equals the compile-time constant"
    );
    assert_eq!(
        response_byte_budget(PHASE_B_CAP),
        8_388_608,
        "Phase B soft budget is half the raised cap"
    );
    for cap in [MAX_WIRE_MSG_BYTES, PHASE_B_CAP] {
        assert!(
            response_byte_budget(cap) + RESPONSE_HEADER_BYTES + 2 < cap as usize,
            "the soft budget must sit strictly inside the send frame at \
             cap {cap}"
        );
    }
}
