"""Signal type 12: CompositionCheck.

Extras layout (34 bytes)::

    [target_entity_id:32][failed_dependency_idx:1][failure_reason:1]
"""

from __future__ import annotations

from novai_sdk._hex import coerce_address


def build_composition_check_extras(
    target_entity_id: bytes | str,
    failed_dependency_idx: int,
    failure_reason: int,
) -> bytes:
    """Build the CompositionCheck extras tail (34 bytes)."""
    if not 0 <= failed_dependency_idx <= 0xFF:
        raise ValueError(f"failed_dependency_idx must fit in u8, got {failed_dependency_idx}")
    if not 0 <= failure_reason <= 0xFF:
        raise ValueError(f"failure_reason must fit in u8, got {failure_reason}")
    target = coerce_address(target_entity_id, field="target_entity_id")
    return target + bytes([failed_dependency_idx, failure_reason])
