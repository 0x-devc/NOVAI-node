"""PURPOSE: Canonical observation encoding and signal byte construction.

This module turns a GPU price observation into the exact on-chain bytes for
an OracleAnchor (signal type 22) and, optionally, a ReputationUpdate (signal
type 7). All construction goes through the novai_sdk builders so the bytes
match the protocol source of truth, not a hand-rolled copy.

INVARIANTS:
- Same (model, price, timestamp) produces the same data_hash byte for byte.
  The encoding is canonical ASCII per OBSERVATION_FORMAT and never depends on
  Python's float repr; the price is fixed to 4 decimals.
- The OracleAnchor signal_hash, extras, and payload come from
  novai_sdk.signals.oracle and novai_sdk.tx.signal verbatim.

NEVER-LIE:
- data_hash commits only to a price the caller actually computed. The loop
  skips a cycle on missing data rather than hashing a placeholder.
- source_hash is the hash of the source identifier the agent queried.

KNOWN SDK GAP (flagged for the held live step):
- novai_sdk ships build_reputation_update_extras but no
  derive_reputation_update_signal_hash and no high-level post_reputation_update.
  build_reputation_update below derives a LOCAL content id for the dry-run
  envelope. The chain's canonical reputation signal-hash derivation must be
  confirmed before any live reputation submission.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

from novai_sdk.crypto import blake3_hash, blake3_keyed
from novai_sdk.enums import AiSignalType
from novai_sdk.signals.oracle import (
    build_oracle_anchor_extras,
    derive_oracle_anchor_signal_hash,
)
from novai_sdk.signals.reputation import build_reputation_update_extras
from novai_sdk.tx.signal import build_signal_commitment_payload

# Canonical observation string. model is the normalized label (e.g. RTX4090),
# price is per-GPU on-demand USD per hour fixed to 4 decimals.
OBSERVATION_FORMAT = "GPU-{model}-USD-HR@{ts}={price:.4f}"

# Local domain tag for the dry-run reputation content id (see KNOWN SDK GAP).
_REPUTATION_LOCAL_DOMAIN = b"novai-compute-oracle-reputation-local-v1"


def canonical_observation_bytes(model: str, price_usd_per_hour: float, timestamp: int) -> bytes:
    """Return the exact ASCII bytes hashed for an observation."""
    if not isinstance(model, str) or not model:
        raise ValueError("model must be a non-empty string")
    if not math.isfinite(price_usd_per_hour):
        raise ValueError(f"price must be finite, got {price_usd_per_hour}")
    if price_usd_per_hour <= 0:
        raise ValueError(f"price must be positive, got {price_usd_per_hour}")
    if not 0 < timestamp < 2**63:
        raise ValueError(f"timestamp must be positive and fit in i63, got {timestamp}")
    return OBSERVATION_FORMAT.format(
        model=model, ts=int(timestamp), price=float(price_usd_per_hour)
    ).encode("ascii")


def build_data_hash(model: str, price_usd_per_hour: float, timestamp: int) -> bytes:
    """Compute the 32-byte blake3 of the canonical observation bytes."""
    return blake3_hash(canonical_observation_bytes(model, price_usd_per_hour, timestamp))


def source_hash_for(source_id: str | bytes) -> bytes:
    """Compute the 32-byte provenance hash of the source identifier queried."""
    raw = source_id.encode("utf-8") if isinstance(source_id, str) else source_id
    return blake3_hash(raw)


@dataclass(frozen=True)
class OracleAnchorArtifacts:
    issuer_entity_id: bytes
    data_hash: bytes
    external_timestamp: int
    source_hash: bytes
    expiry_height: int
    data_tag: str
    signal_hash: bytes
    extras: bytes
    payload: bytes


def build_oracle_anchor(
    issuer_entity_id: bytes,
    data_hash: bytes,
    external_timestamp: int,
    source_hash: bytes,
    expiry_height: int,
    data_tag: str,
) -> OracleAnchorArtifacts:
    """Construct the full OracleAnchor commitment payload via the SDK builders.

    Layout (66-byte base + 82..=113-byte tail):
        [0x02][signal_hash:32][22][issuer:32]
        [data_hash:32][ext_ts:8 BE][source_hash:32][expiry:8 BE][tag_len:1][tag]
    """
    signal_hash = derive_oracle_anchor_signal_hash(
        issuer_entity_id=issuer_entity_id,
        data_hash=data_hash,
        external_timestamp=external_timestamp,
        source_hash=source_hash,
        data_tag=data_tag,
    )
    extras = build_oracle_anchor_extras(
        data_hash=data_hash,
        external_timestamp=external_timestamp,
        source_hash=source_hash,
        expiry_height=expiry_height,
        data_tag=data_tag,
    )
    payload = build_signal_commitment_payload(
        signal_hash, AiSignalType.ORACLE_ANCHOR, issuer_entity_id, extras
    )
    return OracleAnchorArtifacts(
        issuer_entity_id=issuer_entity_id,
        data_hash=data_hash,
        external_timestamp=external_timestamp,
        source_hash=source_hash,
        expiry_height=expiry_height,
        data_tag=data_tag,
        signal_hash=signal_hash,
        extras=extras,
        payload=payload,
    )


@dataclass(frozen=True)
class ReputationArtifacts:
    issuer_entity_id: bytes
    target_entity_id: bytes
    event_type: int
    points_delta: int
    signal_hash: bytes
    extras: bytes
    payload: bytes


def _derive_local_reputation_signal_hash(
    issuer_entity_id: bytes,
    target_entity_id: bytes,
    event_type: int,
    points_delta: int,
) -> bytes:
    """Local content id for the dry-run reputation envelope. See KNOWN SDK GAP."""
    return blake3_keyed(
        _REPUTATION_LOCAL_DOMAIN,
        issuer_entity_id,
        target_entity_id,
        bytes([event_type & 0xFF]),
        points_delta.to_bytes(2, "big", signed=True),
    )


def build_reputation_update(
    issuer_entity_id: bytes,
    target_entity_id: bytes,
    event_type: int,
    points_delta: int,
) -> ReputationArtifacts:
    """Construct the ReputationUpdate commitment payload (35-byte tail) via the SDK.

    The extras tail comes from novai_sdk.signals.reputation. The signal_hash is
    a local content id (the SDK ships no canonical derivation for this type).
    """
    extras = build_reputation_update_extras(target_entity_id, event_type, points_delta)
    signal_hash = _derive_local_reputation_signal_hash(
        issuer_entity_id, target_entity_id, event_type, points_delta
    )
    payload = build_signal_commitment_payload(
        signal_hash, AiSignalType.REPUTATION_UPDATE, issuer_entity_id, extras
    )
    return ReputationArtifacts(
        issuer_entity_id=issuer_entity_id,
        target_entity_id=target_entity_id,
        event_type=event_type,
        points_delta=points_delta,
        signal_hash=signal_hash,
        extras=extras,
        payload=payload,
    )
