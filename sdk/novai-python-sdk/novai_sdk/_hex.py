"""Internal hex helpers.

The NOVAI RPC convention is lowercase hex without the ``0x`` prefix and with a
strict length match for fixed-size fields. These helpers normalize input and
fail fast on bad data so encoding bugs surface client-side rather than as
opaque chain rejections.
"""

from __future__ import annotations


def hex_to_bytes(value: str, *, expected_len: int | None = None, field: str = "value") -> bytes:
    """Decode hex into bytes, tolerating an optional ``0x`` prefix.

    Args:
        value: Hex string (case-insensitive, with or without ``0x``).
        expected_len: If set, require the decoded bytes to have this exact length.
        field: Human-readable field name used in error messages.

    Raises:
        ValueError: If ``value`` is not valid hex, or if ``expected_len`` is set
            and the decoded length does not match.
    """
    if not isinstance(value, str):
        raise ValueError(f"{field} must be a hex string, got {type(value).__name__}")
    stripped = value[2:] if value.startswith(("0x", "0X")) else value
    try:
        out = bytes.fromhex(stripped)
    except ValueError as exc:
        raise ValueError(f"{field} is not valid hex: {exc}") from exc
    if expected_len is not None and len(out) != expected_len:
        raise ValueError(
            f"{field} must be exactly {expected_len} bytes "
            f"({expected_len * 2} hex chars), got {len(out)} bytes"
        )
    return out


def bytes_to_hex(value: bytes) -> str:
    """Encode bytes as lowercase hex with no ``0x`` prefix.

    Matches the wire convention used by NOVAI RPC params and responses.
    """
    return value.hex()


def coerce_address(value: bytes | str, *, field: str = "address") -> bytes:
    """Coerce ``value`` to a 32-byte address. Accepts raw bytes or hex string."""
    if isinstance(value, bytes):
        if len(value) != 32:
            raise ValueError(f"{field} must be exactly 32 bytes, got {len(value)}")
        return value
    return hex_to_bytes(value, expected_len=32, field=field)


def coerce_hash32(value: bytes | str, *, field: str = "hash") -> bytes:
    """Coerce ``value`` to a 32-byte hash. Accepts raw bytes or hex string."""
    return coerce_address(value, field=field)


def coerce_signature(value: bytes | str, *, field: str = "signature") -> bytes:
    """Coerce ``value`` to a 64-byte signature. Accepts raw bytes or hex string."""
    if isinstance(value, bytes):
        if len(value) != 64:
            raise ValueError(f"{field} must be exactly 64 bytes, got {len(value)}")
        return value
    return hex_to_bytes(value, expected_len=64, field=field)
