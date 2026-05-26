"""Auto-pagination helpers for height-windowed RPC queries.

The chain caps every range query at ``MAX_QUERY_HEIGHT_RANGE`` (10,000) blocks
per call. These helpers chunk a larger range into successive RPC calls and
yield individual rows so callers can write a single ``async for`` loop over
arbitrary spans without thinking about the cap.
"""

from __future__ import annotations

from collections.abc import AsyncIterator, Awaitable
from typing import Any, Callable

from novai_sdk.constants import MAX_QUERY_HEIGHT_RANGE


async def iter_height_chunks(
    fetch: Callable[[int, int], Awaitable[list[Any]]],
    start_height: int,
    end_height: int,
    *,
    chunk_size: int = MAX_QUERY_HEIGHT_RANGE,
) -> AsyncIterator[Any]:
    """Yield rows from a height-range RPC, chunking past the 10K block cap.

    Args:
        fetch: Async callable taking ``(chunk_start, chunk_end)`` and
            returning a list of rows for that inclusive height window.
        start_height: Inclusive lower bound for the entire iteration.
        end_height: Inclusive upper bound for the entire iteration.
        chunk_size: Maximum span per underlying RPC call. Defaults to the
            chain's cap of 10,000 blocks.

    Each chunk is inclusive on both bounds; the next chunk starts at
    ``previous_end + 1``. Order across chunks follows the order each RPC
    returns within a chunk (typically ascending height).
    """
    if start_height > end_height:
        return
    if chunk_size < 1:
        raise ValueError("chunk_size must be >= 1")
    cursor = start_height
    while cursor <= end_height:
        chunk_end = min(cursor + chunk_size - 1, end_height)
        rows = await fetch(cursor, chunk_end)
        for row in rows:
            yield row
        cursor = chunk_end + 1
