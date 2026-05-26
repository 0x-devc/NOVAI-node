"""PaymentChannel memory object (type 15, Week 32).

Fixed wire layout (222 bytes)::

    version:1
    party_a_entity_id:32
    party_b_entity_id:32
    sla_object_id:32             (zero = no SLA reference)
    status:1
    deposit_a_be:16
    deposit_b_be:16              (zero at create; party B funds at accept)
    balance_a_be:16              (== deposit_a at create)
    balance_b_be:16              (zero at create)
    nonce_be:8                   (0 at create)
    proposed_at_height_be:8      (0 at create; runtime fills)
    accepted_at_height_be:8      (0 until accept)
    closing_at_height_be:8       (0 until close)
    dispute_deadline_height_be:8 (0 until close)
    dispute_window_blocks_be:4
    reserved:16                  (MUST be zero on create/update)
"""

from __future__ import annotations

from novai_sdk._hex import coerce_address, coerce_hash32
from novai_sdk.enums import ChannelStatus

PAYMENT_CHANNEL_VERSION: int = 1
PAYMENT_CHANNEL_SIZE: int = 222
DISPUTE_WINDOW_MIN_BLOCKS: int = 100
DISPUTE_WINDOW_MAX_BLOCKS: int = 10_000


def encode_payment_channel(
    *,
    party_a_entity_id: bytes | str,
    party_b_entity_id: bytes | str,
    deposit_a: int,
    dispute_window_blocks: int,
    sla_object_id: bytes | str | None = None,
) -> bytes:
    """Encode the canonical 222-byte PaymentChannel data block for a Proposed channel.

    Args:
        party_a_entity_id: 32-byte entity ID of the channel proposer (memory
            object owner).
        party_b_entity_id: 32-byte entity ID of the counterparty.
        deposit_a: Party A's collateral, in base units (u128). Must be > 0.
        dispute_window_blocks: Number of blocks the chain waits for a higher-
            nonce dispute after a unilateral close. Range [100, 10_000].
        sla_object_id: Optional 32-byte memory object ID of an associated SLA.
            ``None`` (or all-zero bytes) means no SLA reference.
    """
    if not 0 < deposit_a < 2**128:
        raise ValueError("deposit_a must be in (0, 2^128)")
    if not DISPUTE_WINDOW_MIN_BLOCKS <= dispute_window_blocks <= DISPUTE_WINDOW_MAX_BLOCKS:
        raise ValueError(
            f"dispute_window_blocks must be in "
            f"[{DISPUTE_WINDOW_MIN_BLOCKS}, {DISPUTE_WINDOW_MAX_BLOCKS}], "
            f"got {dispute_window_blocks}"
        )
    pa = coerce_address(party_a_entity_id, field="party_a_entity_id")
    pb = coerce_address(party_b_entity_id, field="party_b_entity_id")
    if pa == pb:
        raise ValueError("party_a_entity_id and party_b_entity_id must differ")
    if sla_object_id is None:
        sla = bytes(32)
    else:
        sla = coerce_hash32(sla_object_id, field="sla_object_id")
    out = bytearray(PAYMENT_CHANNEL_SIZE)
    out[0] = PAYMENT_CHANNEL_VERSION
    out[1:33] = pa
    out[33:65] = pb
    out[65:97] = sla
    out[97] = int(ChannelStatus.PROPOSED)
    out[98:114] = deposit_a.to_bytes(16, "big")
    # out[114:130] deposit_b (0 until accept)
    out[130:146] = deposit_a.to_bytes(16, "big")  # balance_a == deposit_a at create
    # out[146:162] balance_b (0 until accept)
    # out[162:170] nonce (0)
    # out[170:178] proposed_at_height (0)
    # out[178:186] accepted_at_height (0)
    # out[186:194] closing_at_height (0)
    # out[194:202] dispute_deadline_height (0)
    out[202:206] = dispute_window_blocks.to_bytes(4, "big")
    # out[206:222] reserved (all-zero)
    return bytes(out)
