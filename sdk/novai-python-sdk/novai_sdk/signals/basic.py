"""Signal types 0-6: empty-extras signals.

Anomaly, Optimization, Prediction, RiskScore, AuditReport, SpamRisk, and
CongestionForecast all carry no inline extras; the signal commitment
payload is the 66-byte envelope alone.
"""

from __future__ import annotations


def build_empty_extras() -> bytes:
    """Return the empty extras byte string used by signal types 0-6."""
    return b""
