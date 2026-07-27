"""
PURPOSE: Pure-function alert evaluators and pure cross-node predicates for the
NOVAI metrics monitor. Two kinds of logic live here:

1. Per-node evaluators (the NODE_ALERTS registry). Each takes a snapshot dict
   (current scrape), an optional previous snapshot, an optional list of recent
   (timestamp, snapshot) history points, and returns whether the alert
   condition is currently true plus a short detail string.

2. Cross-node pure predicates (node_stuck_fire, cluster_halt_fire,
   healthy_labels, fault_tolerance_state, classify_divergence) and host
   predicates (host_disk_*_fire, host_mem_low_fire). These take already
   gathered values and decide firing. The orchestrator in novai_monitor.py
   does the I/O (scraping the four nodes and the RPC), then calls these.

INVARIANTS:
- Everything here is pure. No I/O, no logging, no mutation of inputs.
- A per-node evaluator that cannot decide (missing metric, insufficient
  history) returns firing=False with detail="insufficient_data". The
  orchestrator treats this as a non-firing observation, not an alert.
- Counter resets (value decreases between samples) are absorbed by
  compute_counter_rate_per_minute as zero-increment intervals, so a
  legitimate node restart does not trigger a false spike alert.
- Quorum is decided ONLY from healthy validator count (height based), never
  from transport peer_count. peer_count drives a demoted diagnostic only.
- Divergence is decided by majority grouping of state_roots at a common
  height, never by comparing every node to a single reference node.

FAILURE MODES:
- A metric absent from the snapshot dict is treated as missing, not zero.
  This matters for counters where 0 is a meaningful value.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Callable, Dict, List, Optional, Set, Tuple

# novai_anomaly_last_confidence is the raw byte (0..=255) emitted by the
# copilot anomaly observer, not a 0.0..=1.0 float. Threshold 0.8 == byte 204.
# See crates/node/src/metrics.rs HELP text and crates/copilot/src/observer.rs.
ANOMALY_CONFIDENCE_BYTE_HIGH = 204

MEMPOOL_EMPTY_VALUE = 0
MEMPOOL_BACKLOG_THRESHOLD = 1000

VIEW_CHANGE_SPIKE_PER_MIN = 6.0
VIEW_CHANGE_ELEVATED_PER_MIN = 2.0

COPILOT_HEARTBEAT_RATE_FLOOR = 0.001  # observations per minute below this is "dead"

# Cross-node calibration. HEIGHT_SKEW_BLOCKS is the lag (cluster max minus this
# node) at or under which a reachable node still counts as healthy. Locked from
# a live healthy-cluster read this session: observed healthy tip skew was at
# most 1 block, so 5 is a safe margin.
HEIGHT_SKEW_BLOCKS = 5

# Compare state_roots this many blocks below the lowest tip, so a node that is
# one or two blocks behind is never mislabeled as diverged.
DIVERGENCE_DEPTH_BLOCKS = 2

# Host resource thresholds (percent free / available).
HOST_DISK_CRIT_PCT = 10.0
HOST_DISK_WARN_PCT = 20.0
HOST_MEM_WARN_PCT = 10.0

# Commit-stall dual trigger (incident WEDGE-20260718: commits froze while
# consensus certified 818,258 heights over five days, and nothing paged).
# The gap threshold is a quarter of the node's COMMIT_WINDOW (1024, see
# crates/consensus/src/lib.rs), so the page lands with headroom before the
# commit-window rule parks the fleet. The healthy commit-to-frontier gap
# under the 3-chain rule is 2 to 3 blocks REGARDLESS of block rate, so the
# gap trigger never false-fires as throughput grows. A fixed block-count
# threshold alone gives shrinking wall-clock warning as block rate rises
# (256 blocks is about 64 seconds at 4 blocks/s but about 10 seconds at 25
# blocks/s), so the 30 second clock is what keeps the warning wall-clock
# stable as the chain speeds up. test_commit_stall.py pins both values and
# the quarter-of-window relation.
COMMIT_GAP_RUNAWAY_BLOCKS = 256
COMMIT_STALL_SECS = 30.0

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


# ---------------------------------------------------------------------------
# Per-node evaluators (unchanged behavior, now evaluated once per node)
# ---------------------------------------------------------------------------

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


def eval_commit_stall(snap, prev, history, now) -> EvalResult:
    """
    Dual trigger, fires on EITHER condition:

    - gap trigger: novai_consensus_commit_gap above COMMIT_GAP_RUNAWAY_BLOCKS.
      Catches a runaway frontier (consensus certifying ahead of a stalled
      committed cursor) with headroom before the node-side commit-window
      rule parks the fleet at gap 1024.
    - time trigger: novai_seconds_since_last_commit above COMMIT_STALL_SECS.
      Catches a silent commit stall at any block rate; the node measures
      the age itself, so a monitor restart does not reset the clock.

    Either metric alone is decidable (the trigger whose metric is absent
    simply contributes nothing); with both metrics absent this is
    insufficient_data, never a page, per the file invariant that a missing
    metric is missing, not zero.
    """
    gap = _get(snap, "novai_consensus_commit_gap")
    secs = _get(snap, "novai_seconds_since_last_commit")
    if gap is None and secs is None:
        return EvalResult(False, "insufficient_data")
    gap_fires = gap is not None and gap > COMMIT_GAP_RUNAWAY_BLOCKS
    time_fires = secs is not None and secs > COMMIT_STALL_SECS
    parts = []
    if gap is not None:
        note = f" (above {COMMIT_GAP_RUNAWAY_BLOCKS})" if gap_fires else ""
        parts.append(f"gap={int(gap)}{note}")
    if secs is not None:
        note = f" (above {int(COMMIT_STALL_SECS)}s)" if time_fires else ""
        parts.append(f"secs_since_commit={int(secs)}{note}")
    return EvalResult(gap_fires or time_fires, " ".join(parts))


Evaluator = Callable[[Dict[str, float], Optional[Dict[str, float]], History, float], EvalResult]


# Per-node alerts. Evaluated once for every validator, with state keyed
# f"{alert_id}:{label}" so node0 and node1 fire and recover independently.
# block_height_stuck is retired (superseded by node_stuck + cluster_halt).
# peer_count_below_quorum and peer_count_degraded are retired as quorum
# signals (transport_peers_low keeps a demoted transport diagnostic).
NODE_ALERTS: List[Tuple[AlertSpec, Evaluator]] = [
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
    # WEDGE-20260718: pages on a runaway frontier OR a silent commit stall.
    # The 30 s debounce window stacks on the node-side 30 s clock, so the
    # time trigger pages about a minute after commits freeze; the gap
    # trigger pages within the same debounce of the gap crossing 256.
    (AlertSpec("commit_stall", "CRITICAL", 30.0,
               "Commit stall: frontier gap or commit age beyond bounds (dual trigger)",
               "EMERGENCY_FREEZE.md"), eval_commit_stall),
]


# ---------------------------------------------------------------------------
# Cross-node pure predicates
# ---------------------------------------------------------------------------

def node_stuck_fire(
    label: str,
    heights_now: Dict[str, float],
    heights_prev: Dict[str, float],
) -> bool:
    """
    True when this node's committed height did not advance between the previous
    and current scrape AND at least one other node did advance. The second
    clause is what makes this "this node is the problem" rather than a whole
    chain halt, and it keeps the alert silent during a genuine cluster halt
    (where no node advances) and during true idle (which does not happen at the
    chain level because the chain emits empty blocks on cadence).
    Needs a previous sample for this node; returns False without one.
    """
    if label not in heights_now or label not in heights_prev:
        return False
    node_flat = heights_now[label] <= heights_prev[label]
    others_advanced = any(
        other != label and other in heights_now and heights_now[other] > heights_prev[other]
        for other in heights_prev
    )
    return node_flat and others_advanced


def cluster_halt_fire(
    heights_now: Dict[str, float],
    heights_prev: Dict[str, float],
    full_count: int,
) -> bool:
    """
    True when no reachable node advanced its committed height between the
    previous and current scrape. Because a healthy chain emits empty blocks on
    cadence, a cluster whose max height does not move is halted, not idle.
    For a single configured node this reduces to "that node is not advancing".
    For two or more configured nodes I require at least two comparable nodes so
    a moment when the monitor can see only one node does not read as a halt.
    """
    comparable = [label for label in heights_prev if label in heights_now]
    min_needed = 1 if full_count <= 1 else 2
    if len(comparable) < min_needed:
        return False
    return all(heights_now[label] <= heights_prev[label] for label in comparable)


def healthy_labels(
    heights_now: Dict[str, float],
    reachable: Set[str],
    skew: int,
) -> Set[str]:
    """
    The set of reachable nodes whose committed height is within `skew` blocks of
    the current cluster max. This is the consensus-liveness view of "healthy",
    derived from height, not from transport peer_count.
    """
    healthy: Set[str] = set()
    if not heights_now:
        return healthy
    cluster_max = max(heights_now.values())
    for label in reachable:
        height = heights_now.get(label)
        if height is not None and (cluster_max - height) <= skew:
            healthy.add(label)
    return healthy


def fault_tolerance_state(healthy_count: int, full_count: int) -> Tuple[bool, bool, int]:
    """
    Return (degraded, critical, quorum) for a cluster of full_count validators
    with healthy_count of them keeping up. For the four-node cluster this is
    degraded when healthy_count == 3 (bare quorum, zero remaining fault
    tolerance) and critical when healthy_count < 3 (below quorum). The two
    bands are mutually exclusive for the four-node case.
    """
    quorum = (2 * full_count) // 3 + 1
    degraded = healthy_count == full_count - 1
    critical = healthy_count < quorum
    return degraded, critical, quorum


@dataclass(frozen=True)
class DivergenceVerdict:
    considered: int
    canonical: Optional[str]
    minority: Tuple[str, ...]
    is_split: bool


def classify_divergence(roots: Dict[str, str]) -> DivergenceVerdict:
    """
    Group the given {label: state_root} (already gathered at a common height,
    only for nodes that responded) by value, take the largest group as
    canonical, and flag every node outside it as a minority. If no group holds
    a strict majority (for example two versus two), there is no canonical root
    and is_split is True.

    This groups by value and takes the majority. It never compares every node
    to a single reference node, because if the reference were the forked node
    that would mislabel the honest majority as diverged.
    """
    labels = sorted(roots)
    if len(labels) < 2:
        return DivergenceVerdict(len(labels), None, (), False)
    groups: Dict[str, List[str]] = {}
    for label in labels:
        groups.setdefault(roots[label], []).append(label)
    # Deterministic pick when sizes are equal; the size tie is handled as a split below.
    canonical_root = max(groups, key=lambda root: (len(groups[root]), root))
    largest = len(groups[canonical_root])
    if largest * 2 <= len(labels):
        return DivergenceVerdict(len(labels), None, (), True)
    minority = tuple(sorted(label for label in labels if roots[label] != canonical_root))
    return DivergenceVerdict(len(labels), canonical_root, minority, False)


def host_disk_critical_fire(free_pct: Optional[float]) -> bool:
    return free_pct is not None and free_pct < HOST_DISK_CRIT_PCT


def host_disk_low_fire(free_pct: Optional[float]) -> bool:
    # The warning band sits above the critical band so the two never page at once.
    return free_pct is not None and HOST_DISK_CRIT_PCT <= free_pct < HOST_DISK_WARN_PCT


def host_mem_low_fire(avail_pct: Optional[float]) -> bool:
    return avail_pct is not None and avail_pct < HOST_MEM_WARN_PCT


# ---------------------------------------------------------------------------
# Cross-node, host, and transport alert specs.
# These are driven by the orchestrator in novai_monitor.py, which supplies the
# already gathered values to the predicates above. Per-node specs are keyed
# f"{alert_id}:{label}"; cluster-wide specs use the bare alert_id.
# ---------------------------------------------------------------------------

SPEC_NODE_STUCK = AlertSpec(
    "node_stuck", "CRITICAL", 90.0,
    "Validator committed height not advancing while the cluster advances",
    "EMERGENCY_FREEZE.md")

SPEC_CLUSTER_HALT = AlertSpec(
    "cluster_halt", "CRITICAL", 60.0,
    "Cluster committed height not advancing (consensus halted)",
    "EMERGENCY_FREEZE.md")

SPEC_FT_DEGRADED = AlertSpec(
    "fault_tolerance_degraded", "WARN", 60.0,
    "Cluster at bare quorum: zero remaining fault tolerance",
    "VALIDATOR_COMPROMISE.md")

SPEC_FT_CRITICAL = AlertSpec(
    "fault_tolerance_critical", "CRITICAL", 30.0,
    "Healthy validator count below quorum",
    "VALIDATOR_COMPROMISE.md")

SPEC_DIVERGENCE = AlertSpec(
    "state_root_divergence", "CRITICAL", 60.0,
    "Validator state_root differs from cluster majority at a common height",
    "EMERGENCY_FREEZE.md")

SPEC_DIVERGENCE_SPLIT = AlertSpec(
    "divergence_split", "CRITICAL", 60.0,
    "No majority state_root across the cluster (split)",
    "EMERGENCY_FREEZE.md")

SPEC_NODE_UNREACHABLE = AlertSpec(
    "node_unreachable", "WARN", 120.0,
    "Validator metrics endpoint unreachable",
    None)

SPEC_HOST_DISK_CRIT = AlertSpec(
    "host_disk_critical", "CRITICAL", 120.0,
    "Host disk free below 10 percent",
    None)

SPEC_HOST_DISK_LOW = AlertSpec(
    "host_disk_low", "WARN", 120.0,
    "Host disk free below 20 percent",
    None)

SPEC_HOST_MEM_LOW = AlertSpec(
    "host_mem_low", "WARN", 120.0,
    "Host memory available below 10 percent",
    None)

# Demoted former peer_count alert. This is a TRANSPORT diagnostic only and is
# explicitly NOT a quorum signal. Quorum is owned by fault_tolerance_* above,
# which counts healthy validators from height. A node can show a full transport
# peer count while being consensus-dead, which is exactly why peer_count must
# never gate quorum.
SPEC_TRANSPORT_PEERS_LOW = AlertSpec(
    "transport_peers_low", "WARN", 120.0,
    "Transport peer links below expected (transport diagnostic only, not a quorum signal)",
    None)
