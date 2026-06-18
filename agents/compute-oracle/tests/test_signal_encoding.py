"""Signal construction: canonical observation, OracleAnchor, ReputationUpdate.

The centerpiece is test_oracle_anchor_matches_rust_cli_golden_vector, which
proves the SDK construction path reproduces the exact 160-byte payload pinned
by the Rust CLI test ``anchor_payload_layout_is_correct`` in
tools/novai-cli/src/commands/oracle.rs.
"""

from __future__ import annotations

import pytest
from novai_sdk.enums import AiSignalType
from novai_sdk.signals.oracle import build_oracle_anchor_extras
from novai_sdk.tx.signal import build_signal_commitment_payload

from lib import signal as sig


# -- Canonical observation ---------------------------------------------------


def test_observation_is_canonical_ascii():
    raw = sig.canonical_observation_bytes("RTX4090", 0.34, 1_718_000_000)
    assert raw == b"GPU-RTX4090-USD-HR@1718000000=0.3400"


def test_observation_fixes_four_decimals():
    raw = sig.canonical_observation_bytes("RTX4090", 0.1, 1_718_000_000)
    assert raw.endswith(b"=0.1000")


def test_observation_truncates_extra_precision():
    raw = sig.canonical_observation_bytes("RTX4090", 0.123456, 1_718_000_000)
    assert raw.endswith(b"=0.1235")


def test_data_hash_is_deterministic_and_32_bytes():
    a = sig.build_data_hash("RTX4090", 0.34, 1_718_000_000)
    b = sig.build_data_hash("RTX4090", 0.34, 1_718_000_000)
    assert a == b
    assert len(a) == 32


def test_data_hash_changes_with_price_or_timestamp_or_model():
    base = sig.build_data_hash("RTX4090", 0.34, 1_718_000_000)
    assert base != sig.build_data_hash("RTX4090", 0.35, 1_718_000_000)
    assert base != sig.build_data_hash("RTX4090", 0.34, 1_718_000_001)
    assert base != sig.build_data_hash("A100", 0.34, 1_718_000_000)


@pytest.mark.parametrize("bad_price", [0.0, -1.0, float("inf"), float("nan")])
def test_observation_rejects_bad_price(bad_price):
    with pytest.raises(ValueError):
        sig.canonical_observation_bytes("RTX4090", bad_price, 1_718_000_000)


@pytest.mark.parametrize("bad_ts", [0, -1, 2**63])
def test_observation_rejects_bad_timestamp(bad_ts):
    with pytest.raises(ValueError):
        sig.canonical_observation_bytes("RTX4090", 0.34, bad_ts)


def test_source_hash_is_deterministic_and_32_bytes():
    a = sig.source_hash_for("vast.ai/api/v0/bundles")
    assert len(a) == 32
    assert a == sig.source_hash_for(b"vast.ai/api/v0/bundles")


# -- Golden vector against the Rust CLI --------------------------------------


def test_oracle_anchor_matches_rust_cli_golden_vector():
    """Reproduce tools/novai-cli/.../oracle.rs::anchor_payload_layout_is_correct.

    The SDK builders must produce the same 160-byte payload as the Rust CLI for
    identical inputs. This binds the Python construction path to the protocol
    source of truth rather than to itself.
    """
    signal_hash = bytes([0x10] * 32)
    issuer = bytes([0x01] * 32)
    data_hash = bytes([0xAB] * 32)
    external_timestamp = 0x0102_0304_0506_0708
    source_hash = bytes([0xCD] * 32)
    expiry_height = 5000
    data_tag = b"price/ETH-USD"  # 13 bytes

    extras = build_oracle_anchor_extras(
        data_hash=data_hash,
        external_timestamp=external_timestamp,
        source_hash=source_hash,
        expiry_height=expiry_height,
        data_tag=data_tag,
    )
    payload = build_signal_commitment_payload(
        signal_hash, AiSignalType.ORACLE_ANCHOR, issuer, extras
    )

    assert len(payload) == 66 + 81 + 13  # 160
    assert payload[0] == 2  # commitment version
    assert payload[1:33] == signal_hash
    assert payload[33] == 22  # OracleAnchor signal type
    assert payload[34:66] == issuer
    assert payload[66:98] == data_hash
    assert payload[98:106] == external_timestamp.to_bytes(8, "big")
    assert payload[106:138] == source_hash
    assert payload[138:146] == (5000).to_bytes(8, "big")
    assert payload[146] == 13
    assert payload[147:160] == data_tag


# -- The agent's own construction wires the SDK correctly --------------------


def test_build_oracle_anchor_layout_and_signal_hash_consistency():
    issuer = bytes([0x07] * 32)
    data_hash = sig.build_data_hash("RTX4090", 0.34, 1_718_000_000)
    source_hash = sig.source_hash_for("vast.ai/api/v0/bundles")
    ts = 1_718_000_000
    tag = "compute/rtx4090-usd-hr"

    art = sig.build_oracle_anchor(issuer, data_hash, ts, source_hash, 0, tag)

    tag_bytes = tag.encode("utf-8")
    assert len(art.payload) == 66 + 81 + len(tag_bytes)
    assert art.payload[0] == 2
    assert art.payload[1:33] == art.signal_hash
    assert art.payload[33] == int(AiSignalType.ORACLE_ANCHOR)
    assert art.payload[34:66] == issuer
    assert art.payload[66:98] == data_hash
    assert art.payload[98:106] == ts.to_bytes(8, "big")
    assert art.payload[106:138] == source_hash
    assert art.payload[138:146] == (0).to_bytes(8, "big")
    assert art.payload[146] == len(tag_bytes)
    assert art.payload[147:] == tag_bytes

    # The signal_hash is the SDK's content-addressed derivation, not invented.
    from novai_sdk.signals.oracle import derive_oracle_anchor_signal_hash

    assert art.signal_hash == derive_oracle_anchor_signal_hash(
        issuer_entity_id=issuer,
        data_hash=data_hash,
        external_timestamp=ts,
        source_hash=source_hash,
        data_tag=tag,
    )
    # extras matches the SDK builder exactly.
    assert art.extras == build_oracle_anchor_extras(
        data_hash=data_hash,
        external_timestamp=ts,
        source_hash=source_hash,
        expiry_height=0,
        data_tag=tag,
    )


def test_build_reputation_update_is_101_bytes_with_correct_tail():
    issuer = bytes([0x07] * 32)
    target = bytes([0x09] * 32)
    art = sig.build_reputation_update(issuer, target, event_type=3, points_delta=-5)

    assert len(art.payload) == 101  # 66 base + 35 tail
    assert art.payload[0] == 2
    assert art.payload[1:33] == art.signal_hash
    assert art.payload[33] == int(AiSignalType.REPUTATION_UPDATE)
    assert art.payload[34:66] == issuer
    assert art.payload[66:98] == target
    assert art.payload[98] == 3
    assert art.payload[99:101] == (-5).to_bytes(2, "big", signed=True)
    assert len(art.signal_hash) == 32
