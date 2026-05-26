"""Tests for novai_sdk.signals (signal extras encoders 7-15, 17, 18, 19, 21)."""

from __future__ import annotations

import pytest

from novai_sdk import AiSignalType
from novai_sdk.signals import (
    build_channel_accept_extras,
    build_channel_close_extras,
    build_channel_finalize_extras,
    build_composition_check_extras,
    build_empty_extras,
    build_proof_submission_extras_groth16,
    build_proof_submission_extras_groth16_registered,
    build_proof_submission_extras_v1_stub,
    build_reputation_update_extras,
    build_service_attestation_extras,
    build_signal_purchase_extras,
    build_sla_accept_extras,
    build_stake_deposit_extras,
    build_stake_slash_extras,
    build_stake_withdraw_extras,
    build_subscription_cancel_extras,
    build_subscription_create_extras,
    derive_sla_accept_signal_hash,
)


class TestBasic:
    def test_empty_extras(self) -> None:
        assert build_empty_extras() == b""


class TestReputationUpdate:
    def test_layout(self) -> None:
        target = bytes([0x11] * 32)
        e = build_reputation_update_extras(target, event_type=5, points_delta=-3)
        assert len(e) == 35
        assert e[0:32] == target
        assert e[32] == 5
        assert e[33:35] == (-3).to_bytes(2, "big", signed=True)

    def test_positive_delta(self) -> None:
        e = build_reputation_update_extras(bytes(32), event_type=0, points_delta=100)
        assert e[33:35] == (100).to_bytes(2, "big", signed=True)

    def test_rejects_overflow_delta(self) -> None:
        with pytest.raises(ValueError):
            build_reputation_update_extras(bytes(32), event_type=0, points_delta=40_000)


class TestSignalPurchase:
    def test_layout(self) -> None:
        seller = bytes([0x22] * 32)
        e = build_signal_purchase_extras(seller, AiSignalType.ANOMALY, max_price=999_999)
        assert len(e) == 41
        assert e[0:32] == seller
        assert e[32] == 0
        assert e[33:41] == (999_999).to_bytes(8, "big")


class TestStake:
    def test_deposit_layout(self) -> None:
        e = build_stake_deposit_extras(amount=10**18)
        assert len(e) == 16
        assert e == (10**18).to_bytes(16, "big")

    def test_withdraw_layout(self) -> None:
        e = build_stake_withdraw_extras(amount=42)
        assert len(e) == 16
        assert e == (42).to_bytes(16, "big")

    def test_slash_layout(self) -> None:
        target = bytes([0x33] * 32)
        e = build_stake_slash_extras(
            target, slash_amount=500_000, rep_event_type=7, points_delta=-5
        )
        assert len(e) == 51
        assert e[0:32] == target
        assert e[32:48] == (500_000).to_bytes(16, "big")
        assert e[48] == 7
        assert e[49:51] == (-5).to_bytes(2, "big", signed=True)


class TestComposition:
    def test_layout(self) -> None:
        target = bytes([0x44] * 32)
        e = build_composition_check_extras(target, failed_dependency_idx=3, failure_reason=2)
        assert len(e) == 34
        assert e[0:32] == target
        assert e[32] == 3
        assert e[33] == 2


class TestProof:
    def test_stub_layout_65_bytes(self) -> None:
        e = build_proof_submission_extras_v1_stub(bytes([0xAA] * 32), bytes([0xBB] * 32))
        assert len(e) == 65
        assert e[0] == 0  # PROOF_TYPE_STUB
        assert e[1:33] == bytes([0xAA] * 32)
        assert e[33:65] == bytes([0xBB] * 32)

    def test_groth16_inline_vk_layout(self) -> None:
        vk = b"V" * 100
        proof = b"P" * 80
        e = build_proof_submission_extras_groth16(
            bytes(32), bytes(32), vk_bytes=vk, proof_bytes=proof
        )
        # 1(proof_type) + 32(code) + 32(comp) + 4(vk_len) + 100(vk) + 4(proof_len) + 80(proof)
        assert len(e) == 1 + 32 + 32 + 4 + 100 + 4 + 80
        assert e[0] == 1
        assert e[65:69] == (100).to_bytes(4, "big")
        assert e[69:169] == vk
        assert e[169:173] == (80).to_bytes(4, "big")
        assert e[173:253] == proof

    def test_groth16_registered_uses_32_byte_vk_handle(self) -> None:
        vk_id = bytes([0xCC] * 32)
        proof = b"P" * 50
        e = build_proof_submission_extras_groth16_registered(
            bytes(32), bytes(32), vk_id=vk_id, proof_bytes=proof
        )
        # 1 + 32 + 32 + 4 + 32 + 4 + 50
        assert len(e) == 155
        assert e[0] == 3  # PROOF_TYPE_GROTH16_REGISTERED
        assert e[65:69] == (32).to_bytes(4, "big")  # vk_len is fixed at 32 in registered mode
        assert e[69:101] == vk_id


class TestSubscription:
    def test_create_layout(self) -> None:
        producer = bytes([0x55] * 32)
        e = build_subscription_create_extras(
            producer,
            covered_signal_type=AiSignalType.PREDICTION,
            rate_per_block=10,
            duration_blocks=1000,
        )
        assert len(e) == 49
        assert e[0:32] == producer
        assert e[32] == int(AiSignalType.PREDICTION)
        assert e[33:41] == (10).to_bytes(8, "big")
        assert e[41:49] == (1000).to_bytes(8, "big")

    def test_create_rejects_below_min_duration(self) -> None:
        with pytest.raises(ValueError, match="MIN_SUBSCRIPTION_DURATION"):
            build_subscription_create_extras(
                bytes(32), covered_signal_type=0, rate_per_block=1, duration_blocks=50
            )

    def test_cancel_layout(self) -> None:
        sub_id = bytes([0x66] * 32)
        e = build_subscription_cancel_extras(sub_id)
        assert e == sub_id


class TestServiceAttestation:
    def test_layout(self) -> None:
        from novai_sdk import PaymentAttestationStatus

        psh = bytes([0x77] * 32)
        payee = bytes([0x88] * 32)
        e = build_service_attestation_extras(psh, payee, PaymentAttestationStatus.FAILED)
        assert len(e) == 65
        assert e[0:32] == psh
        assert e[32:64] == payee
        assert e[64] == 1  # FAILED


class TestSlaAccept:
    def test_layout(self) -> None:
        sla = bytes([0x99] * 32)
        buyer = bytes([0xAA] * 32)
        e = build_sla_accept_extras(sla, buyer)
        assert len(e) == 64
        assert e[0:32] == sla
        assert e[32:64] == buyer

    def test_derive_signal_hash_is_deterministic(self) -> None:
        sla = bytes([0x99] * 32)
        buyer = bytes([0xAA] * 32)
        h1 = derive_sla_accept_signal_hash(sla, buyer)
        h2 = derive_sla_accept_signal_hash(sla, buyer)
        assert h1 == h2
        assert len(h1) == 32

    def test_derive_signal_hash_changes_with_inputs(self) -> None:
        base = derive_sla_accept_signal_hash(bytes(32), bytes(32))
        assert base != derive_sla_accept_signal_hash(bytes([1]) + bytes(31), bytes(32))
        assert base != derive_sla_accept_signal_hash(bytes(32), bytes([1]) + bytes(31))


class TestChannelAccept:
    def test_layout(self) -> None:
        cid = bytes([0xBB] * 32)
        pa = bytes([0xCC] * 32)
        e = build_channel_accept_extras(cid, pa)
        assert len(e) == 64
        assert e[0:32] == cid
        assert e[32:64] == pa


class TestChannelFinalize:
    def test_layout(self) -> None:
        cid = bytes([0xDD] * 32)
        pa = bytes([0xEE] * 32)
        e = build_channel_finalize_extras(cid, pa)
        assert len(e) == 64


class TestChannelClose:
    def test_layout(self) -> None:
        cid = bytes([0x10] * 32)
        pa = bytes([0x20] * 32)
        sig_a = bytes([0x30] * 64)
        sig_b = bytes([0x40] * 64)
        e = build_channel_close_extras(
            cid,
            pa,
            nonce=7,
            balance_a=1000,
            balance_b=500,
            is_final=True,
            sig_a=sig_a,
            sig_b=sig_b,
        )
        assert len(e) == 233
        assert e[0:32] == cid
        assert e[32:64] == pa
        assert e[64:72] == (7).to_bytes(8, "big")
        assert e[72:88] == (1000).to_bytes(16, "big")
        assert e[88:104] == (500).to_bytes(16, "big")
        assert e[104] == 1  # is_final True
        assert e[105:169] == sig_a
        assert e[169:233] == sig_b

    def test_is_final_false_encodes_zero(self) -> None:
        e = build_channel_close_extras(
            bytes(32),
            bytes(32),
            nonce=0,
            balance_a=0,
            balance_b=0,
            is_final=False,
            sig_a=bytes(64),
            sig_b=bytes(64),
        )
        assert e[104] == 0
