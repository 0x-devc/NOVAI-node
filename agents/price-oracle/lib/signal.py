"""PURPOSE: Deterministic data_hash builder for the OracleAnchor payload.

INVARIANTS:
- Same (price, timestamp) produces the same data_hash byte-for-byte.
- Different price (down to the canonical 2 decimal places) or different
  timestamp produces a different data_hash.
- Encoding is canonical ASCII per OBSERVATION_FORMAT; never depends on
  Python's float repr.

FAILURE MODES:
- ValueError on non-finite or non-positive price.
- ValueError on non-positive or out-of-range timestamp.
"""

from __future__ import annotations

import math

from novai_sdk.crypto import blake3_hash

DATA_TAG = "price/BTC-USD"
OBSERVATION_FORMAT = "BTC-USD@{ts}={price:.2f}"


def canonical_observation_bytes(price_usd: float, timestamp: int) -> bytes:
    """Return the exact ASCII bytes hashed for an observation."""
    if not math.isfinite(price_usd):
        raise ValueError(f"price must be finite, got {price_usd}")
    if price_usd <= 0:
        raise ValueError(f"price must be positive, got {price_usd}")
    if not 0 < timestamp < 2**63:
        raise ValueError(f"timestamp must be positive and fit in i63, got {timestamp}")
    return OBSERVATION_FORMAT.format(ts=int(timestamp), price=float(price_usd)).encode("ascii")


def build_data_hash(price_usd: float, timestamp: int) -> bytes:
    """Compute the 32-byte blake3 of the canonical observation bytes."""
    return blake3_hash(canonical_observation_bytes(price_usd, timestamp))
