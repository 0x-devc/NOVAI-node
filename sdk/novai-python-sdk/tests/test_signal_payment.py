"""Tests for the PaymentRequest three-way dispatch: legacy / splits / condition / both.

This is the most consequential builder in the SDK: the wire format
overloads byte 178 of the signal payload (byte 112 of the extras) to
distinguish between:

* legacy single-recipient (no trailing bytes)
* Week 33 splits trailer (byte 178 == split count in [2, 8])
* Week 36 condition (byte 178 == PAYMENT_CONDITION_MARKER 0xC1)
* both: condition body immediately followed by splits

Tests verify byte-level layout for each shape plus the same validation
rules the chain enforces.
"""

from __future__ import annotations

import pytest

from novai_sdk.constants import (
    BPS_DENOMINATOR,
    PAYMENT_CONDITION_MARKER,
)
from novai_sdk.enums import PaymentConditionKind
from novai_sdk.signals.payments import (
    PaymentCondition,
    PaymentSplit,
    build_payment_request_extras,
    validate_splits,
)

# Convenient deterministic fixtures.
PAYEE = bytes([0x11] * 32)
SD_HASH = bytes([0x22] * 32)
REQ_HASH = bytes([0x33] * 32)
ANCHOR = bytes([0x44] * 32)
EXPECTED_DATA = bytes([0x55] * 32)


def _base(amount: int = 5000) -> bytes:
    """Build the legacy 112-byte base extras."""
    return build_payment_request_extras(
        payee_entity_id=PAYEE,
        amount=amount,
        service_descriptor_hash=SD_HASH,
        request_hash=REQ_HASH,
        max_block_height=1_000_000,
    )


class TestLegacyShape:
    def test_length_112(self) -> None:
        assert len(_base()) == 112

    def test_layout(self) -> None:
        extras = _base(amount=42)
        assert extras[0:32] == PAYEE
        assert extras[32:40] == (42).to_bytes(8, "big")
        assert extras[40:72] == SD_HASH
        assert extras[72:104] == REQ_HASH
        assert extras[104:112] == (1_000_000).to_bytes(8, "big")


class TestSplitsShape:
    def _two_split(self) -> list[PaymentSplit]:
        return [
            PaymentSplit(recipient_entity_id=PAYEE, basis_points=7000),
            PaymentSplit(recipient_entity_id=bytes([0xAA] * 32), basis_points=3000),
        ]

    def test_length(self) -> None:
        extras = build_payment_request_extras(
            PAYEE, 5000, SD_HASH, REQ_HASH, 1_000_000, splits=self._two_split()
        )
        # 112 + 1 + 2 * 34 = 181
        assert len(extras) == 181

    def test_dispatch_byte_at_112_is_count(self) -> None:
        extras = build_payment_request_extras(
            PAYEE, 5000, SD_HASH, REQ_HASH, 1_000_000, splits=self._two_split()
        )
        assert extras[112] == 2  # split count

    def test_split_entries_layout(self) -> None:
        extras = build_payment_request_extras(
            PAYEE, 5000, SD_HASH, REQ_HASH, 1_000_000, splits=self._two_split()
        )
        # entry 0 starts at 113
        assert extras[113:145] == PAYEE
        assert extras[145:147] == (7000).to_bytes(2, "big")
        assert extras[147:179] == bytes([0xAA] * 32)
        assert extras[179:181] == (3000).to_bytes(2, "big")

    def test_max_8_splits(self) -> None:
        splits = [PaymentSplit(recipient_entity_id=PAYEE, basis_points=BPS_DENOMINATOR - 7)]
        for i in range(7):
            splits.append(
                PaymentSplit(recipient_entity_id=bytes([0xB0 + i] * 32), basis_points=1)
            )
        extras = build_payment_request_extras(
            PAYEE, 5000, SD_HASH, REQ_HASH, 1_000_000, splits=splits
        )
        # 112 + 1 + 8 * 34 = 385
        assert len(extras) == 385
        assert extras[112] == 8

    def test_rejects_one_split(self) -> None:
        with pytest.raises(ValueError):
            build_payment_request_extras(
                PAYEE, 5000, SD_HASH, REQ_HASH, 1_000_000,
                splits=[PaymentSplit(recipient_entity_id=PAYEE, basis_points=10_000)],
            )

    def test_rejects_nine_splits(self) -> None:
        splits = [PaymentSplit(recipient_entity_id=PAYEE, basis_points=10_000 - 8)]
        for i in range(8):
            splits.append(
                PaymentSplit(recipient_entity_id=bytes([0xB0 + i] * 32), basis_points=1)
            )
        with pytest.raises(ValueError):
            build_payment_request_extras(PAYEE, 5000, SD_HASH, REQ_HASH, 1_000_000, splits=splits)

    def test_rejects_basis_points_not_summing_to_10000(self) -> None:
        with pytest.raises(ValueError, match="basis_points must equal"):
            build_payment_request_extras(
                PAYEE, 5000, SD_HASH, REQ_HASH, 1_000_000,
                splits=[
                    PaymentSplit(recipient_entity_id=PAYEE, basis_points=7000),
                    PaymentSplit(recipient_entity_id=bytes([0xAA] * 32), basis_points=2999),
                ],
            )

    def test_rejects_primary_not_first(self) -> None:
        with pytest.raises(ValueError, match="splits\\[0\\]"):
            build_payment_request_extras(
                PAYEE, 5000, SD_HASH, REQ_HASH, 1_000_000,
                splits=[
                    PaymentSplit(recipient_entity_id=bytes([0xAA] * 32), basis_points=7000),
                    PaymentSplit(recipient_entity_id=PAYEE, basis_points=3000),
                ],
            )

    def test_rejects_duplicate_recipient(self) -> None:
        with pytest.raises(ValueError, match="more than once"):
            build_payment_request_extras(
                PAYEE, 5000, SD_HASH, REQ_HASH, 1_000_000,
                splits=[
                    PaymentSplit(recipient_entity_id=PAYEE, basis_points=7000),
                    PaymentSplit(recipient_entity_id=PAYEE, basis_points=3000),
                ],
            )

    def test_validate_splits_passes_with_correct_input(self) -> None:
        # Should not raise.
        validate_splits(
            [
                PaymentSplit(recipient_entity_id=PAYEE, basis_points=5000),
                PaymentSplit(recipient_entity_id=bytes([0xAA] * 32), basis_points=5000),
            ],
            primary_payee=PAYEE,
        )


class TestConditionShape:
    def test_anchor_exists_length(self) -> None:
        cond = PaymentCondition.anchor_exists(ANCHOR)
        extras = build_payment_request_extras(
            PAYEE, 5000, SD_HASH, REQ_HASH, 1_000_000, condition=cond
        )
        # 112 + 1(marker) + 1(kind) + 32(anchor) = 146
        assert len(extras) == 146

    def test_anchor_exists_dispatch_byte_is_marker(self) -> None:
        cond = PaymentCondition.anchor_exists(ANCHOR)
        extras = build_payment_request_extras(
            PAYEE, 5000, SD_HASH, REQ_HASH, 1_000_000, condition=cond
        )
        assert extras[112] == PAYMENT_CONDITION_MARKER  # 0xC1
        assert extras[113] == int(PaymentConditionKind.ANCHOR_EXISTS)
        assert extras[114:146] == ANCHOR

    def test_anchor_data_hash_equals_length_and_layout(self) -> None:
        cond = PaymentCondition.anchor_data_hash_equals(ANCHOR, EXPECTED_DATA)
        extras = build_payment_request_extras(
            PAYEE, 5000, SD_HASH, REQ_HASH, 1_000_000, condition=cond
        )
        # 112 + 1(marker) + 1(kind) + 32(anchor) + 32(expected) = 178
        assert len(extras) == 178
        assert extras[112] == PAYMENT_CONDITION_MARKER
        assert extras[113] == int(PaymentConditionKind.ANCHOR_DATA_HASH_EQUALS)
        assert extras[114:146] == ANCHOR
        assert extras[146:178] == EXPECTED_DATA

    def test_anchor_tag_equals_length_and_layout(self) -> None:
        cond = PaymentCondition.anchor_tag_equals(ANCHOR, "price/ETH-USD")
        extras = build_payment_request_extras(
            PAYEE, 5000, SD_HASH, REQ_HASH, 1_000_000, condition=cond
        )
        # 112 + 1(marker) + 1(kind) + 32(anchor) + 1(tag_len) + 13(tag) = 160
        assert len(extras) == 160
        assert extras[112] == PAYMENT_CONDITION_MARKER
        assert extras[113] == int(PaymentConditionKind.ANCHOR_TAG_EQUALS)
        assert extras[114:146] == ANCHOR
        assert extras[146] == 13
        assert extras[147:160] == b"price/ETH-USD"

    def test_anchor_not_expired_length(self) -> None:
        cond = PaymentCondition.anchor_not_expired(ANCHOR)
        extras = build_payment_request_extras(
            PAYEE, 5000, SD_HASH, REQ_HASH, 1_000_000, condition=cond
        )
        # 112 + 1 + 1 + 32 = 146
        assert len(extras) == 146
        assert extras[113] == int(PaymentConditionKind.ANCHOR_NOT_EXPIRED)

    def test_tag_equals_rejects_oversized_tag(self) -> None:
        with pytest.raises(ValueError):
            PaymentCondition.anchor_tag_equals(ANCHOR, "x" * 33)

    def test_tag_equals_rejects_empty_tag(self) -> None:
        with pytest.raises(ValueError):
            PaymentCondition.anchor_tag_equals(ANCHOR, "")


class TestConditionPlusSplits:
    def test_layout_is_condition_then_splits(self) -> None:
        """When both are present the condition body precedes the splits trailer."""
        cond = PaymentCondition.anchor_exists(ANCHOR)
        splits = [
            PaymentSplit(recipient_entity_id=PAYEE, basis_points=6000),
            PaymentSplit(recipient_entity_id=bytes([0xAA] * 32), basis_points=4000),
        ]
        extras = build_payment_request_extras(
            PAYEE, 5000, SD_HASH, REQ_HASH, 1_000_000,
            condition=cond, splits=splits,
        )
        # 112 base + 34 condition (marker+kind+anchor) + 1 count + 2*34 splits = 215
        assert len(extras) == 215
        # The marker still sits at offset 112 (dispatch byte).
        assert extras[112] == PAYMENT_CONDITION_MARKER
        # After the 34-byte condition the splits count appears at offset 146.
        assert extras[146] == 2
        # And the first split entry at 147 is the primary payee.
        assert extras[147:179] == PAYEE
        assert extras[179:181] == (6000).to_bytes(2, "big")
