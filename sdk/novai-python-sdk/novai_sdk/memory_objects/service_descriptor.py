"""ServiceDescriptor memory object (type 12, Week 29).

Fixed wire layout (144 bytes)::

    version:1
    service_name_hash:32
    service_url_hash:32
    description_hash:32
    category:1
    price_per_call_be:8
    subscription_rate_per_block_be:8
    min_reputation_score_be:2
    min_stake_be:16
    capability_tags_be:4
    status:1
    reserved:7         (MUST be all zero on create/update)
"""

from __future__ import annotations

from novai_sdk._hex import coerce_hash32
from novai_sdk.enums import ServiceCategory, ServiceDescriptorStatus

SERVICE_DESCRIPTOR_VERSION: int = 1
SERVICE_DESCRIPTOR_SIZE: int = 144


def encode_service_descriptor(
    *,
    service_name_hash: bytes | str,
    service_url_hash: bytes | str,
    description_hash: bytes | str,
    category: ServiceCategory | int,
    price_per_call: int,
    subscription_rate_per_block: int,
    min_reputation_score: int,
    min_stake: int,
    capability_tags: int,
    status: ServiceDescriptorStatus | int = ServiceDescriptorStatus.ACTIVE,
) -> bytes:
    """Encode a ServiceDescriptor data block (144 bytes).

    The result is the inner ``data`` payload for a CreateMemoryObject /
    UpdateMemoryObject tx of object_type ``SERVICE_DESCRIPTOR (12)``.
    """
    if not 0 <= price_per_call < 2**64:
        raise ValueError("price_per_call must fit in u64")
    if not 0 <= subscription_rate_per_block < 2**64:
        raise ValueError("subscription_rate_per_block must fit in u64")
    if not 0 <= min_reputation_score <= 100:
        raise ValueError("min_reputation_score must be in [0, 100]")
    if not 0 <= min_stake < 2**128:
        raise ValueError("min_stake must fit in u128")
    if not 0 <= capability_tags < 2**32:
        raise ValueError("capability_tags must fit in u32")
    cat = int(category)
    if not 0 <= cat <= 0xFF:
        raise ValueError("category must fit in u8")
    st = int(status)
    if not 0 <= st <= 0xFF:
        raise ValueError("status must fit in u8")
    name = coerce_hash32(service_name_hash, field="service_name_hash")
    url = coerce_hash32(service_url_hash, field="service_url_hash")
    desc = coerce_hash32(description_hash, field="description_hash")
    out = bytearray(SERVICE_DESCRIPTOR_SIZE)
    out[0] = SERVICE_DESCRIPTOR_VERSION
    out[1:33] = name
    out[33:65] = url
    out[65:97] = desc
    out[97] = cat
    out[98:106] = price_per_call.to_bytes(8, "big")
    out[106:114] = subscription_rate_per_block.to_bytes(8, "big")
    out[114:116] = min_reputation_score.to_bytes(2, "big")
    out[116:132] = min_stake.to_bytes(16, "big")
    out[132:136] = capability_tags.to_bytes(4, "big")
    out[136] = st
    # out[137:144] stays zero (reserved bytes).
    return bytes(out)
