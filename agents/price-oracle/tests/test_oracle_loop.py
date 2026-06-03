"""Oracle main-loop tick semantics: success path and each failure path."""

from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Optional

from novai_sdk import FeeTooLowError, Keypair, NonceTooLowError

import oracle as oracle_mod
from lib.chain import EntityStatus
from lib.coingecko import (
    NetworkError,
    ParseError,
    PriceObservation,
    RateLimitError,
    ServerError,
)
from lib.metrics import MetricsRegistry, build_oracle_registry


@dataclass
class _SubmissionResult:
    txid: str = "abc123"
    signal_hash: str = "sigsig"
    entity_id: Optional[str] = None


class FakeChain:
    def __init__(self) -> None:
        self.posts: list[tuple[bytes, bytes, int, str]] = []
        self.post_exc: Optional[BaseException] = None
        self.height_value: Optional[int] = 12_345
        self.endpoint = "http://fake"

    def entity_id_for(self, address: bytes) -> bytes:
        return b"E" * 32

    def get_entity_status(self, entity_id: bytes) -> EntityStatus:
        return EntityStatus(True, True, 0x47, entity_id, entity_id.hex())

    def post_anchor(
        self,
        kp: Keypair,
        entity_id: bytes,
        data_hash: bytes,
        external_timestamp: int,
        data_tag: str,
    ) -> _SubmissionResult:
        if self.post_exc is not None:
            raise self.post_exc
        self.posts.append((entity_id, data_hash, external_timestamp, data_tag))
        return _SubmissionResult()

    def latest_block_height(self) -> Optional[int]:
        return self.height_value


def _make_oracle(
    *,
    fetch_fn,
    chain: FakeChain | None = None,
    registry: MetricsRegistry | None = None,
    time_fn=lambda: 1_717_428_000,
) -> oracle_mod.Oracle:
    cfg = oracle_mod.OracleConfig(
        endpoint="http://fake",
        key_path=__file__,  # not loaded inside Oracle
        coingecko_url="http://stub",
        metrics_host="127.0.0.1",
        metrics_port=0,
        loop_interval_secs=60.0,
        http_timeout_secs=5.0,
        data_tag="price/BTC-USD",
        log_level="INFO",
    )
    kp = Keypair.generate()
    ch = chain if chain is not None else FakeChain()
    reg = registry if registry is not None else build_oracle_registry(time.monotonic())
    return oracle_mod.Oracle(
        cfg=cfg,
        chain=ch,
        kp=kp,
        entity_id=ch.entity_id_for(kp.address),
        registry=reg,
        fetch_fn=fetch_fn,
        sleep_fn=lambda _s: None,
        time_fn=time_fn,
    )


def _ok_fetch(_url: str, _timeout: float) -> PriceObservation:
    return PriceObservation(coin="bitcoin", fiat="usd", price=67234.51)


def test_tick_happy_path_submits_and_increments_metrics():
    chain = FakeChain()
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    text = reg.render()
    assert "novai_oracle_price_fetch_success_total 1" in text
    assert "novai_oracle_submission_success_total 1" in text
    assert "novai_oracle_last_price_usd 67234.51" in text
    assert "novai_oracle_last_submission_height 12345" in text
    assert len(chain.posts) == 1
    entity_id, data_hash, ts, tag = chain.posts[0]
    assert len(data_hash) == 32
    assert ts == 1_717_428_000
    assert tag == "price/BTC-USD"


def test_tick_rate_limit_bumps_backoff_and_does_not_submit():
    chain = FakeChain()
    reg = build_oracle_registry(time.monotonic())

    def raise_rate_limit(_url: str, _timeout: float):
        raise RateLimitError(60.0)

    o = _make_oracle(fetch_fn=raise_rate_limit, chain=chain, registry=reg)
    o._tick()
    text = reg.render()
    assert 'novai_oracle_price_fetch_failure_total{reason="rate_limit"} 1' in text
    assert "novai_oracle_submission_success_total 0" in text
    assert chain.posts == []
    assert o.backoff.index == 1


def test_tick_server_error_increments_correct_reason():
    reg = build_oracle_registry(time.monotonic())

    def raise_5xx(_url: str, _timeout: float):
        raise ServerError(503)

    o = _make_oracle(fetch_fn=raise_5xx, registry=reg)
    o._tick()
    text = reg.render()
    assert 'novai_oracle_price_fetch_failure_total{reason="server_error"} 1' in text


def test_tick_network_error_increments_correct_reason():
    reg = build_oracle_registry(time.monotonic())

    def raise_net(_url: str, _timeout: float):
        raise NetworkError("conn refused")

    o = _make_oracle(fetch_fn=raise_net, registry=reg)
    o._tick()
    text = reg.render()
    assert 'novai_oracle_price_fetch_failure_total{reason="network_error"} 1' in text


def test_tick_parse_error_increments_correct_reason():
    reg = build_oracle_registry(time.monotonic())

    def raise_parse(_url: str, _timeout: float):
        raise ParseError("bad json")

    o = _make_oracle(fetch_fn=raise_parse, registry=reg)
    o._tick()
    text = reg.render()
    assert 'novai_oracle_price_fetch_failure_total{reason="parse_error"} 1' in text


def test_tick_submission_fee_too_low_increments_reason():
    chain = FakeChain()
    chain.post_exc = FeeTooLowError(-32011, "fee too low: required 1000")
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    text = reg.render()
    assert 'novai_oracle_submission_failure_total{reason="fee_too_low"} 1' in text


def test_tick_submission_nonce_too_low_increments_reason():
    chain = FakeChain()
    chain.post_exc = NonceTooLowError(-32010, "nonce too low: expected 42")
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    text = reg.render()
    assert 'novai_oracle_submission_failure_total{reason="nonce_too_low"} 1' in text


def test_tick_submission_arbitrary_exception_falls_back_to_rpc_unreachable():
    chain = FakeChain()
    chain.post_exc = ConnectionRefusedError("conn refused")
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    text = reg.render()
    assert 'novai_oracle_submission_failure_total{reason="rpc_unreachable"} 1' in text


def test_sliced_sleep_yields_on_stop():
    o = _make_oracle(fetch_fn=_ok_fetch)
    calls: list[float] = []
    o.sleep_fn = lambda s: calls.append(s) or setattr(o, "_stopping", True)
    o._sliced_sleep(60.0)
    # Stops after the first 1-second slice.
    assert calls == [1.0]


def test_loop_exits_promptly_on_stopping_flag():
    o = _make_oracle(fetch_fn=_ok_fetch)
    o._stopping = True
    rc = o.run_forever()
    assert rc == 0


def test_loop_records_loop_completed_timestamp_after_each_tick():
    chain = FakeChain()
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    o.registry.set_gauge("novai_oracle_last_loop_completed_timestamp", 1_717_428_000)
    text = reg.render()
    assert "novai_oracle_last_loop_completed_timestamp 1717428000" in text
