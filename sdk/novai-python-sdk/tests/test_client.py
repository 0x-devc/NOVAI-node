"""Tests for novai_sdk.client (async RPC dispatch with mocked HTTP)."""

from __future__ import annotations

import pytest
from aioresponses import aioresponses

from novai_sdk import AsyncNOVAIClient, Keypair, TxV1
from novai_sdk.errors import (
    FeeTooLowError,
    MempoolFullError,
    NonceTooLowError,
    NovaiError,
    NovaiRpcError,
    ValidationError,
)

ENDPOINT = "http://localhost:3030"


@pytest.fixture
def mock_rpc() -> aioresponses:
    with aioresponses() as m:
        yield m


@pytest.mark.asyncio
async def test_get_nonce_returns_int(mock_rpc: aioresponses) -> None:
    mock_rpc.post(ENDPOINT, payload={"jsonrpc": "2.0", "id": 1, "result": {"nonce": 42}})
    async with AsyncNOVAIClient(ENDPOINT) as client:
        nonce = await client.get_nonce(bytes(32))
    assert nonce == 42


@pytest.mark.asyncio
async def test_get_nonce_accepts_hex_string(mock_rpc: aioresponses) -> None:
    mock_rpc.post(ENDPOINT, payload={"jsonrpc": "2.0", "id": 1, "result": {"nonce": 7}})
    async with AsyncNOVAIClient(ENDPOINT) as client:
        nonce = await client.get_nonce("00" * 32)
    assert nonce == 7


@pytest.mark.asyncio
async def test_get_balance_returns_dataclass(mock_rpc: aioresponses) -> None:
    mock_rpc.post(
        ENDPOINT,
        payload={
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"balance": "1000000000000", "nonce": 5},
        },
    )
    async with AsyncNOVAIClient(ENDPOINT) as client:
        result = await client.get_balance(bytes(32))
    assert result.balance == "1000000000000"
    assert result.nonce == 5


@pytest.mark.asyncio
async def test_faucet_returns_dataclass(mock_rpc: aioresponses) -> None:
    mock_rpc.post(
        ENDPOINT,
        payload={
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"txid": "ab" * 32, "amount": "10000000"},
        },
    )
    async with AsyncNOVAIClient(ENDPOINT) as client:
        result = await client.faucet(bytes(32))
    assert result.txid == "ab" * 32
    assert result.amount == "10000000"


@pytest.mark.asyncio
async def test_submit_tx_refuses_unsigned(kp_alice: Keypair) -> None:
    tx = TxV1(
        from_address=kp_alice.address,
        pubkey=kp_alice.pubkey,
        nonce=0,
        fee=100,
        payload=b"",
    )
    async with AsyncNOVAIClient(ENDPOINT) as client:
        with pytest.raises(NovaiError, match="unsigned"):
            await client.submit_tx(tx)


@pytest.mark.asyncio
async def test_submit_tx_dispatches_signed_hex(
    mock_rpc: aioresponses, kp_alice: Keypair
) -> None:
    from novai_sdk import sign_tx_v1

    tx = TxV1(
        from_address=kp_alice.address,
        pubkey=kp_alice.pubkey,
        nonce=0,
        fee=100,
        payload=b"hello",
    )
    tx.sig = sign_tx_v1(kp_alice.signing_key, tx)
    mock_rpc.post(ENDPOINT, payload={"jsonrpc": "2.0", "id": 1, "result": {"txid": "cd" * 32}})
    async with AsyncNOVAIClient(ENDPOINT) as client:
        txid = await client.submit_tx(tx)
    assert txid == "cd" * 32


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "code,cls",
    [
        (-32010, NonceTooLowError),
        (-32011, FeeTooLowError),
        (-32001, MempoolFullError),
        (-32013, ValidationError),
    ],
)
async def test_rpc_error_codes_map_to_specific_classes(
    mock_rpc: aioresponses, code: int, cls: type[NovaiRpcError]
) -> None:
    mock_rpc.post(
        ENDPOINT,
        payload={
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": code, "message": "test failure"},
        },
    )
    async with AsyncNOVAIClient(ENDPOINT) as client:
        with pytest.raises(cls) as exc_info:
            await client.get_nonce(bytes(32))
    assert exc_info.value.code == code


@pytest.mark.asyncio
async def test_malformed_response_raises_novai_error(mock_rpc: aioresponses) -> None:
    mock_rpc.post(ENDPOINT, payload={"jsonrpc": "2.0", "id": 1})  # no result, no error
    async with AsyncNOVAIClient(ENDPOINT) as client:
        with pytest.raises(NovaiError, match="missing 'result'"):
            await client.get_nonce(bytes(32))


@pytest.mark.asyncio
async def test_http_500_raises_novai_error(mock_rpc: aioresponses) -> None:
    mock_rpc.post(ENDPOINT, status=500, body="internal error")
    async with AsyncNOVAIClient(ENDPOINT) as client:
        with pytest.raises(NovaiError, match="HTTP 500"):
            await client.get_nonce(bytes(32))


@pytest.mark.asyncio
async def test_call_with_positional_params(mock_rpc: aioresponses) -> None:
    mock_rpc.post(ENDPOINT, payload={"jsonrpc": "2.0", "id": 1, "result": "ok"})
    async with AsyncNOVAIClient(ENDPOINT) as client:
        result = await client.call("custom_method", [1, 2, 3])
    assert result == "ok"


@pytest.mark.asyncio
async def test_context_manager_closes_session() -> None:
    client = AsyncNOVAIClient(ENDPOINT)
    async with client:
        pass
    # After exit, the session should be torn down.
    assert client._session is None  # type: ignore[attr-defined]
