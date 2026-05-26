"""Tests for novai_sdk._hex."""

from __future__ import annotations

import pytest

from novai_sdk._hex import (
    bytes_to_hex,
    coerce_address,
    coerce_hash32,
    coerce_signature,
    hex_to_bytes,
)


class TestHexToBytes:
    def test_decodes_lowercase_no_prefix(self) -> None:
        assert hex_to_bytes("ab", expected_len=1) == b"\xab"

    def test_decodes_uppercase(self) -> None:
        assert hex_to_bytes("AB", expected_len=1) == b"\xab"

    def test_strips_0x_prefix(self) -> None:
        assert hex_to_bytes("0xab", expected_len=1) == b"\xab"
        assert hex_to_bytes("0Xab", expected_len=1) == b"\xab"

    def test_rejects_non_string(self) -> None:
        with pytest.raises(ValueError, match="must be a hex string"):
            hex_to_bytes(b"ab")  # type: ignore[arg-type]

    def test_rejects_invalid_chars(self) -> None:
        with pytest.raises(ValueError, match="not valid hex"):
            hex_to_bytes("zz", expected_len=1)

    def test_rejects_wrong_length(self) -> None:
        with pytest.raises(ValueError, match="exactly 32 bytes"):
            hex_to_bytes("ab", expected_len=32)


class TestBytesToHex:
    def test_lowercase_no_prefix(self) -> None:
        assert bytes_to_hex(b"\xab\xcd") == "abcd"

    def test_empty(self) -> None:
        assert bytes_to_hex(b"") == ""


class TestCoerceAddress:
    def test_accepts_bytes(self) -> None:
        addr = bytes(range(32))
        assert coerce_address(addr) == addr

    def test_accepts_hex(self) -> None:
        addr = bytes(range(32))
        assert coerce_address(addr.hex()) == addr

    def test_accepts_hex_with_0x(self) -> None:
        addr = bytes(range(32))
        assert coerce_address("0x" + addr.hex()) == addr

    def test_rejects_wrong_length_bytes(self) -> None:
        with pytest.raises(ValueError):
            coerce_address(b"\x00" * 31)

    def test_rejects_wrong_length_hex(self) -> None:
        with pytest.raises(ValueError):
            coerce_address("ab")


class TestCoerceHash32:
    def test_alias_of_address(self) -> None:
        h = bytes(range(32))
        assert coerce_hash32(h) == coerce_address(h)


class TestCoerceSignature:
    def test_accepts_64_bytes(self) -> None:
        sig = bytes(range(64))
        assert coerce_signature(sig) == sig

    def test_accepts_hex(self) -> None:
        sig = bytes(range(64))
        assert coerce_signature(sig.hex()) == sig

    def test_rejects_wrong_length(self) -> None:
        with pytest.raises(ValueError):
            coerce_signature(b"\x00" * 32)
