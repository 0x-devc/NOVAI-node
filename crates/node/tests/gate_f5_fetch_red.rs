//! Gate F5 Stage 5 RED tests: the fetch loop.
//!
//! The manifest acceptance gates are where a recovering node decides what it
//! will believe, so each is tested for the reason it exists rather than for
//! coverage. Gate 2, the quorum QC, is the only trust decision; everything
//! after it is integrity. Gate 2 runs BEFORE any chunk is requested, so a
//! hostile peer cannot make a node spend bandwidth on state it would never
//! keep, and the ordering is pinned below.
//!
//! The end-to-end shape here is: produce a real bundle from a real audited
//! copy, encode its manifest, feed it through the gates, feed the real chunks
//! through the digest check, reassemble, and materialise. That is the whole
//! receive path except the wire, which Stage 4 owns.

#[path = "a0_common/mod.rs"]
mod a0_common;

use a0_common::{build_fixture, dev_signing_keys, run_a0, FixtureSpec, TmpDir};
use novai_node::snapshot::bundle::{encode_manifest_v1, SnapshotManifest};
use novai_node::snapshot::fetch::{
    ChunkVerdict, FetchContext, ManifestReject, SnapshotFetch, FRESHNESS_MARGIN_BLOCKS,
};
use novai_node::snapshot::produce::build_bundle;
use novai_node::snapshot::stage::materialize;
use novai_node::snapshot::valset::{dev_valset, quorum};

fn ctx_at(committed: u64, frontier: u64) -> (Vec<(novai_types::Address, ed25519_dalek::VerifyingKey)>, u64, u64) {
    (dev_valset(), committed, frontier)
}

/// A context whose gates 6, 7 and 8 all pass for a snapshot at `h`.
macro_rules! good_ctx {
    ($vs:expr, $h:expr) => {
        FetchContext {
            committed_height: $h - 1,
            highest_qc_height: $h + 2,
            voted_view: None,
            validator_pubkeys: &$vs,
            quorum: quorum($vs.len()),
        }
    };
}

fn bundle_at(tag: &str, t: u64) -> novai_node::snapshot::bundle::SnapshotBundle {
    let fx = build_fixture(tag, FixtureSpec { t, ..FixtureSpec::default() });
    build_bundle(&fx.tmp.0).expect("produce")
}

// ---------------------------------------------------------------------------
// The happy path, end to end except the wire
// ---------------------------------------------------------------------------

#[test]
fn a_real_bundle_passes_every_gate_reassembles_and_materialises() {
    let b = bundle_at("f5f_happy", 20);
    let vs = dev_valset();
    let ctx = good_ctx!(vs, b.manifest.height);

    let mut f = SnapshotFetch::default();
    f.accept_manifest(&encode_manifest_v1(&b.manifest).unwrap(), &ctx)
        .expect("a manifest this node produced itself must pass its own gates");
    assert_eq!(f.height(), Some(b.manifest.height));
    assert_eq!(f.missing_indexes().len(), b.chunks.len());

    for (i, c) in b.chunks.iter().enumerate() {
        let v = f.accept_chunk(b.manifest.height, i as u32, c);
        assert!(matches!(v, ChunkVerdict::Accepted { .. }), "chunk {i}: {v:?}");
    }
    assert!(f.is_complete());
    assert!(f.missing_indexes().is_empty());

    let rebuilt = f.into_bundle().expect("complete");
    assert_eq!(rebuilt.manifest, b.manifest);
    assert_eq!(rebuilt.chunks, b.chunks);

    // And the reassembled bundle installs to the same verdict as the source.
    let dir = TmpDir::new("f5f_happy_stage");
    let target = dir.0.join("staged");
    materialize(&rebuilt, &target).expect("materialise");
    let (code, out, err) = run_a0(&["audit", "--db", target.to_str().unwrap()]);
    assert_eq!(code, 0, "stdout:\n{out}\nstderr:\n{err}");
    assert!(out.contains(&format!("RESULT PASS height={}", b.manifest.height)), "{out}");
    assert!(out.contains(&hex::encode(b.manifest.state_root)), "{out}");
}

// ---------------------------------------------------------------------------
// The gates, one test per reason
// ---------------------------------------------------------------------------

#[test]
fn gate_1_refuses_an_unknown_format_version() {
    let b = bundle_at("f5f_g1", 20);
    let vs = dev_valset();
    let ctx = good_ctx!(vs, b.manifest.height);
    let mut bytes = encode_manifest_v1(&b.manifest).unwrap();
    bytes[0] = 99;
    let mut f = SnapshotFetch::default();
    assert!(matches!(
        f.accept_manifest(&bytes, &ctx),
        Err(ManifestReject::Undecodable(_)) | Err(ManifestReject::UnknownVersion(_))
    ));
}

#[test]
fn gate_2_refuses_a_manifest_no_quorum_signed() {
    // THE trust anchor. A sub-quorum QC is not a difference of opinion: it is
    // the difference between installing canonical state and installing a fork.
    let fx = build_fixture(
        "f5f_g2",
        FixtureSpec {
            t: 20,
            voters: vec![0, 1],
            ..FixtureSpec::default()
        },
    );
    // The producer already refuses this, so build the manifest by hand from a
    // good bundle and swap in the sub-quorum QC.
    let good = bundle_at("f5f_g2_good", 20);
    let bad_qc = a0_common::make_qc(&fx.block_t1, &[0, 1]);
    let m = SnapshotManifest {
        qc_h1: bad_qc,
        ..good.manifest.clone()
    };
    let vs = dev_valset();
    let ctx = good_ctx!(vs, m.height);
    let mut f = SnapshotFetch::default();
    match f.accept_manifest(&encode_manifest_v1(&m).unwrap(), &ctx) {
        Err(ManifestReject::NotCertified(_)) => {}
        other => panic!("a sub-quorum manifest must be refused at gate 2, got {other:?}"),
    }
}

#[test]
fn gate_2_runs_before_any_chunk_is_requested() {
    // Ordering matters: a refused manifest must leave nothing to fetch, so a
    // hostile peer cannot make this node spend bandwidth.
    let good = bundle_at("f5f_order", 20);
    let fx = build_fixture("f5f_order_fx", FixtureSpec { t: 20, voters: vec![0], ..FixtureSpec::default() });
    let m = SnapshotManifest {
        qc_h1: a0_common::make_qc(&fx.block_t1, &[0]),
        ..good.manifest.clone()
    };
    let vs = dev_valset();
    let ctx = good_ctx!(vs, m.height);
    let mut f = SnapshotFetch::default();
    assert!(f.accept_manifest(&encode_manifest_v1(&m).unwrap(), &ctx).is_err());
    assert!(
        f.missing_indexes().is_empty() && f.manifest().is_none(),
        "a refused manifest must leave NOTHING to fetch"
    );
}

#[test]
fn gate_5_refuses_a_manifest_whose_root_is_not_its_headers() {
    // The lag-0 identity. If header(H).state_root is not the claimed root, the
    // manifest is describing a different state than the one it certifies.
    let b = bundle_at("f5f_g5", 20);
    let m = SnapshotManifest {
        state_root: [0x99; 32],
        ..b.manifest.clone()
    };
    let vs = dev_valset();
    let ctx = good_ctx!(vs, m.height);
    let mut f = SnapshotFetch::default();
    assert_eq!(
        f.accept_manifest(&encode_manifest_v1(&m).unwrap(), &ctx),
        Err(ManifestReject::IdentityViolated)
    );
}

#[test]
fn gate_6_refuses_a_snapshot_that_would_not_move_this_node_forward() {
    let b = bundle_at("f5f_g6", 20);
    let vs = dev_valset();
    let ctx = FetchContext {
        committed_height: b.manifest.height,
        highest_qc_height: b.manifest.height + 2,
        voted_view: None,
        validator_pubkeys: &vs,
        quorum: quorum(vs.len()),
    };
    let mut f = SnapshotFetch::default();
    assert!(matches!(
        f.accept_manifest(&encode_manifest_v1(&b.manifest).unwrap(), &ctx),
        Err(ManifestReject::NotAhead { .. })
    ));
}

#[test]
fn gate_7_refuses_a_snapshot_at_or_below_this_nodes_highest_vote() {
    // Belt and braces over the installer's max(own, donor) merge. A violation
    // means something is badly wrong, so it refuses loudly rather than relying
    // on the merge to paper over it.
    let b = bundle_at("f5f_g7", 20);
    let vs = dev_valset();
    let ctx = FetchContext {
        committed_height: 0,
        highest_qc_height: b.manifest.height + 2,
        voted_view: Some((b.manifest.height + 5, 0)),
        validator_pubkeys: &vs,
        quorum: quorum(vs.len()),
    };
    let mut f = SnapshotFetch::default();
    assert!(matches!(
        f.accept_manifest(&encode_manifest_v1(&b.manifest).unwrap(), &ctx),
        Err(ManifestReject::WouldRegressVote { .. })
    ));
}

#[test]
fn t5_3_gate_8_refuses_a_stale_manifest_without_fetching_anything() {
    // T5.3. A producer offering a snapshot already far below the frontier
    // would put the receiver straight back where it started. Refused BEFORE a
    // single chunk is requested.
    let b = bundle_at("f5f_g8", 20);
    let vs = dev_valset();
    let frontier = b.manifest.height + FRESHNESS_MARGIN_BLOCKS + 1;
    let ctx = FetchContext {
        committed_height: 0,
        highest_qc_height: frontier,
        voted_view: None,
        validator_pubkeys: &vs,
        quorum: quorum(vs.len()),
    };
    let mut f = SnapshotFetch::default();
    match f.accept_manifest(&encode_manifest_v1(&b.manifest).unwrap(), &ctx) {
        Err(ManifestReject::Stale { height, frontier: fr }) => {
            assert_eq!(height, b.manifest.height);
            assert_eq!(fr, frontier);
        }
        other => panic!("a stale manifest must be refused, got {other:?}"),
    }
    assert!(
        f.manifest().is_none() && f.missing_indexes().is_empty(),
        "and nothing may be queued for fetching"
    );

    // Exactly at the margin is still acceptable: the boundary is not off by one.
    let ok_ctx = FetchContext {
        highest_qc_height: b.manifest.height + FRESHNESS_MARGIN_BLOCKS,
        ..ctx
    };
    let mut f2 = SnapshotFetch::default();
    assert!(f2
        .accept_manifest(&encode_manifest_v1(&b.manifest).unwrap(), &ok_ctx)
        .is_ok());
}

// ---------------------------------------------------------------------------
// T5.2 the byzantine producer: valid manifest, corrupt chunks
// ---------------------------------------------------------------------------

#[test]
fn t5_2_a_corrupt_chunk_is_rejected_and_the_good_one_is_accepted_after() {
    // The adversarial case the per-chunk digest exists for. The manifest is
    // genuine and quorum certified, so the receiver keeps it; only the peer's
    // bytes are wrong, and the same index accepted from an honest peer
    // completes the fetch.
    let b = bundle_at("f5f_byz", 20);
    let vs = dev_valset();
    let ctx = good_ctx!(vs, b.manifest.height);
    let mut f = SnapshotFetch::default();
    f.accept_manifest(&encode_manifest_v1(&b.manifest).unwrap(), &ctx)
        .expect("the manifest itself is honest");

    let h = b.manifest.height;
    let mut corrupt = b.chunks[0].clone();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 0x01;
    assert_eq!(
        f.accept_chunk(h, 0, &corrupt),
        ChunkVerdict::DigestMismatch,
        "one flipped byte must be caught by the manifest's digest"
    );
    assert_eq!(
        f.missing_indexes().len(),
        b.chunks.len(),
        "and a rejected chunk must not be counted as delivered"
    );

    // The honest peer answers the same index.
    assert!(matches!(
        f.accept_chunk(h, 0, &b.chunks[0]),
        ChunkVerdict::Accepted { .. }
    ));
    for (i, c) in b.chunks.iter().enumerate().skip(1) {
        assert!(matches!(
            f.accept_chunk(h, i as u32, c),
            ChunkVerdict::Accepted { .. }
        ));
    }
    assert!(f.is_complete(), "the fetch completes despite the bad peer");
    assert!(f.into_bundle().is_some());
}

#[test]
fn chunks_for_another_snapshot_or_a_bad_index_are_refused() {
    let b = bundle_at("f5f_idx", 20);
    let vs = dev_valset();
    let ctx = good_ctx!(vs, b.manifest.height);
    let mut f = SnapshotFetch::default();
    f.accept_manifest(&encode_manifest_v1(&b.manifest).unwrap(), &ctx)
        .unwrap();
    let h = b.manifest.height;
    assert_eq!(f.accept_chunk(h + 1, 0, &b.chunks[0]), ChunkVerdict::WrongHeight);
    assert_eq!(
        f.accept_chunk(h, 9_999, &b.chunks[0]),
        ChunkVerdict::UnknownIndex
    );
    assert!(matches!(
        f.accept_chunk(h, 0, &b.chunks[0]),
        ChunkVerdict::Accepted { .. }
    ));
    assert_eq!(
        f.accept_chunk(h, 0, &b.chunks[0]),
        ChunkVerdict::Duplicate,
        "a broadcast request draws several answers; duplicates are normal"
    );
}

#[test]
fn an_incomplete_fetch_cannot_produce_a_bundle() {
    let b = bundle_at("f5f_incomplete", 20);
    let vs = dev_valset();
    let ctx = good_ctx!(vs, b.manifest.height);
    let mut f = SnapshotFetch::default();
    f.accept_manifest(&encode_manifest_v1(&b.manifest).unwrap(), &ctx)
        .unwrap();
    assert!(!f.is_complete());
    assert!(
        f.into_bundle().is_none(),
        "a partial fetch must never yield an installable bundle"
    );
}

#[test]
fn a_reset_abandons_everything_in_flight() {
    let b = bundle_at("f5f_reset", 20);
    let vs = dev_valset();
    let ctx = good_ctx!(vs, b.manifest.height);
    let mut f = SnapshotFetch::default();
    f.accept_manifest(&encode_manifest_v1(&b.manifest).unwrap(), &ctx)
        .unwrap();
    f.accept_chunk(b.manifest.height, 0, &b.chunks[0]);
    f.reset();
    assert!(f.manifest().is_none());
    assert!(!f.is_complete());
}

#[test]
fn the_valset_the_gates_check_against_is_this_nodes_own() {
    // Identity never travels in a snapshot. A manifest is judged against the
    // validator set THIS node derives from its launch flags, which is what
    // makes accepting a donor's bundle legitimate at all.
    assert_eq!(dev_signing_keys().len(), 4);
    assert_eq!(quorum(dev_valset().len()), 3);
    let (vs, committed, frontier) = ctx_at(19, 22);
    assert_eq!(vs.len(), 4);
    assert!(frontier > committed);
}
