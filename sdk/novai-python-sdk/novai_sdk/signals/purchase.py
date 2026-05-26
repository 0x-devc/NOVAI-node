"""Signal type 8: SignalPurchase.

Extras layout (41 bytes)::

    [seller_entity_id:32][purchased_signal_type:1][max_price_be:8]
"""

from __future__ import annotations

from novai_sdk._hex import coerce_address
from novai_sdk.enums import AiSignalType


def build_signal_purchase_extras(
    seller_entity_id: bytes | str,
    purchased_signal_type: AiSignalType | int,
    max_price: int,
) -> bytes:
    """Build the SignalPurchase extras tail (41 bytes)."""
    if not 0 <= max_price < 2**64:
        raise ValueError(f"max_price must fit in u64, got {max_price}")
    sig_type_byte = int(purchased_signal_type)
    if not 0 <= sig_type_byte <= 0xFF:
        raise ValueError(f"purchased_signal_type must fit in u8, got {sig_type_byte}")
    seller = coerce_address(seller_entity_id, field="seller_entity_id")
    return seller + bytes([sig_type_byte]) + max_price.to_bytes(8, "big")
