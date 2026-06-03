"""
PURPOSE: Parse the Prometheus text exposition format produced by the
NOVAI node /metrics endpoint into a flat dict of {metric_name: float}.

INVARIANTS:
- Returns only metric samples; HELP and TYPE comment lines are skipped.
- Unlabeled gauges and counters only (matches what crates/node/src/metrics.rs emits).
- Last-write-wins if a metric appears twice (should not happen in NOVAI output).

FAILURE MODES:
- Malformed lines are skipped with a logged warning, not raised. A degraded
  scrape is more useful than a crashed monitor.
"""

from __future__ import annotations

import logging
from typing import Dict

log = logging.getLogger("novai_monitor.parser")


def parse_prometheus_text(text: str) -> Dict[str, float]:
    """
    Parse Prometheus exposition text into {metric_name: float}.

    Accepts only unlabeled samples of the form `name value` (with optional
    trailing timestamp, which is ignored). Lines starting with `#` are
    comments and are skipped. Lines that fail to parse are logged and
    skipped so a single bad line cannot stop the monitor.
    """
    out: Dict[str, float] = {}
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split()
        if len(parts) < 2:
            log.warning("parser_skip event=short_line line=%r", line)
            continue
        name = parts[0]
        if "{" in name:
            log.warning("parser_skip event=labeled_metric_unsupported name=%s", name)
            continue
        try:
            value = float(parts[1])
        except ValueError:
            log.warning("parser_skip event=bad_value name=%s value=%r", name, parts[1])
            continue
        out[name] = value
    return out
