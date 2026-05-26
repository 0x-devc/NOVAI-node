"""Tests for novai_sdk.crypto (signing, address derivation, channel state)."""

from __future__ import annotations

import pytest

from novai_sdk import (
    Keypair,
    TxV1,
    address_from_pubkey,
    compute_entity_id,
    sign_channel_state,
    sign_tx_v1,
    verify_channel_state_signature,
    verify_tx_v1,
)
from novai_sdk.constants import (
    DOMAIN_TAG_ADDRESS_V1,
    DOMAIN_TAG_AI_ENTITY_ID_V1,
    DOMAIN_TAG_CHANNEL_STATE_V1,
    DOMAIN_TAG_TX_V1,
)
from novai_sdk.crypto import (
    blake3_hash,
    channel_state_signing_bytes,
)


class TestAddressFromPubkey:
    def test_deterministic(self) -> None:
        pk = bytes([1] * 32)
        a = address_from_pubkey(pk)
        b = address_from_pubkey(pk)
        assert a == b
        assert len(a) == 32

    def test_different_pubkeys_different_addresses(self) -> None:
        assert address_from_pubkey(bytes([1] * 32)) != address_from_pubkey(bytes([2] * 32))

    def test_matches_blake3_construction(self) -> None:
        """address := blake3(DOMAIN_TAG_ADDRESS_V1 || pubkey)."""
        pk = bytes([7] * 32)
        expected = blake3_hash(DOMAIN_TAG_ADDRESS_V1 + pk)
        assert address_from_pubkey(pk) == expected

    def test_rejects_wrong_length(self) -> None:
        with pytest.raises(ValueError):
            address_from_pubkey(b"\x00" * 31)


class TestComputeEntityId:
    def test_deterministic(self) -> None:
        code_hash = bytes([0x42] * 32)
        creator = bytes([0x01] * 32)
        assert compute_entity_id(code_hash, creator) == compute_entity_id(code_hash, creator)

    def test_changes_with_code(self) -> None:
        creator = bytes([1] * 32)
        a = compute_entity_id(bytes([0x42] * 32), creator)
        b = compute_entity_id(bytes([0x43] * 32), creator)
        assert a != b

    def test_changes_with_creator(self) -> None:
        code = bytes([0x42] * 32)
        a = compute_entity_id(code, bytes([1] * 32))
        b = compute_entity_id(code, bytes([2] * 32))
        assert a != b

    def test_matches_blake3_construction(self) -> None:
        code = bytes([0x42] * 32)
        creator = bytes([0x01] * 32)
        expected = blake3_hash(DOMAIN_TAG_AI_ENTITY_ID_V1 + code + creator)
        assert compute_entity_id(code, creator) == expected


class TestSignTxV1:
    def _build_tx(self, kp: Keypair, payload: bytes = b"hello") -> TxV1:
        return TxV1(
            from_address=kp.address,
            pubkey=kp.pubkey,
            nonce=1,
            fee=5,
            payload=payload,
        )

    def test_sign_and_verify_roundtrip(self, kp_alice: Keypair) -> None:
        tx = self._build_tx(kp_alice)
        sig = sign_tx_v1(kp_alice.signing_key, tx)
        assert len(sig) == 64
        tx.sig = sig
        assert verify_tx_v1(kp_alice.verifying_key, tx) is True

    def test_mutating_field_breaks_signature(self, kp_alice: Keypair) -> None:
        tx = self._build_tx(kp_alice)
        tx.sig = sign_tx_v1(kp_alice.signing_key, tx)
        tx.fee += 1
        assert verify_tx_v1(kp_alice.verifying_key, tx) is False

    def test_signing_is_deterministic(self, kp_alice: Keypair) -> None:
        """ed25519 produces deterministic signatures for fixed seed/message."""
        tx1 = self._build_tx(kp_alice)
        tx2 = self._build_tx(kp_alice)
        sig1 = sign_tx_v1(kp_alice.signing_key, tx1)
        sig2 = sign_tx_v1(kp_alice.signing_key, tx2)
        assert sig1 == sig2

    def test_signing_uses_domain_tag(self, kp_alice: Keypair) -> None:
        """Signature must NOT verify against the unsigned bytes without domain tag.

        This is the contract: ``sign(b"NOVAI_TX_V1" || unsigned_bytes)``.
        """
        from nacl.exceptions import BadSignatureError

        from novai_sdk.codec import encode_tx_v1_unsigned

        tx = self._build_tx(kp_alice)
        tx.sig = sign_tx_v1(kp_alice.signing_key, tx)
        unsigned = encode_tx_v1_unsigned(tx)
        # The wrong message (no domain tag) must fail to verify.
        with pytest.raises(BadSignatureError):
            kp_alice.verifying_key.verify(unsigned, tx.sig)
        # The correct message (with domain tag) must verify.
        kp_alice.verifying_key.verify(DOMAIN_TAG_TX_V1 + unsigned, tx.sig)


class TestChannelStateSigningBytes:
    def test_length_is_167(self) -> None:
        msg = channel_state_signing_bytes(
            chain_id=1,
            channel_object_id=bytes(32),
            party_a=bytes(32),
            party_b=bytes(32),
            nonce=0,
            balance_a=0,
            balance_b=0,
            is_final=False,
        )
        assert len(msg) == 167

    def test_starts_with_domain_tag(self) -> None:
        msg = channel_state_signing_bytes(
            chain_id=1,
            channel_object_id=bytes(32),
            party_a=bytes(32),
            party_b=bytes(32),
            nonce=0,
            balance_a=0,
            balance_b=0,
            is_final=False,
        )
        assert msg.startswith(DOMAIN_TAG_CHANNEL_STATE_V1)

    def test_endianness_is_big(self) -> None:
        """All numeric fields in the channel state are big-endian, not LE."""
        msg = channel_state_signing_bytes(
            chain_id=0x0123_4567_89AB_CDEF,
            channel_object_id=bytes(32),
            party_a=bytes(32),
            party_b=bytes(32),
            nonce=1,
            balance_a=2,
            balance_b=3,
            is_final=True,
        )
        # After the 22-byte domain tag, chain_id occupies offsets 22..30 BE.
        assert msg[22:30] == (0x0123_4567_89AB_CDEF).to_bytes(8, "big")
        assert msg[-1] == 1  # is_final True == 0x01

    def test_is_final_false_encodes_zero(self) -> None:
        msg = channel_state_signing_bytes(
            chain_id=1,
            channel_object_id=bytes(32),
            party_a=bytes(32),
            party_b=bytes(32),
            nonce=0,
            balance_a=0,
            balance_b=0,
            is_final=False,
        )
        assert msg[-1] == 0


class TestSignChannelState:
    def test_sign_verify_roundtrip(self, kp_alice: Keypair, kp_bob: Keypair) -> None:
        channel = bytes([0xAA] * 32)
        sig_a = sign_channel_state(
            kp_alice.signing_key,
            channel_object_id=channel,
            party_a=kp_alice.address,
            party_b=kp_bob.address,
            nonce=1,
            balance_a=1000,
            balance_b=500,
            is_final=False,
        )
        assert verify_channel_state_signature(
            sig_a,
            kp_alice.pubkey,
            channel,
            kp_alice.address,
            kp_bob.address,
            nonce=1,
            balance_a=1000,
            balance_b=500,
            is_final=False,
        )

    def test_mutating_any_field_breaks_signature(
        self, kp_alice: Keypair, kp_bob: Keypair
    ) -> None:
        channel = bytes([0xAA] * 32)
        base = {
            "channel_object_id": channel,
            "party_a": kp_alice.address,
            "party_b": kp_bob.address,
            "nonce": 1,
            "balance_a": 1000,
            "balance_b": 500,
            "is_final": False,
        }
        sig = sign_channel_state(kp_alice.signing_key, **base)  # type: ignore[arg-type]

        mutations: list[dict[str, object]] = [
            {"nonce": 2},
            {"balance_a": 1001},
            {"balance_b": 501},
            {"is_final": True},
            {"channel_object_id": bytes([0xFF] * 32)},
        ]
        for mutation in mutations:
            tampered = {**base, **mutation}
            assert not verify_channel_state_signature(
                sig, kp_alice.pubkey, **tampered  # type: ignore[arg-type]
            )

    def test_cross_key_verification_fails(self, kp_alice: Keypair, kp_bob: Keypair) -> None:
        """A signature from Alice must NOT verify under Bob's pubkey."""
        channel = bytes([0xAA] * 32)
        sig_a = sign_channel_state(
            kp_alice.signing_key,
            channel_object_id=channel,
            party_a=kp_alice.address,
            party_b=kp_bob.address,
            nonce=1,
            balance_a=1000,
            balance_b=500,
            is_final=False,
        )
        assert not verify_channel_state_signature(
            sig_a,
            kp_bob.pubkey,
            channel,
            kp_alice.address,
            kp_bob.address,
            nonce=1,
            balance_a=1000,
            balance_b=500,
            is_final=False,
        )

    def test_garbage_pubkey_returns_false(self, kp_alice: Keypair, kp_bob: Keypair) -> None:
        channel = bytes([0xAA] * 32)
        sig = sign_channel_state(
            kp_alice.signing_key,
            channel_object_id=channel,
            party_a=kp_alice.address,
            party_b=kp_bob.address,
            nonce=1,
            balance_a=1000,
            balance_b=500,
            is_final=False,
        )
        # All-zero pubkey is not a valid ed25519 point.
        assert not verify_channel_state_signature(
            sig,
            bytes(32),
            channel,
            kp_alice.address,
            kp_bob.address,
            nonce=1,
            balance_a=1000,
            balance_b=500,
            is_final=False,
        )
