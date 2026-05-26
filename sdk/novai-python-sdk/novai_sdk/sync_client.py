"""Thin synchronous wrapper around :class:`AsyncNOVAIClient`.

Every public method of the async client is exposed here as a blocking
function. Implementation strategy: each call spins up a private asyncio
event loop, runs the coroutine to completion, and tears the loop back down.
This keeps the codebase single-implementation: there is no parallel sync
transport stack to maintain.

If you are already inside an event loop (e.g. running under Jupyter or a
web framework), use :class:`AsyncNOVAIClient` directly instead. Calling
this sync client from inside a running loop will raise :class:`RuntimeError`.
"""

from __future__ import annotations

import asyncio
from collections.abc import Awaitable
from typing import Any, Callable, TypeVar

from novai_sdk.client import AsyncNOVAIClient, BalanceResult, FaucetResult
from novai_sdk.codec import TxV1

T = TypeVar("T")


class NOVAIClient:
    """Synchronous facade over :class:`AsyncNOVAIClient`.

    Each method blocks on a fresh event loop, runs the corresponding async
    method, then closes the underlying session. Use this when you don't want
    to write ``async def`` everywhere.
    """

    def __init__(self, endpoint: str = "http://localhost:3030", *, timeout_seconds: float = 30.0):
        self._endpoint = endpoint
        self._timeout_seconds = timeout_seconds

    @property
    def endpoint(self) -> str:
        """The HTTP endpoint URL."""
        return self._endpoint

    def call(self, method: str, params: dict[str, Any] | list[Any] | None = None) -> Any:
        """Synchronous JSON-RPC dispatch. See :meth:`AsyncNOVAIClient.call`."""
        return self._run(lambda c: c.call(method, params))

    def submit_tx(self, tx: TxV1) -> str:
        """Synchronous submission. See :meth:`AsyncNOVAIClient.submit_tx`."""
        return self._run(lambda c: c.submit_tx(tx))

    def submit_raw_tx(self, tx_bytes: bytes) -> str:
        """Synchronous submission of pre-encoded bytes."""
        return self._run(lambda c: c.submit_raw_tx(tx_bytes))

    def get_nonce(self, address: bytes | str) -> int:
        """Synchronous nonce query."""
        return self._run(lambda c: c.get_nonce(address))

    def get_balance(self, address: bytes | str) -> BalanceResult:
        """Synchronous balance query."""
        return self._run(lambda c: c.get_balance(address))

    def faucet(self, address: bytes | str) -> FaucetResult:
        """Synchronous faucet dispense."""
        return self._run(lambda c: c.faucet(address))

    def _run(self, fn: Callable[[AsyncNOVAIClient], Awaitable[T]]) -> T:
        try:
            asyncio.get_running_loop()
        except RuntimeError:
            running = False
        else:
            running = True
        if running:
            raise RuntimeError(
                "NOVAIClient is the sync wrapper; you are inside a running event loop. "
                "Use AsyncNOVAIClient(...) directly instead."
            )

        async def _runner() -> T:
            async with AsyncNOVAIClient(
                self._endpoint, timeout_seconds=self._timeout_seconds
            ) as client:
                return await fn(client)

        return asyncio.run(_runner())
