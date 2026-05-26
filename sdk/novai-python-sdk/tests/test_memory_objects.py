"""Tests for novai_sdk.memory_objects (data block encoders for types 12-15)."""

from __future__ import annotations

import pytest

from novai_sdk.enums import (
    ChannelStatus,
    ProofType,
    ServiceCategory,
    ServiceDescriptorStatus,
    SlaStatus,
)
from novai_sdk.memory_objects import (
    PAYMENT_CHANNEL_SIZE,
    SERVICE_DESCRIPTOR_SIZE,
    SLA_AGREEMENT_SIZE,
    VK_REGISTRATION_HEADER_SIZE,
    encode_payment_channel,
    encode_service_descriptor,
    encode_sla_agreement,
    encode_vk_registration,
)


class TestServiceDescriptor:
    def test_length_144(self) -> None:
        data = encode_service_descriptor(
            service_name_hash=bytes(32),
            service_url_hash=bytes(32),
            description_hash=bytes(32),
            category=ServiceCategory.INFERENCE,
            price_per_call=100,
            subscription_rate_per_block=1,
            min_reputation_score=50,
            min_stake=1_000_000,
            capability_tags=0x12345678,
        )
        assert len(data) == SERVICE_DESCRIPTOR_SIZE

    def test_layout(self) -> None:
        data = encode_service_descriptor(
            service_name_hash=bytes([0x11] * 32),
            service_url_hash=bytes([0x22] * 32),
            description_hash=bytes([0x33] * 32),
            category=ServiceCategory.INFERENCE,
            price_per_call=0x0102_0304_0506_0708,
            subscription_rate_per_block=42,
            min_reputation_score=60,
            min_stake=10**18,
            capability_tags=0xAABBCCDD,
            status=ServiceDescriptorStatus.ACTIVE,
        )
        assert data[0] == 1  # version
        assert data[1:33] == bytes([0x11] * 32)
        assert data[33:65] == bytes([0x22] * 32)
        assert data[65:97] == bytes([0x33] * 32)
        assert data[97] == int(ServiceCategory.INFERENCE)
        assert data[98:106] == (0x0102_0304_0506_0708).to_bytes(8, "big")
        assert data[106:114] == (42).to_bytes(8, "big")
        assert data[114:116] == (60).to_bytes(2, "big")
        assert data[116:132] == (10**18).to_bytes(16, "big")
        assert data[132:136] == (0xAABBCCDD).to_bytes(4, "big")
        assert data[136] == 0  # ACTIVE
        assert data[137:144] == bytes(7)  # reserved all-zero

    def test_rejects_reputation_above_100(self) -> None:
        with pytest.raises(ValueError):
            encode_service_descriptor(
                service_name_hash=bytes(32),
                service_url_hash=bytes(32),
                description_hash=bytes(32),
                category=ServiceCategory.GENERIC,
                price_per_call=0,
                subscription_rate_per_block=0,
                min_reputation_score=200,
                min_stake=0,
                capability_tags=0,
            )


class TestSlaAgreement:
    def test_length_210(self) -> None:
        data = encode_sla_agreement(
            buyer_entity_id=bytes([1] * 32),
            seller_entity_id=bytes([2] * 32),
            service_descriptor_hash=bytes(32),
            start_height=100,
            end_height=10_000,
            violation_threshold=3,
            slash_amount=1_000_000,
            price_per_call=10,
        )
        assert len(data) == SLA_AGREEMENT_SIZE

    def test_layout(self) -> None:
        buyer = bytes([0xAA] * 32)
        seller = bytes([0xBB] * 32)
        sd_hash = bytes([0xCC] * 32)
        data = encode_sla_agreement(
            buyer_entity_id=buyer,
            seller_entity_id=seller,
            service_descriptor_hash=sd_hash,
            start_height=1000,
            end_height=2000,
            violation_threshold=5,
            slash_amount=10**12,
            price_per_call=50,
            max_response_time_blocks=200,
            min_uptime_bps=9500,
            min_delivery_success_bps=9800,
        )
        assert data[0] == 1  # version
        assert data[1:33] == buyer
        assert data[33:65] == seller
        assert data[65:97] == sd_hash
        assert data[97] == int(SlaStatus.PROPOSED)
        assert data[98:106] == (0).to_bytes(8, "big")  # created_at_height
        assert data[106:114] == (0).to_bytes(8, "big")  # accepted_at_height
        assert data[114:122] == (1000).to_bytes(8, "big")
        assert data[122:130] == (2000).to_bytes(8, "big")
        assert data[130:134] == (0).to_bytes(4, "big")  # violation_count
        assert data[134:138] == (5).to_bytes(4, "big")
        assert data[138:142] == (200).to_bytes(4, "big")
        assert data[142:144] == (9500).to_bytes(2, "big")
        assert data[144:146] == (9800).to_bytes(2, "big")
        assert data[146:154] == (50).to_bytes(8, "big")
        assert data[154:170] == (10**12).to_bytes(16, "big")
        assert data[170:178] == (0).to_bytes(8, "big")  # terminated_at_height
        assert data[178:194] == (0).to_bytes(16, "big")  # slashed_amount
        assert data[194:210] == bytes(16)  # reserved

    def test_rejects_buyer_eq_seller(self) -> None:
        with pytest.raises(ValueError, match="must differ"):
            encode_sla_agreement(
                buyer_entity_id=bytes([1] * 32),
                seller_entity_id=bytes([1] * 32),
                service_descriptor_hash=bytes(32),
                start_height=0,
                end_height=100,
                violation_threshold=1,
                slash_amount=1,
                price_per_call=0,
            )

    def test_rejects_zero_threshold(self) -> None:
        with pytest.raises(ValueError, match="violation_threshold"):
            encode_sla_agreement(
                buyer_entity_id=bytes([1] * 32),
                seller_entity_id=bytes([2] * 32),
                service_descriptor_hash=bytes(32),
                start_height=0,
                end_height=100,
                violation_threshold=0,
                slash_amount=1,
                price_per_call=0,
            )

    def test_rejects_zero_slash(self) -> None:
        with pytest.raises(ValueError, match="slash_amount"):
            encode_sla_agreement(
                buyer_entity_id=bytes([1] * 32),
                seller_entity_id=bytes([2] * 32),
                service_descriptor_hash=bytes(32),
                start_height=0,
                end_height=100,
                violation_threshold=1,
                slash_amount=0,
                price_per_call=0,
            )

    def test_rejects_start_after_end(self) -> None:
        with pytest.raises(ValueError, match="must be <"):
            encode_sla_agreement(
                buyer_entity_id=bytes([1] * 32),
                seller_entity_id=bytes([2] * 32),
                service_descriptor_hash=bytes(32),
                start_height=200,
                end_height=100,
                violation_threshold=1,
                slash_amount=1,
                price_per_call=0,
            )


class TestPaymentChannel:
    def test_length_222(self) -> None:
        data = encode_payment_channel(
            party_a_entity_id=bytes([1] * 32),
            party_b_entity_id=bytes([2] * 32),
            deposit_a=1_000_000,
            dispute_window_blocks=100,
        )
        assert len(data) == PAYMENT_CHANNEL_SIZE

    def test_layout(self) -> None:
        pa = bytes([0x11] * 32)
        pb = bytes([0x22] * 32)
        sla = bytes([0x33] * 32)
        data = encode_payment_channel(
            party_a_entity_id=pa,
            party_b_entity_id=pb,
            deposit_a=5000,
            dispute_window_blocks=500,
            sla_object_id=sla,
        )
        assert data[0] == 1  # version
        assert data[1:33] == pa
        assert data[33:65] == pb
        assert data[65:97] == sla
        assert data[97] == int(ChannelStatus.PROPOSED)
        assert data[98:114] == (5000).to_bytes(16, "big")  # deposit_a
        assert data[114:130] == (0).to_bytes(16, "big")  # deposit_b (0 at create)
        assert data[130:146] == (5000).to_bytes(16, "big")  # balance_a == deposit_a
        assert data[146:162] == (0).to_bytes(16, "big")  # balance_b (0 at create)
        assert data[162:170] == (0).to_bytes(8, "big")  # nonce
        assert data[202:206] == (500).to_bytes(4, "big")  # dispute_window
        assert data[206:222] == bytes(16)  # reserved

    def test_default_sla_is_zero(self) -> None:
        data = encode_payment_channel(
            party_a_entity_id=bytes([1] * 32),
            party_b_entity_id=bytes([2] * 32),
            deposit_a=1,
            dispute_window_blocks=100,
        )
        assert data[65:97] == bytes(32)

    def test_rejects_party_a_eq_party_b(self) -> None:
        with pytest.raises(ValueError):
            encode_payment_channel(
                party_a_entity_id=bytes([1] * 32),
                party_b_entity_id=bytes([1] * 32),
                deposit_a=1,
                dispute_window_blocks=100,
            )

    def test_rejects_window_below_min(self) -> None:
        with pytest.raises(ValueError):
            encode_payment_channel(
                party_a_entity_id=bytes([1] * 32),
                party_b_entity_id=bytes([2] * 32),
                deposit_a=1,
                dispute_window_blocks=50,
            )

    def test_rejects_window_above_max(self) -> None:
        with pytest.raises(ValueError):
            encode_payment_channel(
                party_a_entity_id=bytes([1] * 32),
                party_b_entity_id=bytes([2] * 32),
                deposit_a=1,
                dispute_window_blocks=20_000,
            )


class TestVkRegistration:
    def test_header_size_39(self) -> None:
        data = encode_vk_registration(
            proof_type=ProofType.GROTH16,
            code_hash=bytes(32),
            label=b"",
            vk_bytes=b"\xAB",
        )
        # 39 header + 0 label + 1 vk = 40
        assert len(data) == VK_REGISTRATION_HEADER_SIZE + 1

    def test_layout(self) -> None:
        code = bytes([0x44] * 32)
        vk = b"\x01\x02\x03\x04"
        data = encode_vk_registration(
            proof_type=ProofType.GROTH16,
            code_hash=code,
            label="my-proof",
            vk_bytes=vk,
        )
        assert data[0] == 1  # VK_REGISTRATION_VERSION
        assert data[1] == int(ProofType.GROTH16)
        assert data[2:34] == code
        assert data[34] == 8  # label_len
        assert data[35:39] == (4).to_bytes(4, "big")  # vk_len BE
        assert data[39:47] == b"my-proof"
        assert data[47:51] == vk

    def test_rejects_empty_vk(self) -> None:
        with pytest.raises(ValueError):
            encode_vk_registration(
                proof_type=ProofType.GROTH16,
                code_hash=bytes(32),
                label=b"",
                vk_bytes=b"",
            )

    def test_rejects_oversized_label(self) -> None:
        with pytest.raises(ValueError):
            encode_vk_registration(
                proof_type=ProofType.GROTH16,
                code_hash=bytes(32),
                label=b"x" * 33,
                vk_bytes=b"\xAB",
            )
