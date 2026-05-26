"""Async JSON-RPC client.

This module provides the Phase 1 surface: raw RPC dispatch plus the
submission/query primitives the rest of the SDK builds on
(``submit_tx``, ``get_nonce``, ``get_balance``, ``faucet``). All other
typed wrappers (entities, payments, channels, etc.) are layered in Phase 3.
"""

from __future__ import annotations

import itertools
from dataclasses import dataclass
from types import TracebackType
from typing import Any, Optional

import aiohttp

from novai_sdk._hex import bytes_to_hex, coerce_address
from novai_sdk.codec import TxV1, encode_tx_v1_signed
from novai_sdk.errors import DecodeError, NovaiError, NovaiRpcError, rpc_error_from


@dataclass(frozen=True)
class FaucetResult:
    """Response shape returned by ``novai_faucet``."""

    txid: str
    amount: str


@dataclass(frozen=True)
class BalanceResult:
    """Response shape returned by ``novai_getBalance``."""

    balance: str
    nonce: int


class AsyncNOVAIClient:
    """Async client for the NOVAI JSON-RPC endpoint.

    The client owns an ``aiohttp.ClientSession`` lazily. Use it as an async
    context manager for deterministic resource cleanup, or call
    :meth:`close` explicitly::

        async with AsyncNOVAIClient("http://localhost:3030") as client:
            nonce = await client.get_nonce(address)

        # or:
        client = AsyncNOVAIClient("http://localhost:3030")
        try:
            ...
        finally:
            await client.close()
    """

    def __init__(
        self,
        endpoint: str = "http://localhost:3030",
        *,
        timeout_seconds: float = 30.0,
        session: aiohttp.ClientSession | None = None,
    ) -> None:
        self._endpoint = endpoint.rstrip("/")
        self._timeout = aiohttp.ClientTimeout(total=timeout_seconds)
        self._session = session
        self._owned_session = session is None
        self._id_counter = itertools.count(1)

    @property
    def endpoint(self) -> str:
        """The HTTP endpoint URL (no trailing slash)."""
        return self._endpoint

    async def __aenter__(self) -> AsyncNOVAIClient:
        await self._ensure_session()
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        await self.close()

    async def close(self) -> None:
        """Close the underlying HTTP session if we own it."""
        if self._owned_session and self._session is not None:
            await self._session.close()
            self._session = None

    async def _ensure_session(self) -> aiohttp.ClientSession:
        if self._session is None:
            self._session = aiohttp.ClientSession(timeout=self._timeout)
            self._owned_session = True
        return self._session

    async def call(self, method: str, params: dict[str, Any] | list[Any] | None = None) -> Any:
        """Dispatch a raw JSON-RPC 2.0 call and return the ``result`` field.

        Args:
            method: RPC method name (e.g. ``"novai_getNonce"``).
            params: Either a JSON object (named params) or a list (positional).
                ``None`` is sent as an empty object.

        Raises:
            NovaiRpcError: If the node returns a JSON-RPC error envelope. The
                concrete class depends on the ``code`` (see :mod:`novai_sdk.errors`).
            NovaiError: On transport failure or malformed response.
        """
        session = await self._ensure_session()
        request_id = next(self._id_counter)
        body = {
            "jsonrpc": "2.0",
            "method": method,
            "params": params if params is not None else {},
            "id": request_id,
        }
        try:
            async with session.post(self._endpoint, json=body) as resp:
                text = await resp.text()
                if resp.status != 200:
                    raise NovaiError(f"HTTP {resp.status} from {self._endpoint}: {text}")
                try:
                    envelope = await resp.json(content_type=None)
                except aiohttp.ContentTypeError as exc:
                    raise NovaiError(
                        f"non-JSON response from {self._endpoint}: {text[:200]}"
                    ) from exc
        except aiohttp.ClientError as exc:
            raise NovaiError(f"transport error contacting {self._endpoint}: {exc}") from exc

        if not isinstance(envelope, dict):
            raise NovaiError(f"unexpected RPC envelope shape: {envelope!r}")
        if "error" in envelope and envelope["error"] is not None:
            err = envelope["error"]
            code = int(err.get("code", -32000))
            message = str(err.get("message", "unknown error"))
            raise rpc_error_from(code, message, err.get("data"))
        if "result" not in envelope:
            raise NovaiError(f"RPC envelope missing 'result' field: {envelope!r}")
        return envelope["result"]

    # ------------------------------------------------------------------
    # Submission + nonce + balance + faucet (Phase 1 primitives)
    # ------------------------------------------------------------------

    async def submit_tx(self, tx: TxV1) -> str:
        """Submit a signed TxV1 to the mempool and return its txid (hex)."""
        if tx.sig == bytes(64):
            raise NovaiError("refusing to submit an unsigned tx (sig is all zeros)")
        tx_hex = bytes_to_hex(encode_tx_v1_signed(tx))
        result = await self.call("novai_submitTransaction", {"tx": tx_hex})
        return _expect_hex(result, "txid")

    async def submit_raw_tx(self, tx_bytes: bytes) -> str:
        """Submit an already-encoded signed tx (escape hatch for pre-built bytes)."""
        result = await self.call("novai_submitTransaction", {"tx": bytes_to_hex(tx_bytes)})
        return _expect_hex(result, "txid")

    async def get_nonce(self, address: bytes | str) -> int:
        """Return the next expected nonce for ``address``."""
        addr_hex = bytes_to_hex(coerce_address(address))
        result = await self.call("novai_getNonce", {"address": addr_hex})
        return _expect_int(result, "nonce")

    async def get_balance(self, address: bytes | str) -> BalanceResult:
        """Return the current balance (decimal string) and nonce for ``address``."""
        addr_hex = bytes_to_hex(coerce_address(address))
        result = await self.call("novai_getBalance", {"address": addr_hex})
        if not isinstance(result, dict):
            raise DecodeError(f"novai_getBalance: expected object, got {result!r}")
        try:
            return BalanceResult(
                balance=str(result["balance"]),
                nonce=int(result["nonce"]),
            )
        except (KeyError, TypeError, ValueError) as exc:
            raise DecodeError(f"novai_getBalance: bad response shape {result!r}") from exc

    async def faucet(self, address: bytes | str) -> FaucetResult:
        """Request a faucet dispense (dev / testnet only).

        Raises:
            RateLimitedError: If the faucet's cooldown is active.
            ServerError: If the faucet is disabled on this node.
        """
        addr_hex = bytes_to_hex(coerce_address(address))
        result = await self.call("novai_faucet", {"address": addr_hex})
        if not isinstance(result, dict):
            raise DecodeError(f"novai_faucet: expected object, got {result!r}")
        try:
            return FaucetResult(
                txid=str(result["txid"]),
                amount=str(result["amount"]),
            )
        except (KeyError, TypeError) as exc:
            raise DecodeError(f"novai_faucet: bad response shape {result!r}") from exc


def _expect_hex(result: Any, field: str) -> str:
    if isinstance(result, dict) and field in result:
        return str(result[field])
    raise DecodeError(f"expected object with '{field}' field, got {result!r}")


def _expect_int(result: Any, field: str) -> int:
    if isinstance(result, dict) and field in result:
        try:
            return int(result[field])
        except (TypeError, ValueError) as exc:
            raise DecodeError(f"field '{field}' is not an int: {result[field]!r}") from exc
    raise DecodeError(f"expected object with '{field}' field, got {result!r}")


# Re-export the canonical RPC error base for callers who want to catch
# any RPC-level failure without picking a concrete subclass.
__all__ = [
    "AsyncNOVAIClient",
    "BalanceResult",
    "FaucetResult",
    "NovaiRpcError",
    "Optional",  # for type completeness in older Pythons
]
