"""Signal type 7: ReputationUpdate.

Extras layout (35 bytes)::

    [target_entity_id:32][event_type:1][points_delta_be:2 i16]
"""

from __future__ import annotations

from novai_sdk._hex import coerce_address


def build_reputation_update_extras(
    target_entity_id: bytes | str,
    event_type: int,
    points_delta: int,
) -> bytes:
    """Build the ReputationUpdate extras tail (35 bytes).

    Args:
        target_entity_id: 32-byte entity ID whose reputation is being mutated.
        event_type: u8 discriminant matching ``REP_EVENT_*`` in
            ``crates/execution``.
        points_delta: Signed reputation points delta (i16, range
            ``[-32768, 32767]``).
    """
    if not 0 <= event_type <= 0xFF:
        raise ValueError(f"event_type must fit in u8, got {event_type}")
    if not -(2**15) <= points_delta < 2**15:
        raise ValueError(f"points_delta must fit in i16, got {points_delta}")
    target = coerce_address(target_entity_id, field="target_entity_id")
    return target + bytes([event_type]) + points_delta.to_bytes(2, "big", signed=True)
