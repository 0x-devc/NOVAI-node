"""Tests for the convenience write methods on AsyncNOVAIClient (Phase 3).

Each test mocks the JSON-RPC layer and verifies that the SDK:

1. Queries the sender's nonce first (or skips when ``nonce=`` is provided).
2. Submits a signed TxV1 whose payload bytes start with the expected
   payload type byte and carry the expected discriminants.
3. Returns a ``SubmissionResult`` with the txid (plus entity_id /
   signal_hash where applicable).
"""

from __future__ import annotations

import pytest
from aioresponses import aioresponses

from novai_sdk import (
    AsyncNOVAIClient,
    AutonomyMode,
    Capabilities,
    Keypair,
    PaymentAttestationStatus,
    PaymentCondition,
    PaymentSplit,
    compute_entity_id,
)
from novai_sdk.codec import decode_tx_v1_signed
from novai_sdk.enums import AiSignalType, MemoryObjectType, TxPayloadType

ENDPOINT = "http://localhost:3030"


@pytest.fixture
def mock_rpc() -> aioresponses:
    with aioresponses() as m:
        yield m


def _ok(payload: object, *, request_id: int = 1) -> dict[str, object]:
    return {"jsonrpc": "2.0", "id": request_id, "result": payload}


def _extract_submitted_tx_hex(mock_rpc: aioresponses) -> str:
    """Pull the most recent novai_submitTransaction request body and return the tx hex."""
    calls = mock_rpc.requests
    for (method, _url), entries in calls.items():
        if method.upper() != "POST":
            continue
        for entry in entries:
            body = entry.kwargs.get("json")
            if body and body.get("method") == "novai_submitTransaction":
                params = body.get("params") or {}
                if isinstance(params, dict) and "tx" in params:
                    return str(params["tx"])
    raise AssertionError("no novai_submitTransaction call recorded")


@pytest.fixture
def alice() -> Keypair:
    return Keypair.from_seed(bytes([0x11] * 32))


@pytest.fixture
def bob() -> Keypair:
    return Keypair.from_seed(bytes([0x22] * 32))


@pytest.mark.asyncio
async def test_transfer_builds_correct_payload(
    mock_rpc: aioresponses, alice: Keypair
) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok({"nonce": 7}))
    mock_rpc.post(ENDPOINT, payload=_ok({"txid": "ab" * 32}))

    async with AsyncNOVAIClient(ENDPOINT) as client:
        result = await client.transfer(alice, bytes([0x99] * 32), amount=5000)

    assert result.txid == "ab" * 32
    tx_hex = _extract_submitted_tx_hex(mock_rpc)
    tx = decode_tx_v1_signed(bytes.fromhex(tx_hex))
    assert tx.from_address == alice.address
    assert tx.nonce == 7
    assert tx.payload[0] == int(TxPayloadType.TRANSFER)
    assert tx.payload[1:33] == bytes([0x99] * 32)
    assert tx.payload[33:41] == (5000).to_bytes(8, "big")


@pytest.mark.asyncio
async def test_register_entity_returns_derived_entity_id(
    mock_rpc: aioresponses, alice: Keypair
) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok({"nonce": 0}))
    mock_rpc.post(ENDPOINT, payload=_ok({"txid": "cd" * 32}))

    code_hash = bytes([0x42] * 32)
    async with AsyncNOVAIClient(ENDPOINT) as client:
        result = await client.register_entity(
            alice,
            code_hash=code_hash,
            capabilities=Capabilities.gated(),
            autonomy_mode=AutonomyMode.GATED,
            initial_balance=1_000_000,
        )

    expected_entity_id = compute_entity_id(code_hash, alice.address)
    assert result.entity_id == expected_entity_id.hex()
    tx_hex = _extract_submitted_tx_hex(mock_rpc)
    tx = decode_tx_v1_signed(bytes.fromhex(tx_hex))
    assert tx.payload[0] == int(TxPayloadType.REGISTER_AI_ENTITY)
    assert tx.payload[1:33] == code_hash


@pytest.mark.asyncio
async def test_register_entity_with_key_layout(
    mock_rpc: aioresponses, alice: Keypair, bob: Keypair
) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok({"nonce": 0}))
    mock_rpc.post(ENDPOINT, payload=_ok({"txid": "ef" * 32}))

    async with AsyncNOVAIClient(ENDPOINT) as client:
        await client.register_entity_with_key(
            alice,
            code_hash=bytes([0x42] * 32),
            entity_pubkey=bob.pubkey,
            capabilities=Capabilities.oracle(),
        )
    tx_hex = _extract_submitted_tx_hex(mock_rpc)
    tx = decode_tx_v1_signed(bytes.fromhex(tx_hex))
    assert tx.payload[0] == int(TxPayloadType.REGISTER_AI_ENTITY_WITH_KEY)
    assert tx.payload[33:65] == bob.pubkey


@pytest.mark.asyncio
async def test_upgrade_entity_layout(
    mock_rpc: aioresponses, alice: Keypair
) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok({"nonce": 1}))
    mock_rpc.post(ENDPOINT, payload=_ok({"txid": "11" * 32}))

    entity_id = bytes([0xAA] * 32)
    new_code = bytes([0xBB] * 32)
    async with AsyncNOVAIClient(ENDPOINT) as client:
        await client.upgrade_entity(alice, entity_id, new_code)
    tx_hex = _extract_submitted_tx_hex(mock_rpc)
    tx = decode_tx_v1_signed(bytes.fromhex(tx_hex))
    assert tx.payload[0] == int(TxPayloadType.ENTITY_UPGRADE)
    assert tx.payload[1:33] == entity_id
    assert tx.payload[33:65] == new_code
    assert tx.payload[65:97] == bytes(32)  # default reason zero


@pytest.mark.asyncio
async def test_pay_basic(mock_rpc: aioresponses, alice: Keypair) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok({"nonce": 2}))
    mock_rpc.post(ENDPOINT, payload=_ok({"txid": "22" * 32}))

    signal_hash = bytes([0x77] * 32)
    payee = bytes([0x88] * 32)
    async with AsyncNOVAIClient(ENDPOINT) as client:
        result = await client.pay(
            alice,
            issuer_entity_id=bytes([0x11] * 32),
            payee=payee,
            amount=5000,
            signal_hash=signal_hash,
            service_descriptor_hash=bytes([0x33] * 32),
            request_hash=bytes([0x44] * 32),
            max_block_height=1_000_000,
        )
    assert result.txid == "22" * 32
    assert result.signal_hash == signal_hash.hex()
    tx_hex = _extract_submitted_tx_hex(mock_rpc)
    tx = decode_tx_v1_signed(bytes.fromhex(tx_hex))
    # Signal commitment envelope.
    assert tx.payload[0] == int(TxPayloadType.SIGNAL_COMMITMENT)
    assert tx.payload[33] == int(AiSignalType.PAYMENT_REQUEST)
    # 66-byte envelope + 112-byte legacy extras = 178 bytes
    assert len(tx.payload) == 178


@pytest.mark.asyncio
async def test_pay_with_splits_and_condition(
    mock_rpc: aioresponses, alice: Keypair
) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok({"nonce": 3}))
    mock_rpc.post(ENDPOINT, payload=_ok({"txid": "33" * 32}))

    payee = bytes([0x88] * 32)
    splits = [
        PaymentSplit(recipient_entity_id=payee, basis_points=6000),
        PaymentSplit(recipient_entity_id=bytes([0xAA] * 32), basis_points=4000),
    ]
    condition = PaymentCondition.anchor_exists(bytes([0x44] * 32))
    async with AsyncNOVAIClient(ENDPOINT) as client:
        await client.pay(
            alice,
            issuer_entity_id=bytes([0x11] * 32),
            payee=payee,
            amount=5000,
            signal_hash=bytes([0x77] * 32),
            service_descriptor_hash=bytes([0x33] * 32),
            request_hash=bytes([0x44] * 32),
            max_block_height=1_000_000,
            splits=splits,
            condition=condition,
        )
    tx_hex = _extract_submitted_tx_hex(mock_rpc)
    tx = decode_tx_v1_signed(bytes.fromhex(tx_hex))
    # Envelope(66) + base(112) + condition(34) + splits-trailer(1 + 2*34 = 69)
    # = 66 + 112 + 34 + 69 = 281
    assert len(tx.payload) == 281
    # Marker at offset 66 + 112 == 178 of the envelope (= offset 112 of extras).
    assert tx.payload[178] == 0xC1


@pytest.mark.asyncio
async def test_attest_payment_layout(mock_rpc: aioresponses, alice: Keypair) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok({"nonce": 1}))
    mock_rpc.post(ENDPOINT, payload=_ok({"txid": "44" * 32}))

    async with AsyncNOVAIClient(ENDPOINT) as client:
        await client.attest_payment(
            alice,
            issuer_entity_id=bytes([0x11] * 32),
            payment_signal_hash=bytes([0x55] * 32),
            payee=bytes([0x88] * 32),
            status=PaymentAttestationStatus.DELIVERED,
            signal_hash=bytes([0x99] * 32),
        )
    tx_hex = _extract_submitted_tx_hex(mock_rpc)
    tx = decode_tx_v1_signed(bytes.fromhex(tx_hex))
    assert tx.payload[33] == int(AiSignalType.SERVICE_ATTESTATION)
    # 66 envelope + 65 extras = 131 bytes
    assert len(tx.payload) == 131
    # Status byte at the end.
    assert tx.payload[130] == 0  # DELIVERED


@pytest.mark.asyncio
async def test_post_oracle_anchor_derives_signal_hash(
    mock_rpc: aioresponses, alice: Keypair
) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok({"nonce": 0}))
    mock_rpc.post(ENDPOINT, payload=_ok({"txid": "55" * 32}))

    issuer = bytes([0x11] * 32)
    data_hash = bytes([0xAB] * 32)
    async with AsyncNOVAIClient(ENDPOINT) as client:
        result = await client.post_oracle_anchor(
            alice,
            issuer_entity_id=issuer,
            data_hash=data_hash,
            external_timestamp=1735776000,
            data_tag="price/ETH-USD",
        )

    # The SDK must populate signal_hash even though the caller did not pass one.
    assert result.signal_hash is not None
    assert len(result.signal_hash) == 64  # 32 bytes hex
    tx_hex = _extract_submitted_tx_hex(mock_rpc)
    tx = decode_tx_v1_signed(bytes.fromhex(tx_hex))
    assert tx.payload[33] == int(AiSignalType.ORACLE_ANCHOR)
    # 66 envelope + 81 fixed extras + 13 tag = 160
    assert len(tx.payload) == 160


@pytest.mark.asyncio
async def test_accept_sla_derives_signal_hash(
    mock_rpc: aioresponses, alice: Keypair
) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok({"nonce": 0}))
    mock_rpc.post(ENDPOINT, payload=_ok({"txid": "66" * 32}))

    async with AsyncNOVAIClient(ENDPOINT) as client:
        result = await client.accept_sla(
            alice,
            seller_entity_id=bytes([0xAA] * 32),
            sla_object_id=bytes([0xBB] * 32),
            buyer_entity_id=bytes([0xCC] * 32),
        )
    assert result.signal_hash is not None
    tx_hex = _extract_submitted_tx_hex(mock_rpc)
    tx = decode_tx_v1_signed(bytes.fromhex(tx_hex))
    assert tx.payload[33] == int(AiSignalType.SLA_ACCEPT)
    # 66 envelope + 64 extras = 130
    assert len(tx.payload) == 130


@pytest.mark.asyncio
async def test_close_channel_carries_both_signatures(
    mock_rpc: aioresponses, alice: Keypair
) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok({"nonce": 0}))
    mock_rpc.post(ENDPOINT, payload=_ok({"txid": "77" * 32}))

    sig_a = bytes([0x30] * 64)
    sig_b = bytes([0x40] * 64)
    async with AsyncNOVAIClient(ENDPOINT) as client:
        await client.close_channel(
            alice,
            issuer_entity_id=bytes([0xAA] * 32),
            channel_object_id=bytes([0xBB] * 32),
            party_a_entity_id=bytes([0xCC] * 32),
            channel_nonce=5,
            balance_a=1000,
            balance_b=500,
            is_final=True,
            sig_a=sig_a,
            sig_b=sig_b,
            signal_hash=bytes([0x99] * 32),
        )
    tx_hex = _extract_submitted_tx_hex(mock_rpc)
    tx = decode_tx_v1_signed(bytes.fromhex(tx_hex))
    assert tx.payload[33] == int(AiSignalType.CHANNEL_CLOSE)
    # 66 envelope + 233 extras = 299
    assert len(tx.payload) == 299
    # sig_a and sig_b at the end of the payload.
    assert tx.payload[-128:-64] == sig_a
    assert tx.payload[-64:] == sig_b


@pytest.mark.asyncio
async def test_credit_entity_layout(
    mock_rpc: aioresponses, alice: Keypair
) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok({"nonce": 5}))
    mock_rpc.post(ENDPOINT, payload=_ok({"txid": "88" * 32}))

    entity_id = bytes([0xDD] * 32)
    async with AsyncNOVAIClient(ENDPOINT) as client:
        await client.credit_entity(alice, entity_id, amount=10**12)
    tx_hex = _extract_submitted_tx_hex(mock_rpc)
    tx = decode_tx_v1_signed(bytes.fromhex(tx_hex))
    assert tx.payload[0] == int(TxPayloadType.CREDIT_AI_ENTITY)
    assert tx.payload[1:33] == entity_id
    assert tx.payload[33:49] == (10**12).to_bytes(16, "big")


@pytest.mark.asyncio
async def test_explicit_nonce_skips_get_nonce(
    mock_rpc: aioresponses, alice: Keypair
) -> None:
    """When caller provides nonce=, the SDK must not call novai_getNonce."""
    mock_rpc.post(ENDPOINT, payload=_ok({"txid": "99" * 32}))
    async with AsyncNOVAIClient(ENDPOINT) as client:
        await client.transfer(alice, bytes([0x11] * 32), amount=1, nonce=42)
    # Only one POST should have happened (the submit).
    calls = mock_rpc.requests
    methods_called: list[str] = []
    for _key, entries in calls.items():
        for entry in entries:
            body = entry.kwargs.get("json")
            if body and "method" in body:
                methods_called.append(body["method"])
    assert methods_called == ["novai_submitTransaction"]


@pytest.mark.asyncio
async def test_create_memory_object_layout(
    mock_rpc: aioresponses, alice: Keypair
) -> None:
    mock_rpc.post(ENDPOINT, payload=_ok({"nonce": 0}))
    mock_rpc.post(ENDPOINT, payload=_ok({"txid": "aa" * 32}))

    data = b"\x01\x02\x03"
    async with AsyncNOVAIClient(ENDPOINT) as client:
        await client.create_memory_object(alice, MemoryObjectType.RATING, data)
    tx_hex = _extract_submitted_tx_hex(mock_rpc)
    tx = decode_tx_v1_signed(bytes.fromhex(tx_hex))
    assert tx.payload[0] == int(TxPayloadType.CREATE_MEMORY)
    assert tx.payload[1] == int(MemoryObjectType.RATING)
    assert tx.payload[2:6] == (3).to_bytes(4, "big")
    assert tx.payload[6:9] == data
