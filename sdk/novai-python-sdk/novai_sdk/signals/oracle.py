"""Signal type 22: OracleAnchor (Week 35).

Extras layout (variable 82..=113 bytes)::

    [data_hash:32][external_timestamp_be:8][source_hash:32]
    [expiry_height_be:8][data_tag_len:1][data_tag:1..=32]

The signal hash is content-addressed: identical inputs hash to identical
signal hashes (and the chain rejects duplicates as replays). The CLI uses::

    blake3("novai-oracle-anchor-v1" || issuer || data_hash ||
           external_timestamp_be || source_hash || tag_len_be:u32 || data_tag)

This module exposes that derivation as :func:`derive_oracle_anchor_signal_hash`.
"""

from __future__ import annotations

import blake3

from novai_sdk._hex import coerce_address, coerce_hash32
from novai_sdk.constants import (
    ORACLE_ANCHOR_DATA_TAG_MAX_LEN,
    ORACLE_ANCHOR_DATA_TAG_MIN_LEN,
)


def _coerce_tag(tag: bytes | str) -> bytes:
    tag_bytes = tag.encode("utf-8") if isinstance(tag, str) else tag
    if not ORACLE_ANCHOR_DATA_TAG_MIN_LEN <= len(tag_bytes) <= ORACLE_ANCHOR_DATA_TAG_MAX_LEN:
        raise ValueError(
            f"data_tag must be {ORACLE_ANCHOR_DATA_TAG_MIN_LEN}..="
            f"{ORACLE_ANCHOR_DATA_TAG_MAX_LEN} bytes, got {len(tag_bytes)}"
        )
    return tag_bytes


def derive_oracle_anchor_signal_hash(
    issuer_entity_id: bytes | str,
    data_hash: bytes | str,
    external_timestamp: int,
    source_hash: bytes | str | None,
    data_tag: bytes | str,
) -> bytes:
    """Derive the canonical content-addressed signal hash for an OracleAnchor.

    Identical inputs produce identical hashes (and the chain rejects them as
    replays). The tag length is encoded as a u32 big-endian in the hash
    input, matching the CLI implementation.
    """
    issuer = coerce_address(issuer_entity_id, field="issuer_entity_id")
    dh = coerce_hash32(data_hash, field="data_hash")
    sh = bytes(32) if source_hash is None else coerce_hash32(source_hash, field="source_hash")
    tag = _coerce_tag(data_tag)
    if not 0 <= external_timestamp < 2**64:
        raise ValueError("external_timestamp must fit in u64")
    hasher = blake3.blake3()
    hasher.update(b"novai-oracle-anchor-v1")
    hasher.update(issuer)
    hasher.update(dh)
    hasher.update(external_timestamp.to_bytes(8, "big"))
    hasher.update(sh)
    hasher.update(len(tag).to_bytes(4, "big"))
    hasher.update(tag)
    return hasher.digest()


def build_oracle_anchor_extras(
    data_hash: bytes | str,
    external_timestamp: int,
    source_hash: bytes | str | None,
    expiry_height: int,
    data_tag: bytes | str,
) -> bytes:
    """Build the OracleAnchor extras tail (82..=113 bytes)."""
    if not 0 < external_timestamp < 2**64:
        raise ValueError(f"external_timestamp must be > 0 and fit in u64, got {external_timestamp}")
    if not 0 <= expiry_height < 2**64:
        raise ValueError(f"expiry_height must fit in u64, got {expiry_height}")
    dh = coerce_hash32(data_hash, field="data_hash")
    if dh == bytes(32):
        raise ValueError("data_hash must be non-zero")
    sh = bytes(32) if source_hash is None else coerce_hash32(source_hash, field="source_hash")
    tag = _coerce_tag(data_tag)
    return (
        dh
        + external_timestamp.to_bytes(8, "big")
        + sh
        + expiry_height.to_bytes(8, "big")
        + bytes([len(tag)])
        + tag
    )
