"""MetricsRegistry render + HTTPServer integration."""

from __future__ import annotations

import time
import urllib.error
import urllib.request

import pytest

from lib.metrics import (
    MetricsRegistry,
    build_oracle_registry,
    start_metrics_server,
)


def test_counter_inc_renders_value():
    reg = MetricsRegistry(time.monotonic())
    reg.declare_counter("foo_total", "help")
    reg.inc_counter("foo_total")
    reg.inc_counter("foo_total")
    text = reg.render()
    assert "# TYPE foo_total counter" in text
    assert "foo_total 2" in text


def test_counter_with_label_renders_all_declared_values_even_at_zero():
    reg = MetricsRegistry(time.monotonic())
    reg.declare_counter(
        "bar_total", "help", label_name="reason", label_values=("a", "b")
    )
    reg.inc_counter("bar_total", "a")
    text = reg.render()
    assert 'bar_total{reason="a"} 1' in text
    assert 'bar_total{reason="b"} 0' in text


def test_gauge_set_and_render():
    reg = MetricsRegistry(time.monotonic())
    reg.declare_gauge("baz", "help")
    reg.set_gauge("baz", 42.5)
    text = reg.render()
    assert "# TYPE baz gauge" in text
    assert "baz 42.5" in text


def test_undeclared_counter_inc_raises():
    reg = MetricsRegistry(time.monotonic())
    with pytest.raises(KeyError):
        reg.inc_counter("never_declared")


def test_undeclared_gauge_set_raises():
    reg = MetricsRegistry(time.monotonic())
    with pytest.raises(KeyError):
        reg.set_gauge("never_declared", 1.0)


def test_uptime_gauge_advances():
    start = time.monotonic() - 5.0
    reg = MetricsRegistry(start)
    reg.declare_gauge("novai_oracle_uptime_seconds", "help")
    text = reg.render()
    value_line = [line for line in text.splitlines() if line.startswith("novai_oracle_uptime_seconds ")][0]
    value = float(value_line.split()[1])
    assert value >= 5.0


def test_oracle_registry_declares_all_locked_metrics():
    reg = build_oracle_registry(time.monotonic())
    text = reg.render()
    for name in (
        "novai_oracle_price_fetch_success_total",
        "novai_oracle_price_fetch_failure_total",
        "novai_oracle_submission_success_total",
        "novai_oracle_submission_failure_total",
        "novai_oracle_last_price_usd",
        "novai_oracle_last_submission_height",
        "novai_oracle_last_loop_completed_timestamp",
        "novai_oracle_uptime_seconds",
    ):
        assert f"# TYPE {name} " in text


def test_metrics_http_server_serves_text():
    reg = build_oracle_registry(time.monotonic())
    reg.inc_counter("novai_oracle_price_fetch_success_total")
    reg.set_gauge("novai_oracle_last_price_usd", 67234.51)
    server = start_metrics_server("127.0.0.1", 0, reg)
    try:
        port = server.server_address[1]
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/metrics", timeout=2.0) as resp:
            body = resp.read().decode("utf-8")
            content_type = resp.headers.get("Content-Type", "")
        assert "text/plain" in content_type
        assert "novai_oracle_price_fetch_success_total 1" in body
        assert "novai_oracle_last_price_usd" in body
    finally:
        server.shutdown()
        server.server_close()


def test_metrics_http_server_404_for_other_paths():
    reg = build_oracle_registry(time.monotonic())
    server = start_metrics_server("127.0.0.1", 0, reg)
    try:
        port = server.server_address[1]
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2.0)
            raise AssertionError("expected 404")
        except urllib.error.HTTPError as exc:
            assert exc.code == 404
    finally:
        server.shutdown()
        server.server_close()
