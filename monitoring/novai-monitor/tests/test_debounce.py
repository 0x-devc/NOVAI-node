"""Test the firing/recovery state machine: sustained-window debouncing,
single-fire semantics, and recovery transitions."""

from alerts import AlertSpec, EvalResult
from novai_monitor import AlertState, transition_event


def _spec(window_secs: float) -> AlertSpec:
    return AlertSpec(
        alert_id="t",
        severity="WARN",
        window_secs=window_secs,
        summary="test",
        playbook=None,
    )


def _r(firing: bool) -> EvalResult:
    return EvalResult(firing=firing, detail="t")


def test_no_fire_before_window_elapses():
    spec = _spec(window_secs=30.0)
    state = AlertState()
    assert transition_event(spec, state, _r(True), now=0.0) is None
    assert transition_event(spec, state, _r(True), now=10.0) is None
    assert transition_event(spec, state, _r(True), now=29.9) is None
    # Window closes exactly at 30.0.
    assert transition_event(spec, state, _r(True), now=30.0) == "FIRE"


def test_fires_only_once_per_episode():
    spec = _spec(window_secs=30.0)
    state = AlertState()
    transition_event(spec, state, _r(True), now=0.0)
    assert transition_event(spec, state, _r(True), now=30.0) == "FIRE"
    # Subsequent ticks while still firing: no re-fire.
    assert transition_event(spec, state, _r(True), now=45.0) is None
    assert transition_event(spec, state, _r(True), now=120.0) is None


def test_recover_emitted_when_condition_clears():
    spec = _spec(window_secs=30.0)
    state = AlertState()
    transition_event(spec, state, _r(True), now=0.0)
    transition_event(spec, state, _r(True), now=30.0)  # FIRE
    assert transition_event(spec, state, _r(False), now=60.0) == "RECOVER"
    # No second RECOVER for further clear observations.
    assert transition_event(spec, state, _r(False), now=90.0) is None


def test_flap_within_window_does_not_fire():
    spec = _spec(window_secs=30.0)
    state = AlertState()
    transition_event(spec, state, _r(True), now=0.0)
    transition_event(spec, state, _r(True), now=10.0)
    # Condition clears before window elapses, so the timer resets.
    assert transition_event(spec, state, _r(False), now=20.0) is None
    # Re-arms only if we cross the window again from scratch.
    transition_event(spec, state, _r(True), now=25.0)
    assert transition_event(spec, state, _r(True), now=40.0) is None
    assert transition_event(spec, state, _r(True), now=55.0) == "FIRE"


def test_zero_window_fires_immediately():
    spec = _spec(window_secs=0.0)
    state = AlertState()
    assert transition_event(spec, state, _r(True), now=0.0) == "FIRE"
    # Still single-fire.
    assert transition_event(spec, state, _r(True), now=1.0) is None


def test_fire_recover_fire_cycle():
    spec = _spec(window_secs=10.0)
    state = AlertState()
    transition_event(spec, state, _r(True), now=0.0)
    assert transition_event(spec, state, _r(True), now=10.0) == "FIRE"
    assert transition_event(spec, state, _r(False), now=20.0) == "RECOVER"
    transition_event(spec, state, _r(True), now=30.0)
    assert transition_event(spec, state, _r(True), now=40.0) == "FIRE"
