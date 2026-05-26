"""Signal types 9, 10, 11: StakeDeposit / StakeWithdraw / StakeSlash."""

from __future__ import annotations

from novai_sdk._hex import coerce_address


def build_stake_deposit_extras(amount: int) -> bytes:
    """Build the StakeDeposit extras (16 bytes, u128 BE)."""
    if not 0 <= amount < 2**128:
        raise ValueError(f"amount must fit in u128, got {amount}")
    return amount.to_bytes(16, "big")


def build_stake_withdraw_extras(amount: int) -> bytes:
    """Build the StakeWithdraw extras (16 bytes, u128 BE)."""
    if not 0 <= amount < 2**128:
        raise ValueError(f"amount must fit in u128, got {amount}")
    return amount.to_bytes(16, "big")


def build_stake_slash_extras(
    target_entity_id: bytes | str,
    slash_amount: int,
    rep_event_type: int,
    points_delta: int,
) -> bytes:
    """Build the StakeSlash extras tail (51 bytes).

    Layout: ``[target:32][slash_amount_be:16][rep_event_type:1][points_delta_be:2 i16]``.
    """
    if not 0 <= slash_amount < 2**128:
        raise ValueError(f"slash_amount must fit in u128, got {slash_amount}")
    if not 0 <= rep_event_type <= 0xFF:
        raise ValueError(f"rep_event_type must fit in u8, got {rep_event_type}")
    if not -(2**15) <= points_delta < 2**15:
        raise ValueError(f"points_delta must fit in i16, got {points_delta}")
    target = coerce_address(target_entity_id, field="target_entity_id")
    return (
        target
        + slash_amount.to_bytes(16, "big")
        + bytes([rep_event_type])
        + points_delta.to_bytes(2, "big", signed=True)
    )
