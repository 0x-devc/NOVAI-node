"""Memory object CRUD tx payload builders (tx types 3, 4, 5)."""

from __future__ import annotations

from novai_sdk._hex import coerce_hash32
from novai_sdk.constants import MAX_MEMORY_OBJECT_SIZE
from novai_sdk.enums import MemoryObjectType, TxPayloadType


def build_create_memory_payload(object_type: MemoryObjectType, data: bytes) -> bytes:
    """Build the tx-type-3 CreateMemoryObject payload.

    Layout: ``[0x03][object_type:1][data_len_be:4][data:N]``. Total length 6 + N.
    """
    if len(data) > MAX_MEMORY_OBJECT_SIZE:
        raise ValueError(
            f"data exceeds MAX_MEMORY_OBJECT_SIZE ({len(data)} > {MAX_MEMORY_OBJECT_SIZE})"
        )
    return (
        bytes([int(TxPayloadType.CREATE_MEMORY)])
        + bytes([int(object_type)])
        + len(data).to_bytes(4, "big")
        + data
    )


def build_update_memory_payload(object_id: bytes | str, new_data: bytes) -> bytes:
    """Build the tx-type-4 UpdateMemoryObject payload.

    Layout: ``[0x04][object_id:32][data_len_be:4][new_data:N]``. Total length 37 + N.
    """
    if len(new_data) > MAX_MEMORY_OBJECT_SIZE:
        raise ValueError(
            f"new_data exceeds MAX_MEMORY_OBJECT_SIZE ({len(new_data)} > {MAX_MEMORY_OBJECT_SIZE})"
        )
    oid = coerce_hash32(object_id, field="object_id")
    return (
        bytes([int(TxPayloadType.UPDATE_MEMORY)])
        + oid
        + len(new_data).to_bytes(4, "big")
        + new_data
    )


def build_delete_memory_payload(object_id: bytes | str) -> bytes:
    """Build the tx-type-5 DeleteMemoryObject payload (33 bytes fixed).

    Layout: ``[0x05][object_id:32]``.
    """
    oid = coerce_hash32(object_id, field="object_id")
    return bytes([int(TxPayloadType.DELETE_MEMORY)]) + oid
