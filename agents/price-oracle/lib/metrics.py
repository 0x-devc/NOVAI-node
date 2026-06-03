"""PURPOSE: Thread-safe Prometheus text registry plus stdlib HTTPServer.

INVARIANTS:
- All mutations and reads of the metric maps happen under self._lock.
- /metrics renders all DECLARED metrics in stable (sorted) order even
  before any increment, so scrapers see zeros instead of disappearing
  series.
- novai_oracle_uptime_seconds is computed at render time from the
  process start time, not stored as a state variable.

FAILURE MODES:
- KeyError on inc_counter / set_gauge for an undeclared metric (caller
  bug; surfaces during tests).
- OSError on HTTPServer bind (port already in use); oracle.py logs and
  exits non-zero so systemd restarts after the port is free.
"""

from __future__ import annotations

import logging
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Optional

LOG = logging.getLogger("price_oracle.metrics")


class MetricsRegistry:
    """Tiny in-memory metrics registry that renders Prometheus text format.

    Labels are limited to a single name=value pair per counter (sufficient
    for the locked metric set; reason="..." on the two failure counters).
    """

    def __init__(self, start_time_monotonic: float) -> None:
        self._lock = threading.Lock()
        self._start = start_time_monotonic
        self._counters: dict[str, dict[tuple[str, str], int]] = {}
        self._counter_help: dict[str, str] = {}
        self._counter_label: dict[str, str] = {}
        self._gauges: dict[str, Optional[float]] = {}
        self._gauge_help: dict[str, str] = {}

    def declare_counter(
        self,
        name: str,
        help_text: str,
        label_name: str = "",
        label_values: tuple[str, ...] = (),
    ) -> None:
        with self._lock:
            self._counter_help[name] = help_text
            self._counter_label[name] = label_name
            existing = self._counters.setdefault(name, {})
            if label_name:
                for value in label_values:
                    existing.setdefault((label_name, value), 0)
            else:
                existing.setdefault(("", ""), 0)

    def declare_gauge(self, name: str, help_text: str) -> None:
        with self._lock:
            self._gauge_help[name] = help_text
            self._gauges.setdefault(name, None)

    def inc_counter(self, name: str, label_value: str = "") -> None:
        with self._lock:
            if name not in self._counters:
                raise KeyError(f"counter not declared: {name}")
            label_name = self._counter_label[name]
            if label_name:
                key = (label_name, label_value)
            else:
                key = ("", "")
            self._counters[name][key] = self._counters[name].get(key, 0) + 1

    def set_gauge(self, name: str, value: float) -> None:
        with self._lock:
            if name not in self._gauges:
                raise KeyError(f"gauge not declared: {name}")
            self._gauges[name] = float(value)

    def render(self) -> str:
        with self._lock:
            counters_snapshot = {
                name: dict(entries) for name, entries in self._counters.items()
            }
            counter_help = dict(self._counter_help)
            gauges_snapshot = dict(self._gauges)
            gauge_help = dict(self._gauge_help)
            uptime = time.monotonic() - self._start

        lines: list[str] = []
        for name in sorted(counter_help):
            lines.append(f"# HELP {name} {counter_help[name]}")
            lines.append(f"# TYPE {name} counter")
            entries = counters_snapshot.get(name, {})
            if not entries:
                lines.append(f"{name} 0")
                continue
            for key in sorted(entries):
                label_name, label_value = key
                value = entries[key]
                if label_name:
                    lines.append(f'{name}{{{label_name}="{label_value}"}} {value}')
                else:
                    lines.append(f"{name} {value}")
        for name in sorted(gauge_help):
            lines.append(f"# HELP {name} {gauge_help[name]}")
            lines.append(f"# TYPE {name} gauge")
            if name == "novai_oracle_uptime_seconds":
                lines.append(f"{name} {uptime:.1f}")
                continue
            value = gauges_snapshot.get(name)
            if value is None:
                lines.append(f"{name} 0")
            else:
                lines.append(f"{name} {_format_gauge(value)}")
        return "\n".join(lines) + "\n"


def _format_gauge(value: float) -> str:
    if float(value).is_integer():
        return str(int(value))
    return f"{value:.6f}".rstrip("0").rstrip(".")


class _Handler(BaseHTTPRequestHandler):
    registry: MetricsRegistry

    def do_GET(self) -> None:  # noqa: N802
        if self.path != "/metrics":
            self.send_error(404, "not found")
            return
        body = self.registry.render().encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:  # noqa: A002
        return


def start_metrics_server(host: str, port: int, registry: MetricsRegistry) -> HTTPServer:
    """Start a daemon-thread HTTPServer; return it so the caller can shut it down."""
    handler_cls = type("_BoundHandler", (_Handler,), {"registry": registry})
    server = HTTPServer((host, port), handler_cls)
    thread = threading.Thread(
        target=server.serve_forever, name="metrics-http", daemon=True
    )
    thread.start()
    LOG.info("metrics_server event=started host=%s port=%d", host, port)
    return server


def build_oracle_registry(start_time_monotonic: float) -> MetricsRegistry:
    """Declare every metric named in the locked architecture.

    Reason labels match the values produced by lib.chain.map_submit_error
    and lib.coingecko exception classes.
    """
    reg = MetricsRegistry(start_time_monotonic)
    reg.declare_counter(
        "novai_oracle_price_fetch_success_total",
        "Number of successful CoinGecko fetches.",
    )
    reg.declare_counter(
        "novai_oracle_price_fetch_failure_total",
        "Number of failed CoinGecko fetches, by reason.",
        label_name="reason",
        label_values=("rate_limit", "server_error", "network_error", "parse_error"),
    )
    reg.declare_counter(
        "novai_oracle_submission_success_total",
        "Number of OracleAnchor signals accepted by the chain.",
    )
    reg.declare_counter(
        "novai_oracle_submission_failure_total",
        "Number of OracleAnchor submissions rejected or failed, by reason.",
        label_name="reason",
        label_values=(
            "rpc_unreachable",
            "rpc_error",
            "rpc_rate_limited",
            "fee_too_low",
            "nonce_too_low",
            "mempool_full",
            "sender_limit",
            "validation_failed",
            "entity_not_registered",
            "encoding_error",
        ),
    )
    reg.declare_gauge(
        "novai_oracle_last_price_usd", "Most recent observed BTC/USD price."
    )
    reg.declare_gauge(
        "novai_oracle_last_submission_height",
        "Chain head height at the moment of the last successful submission.",
    )
    reg.declare_gauge(
        "novai_oracle_last_loop_completed_timestamp",
        "Unix epoch seconds when the last loop iteration finished (success or fail).",
    )
    reg.declare_gauge(
        "novai_oracle_uptime_seconds",
        "Seconds since the oracle process started.",
    )
    return reg
