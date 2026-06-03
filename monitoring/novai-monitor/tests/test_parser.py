"""Test that the Prometheus text parser extracts what the NOVAI node emits."""
import os

from parser import parse_prometheus_text

FIXTURES = os.path.join(os.path.dirname(__file__), "fixtures")


def _load(name: str) -> str:
    with open(os.path.join(FIXTURES, name), encoding="utf-8") as f:
        return f.read()


def test_parser_healthy_extracts_all_named_metrics():
    snap = parse_prometheus_text(_load("metrics_healthy.txt"))
    assert snap["novai_committed_height"] == 184231.0
    assert snap["novai_current_round"] == 1.0
    assert snap["novai_peer_count"] == 4.0
    assert snap["novai_mempool_size"] == 25.0
    assert snap["novai_consensus_view_changes_total"] == 7.0
    assert snap["novai_block_tx_count"] == 5.0
    assert snap["novai_total_txs_committed"] == 50000.0
    assert snap["novai_copilot_observations_total"] == 1234.0
    assert snap["novai_anomaly_signals_total"] == 0.0
    assert snap["novai_anomaly_signals_published"] == 0.0
    assert snap["novai_anomaly_last_confidence"] == 0.0


def test_parser_skips_help_and_type_comments():
    snap = parse_prometheus_text(_load("metrics_healthy.txt"))
    # 11 metrics in the healthy fixture, no extras for comment lines.
    assert len(snap) == 11


def test_parser_handles_blank_lines_and_malformed_input():
    text = """
# HELP a a gauge
# TYPE a gauge
a 1.5

# malformed below this point
not_a_metric
b not_a_number
c 3
"""
    snap = parse_prometheus_text(text)
    assert snap == {"a": 1.5, "c": 3.0}


def test_parser_treats_labeled_metrics_as_unsupported():
    # The node currently emits no labeled metrics; if that changes we want a
    # loud skip rather than silent dropping into a label-stripped dict.
    text = 'novai_thing{instance="node-0"} 42\n'
    snap = parse_prometheus_text(text)
    assert snap == {}


def test_parser_returns_empty_on_empty_input():
    assert parse_prometheus_text("") == {}
    assert parse_prometheus_text("\n\n# only comments\n") == {}
