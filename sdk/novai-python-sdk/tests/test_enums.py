"""Tests for novai_sdk.enums (discriminant byte stability)."""

from __future__ import annotations

from novai_sdk import (
    AiSignalType,
    AutonomyMode,
    ChannelStatus,
    MemoryObjectType,
    PaymentAttestationStatus,
    PaymentConditionKind,
    ProofType,
    ServiceCategory,
    ServiceDescriptorStatus,
    SlaStatus,
    TxPayloadType,
    TxVersion,
)


def test_tx_version_v1_is_one() -> None:
    assert int(TxVersion.V1) == 1


def test_tx_payload_type_bytes() -> None:
    assert int(TxPayloadType.TRANSFER) == 1
    assert int(TxPayloadType.SIGNAL_COMMITMENT) == 2
    assert int(TxPayloadType.CREATE_MEMORY) == 3
    assert int(TxPayloadType.UPDATE_MEMORY) == 4
    assert int(TxPayloadType.DELETE_MEMORY) == 5
    assert int(TxPayloadType.SUBMIT_PROPOSAL) == 6
    assert int(TxPayloadType.EXECUTE_PROPOSAL) == 7
    assert int(TxPayloadType.REGISTER_AI_ENTITY) == 8
    assert int(TxPayloadType.CREDIT_AI_ENTITY) == 9
    assert int(TxPayloadType.REGISTER_AI_ENTITY_WITH_KEY) == 10
    assert int(TxPayloadType.ENTITY_UPGRADE) == 11


def test_autonomy_mode_bytes() -> None:
    assert int(AutonomyMode.ADVISORY) == 0
    assert int(AutonomyMode.GATED) == 1
    assert int(AutonomyMode.AUTONOMOUS) == 2


def test_signal_type_bytes_cover_0_to_22() -> None:
    """The 23-signal universe (Weeks 1-36) must span exactly 0..=22."""
    expected = {
        0: AiSignalType.ANOMALY,
        1: AiSignalType.OPTIMIZATION,
        2: AiSignalType.PREDICTION,
        3: AiSignalType.RISK_SCORE,
        4: AiSignalType.AUDIT_REPORT,
        5: AiSignalType.SPAM_RISK,
        6: AiSignalType.CONGESTION_FORECAST,
        7: AiSignalType.REPUTATION_UPDATE,
        8: AiSignalType.SIGNAL_PURCHASE,
        9: AiSignalType.STAKE_DEPOSIT,
        10: AiSignalType.STAKE_WITHDRAW,
        11: AiSignalType.STAKE_SLASH,
        12: AiSignalType.COMPOSITION_CHECK,
        13: AiSignalType.PROOF_SUBMISSION,
        14: AiSignalType.SUBSCRIPTION_CREATE,
        15: AiSignalType.SUBSCRIPTION_CANCEL,
        16: AiSignalType.PAYMENT_REQUEST,
        17: AiSignalType.SERVICE_ATTESTATION,
        18: AiSignalType.SLA_ACCEPT,
        19: AiSignalType.CHANNEL_ACCEPT,
        20: AiSignalType.CHANNEL_CLOSE,
        21: AiSignalType.CHANNEL_FINALIZE,
        22: AiSignalType.ORACLE_ANCHOR,
    }
    for byte_value, member in expected.items():
        assert int(member) == byte_value


def test_memory_object_type_bytes_cover_0_to_15() -> None:
    expected = {
        0: MemoryObjectType.CHAIN_SUMMARY,
        9: MemoryObjectType.VERIFICATION_RECORD,
        10: MemoryObjectType.DELEGATION_GRANT,
        11: MemoryObjectType.SUBSCRIPTION,
        12: MemoryObjectType.SERVICE_DESCRIPTOR,
        13: MemoryObjectType.VK_REGISTRATION,
        14: MemoryObjectType.SLA_AGREEMENT,
        15: MemoryObjectType.PAYMENT_CHANNEL,
    }
    for byte_value, member in expected.items():
        assert int(member) == byte_value


def test_payment_attestation_status() -> None:
    assert int(PaymentAttestationStatus.DELIVERED) == 0
    assert int(PaymentAttestationStatus.FAILED) == 1


def test_payment_condition_kind_bytes() -> None:
    """Week 36 condition kinds occupy bytes 1..=4."""
    assert int(PaymentConditionKind.ANCHOR_EXISTS) == 1
    assert int(PaymentConditionKind.ANCHOR_DATA_HASH_EQUALS) == 2
    assert int(PaymentConditionKind.ANCHOR_TAG_EQUALS) == 3
    assert int(PaymentConditionKind.ANCHOR_NOT_EXPIRED) == 4


def test_proof_type_bytes() -> None:
    assert int(ProofType.STUB) == 0
    assert int(ProofType.GROTH16) == 1
    assert int(ProofType.GROTH16_REGISTERED) == 3


def test_service_category_bytes() -> None:
    assert int(ServiceCategory.GENERIC) == 0
    assert int(ServiceCategory.INFERENCE) == 2
    assert int(ServiceCategory.GATEWAY) == 9


def test_service_descriptor_status_bytes() -> None:
    assert int(ServiceDescriptorStatus.ACTIVE) == 0
    assert int(ServiceDescriptorStatus.DEPRECATED) == 2


def test_sla_status_bytes() -> None:
    assert int(SlaStatus.PROPOSED) == 0
    assert int(SlaStatus.ACTIVE) == 1
    assert int(SlaStatus.VIOLATED) == 3


def test_channel_status_bytes() -> None:
    assert int(ChannelStatus.PROPOSED) == 0
    assert int(ChannelStatus.OPEN) == 1
    assert int(ChannelStatus.CLOSING) == 2
