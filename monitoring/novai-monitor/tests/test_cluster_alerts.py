"""
Tests for the cross-node alerting: the pure predicates (divergence, healthy
count, node stuck, cluster halt, host thresholds) and end-to-end orchestration
scenarios that drive real FIRE transitions through the reused state machine.

Multi-node fixtures are built as per-node dict snapshots in code, matching the
existing dict-literal convention in test_alerts.py. The Prometheus text parse
path is covered separately in test_parser.py, so these tests construct the
parsed snapshots directly.
"""
import novai_monitor as nm
from alerts import (
    HEIGHT_SKEW_BLOCKS,
    classify_divergence,
    cluster_halt_fire,
    fault_tolerance_state,
    healthy_labels,
    host_disk_critical_fire,
    host_disk_low_fire,
    host_mem_low_fire,
    node_stuck_fire,
)


# ---------------------------------------------------------------------------
# classify_divergence (majority grouping, never first-responder)
# ---------------------------------------------------------------------------

def test_divergence_all_agree_is_silent():
    v = classify_divergence({"node0": "A", "node1": "A", "node2": "A", "node3": "A"})
    assert v.minority == ()
    assert v.is_split is False
    assert v.canonical == "A"


def test_divergence_three_to_one_flags_only_the_minority():
    v = classify_divergence({"node0": "A", "node1": "A", "node2": "A", "node3": "B"})
    assert v.minority == ("node3",)
    assert v.is_split is False
    assert v.canonical == "A"


def test_divergence_uses_majority_not_first_responder():
    # node0 is the lone odd root; the three agreeing nodes are canonical.
    # A naive "compare every node to the first responder" would instead flag
    # node1..node3 against node0, mislabeling the honest majority. This guards
    # against exactly that.
    v = classify_divergence({"node0": "X", "node1": "Y", "node2": "Y", "node3": "Y"})
    assert v.canonical == "Y"
    assert v.minority == ("node0",)
    assert v.is_split is False


def test_divergence_two_two_is_split_with_no_canonical():
    v = classify_divergence({"node0": "A", "node1": "A", "node2": "B", "node3": "B"})
    assert v.is_split is True
    assert v.minority == ()
    assert v.canonical is None


def test_divergence_two_agree_is_silent():
    v = classify_divergence({"node0": "A", "node1": "A"})
    assert v.is_split is False
    assert v.minority == ()


def test_divergence_single_responder_is_inconclusive():
    v = classify_divergence({"node0": "A"})
    assert v.is_split is False
    assert v.minority == ()
    assert v.considered == 1


# ---------------------------------------------------------------------------
# healthy_labels (height-based liveness, not transport)
# ---------------------------------------------------------------------------

def test_healthy_labels_all_within_skew():
    heights = {"node0": 200.0, "node1": 200.0, "node2": 199.0, "node3": 201.0}
    healthy = healthy_labels(heights, set(heights), HEIGHT_SKEW_BLOCKS)
    assert healthy == {"node0", "node1", "node2", "node3"}


def test_healthy_labels_excludes_laggard_beyond_skew():
    heights = {"node0": 200.0, "node1": 200.0, "node2": 200.0, "node3": 100.0}
    healthy = healthy_labels(heights, set(heights), HEIGHT_SKEW_BLOCKS)
    assert "node3" not in healthy
    assert len(healthy) == 3


def test_healthy_labels_excludes_unreachable():
    heights = {"node0": 200.0, "node1": 200.0, "node2": 200.0}
    reachable = {"node0", "node1", "node2"}  # node3 unreachable, absent from heights
    healthy = healthy_labels(heights, reachable, HEIGHT_SKEW_BLOCKS)
    assert "node3" not in healthy
    assert len(healthy) == 3


# ---------------------------------------------------------------------------
# fault_tolerance_state (4-node cluster)
# ---------------------------------------------------------------------------

def test_fault_tolerance_four_healthy_is_clean():
    degraded, critical, quorum = fault_tolerance_state(4, 4)
    assert degraded is False
    assert critical is False
    assert quorum == 3


def test_fault_tolerance_three_healthy_is_degraded_not_critical():
    degraded, critical, _ = fault_tolerance_state(3, 4)
    assert degraded is True
    assert critical is False


def test_fault_tolerance_two_healthy_is_critical_not_degraded():
    degraded, critical, _ = fault_tolerance_state(2, 4)
    assert critical is True
    assert degraded is False


def test_fault_tolerance_zero_healthy_is_critical():
    _, critical, _ = fault_tolerance_state(0, 4)
    assert critical is True


# ---------------------------------------------------------------------------
# node_stuck_fire and cluster_halt_fire
# ---------------------------------------------------------------------------

def test_node_stuck_fires_when_flat_and_others_advance():
    now = {"node0": 100.0, "node1": 110.0, "node2": 110.0, "node3": 110.0}
    prev = {"node0": 100.0, "node1": 100.0, "node2": 100.0, "node3": 100.0}
    assert node_stuck_fire("node0", now, prev) is True


def test_node_stuck_silent_when_all_flat():
    now = {"node0": 100.0, "node1": 100.0}
    prev = {"node0": 100.0, "node1": 100.0}
    assert node_stuck_fire("node0", now, prev) is False


def test_node_stuck_silent_when_node_advances():
    now = {"node0": 105.0, "node1": 110.0}
    prev = {"node0": 100.0, "node1": 100.0}
    assert node_stuck_fire("node0", now, prev) is False


def test_node_stuck_needs_a_previous_sample():
    now = {"node0": 100.0, "node1": 110.0}
    prev = {"node1": 100.0}  # no prev for node0
    assert node_stuck_fire("node0", now, prev) is False


def test_cluster_halt_fires_when_no_node_advances():
    now = {"node0": 100.0, "node1": 100.0, "node2": 100.0, "node3": 100.0}
    prev = {"node0": 100.0, "node1": 100.0, "node2": 100.0, "node3": 100.0}
    assert cluster_halt_fire(now, prev, 4) is True


def test_cluster_halt_silent_when_a_node_advances():
    now = {"node0": 100.0, "node1": 101.0}
    prev = {"node0": 100.0, "node1": 100.0}
    assert cluster_halt_fire(now, prev, 4) is False


def test_cluster_halt_single_node_flat_fires():
    assert cluster_halt_fire({"node0": 100.0}, {"node0": 100.0}, 1) is True


def test_cluster_halt_single_node_advancing_is_silent():
    assert cluster_halt_fire({"node0": 101.0}, {"node0": 100.0}, 1) is False


def test_cluster_halt_needs_two_comparable_for_multinode():
    # Only one node visible in a 4-node config cannot declare a cluster halt.
    assert cluster_halt_fire({"node0": 100.0}, {"node0": 100.0}, 4) is False


# ---------------------------------------------------------------------------
# host thresholds
# ---------------------------------------------------------------------------

def test_host_disk_critical_band():
    assert host_disk_critical_fire(5.0) is True
    assert host_disk_critical_fire(15.0) is False
    assert host_disk_critical_fire(None) is False


def test_host_disk_low_band_sits_between_ten_and_twenty():
    assert host_disk_low_fire(15.0) is True
    assert host_disk_low_fire(5.0) is False    # below 10 is the critical band, not the warn band
    assert host_disk_low_fire(25.0) is False
    assert host_disk_low_fire(None) is False


def test_host_mem_low():
    assert host_mem_low_fire(5.0) is True
    assert host_mem_low_fire(50.0) is False
    assert host_mem_low_fire(None) is False


# ---------------------------------------------------------------------------
# Orchestration scenarios: drive real FIRE transitions through the reused
# state machine, capturing dispatched alerts.
# ---------------------------------------------------------------------------

def _snap(height, peers=3):
    return {"novai_committed_height": float(height), "novai_peer_count": float(peers)}


def _make_monitor(monkeypatch, recorded, node_count=4, divergence=True):
    nodes = [
        nm.NodeEndpoint(f"node{i}", f"http://localhost:{8080 + i}/metrics", f"http://localhost:{3030 + i}")
        for i in range(node_count)
    ]
    cfg = nm.Config(
        nodes=nodes,
        metrics_user="",
        metrics_pass="",
        poll_interval_secs=30.0,
        http_timeout_secs=5.0,
        rearm_grace_secs=120.0,
        telegram_bot_token="token",
        telegram_chat_id="chat",
        log_level="INFO",
        env_label="test",
        undelivered_path="/tmp/novai_monitor_test_undelivered.jsonl",
        divergence_enabled=divergence,
        host_checks_enabled=False,
        disk_path="/",
    )
    mon = nm.Monitor(cfg, dry_run=True)
    mon.startup_ts = 0.0  # so the re-arm grace never suppresses in these tests

    def fake_dispatch(_cfg, spec, transition, detail, _now, _dry_run):
        recorded.append((spec.alert_id, transition, detail))

    monkeypatch.setattr(nm, "dispatch_transition", fake_dispatch)
    return mon


def _fired(recorded, alert_id, node=None):
    for aid, transition, detail in recorded:
        if aid == alert_id and transition == "FIRE" and (node is None or f"node={node}" in detail):
            return True
    return False


def _run_two_frames(mon, frame_a, frame_b):
    """Each frame is (prev_heights_snaps, snaps, reachable, roots, now)."""
    for prev, snaps, reachable, roots, now in (frame_a, frame_b):
        mon.prev_snapshot = prev
        mon._evaluate_cluster_alerts(snaps, reachable, roots, now)


def test_scenario_healthy_four_node_fires_nothing(monkeypatch):
    recorded = []
    mon = _make_monitor(monkeypatch, recorded)
    same_roots = {f"node{i}": "0xsame" for i in range(4)}
    frame_a = (
        {f"node{i}": _snap(100) for i in range(4)},
        {f"node{i}": _snap(110) for i in range(4)},
        {f"node{i}" for i in range(4)},
        same_roots,
        1000.0,
    )
    frame_b = (
        {f"node{i}": _snap(110) for i in range(4)},
        {f"node{i}": _snap(120) for i in range(4)},
        {f"node{i}" for i in range(4)},
        same_roots,
        1200.0,
    )
    _run_two_frames(mon, frame_a, frame_b)
    fires = [r for r in recorded if r[1] == "FIRE"]
    assert fires == [], f"healthy cluster should fire nothing, got {fires}"


def test_scenario_one_node_stuck_fires_node_stuck_not_cluster_halt(monkeypatch):
    recorded = []
    mon = _make_monitor(monkeypatch, recorded)
    frame_a = (
        {f"node{i}": _snap(100) for i in range(4)},
        {"node0": _snap(100), "node1": _snap(110), "node2": _snap(110), "node3": _snap(110)},
        {"node0", "node1", "node2", "node3"},
        {},
        1000.0,
    )
    frame_b = (
        {"node0": _snap(100), "node1": _snap(110), "node2": _snap(110), "node3": _snap(110)},
        {"node0": _snap(100), "node1": _snap(120), "node2": _snap(120), "node3": _snap(120)},
        {"node0", "node1", "node2", "node3"},
        {},
        1091.0,  # past the 90s node_stuck window
    )
    _run_two_frames(mon, frame_a, frame_b)
    assert _fired(recorded, "node_stuck", "node0")
    assert not _fired(recorded, "node_stuck", "node1")
    assert not _fired(recorded, "cluster_halt")
    assert not _fired(recorded, "fault_tolerance_critical")
    # A stuck node also drops the cluster to bare quorum, which is correct.
    assert _fired(recorded, "fault_tolerance_degraded")


def test_scenario_cluster_halt_fires_only_cluster_halt(monkeypatch):
    recorded = []
    mon = _make_monitor(monkeypatch, recorded)
    flat = {f"node{i}": _snap(100) for i in range(4)}
    frame_a = (flat, flat, {f"node{i}" for i in range(4)}, {}, 1000.0)
    frame_b = (flat, flat, {f"node{i}" for i in range(4)}, {}, 1061.0)  # past the 60s window
    _run_two_frames(mon, frame_a, frame_b)
    assert _fired(recorded, "cluster_halt")
    assert not _fired(recorded, "node_stuck")
    assert not _fired(recorded, "fault_tolerance_degraded")
    assert not _fired(recorded, "fault_tolerance_critical")


def test_scenario_one_validator_lagging_fires_fault_tolerance_degraded(monkeypatch):
    recorded = []
    mon = _make_monitor(monkeypatch, recorded)
    # node3 keeps advancing but stays far behind the tip, so it is not "stuck"
    # yet it is not healthy either: healthy count is 3, bare quorum.
    frame_a = (
        {"node0": _snap(200), "node1": _snap(200), "node2": _snap(200), "node3": _snap(100)},
        {"node0": _snap(210), "node1": _snap(210), "node2": _snap(210), "node3": _snap(105)},
        {"node0", "node1", "node2", "node3"},
        {},
        1000.0,
    )
    frame_b = (
        {"node0": _snap(210), "node1": _snap(210), "node2": _snap(210), "node3": _snap(105)},
        {"node0": _snap(220), "node1": _snap(220), "node2": _snap(220), "node3": _snap(110)},
        {"node0", "node1", "node2", "node3"},
        {},
        1061.0,
    )
    _run_two_frames(mon, frame_a, frame_b)
    assert _fired(recorded, "fault_tolerance_degraded")
    assert not _fired(recorded, "fault_tolerance_critical")
    assert not _fired(recorded, "node_stuck", "node3")  # it advances, so not stuck
    assert not _fired(recorded, "cluster_halt")


def test_scenario_node_unreachable_fires_per_node(monkeypatch):
    recorded = []
    mon = _make_monitor(monkeypatch, recorded)
    reachable = {"node0", "node1", "node2"}  # node3 absent (unreachable)
    frame_a = (
        {"node0": _snap(100), "node1": _snap(100), "node2": _snap(100)},
        {"node0": _snap(110), "node1": _snap(110), "node2": _snap(110)},
        reachable,
        {},
        1000.0,
    )
    frame_b = (
        {"node0": _snap(110), "node1": _snap(110), "node2": _snap(110)},
        {"node0": _snap(120), "node1": _snap(120), "node2": _snap(120)},
        reachable,
        {},
        1121.0,  # past the 120s node_unreachable window
    )
    _run_two_frames(mon, frame_a, frame_b)
    assert _fired(recorded, "node_unreachable", "node3")
    assert not _fired(recorded, "node_unreachable", "node0")
    # node3 down also means bare quorum.
    assert _fired(recorded, "fault_tolerance_degraded")
    assert not _fired(recorded, "cluster_halt")


def test_scenario_divergence_fires_for_minority_node_only(monkeypatch):
    recorded = []
    mon = _make_monitor(monkeypatch, recorded)
    roots = {"node0": "0xAAA", "node1": "0xAAA", "node2": "0xAAA", "node3": "0xBBB"}
    frame_a = (
        {f"node{i}": _snap(110) for i in range(4)},
        {f"node{i}": _snap(120) for i in range(4)},
        {f"node{i}" for i in range(4)},
        roots,
        1000.0,
    )
    frame_b = (
        {f"node{i}": _snap(120) for i in range(4)},
        {f"node{i}": _snap(130) for i in range(4)},
        {f"node{i}" for i in range(4)},
        roots,
        1061.0,  # past the 60s divergence window
    )
    _run_two_frames(mon, frame_a, frame_b)
    assert _fired(recorded, "state_root_divergence", "node3")
    assert not _fired(recorded, "state_root_divergence", "node0")
    assert not _fired(recorded, "divergence_split")


def test_scenario_two_two_split_fires_divergence_split(monkeypatch):
    recorded = []
    mon = _make_monitor(monkeypatch, recorded)
    roots = {"node0": "0xAAA", "node1": "0xAAA", "node2": "0xBBB", "node3": "0xBBB"}
    frame_a = (
        {f"node{i}": _snap(110) for i in range(4)},
        {f"node{i}": _snap(120) for i in range(4)},
        {f"node{i}" for i in range(4)},
        roots,
        1000.0,
    )
    frame_b = (
        {f"node{i}": _snap(120) for i in range(4)},
        {f"node{i}": _snap(130) for i in range(4)},
        {f"node{i}" for i in range(4)},
        roots,
        1061.0,
    )
    _run_two_frames(mon, frame_a, frame_b)
    assert _fired(recorded, "divergence_split")
    assert not _fired(recorded, "state_root_divergence")
