"""Tests for novai_sdk.signals.oracle (Week 35 OracleAnchor)."""

from __future__ import annotations

import blake3
import pytest

from novai_sdk.signals.oracle import (
    build_oracle_anchor_extras,
    derive_oracle_anchor_signal_hash,
)


class TestBuildOracleAnchorExtras:
    def test_length_minimum(self) -> None:
        """Minimum tail length is 82 (81-byte fixed + 1-byte tag)."""
        extras = build_oracle_anchor_extras(
            data_hash=bytes([0xAB] * 32),
            external_timestamp=0x0102_0304_0506_0708,
            source_hash=bytes([0xCD] * 32),
            expiry_height=5000,
            data_tag=b"x",
        )
        assert len(extras) == 82

    def test_length_maximum(self) -> None:
        """Maximum tail length is 113 (81 + 32-byte tag)."""
        extras = build_oracle_anchor_extras(
            data_hash=bytes([0xAB] * 32),
            external_timestamp=1,
            source_hash=None,
            expiry_height=0,
            data_tag=b"x" * 32,
        )
        assert len(extras) == 113

    def test_layout(self) -> None:
        """Verify the exact byte layout against the Rust CLI's anchor payload test."""
        extras = build_oracle_anchor_extras(
            data_hash=bytes([0xAB] * 32),
            external_timestamp=0x0102_0304_0506_0708,
            source_hash=bytes([0xCD] * 32),
            expiry_height=5000,
            data_tag=b"price/ETH-USD",
        )
        # 32 (data_hash) + 8 (ts) + 32 (src) + 8 (expiry) + 1 (tag_len) + 13 (tag) = 94
        assert len(extras) == 94
        assert extras[0:32] == bytes([0xAB] * 32)
        assert extras[32:40] == (0x0102_0304_0506_0708).to_bytes(8, "big")
        assert extras[40:72] == bytes([0xCD] * 32)
        assert extras[72:80] == (5000).to_bytes(8, "big")
        assert extras[80] == 13
        assert extras[81:94] == b"price/ETH-USD"

    def test_source_hash_none_encodes_zero(self) -> None:
        extras = build_oracle_anchor_extras(
            data_hash=bytes([0xAB] * 32),
            external_timestamp=1,
            source_hash=None,
            expiry_height=0,
            data_tag=b"x",
        )
        # source_hash field sits at offset 40..72
        assert extras[40:72] == bytes(32)

    def test_str_tag_utf8_encoded(self) -> None:
        extras = build_oracle_anchor_extras(
            data_hash=bytes([1] * 32),
            external_timestamp=1,
            source_hash=None,
            expiry_height=0,
            data_tag="price/BTC",
        )
        assert extras[80] == 9
        assert extras[81:90] == b"price/BTC"

    def test_rejects_zero_data_hash(self) -> None:
        with pytest.raises(ValueError, match="data_hash must be non-zero"):
            build_oracle_anchor_extras(
                data_hash=bytes(32),
                external_timestamp=1,
                source_hash=None,
                expiry_height=0,
                data_tag=b"x",
            )

    def test_rejects_zero_timestamp(self) -> None:
        with pytest.raises(ValueError, match="external_timestamp"):
            build_oracle_anchor_extras(
                data_hash=bytes([1] * 32),
                external_timestamp=0,
                source_hash=None,
                expiry_height=0,
                data_tag=b"x",
            )

    def test_rejects_empty_tag(self) -> None:
        with pytest.raises(ValueError):
            build_oracle_anchor_extras(
                data_hash=bytes([1] * 32),
                external_timestamp=1,
                source_hash=None,
                expiry_height=0,
                data_tag=b"",
            )

    def test_rejects_oversized_tag(self) -> None:
        with pytest.raises(ValueError):
            build_oracle_anchor_extras(
                data_hash=bytes([1] * 32),
                external_timestamp=1,
                source_hash=None,
                expiry_height=0,
                data_tag=b"x" * 33,
            )


class TestDeriveOracleAnchorSignalHash:
    def test_deterministic(self) -> None:
        kwargs = {
            "issuer_entity_id": bytes([1] * 32),
            "data_hash": bytes([2] * 32),
            "external_timestamp": 100,
            "source_hash": bytes([3] * 32),
            "data_tag": b"price/ETH-USD",
        }
        assert derive_oracle_anchor_signal_hash(**kwargs) == derive_oracle_anchor_signal_hash(
            **kwargs
        )

    def test_changes_with_each_input(self) -> None:
        base = derive_oracle_anchor_signal_hash(
            bytes([1] * 32), bytes([2] * 32), 100, bytes([3] * 32), b"price/ETH-USD"
        )
        assert base != derive_oracle_anchor_signal_hash(
            bytes([9] * 32), bytes([2] * 32), 100, bytes([3] * 32), b"price/ETH-USD"
        )
        assert base != derive_oracle_anchor_signal_hash(
            bytes([1] * 32), bytes([9] * 32), 100, bytes([3] * 32), b"price/ETH-USD"
        )
        assert base != derive_oracle_anchor_signal_hash(
            bytes([1] * 32), bytes([2] * 32), 101, bytes([3] * 32), b"price/ETH-USD"
        )
        assert base != derive_oracle_anchor_signal_hash(
            bytes([1] * 32), bytes([2] * 32), 100, bytes([3] * 32), b"price/BTC-USD"
        )

    def test_matches_rust_cli_domain(self) -> None:
        """Verify the domain string and tag-length encoding exactly match the Rust CLI."""
        issuer = bytes([1] * 32)
        data_hash = bytes([2] * 32)
        ts = 100
        source = bytes([3] * 32)
        tag = b"price/ETH-USD"
        # Mirror the Rust CLI's derive_signal_hash() byte-for-byte.
        hasher = blake3.blake3()
        hasher.update(b"novai-oracle-anchor-v1")
        hasher.update(issuer)
        hasher.update(data_hash)
        hasher.update(ts.to_bytes(8, "big"))
        hasher.update(source)
        hasher.update(len(tag).to_bytes(4, "big"))
        hasher.update(tag)
        expected = hasher.digest()
        assert derive_oracle_anchor_signal_hash(issuer, data_hash, ts, source, tag) == expected

    def test_none_source_hash_is_all_zero_in_derivation(self) -> None:
        h_none = derive_oracle_anchor_signal_hash(
            bytes([1] * 32), bytes([2] * 32), 100, None, b"x"
        )
        h_zero = derive_oracle_anchor_signal_hash(
            bytes([1] * 32), bytes([2] * 32), 100, bytes(32), b"x"
        )
        assert h_none == h_zero
