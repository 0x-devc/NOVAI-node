"""Determinism and sensitivity of the OracleAnchor data_hash builder."""

from __future__ import annotations

import pytest

from lib.signal import build_data_hash, canonical_observation_bytes


def test_data_hash_is_deterministic_for_same_inputs():
    h1 = build_data_hash(67234.51, 1717428000)
    h2 = build_data_hash(67234.51, 1717428000)
    assert h1 == h2
    assert len(h1) == 32


def test_data_hash_changes_with_price():
    h1 = build_data_hash(67234.51, 1717428000)
    h2 = build_data_hash(67234.52, 1717428000)
    assert h1 != h2


def test_data_hash_changes_with_timestamp():
    h1 = build_data_hash(67234.51, 1717428000)
    h2 = build_data_hash(67234.51, 1717428001)
    assert h1 != h2


def test_canonical_bytes_two_decimal_format():
    body = canonical_observation_bytes(67234.5, 1717428000)
    assert body == b"BTC-USD@1717428000=67234.50"


def test_canonical_bytes_truncates_extra_precision():
    body = canonical_observation_bytes(67234.567, 1717428000)
    assert body == b"BTC-USD@1717428000=67234.57"


def test_zero_price_rejected():
    with pytest.raises(ValueError):
        build_data_hash(0.0, 1717428000)


def test_negative_price_rejected():
    with pytest.raises(ValueError):
        build_data_hash(-1.0, 1717428000)


def test_non_finite_price_rejected():
    with pytest.raises(ValueError):
        build_data_hash(float("nan"), 1717428000)
    with pytest.raises(ValueError):
        build_data_hash(float("inf"), 1717428000)


def test_zero_timestamp_rejected():
    with pytest.raises(ValueError):
        build_data_hash(67234.51, 0)


def test_huge_timestamp_rejected():
    with pytest.raises(ValueError):
        build_data_hash(67234.51, 2**63)
