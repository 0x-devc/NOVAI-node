"""Canonical TxV1 envelope codec.

Matches ``crates/codec/src/lib.rs`` byte-for-byte. The field order, length
prefixes, and endianness are all consensus-critical: changing them is a hard
fork. Two facts to keep top of mind:

* The envelope uses **little-endian** for ``nonce``, ``fee``, and
  ``payload_len``. Payload-internal numeric fields generally use big-endian,
  but that is the responsibility of each payload builder, not this module.
* The encoded TxV1 fixed overhead is exactly 149 bytes (1 + 32 + 32 + 8 + 8 +
  4 + 64). Add the payload length to get the full size.
"""

from __future__ import annotations

from dataclasses import dataclass, field

import blake3

from novai_sdk.constants import (
    MAX_TX_SIZE,
    TX_V1_VERSION,
)
from novai_sdk.constants import (
    TX_V1_OVERHEAD as TX_V1_OVERHEAD,
)
from novai_sdk.enums import TxVersion
from novai_sdk.errors import DecodeError, EncodingError


@dataclass
class TxV1:
    """Canonical NOVAI transaction.

    Mirrors ``novai_types::TxV1`` field-for-field. The default ``sig`` is the
    64-byte zero vector; callers fill it in via :func:`sign_tx_v1`.
    """

    from_address: bytes
    pubkey: bytes
    nonce: int
    fee: int
    payload: bytes
    sig: bytes = field(default_factory=lambda: bytes(64))
    version: TxVersion = TxVersion.V1

    def __post_init__(self) -> None:
        if len(self.from_address) != 32:
            raise ValueError(f"from_address must be 32 bytes, got {len(self.from_address)}")
        if len(self.pubkey) != 32:
            raise ValueError(f"pubkey must be 32 bytes, got {len(self.pubkey)}")
        if len(self.sig) != 64:
            raise ValueError(f"sig must be 64 bytes, got {len(self.sig)}")
        if not 0 <= self.nonce < 2**64:
            raise ValueError("nonce must fit in u64")
        if not 0 <= self.fee < 2**64:
            raise ValueError("fee must fit in u64")
        if self.version != TxVersion.V1:
            raise ValueError(f"version must be V1, got {self.version}")


def encode_tx_v1_unsigned(tx: TxV1) -> bytes:
    """Encode a TxV1 without its signature (the signing scope).

    Layout (little-endian for multi-byte ints)::

        version(1) || from(32) || pubkey(32) || nonce(8 LE) || fee(8 LE)
        || payload_len(4 LE) || payload(N)
    """
    if not 0 <= len(tx.payload) < 2**32:
        raise EncodingError(f"payload length {len(tx.payload)} does not fit in u32")
    out = bytearray()
    out.append(int(tx.version))
    out.extend(tx.from_address)
    out.extend(tx.pubkey)
    out.extend(tx.nonce.to_bytes(8, "little"))
    out.extend(tx.fee.to_bytes(8, "little"))
    out.extend(len(tx.payload).to_bytes(4, "little"))
    out.extend(tx.payload)
    return bytes(out)


def encode_tx_v1_signed(tx: TxV1) -> bytes:
    """Encode a TxV1 including its 64-byte signature."""
    return encode_tx_v1_unsigned(tx) + tx.sig


def tx_encoded_size(tx: TxV1) -> int:
    """Return the encoded length of a signed TxV1 without allocating."""
    return TX_V1_OVERHEAD + len(tx.payload)


def txid_v1(tx: TxV1) -> bytes:
    """Compute the canonical 32-byte transaction ID.

    ``txid := blake3(encode_tx_v1_unsigned(tx))``. There is **no** domain tag
    on the txid; it is the plain blake3 of the unsigned encoding.
    """
    return blake3.blake3(encode_tx_v1_unsigned(tx)).digest()


def decode_tx_v1_unsigned(data: bytes) -> TxV1:
    """Decode an unsigned TxV1 wire-form. Used in tests against golden vectors."""
    return _decode(data, expect_sig=False)


def decode_tx_v1_signed(data: bytes) -> TxV1:
    """Decode a signed TxV1 wire-form."""
    return _decode(data, expect_sig=True)


def _decode(data: bytes, *, expect_sig: bool) -> TxV1:
    if len(data) < 85:
        raise DecodeError(f"tx encoding too short ({len(data)} bytes)")
    if data[0] != TX_V1_VERSION:
        raise DecodeError(f"unsupported tx version byte {data[0]}")
    if len(data) > MAX_TX_SIZE:
        raise DecodeError(f"tx encoding exceeds MAX_TX_SIZE ({len(data)} > {MAX_TX_SIZE})")
    cursor = 1
    from_address = data[cursor : cursor + 32]
    cursor += 32
    pubkey = data[cursor : cursor + 32]
    cursor += 32
    nonce = int.from_bytes(data[cursor : cursor + 8], "little")
    cursor += 8
    fee = int.from_bytes(data[cursor : cursor + 8], "little")
    cursor += 8
    payload_len = int.from_bytes(data[cursor : cursor + 4], "little")
    cursor += 4
    if cursor + payload_len > len(data):
        raise DecodeError("payload_len exceeds buffer")
    payload = data[cursor : cursor + payload_len]
    cursor += payload_len
    if expect_sig:
        if cursor + 64 != len(data):
            raise DecodeError(f"unexpected trailing bytes after signature ({len(data) - cursor})")
        sig = data[cursor : cursor + 64]
    else:
        if cursor != len(data):
            raise DecodeError(f"unexpected trailing bytes ({len(data) - cursor})")
        sig = bytes(64)
    return TxV1(
        from_address=bytes(from_address),
        pubkey=bytes(pubkey),
        nonce=nonce,
        fee=fee,
        payload=bytes(payload),
        sig=bytes(sig),
    )
