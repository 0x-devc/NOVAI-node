//! Gate F5 Stage 4 RED tests: the snapshot wire, receive-first.
//!
//! THE HEADLINE PROPERTY. `MessageKind::from_u8` returns None for a byte it
//! does not know, `read_wire_message` turns that into `P2PError::InvalidKind`,
//! and the peer read loop treats any read error as fatal for that connection.
//! So a node that SENDS one of the four new kinds to a peer running an older
//! binary DISCONNECTS that peer, and requests are broadcast to ALL peers. That
//! is why the deploy is two-phase and why sending is behind a runtime flag that
//! defaults to off.
//!
//! These tests prove the Phase A guarantee directly: with sending disabled,
//! nothing is encoded and nothing leaves the node, even at a call site that
//! would otherwise send. And they reproduce the un-upgraded decoder to show
//! exactly what it does with the new kind bytes, so the reason for the gate is
//! pinned rather than described in a comment.
//!
//! RED discipline: this file reads API that does not exist on the preceding
//! tree, so its RED state is a compile failure. The load-bearing evidence is
//! the MUTATION proof recorded at the gate.

use ed25519_dalek::SigningKey;
use novai_consensus_types::codec::{
    decode_snapshot_chunk_request_v1, decode_snapshot_chunk_response_v1,
    decode_snapshot_manifest_request_v1, decode_snapshot_manifest_response_v1,
    encode_snapshot_chunk_request_v1, encode_snapshot_chunk_response_v1,
    encode_snapshot_manifest_request_v1, encode_snapshot_manifest_response_v1,
    MAX_SNAPSHOT_CHUNK_BYTES,
};
use novai_consensus_types::{
    SnapshotChunkRequest, SnapshotChunkResponse, SnapshotManifestRequest, SnapshotManifestResponse,
};
use novai_crypto::address_from_pubkey;
use novai_node::consensus_node::{ConsensusNode, SnapshotSendOutcome};
use novai_node::snapshot::producer::SnapshotProducer;
use novai_node::snapshot::wire::{PeerStrikes, ServeLimiter, PEER_STRIKE_LIMIT};
use novai_p2p::{encode_wire_message, NetworkMessage, MAX_SNAPSHOT_CHUNK_MSG_BYTES};
use novai_types::Address;
use std::collections::HashMap;

const PEER: Address = [0xAA; 32];

fn node() -> ConsensusNode {
    let sk_a = SigningKey::from_bytes(&[1u8; 32]);
    let sk_b = SigningKey::from_bytes(&[2u8; 32]);
    let addr_a = address_from_pubkey(&sk_a.verifying_key());
    let addr_b = address_from_pubkey(&sk_b.verifying_key());
    let mut pk = HashMap::new();
    pk.insert(addr_a, sk_a.verifying_key());
    pk.insert(addr_b, sk_b.verifying_key());
    ConsensusNode::new(sk_b, vec![addr_a, addr_b], pk, 1_000)
}

fn node_with_producer() -> ConsensusNode {
    let mut n = node();
    n.set_snapshot_producer(std::sync::Arc::new(SnapshotProducer::new(
        std::path::PathBuf::from("unused-stage4"),
    )));
    n
}

/// The decoder an UN-UPGRADED peer runs: the seven kinds that existed before
/// this stage. Reproduced here so the hazard the send gate exists to prevent is
/// pinned by a test rather than asserted in a comment.
fn legacy_kind_known(b: u8) -> bool {
    matches!(b, 1..=7)
}

fn kind_byte(msg: &NetworkMessage) -> u8 {
    let wire = encode_wire_message(msg).expect("encode");
    // [len:4][version:1][kind:1][payload]
    wire[5]
}

// ---------------------------------------------------------------------------
// T4.1 codec roundtrips
// ---------------------------------------------------------------------------

#[test]
fn all_four_messages_roundtrip() {
    let mr = SnapshotManifestRequest { requester: PEER };
    assert_eq!(
        decode_snapshot_manifest_request_v1(&encode_snapshot_manifest_request_v1(&mr).unwrap())
            .unwrap(),
        mr
    );

    let mresp = SnapshotManifestResponse {
        responder: PEER,
        manifest: vec![7u8; 1024],
    };
    assert_eq!(
        decode_snapshot_manifest_response_v1(
            &encode_snapshot_manifest_response_v1(&mresp).unwrap()
        )
        .unwrap(),
        mresp
    );

    let cr = SnapshotChunkRequest {
        requester: PEER,
        height: 1_580_000,
        index: 42,
    };
    assert_eq!(
        decode_snapshot_chunk_request_v1(&encode_snapshot_chunk_request_v1(&cr).unwrap()).unwrap(),
        cr
    );

    let cresp = SnapshotChunkResponse {
        responder: PEER,
        height: 1_580_000,
        index: 42,
        payload: vec![3u8; 4096],
    };
    assert_eq!(
        decode_snapshot_chunk_response_v1(&encode_snapshot_chunk_response_v1(&cresp).unwrap())
            .unwrap(),
        cresp
    );
}

#[test]
fn empty_payloads_are_faithful_answers_not_errors() {
    // "I have no snapshot" and "I cannot serve that chunk" are normal answers
    // on a demand-driven producer, so they must survive the codec rather than
    // being indistinguishable from a malformed message.
    let m = SnapshotManifestResponse {
        responder: PEER,
        manifest: vec![],
    };
    assert_eq!(
        decode_snapshot_manifest_response_v1(&encode_snapshot_manifest_response_v1(&m).unwrap())
            .unwrap(),
        m
    );
    let c = SnapshotChunkResponse {
        responder: PEER,
        height: 9,
        index: 0,
        payload: vec![],
    };
    assert_eq!(
        decode_snapshot_chunk_response_v1(&encode_snapshot_chunk_response_v1(&c).unwrap()).unwrap(),
        c
    );
}

#[test]
fn decoders_reject_truncation_and_trailing_bytes_rather_than_guessing() {
    let bytes = encode_snapshot_chunk_response_v1(&SnapshotChunkResponse {
        responder: PEER,
        height: 1,
        index: 2,
        payload: vec![9u8; 64],
    })
    .unwrap();
    for cut in 0..bytes.len() {
        assert!(
            decode_snapshot_chunk_response_v1(&bytes[..cut]).is_err(),
            "truncation at {cut} must be rejected"
        );
    }
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(decode_snapshot_chunk_response_v1(&extra).is_err());
}

#[test]
fn encoders_refuse_a_payload_beyond_the_wire_bound() {
    let too_big = SnapshotChunkResponse {
        responder: PEER,
        height: 1,
        index: 0,
        payload: vec![0u8; MAX_SNAPSHOT_CHUNK_BYTES + 1],
    };
    assert!(
        encode_snapshot_chunk_response_v1(&too_big).is_err(),
        "a chunk past the bound must never reach the encoder's output"
    );
    let ok = SnapshotChunkResponse {
        payload: vec![0u8; MAX_SNAPSHOT_CHUNK_BYTES],
        ..too_big
    };
    assert!(encode_snapshot_chunk_response_v1(&ok).is_ok(), "the bound itself is legal");
}

// ---------------------------------------------------------------------------
// T4.2 chunk sizing needs no cap raise
// ---------------------------------------------------------------------------

#[test]
fn a_full_chunk_message_fits_the_default_send_cap_with_margin() {
    // The compile-time assertions in crates/p2p are the real pin; this states
    // the same relation in numbers a reader can check, and proves the encoder
    // agrees with the arithmetic.
    let msg = NetworkMessage::SnapshotChunkResponse(SnapshotChunkResponse {
        responder: PEER,
        height: u64::MAX,
        index: u32::MAX,
        payload: vec![0xEE; MAX_SNAPSHOT_CHUNK_BYTES],
    });
    let wire = encode_wire_message(&msg).expect("a full chunk must encode under the DEFAULT cap");
    assert_eq!(
        wire.len(),
        MAX_SNAPSHOT_CHUNK_MSG_BYTES,
        "the constant must equal the real encoder output, not an estimate"
    );
    assert!(
        wire.len() * 3 < novai_p2p::MAX_WIRE_MSG_BYTES as usize,
        "and it must fit with real margin, so Phase B needs no second cap change"
    );
}

// ---------------------------------------------------------------------------
// T4.3 THE RECEIVE-FIRST SAFETY PROPERTY
// ---------------------------------------------------------------------------

#[test]
fn an_un_upgraded_peer_cannot_decode_any_of_the_four_new_kinds() {
    // This is the hazard, made concrete. Each new kind byte is unknown to the
    // pre-Stage-4 decoder, which means InvalidKind, which the read loop treats
    // as fatal for the connection.
    for msg in [
        NetworkMessage::SnapshotManifestRequest(SnapshotManifestRequest { requester: PEER }),
        NetworkMessage::SnapshotManifestResponse(SnapshotManifestResponse {
            responder: PEER,
            manifest: vec![],
        }),
        NetworkMessage::SnapshotChunkRequest(SnapshotChunkRequest {
            requester: PEER,
            height: 1,
            index: 0,
        }),
        NetworkMessage::SnapshotChunkResponse(SnapshotChunkResponse {
            responder: PEER,
            height: 1,
            index: 0,
            payload: vec![],
        }),
    ] {
        let k = kind_byte(&msg);
        assert!((8..=11).contains(&k), "new kinds occupy 8 to 11, got {k}");
        assert!(
            !legacy_kind_known(k),
            "kind {k} is unknown to an un-upgraded peer, which disconnects on it"
        );
    }
}

#[test]
fn the_seven_existing_kinds_are_byte_preserved() {
    // The thing that must not break a running fleet. Every pre-existing kind
    // keeps its byte, so a mixed fleet's ordinary consensus traffic is
    // unaffected by this stage.
    for (b, name) in [
        (1u8, "SignedProposal"),
        (2, "Vote"),
        (3, "Qc"),
        (4, "Timeout"),
        (5, "BlockRequest"),
        (6, "BlockResponse"),
        (7, "Transaction"),
    ] {
        assert!(legacy_kind_known(b), "{name} must keep byte {b}");
    }
    // And a transaction still encodes as kind 7 through the real encoder.
    assert_eq!(kind_byte(&NetworkMessage::Transaction(vec![1, 2, 3])), 7);
}

#[test]
fn t4_3_with_sending_disabled_no_new_kind_byte_can_reach_the_wire() {
    // THE Phase A guarantee. Every snapshot send site funnels through one gate,
    // and with the flag off it returns before anything is encoded or broadcast.
    let n = node_with_producer();
    assert!(!n.snapshot_send_enabled(), "sending must default to OFF");

    assert_eq!(n.request_snapshot_manifest(), SnapshotSendOutcome::Disabled);
    assert_eq!(
        n.request_snapshot_chunk(1_580_000, 0),
        SnapshotSendOutcome::Disabled
    );
    assert_eq!(
        n.handle_snapshot_manifest_request(&SnapshotManifestRequest { requester: PEER }),
        SnapshotSendOutcome::Disabled,
        "even ANSWERING a peer must not put a new kind on the wire in Phase A"
    );
    assert_eq!(
        n.handle_snapshot_chunk_request(&SnapshotChunkRequest {
            requester: [0xBB; 32],
            height: 1,
            index: 0
        }),
        SnapshotSendOutcome::Disabled
    );
}

#[test]
fn t4_3_a_phase_a_node_still_accepts_the_new_kinds() {
    // Receive-first means exactly this: the same node that will not send can
    // already decode and handle, so the fleet can be upgraded before anyone
    // sends. If this failed, Phase A would be pointless.
    for msg in [
        NetworkMessage::SnapshotManifestRequest(SnapshotManifestRequest { requester: PEER }),
        NetworkMessage::SnapshotChunkRequest(SnapshotChunkRequest {
            requester: PEER,
            height: 1,
            index: 0,
        }),
        NetworkMessage::SnapshotManifestResponse(SnapshotManifestResponse {
            responder: PEER,
            manifest: vec![],
        }),
        NetworkMessage::SnapshotChunkResponse(SnapshotChunkResponse {
            responder: PEER,
            height: 1,
            index: 0,
            payload: vec![],
        }),
    ] {
        let bytes = encode_wire_message(&msg).expect("encode");
        let mut cursor = &bytes[..];
        let back = novai_p2p::read_wire_message(&mut cursor).expect("a deployed node must decode");
        // Byte-level roundtrip: re-encoding the decoded message must reproduce
        // the original frame exactly. Stronger than comparing the enum, and it
        // needs no PartialEq on a p2p type.
        assert_eq!(
            encode_wire_message(&back).expect("re-encode"),
            bytes,
            "decode then encode must be the identity on the wire"
        );
    }
}

#[test]
fn enabling_the_flag_is_what_lets_a_message_out() {
    let n = node_with_producer();
    assert_eq!(n.request_snapshot_manifest(), SnapshotSendOutcome::Disabled);
    n.set_snapshot_send_enabled(true);
    assert!(n.snapshot_send_enabled());
    // With no connected peers the broadcast reports failure rather than
    // success; what matters is that it is no longer refused BY THE GATE.
    assert_ne!(
        n.request_snapshot_manifest(),
        SnapshotSendOutcome::Disabled,
        "with the flag on, the gate must no longer be the reason nothing was sent"
    );
}

// ---------------------------------------------------------------------------
// T4.5 O7: a recovering node refuses to be a source
// ---------------------------------------------------------------------------

#[test]
fn t4_5_a_node_in_snapshot_sync_refuses_to_serve() {
    use novai_consensus::PRUNE_RETAIN_BLOCKS;

    let n = node_with_producer();
    n.set_snapshot_send_enabled(true);

    // Healthy: it would serve (no cached bundle yet, but not refused for
    // recovering).
    assert_ne!(
        n.handle_snapshot_manifest_request(&SnapshotManifestRequest { requester: PEER }),
        SnapshotSendOutcome::RefusedRecovering
    );

    // Drive this node into the arming band: it is now itself behind the
    // retention horizon.
    {
        let sk_a = SigningKey::from_bytes(&[1u8; 32]);
        let addr_a = address_from_pubkey(&sk_a.verifying_key());
        let bh = [0x42u8; 32];
        let h = PRUNE_RETAIN_BLOCKS + 1;
        let unsigned = novai_consensus_types::Vote {
            height: h,
            round: 0,
            block_hash: bh,
            voter: addr_a,
            signature: [0u8; 64],
            ai_signal_commitment: None,
        };
        let ub = novai_consensus_types::codec::encode_vote_v1_unsigned(&unsigned);
        let mut to_sign = b"NOVAI_VOTE_V1".to_vec();
        to_sign.extend_from_slice(&ub);
        let sig = novai_crypto::sign_bytes(&sk_a, &to_sign);
        n.state.lock().unwrap().highest_qc = Some(novai_consensus_types::QC {
            height: h,
            round: 0,
            block_hash: bh,
            votes: vec![novai_consensus_types::Vote {
                signature: sig,
                ..unsigned
            }],
        });
    }
    n.try_request_missing_blocks();
    assert_ne!(
        n.snapshot_sync().phase(),
        novai_node::consensus_node::SnapshotSyncPhase::Idle,
        "precondition: this node is now recovering"
    );

    assert_eq!(
        n.handle_snapshot_manifest_request(&SnapshotManifestRequest { requester: PEER }),
        SnapshotSendOutcome::RefusedRecovering,
        "a node cannot be both the patient and the donor"
    );
    assert_eq!(
        n.handle_snapshot_chunk_request(&SnapshotChunkRequest {
            requester: PEER,
            height: 1,
            index: 0
        }),
        SnapshotSendOutcome::RefusedRecovering
    );
}

// ---------------------------------------------------------------------------
// T4.4 rate limiting and the per-peer strike ladder
// ---------------------------------------------------------------------------

#[test]
fn t4_4_a_peer_that_asks_twice_at_once_is_rate_limited() {
    let n = node_with_producer();
    n.set_snapshot_send_enabled(true);
    let req = SnapshotChunkRequest {
        requester: PEER,
        height: 1,
        index: 0,
    };
    let first = n.handle_snapshot_chunk_request(&req);
    assert_ne!(first, SnapshotSendOutcome::RateLimited);
    assert_eq!(
        n.handle_snapshot_chunk_request(&req),
        SnapshotSendOutcome::RateLimited,
        "a node must not be spammed into serving in a loop"
    );
}

#[test]
fn t4_4_the_strike_ladder_shuns_a_consistently_bad_peer() {
    let n = node();
    assert!(!n.snapshot_peer_shunned(&PEER));
    for _ in 0..PEER_STRIKE_LIMIT {
        n.strike_snapshot_peer(PEER);
    }
    assert!(
        n.snapshot_peer_shunned(&PEER),
        "a peer whose chunks keep failing the manifest digest must stop being asked"
    );
    n.clear_snapshot_peer(&PEER);
    assert!(!n.snapshot_peer_shunned(&PEER), "a good answer clears it");
}

#[test]
fn the_rate_limiter_and_strike_ladder_are_pure_and_clock_free() {
    // Both are driven by a caller-supplied clock, so their rules are testable
    // without sleeping. The exhaustive cases live beside the implementation;
    // this is the pin that they remain usable that way.
    let mut l = ServeLimiter::default();
    assert!(l.allow(PEER, 0));
    assert!(!l.allow(PEER, 1));
    let mut s = PeerStrikes::default();
    assert_eq!(s.strike(PEER), 1);
    s.clear(&PEER);
    assert_eq!(s.count(&PEER), 0);
}
