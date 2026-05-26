"""Tests for novai_sdk.tx.* (11 tx payload builders, exact byte layouts)."""

from __future__ import annotations

import pytest

from novai_sdk import (
    AutonomyMode,
    Capabilities,
    MemoryObjectType,
)
from novai_sdk.tx import (
    build_create_memory_payload,
    build_credit_entity_payload,
    build_delete_memory_payload,
    build_entity_upgrade_payload,
    build_execute_proposal_payload,
    build_register_entity_payload,
    build_register_with_key_payload,
    build_signal_commitment_payload,
    build_submit_proposal_payload,
    build_transfer_payload,
    build_update_memory_payload,
)


class TestTransfer:
    def test_layout(self) -> None:
        to = bytes(range(32))
        p = build_transfer_payload(to, amount=0x0102_0304_0506_0708)
        assert len(p) == 41
        assert p[0] == 1
        assert p[1:33] == to
        assert p[33:41] == (0x0102_0304_0506_0708).to_bytes(8, "big")

    def test_accepts_hex_string(self) -> None:
        addr = "aa" * 32
        p = build_transfer_payload(addr, amount=1)
        assert len(p) == 41
        assert p[1:33] == bytes([0xAA] * 32)

    def test_rejects_zero_amount(self) -> None:
        with pytest.raises(ValueError):
            build_transfer_payload(bytes(32), amount=0)


class TestRegisterAiEntity:
    def test_layout(self) -> None:
        code = bytes(range(32))
        p = build_register_entity_payload(
            code_hash=code,
            autonomy_mode=AutonomyMode.GATED,
            capabilities=Capabilities.gated(),
            initial_balance=0xDEAD_BEEF_CAFE_BABE_F00D_F00D,
        )
        assert len(p) == 51
        assert p[0] == 8
        assert p[1:33] == code
        assert p[33] == int(AutonomyMode.GATED)
        assert p[34] == Capabilities.gated().to_byte()
        assert p[35:51] == (0xDEAD_BEEF_CAFE_BABE_F00D_F00D).to_bytes(16, "big")

    def test_balance_zero_is_legal(self) -> None:
        p = build_register_entity_payload(
            code_hash=bytes(32),
            autonomy_mode=AutonomyMode.ADVISORY,
            capabilities=Capabilities.read_only(),
            initial_balance=0,
        )
        assert len(p) == 51


class TestCreditAiEntity:
    def test_layout(self) -> None:
        eid = bytes(range(32))
        p = build_credit_entity_payload(eid, amount=1_000_000)
        assert len(p) == 49
        assert p[0] == 9
        assert p[1:33] == eid
        assert p[33:49] == (1_000_000).to_bytes(16, "big")

    def test_rejects_zero(self) -> None:
        with pytest.raises(ValueError):
            build_credit_entity_payload(bytes(32), amount=0)


class TestRegisterWithKey:
    def test_layout(self) -> None:
        code = bytes(range(32))
        pk = bytes(range(32, 64))
        p = build_register_with_key_payload(
            code_hash=code,
            entity_pubkey=pk,
            autonomy_mode=AutonomyMode.AUTONOMOUS,
            capabilities=Capabilities.oracle(),
            initial_balance=42,
        )
        assert len(p) == 83
        assert p[0] == 10
        assert p[1:33] == code
        assert p[33:65] == pk
        assert p[65] == int(AutonomyMode.AUTONOMOUS)
        assert p[66] == Capabilities.oracle().to_byte()
        assert p[67:83] == (42).to_bytes(16, "big")


class TestEntityUpgrade:
    def test_layout_with_reason(self) -> None:
        eid = bytes([0x11] * 32)
        new_code = bytes([0x22] * 32)
        reason = bytes([0x33] * 32)
        p = build_entity_upgrade_payload(eid, new_code, reason)
        assert len(p) == 97
        assert p[0] == 11
        assert p[1:33] == eid
        assert p[33:65] == new_code
        assert p[65:97] == reason

    def test_layout_without_reason_zero_fills(self) -> None:
        eid = bytes(range(32))
        new_code = bytes(range(32, 64))
        p = build_entity_upgrade_payload(eid, new_code)
        assert len(p) == 97
        assert p[65:97] == bytes(32)


class TestMemoryCRUD:
    def test_create_layout(self) -> None:
        data = b"hello"
        p = build_create_memory_payload(MemoryObjectType.RATING, data)
        assert len(p) == 6 + 5
        assert p[0] == 3
        assert p[1] == int(MemoryObjectType.RATING)
        assert p[2:6] == (5).to_bytes(4, "big")  # data_len BE!
        assert p[6:11] == b"hello"

    def test_update_layout(self) -> None:
        oid = bytes(range(32))
        p = build_update_memory_payload(oid, b"xy")
        assert len(p) == 37 + 2
        assert p[0] == 4
        assert p[1:33] == oid
        assert p[33:37] == (2).to_bytes(4, "big")
        assert p[37:39] == b"xy"

    def test_delete_layout(self) -> None:
        oid = bytes(range(32))
        p = build_delete_memory_payload(oid)
        assert len(p) == 33
        assert p[0] == 5
        assert p[1:33] == oid

    def test_create_rejects_oversized(self) -> None:
        with pytest.raises(ValueError):
            build_create_memory_payload(MemoryObjectType.RATING, bytes(70_000))


class TestGovernance:
    def test_submit_proposal_layout(self) -> None:
        gate = bytes(range(32))
        data = b"proposal-body"
        p = build_submit_proposal_payload(proposal_type=7, gate_id=gate, proposal_data=data)
        assert len(p) == 38 + len(data)
        assert p[0] == 6
        assert p[1] == 7
        assert p[2:34] == gate
        assert p[34:38] == len(data).to_bytes(4, "big")
        assert p[38:] == data

    def test_execute_proposal_layout(self) -> None:
        pid = bytes(range(32))
        p = build_execute_proposal_payload(pid)
        assert len(p) == 33
        assert p[0] == 7
        assert p[1:33] == pid


class TestSignalCommitmentEnvelope:
    def test_envelope_layout_no_extras(self) -> None:
        from novai_sdk import AiSignalType

        sh = bytes(range(32))
        issuer = bytes(range(32, 64))
        p = build_signal_commitment_payload(sh, AiSignalType.ANOMALY, issuer)
        assert len(p) == 66
        assert p[0] == 2
        assert p[1:33] == sh
        assert p[33] == 0  # ANOMALY = 0
        assert p[34:66] == issuer

    def test_envelope_with_extras(self) -> None:
        from novai_sdk import AiSignalType

        sh = bytes(32)
        issuer = bytes(32)
        extras = b"\xAB" * 5
        p = build_signal_commitment_payload(sh, AiSignalType.PAYMENT_REQUEST, issuer, extras=extras)
        assert len(p) == 66 + 5
        assert p[33] == int(AiSignalType.PAYMENT_REQUEST)
        assert p[66:71] == extras
