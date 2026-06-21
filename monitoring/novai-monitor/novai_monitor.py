#!/usr/bin/env python3
"""
PURPOSE: Poll the NOVAI node Prometheus metrics endpoints across all four
validators, query their RPC for state_root, evaluate a set of per-node and
cross-node chain health conditions, and deliver alerts to a Telegram chat when
those conditions sustain for their configured windows. Designed to run as a
single systemd service on the testnet host, polling every node over loopback.

INVARIANTS:
- Polling cadence is at least as tight as the smallest alert window so no alert
  is missed by a single skipped scrape.
- A FIRE transition is delivered exactly once per firing episode. A RECOVER
  transition is delivered exactly once when the condition clears. Per-node
  alert state is keyed f"{alert_id}:{label}" so node0 and node1 alerts are
  independent.
- A scrape or RPC failure for one node never aborts the others. An unreachable
  node degrades gracefully: it is dropped from the live set, counts as not
  healthy for fault tolerance, is skipped for divergence, and fires its own
  node_unreachable alert.
- Quorum is decided from healthy validator count (height based), never from
  transport peer_count.
- The re-arm grace at startup suppresses outbound alerts but still seeds
  baselines, so the first real alert observed after grace is correct.
- The script never crashes on transient network or HTTP failure. It logs and
  keeps polling at the fixed interval.

FAILURE MODES:
- Telegram delivery failures are buffered to ALERTS_UNDELIVERED_PATH.
- Metrics endpoint unreachable for a node fires that node's node_unreachable
  alert after its window; per-scrape failures are only logged.
"""

from __future__ import annotations

import argparse
import base64
import dataclasses
import datetime as _dt
import json
import logging
import os
import shutil
import signal
import sys
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from typing import Dict, List, Optional, Set, Tuple

from alerts import (
    DIVERGENCE_DEPTH_BLOCKS,
    HEIGHT_SKEW_BLOCKS,
    NODE_ALERTS,
    SPEC_CLUSTER_HALT,
    SPEC_DIVERGENCE,
    SPEC_DIVERGENCE_SPLIT,
    SPEC_FT_CRITICAL,
    SPEC_FT_DEGRADED,
    SPEC_HOST_DISK_CRIT,
    SPEC_HOST_DISK_LOW,
    SPEC_HOST_MEM_LOW,
    SPEC_NODE_STUCK,
    SPEC_NODE_UNREACHABLE,
    SPEC_TRANSPORT_PEERS_LOW,
    AlertSpec,
    EvalResult,
    classify_divergence,
    cluster_halt_fire,
    fault_tolerance_state,
    healthy_labels,
    host_disk_critical_fire,
    host_disk_low_fire,
    host_mem_low_fire,
    node_stuck_fire,
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

DEFAULT_HOST = "localhost"
DEFAULT_NODE_COUNT = 4
DEFAULT_METRICS_BASE_PORT = 8080
DEFAULT_RPC_BASE_PORT = 3030
DEFAULT_DISK_PATH = "/"
HISTORY_RETENTION_SECS = 900.0  # 15 minutes covers the slowest window (10 min copilot)
ALERTS_UNDELIVERED_PATH = "/var/lib/novai-monitor/alerts_undelivered.jsonl"


@dataclasses.dataclass
class NodeEndpoint:
    label: str
    metrics_url: str
    rpc_url: Optional[str]


@dataclasses.dataclass
class Config:
    nodes: List[NodeEndpoint]
    metrics_user: str
    metrics_pass: str
    poll_interval_secs: float
    http_timeout_secs: float
    rearm_grace_secs: float
    telegram_bot_token: str
    telegram_chat_id: str
    log_level: str
    env_label: str
    undelivered_path: str
    divergence_enabled: bool
    host_checks_enabled: bool
    disk_path: str


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


def _env_int(name: str, default: int) -> int:
    raw = os.environ.get(name, "").strip()
    if not raw:
        return default
    try:
        return int(raw)
    except ValueError:
        LOG.warning("config_bad_int event=using_default name=%s value=%r default=%s",
                    name, raw, default)
        return default


def _env_bool(name: str, default: bool) -> bool:
    raw = os.environ.get(name, "").strip().lower()
    if not raw:
        return default
    return raw in ("1", "true", "yes", "on")


def build_nodes() -> List[NodeEndpoint]:
    """
    Resolve the node list by precedence:
      1. NOVAI_MONITOR_NODES (explicit comma list of label@metrics_url@rpc_url).
      2. NOVAI_MONITOR_NODE_COUNT derives node0..node{N-1} from the base ports.
      3. Legacy NOVAI_MONITOR_METRICS_URL runs one node (cross-node alerts
         stay inert). Keeps an old single-node deployment working unchanged.
      4. Default to four derived nodes.
    """
    host = os.environ.get("NOVAI_MONITOR_HOST", DEFAULT_HOST).strip() or DEFAULT_HOST

    explicit = os.environ.get("NOVAI_MONITOR_NODES", "").strip()
    if explicit:
        nodes: List[NodeEndpoint] = []
        for entry in explicit.split(","):
            entry = entry.strip()
            if not entry:
                continue
            parts = entry.split("@")
            label = parts[0].strip()
            metrics_url = parts[1].strip() if len(parts) > 1 and parts[1].strip() else None
            rpc_url = parts[2].strip() if len(parts) > 2 and parts[2].strip() else None
            if not label or not metrics_url:
                LOG.warning("config_bad_node event=skipped entry=%r", entry)
                continue
            nodes.append(NodeEndpoint(label, metrics_url, rpc_url))
        if nodes:
            return nodes
        LOG.warning("config_nodes_empty event=falling_back_to_derived")

    count_env = os.environ.get("NOVAI_MONITOR_NODE_COUNT", "").strip()
    legacy_url = os.environ.get("NOVAI_MONITOR_METRICS_URL", "").strip()
    if not count_env and legacy_url:
        return [NodeEndpoint("node0", legacy_url, None)]

    count = _env_int("NOVAI_MONITOR_NODE_COUNT", DEFAULT_NODE_COUNT)
    if count < 1:
        count = DEFAULT_NODE_COUNT
    metrics_base = _env_int("NOVAI_MONITOR_METRICS_BASE_PORT", DEFAULT_METRICS_BASE_PORT)
    rpc_base = _env_int("NOVAI_MONITOR_RPC_BASE_PORT", DEFAULT_RPC_BASE_PORT)
    derived: List[NodeEndpoint] = []
    for i in range(count):
        derived.append(NodeEndpoint(
            label=f"node{i}",
            metrics_url=f"http://{host}:{metrics_base + i}/metrics",
            rpc_url=f"http://{host}:{rpc_base + i}",
        ))
    return derived


def load_config() -> Config:
    return Config(
        nodes=build_nodes(),
        metrics_user=os.environ.get("NOVAI_MONITOR_METRICS_USER", ""),
        metrics_pass=os.environ.get("NOVAI_MONITOR_METRICS_PASS", ""),
        poll_interval_secs=_env_float("NOVAI_MONITOR_POLL_INTERVAL_SECS", 30.0),
        http_timeout_secs=_env_float("NOVAI_MONITOR_HTTP_TIMEOUT_SECS", 5.0),
        rearm_grace_secs=_env_float("NOVAI_MONITOR_REARM_GRACE_SECS", 120.0),
        telegram_bot_token=os.environ.get("NOVAI_MONITOR_TELEGRAM_BOT_TOKEN", ""),
        telegram_chat_id=os.environ.get("NOVAI_MONITOR_TELEGRAM_CHAT_ID", ""),
        log_level=os.environ.get("NOVAI_MONITOR_LOG_LEVEL", "INFO"),
        env_label=os.environ.get("NOVAI_MONITOR_ENV_LABEL", "unknown"),
        undelivered_path=os.environ.get("NOVAI_MONITOR_UNDELIVERED_PATH", ALERTS_UNDELIVERED_PATH),
        divergence_enabled=_env_bool("NOVAI_MONITOR_DIVERGENCE_ENABLED", True),
        host_checks_enabled=_env_bool("NOVAI_MONITOR_HOST_CHECKS_ENABLED", True),
        disk_path=os.environ.get("NOVAI_MONITOR_DISK_PATH", DEFAULT_DISK_PATH),
    )


# ---------------------------------------------------------------------------
# HTTP scrape and RPC client (stdlib only)
# ---------------------------------------------------------------------------

def fetch_metrics(
    url: str,
    user: str,
    password: str,
    timeout: float,
) -> Tuple[Optional[Dict[str, float]], Optional[str]]:
    """
    GET one metrics endpoint and parse it. Returns (snapshot, error). On
    success error is None. Never raises.
    """
    req = urllib.request.Request(url, method="GET")
    if user and password:
        creds = f"{user}:{password}".encode("utf-8")
        req.add_header("Authorization", "Basic " + base64.b64encode(creds).decode("ascii"))
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
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


def rpc_call(
    url: str,
    method: str,
    params,
    timeout: float,
) -> Tuple[Optional[dict], Optional[str]]:
    """
    POST a JSON-RPC 2.0 request and return (result, error). On success error is
    None and result is the parsed result object. An error envelope, a null
    result, or any network failure returns (None, reason). Never raises.
    """
    payload = json.dumps({"jsonrpc": "2.0", "method": method, "params": params, "id": 1}).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            if not (200 <= resp.status < 300):
                return None, f"http_{resp.status}"
            body = resp.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as e:
        return None, f"http_{e.code}"
    except (urllib.error.URLError, OSError) as e:
        return None, f"network:{e}"
    try:
        obj = json.loads(body)
    except (ValueError, TypeError):
        return None, "bad_json"
    if not isinstance(obj, dict):
        return None, "bad_envelope"
    err = obj.get("error")
    if err is not None:
        code = err.get("code") if isinstance(err, dict) else err
        return None, f"rpc_error:{code}"
    result = obj.get("result")
    if result is None:
        return None, "no_result"
    return result, None


# ---------------------------------------------------------------------------
# Host resource readers (stdlib only, graceful where /proc is absent)
# ---------------------------------------------------------------------------

def read_disk_free_pct(path: str) -> Optional[float]:
    try:
        usage = shutil.disk_usage(path)
    except OSError:
        return None
    if usage.total <= 0:
        return None
    return usage.free / usage.total * 100.0


def _parse_meminfo_kb(line: str) -> Optional[float]:
    parts = line.split()
    if len(parts) < 2:
        return None
    try:
        return float(parts[1])
    except ValueError:
        return None


def read_mem_available_pct() -> Optional[float]:
    try:
        with open("/proc/meminfo", encoding="utf-8") as f:
            text = f.read()
    except OSError:
        return None
    total: Optional[float] = None
    avail: Optional[float] = None
    for line in text.splitlines():
        if line.startswith("MemTotal:"):
            total = _parse_meminfo_kb(line)
        elif line.startswith("MemAvailable:"):
            avail = _parse_meminfo_kb(line)
    if total is None or avail is None or total <= 0:
        return None
    return avail / total * 100.0


def _short_root(root: Optional[str]) -> str:
    if not root:
        return "?"
    return root if len(root) <= 12 else root[:12] + "..."


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
        self.nodes_by_label: Dict[str, NodeEndpoint] = {n.label: n for n in cfg.nodes}
        # Per-node history of (ts, snapshot) and per-node previous snapshot.
        self.history: Dict[str, List[Tuple[float, Dict[str, float]]]] = {}
        self.prev_snapshot: Dict[str, Dict[str, float]] = {}
        # Alert state is created lazily, keyed per alert and per node label.
        self.states: Dict[str, AlertState] = {}
        self.startup_ts = time.time()
        self.stopping = False
        signal.signal(signal.SIGTERM, self._handle_signal)
        signal.signal(signal.SIGINT, self._handle_signal)

    def _handle_signal(self, signum, _frame) -> None:
        LOG.info("shutdown event=signal signum=%d", signum)
        self.stopping = True

    def _within_rearm_grace(self, now: float) -> bool:
        return (now - self.startup_ts) < self.cfg.rearm_grace_secs

    def _state(self, key: str) -> AlertState:
        return self.states.setdefault(key, AlertState())

    def _drive(self, spec: AlertSpec, suffix: str, firing: bool, detail: str, now: float) -> None:
        """Run one alert key through the FIRE/RECOVER machine and dispatch on a transition."""
        key = spec.alert_id if not suffix else f"{spec.alert_id}:{suffix}"
        state = self._state(key)
        transition = transition_event(spec, state, EvalResult(firing, detail), now)
        if transition is None:
            return
        if self._within_rearm_grace(now):
            LOG.info("alert_suppressed event=rearm_grace alert_id=%s key=%s transition=%s",
                     spec.alert_id, key, transition)
            return
        dispatch_transition(self.cfg, spec, transition, detail, now, self.dry_run)

    # --- scraping -----------------------------------------------------------

    def scrape_all_nodes(self) -> Dict[str, Tuple[Optional[Dict[str, float]], Optional[str]]]:
        """Scrape every node concurrently. One node's failure never aborts the others."""
        results: Dict[str, Tuple[Optional[Dict[str, float]], Optional[str]]] = {}
        if not self.cfg.nodes:
            return results
        with ThreadPoolExecutor(max_workers=len(self.cfg.nodes)) as ex:
            futures = {
                ex.submit(fetch_metrics, n.metrics_url, self.cfg.metrics_user,
                          self.cfg.metrics_pass, self.cfg.http_timeout_secs): n.label
                for n in self.cfg.nodes
            }
            for fut in as_completed(futures):
                label = futures[fut]
                try:
                    results[label] = fut.result()
                except Exception as e:  # belt and suspenders: a worker bug must not kill the cycle
                    results[label] = (None, f"exception:{e}")
        return results

    def collect_rpc_tips(self) -> Dict[str, Dict[str, object]]:
        """Query getLatestBlock on every RPC node concurrently. Returns {label: {height, state_root}}."""
        tips: Dict[str, Dict[str, object]] = {}
        if not self.cfg.divergence_enabled:
            return tips
        rpc_nodes = [n for n in self.cfg.nodes if n.rpc_url]
        if not rpc_nodes:
            return tips
        with ThreadPoolExecutor(max_workers=len(rpc_nodes)) as ex:
            futures = {
                ex.submit(rpc_call, n.rpc_url, "novai_getLatestBlock", None,
                          self.cfg.http_timeout_secs): n.label
                for n in rpc_nodes
            }
            for fut in as_completed(futures):
                label = futures[fut]
                try:
                    result, err = fut.result()
                except Exception as e:
                    result, err = None, f"exception:{e}"
                if result is None:
                    LOG.debug("rpc_tip_unavailable node=%s error=%s", label, err)
                    continue
                height = result.get("height")
                root = result.get("state_root")
                if height is None or root is None:
                    LOG.debug("rpc_tip_incomplete node=%s", label)
                    continue
                tips[label] = {"height": int(height), "state_root": str(root)}
        return tips

    def collect_divergence_roots(self, tips: Dict[str, Dict[str, object]]) -> Dict[str, str]:
        """
        Resolve {label: state_root} at a common height for the responders. If
        all tips are at the same height, compare those roots directly. Otherwise
        fetch each node's block at min(height) - DIVERGENCE_DEPTH_BLOCKS so a
        node that is a block or two behind is never mislabeled as diverged.
        """
        if len(tips) < 2:
            return {}
        heights = [int(t["height"]) for t in tips.values()]
        if len(set(heights)) == 1:
            return {label: str(t["state_root"]) for label, t in tips.items()}
        common_height = min(heights) - DIVERGENCE_DEPTH_BLOCKS
        if common_height < 1:
            return {}
        roots: Dict[str, str] = {}
        labels = sorted(tips)
        with ThreadPoolExecutor(max_workers=len(labels)) as ex:
            futures = {}
            for label in labels:
                node = self.nodes_by_label.get(label)
                if node is None or not node.rpc_url:
                    continue
                fut = ex.submit(rpc_call, node.rpc_url, "novai_getBlockByHeight",
                                {"height": common_height}, self.cfg.http_timeout_secs)
                futures[fut] = label
            for fut in as_completed(futures):
                label = futures[fut]
                try:
                    result, err = fut.result()
                except Exception as e:
                    result, err = None, f"exception:{e}"
                if result is None:
                    LOG.debug("rpc_block_unavailable node=%s height=%d error=%s",
                              label, common_height, err)
                    continue
                root = result.get("state_root")
                if root is not None:
                    roots[label] = str(root)
        return roots

    # --- history ------------------------------------------------------------

    def _update_history(self, label: str, snap: Dict[str, float], now: float) -> None:
        prev = self.prev_snapshot.get(label)
        hist = self.history.setdefault(label, [])
        if (prev is not None
                and "novai_total_txs_committed" in prev
                and "novai_total_txs_committed" in snap
                and snap["novai_total_txs_committed"] < prev["novai_total_txs_committed"]):
            LOG.info("counter_reset event=clearing_history node=%s", label)
            hist.clear()
        hist.append((now, snap))
        cutoff = now - HISTORY_RETENTION_SECS
        self.history[label] = [(ts, s) for ts, s in hist if ts >= cutoff]

    # --- evaluation ---------------------------------------------------------

    def _evaluate_node_alerts(self, snaps: Dict[str, Dict[str, float]], now: float) -> None:
        for label, snap in snaps.items():
            prev = self.prev_snapshot.get(label)
            hist = self.history.get(label, [])
            for spec, evaluator in NODE_ALERTS:
                result = evaluator(snap, prev, hist, now)
                detail = f"node={label} {result.detail}"
                self._drive(spec, label, result.firing, detail, now)

    def _evaluate_cluster_alerts(
        self,
        snaps: Dict[str, Dict[str, float]],
        reachable: Set[str],
        roots: Dict[str, str],
        now: float,
    ) -> None:
        all_labels = [n.label for n in self.cfg.nodes]
        full = len(all_labels)
        heights_now = {
            label: s["novai_committed_height"]
            for label, s in snaps.items() if "novai_committed_height" in s
        }
        heights_prev = {
            label: s["novai_committed_height"]
            for label, s in self.prev_snapshot.items() if "novai_committed_height" in s
        }
        max_now = int(max(heights_now.values())) if heights_now else 0

        # node_unreachable: per node, fire when this cycle's scrape failed.
        for label in all_labels:
            up = label in reachable
            detail = f"node={label} " + ("reachable" if up else "metrics scrape failed")
            self._drive(SPEC_NODE_UNREACHABLE, label, not up, detail, now)

        # transport_peers_low: per node, transport diagnostic only, NOT quorum.
        expected_peers = full - 1
        for label in all_labels:
            snap = snaps.get(label)
            if snap is None or "novai_peer_count" not in snap:
                self._drive(SPEC_TRANSPORT_PEERS_LOW, label, False, f"node={label} insufficient_data", now)
                continue
            peers = int(snap["novai_peer_count"])
            firing = peers < expected_peers
            detail = f"node={label} peers={peers} expected>={expected_peers} (transport only, not quorum)"
            self._drive(SPEC_TRANSPORT_PEERS_LOW, label, firing, detail, now)

        # cluster_halt: handles the single-node case too (the predicate adapts).
        halt = cluster_halt_fire(heights_now, heights_prev, full)
        self._drive(SPEC_CLUSTER_HALT, "", halt, f"cluster_max_height={max_now} (no advance)", now)

        # The remaining cross-node alerts need at least two configured nodes.
        if full < 2:
            return

        # node_stuck: this node flat while at least one other node advanced.
        for label in all_labels:
            firing = node_stuck_fire(label, heights_now, heights_prev)
            if label in heights_now:
                detail = (f"node={label} height={int(heights_now[label])} "
                          f"cluster_max={max_now} (flat while cluster advanced)")
            else:
                detail = f"node={label} height=unknown"
            self._drive(SPEC_NODE_STUCK, label, firing, detail, now)

        # fault tolerance from healthy count (height based, NOT transport peer_count).
        healthy = healthy_labels(heights_now, reachable, HEIGHT_SKEW_BLOCKS)
        healthy_count = len(healthy)
        degraded, critical, quorum = fault_tolerance_state(healthy_count, full)
        ft_detail = f"healthy={healthy_count}/{full} quorum={quorum} skew<={HEIGHT_SKEW_BLOCKS}"
        self._drive(SPEC_FT_DEGRADED, "", degraded, ft_detail, now)
        self._drive(SPEC_FT_CRITICAL, "", critical, ft_detail, now)

        # divergence: majority-group state_roots at the common height.
        verdict = classify_divergence(roots)
        for label in all_labels:
            firing = label in verdict.minority
            if firing:
                detail = (f"node={label} state_root={_short_root(roots.get(label))} "
                          f"!= majority={_short_root(verdict.canonical)}")
            else:
                detail = f"node={label} state_root agrees with majority"
            self._drive(SPEC_DIVERGENCE, label, firing, detail, now)
        split_detail = f"considered={verdict.considered} no_majority_state_root"
        self._drive(SPEC_DIVERGENCE_SPLIT, "", verdict.is_split, split_detail, now)

    def _evaluate_host_alerts(self, now: float) -> None:
        if not self.cfg.host_checks_enabled:
            return
        disk_pct = read_disk_free_pct(self.cfg.disk_path)
        if disk_pct is None:
            disk_detail = f"path={self.cfg.disk_path} free=unknown"
        else:
            disk_detail = f"path={self.cfg.disk_path} free={disk_pct:.1f}%"
        self._drive(SPEC_HOST_DISK_CRIT, "", host_disk_critical_fire(disk_pct), disk_detail, now)
        self._drive(SPEC_HOST_DISK_LOW, "", host_disk_low_fire(disk_pct), disk_detail, now)
        mem_pct = read_mem_available_pct()
        mem_detail = f"mem_available={mem_pct:.1f}%" if mem_pct is not None else "mem_available=unknown"
        self._drive(SPEC_HOST_MEM_LOW, "", host_mem_low_fire(mem_pct), mem_detail, now)

    # --- tick ---------------------------------------------------------------

    def tick(self) -> None:
        now = time.time()
        scrape_results = self.scrape_all_nodes()
        snaps: Dict[str, Dict[str, float]] = {}
        for label, (snap, err) in scrape_results.items():
            if snap is None:
                LOG.warning("scrape_failed node=%s error=%s", label, err)
            else:
                snaps[label] = snap
        reachable = set(snaps.keys())

        for label, snap in snaps.items():
            self._update_history(label, snap, now)

        tips = self.collect_rpc_tips()
        roots = self.collect_divergence_roots(tips)

        self._evaluate_node_alerts(snaps, now)
        self._evaluate_cluster_alerts(snaps, reachable, roots, now)
        self._evaluate_host_alerts(now)

        for label, snap in snaps.items():
            self.prev_snapshot[label] = snap

    def run_forever(self) -> None:
        LOG.info("monitor_start event=running nodes=%d poll_interval_secs=%.1f "
                 "rearm_grace_secs=%.1f divergence=%s host_checks=%s",
                 len(self.cfg.nodes), self.cfg.poll_interval_secs, self.cfg.rearm_grace_secs,
                 self.cfg.divergence_enabled, self.cfg.host_checks_enabled)
        while not self.stopping:
            try:
                self.tick()
            except Exception as e:  # last-ditch: never crash the loop on a logic bug in eval
                LOG.exception("tick_exception event=continuing error=%s", e)
            target = self.cfg.poll_interval_secs
            slept = 0.0
            while slept < target and not self.stopping:
                time.sleep(min(1.0, target - slept))
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
    """Run a single scrape and evaluate pass, then exit. Useful for smoke testing."""
    mon = Monitor(cfg, dry_run=dry_run)
    # Force the first tick to be outside the re-arm grace so alerts can fire in --once.
    mon.startup_ts = time.time() - cfg.rearm_grace_secs - 1.0
    mon.tick()
    return 0


def main(argv: Optional[List[str]] = None) -> int:
    ap = argparse.ArgumentParser(description="NOVAI cross-node metrics monitor and alerter")
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
    node_labels = ",".join(n.label for n in cfg.nodes)
    LOG.info("config_loaded event=startup env_label=%s nodes=%s log_level=%s",
             cfg.env_label, node_labels, level)

    if args.test_alert:
        return cmd_test_alert(cfg)
    if args.once:
        return cmd_once(cfg, args.dry_run)
    Monitor(cfg, dry_run=args.dry_run).run_forever()
    return 0


if __name__ == "__main__":
    sys.exit(main())
