#!/usr/bin/env python3
"""
PURPOSE: Poll the NOVAI node Prometheus metrics endpoint, evaluate a set
of chain health conditions, and deliver alerts to a Telegram chat when
those conditions sustain for their configured windows. Designed for
single-node deployment alongside novai-node on the testnet host.

INVARIANTS:
- Polling cadence is at least as tight as the smallest alert window so
  no alert is missed by a single skipped scrape.
- A FIRE transition is delivered exactly once per firing episode. A
  RECOVER transition is delivered exactly once when the condition clears.
- The re-arm grace at startup suppresses outbound alerts but still seeds
  baselines, so the first real alert observed after grace is correct.
- The metrics endpoint is hit over plain HTTP on localhost by default.
  No basic auth is required for the loopback path; auth fields are kept
  for the override case (NOVAI_MONITOR_METRICS_URL pointing at the
  nginx-fronted public endpoint).
- The script never crashes on transient network or HTTP failure. It
  logs, backs off, and keeps trying.

FAILURE MODES:
- Telegram delivery failures are buffered to ALERTS_UNDELIVERED_PATH and
  drained on the next successful send.
- Metrics endpoint unreachable for >= the configured threshold fires a
  single A13 WARN and a single recovery message; per-scrape failures are
  only logged.
"""

from __future__ import annotations

import argparse
import base64
import dataclasses
import datetime as _dt
import json
import logging
import os
import signal
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Dict, List, Optional, Tuple

from alerts import (
    ALERTS,
    AlertSpec,
    EvalResult,
    UNREACHABLE_SPEC,
)
from notifier import (
    append_undelivered,
    format_alert_message,
    render_for_stderr,
    send_alert,
)
from parser import parse_prometheus_text

LOG = logging.getLogger("novai_monitor")


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

DEFAULT_METRICS_URL = "http://localhost:8081/metrics"
HISTORY_RETENTION_SECS = 900.0  # 15 minutes of scrapes is enough for the slowest window (10 min copilot)
ALERTS_UNDELIVERED_PATH = "/var/lib/novai-monitor/alerts_undelivered.jsonl"


@dataclasses.dataclass
class Config:
    metrics_url: str
    metrics_user: str
    metrics_pass: str
    poll_interval_secs: float
    http_timeout_secs: float
    rearm_grace_secs: float
    unreachable_threshold_secs: float
    telegram_bot_token: str
    telegram_chat_id: str
    log_level: str
    env_label: str
    undelivered_path: str


def _env_float(name: str, default: float) -> float:
    raw = os.environ.get(name, "").strip()
    if not raw:
        return default
    try:
        return float(raw)
    except ValueError:
        LOG.warning("config_bad_float event=using_default name=%s value=%r default=%s",
                    name, raw, default)
        return default


def load_config() -> Config:
    return Config(
        metrics_url=os.environ.get("NOVAI_MONITOR_METRICS_URL", DEFAULT_METRICS_URL),
        metrics_user=os.environ.get("NOVAI_MONITOR_METRICS_USER", ""),
        metrics_pass=os.environ.get("NOVAI_MONITOR_METRICS_PASS", ""),
        poll_interval_secs=_env_float("NOVAI_MONITOR_POLL_INTERVAL_SECS", 30.0),
        http_timeout_secs=_env_float("NOVAI_MONITOR_HTTP_TIMEOUT_SECS", 10.0),
        rearm_grace_secs=_env_float("NOVAI_MONITOR_REARM_GRACE_SECS", 120.0),
        unreachable_threshold_secs=_env_float("NOVAI_MONITOR_UNREACHABLE_THRESHOLD_SECS", 120.0),
        telegram_bot_token=os.environ.get("NOVAI_MONITOR_TELEGRAM_BOT_TOKEN", ""),
        telegram_chat_id=os.environ.get("NOVAI_MONITOR_TELEGRAM_CHAT_ID", ""),
        log_level=os.environ.get("NOVAI_MONITOR_LOG_LEVEL", "INFO"),
        env_label=os.environ.get("NOVAI_MONITOR_ENV_LABEL", "unknown"),
        undelivered_path=os.environ.get("NOVAI_MONITOR_UNDELIVERED_PATH", ALERTS_UNDELIVERED_PATH),
    )


# ---------------------------------------------------------------------------
# HTTP scrape
# ---------------------------------------------------------------------------

def scrape_metrics(cfg: Config) -> Tuple[Optional[Dict[str, float]], Optional[str]]:
    """
    GET the metrics endpoint and parse the response. Returns
    (snapshot_dict, error_string). On success error_string is None.
    """
    req = urllib.request.Request(cfg.metrics_url, method="GET")
    if cfg.metrics_user and cfg.metrics_pass:
        creds = f"{cfg.metrics_user}:{cfg.metrics_pass}".encode("utf-8")
        req.add_header("Authorization", "Basic " + base64.b64encode(creds).decode("ascii"))
    try:
        with urllib.request.urlopen(req, timeout=cfg.http_timeout_secs) as resp:
            if not (200 <= resp.status < 300):
                return None, f"http_{resp.status}"
            body = resp.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as e:
        return None, f"http_{e.code}"
    except (urllib.error.URLError, OSError) as e:
        return None, f"network:{e}"
    snapshot = parse_prometheus_text(body)
    if not snapshot:
        return None, "empty_or_unparseable"
    return snapshot, None


# ---------------------------------------------------------------------------
# Firing/recovery state machine
# ---------------------------------------------------------------------------

@dataclasses.dataclass
class AlertState:
    condition_true_since: Optional[float] = None
    firing: bool = False


def transition_event(
    spec: AlertSpec,
    state: AlertState,
    eval_result: EvalResult,
    now: float,
) -> Optional[str]:
    """
    Advance the state machine for one alert based on the latest evaluation.
    Returns "FIRE" or "RECOVER" if a transition crossed, otherwise None.
    """
    if eval_result.firing:
        if state.condition_true_since is None:
            state.condition_true_since = now
        if not state.firing and (now - state.condition_true_since) >= spec.window_secs:
            state.firing = True
            return "FIRE"
        return None
    state.condition_true_since = None
    if state.firing:
        state.firing = False
        return "RECOVER"
    return None


# ---------------------------------------------------------------------------
# Time and ISO formatting
# ---------------------------------------------------------------------------

def utc_now_iso(now: float) -> str:
    return _dt.datetime.fromtimestamp(now, tz=_dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


# ---------------------------------------------------------------------------
# Dispatch
# ---------------------------------------------------------------------------

def dispatch_transition(
    cfg: Config,
    spec: AlertSpec,
    transition: str,
    detail: str,
    now: float,
    dry_run: bool,
) -> None:
    now_iso = utc_now_iso(now)
    LOG.info(render_for_stderr(spec, transition, detail, cfg.env_label, now_iso))
    message = format_alert_message(spec, transition, detail, cfg.env_label, now_iso)
    delivered = send_alert(
        cfg.telegram_bot_token,
        cfg.telegram_chat_id,
        message,
        dry_run=dry_run,
    )
    if not delivered:
        err = append_undelivered(cfg.undelivered_path, spec, transition, message, now_iso)
        if err is not None:
            LOG.error("undelivered_buffer_error event=write_failed path=%s error=%s",
                      cfg.undelivered_path, err)


# ---------------------------------------------------------------------------
# Main loop
# ---------------------------------------------------------------------------

class Monitor:
    def __init__(self, cfg: Config, dry_run: bool = False):
        self.cfg = cfg
        self.dry_run = dry_run
        self.history: List[Tuple[float, Dict[str, float]]] = []
        self.prev_snapshot: Optional[Dict[str, float]] = None
        self.states: Dict[str, AlertState] = {spec.alert_id: AlertState() for spec, _ in ALERTS}
        self.unreachable_state = AlertState()
        self.startup_ts = time.time()
        self.unreachable_since: Optional[float] = None
        self.backoff_secs = cfg.poll_interval_secs
        self.stopping = False
        signal.signal(signal.SIGTERM, self._handle_signal)
        signal.signal(signal.SIGINT, self._handle_signal)

    def _handle_signal(self, signum, _frame) -> None:
        LOG.info("shutdown event=signal signum=%d", signum)
        self.stopping = True

    def _trim_history(self, now: float) -> None:
        cutoff = now - HISTORY_RETENTION_SECS
        self.history = [(ts, snap) for ts, snap in self.history if ts >= cutoff]

    def _within_rearm_grace(self, now: float) -> bool:
        return (now - self.startup_ts) < self.cfg.rearm_grace_secs

    def _handle_unreachable(self, now: float, error: str) -> None:
        if self.unreachable_since is None:
            self.unreachable_since = now
        elapsed = now - self.unreachable_since
        if (not self.unreachable_state.firing
                and elapsed >= self.cfg.unreachable_threshold_secs
                and not self._within_rearm_grace(now)):
            self.unreachable_state.firing = True
            dispatch_transition(
                self.cfg, UNREACHABLE_SPEC, "FIRE",
                f"unreachable_for={elapsed:.0f}s last_error={error}",
                now, self.dry_run,
            )
        self.backoff_secs = min(self.backoff_secs * 2.0, 300.0)

    def _handle_reachable(self, now: float) -> None:
        if self.unreachable_state.firing:
            elapsed = (now - self.unreachable_since) if self.unreachable_since else 0.0
            self.unreachable_state.firing = False
            dispatch_transition(
                self.cfg, UNREACHABLE_SPEC, "RECOVER",
                f"recovered_after={elapsed:.0f}s",
                now, self.dry_run,
            )
        self.unreachable_since = None
        self.backoff_secs = self.cfg.poll_interval_secs

    def _evaluate_all(self, snapshot: Dict[str, float], now: float) -> None:
        for spec, evaluator in ALERTS:
            result = evaluator(snapshot, self.prev_snapshot, self.history, now)
            state = self.states[spec.alert_id]
            transition = transition_event(spec, state, result, now)
            if transition is None:
                continue
            if self._within_rearm_grace(now):
                LOG.info("alert_suppressed event=rearm_grace alert_id=%s transition=%s",
                         spec.alert_id, transition)
                continue
            dispatch_transition(self.cfg, spec, transition, result.detail, now, self.dry_run)

    def tick(self) -> None:
        now = time.time()
        snapshot, error = scrape_metrics(self.cfg)
        if snapshot is None:
            LOG.warning("scrape_failed event=retrying error=%s next_backoff_secs=%.1f",
                        error, self.backoff_secs)
            self._handle_unreachable(now, error or "unknown")
            return
        LOG.debug("scrape_ok event=parsed n_metrics=%d", len(snapshot))
        self._handle_reachable(now)
        # Detect counter reset by sampling one known counter; if it drops, clear history.
        if (self.prev_snapshot
                and "novai_total_txs_committed" in self.prev_snapshot
                and "novai_total_txs_committed" in snapshot
                and snapshot["novai_total_txs_committed"] < self.prev_snapshot["novai_total_txs_committed"]):
            LOG.info("counter_reset event=clearing_history")
            self.history.clear()
        self.history.append((now, snapshot))
        self._trim_history(now)
        self._evaluate_all(snapshot, now)
        self.prev_snapshot = snapshot

    def run_forever(self) -> None:
        LOG.info("monitor_start event=running url=%s poll_interval_secs=%.1f rearm_grace_secs=%.1f",
                 self.cfg.metrics_url, self.cfg.poll_interval_secs, self.cfg.rearm_grace_secs)
        while not self.stopping:
            try:
                self.tick()
            except Exception as e:  # last-ditch: never crash the loop on logic bugs in eval
                LOG.exception("tick_exception event=continuing error=%s", e)
            sleep_target = self.backoff_secs
            slept = 0.0
            while slept < sleep_target and not self.stopping:
                time.sleep(min(1.0, sleep_target - slept))
                slept += 1.0
        LOG.info("monitor_stop event=clean_exit")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def _setup_logging(level: str) -> None:
    logging.basicConfig(
        stream=sys.stderr,
        level=getattr(logging, level.upper(), logging.INFO),
        format="%(asctime)sZ level=%(levelname)s logger=%(name)s %(message)s",
        datefmt="%Y-%m-%dT%H:%M:%S",
    )


def cmd_test_alert(cfg: Config) -> int:
    """Send one synthetic alert to verify Telegram delivery, then exit."""
    now = time.time()
    now_iso = utc_now_iso(now)
    spec = AlertSpec(
        alert_id="test_alert",
        severity="WARN",
        window_secs=0.0,
        summary="Synthetic alert from --test-alert",
        playbook=None,
    )
    message = format_alert_message(spec, "FIRE", "this is a delivery smoke test", cfg.env_label, now_iso)
    ok = send_alert(cfg.telegram_bot_token, cfg.telegram_chat_id, message, dry_run=False)
    if ok:
        LOG.info("test_alert event=delivered")
        return 0
    LOG.error("test_alert event=failed")
    return 1


def cmd_once(cfg: Config, dry_run: bool) -> int:
    """Run a single scrape + evaluate pass, then exit. Useful for smoke testing."""
    mon = Monitor(cfg, dry_run=dry_run)
    # Force the first tick to be outside the re-arm grace so alerts can actually fire in --once.
    mon.startup_ts = time.time() - cfg.rearm_grace_secs - 1.0
    mon.tick()
    return 0


def main(argv: Optional[List[str]] = None) -> int:
    ap = argparse.ArgumentParser(description="NOVAI metrics monitor and alerter")
    ap.add_argument("--once", action="store_true",
                    help="Run a single scrape and exit (smoke test)")
    ap.add_argument("--dry-run", action="store_true",
                    help="Never POST to Telegram; log rendered alerts instead")
    ap.add_argument("--test-alert", action="store_true",
                    help="Send one synthetic alert via Telegram and exit")
    ap.add_argument("--log-level", default=None,
                    help="Override NOVAI_MONITOR_LOG_LEVEL (DEBUG/INFO/WARNING/ERROR)")
    args = ap.parse_args(argv)

    cfg = load_config()
    level = args.log_level or cfg.log_level
    _setup_logging(level)
    LOG.info("config_loaded event=startup env_label=%s url=%s log_level=%s",
             cfg.env_label, cfg.metrics_url, level)

    if args.test_alert:
        return cmd_test_alert(cfg)
    if args.once:
        return cmd_once(cfg, args.dry_run)
    Monitor(cfg, dry_run=args.dry_run).run_forever()
    return 0


if __name__ == "__main__":
    sys.exit(main())
