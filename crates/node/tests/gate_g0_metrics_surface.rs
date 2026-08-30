//! Gate G0: the measurement surface reaches Prometheus with the right values.
//!
//! `to_prometheus` renders from a single positional `format!` argument list,
//! so a metric inserted at the wrong argument position silently reports a
//! NEIGHBOUR's number and every test that only checks presence still passes.
//! Twelve new fields were inserted into the middle of that list, which is
//! exactly the change that hazard is waiting for.
//!
//! Every value in the fixture below is therefore distinct, and each assertion
//! pins a name to its own value. The existing names are asserted unchanged
//! alongside them, because this change is purely additive and the monitor
//! derives its transaction rate from `novai_total_txs_committed`.

use novai_node::metrics::MetricsSnapshot;

/// Distinct values throughout, so a positional misalignment cannot render a
/// neighbour's number and still pass. Coherent where the metrics are related:
/// 0.25 s per block over a 300 s window is 1,200 blocks, and the db byte
/// buckets partition the total.
fn snapshot() -> MetricsSnapshot {
    MetricsSnapshot {
        committed_height: 1_900_001,
        highest_qc_height: 1_900_003,
        seconds_since_last_commit: 1,
        sync_mode: 0,
        snapshot_produce_seconds: 0.125,
        snapshot_background_seconds: 2.5,
        snapshot_height: 1_800_000,
        current_round: 2,
        peer_count: 3,
        mempool_size: 41,
        mempool_ready: 42,
        mempool_waiting: 43,
        mempool_gapped: 44,
        mempool_senders: 45,
        mempool_rejects_nonce_too_low: 46,
        mempool_rejects_nonce_too_high: 47,
        mempool_rejects_sender_limit: 48,
        mempool_rejects_fee_too_low: 49,
        mempool_rejects_full: 50,
        view_changes_total: 51,
        block_interval_seconds: 0.25,
        block_interval_window_seconds: 300.0,
        block_interval_window_blocks: 1_200,
        commit_latency_seconds: 0.487,
        commit_latency_pending: 2,
        block_bytes: 21_365,
        total_block_bytes: 987_654_321,
        db_bytes_total: 25_000_000_000,
        db_bytes_smt_nodes: 23_000_000_000,
        db_bytes_straddling: 400_000_000,
        db_bytes_scan_seconds: 0.003_5,
        db_bytes_age_seconds: 17,
        block_tx_count: 11,
        total_txs_committed: 12_400,
        block_applied_tx_count: 7,
        total_txs_applied: 4_100,
        total_txs_skipped: 8_300,
        copilot_observations_total: 52,
        anomaly_signals_total: 53,
        anomaly_signals_published: 54,
        anomaly_last_confidence: 55,
    }
}

#[test]
fn g0_gauges_render_with_their_own_values() {
    let out = snapshot().to_prometheus();

    // The block interval, published with the two numbers it was derived from
    // so a reader can confirm the division rather than trust it.
    assert!(out.contains("\nnovai_block_interval_seconds 0.250000\n"), "{out}");
    assert!(out.contains("\nnovai_block_interval_window_seconds 300.000\n"), "{out}");
    assert!(out.contains("\nnovai_block_interval_window_blocks 1200\n"), "{out}");

    // Commit latency and its in-flight depth.
    assert!(out.contains("\nnovai_commit_latency_seconds 0.487000\n"), "{out}");
    assert!(out.contains("\nnovai_commit_latency_pending 2\n"), "{out}");

    // Block bytes: the first byte metric the node has ever published.
    assert!(out.contains("\nnovai_block_bytes 21365\n"), "{out}");
    assert!(out.contains("\nnovai_total_block_bytes 987654321\n"), "{out}");

    // Database size by family.
    assert!(out.contains("\nnovai_db_bytes_total 25000000000\n"), "{out}");
    assert!(out.contains("\nnovai_db_bytes_smt_nodes 23000000000\n"), "{out}");
    assert!(out.contains("\nnovai_db_bytes_straddling 400000000\n"), "{out}");
    assert!(out.contains("\nnovai_db_bytes_scan_seconds 0.003500\n"), "{out}");
    assert!(out.contains("\nnovai_db_bytes_age_seconds 17\n"), "{out}");
}

#[test]
fn db_bytes_other_is_derived_so_the_parts_cannot_disagree_with_the_remainder() {
    // 25,000,000,000 - 23,000,000,000 - 400,000,000 = 1,600,000,000.
    // Derived inside the renderer for the same reason the commit gap is: two
    // independently published numbers that ought to sum can drift, and a
    // dashboard reading a stale remainder against fresh parts is worse than
    // no remainder at all.
    let out = snapshot().to_prometheus();
    assert!(out.contains("\nnovai_db_bytes_other 1600000000\n"), "{out}");
}

#[test]
fn db_bytes_other_saturates_rather_than_wrapping() {
    // A sample taken across a compaction can report parts that exceed the
    // total. The remainder must clamp to zero, never wrap to 18 exabytes.
    let mut snap = snapshot();
    snap.db_bytes_total = 1_000;
    snap.db_bytes_smt_nodes = 900;
    snap.db_bytes_straddling = 500;
    let out = snap.to_prometheus();
    assert!(out.contains("\nnovai_db_bytes_other 0\n"), "{out}");
}

#[test]
fn every_g0_name_carries_help_and_type_lines() {
    let out = snapshot().to_prometheus();
    for (name, kind) in [
        ("novai_block_interval_seconds", "gauge"),
        ("novai_block_interval_window_seconds", "gauge"),
        ("novai_block_interval_window_blocks", "gauge"),
        ("novai_commit_latency_seconds", "gauge"),
        ("novai_commit_latency_pending", "gauge"),
        ("novai_block_bytes", "gauge"),
        ("novai_total_block_bytes", "counter"),
        ("novai_db_bytes_total", "gauge"),
        ("novai_db_bytes_smt_nodes", "gauge"),
        ("novai_db_bytes_straddling", "gauge"),
        ("novai_db_bytes_other", "gauge"),
        ("novai_db_bytes_scan_seconds", "gauge"),
        ("novai_db_bytes_age_seconds", "gauge"),
    ] {
        assert!(out.contains(&format!("# HELP {name} ")), "missing HELP for {name}");
        assert!(
            out.contains(&format!("# TYPE {name} {kind}\n")),
            "missing or wrong TYPE for {name}"
        );
    }
}

#[test]
fn the_existing_surface_is_unchanged() {
    // Purely additive. The monitor derives its transaction rate from
    // novai_total_txs_committed and every historical measurement is
    // denominated in these names.
    let out = snapshot().to_prometheus();
    assert!(out.contains("\nnovai_committed_height 1900001\n"));
    assert!(out.contains("\nnovai_consensus_commit_gap 2\n"));
    assert!(out.contains("\nnovai_seconds_since_last_commit 1\n"));
    assert!(out.contains("\nnovai_block_tx_count 11\n"));
    assert!(out.contains("\nnovai_total_txs_committed 12400\n"));
    assert!(out.contains("\nnovai_block_applied_tx_count 7\n"));
    assert!(out.contains("\nnovai_total_txs_applied 4100\n"));
    assert!(out.contains("\nnovai_total_txs_skipped 8300\n"));
    assert!(out.contains("\nnovai_consensus_view_changes_total 51\n"));
    assert!(out.contains("\nnovai_anomaly_last_confidence 55\n"));
}

#[test]
fn the_block_interval_help_states_the_definition() {
    // The gauge is only comparable across runs if its definition travels with
    // it. Two runs a week apart are compared by a human reading this line.
    let out = snapshot().to_prometheus();
    let help = out
        .lines()
        .find(|l| l.starts_with("# HELP novai_block_interval_seconds"))
        .expect("the block interval must carry a HELP line");
    assert!(
        help.contains("trailing window"),
        "the HELP must say the measurement is over a trailing window: {help}"
    );
    assert!(
        help.contains("novai_block_interval_window_seconds")
            && help.contains("novai_block_interval_window_blocks"),
        "the HELP must name the two gauges the quotient is computed from: {help}"
    );
}
