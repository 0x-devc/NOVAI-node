"""
PURPOSE: Pure-function alert evaluators for the NOVAI metrics monitor.
Each evaluator takes a snapshot dict (current scrape), an optional previous
snapshot, an optional list of recent (timestamp, snapshot) history points,
and returns whether the alert condition is currently true plus a short
detail string used in the outbound notification body.

INVARIANTS:
- Evaluators are pure. No I/O, no logging, no mutation of inputs.
- An evaluator that cannot decide (missing metric, insufficient history)
  returns firing=False with detail="insufficient_data". The orchestrator
  treats this as a non-firing observation, not an alert.
- Counter resets (value decreases between samples) are absorbed by
  compute_counter_rate_per_minute as zero-increment intervals, so a
  legitimate node restart does not trigger a false spike alert.

FAILURE MODES:
- A metric absent from the snapshot dict is treated as missing, not zero.
  This matters for counters where 0 is a meaningful value.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Callable, Dict, List, Optional, Tuple

# novai_anomaly_last_confidence is the raw byte (0..=255) emitted by the
# copilot anomaly observer, not a 0.0..=1.0 float. Threshold 0.8 == byte 204.
# See crates/node/src/metrics.rs HELP text and crates/copilot/src/observer.rs.
ANOMALY_CONFIDENCE_BYTE_HIGH = 204

# 4-node BFT cluster: quorum is 2f+1 with f=1, so 3 peers is the floor.
PEER_QUORUM_FLOOR = 3
PEER_FULL_SET = 4

MEMPOOL_EMPTY_VALUE = 0
MEMPOOL_BACKLOG_THRESHOLD = 1000

VIEW_CHANGE_SPIKE_PER_MIN = 6.0
VIEW_CHANGE_ELEVATED_PER_MIN = 2.0

COPILOT_HEARTBEAT_RATE_FLOOR = 0.001  # observations per minute below this is "dead"

HistoryPoint = Tuple[float, Dict[str, float]]
History = List[HistoryPoint]


@dataclass(frozen=True)
class AlertSpec:
    alert_id: str
    severity: str   # "CRITICAL" or "WARN"
    window_secs: float
    summary: str
    playbook: Optional[str]  # filename under docs/playbooks/, or None


@dataclass(frozen=True)
class EvalResult:
    firing: bool
    detail: str


def compute_counter_rate_per_minute(
    history: History,
    metric_name: str,
    window_secs: float,
    now: float,
) -> Optional[float]:
    """
    Rate of counter increase per minute over the trailing window_secs.
    Returns None when history is empty or the window contains under 2 samples.
    Negative deltas are treated as counter resets (skipped, not subtracted) so
    a node restart does not produce a fake spike.
    """
    window_start = now - window_secs
    points = [(ts, snap) for (ts, snap) in history if ts >= window_start and metric_name in snap]
    if len(points) < 2:
        return None
    first_ts, _ = points[0]
    last_ts, _ = points[-1]
    elapsed = last_ts - first_ts
    if elapsed <= 0:
        return None
    total_increment = 0.0
    for (a_ts, a_snap), (b_ts, b_snap) in zip(points[:-1], points[1:]):
        if metric_name not in a_snap or metric_name not in b_snap:
            continue
        delta = b_snap[metric_name] - a_snap[metric_name]
        if delta >= 0:
            total_increment += delta
    return (total_increment / elapsed) * 60.0


def _get(snap: Optional[Dict[str, float]], name: str) -> Optional[float]:
    if snap is None:
        return None
    return snap.get(name)


def eval_block_height_stuck(snap, prev, history, now) -> EvalResult:
    cur = _get(snap, "novai_committed_height")
    prv = _get(prev, "novai_committed_height")
    if cur is None or prv is None:
        return EvalResult(False, "insufficient_data")
    if cur == prv:
        return EvalResult(True, f"height={int(cur)} (no change)")
    return EvalResult(False, f"height={int(cur)}")


def eval_peer_count_below_quorum(snap, prev, history, now) -> EvalResult:
    peers = _get(snap, "novai_peer_count")
    if peers is None:
        return EvalResult(False, "insufficient_data")
    if peers < PEER_QUORUM_FLOOR:
        return EvalResult(True, f"peers={int(peers)} (quorum needs {PEER_QUORUM_FLOOR})")
    return EvalResult(False, f"peers={int(peers)}")


def eval_peer_count_degraded(snap, prev, history, now) -> EvalResult:
    peers = _get(snap, "novai_peer_count")
    if peers is None:
        return EvalResult(False, "insufficient_data")
    if peers < PEER_FULL_SET:
        return EvalResult(True, f"peers={int(peers)}/{PEER_FULL_SET}")
    return EvalResult(False, f"peers={int(peers)}")


def eval_mempool_empty(snap, prev, history, now) -> EvalResult:
    size = _get(snap, "novai_mempool_size")
    if size is None:
        return EvalResult(False, "insufficient_data")
    if size <= MEMPOOL_EMPTY_VALUE:
        return EvalResult(True, f"mempool_size={int(size)}")
    return EvalResult(False, f"mempool_size={int(size)}")


def eval_mempool_backlog(snap, prev, history, now) -> EvalResult:
    size = _get(snap, "novai_mempool_size")
    if size is None:
        return EvalResult(False, "insufficient_data")
    if size > MEMPOOL_BACKLOG_THRESHOLD:
        return EvalResult(True, f"mempool_size={int(size)} (above {MEMPOOL_BACKLOG_THRESHOLD})")
    return EvalResult(False, f"mempool_size={int(size)}")


def eval_view_change_spike(snap, prev, history, now) -> EvalResult:
    # Rate measured over the trailing 3 minutes so the loop's poll interval does not whipsaw it.
    rate = compute_counter_rate_per_minute(history, "novai_consensus_view_changes_total", 180.0, now)
    if rate is None:
        return EvalResult(False, "insufficient_data")
    if rate > VIEW_CHANGE_SPIKE_PER_MIN:
        return EvalResult(True, f"view_changes={rate:.2f}/min (above {VIEW_CHANGE_SPIKE_PER_MIN}/min)")
    return EvalResult(False, f"view_changes={rate:.2f}/min")


def eval_view_change_elevated(snap, prev, history, now) -> EvalResult:
    rate = compute_counter_rate_per_minute(history, "novai_consensus_view_changes_total", 300.0, now)
    if rate is None:
        return EvalResult(False, "insufficient_data")
    if rate > VIEW_CHANGE_ELEVATED_PER_MIN:
        return EvalResult(True, f"view_changes={rate:.2f}/min (above {VIEW_CHANGE_ELEVATED_PER_MIN}/min)")
    return EvalResult(False, f"view_changes={rate:.2f}/min")


def eval_anomaly_published(snap, prev, history, now) -> EvalResult:
    cur = _get(snap, "novai_anomaly_signals_published")
    prv = _get(prev, "novai_anomaly_signals_published")
    if cur is None or prv is None:
        return EvalResult(False, "insufficient_data")
    delta = cur - prv
    if delta > 0:
        return EvalResult(True, f"published={int(cur)} (+{int(delta)} since last scrape)")
    return EvalResult(False, f"published={int(cur)}")


def eval_anomaly_high_confidence(snap, prev, history, now) -> EvalResult:
    val = _get(snap, "novai_anomaly_last_confidence")
    if val is None:
        return EvalResult(False, "insufficient_data")
    if val > ANOMALY_CONFIDENCE_BYTE_HIGH:
        return EvalResult(True, f"confidence_byte={int(val)} (above {ANOMALY_CONFIDENCE_BYTE_HIGH}, ~0.8)")
    return EvalResult(False, f"confidence_byte={int(val)}")


def eval_copilot_heartbeat_dead(snap, prev, history, now) -> EvalResult:
    rate = compute_counter_rate_per_minute(history, "novai_copilot_observations_total", 600.0, now)
    if rate is None:
        return EvalResult(False, "insufficient_data")
    if rate <= COPILOT_HEARTBEAT_RATE_FLOOR:
        return EvalResult(True, f"copilot_rate={rate:.4f}/min (heartbeat stopped)")
    return EvalResult(False, f"copilot_rate={rate:.4f}/min")


def eval_proposer_skipping_txs(snap, prev, history, now) -> EvalResult:
    """
    Empty blocks while mempool has txs queued: total_txs_committed flat over
    a window AND mempool_size > 0 right now. Implies the proposer is producing
    blocks (committed_height advancing covered by other alerts) but not
    including queued transactions.
    """
    mempool = _get(snap, "novai_mempool_size")
    if mempool is None or mempool <= 0:
        return EvalResult(False, f"mempool_size={int(mempool) if mempool is not None else 0}")
    tx_rate = compute_counter_rate_per_minute(history, "novai_total_txs_committed", 120.0, now)
    if tx_rate is None:
        return EvalResult(False, "insufficient_data")
    if tx_rate <= 0.0:
        return EvalResult(True, f"tx_rate=0/min mempool_size={int(mempool)}")
    return EvalResult(False, f"tx_rate={tx_rate:.2f}/min mempool_size={int(mempool)}")


Evaluator = Callable[[Dict[str, float], Optional[Dict[str, float]], History, float], EvalResult]


ALERTS: List[Tuple[AlertSpec, Evaluator]] = [
    (AlertSpec("block_height_stuck", "CRITICAL", 30.0,
               "Block height has not advanced",
               "EMERGENCY_FREEZE.md"), eval_block_height_stuck),
    (AlertSpec("peer_count_below_quorum", "CRITICAL", 60.0,
               "Peer count below BFT quorum",
               "VALIDATOR_COMPROMISE.md"), eval_peer_count_below_quorum),
    (AlertSpec("peer_count_degraded", "WARN", 120.0,
               "Validator set incomplete",
               "VALIDATOR_COMPROMISE.md"), eval_peer_count_degraded),
    (AlertSpec("mempool_empty", "WARN", 300.0,
               "Mempool empty: tx flow regression",
               None), eval_mempool_empty),
    (AlertSpec("mempool_backlog", "WARN", 300.0,
               "Mempool above backlog threshold",
               None), eval_mempool_backlog),
    (AlertSpec("view_change_spike", "CRITICAL", 180.0,
               "View change rate spiking",
               None), eval_view_change_spike),
    (AlertSpec("view_change_elevated", "WARN", 300.0,
               "View change rate elevated",
               None), eval_view_change_elevated),
    (AlertSpec("anomaly_published", "WARN", 0.0,
               "New anomaly signal published on-chain",
               "ROLLBACK_BAD_MODULE.md"), eval_anomaly_published),
    (AlertSpec("anomaly_high_confidence", "CRITICAL", 0.0,
               "High-confidence anomaly detected",
               "ROLLBACK_BAD_MODULE.md"), eval_anomaly_high_confidence),
    (AlertSpec("copilot_heartbeat_dead", "WARN", 600.0,
               "Copilot observation loop stopped",
               None), eval_copilot_heartbeat_dead),
    (AlertSpec("proposer_skipping_txs", "WARN", 120.0,
               "Proposer producing empty blocks while mempool has txs",
               None), eval_proposer_skipping_txs),
]

# A13 (metrics_endpoint_unreachable) is handled by the loop itself, not as an
# evaluator over a snapshot. Its spec is exposed here for the notifier.
UNREACHABLE_SPEC = AlertSpec(
    alert_id="metrics_endpoint_unreachable",
    severity="WARN",
    window_secs=120.0,
    summary="Cannot reach metrics endpoint",
    playbook=None,
)
