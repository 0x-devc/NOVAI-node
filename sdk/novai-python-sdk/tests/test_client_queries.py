"""Tests for the typed query methods on AsyncNOVAIClient (Phase 3)."""

from __future__ import annotations

import pytest
from aioresponses import aioresponses

from novai_sdk import AsyncNOVAIClient, ServiceCategory

ENDPOINT = "http://localhost:3030"


@pytest.fixture
def mock_rpc() -> aioresponses:
    with aioresponses() as m:
        yield m


def _ok(payload: object) -> dict[str, object]:
    return {"jsonrpc": "2.0", "id": 1, "result": payload}


@pytest.mark.asyncio
async def test_get_block_by_height_parses_header(mock_rpc: aioresponses) -> None:
    mock_rpc.post(
        ENDPOINT,
        payload=_ok(
            {
                "height": 100,
                "round": 0,
                "block_hash": "aa" * 32,
                "parent_hash": "bb" * 32,
                "state_root": "cc" * 32,
                "tx_count": 5,
            }
        ),
    )
    async with AsyncNOVAIClient(ENDPOINT) as client:
        block = await client.get_block_by_height(100)
    assert block is not None
    assert block.height == 100
    assert block.block_hash == "aa" * 32
    assert block.tx_count == 5


@pytest.mark.asyncio
async def test_get_block_by_height_returns_none_for_null(mock_rpc: aioresponses) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok(None))
    async with AsyncNOVAIClient(ENDPOINT) as client:
        block = await client.get_block_by_height(99999)
    assert block is None


@pytest.mark.asyncio
async def test_get_ai_entity_parses_full_entity(mock_rpc: aioresponses) -> None:
    mock_rpc.post(
        ENDPOINT,
        payload=_ok(
            {
                "entity": {
                    "id": "11" * 32,
                    "code_hash": "22" * 32,
                    "creator": "33" * 32,
                    "autonomy_mode": 1,
                    "capabilities": 0b0100_0111,
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
                    "upgrade_count": 1,
                    "last_upgrade_height": 250,
                }
            }
        ),
    )
    async with AsyncNOVAIClient(ENDPOINT) as client:
        entity = await client.get_ai_entity(bytes([0x11] * 32))
    assert entity is not None
    assert entity.id == "11" * 32
    assert entity.economic_balance == "1000000"
    assert entity.is_active is True
    assert entity.upgrade_count == 1


@pytest.mark.asyncio
async def test_get_ai_entity_returns_none_when_entity_null(
    mock_rpc: aioresponses,
) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok({"entity": None}))
    async with AsyncNOVAIClient(ENDPOINT) as client:
        entity = await client.get_ai_entity(bytes(32))
    assert entity is None


@pytest.mark.asyncio
async def test_get_signals_by_height_parses_list(mock_rpc: aioresponses) -> None:
    mock_rpc.post(
        ENDPOINT,
        payload=_ok(
            {
                "signals": [
                    {
                        "commitment_hash": "aa" * 32,
                        "signal_type": 22,
                        "height": 100,
                        "issuer": "bb" * 32,
                    }
                ]
            }
        ),
    )
    async with AsyncNOVAIClient(ENDPOINT) as client:
        signals = await client.get_signals_by_height(100)
    assert len(signals) == 1
    assert signals[0].signal_type == 22
    assert signals[0].issuer == "bb" * 32


@pytest.mark.asyncio
async def test_get_payments_by_entity_parses_splits_and_condition(
    mock_rpc: aioresponses,
) -> None:
    mock_rpc.post(
        ENDPOINT,
        payload=_ok(
            {
                "payments": [
                    {
                        "payer": "11" * 32,
                        "payee": "22" * 32,
                        "amount": "5000",
                        "service_descriptor_hash": "33" * 32,
                        "request_hash": "44" * 32,
                        "payment_height": 200,
                        "max_block_height": 500,
                        "attested_status": "delivered",
                        "attested_height": 210,
                        "splits": [
                            {
                                "recipient_entity_id": "22" * 32,
                                "basis_points": 7000,
                                "credited_amount": "3500",
                            },
                            {
                                "recipient_entity_id": "55" * 32,
                                "basis_points": 3000,
                                "credited_amount": "1500",
                            },
                        ],
                        "condition": {
                            "kind": "anchor_exists",
                            "anchor_signal_hash": "66" * 32,
                            "expected_data_hash": None,
                            "expected_tag": None,
                            "expected_tag_hex": None,
                        },
                    }
                ]
            }
        ),
    )
    async with AsyncNOVAIClient(ENDPOINT) as client:
        payments = await client.get_payments_by_entity(bytes(32), "payee", 0, 1000)
    assert len(payments) == 1
    p = payments[0]
    assert p.amount == "5000"
    assert p.attested_status == "delivered"
    assert p.splits is not None
    assert len(p.splits) == 2
    assert p.splits[0].basis_points == 7000
    assert p.condition is not None
    assert p.condition.kind == "anchor_exists"


@pytest.mark.asyncio
async def test_get_payments_by_entity_handles_legacy_null_splits(
    mock_rpc: aioresponses,
) -> None:
    mock_rpc.post(
        ENDPOINT,
        payload=_ok(
            {
                "payments": [
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
                ]
            }
        ),
    )
    async with AsyncNOVAIClient(ENDPOINT) as client:
        payments = await client.get_payments_by_entity(bytes(32), "payer", 0, 100)
    assert payments[0].splits is None
    assert payments[0].condition is None


@pytest.mark.asyncio
async def test_get_payments_rejects_invalid_role() -> None:
    async with AsyncNOVAIClient(ENDPOINT) as client:
        with pytest.raises(ValueError, match="role must be"):
            await client.get_payments_by_entity(bytes(32), "owner", 0, 100)


@pytest.mark.asyncio
async def test_discover_services_by_enum(mock_rpc: aioresponses) -> None:
    mock_rpc.post(
        ENDPOINT,
        payload=_ok(
            {
                "descriptors": [
                    {
                        "object_id": "11" * 32,
                        "owner_entity": "22" * 32,
                        "created_at": 100,
                        "updated_at": 200,
                        "version": 1,
                        "service_name_hash": "33" * 32,
                        "service_url_hash": "44" * 32,
                        "description_hash": "55" * 32,
                        "category": 2,
                        "category_label": "inference",
                        "price_per_call": "100",
                        "subscription_rate_per_block": "1",
                        "min_reputation_score": 50,
                        "min_stake": "1000",
                        "capability_tags": 0,
                        "status": 0,
                        "status_label": "active",
                    }
                ]
            }
        ),
    )
    async with AsyncNOVAIClient(ENDPOINT) as client:
        services = await client.discover_services(ServiceCategory.INFERENCE)
    assert len(services) == 1
    assert services[0].category == 2
    # The convenience .entity_id alias should resolve to owner_entity.
    assert services[0].entity_id == "22" * 32


@pytest.mark.asyncio
async def test_discover_services_by_string_name(mock_rpc: aioresponses) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok({"descriptors": []}))
    async with AsyncNOVAIClient(ENDPOINT) as client:
        services = await client.discover_services("inference")
    assert services == []


@pytest.mark.asyncio
async def test_discover_services_by_kebab_name(mock_rpc: aioresponses) -> None:
    """Service categories with hyphens map through ``ServiceCategory[*]`` correctly."""
    mock_rpc.post(ENDPOINT, payload=_ok({"descriptors": []}))
    async with AsyncNOVAIClient(ENDPOINT) as client:
        await client.discover_services("data-oracle")
    # We just want this to not raise (the param mapping must succeed).


@pytest.mark.asyncio
async def test_discover_services_rejects_unknown_name() -> None:
    async with AsyncNOVAIClient(ENDPOINT) as client:
        with pytest.raises(ValueError, match="unknown service category"):
            await client.discover_services("totally-fake-category")


@pytest.mark.asyncio
async def test_get_oracle_anchor_returns_none_when_anchor_null(
    mock_rpc: aioresponses,
) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok({"anchor": None}))
    async with AsyncNOVAIClient(ENDPOINT) as client:
        anchor = await client.get_oracle_anchor(bytes(32))
    assert anchor is None


@pytest.mark.asyncio
async def test_get_oracle_anchors_by_entity_passes_ts_filters(
    mock_rpc: aioresponses,
) -> None:
    mock_rpc.post(
        ENDPOINT,
        payload=_ok(
            {
                "anchors": [
                    {
                        "issuer_entity_id": "11" * 32,
                        "data_hash": "22" * 32,
                        "external_timestamp": 1735776000,
                        "source_hash": "00" * 32,
                        "expiry_height": 0,
                        "anchor_height": 500,
                        "data_tag": "price/ETH-USD",
                        "data_tag_hex": "70726963652f4554482d555344",
                    }
                ]
            }
        ),
    )
    async with AsyncNOVAIClient(ENDPOINT) as client:
        anchors = await client.get_oracle_anchors_by_entity(
            bytes([1] * 32), 0, 1000, ts_min=1_000_000, ts_max=2_000_000_000
        )
    assert len(anchors) == 1
    assert anchors[0].data_tag == "price/ETH-USD"


@pytest.mark.asyncio
async def test_get_channel_dispute_status(mock_rpc: aioresponses) -> None:
    mock_rpc.post(
        ENDPOINT,
        payload=_ok(
            {
                "found": True,
                "status": 2,
                "status_label": "closing",
                "closing_at_height": 1000,
                "dispute_deadline_height": 1100,
                "current_height": 1050,
                "blocks_remaining": 50,
                "finalize_ready": False,
            }
        ),
    )
    async with AsyncNOVAIClient(ENDPOINT) as client:
        status = await client.get_channel_dispute_status(bytes(32), bytes(32))
    assert status.found is True
    assert status.status_label == "closing"
    assert status.blocks_remaining == 50
    assert status.finalize_ready is False


@pytest.mark.asyncio
async def test_iter_signals_by_issuer_paginates(mock_rpc: aioresponses) -> None:
    """The async iterator should chunk a 25K-block range into three RPC calls."""
    issuer = bytes([1] * 32)
    # Three chunks: [0, 9999], [10000, 19999], [20000, 25000].
    mock_rpc.post(
        ENDPOINT,
        payload=_ok(
            {
                "signals": [
                    {
                        "commitment_hash": "aa" * 32,
                        "signal_type": 0,
                        "height": 5,
                        "issuer": "01" * 32,
                    }
                ]
            }
        ),
    )
    mock_rpc.post(
        ENDPOINT,
        payload=_ok(
            {
                "signals": [
                    {
                        "commitment_hash": "bb" * 32,
                        "signal_type": 0,
                        "height": 15_000,
                        "issuer": "01" * 32,
                    }
                ]
            }
        ),
    )
    mock_rpc.post(
        ENDPOINT,
        payload=_ok(
            {
                "signals": [
                    {
                        "commitment_hash": "cc" * 32,
                        "signal_type": 0,
                        "height": 24_000,
                        "issuer": "01" * 32,
                    }
                ]
            }
        ),
    )

    async with AsyncNOVAIClient(ENDPOINT) as client:
        collected = [s async for s in client.iter_signals_by_issuer(issuer, 0, 25_000)]
    assert len(collected) == 3
    assert collected[0].commitment_hash == "aa" * 32
    assert collected[1].commitment_hash == "bb" * 32
    assert collected[2].commitment_hash == "cc" * 32
