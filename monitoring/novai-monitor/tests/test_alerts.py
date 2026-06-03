"""Test the pure-function alert evaluators against synthetic snapshots."""
import os

from alerts import (
    ALERTS,
    ANOMALY_CONFIDENCE_BYTE_HIGH,
    compute_counter_rate_per_minute,
    eval_anomaly_high_confidence,
    eval_anomaly_published,
    eval_block_height_stuck,
    eval_copilot_heartbeat_dead,
    eval_mempool_backlog,
    eval_mempool_empty,
    eval_peer_count_below_quorum,
    eval_peer_count_degraded,
    eval_proposer_skipping_txs,
    eval_view_change_elevated,
    eval_view_change_spike,
)
from parser import parse_prometheus_text

FIXTURES = os.path.join(os.path.dirname(__file__), "fixtures")


def _load(name: str) -> dict:
    with open(os.path.join(FIXTURES, name), encoding="utf-8") as f:
        return parse_prometheus_text(f.read())


# ---------------------------------------------------------------------------
# block_height_stuck
# ---------------------------------------------------------------------------

def test_block_height_stuck_fires_when_height_does_not_advance():
    snap = {"novai_committed_height": 100.0}
    prev = {"novai_committed_height": 100.0}
    r = eval_block_height_stuck(snap, prev, [], 0.0)
    assert r.firing is True


def test_block_height_stuck_clears_when_height_advances():
    snap = {"novai_committed_height": 101.0}
    prev = {"novai_committed_height": 100.0}
    r = eval_block_height_stuck(snap, prev, [], 0.0)
    assert r.firing is False


def test_block_height_stuck_returns_insufficient_when_no_prev():
    snap = {"novai_committed_height": 100.0}
    r = eval_block_height_stuck(snap, None, [], 0.0)
    assert r.firing is False
    assert "insufficient" in r.detail


# ---------------------------------------------------------------------------
# peer count
# ---------------------------------------------------------------------------

def test_peer_count_below_quorum_fires_at_two():
    assert eval_peer_count_below_quorum({"novai_peer_count": 2.0}, None, [], 0.0).firing is True


def test_peer_count_below_quorum_clears_at_three():
    assert eval_peer_count_below_quorum({"novai_peer_count": 3.0}, None, [], 0.0).firing is False


def test_peer_count_degraded_warns_at_three_but_not_four():
    assert eval_peer_count_degraded({"novai_peer_count": 3.0}, None, [], 0.0).firing is True
    assert eval_peer_count_degraded({"novai_peer_count": 4.0}, None, [], 0.0).firing is False


# ---------------------------------------------------------------------------
# mempool
# ---------------------------------------------------------------------------

def test_mempool_empty_fires_at_zero():
    assert eval_mempool_empty({"novai_mempool_size": 0.0}, None, [], 0.0).firing is True


def test_mempool_empty_clears_with_any_tx():
    assert eval_mempool_empty({"novai_mempool_size": 1.0}, None, [], 0.0).firing is False


def test_mempool_backlog_fires_above_threshold():
    assert eval_mempool_backlog({"novai_mempool_size": 1500.0}, None, [], 0.0).firing is True
    assert eval_mempool_backlog({"novai_mempool_size": 999.0}, None, [], 0.0).firing is False


# ---------------------------------------------------------------------------
# view change rate
# ---------------------------------------------------------------------------

def _history_with_counter(values, start_ts=0.0, step=15.0):
    """Build a synthetic history list with a single counter advancing by values[i]."""
    history = []
    total = 0.0
    ts = start_ts
    for v in values:
        total = v
        history.append((ts, {"novai_consensus_view_changes_total": total}))
        ts += step
    return history


def test_view_change_spike_fires_above_six_per_minute():
    # 7 view changes in 60 seconds = 7/min, above the 6/min spike threshold.
    history = [
        (0.0, {"novai_consensus_view_changes_total": 0.0}),
        (60.0, {"novai_consensus_view_changes_total": 7.0}),
    ]
    r = eval_view_change_spike({"novai_consensus_view_changes_total": 7.0}, None, history, 60.0)
    assert r.firing is True


def test_view_change_spike_clears_below_six_per_minute():
    history = [
        (0.0, {"novai_consensus_view_changes_total": 0.0}),
        (60.0, {"novai_consensus_view_changes_total": 3.0}),
    ]
    r = eval_view_change_spike({"novai_consensus_view_changes_total": 3.0}, None, history, 60.0)
    assert r.firing is False


def test_view_change_elevated_warns_between_two_and_six_per_minute():
    history = [
        (0.0, {"novai_consensus_view_changes_total": 0.0}),
        (60.0, {"novai_consensus_view_changes_total": 3.0}),
    ]
    r = eval_view_change_elevated({"novai_consensus_view_changes_total": 3.0}, None, history, 60.0)
    assert r.firing is True


def test_counter_reset_does_not_create_fake_spike():
    # Counter resets to 0 mid-window. A naive last-minus-first would compute a
    # huge negative, then the rate would be negative. We want it to behave as
    # if the resets are non-events.
    history = [
        (0.0, {"novai_consensus_view_changes_total": 50.0}),
        (30.0, {"novai_consensus_view_changes_total": 55.0}),
        (60.0, {"novai_consensus_view_changes_total": 0.0}),  # node restart
        (90.0, {"novai_consensus_view_changes_total": 2.0}),
    ]
    rate = compute_counter_rate_per_minute(history, "novai_consensus_view_changes_total", 180.0, 90.0)
    # Positive deltas: 55-50=5, then skip 0-55, then 2-0=2. Total 7 over 90s = 4.67/min.
    assert rate is not None
    assert 4.0 < rate < 5.0


# ---------------------------------------------------------------------------
# anomaly
# ---------------------------------------------------------------------------

def test_anomaly_published_fires_on_any_new_publish():
    snap = {"novai_anomaly_signals_published": 3.0}
    prev = {"novai_anomaly_signals_published": 2.0}
    assert eval_anomaly_published(snap, prev, [], 0.0).firing is True


def test_anomaly_published_does_not_fire_when_unchanged():
    snap = {"novai_anomaly_signals_published": 2.0}
    prev = {"novai_anomaly_signals_published": 2.0}
    assert eval_anomaly_published(snap, prev, [], 0.0).firing is False


def test_anomaly_high_confidence_uses_byte_threshold_not_float():
    # 0.8 confidence is byte 204. The metric is the raw byte, so passing 0.8
    # would never fire. Verify the threshold is the byte.
    assert ANOMALY_CONFIDENCE_BYTE_HIGH == 204
    assert eval_anomaly_high_confidence({"novai_anomaly_last_confidence": 230.0}, None, [], 0.0).firing is True
    assert eval_anomaly_high_confidence({"novai_anomaly_last_confidence": 200.0}, None, [], 0.0).firing is False
    # Passing the float 0.8 must not fire (this is the gotcha we are guarding against).
    assert eval_anomaly_high_confidence({"novai_anomaly_last_confidence": 0.8}, None, [], 0.0).firing is False


# ---------------------------------------------------------------------------
# copilot heartbeat
# ---------------------------------------------------------------------------

def test_copilot_heartbeat_dead_fires_when_counter_flat():
    history = [
        (0.0, {"novai_copilot_observations_total": 100.0}),
        (300.0, {"novai_copilot_observations_total": 100.0}),
        (600.0, {"novai_copilot_observations_total": 100.0}),
    ]
    r = eval_copilot_heartbeat_dead({"novai_copilot_observations_total": 100.0}, None, history, 600.0)
    assert r.firing is True


def test_copilot_heartbeat_alive_when_counter_advancing():
    history = [
        (0.0, {"novai_copilot_observations_total": 100.0}),
        (300.0, {"novai_copilot_observations_total": 200.0}),
        (600.0, {"novai_copilot_observations_total": 300.0}),
    ]
    r = eval_copilot_heartbeat_dead({"novai_copilot_observations_total": 300.0}, None, history, 600.0)
    assert r.firing is False


# ---------------------------------------------------------------------------
# proposer skipping txs
# ---------------------------------------------------------------------------

def test_proposer_skipping_txs_fires_when_mempool_nonempty_but_no_commits():
    history = [
        (0.0, {"novai_total_txs_committed": 1000.0}),
        (60.0, {"novai_total_txs_committed": 1000.0}),
        (120.0, {"novai_total_txs_committed": 1000.0}),
    ]
    snap = {"novai_mempool_size": 50.0, "novai_total_txs_committed": 1000.0}
    r = eval_proposer_skipping_txs(snap, None, history, 120.0)
    assert r.firing is True


def test_proposer_skipping_txs_quiet_when_mempool_empty():
    history = [
        (0.0, {"novai_total_txs_committed": 1000.0}),
        (60.0, {"novai_total_txs_committed": 1000.0}),
    ]
    snap = {"novai_mempool_size": 0.0, "novai_total_txs_committed": 1000.0}
    assert eval_proposer_skipping_txs(snap, None, history, 60.0).firing is False


# ---------------------------------------------------------------------------
# fixture-driven full evaluation
# ---------------------------------------------------------------------------

def test_full_eval_stalled_fixture_fires_expected_alerts():
    """Apply every evaluator against the stalled fixture vs the healthy fixture as the prev,
    confirm the right alerts fire and the rest stay quiet."""
    prev = _load("metrics_healthy.txt")
    snap = _load("metrics_stalled.txt")
    # The spike alert uses a 180s trailing window, so 2 points 300s apart leave only
    # 1 sample inside the window (insufficient_data). Add an intermediate sample to
    # show view changes accruing through the window.
    mid = dict(prev)
    mid["novai_consensus_view_changes_total"] = 50.0
    # Copilot counter intentionally unchanged from healthy so heartbeat_dead fires.
    history = [
        (0.0, prev),
        (180.0, mid),
        (300.0, snap),
    ]
    now = 300.0
    firing_ids = set()
    for spec, evaluator in ALERTS:
        if evaluator(snap, prev, history, now).firing:
            firing_ids.add(spec.alert_id)
    # Stalled fixture: height same -> stuck. peers 2 -> below quorum + degraded.
    # mempool 0 -> empty. view changes jumped 7 -> 100 over 300s = 18.6/min ->
    # both elevated AND spike fire. Copilot counter unchanged -> heartbeat dead.
    # No tx commits with empty mempool, so proposer_skipping_txs does NOT fire
    # (it requires mempool > 0).
    assert "block_height_stuck" in firing_ids
    assert "peer_count_below_quorum" in firing_ids
    assert "peer_count_degraded" in firing_ids
    assert "mempool_empty" in firing_ids
    assert "view_change_elevated" in firing_ids
    assert "view_change_spike" in firing_ids
    assert "copilot_heartbeat_dead" in firing_ids
    assert "proposer_skipping_txs" not in firing_ids
    assert "anomaly_high_confidence" not in firing_ids
    assert "anomaly_published" not in firing_ids
    assert "mempool_backlog" not in firing_ids


def test_full_eval_high_anomaly_fixture_fires_expected_alerts():
    prev = _load("metrics_healthy.txt")
    snap = _load("metrics_high_anomaly.txt")
    history = [(0.0, prev), (60.0, snap)]
    now = 60.0
    firing_ids = set()
    for spec, evaluator in ALERTS:
        if evaluator(snap, prev, history, now).firing:
            firing_ids.add(spec.alert_id)
    assert "anomaly_high_confidence" in firing_ids
    assert "anomaly_published" in firing_ids
    # Chain is healthy in the anomaly fixture; consensus alerts should NOT fire.
    assert "block_height_stuck" not in firing_ids
    assert "peer_count_below_quorum" not in firing_ids
    assert "mempool_empty" not in firing_ids
