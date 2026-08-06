"""Test the pure-function alert evaluators against synthetic snapshots."""
import os

from alerts import (
    NODE_ALERTS,
    ANOMALY_CONFIDENCE_BYTE_HIGH,
    compute_counter_rate_per_minute,
    eval_anomaly_high_confidence,
    eval_anomaly_published,
    eval_copilot_heartbeat_dead,
    eval_mempool_gapped_high,
    eval_generator_desync,
    eval_mempool_empty,
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
# mempool
# ---------------------------------------------------------------------------

def test_mempool_empty_fires_at_zero():
    assert eval_mempool_empty({"novai_mempool_size": 0.0}, None, [], 0.0).firing is True


def test_mempool_empty_clears_with_any_tx():
    assert eval_mempool_empty({"novai_mempool_size": 1.0}, None, [], 0.0).firing is False


def test_mempool_gapped_high_fires_above_threshold():
    assert eval_mempool_gapped_high({"novai_mempool_gapped": 1500.0}, None, [], 0.0).firing is True
    assert eval_mempool_gapped_high({"novai_mempool_gapped": 999.0}, None, [], 0.0).firing is False


def test_a_deep_healthy_backlog_never_fires_the_gapped_alarm():
    """
    Gate SOAK C3. The point of moving this alarm off total depth. A pool
    holding thousands of transactions that are all waiting their turn is
    correct behaviour for a soak, and must be silent.
    """
    snap = {"novai_mempool_size": 50_000.0, "novai_mempool_waiting": 49_999.0,
            "novai_mempool_ready": 1.0, "novai_mempool_gapped": 0.0}
    assert eval_mempool_gapped_high(snap, None, [], 0.0).firing is False


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
    # huge negative, then the rate would be negative. The expected behavior is
    # to treat the resets as non-events.
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
    # Passing the float 0.8 must not fire (this is the gotcha being guarded against).
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
    snap = {"novai_mempool_size": 50.0, "novai_mempool_ready": 50.0,
            "novai_total_txs_committed": 1000.0}
    r = eval_proposer_skipping_txs(snap, None, history, 120.0)
    assert r.firing is True


def test_proposer_skipping_txs_stays_quiet_when_nothing_is_ready():
    """
    Gate SOAK C3. A pool full of unreachable transactions is not the
    proposer's fault: it is correctly producing empty blocks because it has
    nothing it may include. Blaming the proposer here would page on a
    client-side desync under a name that sends the operator to the wrong
    subsystem; generator_desync and mempool_gapped_high name it properly.
    """
    history = [
        (0.0, {"novai_total_txs_committed": 1000.0}),
        (60.0, {"novai_total_txs_committed": 1000.0}),
        (120.0, {"novai_total_txs_committed": 1000.0}),
    ]
    snap = {"novai_mempool_size": 500.0, "novai_mempool_ready": 0.0,
            "novai_mempool_gapped": 500.0, "novai_total_txs_committed": 1000.0}
    assert eval_proposer_skipping_txs(snap, None, history, 120.0).firing is False


def test_proposer_skipping_txs_still_fires_on_the_real_fault():
    """
    THE PIN THAT MUST NOT REGRESS. Ready transactions exist and nothing is
    committing: that is the genuine fault this alarm was added for, and the
    C3 change must not silence it.
    """
    history = [
        (0.0, {"novai_total_txs_committed": 1000.0}),
        (60.0, {"novai_total_txs_committed": 1000.0}),
        (120.0, {"novai_total_txs_committed": 1000.0}),
    ]
    snap = {"novai_mempool_size": 20.0, "novai_mempool_ready": 20.0,
            "novai_total_txs_committed": 1000.0}
    assert eval_proposer_skipping_txs(snap, None, history, 120.0).firing is True


def test_generator_desync_fires_on_sustained_nonce_state_rejections():
    history = [
        (0.0, {"novai_mempool_rejects_nonce_too_high": 0.0}),
        (150.0, {"novai_mempool_rejects_nonce_too_high": 500.0}),
        (300.0, {"novai_mempool_rejects_nonce_too_high": 1000.0}),
    ]
    snap = {"novai_mempool_rejects_nonce_too_high": 1000.0}
    assert eval_generator_desync(snap, None, history, 300.0).firing is True


def test_generator_desync_quiet_on_a_healthy_chain():
    history = [
        (0.0, {"novai_mempool_rejects_nonce_too_low": 0.0}),
        (150.0, {"novai_mempool_rejects_nonce_too_low": 1.0}),
        (300.0, {"novai_mempool_rejects_nonce_too_low": 2.0}),
    ]
    snap = {"novai_mempool_rejects_nonce_too_low": 2.0}
    assert eval_generator_desync(snap, None, history, 300.0).firing is False


def test_generator_desync_is_insufficient_data_without_the_counters():
    assert eval_generator_desync({}, None, [], 0.0).detail == "insufficient_data"


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
    for spec, evaluator in NODE_ALERTS:
        if evaluator(snap, prev, history, now).firing:
            firing_ids.add(spec.alert_id)
    # Stalled fixture, per-node evaluators only. mempool 0 -> empty. view changes
    # jumped 7 -> 100 over 300s = 18.6/min -> both elevated AND spike fire.
    # Copilot counter unchanged -> heartbeat dead. No tx commits with empty
    # mempool, so proposer_skipping_txs does NOT fire (it requires mempool > 0).
    # Stuck height and lost peers are now cross-node concerns (node_stuck,
    # cluster_halt, fault_tolerance), covered in test_cluster_alerts.py.
    assert "mempool_empty" in firing_ids
    assert "view_change_elevated" in firing_ids
    assert "view_change_spike" in firing_ids
    assert "copilot_heartbeat_dead" in firing_ids
    assert "proposer_skipping_txs" not in firing_ids
    assert "anomaly_high_confidence" not in firing_ids
    assert "anomaly_published" not in firing_ids
    assert "mempool_gapped_high" not in firing_ids


def test_full_eval_high_anomaly_fixture_fires_expected_alerts():
    prev = _load("metrics_healthy.txt")
    snap = _load("metrics_high_anomaly.txt")
    history = [(0.0, prev), (60.0, snap)]
    now = 60.0
    firing_ids = set()
    for spec, evaluator in NODE_ALERTS:
        if evaluator(snap, prev, history, now).firing:
            firing_ids.add(spec.alert_id)
    assert "anomaly_high_confidence" in firing_ids
    assert "anomaly_published" in firing_ids
    # Chain is healthy in the anomaly fixture; node-local consensus alerts
    # should NOT fire.
    assert "mempool_empty" not in firing_ids
    assert "mempool_gapped_high" not in firing_ids


# ---------------------------------------------------------------------------
# Gate SOAK C5: the anomaly pair is no longer zero-debounce CRITICAL
# ---------------------------------------------------------------------------

def test_no_alert_dispatches_on_a_single_scrape():
    """
    anomaly_published and anomaly_high_confidence were the only two alarms in
    the stack with a zero second window, so they paged on the first
    qualifying scrape. Their upstream trigger is mempool growth past about
    4x a rolling baseline, which any load ramp from idle crosses, so a
    CRITICAL page pointing at a module-rollback playbook was the routine
    response to load starting. That was the largest single source of noise
    on this stack.

    No alarm may dispatch on one observation now.
    """
    from alerts import NODE_ALERTS
    zero_window = [spec.alert_id for spec, _ in NODE_ALERTS if spec.window_secs <= 0.0]
    assert zero_window == [], (
        f"these alarms page on a single scrape with no persistence "
        f"requirement: {zero_window}"
    )


def test_anomaly_high_confidence_is_not_critical():
    """
    A mempool growth spike is not a roll-back-the-module event. The genuine
    desync case now has alarms that name it (generator_desync,
    mempool_gapped_high), so this one does not need to carry CRITICAL.
    """
    from alerts import NODE_ALERTS
    spec = next(s for s, _ in NODE_ALERTS if s.alert_id == "anomaly_high_confidence")
    assert spec.severity == "WARN"
    assert spec.window_secs >= 60.0


def test_commit_stall_is_untouched_by_this_gate():
    """
    commit_stall is the genuine wedge detector and the only CRITICAL in this
    group that must survive every noise-reduction change. Pinned here so a
    future tuning pass cannot quietly soften it.

    NOTE: this pins the code, not the deployment. Whether the running monitor
    actually has commit_stall is a separate, server-side question.
    """
    from alerts import NODE_ALERTS, COMMIT_GAP_RUNAWAY_BLOCKS, COMMIT_STALL_SECS
    spec = next(s for s, _ in NODE_ALERTS if s.alert_id == "commit_stall")
    assert spec.severity == "CRITICAL"
    assert spec.window_secs == 30.0
    assert spec.playbook == "EMERGENCY_FREEZE.md"
    assert COMMIT_GAP_RUNAWAY_BLOCKS == 256
    assert COMMIT_STALL_SECS == 30.0
