"""Tests for novai_sdk.capabilities."""

from __future__ import annotations

import pytest

from novai_sdk import Capabilities


class TestEncoding:
    def test_empty_encodes_to_zero(self) -> None:
        assert Capabilities().to_byte() == 0

    def test_all_set_encodes_to_seven_bits(self) -> None:
        caps = Capabilities(
            read_public_chain=True,
            read_memory_objects=True,
            emit_proposals=True,
            request_execution=True,
            read_nnpx_derived=True,
            submit_reputation_updates=True,
            post_oracle_anchors=True,
        )
        assert caps.to_byte() == 0b0111_1111

    @pytest.mark.parametrize(
        "field,bit",
        [
            ("read_public_chain", 0),
            ("read_memory_objects", 1),
            ("emit_proposals", 2),
            ("request_execution", 3),
            ("read_nnpx_derived", 4),
            ("submit_reputation_updates", 5),
            ("post_oracle_anchors", 6),
        ],
    )
    def test_single_bit_layout(self, field: str, bit: int) -> None:
        """Each flag must occupy the bit position pinned in Rust."""
        caps = Capabilities(**{field: True})
        assert caps.to_byte() == (1 << bit)


class TestRoundtrip:
    @pytest.mark.parametrize("byte_value", [0, 1, 0b0111_1111, 0b0100_0010])
    def test_roundtrip(self, byte_value: int) -> None:
        decoded = Capabilities.from_byte(byte_value)
        # Re-encoding preserves the byte (modulo the reserved high bit).
        assert decoded.to_byte() == (byte_value & 0b0111_1111)


class TestConstructors:
    def test_read_only(self) -> None:
        caps = Capabilities.read_only()
        assert caps.read_public_chain is True
        assert caps.read_memory_objects is True
        assert caps.emit_proposals is False
        assert caps.request_execution is False

    def test_advisory(self) -> None:
        caps = Capabilities.advisory()
        assert caps.emit_proposals is True
        assert caps.request_execution is False

    def test_gated(self) -> None:
        caps = Capabilities.gated()
        assert caps.emit_proposals is True
        assert caps.request_execution is True
        assert caps.post_oracle_anchors is False

    def test_oracle(self) -> None:
        caps = Capabilities.oracle()
        assert caps.post_oracle_anchors is True
        assert caps.emit_proposals is True
        assert caps.request_execution is False


class TestUnion:
    def test_or_combines_disjoint_sets(self) -> None:
        a = Capabilities(read_public_chain=True)
        b = Capabilities(post_oracle_anchors=True)
        merged = a | b
        assert merged.read_public_chain is True
        assert merged.post_oracle_anchors is True

    def test_or_is_idempotent(self) -> None:
        caps = Capabilities.gated()
        assert (caps | caps).to_byte() == caps.to_byte()
