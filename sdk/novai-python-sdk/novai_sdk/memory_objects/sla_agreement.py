"""SlaAgreement memory object (type 14, Week 31).

Fixed wire layout (210 bytes)::

    version:1
    buyer_entity_id:32
    seller_entity_id:32
    service_descriptor_hash:32
    status:1
    created_at_height_be:8       (0 at create; runtime fills it in)
    accepted_at_height_be:8      (0 until SlaAccept)
    start_height_be:8
    end_height_be:8
    violation_count_be:4         (0 at create; runtime increments)
    violation_threshold_be:4
    max_response_time_blocks_be:4 (reserved v1)
    min_uptime_bps_be:2           (reserved v1)
    min_delivery_success_bps_be:2 (reserved v1)
    price_per_call_be:8
    slash_amount_be:16
    terminated_at_height_be:8    (0 until terminated)
    slashed_amount_be:16         (0 until breach)
    reserved:16                  (MUST be zero on create/update)
"""

from __future__ import annotations

from novai_sdk._hex import coerce_address, coerce_hash32
from novai_sdk.enums import SlaStatus

SLA_AGREEMENT_VERSION: int = 1
SLA_AGREEMENT_SIZE: int = 210


def encode_sla_agreement(
    *,
    buyer_entity_id: bytes | str,
    seller_entity_id: bytes | str,
    service_descriptor_hash: bytes | str,
    start_height: int,
    end_height: int,
    violation_threshold: int,
    slash_amount: int,
    price_per_call: int,
    max_response_time_blocks: int = 0,
    min_uptime_bps: int = 0,
    min_delivery_success_bps: int = 0,
) -> bytes:
    """Encode the canonical 210-byte SlaAgreement data block for a Proposed SLA.

    At create-time the runtime fills in ``created_at_height``, leaves the
    accepted/violated/terminated fields at zero, and freezes the rest. This
    builder sets all status to PROPOSED and the runtime-managed counters to
    zero, matching the CLI's ``sla propose`` behavior.
    """
    if not start_height < end_height:
        raise ValueError(f"start_height ({start_height}) must be < end_height ({end_height})")
    if not 0 <= start_height < 2**64 or not 0 <= end_height < 2**64:
        raise ValueError("start_height / end_height must fit in u64")
    if violation_threshold < 1 or violation_threshold >= 2**32:
        raise ValueError("violation_threshold must be in [1, 2^32)")
    if not 0 < slash_amount < 2**128:
        raise ValueError("slash_amount must be in (0, 2^128)")
    if not 0 <= price_per_call < 2**64:
        raise ValueError("price_per_call must fit in u64")
    if not 0 <= max_response_time_blocks < 2**32:
        raise ValueError("max_response_time_blocks must fit in u32")
    if not 0 <= min_uptime_bps <= 10_000:
        raise ValueError("min_uptime_bps must be in [0, 10000]")
    if not 0 <= min_delivery_success_bps <= 10_000:
        raise ValueError("min_delivery_success_bps must be in [0, 10000]")
    buyer = coerce_address(buyer_entity_id, field="buyer_entity_id")
    seller = coerce_address(seller_entity_id, field="seller_entity_id")
    if buyer == seller:
        raise ValueError("buyer_entity_id and seller_entity_id must differ")
    sd_hash = coerce_hash32(service_descriptor_hash, field="service_descriptor_hash")
    out = bytearray(SLA_AGREEMENT_SIZE)
    out[0] = SLA_AGREEMENT_VERSION
    out[1:33] = buyer
    out[33:65] = seller
    out[65:97] = sd_hash
    out[97] = int(SlaStatus.PROPOSED)
    # out[98:106] created_at_height (0)
    # out[106:114] accepted_at_height (0)
    out[114:122] = start_height.to_bytes(8, "big")
    out[122:130] = end_height.to_bytes(8, "big")
    # out[130:134] violation_count (0)
    out[134:138] = violation_threshold.to_bytes(4, "big")
    out[138:142] = max_response_time_blocks.to_bytes(4, "big")
    out[142:144] = min_uptime_bps.to_bytes(2, "big")
    out[144:146] = min_delivery_success_bps.to_bytes(2, "big")
    out[146:154] = price_per_call.to_bytes(8, "big")
    out[154:170] = slash_amount.to_bytes(16, "big")
    # out[170:178] terminated_at_height (0)
    # out[178:194] slashed_amount (0)
    # out[194:210] reserved (all-zero)
    return bytes(out)
