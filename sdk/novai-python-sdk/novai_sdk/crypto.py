"""Cryptographic primitives.

All hash functions, domain separators, and signing flows in this module match
the corresponding Rust definitions in ``crates/crypto/src/lib.rs`` and
``crates/codec/src/lib.rs`` byte-for-byte. The canonical reference for
``sign_tx_v1`` is::

    sign_bytes(sk, b"NOVAI_TX_V1" || encode_tx_v1_unsigned(tx))

That is: prepend the 11-byte ASCII tag to the canonical unsigned bytes and
then perform a plain ed25519 detached signature over the result.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

import blake3
from nacl.exceptions import BadSignatureError
from nacl.signing import SigningKey, VerifyKey

from novai_sdk.constants import (
    DOMAIN_TAG_ADDRESS_V1,
    DOMAIN_TAG_AI_ENTITY_ID_V1,
    DOMAIN_TAG_CHANNEL_STATE_V1,
    DOMAIN_TAG_TX_V1,
    NOVAI_CHANNEL_CHAIN_ID,
)

if TYPE_CHECKING:
    from novai_sdk.codec import TxV1


def address_from_pubkey(pubkey: bytes) -> bytes:
    """Derive the canonical 32-byte NOVAI address from a 32-byte ed25519 pubkey.

    Address := ``blake3("NOVAI_ADDRESS_V1" || pubkey)``.
    """
    if len(pubkey) != 32:
        raise ValueError(f"pubkey must be exactly 32 bytes, got {len(pubkey)}")
    hasher = blake3.blake3()
    hasher.update(DOMAIN_TAG_ADDRESS_V1)
    hasher.update(pubkey)
    return hasher.digest()


def compute_entity_id(code_hash: bytes, creator: bytes) -> bytes:
    """Derive the canonical 32-byte AI entity ID.

    Entity ID := ``blake3("NOVAI_AI_ENTITY_ID_V1" || code_hash || creator)``.
    """
    if len(code_hash) != 32:
        raise ValueError(f"code_hash must be 32 bytes, got {len(code_hash)}")
    if len(creator) != 32:
        raise ValueError(f"creator must be 32 bytes, got {len(creator)}")
    hasher = blake3.blake3()
    hasher.update(DOMAIN_TAG_AI_ENTITY_ID_V1)
    hasher.update(code_hash)
    hasher.update(creator)
    return hasher.digest()


def sign_tx_v1(signing_key: SigningKey, tx: TxV1) -> bytes:
    """Compute the canonical 64-byte ed25519 signature for a TxV1.

    Returns the raw signature bytes; the caller is responsible for placing
    them into ``tx.sig``. The signature is taken over::

        b"NOVAI_TX_V1" || encode_tx_v1_unsigned(tx)

    Mutating any field of ``tx`` after this call invalidates the signature.
    """
    from novai_sdk.codec import encode_tx_v1_unsigned

    unsigned = encode_tx_v1_unsigned(tx)
    message = DOMAIN_TAG_TX_V1 + unsigned
    return bytes(signing_key.sign(message).signature)


def verify_tx_v1(verifying_key: VerifyKey, tx: TxV1) -> bool:
    """Verify a TxV1's signature against the canonical signing domain."""
    from novai_sdk.codec import encode_tx_v1_unsigned

    unsigned = encode_tx_v1_unsigned(tx)
    message = DOMAIN_TAG_TX_V1 + unsigned
    try:
        verifying_key.verify(message, tx.sig)
    except BadSignatureError:
        return False
    return True


def channel_state_signing_bytes(
    chain_id: int,
    channel_object_id: bytes,
    party_a: bytes,
    party_b: bytes,
    nonce: int,
    balance_a: int,
    balance_b: int,
    is_final: bool,
) -> bytes:
    """Build the canonical 167-byte payload both parties sign for a channel update.

    Layout::

        "NOVAI_CHANNEL_STATE_V1" (22) || chain_id_be(8) ||
        channel_object_id(32) || party_a(32) || party_b(32) ||
        nonce_be(8) || balance_a_be(16) || balance_b_be(16) || is_final(1)

    Note the BE endianness throughout; this is distinct from the TxV1 envelope
    which is LE. The chain ID and channel ID prevent cross-deployment and
    cross-channel replay respectively.
    """
    if len(channel_object_id) != 32:
        raise ValueError("channel_object_id must be 32 bytes")
    if len(party_a) != 32:
        raise ValueError("party_a must be 32 bytes")
    if len(party_b) != 32:
        raise ValueError("party_b must be 32 bytes")
    if not 0 <= chain_id < 2**64:
        raise ValueError("chain_id must fit in u64")
    if not 0 <= nonce < 2**64:
        raise ValueError("nonce must fit in u64")
    if not 0 <= balance_a < 2**128:
        raise ValueError("balance_a must fit in u128")
    if not 0 <= balance_b < 2**128:
        raise ValueError("balance_b must fit in u128")
    return (
        DOMAIN_TAG_CHANNEL_STATE_V1
        + chain_id.to_bytes(8, "big")
        + channel_object_id
        + party_a
        + party_b
        + nonce.to_bytes(8, "big")
        + balance_a.to_bytes(16, "big")
        + balance_b.to_bytes(16, "big")
        + (b"\x01" if is_final else b"\x00")
    )


def sign_channel_state(
    signing_key: SigningKey,
    channel_object_id: bytes,
    party_a: bytes,
    party_b: bytes,
    nonce: int,
    balance_a: int,
    balance_b: int,
    is_final: bool,
    *,
    chain_id: int = NOVAI_CHANNEL_CHAIN_ID,
) -> bytes:
    """Sign an off-chain `PaymentChannel` state update.

    Returns the raw 64-byte ed25519 signature suitable for placement in
    ``sig_a`` or ``sig_b`` of a `ChannelClose` signal. Both parties of a
    channel run this independently and exchange the result over their own
    transport.
    """
    msg = channel_state_signing_bytes(
        chain_id,
        channel_object_id,
        party_a,
        party_b,
        nonce,
        balance_a,
        balance_b,
        is_final,
    )
    return bytes(signing_key.sign(msg).signature)


def verify_channel_state_signature(
    signature: bytes,
    pubkey: bytes,
    channel_object_id: bytes,
    party_a: bytes,
    party_b: bytes,
    nonce: int,
    balance_a: int,
    balance_b: int,
    is_final: bool,
    *,
    chain_id: int = NOVAI_CHANNEL_CHAIN_ID,
) -> bool:
    """Verify a channel state signature without constructing a VerifyingKey object."""
    if len(signature) != 64:
        raise ValueError("signature must be 64 bytes")
    if len(pubkey) != 32:
        return False
    try:
        vk = VerifyKey(pubkey)
    except Exception:
        return False
    msg = channel_state_signing_bytes(
        chain_id,
        channel_object_id,
        party_a,
        party_b,
        nonce,
        balance_a,
        balance_b,
        is_final,
    )
    try:
        vk.verify(msg, signature)
    except BadSignatureError:
        return False
    return True


def blake3_hash(data: bytes) -> bytes:
    """Convenience: ``blake3(data)`` returning the raw 32-byte digest."""
    return blake3.blake3(data).digest()


def blake3_keyed(*chunks: bytes) -> bytes:
    """Convenience: blake3 over the concatenation of ``chunks``, returning 32 bytes."""
    hasher = blake3.blake3()
    for chunk in chunks:
        hasher.update(chunk)
    return hasher.digest()
