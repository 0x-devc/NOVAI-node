"""Signal commitment envelope builder (tx type 2).

The signal commitment payload has a fixed 66-byte envelope followed by a
type-specific extras tail. Each ``AiSignalType`` has its own extras encoder
in ``novai_sdk.signals``; this module assembles the envelope and concatenates
the extras.
"""

from __future__ import annotations

from novai_sdk._hex import coerce_address, coerce_hash32
from novai_sdk.enums import AiSignalType, TxPayloadType


def build_signal_commitment_payload(
    signal_hash: bytes | str,
    signal_type: AiSignalType,
    issuer_entity_id: bytes | str,
    extras: bytes = b"",
) -> bytes:
    """Build the full tx-type-2 SignalCommitment payload.

    Layout: ``[0x02][signal_hash:32][signal_type:1][issuer:32][extras:N]``.

    Total length: ``66 + len(extras)``. The 23 signal types have different
    extras layouts; pass the appropriate extras bytes from
    ``novai_sdk.signals`` (e.g. ``build_payment_request_extras(...)``).
    """
    sh = coerce_hash32(signal_hash, field="signal_hash")
    issuer = coerce_address(issuer_entity_id, field="issuer_entity_id")
    return (
        bytes([int(TxPayloadType.SIGNAL_COMMITMENT)])
        + sh
        + bytes([int(signal_type)])
        + issuer
        + extras
    )
