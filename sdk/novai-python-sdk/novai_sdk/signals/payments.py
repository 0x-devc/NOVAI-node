"""Signal types 16 (PaymentRequest) and 17 (ServiceAttestation).

The PaymentRequest extras carry three optional features layered on top of a
fixed 112-byte base:

* Week 28 base (112 bytes): payee, amount, service_descriptor_hash,
  request_hash, max_block_height.
* Week 33 splits (optional, 1 + N*34 bytes for N in [2, 8]).
* Week 36 condition (optional, 2..=66 bytes).

The on-chain decoder dispatches at offset 178 of the signal payload (which
is offset 112 within these extras) on the marker byte:

* absent (extras length == 112) -> legacy single-recipient payment
* value in [2, 8] -> Week 33 splits trailer with that many entries
* value == 0xC1 (PAYMENT_CONDITION_MARKER) -> Week 36 condition body,
  optionally followed by a splits trailer

The CLI appends the condition first then the splits when both are present;
this builder follows the same order so the resulting bytes are
byte-identical to the CLI output for the same inputs.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass

from novai_sdk._hex import coerce_address, coerce_hash32
from novai_sdk.constants import (
    BPS_DENOMINATOR,
    MAX_PAYMENT_SPLITS,
    MIN_PAYMENT_SPLITS_WHEN_PRESENT,
    ORACLE_ANCHOR_DATA_TAG_MAX_LEN,
    PAYMENT_CONDITION_MARKER,
)
from novai_sdk.enums import PaymentAttestationStatus, PaymentConditionKind


@dataclass(frozen=True)
class PaymentSplit:
    """One entry in the multi-party splits trailer (Week 33).

    Args:
        recipient_entity_id: 32-byte entity ID receiving this share.
        basis_points: u16 share in basis points (1..=10_000). All split
            basis_points across an entire request must sum to exactly
            ``BPS_DENOMINATOR (10_000)``.
    """

    recipient_entity_id: bytes
    basis_points: int

    def __post_init__(self) -> None:
        if len(self.recipient_entity_id) != 32:
            raise ValueError(
                f"recipient_entity_id must be 32 bytes, got {len(self.recipient_entity_id)}"
            )
        if not 1 <= self.basis_points <= BPS_DENOMINATOR:
            raise ValueError(
                f"basis_points must be in [1, {BPS_DENOMINATOR}], got {self.basis_points}"
            )


@dataclass(frozen=True)
class PaymentCondition:
    """A Week 36 conditional-execution gate referencing an oracle anchor.

    Construct via the classmethods, not the raw initializer. Each kind has
    its own operand layout:

    * ``ANCHOR_EXISTS``: only ``anchor_signal_hash`` matters.
    * ``ANCHOR_DATA_HASH_EQUALS``: also requires ``expected_data_hash``.
    * ``ANCHOR_TAG_EQUALS``: also requires ``expected_tag`` (1..=32 bytes).
    * ``ANCHOR_NOT_EXPIRED``: only ``anchor_signal_hash`` matters.
    """

    kind: PaymentConditionKind
    anchor_signal_hash: bytes
    expected_data_hash: bytes | None = None
    expected_tag: bytes | None = None

    def __post_init__(self) -> None:
        if len(self.anchor_signal_hash) != 32:
            raise ValueError("anchor_signal_hash must be 32 bytes")
        if self.kind == PaymentConditionKind.ANCHOR_DATA_HASH_EQUALS and (
            self.expected_data_hash is None or len(self.expected_data_hash) != 32
        ):
            raise ValueError("ANCHOR_DATA_HASH_EQUALS requires a 32-byte expected_data_hash")
        if self.kind == PaymentConditionKind.ANCHOR_TAG_EQUALS:
            if not self.expected_tag:
                raise ValueError("ANCHOR_TAG_EQUALS requires a non-empty expected_tag")
            if len(self.expected_tag) > ORACLE_ANCHOR_DATA_TAG_MAX_LEN:
                raise ValueError(
                    f"expected_tag must be <= {ORACLE_ANCHOR_DATA_TAG_MAX_LEN} bytes"
                )

    @classmethod
    def anchor_exists(cls, anchor_signal_hash: bytes | str) -> PaymentCondition:
        """Condition kind 1: the referenced anchor must exist on-chain."""
        return cls(
            kind=PaymentConditionKind.ANCHOR_EXISTS,
            anchor_signal_hash=coerce_hash32(anchor_signal_hash, field="anchor_signal_hash"),
        )

    @classmethod
    def anchor_data_hash_equals(
        cls, anchor_signal_hash: bytes | str, expected_data_hash: bytes | str
    ) -> PaymentCondition:
        """Condition kind 2: the anchor's ``data_hash`` must match ``expected_data_hash``."""
        return cls(
            kind=PaymentConditionKind.ANCHOR_DATA_HASH_EQUALS,
            anchor_signal_hash=coerce_hash32(anchor_signal_hash, field="anchor_signal_hash"),
            expected_data_hash=coerce_hash32(expected_data_hash, field="expected_data_hash"),
        )

    @classmethod
    def anchor_tag_equals(
        cls, anchor_signal_hash: bytes | str, expected_tag: bytes | str
    ) -> PaymentCondition:
        """Condition kind 3: the anchor's ``data_tag`` must equal ``expected_tag``."""
        tag_bytes = expected_tag.encode("utf-8") if isinstance(expected_tag, str) else expected_tag
        return cls(
            kind=PaymentConditionKind.ANCHOR_TAG_EQUALS,
            anchor_signal_hash=coerce_hash32(anchor_signal_hash, field="anchor_signal_hash"),
            expected_tag=tag_bytes,
        )

    @classmethod
    def anchor_not_expired(cls, anchor_signal_hash: bytes | str) -> PaymentCondition:
        """Condition kind 4: anchor's ``expiry_height`` must be 0 or >= current height."""
        return cls(
            kind=PaymentConditionKind.ANCHOR_NOT_EXPIRED,
            anchor_signal_hash=coerce_hash32(anchor_signal_hash, field="anchor_signal_hash"),
        )

    def encode(self) -> bytes:
        """Encode the condition into its wire trailer (without the 0xC1 marker)."""
        kind_byte = bytes([int(self.kind)])
        if self.kind == PaymentConditionKind.ANCHOR_EXISTS:
            return kind_byte + self.anchor_signal_hash
        if self.kind == PaymentConditionKind.ANCHOR_DATA_HASH_EQUALS:
            assert self.expected_data_hash is not None
            return kind_byte + self.anchor_signal_hash + self.expected_data_hash
        if self.kind == PaymentConditionKind.ANCHOR_TAG_EQUALS:
            assert self.expected_tag is not None
            return (
                kind_byte
                + self.anchor_signal_hash
                + bytes([len(self.expected_tag)])
                + self.expected_tag
            )
        if self.kind == PaymentConditionKind.ANCHOR_NOT_EXPIRED:
            return kind_byte + self.anchor_signal_hash
        raise ValueError(f"unknown PaymentConditionKind {self.kind}")


def validate_splits(splits: Sequence[PaymentSplit], primary_payee: bytes) -> None:
    """Run the same client-side validation the Rust CLI does.

    Raises:
        ValueError: If any of the runtime invariants would fail.
    """
    if not (MIN_PAYMENT_SPLITS_WHEN_PRESENT <= len(splits) <= MAX_PAYMENT_SPLITS):
        raise ValueError(
            f"splits must contain [{MIN_PAYMENT_SPLITS_WHEN_PRESENT}, {MAX_PAYMENT_SPLITS}] "
            f"entries when present, got {len(splits)}"
        )
    if splits[0].recipient_entity_id != primary_payee:
        raise ValueError("splits[0].recipient_entity_id must equal the primary payee_entity_id")
    seen: set[bytes] = set()
    total_bp = 0
    for split in splits:
        if split.recipient_entity_id in seen:
            raise ValueError(
                f"split recipient {split.recipient_entity_id.hex()} appears more than once"
            )
        seen.add(split.recipient_entity_id)
        total_bp += split.basis_points
    if total_bp != BPS_DENOMINATOR:
        raise ValueError(
            f"sum of basis_points must equal {BPS_DENOMINATOR}, got {total_bp}"
        )


def _encode_splits(splits: Sequence[PaymentSplit]) -> bytes:
    """Encode a splits trailer (count + N entries) into wire bytes."""
    out = bytearray([len(splits)])
    for split in splits:
        out.extend(split.recipient_entity_id)
        out.extend(split.basis_points.to_bytes(2, "big"))
    return bytes(out)


def build_payment_request_extras(
    payee_entity_id: bytes | str,
    amount: int,
    service_descriptor_hash: bytes | str,
    request_hash: bytes | str,
    max_block_height: int,
    *,
    splits: Sequence[PaymentSplit] | None = None,
    condition: PaymentCondition | None = None,
) -> bytes:
    """Build the PaymentRequest extras tail.

    Total length varies::

        * legacy: 112 bytes
        * with condition only: 112 + condition_len (2..=66 + marker byte)
        * with splits only: 112 + 1 + N*34 (N in [2, 8])
        * with both: 112 + 1 + condition_body + 1 + N*34

    Condition is appended BEFORE splits (matches the CLI). The runtime
    decoder dispatches at offset 178 of the signal payload (which is byte
    112 of this extras blob) on whether that byte is 0xC1 (condition) or in
    [2, 8] (splits count).
    """
    if not 0 < amount < 2**64:
        raise ValueError(f"amount must be in (0, 2^64), got {amount}")
    if not 0 <= max_block_height < 2**64:
        raise ValueError(f"max_block_height must fit in u64, got {max_block_height}")
    payee = coerce_address(payee_entity_id, field="payee_entity_id")
    sd_hash = coerce_hash32(service_descriptor_hash, field="service_descriptor_hash")
    req_hash = coerce_hash32(request_hash, field="request_hash")

    out = bytearray()
    out.extend(payee)
    out.extend(amount.to_bytes(8, "big"))
    out.extend(sd_hash)
    out.extend(req_hash)
    out.extend(max_block_height.to_bytes(8, "big"))

    # Optional Week 36 condition trailer (marker | kind | body).
    if condition is not None:
        out.append(PAYMENT_CONDITION_MARKER)
        out.extend(condition.encode())

    # Optional Week 33 splits trailer.
    if splits is not None:
        if len(splits) == 0:
            raise ValueError("splits cannot be an empty sequence; pass None for legacy mode")
        validate_splits(splits, payee)
        out.extend(_encode_splits(splits))

    return bytes(out)


def build_service_attestation_extras(
    payment_signal_hash: bytes | str,
    payee_entity_id: bytes | str,
    status: PaymentAttestationStatus,
) -> bytes:
    """Build the ServiceAttestation extras tail (65 bytes, signal type 17).

    Layout: ``[payment_signal_hash:32][payee_entity_id:32][status:1]``.
    """
    psh = coerce_hash32(payment_signal_hash, field="payment_signal_hash")
    payee = coerce_address(payee_entity_id, field="payee_entity_id")
    return psh + payee + bytes([int(status)])
