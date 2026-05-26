"""Signal types 19, 20, 21: payment channel signals (Week 32).

* ChannelAccept (19): 64-byte tail. Issuer is party B; carries the channel
  object ID and party A entity ID so the runtime can resolve the memory
  object owner.
* ChannelClose (20): 233-byte tail. Carries the off-chain state (nonce,
  balance_a, balance_b, is_final) plus both parties' ed25519 signatures
  over those bytes (see :func:`novai_sdk.sign_channel_state`).
* ChannelFinalize (21): 64-byte tail. Permissionless after the dispute
  deadline expires; carries the channel object ID and party A entity ID.

The off-chain state signatures bound to a ``ChannelClose`` are computed
externally via :func:`novai_sdk.crypto.sign_channel_state` and exchanged
between the two parties over their own transport before either submits
the on-chain close.
"""

from __future__ import annotations

from novai_sdk._hex import coerce_address, coerce_hash32, coerce_signature


def build_channel_accept_extras(
    channel_object_id: bytes | str,
    party_a_entity_id: bytes | str,
) -> bytes:
    """Build the ChannelAccept extras tail (64 bytes)."""
    cid = coerce_hash32(channel_object_id, field="channel_object_id")
    pa = coerce_address(party_a_entity_id, field="party_a_entity_id")
    return cid + pa


def build_channel_finalize_extras(
    channel_object_id: bytes | str,
    party_a_entity_id: bytes | str,
) -> bytes:
    """Build the ChannelFinalize extras tail (64 bytes)."""
    cid = coerce_hash32(channel_object_id, field="channel_object_id")
    pa = coerce_address(party_a_entity_id, field="party_a_entity_id")
    return cid + pa


def build_channel_close_extras(
    channel_object_id: bytes | str,
    party_a_entity_id: bytes | str,
    nonce: int,
    balance_a: int,
    balance_b: int,
    is_final: bool,
    sig_a: bytes | str,
    sig_b: bytes | str,
) -> bytes:
    """Build the ChannelClose extras tail (233 bytes).

    Layout::

        [channel_object_id:32][party_a:32][nonce_be:8]
        [balance_a_be:16][balance_b_be:16][is_final:1]
        [sig_a:64][sig_b:64]

    ``sig_a`` and ``sig_b`` must each be a valid ed25519 signature over the
    canonical 167-byte channel state bytes (see
    :func:`novai_sdk.crypto.channel_state_signing_bytes`), produced by the
    matching party's signing key.
    """
    if not 0 <= nonce < 2**64:
        raise ValueError("nonce must fit in u64")
    if not 0 <= balance_a < 2**128:
        raise ValueError("balance_a must fit in u128")
    if not 0 <= balance_b < 2**128:
        raise ValueError("balance_b must fit in u128")
    cid = coerce_hash32(channel_object_id, field="channel_object_id")
    pa = coerce_address(party_a_entity_id, field="party_a_entity_id")
    sa = coerce_signature(sig_a, field="sig_a")
    sb = coerce_signature(sig_b, field="sig_b")
    return (
        cid
        + pa
        + nonce.to_bytes(8, "big")
        + balance_a.to_bytes(16, "big")
        + balance_b.to_bytes(16, "big")
        + (b"\x01" if is_final else b"\x00")
        + sa
        + sb
    )
