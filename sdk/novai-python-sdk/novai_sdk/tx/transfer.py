"""Type 1: Transfer.

Payload layout (41 bytes fixed)::

    [version=0x01][to:32][amount_be:8]
"""

from __future__ import annotations

from novai_sdk._hex import coerce_address
from novai_sdk.enums import TxPayloadType


def build_transfer_payload(to: bytes | str, amount: int) -> bytes:
    """Build the binary payload for a Transfer tx (tx type 1).

    Args:
        to: 32-byte recipient address (raw bytes or hex string).
        amount: Transfer amount in base units. Must fit in u64 and be > 0.

    Returns:
        The 41-byte payload ready to wrap in a TxV1 envelope.
    """
    if not 0 < amount < 2**64:
        raise ValueError(f"amount must be in (0, 2^64), got {amount}")
    to_bytes = coerce_address(to, field="to")
    return bytes([int(TxPayloadType.TRANSFER)]) + to_bytes + amount.to_bytes(8, "big")
