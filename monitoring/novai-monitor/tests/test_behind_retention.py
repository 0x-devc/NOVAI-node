"""
The behind_retention alarm (gate F5 Stage 1).

A validator that falls further behind than the fleet's prune horizon
cannot recover by block sync: the blocks it needs no longer exist on any
peer. The node already detects this (SyncRequestOutcome::BehindRetention,
crates/node/src/consensus_node.rs) and logs at ERROR once a minute, but
nothing paged, because the only alarm that fired was commit_stall, whose
gap trigger also fires for a 30 second hiccup. A 346,000 block
unrecoverable gap and a brief stall looked identical to the operator, and
the responses are completely different: one waits, the other needs a
state snapshot installed.

This alarm reads novai_sync_mode, the node's own detection phase:

  0  block-range sync is viable (or there is no gap)
  1  the gap is past the prune horizon and probes are coming back unserved
  2  armed: block-range sync is structurally impossible for this node

Values 3 to 5 are reserved for the fetch, verify and staged phases of the
later F5 stages, so the encoding never has to be renumbered; the alarm
treats anything at or above 1 as firing.

Mode 1 is never benign. It means the gap already exceeds
PRUNE_RETAIN_BLOCKS (50,000), which at the fleet's cadence is hours of
lag, and no honest peer retains the range. The alarm still carries a
window in the registry so the self-correcting case right at the retention
boundary (a peer whose own committed height lags slightly may still hold
the block) resolves without paging.

These tests resolve the alarm through the NODE_ALERTS registry, so on a
tree that predates the alarm every test fails with the registration
message (the right reason: the alarm does not exist), never with an
import error.
"""
import os

import alerts
from alerts import NODE_ALERTS
from parser import parse_prometheus_text

FIXTURES = os.path.join(os.path.dirname(__file__), "fixtures")


def _load(name):
    with open(os.path.join(FIXTURES, name), encoding="utf-8") as f:
        return parse_prometheus_text(f.read())


def _resolve(alert_id):
    matches = [(spec, ev) for spec, ev in NODE_ALERTS if spec.alert_id == alert_id]
    assert matches, f"{alert_id} alarm is not registered in NODE_ALERTS"
    assert len(matches) == 1, f"{alert_id} must be registered exactly once"
    return matches[0]


def _behind_retention():
    return _resolve("behind_retention")


def _eval(snap):
    _spec, evaluator = _behind_retention()
    return evaluator(snap, None, [], 0.0)


def _eval_commit_stall(snap):
    _spec, evaluator = _resolve("commit_stall")
    return evaluator(snap, None, [], 0.0)


# ---------------------------------------------------------------------------
# Registration
# ---------------------------------------------------------------------------

def test_behind_retention_registered_critical():
    spec, _ev = _behind_retention()
    assert spec.severity == "CRITICAL"
    assert spec.window_secs > 0, "the alarm must debounce, not page on one scrape"


def test_sync_mode_encoding_pinned():
    # The gauge encoding is a contract between crates/node/src/metrics.rs and
    # this file, in two different languages. Pin it numerically so a change on
    # either side trips here.
    assert getattr(alerts, "SYNC_MODE_IDLE", None) == 0
    assert getattr(alerts, "SYNC_MODE_ARMING", None) == 1
    assert getattr(alerts, "SYNC_MODE_ARMED", None) == 2


# ---------------------------------------------------------------------------
# Firing behaviour
# ---------------------------------------------------------------------------

def test_idle_mode_is_quiet():
    r = _eval({"novai_sync_mode": 0.0})
    assert r.firing is False
    assert "sync_mode=0" in r.detail


def test_arming_mode_fires():
    # Past the prune horizon with probes coming back unserved. Not benign:
    # the gap already exceeds 50,000 blocks.
    r = _eval({"novai_sync_mode": 1.0})
    assert r.firing is True
    assert "sync_mode=1" in r.detail


def test_armed_mode_fires_and_names_the_snapshot_requirement():
    r = _eval({"novai_sync_mode": 2.0})
    assert r.firing is True
    assert "sync_mode=2" in r.detail
    assert "snapshot" in r.detail.lower(), (
        "the armed detail must name the required action, since it is the "
        "whole reason this alarm exists separately from commit_stall"
    )


def test_reserved_later_phases_still_fire():
    # The later F5 stages report 3, 4 and 5 as they fetch, verify and stage.
    # A node in any of those phases is still unrecoverable by block sync, so
    # the alarm must not go quiet when the encoding grows.
    for mode in (3.0, 4.0, 5.0):
        r = _eval({"novai_sync_mode": mode})
        assert r.firing is True, f"mode {mode} must still fire"


def test_missing_metric_is_insufficient_data_not_zero():
    # File invariant: an absent metric is missing, not zero. A node running an
    # older binary must not read as healthy.
    r = _eval({})
    assert r.firing is False
    assert r.detail == "insufficient_data"


# ---------------------------------------------------------------------------
# Distinctness from commit_stall (the point of the alarm)
# ---------------------------------------------------------------------------

def test_distinct_alert_ids_and_evaluators():
    br_spec, br_ev = _behind_retention()
    cs_spec, cs_ev = _resolve("commit_stall")
    assert br_spec.alert_id != cs_spec.alert_id
    assert br_ev is not cs_ev
    assert br_spec.summary != cs_spec.summary


def test_plain_commit_stall_does_not_fire_behind_retention():
    # A 31 second stall with a healthy gap: commit_stall fires, behind
    # retention stays quiet. Before this alarm existed, the operator had one
    # signal for both situations.
    snap = {
        "novai_consensus_commit_gap": 2.0,
        "novai_seconds_since_last_commit": 31.0,
        "novai_sync_mode": 0.0,
    }
    assert _eval_commit_stall(snap).firing is True
    assert _eval(snap).firing is False


def test_unrecoverable_gap_fires_both_with_the_specific_one_naming_it():
    # The live node1 geometry on 2026-08-10: committed 1,580,000, frontier
    # about 1,925,939, gap about 345,939, armed. commit_stall still fires (the
    # gap is enormous), and it should; behind_retention fires alongside it and
    # is the one that names the required action.
    snap = {
        "novai_consensus_commit_gap": 345_939.0,
        "novai_seconds_since_last_commit": 86_400.0,
        "novai_sync_mode": 2.0,
    }
    assert _eval_commit_stall(snap).firing is True
    br = _eval(snap)
    assert br.firing is True
    assert "snapshot" in br.detail.lower()


# ---------------------------------------------------------------------------
# Fixture parity: the healthy fixture must not fire
# ---------------------------------------------------------------------------

def test_healthy_fixture_is_quiet_or_undecidable():
    # metrics_healthy.txt predates the gauge, so the honest outcome is
    # insufficient_data. Once the fixture carries the gauge at 0 this stays
    # quiet either way; what must never happen is a page on a healthy node.
    r = _eval(_load("metrics_healthy.txt"))
    assert r.firing is False
