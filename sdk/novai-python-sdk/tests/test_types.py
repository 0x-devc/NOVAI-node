"""Tests for novai_sdk.types (dataclass parsing)."""

from __future__ import annotations

import pytest

from novai_sdk.types import (
    AiEntityInfo,
    BlockHeader,
    MemoryObjectInfo,
    OracleAnchorInfo,
    PaymentRecord,
    SubmissionResult,
)


class TestBlockHeader:
    def test_parses_complete_payload(self) -> None:
        block = BlockHeader.from_json(
            {
                "height": 100,
                "round": 0,
                "block_hash": "aa" * 32,
                "parent_hash": "bb" * 32,
                "state_root": "cc" * 32,
                "tx_count": 5,
            }
        )
        assert block.height == 100
        assert block.tx_count == 5

    def test_raises_on_missing_field(self) -> None:
        with pytest.raises(KeyError):
            BlockHeader.from_json({"height": 1})


class TestAiEntityInfo:
    def test_parses_with_upgrade_fields(self) -> None:
        info = AiEntityInfo.from_json(
            {
                "id": "11" * 32,
                "code_hash": "22" * 32,
                "creator": "33" * 32,
                "autonomy_mode": 1,
                "capabilities": 0xFF,
                "economic_balance": "1000000",
                "nonce": 5,
                "pubkey": "44" * 32,
                "memory_root": "55" * 32,
                "params_root": "66" * 32,
                "registered_at": 100,
                "last_active_at": 200,
                "is_active": True,
                "reputation_score": 75,
                "total_transactions": 10,
                "reputation_events_count": 2,
                "stake_balance": "50000",
                "stake_locked_until": 300,
                "upgrade_count": 3,
                "last_upgrade_height": 1500,
            }
        )
        assert info.upgrade_count == 3
        assert info.last_upgrade_height == 1500

    def test_upgrade_fields_default_to_zero_when_missing(self) -> None:
        """Older nodes may omit upgrade_count / last_upgrade_height fields."""
        info = AiEntityInfo.from_json(
            {
                "id": "11" * 32,
                "code_hash": "22" * 32,
                "creator": "33" * 32,
                "autonomy_mode": 0,
                "capabilities": 0,
                "economic_balance": "0",
                "nonce": 0,
                "pubkey": "44" * 32,
                "memory_root": "55" * 32,
                "params_root": "66" * 32,
                "registered_at": 1,
                "last_active_at": 1,
                "is_active": True,
                "reputation_score": 50,
                "total_transactions": 0,
                "reputation_events_count": 0,
                "stake_balance": "0",
                "stake_locked_until": 0,
            }
        )
        assert info.upgrade_count == 0
        assert info.last_upgrade_height == 0


class TestPaymentRecord:
    def test_legacy_payment_has_no_splits_no_condition(self) -> None:
        p = PaymentRecord.from_json(
            {
                "payer": "11" * 32,
                "payee": "22" * 32,
                "amount": "100",
                "service_descriptor_hash": "33" * 32,
                "request_hash": "44" * 32,
                "payment_height": 5,
                "max_block_height": 50,
                "attested_status": None,
                "attested_height": None,
                "splits": None,
                "condition": None,
            }
        )
        assert p.splits is None
        assert p.condition is None
        assert p.attested_status is None

    def test_splits_parse_into_typed_objects(self) -> None:
        p = PaymentRecord.from_json(
            {
                "payer": "11" * 32,
                "payee": "22" * 32,
                "amount": "1000",
                "service_descriptor_hash": "33" * 32,
                "request_hash": "44" * 32,
                "payment_height": 5,
                "max_block_height": 50,
                "attested_status": "delivered",
                "attested_height": 10,
                "splits": [
                    {
                        "recipient_entity_id": "22" * 32,
                        "basis_points": 7000,
                        "credited_amount": "700",
                    }
                ],
                "condition": None,
            }
        )
        assert p.splits is not None
        assert len(p.splits) == 1
        assert p.splits[0].basis_points == 7000


class TestOracleAnchorInfo:
    def test_parses_anchor_row(self) -> None:
        a = OracleAnchorInfo.from_json(
            {
                "issuer_entity_id": "11" * 32,
                "data_hash": "22" * 32,
                "external_timestamp": 1735776000,
                "source_hash": "00" * 32,
                "expiry_height": 0,
                "anchor_height": 100,
                "data_tag": "price/ETH-USD",
                "data_tag_hex": "70726963652f4554482d555344",
            }
        )
        assert a.data_tag == "price/ETH-USD"
        assert a.external_timestamp == 1735776000


class TestMemoryObjectInfo:
    def test_data_property_decodes_hex(self) -> None:
        m = MemoryObjectInfo.from_json(
            {
                "object_id": "11" * 32,
                "object_type": 12,
                "owner_entity": "22" * 32,
                "created_at": 1,
                "updated_at": 1,
                "data_size": 3,
                "data": "010203",
            }
        )
        assert m.data == b"\x01\x02\x03"


class TestSubmissionResult:
    def test_dataclass_holds_optional_fields(self) -> None:
        r = SubmissionResult(txid="ab" * 32, entity_id="11" * 32, signal_hash=None)
        assert r.txid == "ab" * 32
        assert r.entity_id == "11" * 32
        assert r.signal_hash is None
