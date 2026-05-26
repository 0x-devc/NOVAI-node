"""Tests for novai_sdk.paginate (height-chunk iterator)."""

from __future__ import annotations

import pytest

from novai_sdk.paginate import iter_height_chunks


class TestIterHeightChunks:
    @pytest.mark.asyncio
    async def test_single_chunk_when_range_fits(self) -> None:
        calls: list[tuple[int, int]] = []

        async def fetch(s: int, e: int) -> list[int]:
            calls.append((s, e))
            return list(range(s, e + 1))

        out: list[int] = []
        async for row in iter_height_chunks(fetch, 0, 99, chunk_size=10_000):
            out.append(row)
        assert out == list(range(0, 100))
        assert calls == [(0, 99)]

    @pytest.mark.asyncio
    async def test_chunks_when_range_exceeds_cap(self) -> None:
        calls: list[tuple[int, int]] = []

        async def fetch(s: int, e: int) -> list[str]:
            calls.append((s, e))
            return [f"row-{s}-{e}"]

        out: list[str] = []
        async for row in iter_height_chunks(fetch, 0, 2_500, chunk_size=1_000):
            out.append(row)
        assert calls == [(0, 999), (1_000, 1_999), (2_000, 2_500)]
        assert out == ["row-0-999", "row-1000-1999", "row-2000-2500"]

    @pytest.mark.asyncio
    async def test_empty_range(self) -> None:
        async def fetch(s: int, e: int) -> list[int]:
            raise AssertionError("fetch should not be called for empty range")

        out: list[int] = []
        async for row in iter_height_chunks(fetch, 100, 50):
            out.append(row)
        assert out == []

    @pytest.mark.asyncio
    async def test_yields_in_order(self) -> None:
        async def fetch(s: int, e: int) -> list[int]:
            return [s, s + 1]

        out: list[int] = []
        async for row in iter_height_chunks(fetch, 0, 9, chunk_size=5):
            out.append(row)
        # First chunk [0, 4] yields [0, 1], second chunk [5, 9] yields [5, 6].
        assert out == [0, 1, 5, 6]
