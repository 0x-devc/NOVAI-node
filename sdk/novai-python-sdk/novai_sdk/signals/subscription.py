"""Signal types 14, 15: SubscriptionCreate / SubscriptionCancel."""

from __future__ import annotations

from novai_sdk._hex import coerce_address, coerce_hash32
from novai_sdk.constants import MIN_SUBSCRIPTION_DURATION
from novai_sdk.enums import AiSignalType


def build_subscription_create_extras(
    producer_entity_id: bytes | str,
    covered_signal_type: AiSignalType | int,
    rate_per_block: int,
    duration_blocks: int,
) -> bytes:
    """Build the SubscriptionCreate extras tail (49 bytes).

    Layout: ``[producer:32][covered_type:1][rate_be:8][duration_be:8]``.
    """
    if not 0 <= rate_per_block < 2**64:
        raise ValueError(f"rate_per_block must fit in u64, got {rate_per_block}")
    if duration_blocks < MIN_SUBSCRIPTION_DURATION:
        raise ValueError(
            f"duration_blocks must be >= MIN_SUBSCRIPTION_DURATION "
            f"({MIN_SUBSCRIPTION_DURATION}), got {duration_blocks}"
        )
    if not 0 <= duration_blocks < 2**64:
        raise ValueError(f"duration_blocks must fit in u64, got {duration_blocks}")
    covered = int(covered_signal_type)
    if not 0 <= covered <= 0xFF:
        raise ValueError(f"covered_signal_type must fit in u8, got {covered}")
    producer = coerce_address(producer_entity_id, field="producer_entity_id")
    return (
        producer
        + bytes([covered])
        + rate_per_block.to_bytes(8, "big")
        + duration_blocks.to_bytes(8, "big")
    )


def build_subscription_cancel_extras(subscription_id: bytes | str) -> bytes:
    """Build the SubscriptionCancel extras tail (32 bytes).

    Layout: ``[subscription_id:32]``. The id is the memory object ID of the
    Subscription being cancelled.
    """
    return coerce_hash32(subscription_id, field="subscription_id")
