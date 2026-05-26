"""Signal type 18: SlaAccept (Week 31).

Extras layout (64 bytes)::

    [sla_object_id:32][buyer_entity_id:32]

The issuer of a SlaAccept signal is the SELLER (the entity whose stake is at
risk on threshold breach). The CLI derives a content-addressed
``signal_hash`` from ``blake3("novai-sla-accept-v1" || sla_object_id ||
buyer_entity_id)``; that derivation is exposed here as
:func:`derive_sla_accept_signal_hash`.
"""

from __future__ import annotations

from novai_sdk._hex import coerce_address, coerce_hash32
from novai_sdk.crypto import blake3_keyed


def derive_sla_accept_signal_hash(
    sla_object_id: bytes | str,
    buyer_entity_id: bytes | str,
) -> bytes:
    """Derive the canonical signal hash for an SlaAccept signal.

    ``signal_hash := blake3("novai-sla-accept-v1" || sla_object_id || buyer_entity_id)``.
    """
    return blake3_keyed(
        b"novai-sla-accept-v1",
        coerce_hash32(sla_object_id, field="sla_object_id"),
        coerce_address(buyer_entity_id, field="buyer_entity_id"),
    )


def build_sla_accept_extras(
    sla_object_id: bytes | str,
    buyer_entity_id: bytes | str,
) -> bytes:
    """Build the SlaAccept extras tail (64 bytes)."""
    sla_id = coerce_hash32(sla_object_id, field="sla_object_id")
    buyer = coerce_address(buyer_entity_id, field="buyer_entity_id")
    return sla_id + buyer
