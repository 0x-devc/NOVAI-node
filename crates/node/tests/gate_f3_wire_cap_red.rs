//! Gate F3 RED tests: the wire cap strands valid over-2-MiB messages.
//!
//! Anchor: HEAD 2ed6e93. Spec: docs/gate-f3-diagnosis.md (sections 1-11
//! plus the approved section 12 amendment) and
//! docs/gate-syncbudget-fixplan.md T7 (:150) plus the frontier definition
//! (:122). T-series mapping: this file is T7 plus the emission-side
//! floor-3 companion (diagnosis 12.8 item 3). The F2 files
//! (gate_sync_budget_red.rs / gate_sync_budget_green.rs, T1-T3b) pin the
//! budget shaping UNDER the 2 MiB cap; this file pins what must cross it
//! and what the responder must serve to keep the frontier moving.
//!
//! The hole: one constant gates both directions of the wire.
//! `read_wire_message` rejects any frame whose length prefix exceeds
//! MAX_WIRE_MSG_BYTES = 2 MiB (p2p/src/lib.rs:145, constant :25), so a
//! valid single (block, QC) pair whose encoding tops 2 MiB can never be
//! received by anyone, and F2 deliberately left that pair stranded
//! (consensus_node.rs:904-907: "a single pair beyond the hard wire cap
//! stays unservable until the F3 cap raise"). The full-load frontier
//! guarantee (fixplan :122) additionally requires one response to carry
//! 3 FULL (block, QC) pairs, whose worst valid encoding is 12,165,932
//! wire bytes (diagnosis 12.1), so the approved fix raises the RECEIVE
//! side to a new MAX_RECV_WIRE_MSG_BYTES = 16 MiB (clears the 3-pair
//! maximum with 27.5 percent headroom, diagnosis 12.2) while the send
//! side stays on the 2 MiB MAX_WIRE_MSG_BYTES default (two-phase deploy,
//! receive first; diagnosis sections 2, 5, 7, 12.6), and replaces the
//! 1-pair floor with a 3-pair floor bounded by the send frame plus a
//! soft budget of half the runtime send cap (diagnosis 12.3).
//!
//! RED/GREEN contract: every test here fails on HEAD at the receive-side
//! check (p2p:145) and must flip GREEN with NO edit to this file once the
//! receive constant lands. The send-side flip (Phase B, runtime
//! --wire-send-cap-bytes) is deliberately NOT asserted here: the default
//! send cap stays 2 MiB forever (mixed-fleet emission bound, diagnosis
//! section 7), so an encode_wire_message success on an over-cap message
//! can never be a flip-unchanged assertion. Where this file touches
//! encode_wire_message it asserts the STABLE behavior (rejection) that
//! must survive the fix. Phase B send assertions live in the step-2
//! contract block of gate_f3_wire_cap_green.rs.
//!
//! Receive-side fixtures are framed by hand against the documented wire
//! layout ([len: u32 be][version: u8][kind: u8][payload], p2p/src/lib.rs:3,
//! encoder :126-130) because no encoder on HEAD can legally produce an
//! over-2-MiB frame; the framing helper is 4 lines and mirrors the format
//! the decoder itself documents. Preconditions on seeded data go through
//! the real codec (encode_block_response_v2), never through
//! build_block_response output, so they hold before and after the fix
//! (the F2 idiom).

use ed25519_dalek::SigningKey;
use novai_consensus_types::codec::{encode_block_response_v2, encode_block_v1, encode_qc_v1};
use novai_consensus_types::{Block, BlockRequest, BlockResponse, Vote, QC};
use novai_crypto::address_from_pubkey;
use novai_node::consensus_node::ConsensusNode;
use novai_p2p::{
    encode_wire_message, encode_wire_message_with_cap, read_wire_message, NetworkMessage,
};
use novai_state::Kv;
use novai_types::{Address, TxV1, TxVersion};
use std::collections::HashMap;

/// Mirror of MAX_WIRE_MSG_BYTES (p2p/src/lib.rs:25), for fix-agnostic
/// preconditions on seeded data. Behavior assertions go through the real
/// read_wire_message / encode_wire_message, which enforce the real
/// constants.
const HARD_WIRE_CAP: usize = 2 * 1024 * 1024;

/// Mirror of the locked F3 receive cap (MAX_RECV_WIRE_MSG_BYTES = 16 MiB,
/// diagnosis 12.2: the worst valid 3-full-pair response is 12,165,932
/// wire bytes, and 16 MiB clears it with 27.5 percent headroom). Used only
/// to pin that fixtures sit INSIDE the raised cap; the accept assertions
/// go through read_wire_message.
const RECV_WIRE_CAP: usize = 16 * 1024 * 1024;

/// Mirror of the F2 soft budget as deployed (RESPONSE_BYTE_BUDGET =
/// MAX_WIRE_MSG_BYTES / 2 = 1 MiB, consensus_node.rs:34), used ONLY for
/// the fix-agnostic precondition of the floor-3 test. Post-fix the budget
/// derives from the runtime send cap (diagnosis 12.3) and evaluates to
/// this same value at the 2 MiB default, so the precondition holds on
/// both sides.
const SOFT_BUDGET: usize = HARD_WIRE_CAP / 2;

/// Every fixture block claims the same state root: the harness registers
/// no commit callback, so committed blocks are never executed and the
/// requester's seeded SMT root stays constant (the F2/sync_test.rs idiom).
const ROOT: [u8; 32] = [0xaa; 32];

/// Frame a payload by hand per the documented wire format
/// ([len: u32 be][version: u8][kind: u8][payload], p2p/src/lib.rs:3;
/// len counts version + kind + payload, encoder :121). Receive-side
/// fixtures only: on HEAD no encoder can produce an over-cap frame.
fn hand_frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let len = (payload.len() as u32) + 2;
    let mut frame = Vec::with_capacity(4 + len as usize);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.push(1); // version
    frame.push(kind);
    frame.extend_from_slice(payload);
    frame
}

/// MessageKind::Transaction wire tag (p2p/src/lib.rs:37).
const KIND_TRANSACTION: u8 = 7;
/// MessageKind::BlockResponse wire tag (p2p/src/lib.rs:36).
const KIND_BLOCK_RESPONSE: u8 = 6;

/// A domain-separated, validly signed vote (mirrors gate_sync_budget_red.rs).
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
/// f = (n-1)/3 = 0, quorum = 2f+1 = 1; the gate_sync_budget_red.rs idiom).
/// Distinct-voter rule note: encode_qc_v1 rejects duplicate voters
/// (codec.rs:197-202); these QCs carry exactly one voter, so the rule is
/// trivially satisfied. The fixture bulk lives in the BLOCK (a valid block
/// reaches MAX_BLOCK_SIZE exactly with 16 max-size txs, diagnosis 11.1),
/// because the requester must VERIFY these QCs to commit, and only the two
/// harness validators have keys.
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

/// A syntactically valid transaction carrying `payload_len` opaque bytes;
/// encoded size is TX_V1_OVERHEAD (149, codec/src/lib.rs:231) + payload.
/// Signature and keys are junk: the responder serves blocks opaquely and
/// the harness never executes them, so only the ENCODED SIZE matters
/// (the F2 fixture idiom).
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

/// Payload length that makes one tx encode to exactly MAX_TX_SIZE
/// (131,072 = 149 + 130,923; types/src/lib.rs:19, codec/src/lib.rs:231).
const MAX_TX_PAYLOAD: usize = 131_072 - 149;

/// A block at `height` whose tx bytes sum to exactly MAX_BLOCK_SIZE
/// (2,097,152 = 16 x 131,072; types/src/lib.rs:22): the largest block
/// verify_block accepts (consensus/src/lib.rs:359-386), per the
/// diagnosis 11.1 bound arithmetic.
fn max_size_block(height: u64, parent_hash: [u8; 32]) -> Block {
    let txs = (0..16)
        .map(|i| {
            fat_tx(
                (height as u8).wrapping_mul(31).wrapping_add(i as u8) | 1,
                i as u64,
                MAX_TX_PAYLOAD,
            )
        })
        .collect();
    Block {
        height,
        round: 0,
        parent_hash,
        state_root: ROOT,
        txs,
    }
}

/// An empty block at `height` (the current fleet's steady state).
fn empty_block(height: u64, parent_hash: [u8; 32]) -> Block {
    Block {
        height,
        round: 0,
        parent_hash,
        state_root: ROOT,
        txs: Vec::new(),
    }
}

/// Two-validator world with deterministic keys (per gate_sync_budget_red.rs):
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
/// build_block_response reads (block_key / qc_key).
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

/// Encoded payload length of a response via the same codec the wire path
/// uses; the wire check compares payload + 2 against the cap (p2p:121-122).
fn payload_len(resp: &BlockResponse) -> usize {
    encode_block_response_v2(resp)
        .expect("codec accepts any count-legal response; only the wire cap rejects")
        .len()
}

/// T7, receive half (RED today, GREEN after the receive-cap raise): a frame
/// between 2 MiB and 16 MiB must be accepted by the receive path.
///
/// On HEAD read_wire_message rejects the length prefix before reading the
/// payload (p2p/src/lib.rs:145 against MAX_WIRE_MSG_BYTES :25) and the
/// inbound loop disconnects the peer (consensus_node.rs:2232-2235): every
/// oversized frame is a guaranteed link flap, the mixed-fleet partition
/// mechanism of diagnosis section 7. After the fix the same frame decodes.
/// The over-16-MiB rejection pinned at the end is STABLE (true on HEAD and
/// post-fix): per diagnosis 12.1 the worst valid message is 12,165,932
/// wire bytes, so nothing above the raised cap may ever be accepted.
#[test]
fn over_two_mib_frame_is_accepted_by_the_receive_path() {
    // A 3 MiB Transaction frame: kind 7 carries raw payload bytes with no
    // codec constraints (p2p/src/lib.rs:117, :201), so this isolates the
    // receive-side length check from any payload decoding.
    let payload = vec![0x5a_u8; 3 * 1024 * 1024];
    let frame = hand_frame(KIND_TRANSACTION, &payload);

    let decoded = read_wire_message(&mut frame.as_slice());
    match decoded {
        Ok(NetworkMessage::Transaction(bytes)) => {
            assert_eq!(
                bytes.len(),
                payload.len(),
                "decoded transaction payload must round-trip intact"
            );
        }
        Ok(other) => panic!("a kind-7 frame must decode as Transaction, got {other:?}"),
        Err(e) => panic!(
            "RECEIVE CAP HOLE (F3, RED): read_wire_message rejected a valid \
             3 MiB frame ({e:?}) at the single receive check \
             (p2p/src/lib.rs:145). One constant gates both directions, so \
             no node can ever accept the over-2-MiB messages the protocol \
             can legally produce (worst valid single message ~3.87 MiB, \
             diagnosis 11.1; worst valid 3-full-pair response 12,165,932 \
             bytes, diagnosis 12.1). The receive side must accept frames \
             up to MAX_RECV_WIRE_MSG_BYTES = 16 MiB while the send side \
             stays on the 2 MiB default."
        ),
    }

    // STABLE pin, both sides of the fix: a frame OVER 16 MiB stays
    // rejected. len = payload + 2 = 16 MiB + 1 here. On HEAD it is over
    // the 2 MiB cap; post-fix it is over the 16 MiB receive cap. No valid
    // message can exceed the 3-full-pair maximum of 12,165,932 bytes
    // (diagnosis 12.1), so acceptance above 16 MiB would be a bug in
    // either world.
    let oversized = vec![0u8; 16 * 1024 * 1024 - 1];
    let frame = hand_frame(KIND_TRANSACTION, &oversized);
    assert!(
        read_wire_message(&mut frame.as_slice()).is_err(),
        "a frame whose length prefix exceeds the receive cap must be \
         rejected before payload allocation, on HEAD and after the fix"
    );
}

/// T7, stranded-pair half (RED today, GREEN after the receive-cap raise):
/// the single (block, QC) pair F2 left unservable must reach the requester
/// and commit.
///
/// F2's floor always serves the first pair regardless of size
/// (consensus_node.rs:970, the !blocks.is_empty() short-circuit) but a
/// pair whose response encodes past 2 MiB cannot traverse the wire: the
/// requester rejects the frame at p2p:145 (and on the send side the
/// responder's broadcast dies at encode, p2p:122 via :302, sending nothing
/// to anyone). Either way the requester re-requests the same range forever
/// and catch-up is dead past that height (diagnosis section 6).
///
/// This test drives BOTH halves end to end, mirroring the real deploy:
/// Phase A, the floor response framed, accepted, decoded, and stored by
/// the requester (with the default send path still refusing it, the
/// emission bound); then Phase B, the responder's runtime cap raised, the
/// SAME re-requested range served as one floor-3 response through the
/// real runtime send surface, and the fat block COMMITTED (diagnosis
/// 12.3, 12.5: Phase A stores the pair, Phase B progresses past it).
#[test]
fn stranded_single_pair_over_the_hard_cap_reaches_and_commits_on_the_requester() {
    let (responder, sk_a, addr_a, addr_b) = two_validator_world();

    // Chain: height 1 is the largest valid block (tx bytes exactly
    // MAX_BLOCK_SIZE, diagnosis 11.1), heights 2..4 are empty so the
    // 3-chain rule (commit h needs the QC chain through h+2, fixplan
    // frontier paragraph) can commit height 1 after a follow-up range.
    let b1 = max_size_block(1, [0u8; 32]);
    let b2 = empty_block(2, novai_consensus_types::block_hash(&b1));
    let b3 = empty_block(3, novai_consensus_types::block_hash(&b2));
    let b4 = empty_block(4, novai_consensus_types::block_hash(&b3));
    let blocks = vec![b1, b2, b3, b4];
    let qcs: Vec<QC> = blocks
        .iter()
        .map(|b| certifying_qc(&sk_a, addr_a, b))
        .collect();
    seed_responder_db(&responder, &blocks, &qcs);

    // The floor response for the stranded pair, from the real responder.
    let response_a = responder.build_block_response(&BlockRequest {
        requester: addr_b,
        start_height: 1,
        end_height: 4,
    });
    assert_eq!(
        response_a.blocks.len(),
        1,
        "precondition (F2 floor, consensus_node.rs:970): the over-budget \
         first pair is served alone"
    );
    assert_eq!(response_a.blocks[0].height, 1, "floor pair is height 1");

    // Fix-agnostic size preconditions via the real codec: the pair is over
    // the 2 MiB hard cap (stranded on HEAD) and inside the 16 MiB receive
    // cap (servable after the raise; diagnosis 12.1/12.2 arithmetic).
    let payload = payload_len(&response_a);
    assert!(
        payload + 2 > HARD_WIRE_CAP,
        "precondition: the floor pair must exceed the 2 MiB wire cap \
         (got {payload} payload bytes)"
    );
    assert!(
        payload + 2 <= RECV_WIRE_CAP,
        "precondition: the floor pair must sit under the 16 MiB receive \
         cap (got {payload} payload bytes)"
    );

    // STABLE pin, both sides of the fix: the DEFAULT send path cannot emit
    // this response. On HEAD that is the strand; post-fix it is the Phase A
    // emission bound (send default stays 2 MiB, diagnosis section 7; the
    // Phase B runtime cap is a flag, never the default). If this ever
    // encodes through the default path, the mixed-fleet safety proof is
    // broken.
    assert!(
        encode_wire_message(&NetworkMessage::BlockResponse(response_a.clone())).is_err(),
        "the 2 MiB default send cap must keep refusing the over-cap \
         response (Phase A emission bound, diagnosis section 7)"
    );

    // THE RED FLIP: the same response framed per the documented wire
    // layout must be ACCEPTED by the receive path.
    let wire_payload =
        encode_block_response_v2(&response_a).expect("codec accepts the floor response");
    let frame = hand_frame(KIND_BLOCK_RESPONSE, &wire_payload);
    let decoded = match read_wire_message(&mut frame.as_slice()) {
        Ok(NetworkMessage::BlockResponse(resp)) => resp,
        Ok(other) => panic!("a kind-6 frame must decode as BlockResponse, got {other:?}"),
        Err(e) => panic!(
            "STRANDED PAIR (F3, RED): the requester rejected the floor \
             response for a maximal valid block at the receive check \
             (p2p/src/lib.rs:145): {e:?}. F2 serves the pair (floor, \
             consensus_node.rs:970) but no peer can accept its {payload} \
             payload bytes under the 2 MiB cap, so catch-up past this \
             height is impossible until MAX_RECV_WIRE_MSG_BYTES = 16 MiB \
             lands (diagnosis sections 6 and 12.2)."
        ),
    };
    assert_eq!(
        novai_consensus_types::block_hash(&decoded.blocks[0]),
        novai_consensus_types::block_hash(&blocks[0]),
        "the fat block must round-trip the wire byte-identically"
    );

    // The requester: fresh node B, committed 0, SMT root seeded so C-01
    // passes, gossip already told it the tip is height 4 (the F2 T2 idiom).
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
        Some(certifying_qc(&sk_a, addr_a, &blocks[3]));

    // Arm the pending slot exactly as production does, then deliver the
    // decoded floor response.
    requester.try_request_missing_blocks();
    requester
        .handle_block_response(decoded)
        .expect("a well-formed floor response must be accepted");

    // Phase B: the responder's send cap is raised by restart (the runtime
    // flag), and the SAME re-requested range now serves the fat pair WITH
    // its followers in one floor-3 response, which is what actually
    // commits past the stranded height (diagnosis 12.3, 12.5: Phase A
    // stores the pair, Phase B progresses past it). The requester
    // re-requests from committed+1 = 1, exactly production's restart, and
    // the response travels the REAL runtime send surface.
    responder
        .set_wire_send_cap(16 * 1024 * 1024)
        .expect("the Phase B cap is a legal configuration");
    let response_b = responder.build_block_response(&BlockRequest {
        requester: addr_b,
        start_height: 1,
        end_height: 4,
    });
    assert_eq!(
        response_b.blocks.len(),
        4,
        "Phase B: the floor serves the fat pair with its followers in one \
         response"
    );
    let wire_b = encode_wire_message_with_cap(
        &NetworkMessage::BlockResponse(response_b),
        16 * 1024 * 1024,
    )
    .expect("the Phase B send cap encodes the response");
    let decoded_b = match read_wire_message(&mut wire_b.as_slice()) {
        Ok(NetworkMessage::BlockResponse(resp)) => resp,
        other => panic!("wire round trip must yield a BlockResponse, got {other:?}"),
    };
    requester
        .handle_block_response(decoded_b)
        .expect("the Phase B response must be accepted");

    let committed_after = requester.state.lock().unwrap().committed_height;
    assert!(
        committed_after >= 1,
        "the previously stranded maximal block must COMMIT once its 3-chain \
         QCs arrive (committed stayed at {committed_after}); serving the \
         pair is only half the fix, the requester must make progress past it"
    );
}

/// T7, literal shape (RED today, GREEN after the receive-cap raise): the
/// fixplan's frontier-sizing response, three near-cap pairs, fits the
/// raised cap and is accepted by the receive path.
///
/// The fixplan sizes the cap by "at least 3 x (max committed block wire
/// size + max QC size) + envelope" (docs/gate-syncbudget-fixplan.md:137-139)
/// so a minimum-viable-progress response (blocks committed+1..K plus QCs,
/// K at least committed+3) is never wire-impossible. Three maximal pairs
/// encode to about 6.0 MiB here: over the 2 MiB cap, comfortably under
/// the 16 MiB receive cap.
///
/// SCOPE NOTE, resolved by the approved section 12 amendment: this test
/// pins the WIRE capability. The emission side (whether the responder
/// assembles such a frame) is governed by the 12.3 rule, a 3-pair floor
/// bounded by the send frame plus a soft budget of half the runtime send
/// cap, and is pinned by the floor-3 test below at the Phase A scale and
/// by the step-2 contract tests at the Phase B scale
/// (gate_f3_wire_cap_green.rs).
#[test]
fn three_near_cap_pairs_fit_the_raised_cap_and_are_accepted() {
    let (_responder, sk_a, addr_a, _addr_b) = two_validator_world();

    let b1 = max_size_block(1, [0u8; 32]);
    let b2 = max_size_block(2, novai_consensus_types::block_hash(&b1));
    let b3 = max_size_block(3, novai_consensus_types::block_hash(&b2));
    let blocks = vec![b1, b2, b3];
    let qcs: Vec<QC> = blocks
        .iter()
        .map(|b| certifying_qc(&sk_a, addr_a, b))
        .collect();

    let response = BlockResponse {
        responder: addr_a,
        request_start: 1,
        request_end: 3,
        blocks: blocks.clone(),
        qcs: qcs.iter().cloned().map(Some).collect(),
    };

    // Fix-agnostic sizing preconditions (the T7 arithmetic pin): over the
    // 2 MiB cap, under the 16 MiB receive cap.
    let payload = payload_len(&response);
    assert!(
        payload + 2 > HARD_WIRE_CAP,
        "precondition: three near-cap pairs must exceed the 2 MiB cap \
         (got {payload} payload bytes)"
    );
    assert!(
        payload + 2 <= RECV_WIRE_CAP,
        "precondition (diagnosis 12.1 sizing): three near-cap pairs plus \
         envelope must fit the 16 MiB receive cap \
         (got {payload} payload bytes)"
    );

    let wire_payload = encode_block_response_v2(&response).expect("codec accepts the response");
    let frame = hand_frame(KIND_BLOCK_RESPONSE, &wire_payload);
    match read_wire_message(&mut frame.as_slice()) {
        Ok(NetworkMessage::BlockResponse(resp)) => {
            let heights: Vec<u64> = resp.blocks.iter().map(|b| b.height).collect();
            assert_eq!(
                heights,
                vec![1, 2, 3],
                "the three-pair frontier response must decode intact"
            );
        }
        Ok(other) => panic!("a kind-6 frame must decode as BlockResponse, got {other:?}"),
        Err(e) => panic!(
            "FRONTIER SIZING (F3/T7, RED): the receive path rejected the \
             fixplan's minimum-viable-progress response (three near-cap \
             pairs, {payload} payload bytes): {e:?}. The 3-chain rule needs \
             blocks committed+1..committed+3 with QCs to advance the \
             frontier (fixplan :122), and under the 2 MiB cap that \
             response is wire-impossible for near-cap blocks. It must fit \
             under MAX_RECV_WIRE_MSG_BYTES = 16 MiB (diagnosis 12.2)."
        ),
    }
}

/// A QC at the codec ceiling: MAX_VOTES_PER_QC = 11,000 votes
/// (codec.rs:65), every voter DISTINCT (encode_qc_v1 rejects duplicates,
/// codec.rs:197-202), each vote at its 178-byte maximum (81 unsigned + 64
/// signature + 1 has_signal flag + 32 commitment, codec.rs:154-167).
/// Signatures are junk: this QC exists to round-trip the WIRE at the
/// worst valid size (encode + read only, no requester verification), so
/// only the codec's own well-formedness rules bind. Encoded size is
/// exactly 53 + 11,000 x 178 = 1,958,053 bytes (diagnosis 12.1).
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

/// The cap discriminator (RED today, GREEN only under a cap of at least
/// 16 MiB): three pairs at the AMENDED maximum, block at MAX_BLOCK_SIZE
/// with a max-voter QC each, must be accepted and decode whole.
///
/// This is the only test that pins the 12.2 cap choice against anything
/// smaller: the near-cap frontier test above uses live-fleet single-vote
/// QCs and tops out at about 6.0 MiB, which the SUPERSEDED 8 MiB
/// candidate cap would also accept. This fixture encodes to exactly
/// 12,165,930 payload bytes (12,165,932 checked wire length, diagnosis
/// 12.1): over 8 MiB, under 16 MiB with 27.5 percent headroom. If the fix
/// ships any cap below 16 MiB, this test stays red. The emission half
/// (the responder serving all three despite the soft budget) is the
/// step-2 contract test floor_serves_three_full_pairs_at_phase_b
/// (gate_f3_wire_cap_green.rs), which needs the runtime send-cap API.
#[test]
fn three_maximal_pairs_at_the_codec_ceiling_are_accepted_whole() {
    let b1 = max_size_block(1, [0u8; 32]);
    let b2 = max_size_block(2, novai_consensus_types::block_hash(&b1));
    let b3 = max_size_block(3, novai_consensus_types::block_hash(&b2));
    let blocks = vec![b1, b2, b3];
    let qcs: Vec<QC> = blocks.iter().map(codec_ceiling_qc).collect();

    let response = BlockResponse {
        responder: [0xaa; 32],
        request_start: 1,
        request_end: 3,
        blocks: blocks.clone(),
        qcs: qcs.iter().cloned().map(Some).collect(),
    };

    // The arithmetic pin (diagnosis 12.1, derived not estimated): if this
    // value moves, a codec constant changed and the 12.2 cap table must be
    // re-derived before anything ships.
    let payload = payload_len(&response);
    assert_eq!(
        payload, 12_165_930,
        "the 3-full-pair maximum must match the diagnosis 12.1 derivation \
         (57 + 3 x (2,097,237 + 1 + 1,958,053)); a mismatch means a codec \
         constant moved and the cap choice needs re-derivation"
    );
    assert!(
        payload + 2 > 8 * 1024 * 1024,
        "precondition: the ceiling response must EXCEED the superseded \
         8 MiB candidate cap, or this test cannot discriminate the 12.2 \
         decision (got {payload} payload bytes)"
    );
    assert!(
        payload + 2 <= RECV_WIRE_CAP,
        "precondition: the ceiling response must fit the 16 MiB receive \
         cap (got {payload} payload bytes)"
    );

    let wire_payload = encode_block_response_v2(&response).expect("codec accepts the response");
    let frame = hand_frame(KIND_BLOCK_RESPONSE, &wire_payload);
    match read_wire_message(&mut frame.as_slice()) {
        Ok(NetworkMessage::BlockResponse(resp)) => {
            let heights: Vec<u64> = resp.blocks.iter().map(|b| b.height).collect();
            assert_eq!(heights, vec![1, 2, 3], "all three maximal blocks decode");
            for (i, (seeded, got)) in blocks.iter().zip(&resp.blocks).enumerate() {
                assert_eq!(
                    novai_consensus_types::block_hash(seeded),
                    novai_consensus_types::block_hash(got),
                    "maximal block {} must round-trip byte-identically",
                    i + 1
                );
            }
            assert!(
                resp.qcs
                    .iter()
                    .all(|q| q.as_ref().is_some_and(|qc| qc.votes.len() == 11_000)),
                "all three ceiling QCs must decode with their full vote sets"
            );
        }
        Ok(other) => panic!("a kind-6 frame must decode as BlockResponse, got {other:?}"),
        Err(e) => panic!(
            "CAP DISCRIMINATOR (F3, RED): the receive path rejected the \
             worst VALID 3-full-pair response ({payload} payload bytes): \
             {e:?}. This is the frame the amended frontier guarantee \
             (fixplan :122, diagnosis 12.1) requires the wire to carry at \
             full load, and it exceeds the superseded 8 MiB candidate, so \
             only MAX_RECV_WIRE_MSG_BYTES = 16 MiB turns this green. Any \
             smaller cap re-strands the frontier at the codec ceiling."
        ),
    }
}

/// A block at `height` carrying `tx_count` txs of `tx_encoded` encoded
/// bytes each (payload = encoded - 149, codec/src/lib.rs:231).
fn sized_block(height: u64, parent_hash: [u8; 32], tx_count: usize, tx_encoded: usize) -> Block {
    let txs = (0..tx_count)
        .map(|i| {
            fat_tx(
                (height as u8).wrapping_mul(37).wrapping_add(i as u8) | 1,
                i as u64,
                tx_encoded - 149,
            )
        })
        .collect();
    Block {
        height,
        round: 0,
        parent_hash,
        state_root: ROOT,
        txs,
    }
}

/// Floor-3 emission (RED today, GREEN after the 12.3 responder rule; the
/// diagnosis 12.8 item 3 test at the Phase A scale, no runtime-cap API
/// needed). Three mid-size pairs that together overflow the 1 MiB soft
/// budget but fit the DEFAULT 2 MiB send frame must be served in ONE
/// response.
///
/// This is the emission half of the frontier guarantee (fixplan :122):
/// the requester restarts at committed+1 (consensus_node.rs:2344, :2362)
/// with no cross-response accumulation, so a response that carries fewer
/// than 3 heights re-serves the identical prefix forever. On HEAD the
/// soft budget cuts this range at 2 pairs (RESPONSE_BYTE_BUDGET = 1 MiB,
/// consensus_node.rs:34, cut at :970) and the requester livelocks. Under
/// the 12.3 rule the first min(3, available) pairs are exempt from the
/// soft budget and bounded only by the send frame, so all three are
/// served, and the response still encodes under the UNCHANGED 2 MiB
/// default (asserted below), which keeps the Phase A emission bound
/// intact (diagnosis 12.5).
#[test]
fn three_pairs_over_the_soft_budget_are_served_in_one_response() {
    let (responder, sk_a, addr_a, addr_b) = two_validator_world();

    // Three blocks of 4 x 112,500-byte txs: block 450,085 bytes encoded,
    // pair 450,285 with the single-vote QC. Three pairs plus the 57-byte
    // header sum to 1,350,912 payload bytes: over the 1 MiB soft budget
    // at pair 3, comfortably under the 2 MiB send frame.
    let b1 = sized_block(1, [0u8; 32], 4, 112_500);
    let b2 = sized_block(2, novai_consensus_types::block_hash(&b1), 4, 112_500);
    let b3 = sized_block(3, novai_consensus_types::block_hash(&b2), 4, 112_500);
    let blocks = vec![b1, b2, b3];
    let qcs: Vec<QC> = blocks
        .iter()
        .map(|b| certifying_qc(&sk_a, addr_a, b))
        .collect();
    seed_responder_db(&responder, &blocks, &qcs);

    // Fix-agnostic preconditions via the real codec: the full 3-pair
    // response overflows the soft budget (so the HEAD cut engages) and
    // fits the default send frame (so serving all three never violates
    // the Phase A emission bound).
    let full = BlockResponse {
        responder: addr_a,
        request_start: 1,
        request_end: 3,
        blocks: blocks.clone(),
        qcs: qcs.iter().cloned().map(Some).collect(),
    };
    let payload = payload_len(&full);
    assert!(
        payload > SOFT_BUDGET,
        "precondition: three pairs must overflow the 1 MiB soft budget \
         (got {payload} payload bytes)"
    );
    assert!(
        payload + 2 <= HARD_WIRE_CAP,
        "precondition: three pairs must fit the default 2 MiB send frame \
         (got {payload} payload bytes)"
    );

    let response = responder.build_block_response(&BlockRequest {
        requester: addr_b,
        start_height: 1,
        end_height: 3,
    });
    let served: Vec<u64> = response.blocks.iter().map(|b| b.height).collect();
    assert_eq!(
        served,
        vec![1, 2, 3],
        "FRONTIER FLOOR (F3, RED): the responder served only {} of 3 pairs \
         for a range whose 3 pairs fit the send frame. The 1 MiB soft \
         budget (consensus_node.rs:34) cuts at :970 before the third pair, \
         the requester commits nothing (3-chain needs committed+1..+3, \
         fixplan :122), restarts at committed+1 (consensus_node.rs:2344, \
         :2362), and re-receives the identical prefix forever. The 12.3 \
         rule must serve the first min(3, available) pairs exempt from the \
         soft budget, bounded only by the send frame.",
        served.len()
    );
    assert_eq!(
        response.qcs.iter().filter(|q| q.is_some()).count(),
        3,
        "all three served pairs must keep their QCs"
    );

    // The three-pair response must remain emittable under the UNCHANGED
    // 2 MiB default: floor-3 never licenses an over-cap emission
    // (diagnosis 12.5, Phase A emission bound).
    assert!(
        encode_wire_message(&NetworkMessage::BlockResponse(response)).is_ok(),
        "the floor-3 response for this range must encode under the \
         default send cap"
    );
}
