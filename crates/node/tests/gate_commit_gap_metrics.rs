//! Commit-gap observability (incident WEDGE-20260718): the metrics surface
//! must expose the consensus frontier, the consensus-minus-committed gap,
//! and the wall-clock age of the last committed advance.
//!
//! In the 20260718 incident the frontier ran 818,258 heights above the
//! committed floor over five days and no metric made that visible: the
//! surface exposed committed height alone, so the runaway was invisible to
//! the monitor until the fleet was already wedged. These tests pin the
//! three gauges the monitor's commit_stall dual-trigger alarm reads.
//!
//! RED discipline note: the TYPE-line assertions here are the RED proof,
//! written and proven failing against the tree that predates the gauges.
//! The snapshot constructor necessarily grows fields with the
//! implementation (the compiler forces that), and value-level assertions
//! for the new gauges arrive with it; the TYPE-line assertions stay
//! untouched from RED to GREEN. Value-level tests for the gap derivation
//! edge cases and the commit clock live next to the implementation in
//! crates/node/src/metrics.rs.

use novai_node::metrics::MetricsSnapshot;

fn snapshot() -> MetricsSnapshot {
    MetricsSnapshot {
        committed_height: 6_980_080,
        current_round: 0,
        peer_count: 3,
        mempool_size: 10,
        mempool_ready: 0,
        mempool_waiting: 0,
        mempool_gapped: 0,
        mempool_senders: 0,
        mempool_rejects_nonce_too_low: 0,
        mempool_rejects_nonce_too_high: 0,
        mempool_rejects_sender_limit: 0,
        mempool_rejects_fee_too_low: 0,
        mempool_rejects_full: 0,
        view_changes_total: 5,
        block_tx_count: 2,
        total_txs_committed: 1_000,
        // Gate ACCEL-Q8 fields; same compiler-forced growth as above.
        // Kept coherent with the two inclusion counters (1 <= 2, and
        // 350 + 650 == 1_000). Their own pins live in
        // gate_applied_tx_metrics.rs; the assertions below are untouched.
        block_applied_tx_count: 1,
        total_txs_applied: 350,
        total_txs_skipped: 650,
        // Gate G0 fields; compiler-forced constructor growth, zero like the
        // rest of this fixture. The G0 surface is asserted with distinct
        // values in gate_g0_metrics_surface.rs.
        block_interval_seconds: 0.0,
        block_interval_window_seconds: 0.0,
        block_interval_window_blocks: 0,
        commit_latency_seconds: 0.0,
        commit_latency_pending: 0,
        block_bytes: 0,
        total_block_bytes: 0,
        db_bytes_total: 0,
        db_bytes_smt_nodes: 0,
        db_bytes_straddling: 0,
        db_bytes_scan_seconds: 0.0,
        db_bytes_age_seconds: 0,
        copilot_observations_total: 0,
        anomaly_signals_total: 0,
        anomaly_signals_published: 0,
        anomaly_last_confidence: 0,
        highest_qc_height: 6_980_083,
        seconds_since_last_commit: 1,
        // Gate F5 Stage 1 added this field; the compiler forces the
        // constructor to carry it, exactly as this file's RED discipline note
        // anticipates. The gauge's own pins live in gate_f5_detection_red.rs;
        // the assertions below are untouched.
        sync_mode: 0,
        // Gate F5 Stage 2 fields; same compiler-forced growth as above.
        snapshot_produce_seconds: 0.0,
        snapshot_background_seconds: 0.0,
        snapshot_height: 0,
    }
}

#[test]
fn metrics_surface_exposes_highest_qc_height() {
    let out = snapshot().to_prometheus();
    assert!(
        out.contains("# TYPE novai_highest_qc_height gauge"),
        "the metrics surface must expose the consensus frontier as \
         novai_highest_qc_height; an 818k-height runaway was invisible \
         without it"
    );
    assert!(out.contains("novai_highest_qc_height 6980083"));
}

#[test]
fn metrics_surface_exposes_commit_gap() {
    let out = snapshot().to_prometheus();
    assert!(
        out.contains("# TYPE novai_consensus_commit_gap gauge"),
        "the metrics surface must expose the consensus-minus-committed gap \
         directly, so the monitor's gap trigger reads one authoritative \
         number instead of deriving it"
    );
    assert!(
        out.contains("novai_consensus_commit_gap 3"),
        "the gap must be derived from the two exposed heights (6980083 \
         minus 6980080)"
    );
}

#[test]
fn metrics_surface_exposes_seconds_since_last_commit() {
    let out = snapshot().to_prometheus();
    assert!(
        out.contains("# TYPE novai_seconds_since_last_commit gauge"),
        "the metrics surface must expose the wall-clock age of the last \
         committed advance, the rate-independent half of the dual-trigger \
         alarm"
    );
    assert!(out.contains("novai_seconds_since_last_commit 1"));
}
