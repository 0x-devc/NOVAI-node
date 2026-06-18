"""GPU pricing source: success, median, 429, 5xx, network, malformed, no-data."""

from __future__ import annotations

import io
import json
import socket
import urllib.error
from typing import Any

import pytest

from lib.gpu_source import (
    BackoffState,
    GpuPriceObservation,
    NetworkError,
    NoDataError,
    ParseError,
    RateLimitError,
    ServerError,
    fetch_gpu_price,
    normalize_model_label,
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


def _body(offers: list[dict]) -> bytes:
    return json.dumps({"offers": offers}).encode("utf-8")


def test_normalize_model_label():
    assert normalize_model_label("RTX 4090") == "RTX4090"
    assert normalize_model_label("  geforce rtx 4090 ") == "GEFORCERTX4090"


def test_fetch_success_returns_median_per_gpu_price():
    offers = [
        {"id": 1, "gpu_name": "RTX 4090", "num_gpus": 1, "dph_total": 0.30},
        {"id": 2, "gpu_name": "RTX 4090", "num_gpus": 1, "dph_total": 0.40},
        {"id": 3, "gpu_name": "GeForce RTX 4090", "num_gpus": 1, "dph_total": 0.50},
        {"id": 4, "gpu_name": "RTX 3090", "num_gpus": 1, "dph_total": 0.20},
    ]
    opener = _StubOpener(_StubResponse(_body(offers)))
    obs = fetch_gpu_price(url="http://stub", timeout=5.0, model="RTX 4090", opener=opener)
    assert isinstance(obs, GpuPriceObservation)
    assert obs.model == "RTX4090"
    assert obs.currency == "usd"
    assert obs.unit == "hour"
    # matching per-gpu prices [0.30, 0.40, 0.50] -> median 0.40
    assert obs.price == pytest.approx(0.40)
    assert obs.sample_size == 3
    assert opener.calls == [("http://stub", 5.0)]


def test_fetch_divides_by_num_gpus_for_per_gpu_price():
    offers = [{"gpu_name": "RTX 4090", "num_gpus": 4, "dph_total": 1.60}]
    opener = _StubOpener(_StubResponse(_body(offers)))
    obs = fetch_gpu_price(url="http://stub", model="RTX 4090", opener=opener)
    # 1.60 total / 4 gpus = 0.40 per gpu
    assert obs.price == pytest.approx(0.40)
    assert obs.sample_size == 1


def test_fetch_skips_malformed_offers_but_uses_valid_ones():
    offers = [
        {"gpu_name": "RTX 4090", "num_gpus": 1, "dph_total": 0.42},
        {"gpu_name": "RTX 4090", "num_gpus": 1},  # missing dph_total -> skipped
        {"gpu_name": "RTX 4090", "num_gpus": 0, "dph_total": 0.99},  # bad num_gpus -> skipped
        {"gpu_name": "RTX 4090", "num_gpus": 1, "dph_total": -1.0},  # non-positive -> skipped
        "not-an-offer",  # wrong type -> skipped
    ]
    opener = _StubOpener(_StubResponse(_body(offers)))
    obs = fetch_gpu_price(url="http://stub", model="RTX 4090", opener=opener)
    assert obs.price == pytest.approx(0.42)
    assert obs.sample_size == 1


def test_fetch_no_matching_model_raises_no_data():
    offers = [{"gpu_name": "RTX 3090", "num_gpus": 1, "dph_total": 0.20}]
    opener = _StubOpener(_StubResponse(_body(offers)))
    with pytest.raises(NoDataError):
        fetch_gpu_price(url="http://stub", model="RTX 4090", opener=opener)


def test_fetch_empty_offers_raises_no_data():
    opener = _StubOpener(_StubResponse(_body([])))
    with pytest.raises(NoDataError):
        fetch_gpu_price(url="http://stub", model="RTX 4090", opener=opener)


def test_fetch_malformed_json_raises_parse_error():
    opener = _StubOpener(_StubResponse(b"not json"))
    with pytest.raises(ParseError):
        fetch_gpu_price(url="http://stub", model="RTX 4090", opener=opener)


def test_fetch_non_dict_top_level_raises_parse_error():
    opener = _StubOpener(_StubResponse(b"[1, 2, 3]"))
    with pytest.raises(ParseError):
        fetch_gpu_price(url="http://stub", model="RTX 4090", opener=opener)


def test_fetch_missing_offers_list_raises_parse_error():
    opener = _StubOpener(_StubResponse(b'{"data": []}'))
    with pytest.raises(ParseError):
        fetch_gpu_price(url="http://stub", model="RTX 4090", opener=opener)


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
        fetch_gpu_price(url="http://stub", model="RTX 4090", opener=opener)
    assert info.value.retry_after_secs == 30.0


def test_fetch_429_without_retry_after_defaults_to_60():
    exc = urllib.error.HTTPError(
        url="http://stub", code=429, msg="x", hdrs={}, fp=io.BytesIO(b"")  # type: ignore[arg-type]
    )
    opener = _StubOpener(exc)
    with pytest.raises(RateLimitError) as info:
        fetch_gpu_price(url="http://stub", model="RTX 4090", opener=opener)
    assert info.value.retry_after_secs == 60.0


def test_fetch_500_raises_server_error():
    exc = urllib.error.HTTPError(
        url="http://stub", code=503, msg="x", hdrs={}, fp=io.BytesIO(b"")  # type: ignore[arg-type]
    )
    opener = _StubOpener(exc)
    with pytest.raises(ServerError) as info:
        fetch_gpu_price(url="http://stub", model="RTX 4090", opener=opener)
    assert info.value.status == 503


def test_fetch_400_raises_network_error():
    exc = urllib.error.HTTPError(
        url="http://stub", code=400, msg="x", hdrs={}, fp=io.BytesIO(b"")  # type: ignore[arg-type]
    )
    opener = _StubOpener(exc)
    with pytest.raises(NetworkError):
        fetch_gpu_price(url="http://stub", model="RTX 4090", opener=opener)


def test_fetch_timeout_raises_network_error():
    opener = _StubOpener(socket.timeout("timed out"))
    with pytest.raises(NetworkError):
        fetch_gpu_price(url="http://stub", model="RTX 4090", opener=opener)


def test_fetch_url_error_raises_network_error():
    opener = _StubOpener(urllib.error.URLError("connection refused"))
    with pytest.raises(NetworkError):
        fetch_gpu_price(url="http://stub", model="RTX 4090", opener=opener)


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
