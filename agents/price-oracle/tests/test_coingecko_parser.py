"""CoinGecko response handling: success, 429, 5xx, network, malformed."""

from __future__ import annotations

import io
import socket
import urllib.error
from typing import Any

import pytest

from lib.coingecko import (
    BackoffState,
    NetworkError,
    ParseError,
    PriceObservation,
    RateLimitError,
    ServerError,
    fetch_btc_usd,
)


class _StubResponse:
    def __init__(self, body: bytes, status: int = 200) -> None:
        self._body = body
        self.status = status

    def read(self) -> bytes:
        return self._body

    def __enter__(self) -> "_StubResponse":
        return self

    def __exit__(self, *args: Any) -> None:
        return None


class _StubOpener:
    def __init__(self, response_or_exc: Any) -> None:
        self.response_or_exc = response_or_exc
        self.calls: list[tuple[str, float]] = []

    def open(self, request: Any, timeout: float | None = None) -> Any:
        self.calls.append((getattr(request, "full_url", str(request)), timeout))
        if isinstance(self.response_or_exc, BaseException):
            raise self.response_or_exc
        return self.response_or_exc


def test_fetch_success_returns_price_observation():
    opener = _StubOpener(_StubResponse(b'{"bitcoin": {"usd": 67234.51}}'))
    obs = fetch_btc_usd(url="http://stub", timeout=5.0, opener=opener)
    assert isinstance(obs, PriceObservation)
    assert obs.coin == "bitcoin"
    assert obs.fiat == "usd"
    assert obs.price == 67234.51
    assert opener.calls == [("http://stub", 5.0)]


def test_fetch_429_raises_rate_limit_with_retry_after():
    exc = urllib.error.HTTPError(
        url="http://stub",
        code=429,
        msg="Too Many Requests",
        hdrs={"Retry-After": "30"},  # type: ignore[arg-type]
        fp=io.BytesIO(b"rate limited"),
    )
    opener = _StubOpener(exc)
    with pytest.raises(RateLimitError) as info:
        fetch_btc_usd(url="http://stub", opener=opener)
    assert info.value.retry_after_secs == 30.0


def test_fetch_429_without_retry_after_defaults_to_60():
    exc = urllib.error.HTTPError(
        url="http://stub", code=429, msg="x", hdrs={}, fp=io.BytesIO(b"")  # type: ignore[arg-type]
    )
    opener = _StubOpener(exc)
    with pytest.raises(RateLimitError) as info:
        fetch_btc_usd(url="http://stub", opener=opener)
    assert info.value.retry_after_secs == 60.0


def test_fetch_500_raises_server_error():
    exc = urllib.error.HTTPError(
        url="http://stub", code=503, msg="x", hdrs={}, fp=io.BytesIO(b"")  # type: ignore[arg-type]
    )
    opener = _StubOpener(exc)
    with pytest.raises(ServerError) as info:
        fetch_btc_usd(url="http://stub", opener=opener)
    assert info.value.status == 503


def test_fetch_400_raises_network_error():
    exc = urllib.error.HTTPError(
        url="http://stub", code=400, msg="x", hdrs={}, fp=io.BytesIO(b"")  # type: ignore[arg-type]
    )
    opener = _StubOpener(exc)
    with pytest.raises(NetworkError):
        fetch_btc_usd(url="http://stub", opener=opener)


def test_fetch_timeout_raises_network_error():
    opener = _StubOpener(socket.timeout("timed out"))
    with pytest.raises(NetworkError):
        fetch_btc_usd(url="http://stub", opener=opener)


def test_fetch_url_error_raises_network_error():
    opener = _StubOpener(urllib.error.URLError("connection refused"))
    with pytest.raises(NetworkError):
        fetch_btc_usd(url="http://stub", opener=opener)


def test_fetch_malformed_json_raises_parse_error():
    opener = _StubOpener(_StubResponse(b"not json"))
    with pytest.raises(ParseError):
        fetch_btc_usd(url="http://stub", opener=opener)


def test_fetch_missing_bitcoin_key_raises_parse_error():
    opener = _StubOpener(_StubResponse(b'{"ethereum": {"usd": 1.0}}'))
    with pytest.raises(ParseError):
        fetch_btc_usd(url="http://stub", opener=opener)


def test_fetch_missing_usd_field_raises_parse_error():
    opener = _StubOpener(_StubResponse(b'{"bitcoin": {"eur": 1.0}}'))
    with pytest.raises(ParseError):
        fetch_btc_usd(url="http://stub", opener=opener)


def test_fetch_non_positive_price_raises_parse_error():
    opener = _StubOpener(_StubResponse(b'{"bitcoin": {"usd": -1}}'))
    with pytest.raises(ParseError):
        fetch_btc_usd(url="http://stub", opener=opener)


def test_backoff_state_climbs_then_holds_at_ceiling():
    b = BackoffState()
    assert b.on_rate_limit() == 60.0
    assert b.on_rate_limit() == 120.0
    assert b.on_rate_limit() == 240.0
    assert b.on_rate_limit() == 300.0
    assert b.on_rate_limit() == 300.0


def test_backoff_state_resets_on_success():
    b = BackoffState()
    b.on_rate_limit()
    b.on_rate_limit()
    b.reset()
    assert b.on_rate_limit() == 60.0
