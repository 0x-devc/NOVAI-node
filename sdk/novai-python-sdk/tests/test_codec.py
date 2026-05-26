"""Tests for novai_sdk.codec (TxV1 encoding)."""

from __future__ import annotations

import pytest

from novai_sdk import (
    TX_V1_OVERHEAD,
    TxV1,
    encode_tx_v1_signed,
    encode_tx_v1_unsigned,
    tx_encoded_size,
    txid_v1,
)
from novai_sdk.codec import decode_tx_v1_signed, decode_tx_v1_unsigned
from novai_sdk.errors import DecodeError


def _sample_tx(payload: bytes = b"") -> TxV1:
    return TxV1(
        from_address=bytes(range(32)),
        pubkey=bytes(range(32, 64)),
        nonce=42,
        fee=100,
        payload=payload,
        sig=bytes([0xCC] * 64),
    )


class TestTxV1Validation:
    def test_rejects_wrong_from_length(self) -> None:
        with pytest.raises(ValueError, match="from_address"):
            TxV1(
                from_address=b"\x00" * 31,
                pubkey=b"\x00" * 32,
                nonce=0,
                fee=0,
                payload=b"",
            )

    def test_rejects_wrong_pubkey_length(self) -> None:
        with pytest.raises(ValueError, match="pubkey"):
            TxV1(
                from_address=b"\x00" * 32,
                pubkey=b"\x00" * 33,
                nonce=0,
                fee=0,
                payload=b"",
            )

    def test_rejects_wrong_sig_length(self) -> None:
        with pytest.raises(ValueError, match="sig"):
            TxV1(
                from_address=b"\x00" * 32,
                pubkey=b"\x00" * 32,
                nonce=0,
                fee=0,
                payload=b"",
                sig=b"\x00" * 63,
            )

    def test_rejects_nonce_out_of_u64(self) -> None:
        with pytest.raises(ValueError, match="nonce"):
            TxV1(
                from_address=b"\x00" * 32,
                pubkey=b"\x00" * 32,
                nonce=2**64,
                fee=0,
                payload=b"",
            )

    def test_default_sig_is_zeros(self) -> None:
        tx = TxV1(
            from_address=b"\x00" * 32,
            pubkey=b"\x00" * 32,
            nonce=0,
            fee=0,
            payload=b"",
        )
        assert tx.sig == bytes(64)


class TestEncoding:
    def test_unsigned_layout(self) -> None:
        """Verify field order and endianness explicitly."""
        tx = _sample_tx(payload=b"hello")
        encoded = encode_tx_v1_unsigned(tx)

        # Layout: ver(1) + from(32) + pubkey(32) + nonce(8 LE) + fee(8 LE)
        #         + payload_len(4 LE) + payload(N)
        assert encoded[0] == 1  # version
        assert encoded[1:33] == tx.from_address
        assert encoded[33:65] == tx.pubkey
        assert encoded[65:73] == (42).to_bytes(8, "little")
        assert encoded[73:81] == (100).to_bytes(8, "little")
        assert encoded[81:85] == (5).to_bytes(4, "little")
        assert encoded[85:90] == b"hello"
        assert len(encoded) == 90

    def test_signed_appends_sig(self) -> None:
        tx = _sample_tx(payload=b"x")
        unsigned = encode_tx_v1_unsigned(tx)
        signed = encode_tx_v1_signed(tx)
        assert signed[: len(unsigned)] == unsigned
        assert signed[len(unsigned) :] == tx.sig

    def test_overhead_constant_matches_empty_payload_size(self) -> None:
        tx = _sample_tx(payload=b"")
        assert len(encode_tx_v1_signed(tx)) == TX_V1_OVERHEAD

    @pytest.mark.parametrize("payload_len", [0, 1, 5, 1024, 64 * 1024])
    def test_tx_encoded_size_matches_actual(self, payload_len: int) -> None:
        tx = _sample_tx(payload=bytes(payload_len))
        assert tx_encoded_size(tx) == len(encode_tx_v1_signed(tx))

    def test_payload_len_is_little_endian(self) -> None:
        """A 5-byte payload must encode the length as 05 00 00 00, not 00 00 00 05."""
        tx = _sample_tx(payload=b"hello")
        encoded = encode_tx_v1_unsigned(tx)
        assert encoded[81:85] == b"\x05\x00\x00\x00"


class TestRoundtrip:
    @pytest.mark.parametrize("payload", [b"", b"hello", bytes(range(256)) * 4])
    def test_unsigned_roundtrip(self, payload: bytes) -> None:
        tx = _sample_tx(payload=payload)
        encoded = encode_tx_v1_unsigned(tx)
        decoded = decode_tx_v1_unsigned(encoded)
        assert decoded.from_address == tx.from_address
        assert decoded.pubkey == tx.pubkey
        assert decoded.nonce == tx.nonce
        assert decoded.fee == tx.fee
        assert decoded.payload == tx.payload
        # Unsigned decode zero-fills the sig field by contract.
        assert decoded.sig == bytes(64)

    @pytest.mark.parametrize("payload", [b"", b"hello", bytes(range(256))])
    def test_signed_roundtrip(self, payload: bytes) -> None:
        tx = _sample_tx(payload=payload)
        encoded = encode_tx_v1_signed(tx)
        decoded = decode_tx_v1_signed(encoded)
        assert decoded.sig == tx.sig
        assert decoded.payload == tx.payload


class TestDecodeErrors:
    def test_truncated(self) -> None:
        with pytest.raises(DecodeError):
            decode_tx_v1_signed(b"\x01")

    def test_wrong_version(self) -> None:
        tx = _sample_tx()
        encoded = bytearray(encode_tx_v1_signed(tx))
        encoded[0] = 2
        with pytest.raises(DecodeError, match="unsupported tx version"):
            decode_tx_v1_signed(bytes(encoded))

    def test_trailing_bytes(self) -> None:
        tx = _sample_tx()
        encoded = encode_tx_v1_signed(tx) + b"\x00"
        with pytest.raises(DecodeError, match="trailing bytes"):
            decode_tx_v1_signed(encoded)


class TestTxId:
    def test_is_32_bytes(self) -> None:
        tx = _sample_tx(payload=b"hello")
        assert len(txid_v1(tx)) == 32

    def test_deterministic(self) -> None:
        tx = _sample_tx(payload=b"hello")
        assert txid_v1(tx) == txid_v1(tx)

    def test_changes_with_payload(self) -> None:
        a = _sample_tx(payload=b"a")
        b = _sample_tx(payload=b"b")
        assert txid_v1(a) != txid_v1(b)

    def test_invariant_to_sig(self) -> None:
        """txid is taken over UNSIGNED bytes; sig changes must not affect it."""
        tx1 = _sample_tx(payload=b"hello")
        tx2 = _sample_tx(payload=b"hello")
        tx2.sig = bytes(64)  # different sig
        assert txid_v1(tx1) == txid_v1(tx2)
