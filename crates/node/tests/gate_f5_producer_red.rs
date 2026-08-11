//! Gate F5 Stage 2 RED tests: the demand-driven snapshot producer.
//!
//! THE PROPERTY THIS FILE EXISTS FOR. Stages 0 and 1 were relocation and
//! detection: no state, nothing on the commit path. Stage 2 puts a hook on the
//! commit path, under the database lock, which is the same place the forced
//! compaction lives and the same defect class that stranded the node this gate
//! is recovering. So the load-bearing test here is not "the bundle is correct",
//! it is "the commit path did almost nothing and the expensive work provably
//! happened somewhere else".
//!
//! That is proven three ways, none of them a timing assertion:
//! - BEHAVIOURAL: after the hook returns, no bundle exists. It only appears
//!   after the separate off-lock step runs. So the scan, the audit and the SMT
//!   rebuild cannot have run inside the hook.
//! - METRIC: the under-lock number is recorded by the hook and is UNCHANGED by
//!   the background step, so the commit-path cost cannot hide inside a total.
//! - STRUCTURAL: production takes a filesystem path and nothing else. It has no
//!   handle to the live database, so it cannot hold the commit lock even by
//!   mistake.
//!
//! RED discipline: this file reads API that does not exist on the preceding
//! tree, so its RED state is a compile failure, which is weak on its own. The
//! load-bearing evidence is the MUTATION proof recorded at the gate.

#[path = "a0_common/mod.rs"]
mod a0_common;

use a0_common::{build_fixture, run_a0, Evidence, FixtureSpec, TmpDir};
use novai_node::consensus_node::Storage;
use novai_node::snapshot::bundle::{
    decode_manifest_v1, encode_manifest_v1, SnapshotBundle, SNAPSHOT_FORMAT_VERSION,
};
use novai_node::snapshot::produce::{build_bundle, extract_leaf_set, ProduceError};
use novai_node::snapshot::producer::{HookOutcome, SnapshotProducer};
use novai_node::snapshot::stage::materialize;
use novai_state::{Kv, RocksKv, KEY_EXECUTED_HEIGHT, KEY_HIGHEST_QC};

fn producer_for(tag: &str) -> (SnapshotProducer, TmpDir) {
    let work = TmpDir::new(tag);
    // TmpDir::new creates the directory; RocksDB requires the checkpoint TARGET
    // not to exist, and the producer composes a fresh child path per capture,
    // so an existing parent is exactly right.
    (SnapshotProducer::new(work.0.clone()), work)
}

// ---------------------------------------------------------------------------
// The commit-path property
// ---------------------------------------------------------------------------

#[test]
fn with_no_demand_the_commit_hook_does_nothing_at_all() {
    // The steady state on a healthy fleet. Nobody is recovering, so the commit
    // path must not checkpoint, must not flush, must not touch disk.
    let fx = build_fixture("f5_nodemand", FixtureSpec::default());
    let (producer, work) = producer_for("f5_nodemand_work");
    let db = Storage::Rocks(fx.reopen());

    assert_eq!(producer.on_commit(&db, fx.t), HookOutcome::Skipped);
    assert!(!producer.has_pending());
    assert_eq!(producer.last_checkpoint_micros(), 0);
    assert!(producer.cached().is_none());

    let entries: Vec<_> = std::fs::read_dir(&work.0)
        .expect("work dir readable")
        .filter_map(Result::ok)
        .collect();
    assert!(
        entries.is_empty(),
        "an undemanded commit must leave no checkpoint behind, found {entries:?}"
    );
}

#[test]
fn the_commit_hook_does_no_production_work_and_the_metric_says_so() {
    let fx = build_fixture("f5_split", FixtureSpec::default());
    let (producer, _work) = producer_for("f5_split_work");
    let db = Storage::Rocks(fx.reopen());

    producer.request();
    assert_eq!(producer.on_commit(&db, fx.t), HookOutcome::CheckpointTaken);

    // BEHAVIOURAL: the hook created a checkpoint and stopped. Nothing has been
    // scanned, audited, rebuilt or chunked.
    assert!(producer.has_pending());
    assert!(
        producer.cached().is_none(),
        "the hook must not produce a bundle: production is off-lock work"
    );
    let under_lock = producer.last_checkpoint_micros();
    assert_eq!(
        producer.last_background_micros(),
        0,
        "no background work can have happened yet"
    );

    // The off-lock step does the real work.
    let height = producer
        .run_pending_production()
        .expect("a checkpoint was pending")
        .expect("production must succeed on a healthy checkpoint");
    assert_eq!(height, fx.t);
    assert!(producer.cached().is_some());
    assert!(
        producer.last_background_micros() > 0,
        "the expensive half must be measured"
    );

    // METRIC: the commit-path number did not absorb the background time. This
    // is what keeps novai_snapshot_produce_seconds honest.
    assert_eq!(
        producer.last_checkpoint_micros(),
        under_lock,
        "novai_snapshot_produce_seconds must measure the under-lock portion \
         ONLY; if the background step can move it, the commit-path cost is \
         hidden inside a total and the gauge is worthless"
    );
}

#[test]
fn production_needs_only_a_path_never_a_handle_to_the_live_database() {
    // STRUCTURAL: build_bundle's entire input is a directory path. That is the
    // type-level reason the scan and the rebuild cannot run under the commit
    // lock: this code is never given anything that holds one.
    let fx = build_fixture("f5_pathonly", FixtureSpec::default());
    let bundle = build_bundle(&fx.tmp.0).expect("a healthy directory produces a bundle");
    assert_eq!(bundle.manifest.height, fx.t);
    assert_eq!(bundle.manifest.state_root, fx.r1);
}

#[test]
fn a_fresh_cached_bundle_answers_the_demand_without_another_checkpoint() {
    let fx = build_fixture("f5_fresh", FixtureSpec::default());
    let (producer, _work) = producer_for("f5_fresh_work");
    let db = Storage::Rocks(fx.reopen());

    producer.request();
    assert_eq!(producer.on_commit(&db, fx.t), HookOutcome::CheckpointTaken);
    producer.run_pending_production().unwrap().unwrap();
    assert!(!producer.demanded(), "producing answers the demand");

    // A new demand while the cache is fresh must not re-checkpoint.
    producer.request();
    assert_eq!(
        producer.on_commit(&db, fx.t + 1),
        HookOutcome::Skipped,
        "a fresh cached bundle answers the demand with zero commit-path cost"
    );
    assert!(!producer.has_pending());
}

#[test]
fn the_metrics_surface_separates_commit_path_cost_from_background_cost() {
    let out = novai_node::metrics::MetricsSnapshot {
        committed_height: 100,
        highest_qc_height: 102,
        seconds_since_last_commit: 1,
        sync_mode: 0,
        snapshot_produce_seconds: 0.012_5,
        snapshot_background_seconds: 3.5,
        snapshot_height: 98,
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
    }
    .to_prometheus();

    for ty in [
        "# TYPE novai_snapshot_produce_seconds gauge",
        "# TYPE novai_snapshot_background_seconds gauge",
        "# TYPE novai_snapshot_height gauge",
    ] {
        assert!(out.contains(ty), "missing {ty}:\n{out}");
    }
    assert!(out.contains("novai_snapshot_produce_seconds 0.012500"));
    assert!(out.contains("novai_snapshot_background_seconds 3.500000"));
    assert!(out.contains("novai_snapshot_height 98"));

    // The HELP text is the contract with whoever reads the dashboard at three
    // in the morning. It must say, in words, that this number is the
    // commit-path share and not the total, because a number labelled
    // "produce_seconds" that silently meant "total" would make an expensive
    // commit-path hook look cheap.
    let help = out
        .lines()
        .find(|l| l.starts_with("# HELP novai_snapshot_produce_seconds"))
        .expect("help line");
    assert!(
        help.contains("COMMIT-PATH") && help.contains("excludes"),
        "the help text must state the scope explicitly: {help}"
    );
}

// ---------------------------------------------------------------------------
// T2.3 clean boundary only
// ---------------------------------------------------------------------------

#[test]
fn the_hook_refuses_mid_batch_when_the_cursors_disagree() {
    // Inside a multi-block commit batch the committed cursor already sits at
    // the last block while the executed cursor trails, so only the final block
    // of a batch is a capture point.
    let fx = build_fixture("f5_torn_hook", FixtureSpec::default());
    {
        let mut db = fx.reopen();
        db.put(KEY_EXECUTED_HEIGHT, &(fx.t - 1).to_be_bytes())
            .expect("tear the cursors");
    }
    let (producer, _work) = producer_for("f5_torn_hook_work");
    let db = Storage::Rocks(fx.reopen());
    producer.request();
    assert_eq!(producer.on_commit(&db, fx.t), HookOutcome::NotAtBoundary);
    assert!(!producer.has_pending());
}

#[test]
fn production_refuses_a_torn_checkpoint() {
    let fx = build_fixture("f5_torn_produce", FixtureSpec::default());
    {
        let mut db = fx.reopen();
        db.put(KEY_EXECUTED_HEIGHT, &(fx.t - 1).to_be_bytes())
            .expect("tear the cursors");
    }
    match build_bundle(&fx.tmp.0) {
        Err(ProduceError::AuditFailed { failures, .. }) => {
            assert!(
                failures.iter().any(|f| f.contains("A1") && f.contains("cursors differ")),
                "the refusal must name the torn cursors: {failures:?}"
            );
        }
        other => panic!("a torn copy must be refused, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// T2.2 fail closed on anything the classification table does not know
// ---------------------------------------------------------------------------

#[test]
fn production_refuses_an_unknown_key_rather_than_dropping_it() {
    let fx = build_fixture("f5_unknown", FixtureSpec::default());
    {
        let mut db = fx.reopen();
        db.put(b"wat/undocumented_family/x", b"v").expect("plant");
    }
    match build_bundle(&fx.tmp.0) {
        Err(ProduceError::AuditFailed { failures, .. }) => {
            assert!(
                failures
                    .iter()
                    .any(|f| f.contains("unknown key") && f.contains("wat/undocumented_family/x")),
                "the refusal must NAME the key, or an operator cannot act: {failures:?}"
            );
        }
        other => panic!("an unknown key must refuse production, got {other:?}"),
    }
}

#[test]
fn production_refuses_a_defined_but_unwritten_key() {
    let fx = build_fixture("f5_unwritten", FixtureSpec::default());
    {
        let mut db = fx.reopen();
        db.put(b"derived_views/planted", b"v").expect("plant");
    }
    assert!(
        build_bundle(&fx.tmp.0).is_err(),
        "a family with no production writer at this HEAD has unknown provenance \
         and must fail closed"
    );
}

#[test]
fn the_leaf_extraction_refuses_independently_of_the_audit() {
    // In the full pipeline the audit's A3 catches these first, so this guard is
    // unreachable there. It is tested directly so it is not uncovered code, and
    // so the extraction does not silently inherit its correctness from another
    // module's check ordering.
    let fx = build_fixture("f5_extract", FixtureSpec::default());
    {
        let mut db = fx.reopen();
        db.put(b"wat/unknown", b"v").expect("plant");
    }
    let db = RocksKv::open(&fx.tmp.0).expect("reopen");
    match extract_leaf_set(&db) {
        Err(ProduceError::UnclassifiableKey { key, .. }) => assert_eq!(key, "wat/unknown"),
        other => panic!("expected a named refusal, got {other:?}"),
    }
}

#[test]
fn the_leaf_extraction_returns_canonical_order() {
    let fx = build_fixture("f5_order", FixtureSpec::default());
    let db = RocksKv::open(&fx.tmp.0).expect("reopen");
    let pairs = extract_leaf_set(&db).expect("healthy copy");
    let mut sorted = pairs.clone();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        pairs, sorted,
        "chunk digests depend on order, so two producers over identical state \
         must chunk identically"
    );
    assert_eq!(pairs.len(), 4, "the fixture's four accounts are the leaf set");
}

// ---------------------------------------------------------------------------
// T2.4 the bundle codec
// ---------------------------------------------------------------------------

#[test]
fn manifest_roundtrips_through_its_codec() {
    let fx = build_fixture("f5_codec", FixtureSpec::default());
    let bundle = build_bundle(&fx.tmp.0).expect("produce");

    let bytes = encode_manifest_v1(&bundle.manifest).expect("encode");
    let back = decode_manifest_v1(&bytes).expect("decode");
    assert_eq!(back, bundle.manifest);
    assert_eq!(back.version, SNAPSHOT_FORMAT_VERSION);

    // Truncation at any point is rejected, never half-decoded.
    for cut in 0..bytes.len() {
        assert!(
            decode_manifest_v1(&bytes[..cut]).is_err(),
            "a manifest truncated at {cut} must be rejected"
        );
    }
    // An unknown format version is refused: this is the mixed-binary guard.
    let mut wrong = bytes.clone();
    wrong[0] = SNAPSHOT_FORMAT_VERSION + 1;
    assert!(decode_manifest_v1(&wrong).is_err());
}

#[test]
fn every_chunk_matches_the_digest_the_manifest_claims() {
    let fx = build_fixture("f5_digest", FixtureSpec::default());
    let bundle = build_bundle(&fx.tmp.0).expect("produce");
    bundle.verify_digests().expect("a fresh bundle is self consistent");
    assert_eq!(bundle.manifest.chunk_digests.len(), bundle.chunks.len());
    assert_eq!(
        bundle.pairs().expect("decode").len(),
        bundle.manifest.leaf_count as usize
    );

    // One flipped payload byte must be caught before anything is written.
    let mut tampered = SnapshotBundle {
        manifest: bundle.manifest.clone(),
        chunks: bundle.chunks.clone(),
    };
    let last = tampered.chunks[0].len() - 1;
    tampered.chunks[0][last] ^= 0x01;
    assert!(tampered.verify_digests().is_err());

    let dir = TmpDir::new("f5_digest_stage");
    let target = dir.0.join("staged");
    assert!(
        materialize(&tampered, &target).is_err(),
        "a tampered chunk must never reach disk"
    );
}

// ---------------------------------------------------------------------------
// T2.5 the equivalence claim, executed
// ---------------------------------------------------------------------------

#[test]
fn a_materialized_bundle_audits_to_the_same_height_and_root_as_the_source() {
    let fx = build_fixture("f5_equiv", FixtureSpec::default());

    // The source's own verdict.
    let (src_code, src_out, _e) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(src_code, 0, "{src_out}");
    let src_root = a0_common::parse_result_root(&src_out);

    // Produce, then materialise into a FRESH directory.
    let bundle = build_bundle(&fx.tmp.0).expect("produce");
    let dir = TmpDir::new("f5_equiv_stage");
    let target = dir.0.join("staged");
    materialize(&bundle, &target).expect("materialise");

    // The materialised directory's verdict must be identical.
    let (code, out, err) = run_a0(&["audit", "--db", target.to_str().unwrap()]);
    assert_eq!(code, 0, "stdout:\n{out}\nstderr:\n{err}");
    for check in ["A1", "A2", "A3", "A4", "A5", "A6", "A7", "A8"] {
        assert!(out.contains(&format!("{check} PASS")), "missing {check}:\n{out}");
    }
    assert_eq!(
        a0_common::parse_result_root(&out),
        src_root,
        "the whole gate rests on this: a bundle carries exactly the state the \
         source proved, so the rebuilt root must be byte identical"
    );
    assert!(out.contains(&format!("RESULT PASS height={}", fx.t)));
    assert_eq!(hex::encode(bundle.manifest.state_root), src_root);
}

#[test]
fn materializing_refuses_a_directory_that_already_holds_a_database() {
    // A snapshot is only ever installed into a FRESH directory. A surviving
    // stale flat row would be READ by execution (index families really are
    // deleted in production) and would diverge the state silently.
    let fx = build_fixture("f5_notempty", FixtureSpec::default());
    let bundle = build_bundle(&fx.tmp.0).expect("produce");
    assert!(
        materialize(&bundle, &fx.tmp.0).is_err(),
        "materialising over an existing database must be refused"
    );
}

// ---------------------------------------------------------------------------
// T2.6 fork rejection
// ---------------------------------------------------------------------------

#[test]
fn a_self_consistent_but_uncertified_state_passes_a5_and_fails_a6() {
    // This is the property that stops A0 from degrading into a
    // self-consistency checker that would happily bless a fork. It was
    // confirmed in the field on a real diverged directory; this is the same
    // shape as a fixture, and it is what the producer's mandatory self-audit
    // relies on.
    let fx = build_fixture("f5_fork", FixtureSpec::default());
    {
        let mut db = fx.reopen();
        // Strip every trace of certification, leaving the state and its root
        // perfectly consistent with each other and with nothing else.
        db.delete(&novai_state::qc_key(fx.t + 1)).expect("drop qc row");
        db.delete(KEY_HIGHEST_QC).expect("drop highest qc");
    }
    let (code, out, _e) = run_a0(&["audit", "--db", &fx.db_arg()]);
    assert_eq!(code, 1, "an uncertified copy must FAIL:\n{out}");
    assert!(
        out.contains("A5 PASS"),
        "the state is self consistent, so the rebuild still matches:\n{out}"
    );
    assert!(
        out.contains("A6 FAIL"),
        "but nothing certifies it, so the canonicity check must fail:\n{out}"
    );

    assert!(
        build_bundle(&fx.tmp.0).is_err(),
        "a node must never cache a bundle over state no quorum signed"
    );
}

#[test]
fn production_refuses_when_the_certifying_quorum_is_short() {
    let fx = build_fixture(
        "f5_subquorum",
        FixtureSpec {
            voters: vec![0, 1],
            ..FixtureSpec::default()
        },
    );
    match build_bundle(&fx.tmp.0) {
        Err(ProduceError::AuditFailed { failures, .. }) => assert!(
            failures.iter().any(|f| f.contains("A6")),
            "a sub-quorum QC must fail the certification check: {failures:?}"
        ),
        other => panic!("expected refusal, got {other:?}"),
    }
}

#[test]
fn production_carries_the_certification_evidence_in_the_bundle() {
    // A history-free bundle cannot be certified: the three checks that bind
    // state to the canonical chain are exactly the three that need block
    // history. So the evidence must TRAVEL, not merely have been present.
    let fx = build_fixture(
        "f5_evidence",
        FixtureSpec {
            evidence: Evidence::QcRow,
            ..FixtureSpec::default()
        },
    );
    let b = build_bundle(&fx.tmp.0).expect("produce");
    assert_eq!(b.manifest.block_h.height, fx.t);
    assert_eq!(b.manifest.block_h1.height, fx.t + 1);
    assert_eq!(b.manifest.qc_h1.height, fx.t + 1);
    assert_eq!(
        b.manifest.block_h.state_root, b.manifest.state_root,
        "the lag-0 identity: header(H) commits to post-state(H)"
    );
    assert_eq!(
        b.manifest.block_h1.parent_hash,
        novai_consensus_types::codec::hash_block_v1(&b.manifest.block_h).unwrap(),
        "the certified successor must anchor the block whose state travels"
    );
}
