//! Prometheus metrics HTTP endpoint.
//!
//! PURPOSE: Expose node metrics in Prometheus text format for monitoring.
//!
//! INVARIANTS:
//! - Server binds to specified address on startup
//! - /metrics returns valid Prometheus text format
//! - /health returns 200 OK if server is running
//!
//! FAILURE MODES:
//! - Port already in use → returns error on start
//! - Panic in collect_fn → request returns 500

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::thread;
use std::time::{Duration, Instant};
use tiny_http::{Response, Server, StatusCode};

/// Point-in-time snapshot of node metrics.
/// Gate SOAK C1/C2: process-wide admission and pool-shape counters.
///
/// Statics rather than threaded parameters because there is one node per
/// process and these are pure observation: threading five counters through
/// the RPC server signature would touch far more code than the measurement
/// is worth.
///
/// The pool-shape gauges are CACHED here by a periodic pass and read
/// lock-free at scrape time. Computing the census inside the scrape would
/// hold the mempool mutex against admission on every poll, and that mutex is
/// already shared by five threads (RPC, gossip, propose loop, observer,
/// metrics).
pub mod pool_metrics {
    use std::sync::atomic::{AtomicU64, Ordering};

    pub static READY: AtomicU64 = AtomicU64::new(0);
    pub static WAITING: AtomicU64 = AtomicU64::new(0);
    pub static GAPPED: AtomicU64 = AtomicU64::new(0);
    pub static SENDERS: AtomicU64 = AtomicU64::new(0);

    pub static REJ_NONCE_TOO_LOW: AtomicU64 = AtomicU64::new(0);
    pub static REJ_NONCE_TOO_HIGH: AtomicU64 = AtomicU64::new(0);
    pub static REJ_SENDER_LIMIT: AtomicU64 = AtomicU64::new(0);
    pub static REJ_FEE_TOO_LOW: AtomicU64 = AtomicU64::new(0);
    pub static REJ_FULL: AtomicU64 = AtomicU64::new(0);

    /// Publish a freshly computed census. Called by the periodic pass.
    pub fn publish_census(c: &mempool::PoolCensus) {
        READY.store(c.ready as u64, Ordering::Relaxed);
        WAITING.store(c.waiting as u64, Ordering::Relaxed);
        GAPPED.store(c.gapped as u64, Ordering::Relaxed);
        SENDERS.store(c.senders as u64, Ordering::Relaxed);
    }

    /// Count one admission rejection by reason. Called from every admission
    /// path, including gossip, whose rejections were previously invisible.
    pub fn record_rejection(err: &mempool::TxMempoolError) {
        let counter = match err {
            mempool::TxMempoolError::NonceTooLow { .. } => &REJ_NONCE_TOO_LOW,
            mempool::TxMempoolError::NonceTooHigh { .. } => &REJ_NONCE_TOO_HIGH,
            mempool::TxMempoolError::SenderLimitExceeded { .. } => &REJ_SENDER_LIMIT,
            mempool::TxMempoolError::FeeTooLow { .. } => &REJ_FEE_TOO_LOW,
            mempool::TxMempoolError::MempoolFull { .. } => &REJ_FULL,
            _ => return,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

/// Gate ACCEL-Q8: one committed block's outcome vector split into the work
/// that executed and the work that did not.
///
/// `novai_block_tx_count` and `novai_total_txs_committed` count INCLUSIONS.
/// The proposer's only selection predicate is `tx.nonce == expected_nonce`
/// and expected advances only at commit, which lands at trigger height minus
/// two while the leader rotates every height, so a transaction proposed at H
/// stays selectable by the H+1 and H+2 leaders. Those re-inclusions execute as
/// `TxOutcome::Skipped` and change no state, so this is a MEASUREMENT bug and
/// not a safety bug: the throughput counters overstate executed work by a
/// duplication factor that is unknown and load-dependent. Tallying the
/// outcomes makes the factor directly observable as applied over committed.
///
/// Commit-path safe by construction: one pass over a borrowed slice, no
/// allocation, no lock, no I/O, no syscall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OutcomeTally {
    /// Transactions that executed and moved state.
    pub applied: u64,
    /// Transactions that were committed but skipped root-neutrally.
    pub skipped: u64,
}

/// Tally one block's per-tx outcomes.
///
/// The match is deliberately exhaustive with no wildcard arm: a future
/// `TxOutcome` variant must fail to compile here rather than being silently
/// folded into `skipped` and quietly corrupting the duplication factor.
#[must_use]
pub fn tally_outcomes(outcomes: &[novai_execution::TxOutcome]) -> OutcomeTally {
    let mut tally = OutcomeTally::default();
    for outcome in outcomes {
        match outcome {
            novai_execution::TxOutcome::Applied => tally.applied += 1,
            novai_execution::TxOutcome::Skipped => tally.skipped += 1,
        }
    }
    tally
}

pub struct MetricsSnapshot {
    /// Height of last committed block.
    pub committed_height: u64,
    /// Height certified by this node's highest QC (the consensus
    /// frontier). WEDGE-20260718: the frontier ran 818,258 heights above
    /// the committed floor with no metric making it visible; this gauge
    /// plus the derived gap is that visibility.
    pub highest_qc_height: u64,
    /// Seconds since this node's metrics collector last observed the
    /// committed height advance (see `CommitClock`). The rate-independent
    /// half of the monitor's commit_stall dual-trigger alarm.
    pub seconds_since_last_commit: u64,
    /// Gate F5 Stage 1: the snapshot-sync detection phase
    /// (`SnapshotSyncMachine::gauge`). 0 block-range sync is viable, 1 the gap
    /// is past the fleet prune horizon and probes are coming back unserved,
    /// 2 armed (only an installed state snapshot can recover this node);
    /// 3 to 5 are reserved for the later fetch, verify and staged phases.
    ///
    /// Without this gauge an unrecoverable 346,000 block gap and a 30 second
    /// commit hiccup both surfaced as commit_stall alone, and the operator
    /// response to those two is completely different.
    pub sync_mode: u64,
    /// Gate F5 Stage 2: seconds spent UNDER THE DATABASE LOCK creating the last
    /// snapshot checkpoint. This is the commit-path cost of snapshot
    /// production and nothing else: the audit, the key scan, the leaf
    /// extraction and the SMT rebuild all run off the lock and are counted in
    /// `snapshot_background_seconds`.
    ///
    /// Split deliberately. The incident that motivated this whole gate started
    /// with unbounded blocking work on the commit path, so the commit-path
    /// share must be visible on its own rather than buried inside a total that
    /// makes it look small.
    pub snapshot_produce_seconds: f64,
    /// Gate F5 Stage 2: seconds spent OFF the lock on the last production.
    /// Its counterpart above is the one that can hurt consensus.
    pub snapshot_background_seconds: f64,
    /// Gate F5 Stage 2: height of the cached servable bundle, 0 when none.
    pub snapshot_height: u64,
    /// Current consensus round.
    pub current_round: u64,
    /// Number of connected peers.
    pub peer_count: u64,
    /// Transactions in mempool.
    pub mempool_size: u64,
    /// Gate SOAK C1: the pool split by how close each transaction is to
    /// inclusion. mempool_size alone cannot tell a healthy deep backlog from
    /// a jam, because it counts both, so no threshold on it is right in both
    /// directions. Flat unlabeled names on purpose: the monitor's parser
    /// drops any labeled sample, so a counter vector would be silently
    /// discarded and every alarm built on it would sit at insufficient_data.
    pub mempool_ready: u64,
    pub mempool_waiting: u64,
    pub mempool_gapped: u64,
    pub mempool_senders: u64,
    /// Gate SOAK C2: admission rejections by reason. Nothing on the dashboard
    /// could previously tell "the generator stopped" from "every submit is
    /// being refused"; gossip rejections in particular were swallowed
    /// silently.
    pub mempool_rejects_nonce_too_low: u64,
    pub mempool_rejects_nonce_too_high: u64,
    pub mempool_rejects_sender_limit: u64,
    pub mempool_rejects_fee_too_low: u64,
    pub mempool_rejects_full: u64,
    /// Total view changes (round advances due to timeouts).
    pub view_changes_total: u64,
    /// Gate G0: mean seconds per committed block over a trailing window,
    /// with the span and the height delta it was computed from published
    /// beside it. Zero means undefined, not instantaneous.
    pub block_interval_seconds: f64,
    pub block_interval_window_seconds: f64,
    pub block_interval_window_blocks: u64,
    /// Gate G0: measured seconds from proposing a block to that same block
    /// committing, for blocks THIS node proposed. Zero means this node has
    /// had none of its own proposals commit since it started.
    pub commit_latency_seconds: f64,
    /// Gate G0: proposals this node has stamped that have neither committed
    /// nor been reaped. Healthy is 0 to 2; a rising value is a view-change
    /// storm.
    pub commit_latency_pending: u64,
    /// Gate G0: encoded size of the last committed block, and the running
    /// total. There was previously no byte metric anywhere in the node, so
    /// every capacity statement in the throughput plan was arithmetic rather
    /// than measurement.
    pub block_bytes: u64,
    pub total_block_bytes: u64,
    /// Gate G0: live SST bytes, split by key family. The `smt/node/` family
    /// is content addressed and never garbage collected, so this is the
    /// instrument for how much of a node's disk is dead SMT versions.
    /// `straddling` is the share in files crossing the family boundary,
    /// reported separately rather than guessed onto one side.
    pub db_bytes_total: u64,
    pub db_bytes_smt_nodes: u64,
    pub db_bytes_straddling: u64,
    /// Gate G0: cost and freshness of the sample above. A timer-fed gauge
    /// whose feeder stopped looks identical to a database that stopped
    /// growing, and the age is what tells them apart.
    pub db_bytes_scan_seconds: f64,
    pub db_bytes_age_seconds: u64,
    /// Number of transactions in last committed block.
    pub block_tx_count: u64,
    /// Total transactions committed across all blocks.
    pub total_txs_committed: u64,
    /// Gate ACCEL-Q8: transactions in the last committed block that actually
    /// executed. Sits beside `block_tx_count`, which counts inclusions;
    /// the two differ by the block's skipped re-inclusions.
    pub block_applied_tx_count: u64,
    /// Gate ACCEL-Q8: total transactions that executed as Applied since
    /// startup. `total_txs_applied / total_txs_committed` IS the duplication
    /// factor, read straight off the surface with no join and no query.
    pub total_txs_applied: u64,
    /// Gate ACCEL-Q8: total transactions that committed but were skipped
    /// root-neutrally since startup. Carried because the outcome tally
    /// already computes it, and because a rising skip rate at flat applied
    /// throughput is the signature the duplication window is widening.
    pub total_txs_skipped: u64,

    // Copilot metrics
    /// Total copilot observation cycles.
    pub copilot_observations_total: u64,
    /// Total anomalies detected by copilot.
    pub anomaly_signals_total: u64,
    /// Total signals published on-chain.
    pub anomaly_signals_published: u64,
    /// Confidence level of last detected anomaly (0-255).
    pub anomaly_last_confidence: u64,
}

impl MetricsSnapshot {
    /// Format metrics as Prometheus text exposition format.
    ///
    /// Returns metrics in Prometheus text format:
    /// - One block per metric with HELP, TYPE, and value lines
    /// - Gauges for instantaneous values (height, round, peer_count, mempool_size)
    /// - Counter for monotonically increasing values (view_changes_total)
    pub fn to_prometheus(&self) -> String {
        format!(
            r#"# HELP novai_committed_height Height of last committed block
# TYPE novai_committed_height gauge
novai_committed_height {}

# HELP novai_highest_qc_height Height certified by the highest QC (consensus frontier)
# TYPE novai_highest_qc_height gauge
novai_highest_qc_height {}

# HELP novai_consensus_commit_gap Consensus frontier minus committed height (healthy: 2 to 3 at any block rate)
# TYPE novai_consensus_commit_gap gauge
novai_consensus_commit_gap {}

# HELP novai_seconds_since_last_commit Seconds since the committed height last advanced
# TYPE novai_seconds_since_last_commit gauge
novai_seconds_since_last_commit {}

# HELP novai_sync_mode Snapshot-sync detection phase (0 block sync viable, 1 past the prune horizon, 2 armed, 3-5 reserved)
# TYPE novai_sync_mode gauge
novai_sync_mode {}

# HELP novai_snapshot_produce_seconds Seconds under the db lock for the last snapshot checkpoint (COMMIT-PATH cost only, excludes the off-lock audit, scan and rebuild)
# TYPE novai_snapshot_produce_seconds gauge
novai_snapshot_produce_seconds {:.6}

# HELP novai_snapshot_background_seconds Seconds off the db lock for the last snapshot production (audit, scan, chunk)
# TYPE novai_snapshot_background_seconds gauge
novai_snapshot_background_seconds {:.6}

# HELP novai_snapshot_height Height of the cached servable snapshot bundle (0 = none)
# TYPE novai_snapshot_height gauge
novai_snapshot_height {}

# HELP novai_current_round Current consensus round
# TYPE novai_current_round gauge
novai_current_round {}

# HELP novai_peer_count Number of connected peers
# TYPE novai_peer_count gauge
novai_peer_count {}

# HELP novai_mempool_size Transactions pending in mempool
# TYPE novai_mempool_size gauge
novai_mempool_size {}

# HELP novai_mempool_ready Pooled txs at the sender's expected nonce (includable next block)
# TYPE novai_mempool_ready gauge
novai_mempool_ready {}

# HELP novai_mempool_waiting Pooled txs in the reachable run above expected (healthy backlog)
# TYPE novai_mempool_waiting gauge
novai_mempool_waiting {}

# HELP novai_mempool_gapped Pooled txs unreachable from the sender's expected nonce
# TYPE novai_mempool_gapped gauge
novai_mempool_gapped {}

# HELP novai_mempool_senders Distinct senders holding at least one pooled tx
# TYPE novai_mempool_senders gauge
novai_mempool_senders {}

# HELP novai_mempool_rejects_nonce_too_low Admission rejections: nonce below expected
# TYPE novai_mempool_rejects_nonce_too_low counter
novai_mempool_rejects_nonce_too_low {}

# HELP novai_mempool_rejects_nonce_too_high Admission rejections: nonce past the horizon
# TYPE novai_mempool_rejects_nonce_too_high counter
novai_mempool_rejects_nonce_too_high {}

# HELP novai_mempool_rejects_sender_limit Admission rejections: per-sender slot cap
# TYPE novai_mempool_rejects_sender_limit counter
novai_mempool_rejects_sender_limit {}

# HELP novai_mempool_rejects_fee_too_low Admission rejections: below the effective fee floor
# TYPE novai_mempool_rejects_fee_too_low counter
novai_mempool_rejects_fee_too_low {}

# HELP novai_mempool_rejects_full Admission rejections: mempool byte cap
# TYPE novai_mempool_rejects_full counter
novai_mempool_rejects_full {}

# HELP novai_consensus_view_changes_total Total view changes (round advances)
# TYPE novai_consensus_view_changes_total counter
novai_consensus_view_changes_total {}

# HELP novai_block_tx_count Transactions in last committed block
# TYPE novai_block_tx_count gauge
novai_block_tx_count {}

# HELP novai_block_interval_seconds Mean seconds per committed block over a trailing window, computed as novai_block_interval_window_seconds divided by novai_block_interval_window_blocks; 0 means undefined (fewer than two samples, or no commit in the window)
# TYPE novai_block_interval_seconds gauge
novai_block_interval_seconds {:.6}

# HELP novai_block_interval_window_seconds Wall seconds spanned by the samples the block interval was computed from
# TYPE novai_block_interval_window_seconds gauge
novai_block_interval_window_seconds {:.3}

# HELP novai_block_interval_window_blocks Committed heights gained across that span
# TYPE novai_block_interval_window_blocks gauge
novai_block_interval_window_blocks {}

# HELP novai_commit_latency_seconds Measured seconds from proposing a block to that block committing, for blocks THIS node proposed (about 1 height in 4); 0 means none of this node's proposals has committed since startup
# TYPE novai_commit_latency_seconds gauge
novai_commit_latency_seconds {:.6}

# HELP novai_commit_latency_pending Proposals stamped by this node that have neither committed nor been reaped (healthy 0 to 2)
# TYPE novai_commit_latency_pending gauge
novai_commit_latency_pending {}

# HELP novai_block_bytes Encoded size in bytes of the last committed block (85 byte header plus the signed transactions; the justify QC is not part of a block)
# TYPE novai_block_bytes gauge
novai_block_bytes {}

# HELP novai_total_block_bytes Total encoded block bytes committed since startup
# TYPE novai_total_block_bytes counter
novai_total_block_bytes {}

# HELP novai_db_bytes_total Live SST bytes across all column families (compressed on-disk size); excludes WAL, MANIFEST and LOG, so a young node reads 0 until its first memtable flush
# TYPE novai_db_bytes_total gauge
novai_db_bytes_total {}

# HELP novai_db_bytes_smt_nodes Live SST bytes in files lying wholly inside the smt/node/ key range (never garbage collected)
# TYPE novai_db_bytes_smt_nodes gauge
novai_db_bytes_smt_nodes {}

# HELP novai_db_bytes_straddling Live SST bytes in files crossing the smt/node/ boundary, attributable to neither side
# TYPE novai_db_bytes_straddling gauge
novai_db_bytes_straddling {}

# HELP novai_db_bytes_other Live SST bytes outside the smt/node/ range, derived as total minus smt_nodes minus straddling
# TYPE novai_db_bytes_other gauge
novai_db_bytes_other {}

# HELP novai_db_bytes_scan_seconds Seconds the last db-size sample held the database lock
# TYPE novai_db_bytes_scan_seconds gauge
novai_db_bytes_scan_seconds {:.6}

# HELP novai_db_bytes_age_seconds Seconds since the db-size sample was taken; a growing value with a nonzero total means the sampler has stopped
# TYPE novai_db_bytes_age_seconds gauge
novai_db_bytes_age_seconds {}

# HELP novai_total_txs_committed Total transactions committed across all blocks
# TYPE novai_total_txs_committed counter
novai_total_txs_committed {}

# HELP novai_block_applied_tx_count Transactions in last committed block that executed (Applied), against novai_block_tx_count which counts inclusions
# TYPE novai_block_applied_tx_count gauge
novai_block_applied_tx_count {}

# HELP novai_total_txs_applied Total transactions that executed (Applied) since startup; applied over committed is the duplicate-inclusion factor
# TYPE novai_total_txs_applied counter
novai_total_txs_applied {}

# HELP novai_total_txs_skipped Total transactions committed but skipped root-neutrally since startup
# TYPE novai_total_txs_skipped counter
novai_total_txs_skipped {}

# HELP novai_copilot_observations_total Total copilot observation cycles
# TYPE novai_copilot_observations_total counter
novai_copilot_observations_total {}

# HELP novai_anomaly_signals_total Total anomalies detected by copilot
# TYPE novai_anomaly_signals_total counter
novai_anomaly_signals_total {}

# HELP novai_anomaly_signals_published Total signals published on-chain
# TYPE novai_anomaly_signals_published counter
novai_anomaly_signals_published {}

# HELP novai_anomaly_last_confidence Confidence of last detected anomaly (0-255)
# TYPE novai_anomaly_last_confidence gauge
novai_anomaly_last_confidence {}
"#,
            self.committed_height,
            self.highest_qc_height,
            // Saturating: a fresh node (no QC yet) reports frontier 0 with a
            // positive committed height after recovery; the gap is 0, never
            // an underflow.
            self.highest_qc_height.saturating_sub(self.committed_height),
            self.seconds_since_last_commit,
            self.sync_mode,
            self.snapshot_produce_seconds,
            self.snapshot_background_seconds,
            self.snapshot_height,
            self.current_round,
            self.peer_count,
            self.mempool_size,
            self.mempool_ready,
            self.mempool_waiting,
            self.mempool_gapped,
            self.mempool_senders,
            self.mempool_rejects_nonce_too_low,
            self.mempool_rejects_nonce_too_high,
            self.mempool_rejects_sender_limit,
            self.mempool_rejects_fee_too_low,
            self.mempool_rejects_full,
            self.view_changes_total,
            self.block_tx_count,
            self.block_interval_seconds,
            self.block_interval_window_seconds,
            self.block_interval_window_blocks,
            self.commit_latency_seconds,
            self.commit_latency_pending,
            self.block_bytes,
            self.total_block_bytes,
            self.db_bytes_total,
            self.db_bytes_smt_nodes,
            self.db_bytes_straddling,
            // Derived in the renderer, like the commit gap above, so the
            // exposed parts can never disagree with the exposed remainder.
            self.db_bytes_total
                .saturating_sub(self.db_bytes_smt_nodes)
                .saturating_sub(self.db_bytes_straddling),
            self.db_bytes_scan_seconds,
            self.db_bytes_age_seconds,
            self.total_txs_committed,
            self.block_applied_tx_count,
            self.total_txs_applied,
            self.total_txs_skipped,
            self.copilot_observations_total,
            self.anomaly_signals_total,
            self.anomaly_signals_published,
            self.anomaly_last_confidence,
        )
    }
}

/// One reading of the block-interval gauge: the quotient and the two numbers
/// it was computed from.
///
/// The numerator and denominator are published alongside the quotient
/// deliberately. Every throughput ceiling in the plan divides by a block rate,
/// so a bare quotient asks the reader to trust an unstated window; publishing
/// the span and the height delta lets a reader a week later confirm what was
/// divided by what, which is what makes two runs comparable rather than merely
/// both present.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BlockRate {
    /// Mean seconds per committed block over the window. Zero means
    /// UNDEFINED, not instantaneous: either fewer than two samples, or a
    /// window in which the committed height did not move. `window_blocks`
    /// and `novai_seconds_since_last_commit` separate those two cases.
    pub interval_seconds: f64,
    /// Wall seconds actually spanned by the retained samples.
    pub window_seconds: f64,
    /// Committed heights actually gained across that span.
    pub window_blocks: u64,
}

/// Trailing-window block rate, feeding `novai_block_interval_seconds` and its
/// two window gauges (gate G0).
///
/// DEFINITION, which is the whole point of this type: the mean seconds per
/// committed block over a trailing wall-clock window, computed as elapsed time
/// divided by committed-height delta across the scrape samples retained in
/// that window.
///
/// That definition is chosen over the obvious alternative, the gap between two
/// consecutive commits, because the obvious one is not reproducible. Catch-up
/// commits a burst of blocks in microseconds and a stalled leader commits none
/// for a second, so the consecutive-commit gap describes neither steady state
/// nor anything that two runs a week apart could compare. Elapsed over
/// height-delta is arithmetically the same operation as the cleanest
/// measurement the project has, 158,321 blocks in 39,600 seconds, which is why
/// the gauge can be checked against that number instead of merely believed.
///
/// SCRAPE DRIVEN, like `CommitClock`: the collector calls `observe` with the
/// current committed height on every scrape. The commit path is not touched at
/// all, so the gauge costs the consensus critical path exactly nothing. The
/// cost is that the window is made of scrape samples, so a scraper that stops
/// freezes the window; the published `window_seconds` is what makes that
/// visible.
pub struct BlockRateClock {
    /// `(observed_at, committed_height)`, oldest first.
    samples: VecDeque<(Instant, u64)>,
    window: Duration,
    capacity: usize,
}

impl BlockRateClock {
    /// A 300 second window, which holds about ten samples at the monitor's
    /// 30 second poll interval and about 1,200 blocks at the measured 4 bps.
    #[must_use]
    pub fn new() -> Self {
        Self::new_with(Duration::from_secs(300), 1024)
    }

    /// `new` with an explicit window and sample cap, so tests are
    /// deterministic and can span the ground-truth measurement's 11 hours.
    #[must_use]
    pub fn new_with(window: Duration, capacity: usize) -> Self {
        Self {
            samples: VecDeque::new(),
            window,
            capacity,
        }
    }

    /// Samples currently retained. Observation only; the cap is what bounds it.
    #[must_use]
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// Record the currently observed committed height and return the trailing
    /// window's block rate.
    pub fn observe(&mut self, committed_height: u64) -> BlockRate {
        self.observe_at(committed_height, Instant::now())
    }

    /// `observe` with an explicit clock, so tests are deterministic.
    fn observe_at(&mut self, committed_height: u64, now: Instant) -> BlockRate {
        // A height REGRESSION means a snapshot install or a chain reset moved
        // this node onto a different history. The retained samples describe the
        // old one, and a saturating subtraction against them would publish a
        // near-zero block count over a full window, which reads as a stall that
        // is not happening. Discard rather than saturate.
        if let Some(&(_, newest_height)) = self.samples.back() {
            if committed_height < newest_height {
                self.samples.clear();
            }
        }
        self.samples.push_back((now, committed_height));

        // Trailing, not cumulative: a node that ran slowly for an hour and then
        // sped up must report the new rate, or every before-and-after
        // measurement is contaminated by the before.
        if let Some(cutoff) = now.checked_sub(self.window) {
            while self.samples.len() > 1 && self.samples[0].0 < cutoff {
                self.samples.pop_front();
            }
        }
        // Belt and braces against a scraper hammering /metrics. This costs
        // window span rather than accuracy, and the published window_seconds is
        // what makes the cost visible instead of silent.
        while self.samples.len() > self.capacity {
            self.samples.pop_front();
        }

        let (Some(&(oldest_at, oldest_height)), Some(&(newest_at, newest_height))) =
            (self.samples.front(), self.samples.back())
        else {
            return BlockRate::default();
        };
        if self.samples.len() < 2 {
            // One sample cannot define an interval. Reporting anything here
            // would be a fabricated number on a freshly started node.
            return BlockRate::default();
        }

        let window_seconds = newest_at.duration_since(oldest_at).as_secs_f64();
        let window_blocks = newest_height - oldest_height;
        BlockRate {
            interval_seconds: if window_blocks == 0 {
                0.0
            } else {
                #[allow(clippy::cast_precision_loss)]
                {
                    window_seconds / window_blocks as f64
                }
            },
            window_seconds,
            window_blocks,
        }
    }
}

impl Default for BlockRateClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Propose-to-commit latency for blocks THIS node proposed, feeding
/// `novai_commit_latency_seconds` (gate G0).
///
/// The plan DERIVES commit latency as `2 / bps + t_vote`. This measures it.
/// Both ends of the interval are readings of one monotonic clock in one
/// process, so no validator clock skew can enter the number. The cost of that
/// soundness is coverage: only blocks this node proposed are measurable,
/// roughly one height in four with a four-validator set. That is the right
/// trade, because the alternative that covers every block is a subtraction
/// across two machines' wall clocks, which would silently publish NTP drift.
///
/// KEYED BY BLOCK HASH, NOT BY HEIGHT. Our proposal at H can be orphaned while
/// a sibling commits at H; the engine has explicit machinery for exactly that
/// case. Keyed by height, an orphan would publish the sibling's commit time
/// against our proposal's stamp, which is a WRONG number rather than a missing
/// one, and a wrong number is the failure this gate exists to prevent.
///
/// Guarded by a plain mutex rather than atomics. The writer is the propose
/// loop on the main thread and the readers are peer-connection threads, but
/// every `on_commit` runs under the database lock, so commit-side calls are
/// already mutually exclusive with each other and this mutex is uncontended.
pub struct ProposalClock {
    /// `block_hash -> (height, proposed_at)` for proposals not yet resolved.
    pending: HashMap<[u8; 32], (u64, Instant)>,
    /// The most recent measured latency, or `None` if this node has not yet
    /// had one of its own proposals commit.
    last_latency: Option<Duration>,
    capacity: usize,
}

impl ProposalClock {
    #[must_use]
    pub fn new() -> Self {
        Self::new_with_capacity(64)
    }

    /// `new` with an explicit cap on outstanding stamps.
    #[must_use]
    pub fn new_with_capacity(capacity: usize) -> Self {
        Self {
            pending: HashMap::new(),
            last_latency: None,
            capacity: capacity.max(1),
        }
    }

    /// Outstanding unresolved proposal stamps. Observation only.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// The published gauge value. Zero means NOT YET MEASURED: this node has
    /// proposed nothing that committed since it started. Zero is safe as the
    /// sentinel because a real measurement is a positive duration.
    #[must_use]
    pub fn last_latency_seconds(&self) -> f64 {
        self.last_latency.map_or(0.0, |d| d.as_secs_f64())
    }

    /// Stamp a block this node has just proposed.
    pub fn note_proposed(&mut self, block_hash: [u8; 32], height: u64) {
        self.note_proposed_at(block_hash, height, Instant::now());
    }

    /// `note_proposed` with an explicit clock, so tests are deterministic.
    fn note_proposed_at(&mut self, block_hash: [u8; 32], height: u64, now: Instant) {
        // Height-based reaping cannot bound this on its own. The leader is
        // (height + round) % validators, so while the chain is stuck at one
        // height with rounds churning we become leader again every fourth
        // ROUND and emit a distinct block at the SAME height each time. The
        // committed frontier does not move, so nothing is ever reaped. The cap
        // is the only thing standing between a view-change storm and unbounded
        // growth; it drops the oldest stamp, which is the one least likely to
        // still be able to commit.
        if self.pending.len() >= self.capacity {
            if let Some(oldest) = self
                .pending
                .iter()
                .min_by_key(|(_, &(h, at))| (h, at))
                .map(|(k, _)| *k)
            {
                self.pending.remove(&oldest);
            }
        }
        self.pending.insert(block_hash, (height, now));
    }

    /// Resolve a committed block against the outstanding stamps, returning the
    /// measured latency if this node proposed exactly that block.
    pub fn note_committed(&mut self, block_hash: &[u8; 32], committed_height: u64) -> Option<Duration> {
        self.note_committed_at(block_hash, committed_height, Instant::now())
    }

    /// `note_committed` with an explicit clock, so tests are deterministic.
    fn note_committed_at(
        &mut self,
        block_hash: &[u8; 32],
        committed_height: u64,
        now: Instant,
    ) -> Option<Duration> {
        let measured = self
            .pending
            .remove(block_hash)
            .map(|(_, proposed_at)| now.duration_since(proposed_at));
        if let Some(latency) = measured {
            self.last_latency = Some(latency);
        }
        // Anything at or below the committed frontier can never commit now, so
        // it is an orphan. Same rule the engine already applies to its own
        // pending execution map.
        self.pending.retain(|_, &mut (height, _)| height > committed_height);
        measured
    }
}

impl Default for ProposalClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Gate G0: the process-wide propose-to-commit clock.
///
/// A static for the same reason `pool_metrics` is one, and the reason is
/// stated there: there is one node per process and this is pure observation.
/// The two ends of the measurement sit in different crates, the propose loop
/// in this library and the commit callback in the binary, so threading a
/// handle between them would change signatures across both for a gauge.
pub mod proposal_metrics {
    use super::ProposalClock;
    use crate::MutexExt;
    use std::sync::{Mutex, OnceLock};

    static CLOCK: OnceLock<Mutex<ProposalClock>> = OnceLock::new();

    fn clock() -> &'static Mutex<ProposalClock> {
        CLOCK.get_or_init(|| Mutex::new(ProposalClock::new()))
    }

    /// Stamp a block this node just proposed, immediately before broadcasting
    /// it. Before rather than after: no peer can vote for a block it has not
    /// received, and a commit needs a QC two heights up, so stamping first
    /// guarantees the stamp is visible to any thread that could later observe
    /// the matching commit.
    pub fn note_proposed(block_hash: [u8; 32], height: u64) {
        clock().lock_or_recover().note_proposed(block_hash, height);
    }

    /// Resolve a committed block against the outstanding stamps.
    pub fn note_committed(block_hash: [u8; 32], committed_height: u64) {
        clock()
            .lock_or_recover()
            .note_committed(&block_hash, committed_height);
    }

    /// Read the published latency for a scrape.
    #[must_use]
    pub fn last_latency_seconds() -> f64 {
        clock().lock_or_recover().last_latency_seconds()
    }

    /// Outstanding unresolved stamps, for the scrape.
    #[must_use]
    pub fn pending() -> u64 {
        clock().lock_or_recover().pending_len() as u64
    }
}

/// Gate G0: database size by key family, sampled on a timer and read
/// lock-free at scrape time.
///
/// Cached rather than computed in the scrape for the same reason the pool
/// census is: the read needs the database lock, which is on the consensus
/// critical path, and a scrape must never be able to take it. The sampling
/// cost and the sample's age are published alongside the sizes, because a
/// timer-fed gauge whose feeder has stopped looks exactly like a database
/// that has stopped growing.
pub mod db_metrics {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::OnceLock;
    use std::time::Instant;

    static BASE: OnceLock<Instant> = OnceLock::new();

    fn base() -> Instant {
        *BASE.get_or_init(Instant::now)
    }

    /// Exclusive upper bound of the `smt/node/` key range. `smt/node/` with
    /// its trailing `/` (0x2F) bumped to `0` (0x30), which is the successor
    /// of the prefix and therefore the end of the half-open range. The family
    /// is lexicographically contiguous with only `nnpx/` below and
    /// `smt/root` above, so nothing else interleaves with it.
    pub const SMT_NODE_RANGE_END: &[u8] = b"smt/node0";

    /// Every live SST byte in the database.
    pub static TOTAL: AtomicU64 = AtomicU64::new(0);
    /// Bytes in files lying wholly inside the `smt/node/` range.
    pub static SMT_NODES: AtomicU64 = AtomicU64::new(0);
    /// Bytes in files crossing the range boundary, attributable to neither
    /// side.
    pub static STRADDLING: AtomicU64 = AtomicU64::new(0);
    /// Microseconds the last sample held the database lock.
    pub static SCAN_MICROS: AtomicU64 = AtomicU64::new(0);
    /// Nanoseconds since process start at the last sample; 0 means never.
    pub static SAMPLED_AT_NANOS: AtomicU64 = AtomicU64::new(0);

    /// Publish a freshly taken sample. Called by the periodic pass.
    pub fn publish(total: u64, smt_nodes: u64, straddling: u64, scan_micros: u64) {
        TOTAL.store(total, Ordering::Relaxed);
        SMT_NODES.store(smt_nodes, Ordering::Relaxed);
        STRADDLING.store(straddling, Ordering::Relaxed);
        SCAN_MICROS.store(scan_micros, Ordering::Relaxed);
        #[allow(clippy::cast_possible_truncation)]
        SAMPLED_AT_NANOS.store(base().elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    /// Whole seconds since the last sample. Zero when nothing has been
    /// sampled yet, which the zero `TOTAL` alongside it disambiguates.
    #[must_use]
    pub fn age_seconds() -> u64 {
        let sampled = SAMPLED_AT_NANOS.load(Ordering::Relaxed);
        if sampled == 0 {
            return 0;
        }
        #[allow(clippy::cast_possible_truncation)]
        let now = base().elapsed().as_nanos() as u64;
        now.saturating_sub(sampled) / 1_000_000_000
    }
}

/// Wall-clock age tracker for the committed height, feeding the
/// `novai_seconds_since_last_commit` gauge (WEDGE-20260718, the
/// rate-independent half of the monitor's commit_stall dual-trigger
/// alarm).
///
/// Scrape driven: the metrics collector calls `observe` with the current
/// committed height on every scrape; the clock stamps each advance and
/// reports how long the height has been flat. The clock starts at
/// construction (process boot), so a node that never commits reports a
/// growing age from boot, which is exactly the alarm-worthy condition. A
/// node restart resets the clock, giving a restarted node its 30 second
/// grace before the time trigger can page again.
pub struct CommitClock {
    last_height: u64,
    last_advance: Instant,
}

impl CommitClock {
    #[must_use]
    pub fn new() -> Self {
        Self::new_at(Instant::now())
    }

    /// `new` with an explicit boot instant, so tests are deterministic.
    fn new_at(boot: Instant) -> Self {
        Self {
            last_height: 0,
            last_advance: boot,
        }
    }

    /// Record the currently observed committed height and return how many
    /// whole seconds it has been since the height last advanced.
    pub fn observe(&mut self, committed_height: u64) -> u64 {
        self.observe_at(committed_height, Instant::now())
    }

    /// `observe` with an explicit clock, so tests are deterministic. A
    /// height that does not advance (or regresses, which cannot happen
    /// from a monotone committed cursor) leaves the stamp untouched: the
    /// age keeps growing, the conservative reading for an alarm input.
    fn observe_at(&mut self, committed_height: u64, now: Instant) -> u64 {
        if committed_height > self.last_height {
            self.last_height = committed_height;
            self.last_advance = now;
        }
        now.duration_since(self.last_advance).as_secs()
    }
}

impl Default for CommitClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Start the metrics HTTP server.
///
/// Spawns a dedicated thread to handle HTTP requests on the specified address.
/// Returns immediately after starting the listener.
///
/// # Endpoints
/// - `GET /metrics` - Prometheus text format metrics
/// - `GET /health` - Health check (returns 200 OK)
/// - All other paths return 404 Not Found
///
/// # Arguments
/// - `bind_addr` - Address to bind the HTTP server (e.g., "0.0.0.0:8080")
/// - `collect_fn` - Closure that collects metrics snapshot from node state
///
/// # Errors
/// Returns error if the server cannot bind to the address (e.g., port in use).
pub fn start_metrics_server<F>(bind_addr: &str, collect_fn: F) -> Result<(), String>
where
    F: Fn() -> MetricsSnapshot + Send + 'static,
{
    let addr: SocketAddr = bind_addr
        .parse()
        .map_err(|e| format!("invalid address: {e}"))?;

    let server = Server::http(addr).map_err(|e| format!("failed to start HTTP server: {e}"))?;

    tracing::info!(%addr, "Metrics server listening");

    thread::spawn(move || {
        for request in server.incoming_requests() {
            let response = match request.url() {
                "/metrics" => {
                    let metrics = collect_fn();
                    let body = metrics.to_prometheus();
                    Response::from_string(body).with_header(
                        "Content-Type: text/plain; version=0.0.4; charset=utf-8"
                            .parse::<tiny_http::Header>()
                            .unwrap(),
                    )
                }
                "/health" => Response::from_string("OK\n"),
                _ => Response::from_string("Not Found").with_status_code(StatusCode(404)),
            };

            // Ignore send errors (client may have disconnected)
            let _ = request.respond(response);
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prometheus_format() {
        let snapshot = MetricsSnapshot {
            committed_height: 42,
            highest_qc_height: 44,
            seconds_since_last_commit: 1,
            sync_mode: 0,
            snapshot_produce_seconds: 0.0,
            snapshot_background_seconds: 0.0,
            snapshot_height: 0,
            current_round: 3,
            peer_count: 4,
            mempool_size: 127,
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
            block_tx_count: 25,
            total_txs_committed: 1050,
            // Gate ACCEL-Q8 fields; compiler-forced constructor growth. Kept
            // coherent with the two inclusion counters (10 <= 25, and
            // 350 + 700 == 1050).
            block_applied_tx_count: 10,
            total_txs_applied: 350,
            total_txs_skipped: 700,
            // Gate G0 fields; compiler-forced constructor growth, zero like
            // the rest of this fixture. The G0 surface is asserted with
            // distinct values in crates/node/tests/gate_g0_metrics_surface.rs.
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
            copilot_observations_total: 200,
            anomaly_signals_total: 3,
            anomaly_signals_published: 2,
            anomaly_last_confidence: 180,
        };

        let output = snapshot.to_prometheus();

        // Check that all metrics are present
        assert!(output.contains("novai_committed_height 42"));
        assert!(output.contains("novai_highest_qc_height 44"));
        // The gap is derived inside the renderer from the two heights, so
        // the exposed pair can never disagree with the exposed gap.
        assert!(output.contains("novai_consensus_commit_gap 2"));
        assert!(output.contains("novai_seconds_since_last_commit 1"));
        assert!(output.contains("novai_current_round 3"));
        assert!(output.contains("novai_peer_count 4"));
        assert!(output.contains("novai_mempool_size 127"));
        assert!(output.contains("novai_consensus_view_changes_total 5"));
        assert!(output.contains("novai_block_tx_count 25"));
        assert!(output.contains("novai_total_txs_committed 1050"));

        // Check copilot metrics
        assert!(output.contains("novai_copilot_observations_total 200"));
        assert!(output.contains("novai_anomaly_signals_total 3"));
        assert!(output.contains("novai_anomaly_signals_published 2"));
        assert!(output.contains("novai_anomaly_last_confidence 180"));

        // Check that HELP and TYPE lines are present
        assert!(output.contains("# HELP novai_committed_height"));
        assert!(output.contains("# TYPE novai_committed_height gauge"));
        assert!(output.contains("# TYPE novai_consensus_view_changes_total counter"));
        assert!(output.contains("# TYPE novai_total_txs_committed counter"));
        assert!(output.contains("# TYPE novai_copilot_observations_total counter"));
        assert!(output.contains("# TYPE novai_anomaly_signals_total counter"));
        assert!(output.contains("# TYPE novai_anomaly_last_confidence gauge"));
    }

    #[test]
    fn test_zero_values() {
        let snapshot = MetricsSnapshot {
            committed_height: 0,
            highest_qc_height: 0,
            seconds_since_last_commit: 0,
            sync_mode: 0,
            snapshot_produce_seconds: 0.0,
            snapshot_background_seconds: 0.0,
            snapshot_height: 0,
            current_round: 0,
            peer_count: 0,
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
            // Gate ACCEL-Q8 fields; compiler-forced constructor growth,
            // zero like the rest of this fixture.
            block_applied_tx_count: 0,
            total_txs_applied: 0,
            total_txs_skipped: 0,
            // Gate G0 fields; compiler-forced constructor growth, zero like
            // the rest of this fixture. The G0 surface is asserted with
            // distinct values in crates/node/tests/gate_g0_metrics_surface.rs.
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
        };

        let output = snapshot.to_prometheus();
        assert!(output.contains("novai_committed_height 0"));
        assert!(output.contains("novai_highest_qc_height 0"));
        assert!(output.contains("novai_consensus_commit_gap 0"));
        assert!(output.contains("novai_seconds_since_last_commit 0"));
        assert!(output.contains("novai_peer_count 0"));
        assert!(output.contains("novai_block_tx_count 0"));
        assert!(output.contains("novai_copilot_observations_total 0"));
        assert!(output.contains("novai_anomaly_signals_total 0"));
        assert!(output.contains("novai_anomaly_last_confidence 0"));
    }

    #[test]
    fn test_commit_gap_saturates_at_zero() {
        // A recovered node has its committed height before its first QC
        // adoption lands in a snapshot; the gap must clamp to 0, never
        // wrap.
        let snapshot = MetricsSnapshot {
            committed_height: 500,
            highest_qc_height: 0,
            seconds_since_last_commit: 0,
            sync_mode: 0,
            snapshot_produce_seconds: 0.0,
            snapshot_background_seconds: 0.0,
            snapshot_height: 0,
            current_round: 0,
            peer_count: 0,
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
            // Gate ACCEL-Q8 fields; compiler-forced constructor growth,
            // zero like the rest of this fixture.
            block_applied_tx_count: 0,
            total_txs_applied: 0,
            total_txs_skipped: 0,
            // Gate G0 fields; compiler-forced constructor growth, zero like
            // the rest of this fixture. The G0 surface is asserted with
            // distinct values in crates/node/tests/gate_g0_metrics_surface.rs.
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
        };
        let output = snapshot.to_prometheus();
        assert!(output.contains("novai_consensus_commit_gap 0"));
    }

    #[test]
    fn test_commit_clock_ages_while_flat_and_resets_on_advance() {
        use std::time::Duration;

        let t0 = Instant::now();
        let mut clock = CommitClock::new_at(t0);

        // First observation stamps the height; age reads 0.
        assert_eq!(clock.observe_at(100, t0), 0);
        // The height stays flat: the age grows with the wall clock. At 31
        // seconds the monitor's time trigger (30 s) would fire.
        assert_eq!(clock.observe_at(100, t0 + Duration::from_secs(10)), 10);
        assert_eq!(clock.observe_at(100, t0 + Duration::from_secs(31)), 31);
        // A committed advance resets the age.
        assert_eq!(clock.observe_at(101, t0 + Duration::from_secs(32)), 0);
        assert_eq!(clock.observe_at(101, t0 + Duration::from_secs(35)), 3);
    }

    #[test]
    fn test_commit_clock_healthy_cadence_stays_near_zero_at_any_rate() {
        use std::time::Duration;

        // A healthy chain commits continuously at any block rate, so every
        // scrape observes a fresh advance and the age never accumulates.
        // One scrape every 5 seconds, heights advancing by the per-interval
        // block count for rates from 1 to 1000 blocks/s.
        for rate in [1u64, 4, 25, 100, 1000] {
            let t0 = Instant::now();
            let mut clock = CommitClock::new_at(t0);
            let mut height = 1_000_000u64;
            for scrape in 1..=12u64 {
                height += rate * 5;
                let age = clock.observe_at(height, t0 + Duration::from_secs(scrape * 5));
                assert_eq!(
                    age, 0,
                    "healthy cadence at {rate} blocks/s must keep the commit age at zero"
                );
            }
        }
    }

    // ==========================================================================
    // Gate G0: the commit latency gauge
    //
    // The plan DERIVES commit latency as 2/bps + t_vote. This measures it
    // instead, on the proposing node, where both ends of the interval are
    // readings of one monotonic clock in one process and no clock skew between
    // validators can enter the number. That restricts coverage to blocks this
    // node proposed, roughly one height in four with a four-validator set,
    // which is a limitation worth having over a cross-node subtraction that
    // would silently measure NTP drift.
    //
    // The measurement is keyed by BLOCK HASH and not by height. Our proposal at
    // H can be orphaned while a sibling block commits at H: the engine has
    // explicit machinery for exactly that case, comparing committed hashes
    // against last_proposed_block_hash. Keyed by height, an orphan would
    // publish the sibling's commit against our proposal's stamp, which is a
    // wrong number rather than a missing one.
    // ==========================================================================

    #[test]
    fn commit_latency_measures_our_own_proposal_end_to_end() {
        use std::time::Duration;

        let t0 = Instant::now();
        let mut clock = ProposalClock::new();
        assert_eq!(clock.last_latency_seconds(), 0.0, "nothing measured yet");

        clock.note_proposed_at([0xaa; 32], 500, t0);
        let measured =
            clock.note_committed_at(&[0xaa; 32], 500, t0 + Duration::from_millis(487));

        assert_eq!(measured, Some(Duration::from_millis(487)));
        assert!(
            (clock.last_latency_seconds() - 0.487).abs() < 1e-9,
            "got {}",
            clock.last_latency_seconds()
        );
    }

    #[test]
    fn commit_latency_reports_nothing_for_a_block_we_did_not_propose() {
        use std::time::Duration;

        // Three quarters of committed blocks are somebody else's. A gauge that
        // produced a number for those would be reporting an unmeasured
        // quantity, which is the exact failure this whole gate exists to stop.
        let t0 = Instant::now();
        let mut clock = ProposalClock::new();

        let measured = clock.note_committed_at(&[0xbb; 32], 500, t0 + Duration::from_secs(1));
        assert_eq!(measured, None);
        assert_eq!(
            clock.last_latency_seconds(),
            0.0,
            "an unproposed block must leave the published latency untouched"
        );
    }

    #[test]
    fn commit_latency_ignores_a_sibling_that_wins_our_height() {
        use std::time::Duration;

        // We propose A at height 500; a sibling B commits at 500 instead. The
        // stamp for A must NOT be reported against B's commit. Keyed by height
        // this test goes red and the gauge publishes a fabricated latency.
        let t0 = Instant::now();
        let mut clock = ProposalClock::new();

        clock.note_proposed_at([0xaa; 32], 500, t0);
        let measured =
            clock.note_committed_at(&[0xbb; 32], 500, t0 + Duration::from_millis(900));

        assert_eq!(measured, None, "an orphaned proposal must not be measured");
        assert_eq!(clock.last_latency_seconds(), 0.0);

        // And the orphan is reaped, not left to accumulate: its height is at or
        // below the committed frontier, so it can never commit.
        assert_eq!(clock.pending_len(), 0);
    }

    #[test]
    fn commit_latency_reaps_stamps_at_or_below_the_committed_frontier() {
        use std::time::Duration;

        let t0 = Instant::now();
        let mut clock = ProposalClock::new();

        clock.note_proposed_at([0x01; 32], 500, t0);
        clock.note_proposed_at([0x02; 32], 504, t0 + Duration::from_millis(10));
        clock.note_proposed_at([0x03; 32], 508, t0 + Duration::from_millis(20));
        assert_eq!(clock.pending_len(), 3);

        // Committing 504 retires 500 and 504 and leaves 508 in flight.
        clock.note_committed_at(&[0x02; 32], 504, t0 + Duration::from_millis(500));
        assert_eq!(clock.pending_len(), 1);

        // 508 still resolves normally afterwards.
        let measured =
            clock.note_committed_at(&[0x03; 32], 508, t0 + Duration::from_millis(600));
        assert_eq!(measured, Some(Duration::from_millis(580)));
        assert_eq!(clock.pending_len(), 0);
    }

    #[test]
    fn commit_latency_pending_map_is_capacity_bounded_under_a_view_change_storm() {
        use std::time::Duration;

        // The leader is (height + round) % validators, so while the chain is
        // stuck at one height with rounds churning we become leader again every
        // fourth ROUND and emit a distinct block, at the same height, every
        // time. Only one can ever commit. Height-based reaping alone never
        // fires here because the committed frontier does not move, so the cap
        // is the only thing standing between a stall and an unbounded map.
        let t0 = Instant::now();
        let mut clock = ProposalClock::new_with_capacity(16);

        for round in 0..1_000u64 {
            let mut hash = [0u8; 32];
            hash[..8].copy_from_slice(&round.to_be_bytes());
            clock.note_proposed_at(hash, 500, t0 + Duration::from_millis(round));
        }

        assert!(
            clock.pending_len() <= 16,
            "pending stamps must stay under the cap, got {}",
            clock.pending_len()
        );
    }

    // ==========================================================================
    // Gate G0: the block interval gauge
    //
    // Every throughput ceiling in the plan divides by a block rate, and the
    // only defensible block rate measurement the project has is a long-window
    // height delta over a wall-clock delta: 158,321 blocks in 39,600 seconds.
    // A gauge that reports the gap between two consecutive commits is NOT that
    // number, because catch-up commits a burst of blocks in microseconds and a
    // stalled leader commits none for a second; the instantaneous gap describes
    // neither steady state nor anything reproducible a week later.
    //
    // So the definition under test is: mean seconds per committed block over a
    // trailing wall-clock window, computed as elapsed / height-delta over the
    // scrape samples retained in that window. That is arithmetically identical
    // to the ground-truth measurement, which is what makes the gauge checkable
    // against it rather than merely plausible.
    // ==========================================================================

    #[test]
    fn block_rate_reproduces_the_measured_fleet_ground_truth() {
        use std::time::Duration;

        // The cleanest block rate number the project has: 158,321 blocks over
        // 39,600 seconds of unloaded steady state, measured on the live fleet.
        // The window is set to that span so the gauge is asked the same
        // question the operator asked the journal.
        let t0 = Instant::now();
        let mut clock = BlockRateClock::new_with(Duration::from_secs(39_600), 1024);

        clock.observe_at(1_000_000, t0);
        let rate = clock.observe_at(1_000_000 + 158_321, t0 + Duration::from_secs(39_600));

        // 39,600 / 158,321 = 0.250125... seconds per block, i.e. 3.998 bps.
        assert!(
            (rate.interval_seconds - 0.250_125).abs() < 1e-6,
            "expected the ground-truth 0.250125 s/block, got {}",
            rate.interval_seconds
        );
        assert!(
            (1.0 / rate.interval_seconds - 3.998).abs() < 0.001,
            "expected the ground-truth 4.0 bps, got {}",
            1.0 / rate.interval_seconds
        );
        // The numerator and denominator are published too, so a reader a week
        // later can confirm what was divided by what instead of trusting the
        // quotient.
        assert!((rate.window_seconds - 39_600.0).abs() < 1e-6);
        assert_eq!(rate.window_blocks, 158_321);
    }

    #[test]
    fn block_rate_is_independent_of_scrape_cadence() {
        use std::time::Duration;

        // Two runs a week apart must be comparable, and the scrape cadence is
        // not a chain property. The same true rate sampled at 10 s, 30 s and
        // 60 s must report the same interval. A gauge defined as "the gap
        // between the last two scrapes" would report the cadence itself here
        // and pass nothing.
        for cadence in [10u64, 30, 60] {
            let t0 = Instant::now();
            let mut clock = BlockRateClock::new_with(Duration::from_secs(300), 1024);
            let mut height = 5_000_000u64;
            let mut rate = BlockRate::default();
            // Four full windows so the eviction path is exercised, not just
            // the fill path.
            for step in 0..(1200 / cadence) {
                height += 4 * cadence; // exactly 4 blocks per second
                rate = clock.observe_at(height, t0 + Duration::from_secs((step + 1) * cadence));
            }
            assert!(
                (rate.interval_seconds - 0.25).abs() < 1e-9,
                "cadence {cadence}s must report 0.25 s/block, got {}",
                rate.interval_seconds
            );
        }
    }

    #[test]
    fn block_rate_is_undefined_before_two_samples_and_while_the_height_is_flat() {
        use std::time::Duration;

        let t0 = Instant::now();
        let mut clock = BlockRateClock::new_with(Duration::from_secs(300), 1024);

        // One sample cannot define an interval. Reporting anything but zero
        // here would be a fabricated number on a fresh node.
        let first = clock.observe_at(900, t0);
        assert_eq!(first.interval_seconds, 0.0);
        assert_eq!(first.window_blocks, 0);
        assert_eq!(first.window_seconds, 0.0);

        // A stalled chain has a defined window and zero blocks in it. The
        // interval is undefined, NOT zero-seconds-per-block, and the caller
        // disambiguates stalled from warming up with
        // novai_seconds_since_last_commit, which keeps counting up.
        let stalled = clock.observe_at(900, t0 + Duration::from_secs(60));
        assert_eq!(stalled.interval_seconds, 0.0);
        assert_eq!(stalled.window_blocks, 0);
        assert!((stalled.window_seconds - 60.0).abs() < 1e-9);
    }

    #[test]
    fn block_rate_window_is_bounded_and_forgets_a_stale_rate() {
        use std::time::Duration;

        // The window must be trailing, not cumulative. A node that ran slowly
        // for an hour and then sped up must report the NEW rate, or every
        // before-and-after measurement in the plan is contaminated by the
        // before.
        let t0 = Instant::now();
        let mut clock = BlockRateClock::new_with(Duration::from_secs(300), 1024);

        // 3600 s at 1 bps.
        let mut height = 0u64;
        for step in 1..=120u64 {
            height += 30;
            clock.observe_at(height, t0 + Duration::from_secs(step * 30));
        }
        // Then 600 s at 8 bps.
        let mut rate = BlockRate::default();
        for step in 1..=20u64 {
            height += 240;
            rate = clock.observe_at(height, t0 + Duration::from_secs(3600 + step * 30));
        }

        assert!(
            rate.window_seconds <= 300.0,
            "the window must stay bounded at 300 s, got {}",
            rate.window_seconds
        );
        assert!(
            (rate.interval_seconds - 0.125).abs() < 1e-9,
            "the trailing window must report the new 8 bps rate, got {} s/block",
            rate.interval_seconds
        );
    }

    #[test]
    fn block_rate_discards_the_window_when_the_height_regresses() {
        use std::time::Duration;

        // A snapshot install or a chain reset moves the committed height
        // backwards. The retained samples then describe a different chain, and
        // a saturating subtraction would silently report a near-zero block
        // count over a full window, i.e. a fabricated stall.
        let t0 = Instant::now();
        let mut clock = BlockRateClock::new_with(Duration::from_secs(300), 1024);

        clock.observe_at(6_000_000, t0);
        clock.observe_at(6_000_120, t0 + Duration::from_secs(30));

        // Reset: the node comes back far below where it was.
        let after = clock.observe_at(1_900_000, t0 + Duration::from_secs(60));
        assert_eq!(
            after.window_blocks, 0,
            "a height regression must discard the pre-regression samples"
        );
        assert_eq!(after.window_seconds, 0.0);
        assert_eq!(after.interval_seconds, 0.0);

        // And it rebuilds from the new chain without carrying the old span.
        let rebuilt = clock.observe_at(1_900_120, t0 + Duration::from_secs(90));
        assert_eq!(rebuilt.window_blocks, 120);
        assert!((rebuilt.interval_seconds - 0.25).abs() < 1e-9);
    }

    #[test]
    fn block_rate_sample_buffer_is_capacity_bounded() {
        use std::time::Duration;

        // A scraper hammering /metrics must not grow the sample buffer without
        // bound. The cap costs window span, and the published window_seconds is
        // what makes that visible rather than silent.
        let t0 = Instant::now();
        let mut clock = BlockRateClock::new_with(Duration::from_secs(300), 8);
        let mut rate = BlockRate::default();
        for step in 1..=200u64 {
            rate = clock.observe_at(step * 4, t0 + Duration::from_millis(step * 100));
        }
        assert_eq!(clock.sample_count(), 8, "the buffer must stay at its cap");
        // 8 samples at 100 ms apart span 700 ms and 28 blocks.
        assert!((rate.window_seconds - 0.7).abs() < 1e-9);
        assert_eq!(rate.window_blocks, 28);
    }

    #[test]
    fn test_commit_clock_starts_at_boot_for_a_never_committing_node() {
        use std::time::Duration;

        // A node that boots and never commits reports a growing age from
        // boot: stalled-from-boot is alarm-worthy, not a blind spot.
        let t0 = Instant::now();
        let mut clock = CommitClock::new_at(t0);
        assert_eq!(clock.observe_at(0, t0 + Duration::from_secs(45)), 45);
    }
}
