"""AI entity lifecycle tx payload builders.

Covers tx types 8 (RegisterAiEntity), 9 (CreditAiEntity), 10
(RegisterAiEntityWithKey), and 11 (EntityUpgrade, Week 34).
"""

from __future__ import annotations

from novai_sdk._hex import coerce_address, coerce_hash32
from novai_sdk.capabilities import Capabilities
from novai_sdk.enums import AutonomyMode, TxPayloadType


def build_register_entity_payload(
    code_hash: bytes | str,
    autonomy_mode: AutonomyMode,
    capabilities: Capabilities,
    initial_balance: int,
) -> bytes:
    """Build the tx-type-8 RegisterAiEntity payload (51 bytes fixed).

    Layout: ``[0x08][code_hash:32][autonomy:1][capabilities:1][initial_balance_be:16]``.
    """
    if not 0 <= initial_balance < 2**128:
        raise ValueError(f"initial_balance must fit in u128, got {initial_balance}")
    code = coerce_hash32(code_hash, field="code_hash")
    return (
        bytes([int(TxPayloadType.REGISTER_AI_ENTITY)])
        + code
        + bytes([int(autonomy_mode)])
        + bytes([capabilities.to_byte()])
        + initial_balance.to_bytes(16, "big")
    )


def build_credit_entity_payload(entity_id: bytes | str, amount: int) -> bytes:
    """Build the tx-type-9 CreditAiEntity payload (49 bytes fixed).

    Layout: ``[0x09][entity_id:32][amount_be:16]``.
    """
    if not 0 < amount < 2**128:
        raise ValueError(f"amount must be in (0, 2^128), got {amount}")
    eid = coerce_hash32(entity_id, field="entity_id")
    return bytes([int(TxPayloadType.CREDIT_AI_ENTITY)]) + eid + amount.to_bytes(16, "big")


def build_register_with_key_payload(
    code_hash: bytes | str,
    entity_pubkey: bytes | str,
    autonomy_mode: AutonomyMode,
    capabilities: Capabilities,
    initial_balance: int,
) -> bytes:
    """Build the tx-type-10 RegisterAiEntityWithKey payload (83 bytes fixed).

    Layout: ``[0x0A][code_hash:32][pubkey:32][autonomy:1][capabilities:1][balance_be:16]``.
    """
    if not 0 <= initial_balance < 2**128:
        raise ValueError(f"initial_balance must fit in u128, got {initial_balance}")
    code = coerce_hash32(code_hash, field="code_hash")
    pk = coerce_address(entity_pubkey, field="entity_pubkey")
    return (
        bytes([int(TxPayloadType.REGISTER_AI_ENTITY_WITH_KEY)])
        + code
        + pk
        + bytes([int(autonomy_mode)])
        + bytes([capabilities.to_byte()])
        + initial_balance.to_bytes(16, "big")
    )


def build_entity_upgrade_payload(
    entity_id: bytes | str,
    new_code_hash: bytes | str,
    reason_hash: bytes | str | None = None,
) -> bytes:
    """Build the tx-type-11 EntityUpgrade payload (97 bytes fixed, Week 34).

    Layout: ``[0x0B][entity_id:32][new_code_hash:32][reason_hash:32]``.

    ``reason_hash`` may be omitted (defaults to all-zero) when no off-chain
    reason commitment is being attached. The chain enforces a per-entity
    cooldown of ``MIN_UPGRADE_INTERVAL_BLOCKS = 1000`` and rejects upgrades
    where ``new_code_hash`` equals the entity's current code hash.
    """
    eid = coerce_hash32(entity_id, field="entity_id")
    new_code = coerce_hash32(new_code_hash, field="new_code_hash")
    reason = coerce_hash32(reason_hash, field="reason_hash") if reason_hash else bytes(32)
    return bytes([int(TxPayloadType.ENTITY_UPGRADE)]) + eid + new_code + reason
