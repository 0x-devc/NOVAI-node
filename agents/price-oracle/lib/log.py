"""PURPOSE: Structured logger setup matching monitoring/novai-monitor.

INVARIANTS:
- Format string is exactly:
    "%(asctime)sZ level=%(levelname)s logger=%(name)s %(message)s"
- Timestamps are UTC via gmtime.
- Messages use lazy "event=foo key=%s" formatting, never f-strings inside
  LOG.info(...) calls.

FAILURE MODES:
- None. Pure configuration.
"""

from __future__ import annotations

import logging
import time

LOG_FORMAT = "%(asctime)sZ level=%(levelname)s logger=%(name)s %(message)s"
DATE_FORMAT = "%Y-%m-%dT%H:%M:%S"


def configure_logging(level: str = "INFO") -> None:
    """Install a single stderr handler with the novai-monitor format."""
    logging.Formatter.converter = time.gmtime
    handler = logging.StreamHandler()
    handler.setFormatter(logging.Formatter(LOG_FORMAT, DATE_FORMAT))
    root = logging.getLogger()
    for existing in list(root.handlers):
        root.removeHandler(existing)
    root.addHandler(handler)
    root.setLevel(level.upper())
