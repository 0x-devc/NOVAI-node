"""
The commit_stall dual-trigger alarm (incident WEDGE-20260718).

In the 20260718 incident commits froze while consensus kept certifying
heights for five days; the monitor's scrape-pair alerts saw committed
height flat, but nothing measured the consensus-minus-committed gap or
the wall-clock stall age, and nothing paged with headroom before the
frontier ran away. This alarm fires on EITHER of two conditions:

- gap trigger: novai_consensus_commit_gap exceeds 256 blocks, a quarter
  of the node's COMMIT_WINDOW of 1024, so it pages with headroom before
  the commit-window rule parks the fleet. The healthy gap under the
  3-chain rule is 2 to 3 blocks REGARDLESS of block rate, so this
  trigger never false-fires as throughput grows.
- time trigger: novai_seconds_since_last_commit exceeds 30 seconds. A
  fixed block-count threshold alone gives shrinking wall-clock warning
  as block rate rises (256 blocks is about 64 seconds at 4 blocks/s but
  about 10 seconds at 25 blocks/s), so the time trigger is what keeps
  the alarm robust as the chain speeds up.

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


def _commit_stall():
    matches = [(spec, ev) for spec, ev in NODE_ALERTS if spec.alert_id == "commit_stall"]
    assert matches, "commit_stall dual-trigger alarm is not registered in NODE_ALERTS"
    assert len(matches) == 1, "commit_stall must be registered exactly once"
    return matches[0]


def _eval(snap):
    _spec, evaluator = _commit_stall()
    return evaluator(snap, None, [], 0.0)


# ---------------------------------------------------------------------------
# Registration and thresholds
# ---------------------------------------------------------------------------

def test_commit_stall_registered_critical_with_playbook():
    spec, _ev = _commit_stall()
    assert spec.severity == "CRITICAL"
    assert spec.playbook == "EMERGENCY_FREEZE.md"


def test_commit_stall_thresholds_pinned():
    # The gap threshold is a quarter of the node's COMMIT_WINDOW (1024,
    # crates/consensus/src/lib.rs). The two constants live in different
    # languages, so the relation is pinned numerically here: if either
    # side changes without the other, this test is the tripwire.
    gap = getattr(alerts, "COMMIT_GAP_RUNAWAY_BLOCKS", None)
    stall = getattr(alerts, "COMMIT_STALL_SECS", None)
    assert gap == 256, "gap trigger must warn at a quarter of COMMIT_WINDOW"
    assert gap * 4 == 1024, "gap threshold must stay a quarter of the window"
    assert stall == 30.0, "time trigger must fire after 30 seconds without a commit"


# ---------------------------------------------------------------------------
# Gap trigger, independently
# ---------------------------------------------------------------------------

def test_gap_trigger_fires_alone():
    # Commits are still advancing (2 seconds since the last one), but the
    # frontier is running away. The gap alone must page.
    r = _eval({"novai_consensus_commit_gap": 257.0, "novai_seconds_since_last_commit": 2.0})
    assert r.firing is True
    assert "gap=257" in r.detail


def test_gap_boundary_exact():
    quiet = _eval({"novai_consensus_commit_gap": 256.0, "novai_seconds_since_last_commit": 0.0})
    assert quiet.firing is False, "gap of exactly 256 is the last quiet value"
    fires = _eval({"novai_consensus_commit_gap": 257.0, "novai_seconds_since_last_commit": 0.0})
    assert fires.firing is True, "the alarm fires when the gap EXCEEDS 256"


def test_gap_trigger_fires_without_time_metric():
    # A snapshot that carries only the gap gauge is still decidable for
    # the gap trigger; the absent time gauge contributes nothing.
    r = _eval({"novai_consensus_commit_gap": 300.0})
    assert r.firing is True
    assert "gap=300" in r.detail


# ---------------------------------------------------------------------------
# Time trigger, independently
# ---------------------------------------------------------------------------

def test_time_trigger_fires_alone():
    # The gap is healthy (2 blocks: a stall parks the frontier a couple of
    # blocks ahead at low rate), but nothing has committed for 31 seconds.
    # The clock alone must page: this is the silent low-rate stall.
    r = _eval({"novai_consensus_commit_gap": 2.0, "novai_seconds_since_last_commit": 31.0})
    assert r.firing is True
    assert "secs_since_commit=31" in r.detail


def test_time_boundary_exact():
    quiet = _eval({"novai_consensus_commit_gap": 2.0, "novai_seconds_since_last_commit": 30.0})
    assert quiet.firing is False, "exactly 30 seconds is the last quiet value"
    fires = _eval({"novai_consensus_commit_gap": 2.0, "novai_seconds_since_last_commit": 31.0})
    assert fires.firing is True, "the alarm fires after MORE than 30 seconds"


def test_time_trigger_fires_without_gap_metric():
    r = _eval({"novai_seconds_since_last_commit": 45.0})
    assert r.firing is True
    assert "secs_since_commit=45" in r.detail


# ---------------------------------------------------------------------------
# Both triggers, and the incident replay
# ---------------------------------------------------------------------------

def test_both_triggers_fire_with_both_named():
    r = _eval({"novai_consensus_commit_gap": 400.0, "novai_seconds_since_last_commit": 120.0})
    assert r.firing is True
    assert "gap=400" in r.detail
    assert "secs_since_commit=120" in r.detail


def test_wedge_20260718_replay_fires():
    # The incident geometry: 818,258 blocks of frontier runaway, five days
    # of commit stall. Either trigger alone would have paged within about
    # a minute of the 18:56 freeze instead of day five.
    r = _eval({
        "novai_consensus_commit_gap": 818258.0,
        "novai_seconds_since_last_commit": 432000.0,
    })
    assert r.firing is True


# ---------------------------------------------------------------------------
# Healthy operation stays quiet at ANY block rate
# ---------------------------------------------------------------------------

def test_healthy_quiet_at_any_block_rate():
    # The design premise: under the 3-chain rule the healthy gap is 2 to 3
    # blocks REGARDLESS of throughput, and commits land continuously. I
    # simulate a minute of healthy scrapes at rates from 1 to 1000
    # blocks/s: the gap gauge stays in its structural band and the commit
    # clock stays near zero, so the alarm must stay quiet at every rate.
    scrape_interval = 5.0
    for rate in (1, 4, 25, 100, 1000):
        committed = 1_000_000.0
        for scrape in range(12):
            committed += rate * scrape_interval
            gap = 2.0 + (scrape % 2)  # oscillates 2..3, the pipeline depth
            snap = {
                "novai_committed_height": committed,
                "novai_highest_qc_height": committed + gap,
                "novai_consensus_commit_gap": gap,
                "novai_seconds_since_last_commit": float(scrape % 2),
            }
            r = _eval(snap)
            assert r.firing is False, (
                f"healthy fleet at {rate} blocks/s paged on scrape {scrape}: {r.detail}"
            )


# ---------------------------------------------------------------------------
# Missing data never pages
# ---------------------------------------------------------------------------

def test_missing_both_metrics_is_insufficient_data():
    r = _eval({"novai_committed_height": 500.0})
    assert r.firing is False
    assert r.detail == "insufficient_data"


# ---------------------------------------------------------------------------
# End to end through the exposition parser, against the real fixtures
# ---------------------------------------------------------------------------

def test_healthy_fixture_stays_quiet():
    r = _eval(_load("metrics_healthy.txt"))
    assert r.firing is False, r.detail


def test_stalled_fixture_fires():
    # The stalled fixture carries the stall signature on both axes: gap 400
    # (above 256) and 300 seconds since the last commit (above 30).
    r = _eval(_load("metrics_stalled.txt"))
    assert r.firing is True
    assert "gap=400" in r.detail
    assert "secs_since_commit=300" in r.detail
