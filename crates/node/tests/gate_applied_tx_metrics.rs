//! Gate ACCEL-Q8: the executed-work metrics surface.
//!
//! `novai_block_tx_count` and `novai_total_txs_committed` count INCLUSIONS.
//! The proposer's only selection predicate is `tx.nonce == expected_nonce`
//! and expected advances only at commit, which lands at trigger height minus
//! two while the leader rotates every height, so a transaction proposed at H
//! stays selectable by the H+1 and H+2 leaders. Those re-inclusions execute
//! as `TxOutcome::Skipped` and change no state, so this is a MEASUREMENT bug
//! and not a safety bug: the live throughput counters overstate executed
//! throughput by a duplication factor that is unknown and load-dependent.
//!
//! These pins cover the two halves the commit-path wiring cannot cover on its
//! own. `tally_outcomes` is the counting rule itself, proven on a mixed slice
//! and on both degenerate slices. The surface tests prove the three new names
//! reach the Prometheus text with the RIGHT VALUES: `to_prometheus` renders
//! from a single positional `format!` argument list, so a metric inserted at
//! the wrong argument position silently reports a neighbour's number, and
//! every value in the fixture below is distinct so that misalignment cannot
//! hide. The two existing names are asserted here as well, unchanged, because
//! this change is purely additive and their meaning is consumed by
//! monitoring/novai-monitor/alerts.py and by every historical measurement.
//!
//! The end-to-end wiring pin (that `on_commit` actually feeds these counters
//! the Applied count and not the included count) lives in
//! `crates/node/src/main.rs`'s `mod tests`, because `ExecutionCommitCallback`
//! and `CommitMetrics` are private to the binary target and unreachable from
//! here.

use novai_execution::TxOutcome;
use novai_node::metrics::{tally_outcomes, MetricsSnapshot};

/// Every value distinct, so a positional-argument misalignment in
/// `to_prometheus` cannot render a neighbour's number and still pass. Kept
/// arithmetically coherent with the inclusion counters it sits beside:
/// 7 applied out of 11 included in the last block, and 4_100 + 8_300 == 12_400
/// committed since startup, a duplication factor of about 3.
fn snapshot() -> MetricsSnapshot {
    MetricsSnapshot {
        committed_height: 1_900_001,
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
        block_tx_count: 11,
        total_txs_committed: 12_400,
        block_applied_tx_count: 7,
        total_txs_applied: 4_100,
        total_txs_skipped: 8_300,
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
        copilot_observations_total: 52,
        anomaly_signals_total: 53,
        anomaly_signals_published: 54,
        anomaly_last_confidence: 55,
        highest_qc_height: 1_900_003,
        seconds_since_last_commit: 1,
        sync_mode: 0,
        snapshot_produce_seconds: 0.0,
        snapshot_background_seconds: 0.0,
        snapshot_height: 0,
    }
}

#[test]
fn tally_splits_a_mixed_block_into_applied_and_skipped() {
    let tally = tally_outcomes(&[
        TxOutcome::Applied,
        TxOutcome::Skipped,
        TxOutcome::Skipped,
        TxOutcome::Applied,
        TxOutcome::Skipped,
    ]);
    assert_eq!(
        tally.applied, 2,
        "the tally must count Applied outcomes only; counting every outcome \
         is exactly the overstatement this gate exists to measure"
    );
    assert_eq!(
        tally.skipped, 3,
        "the tally must count Skipped outcomes only"
    );
    assert_eq!(
        tally.applied + tally.skipped,
        5,
        "every outcome must land in exactly one half, so \
         applied + skipped reconciles against the included count"
    );
}

#[test]
fn tally_handles_the_degenerate_blocks() {
    let empty = tally_outcomes(&[]);
    assert_eq!((empty.applied, empty.skipped), (0, 0), "empty block");

    let all_applied = tally_outcomes(&[TxOutcome::Applied, TxOutcome::Applied]);
    assert_eq!(
        (all_applied.applied, all_applied.skipped),
        (2, 0),
        "a block with no duplicates reports a duplication factor of 1"
    );

    let all_skipped = tally_outcomes(&[TxOutcome::Skipped, TxOutcome::Skipped]);
    assert_eq!(
        (all_skipped.applied, all_skipped.skipped),
        (0, 2),
        "a block that is entirely re-inclusions executes nothing"
    );
}

#[test]
fn metrics_surface_exposes_block_applied_tx_count() {
    let out = snapshot().to_prometheus();
    assert!(
        out.contains("# TYPE novai_block_applied_tx_count gauge"),
        "the metrics surface must expose the last block's EXECUTED tx count \
         as a gauge beside novai_block_tx_count"
    );
    assert!(
        out.contains("\nnovai_block_applied_tx_count 7\n"),
        "the gauge must render the Applied count (7), not the included \
         count (11) and not a neighbouring metric's value; got:\n{out}"
    );
}

#[test]
fn metrics_surface_exposes_total_txs_applied() {
    let out = snapshot().to_prometheus();
    assert!(
        out.contains("# TYPE novai_total_txs_applied counter"),
        "the metrics surface must expose cumulative EXECUTED transactions as \
         a counter; applied over committed is the duplication factor"
    );
    assert!(
        out.contains("\nnovai_total_txs_applied 4100\n"),
        "the counter must render the Applied total (4100), not the committed \
         total (12400); got:\n{out}"
    );
}

#[test]
fn metrics_surface_exposes_total_txs_skipped() {
    let out = snapshot().to_prometheus();
    assert!(
        out.contains("# TYPE novai_total_txs_skipped counter"),
        "the metrics surface must expose cumulative root-neutral skips"
    );
    assert!(
        out.contains("\nnovai_total_txs_skipped 8300\n"),
        "the counter must render the Skipped total (8300); got:\n{out}"
    );
}

/// The purely-additive pin. These two names are read by
/// monitoring/novai-monitor/alerts.py (tx_rate) and by every historical
/// measurement; the executed-work metrics are allowed to sit beside them and
/// are not allowed to change them.
#[test]
fn the_existing_inclusion_counters_are_unchanged() {
    let out = snapshot().to_prometheus();
    assert!(
        out.contains("# TYPE novai_block_tx_count gauge"),
        "novai_block_tx_count must remain a gauge"
    );
    assert!(
        out.contains("\nnovai_block_tx_count 11\n"),
        "novai_block_tx_count must still report INCLUSIONS (11); got:\n{out}"
    );
    assert!(
        out.contains("# TYPE novai_total_txs_committed counter"),
        "novai_total_txs_committed must remain a counter"
    );
    assert!(
        out.contains("\nnovai_total_txs_committed 12400\n"),
        "novai_total_txs_committed must still report INCLUSIONS (12400); \
         alerts.py derives tx_rate from it and every historical measurement \
         is denominated in it; got:\n{out}"
    );
}
