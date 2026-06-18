"""Oracle loop: dry-run cycle, fetch-error handling, and live-path top-ups.

Dry-run cycles construct and log the signal without submitting. The live-path
tests use a FakeChain to exercise the ported two-tier top-up and the two-key
signer split without any network.
"""

from __future__ import annotations

import dataclasses
import time
from typing import Optional

import pytest
from novai_sdk import Keypair

import oracle as oracle_mod
from lib.chain import Chain
from lib.config import ComputeOracleConfig
from lib.gpu_source import (
    GpuPriceObservation,
    NetworkError,
    NoDataError,
    ParseError,
    RateLimitError,
    ServerError,
)
from lib.metrics import build_compute_oracle_registry


def _cfg(**overrides) -> ComputeOracleConfig:
    cfg = ComputeOracleConfig.from_env({})
    return dataclasses.replace(cfg, **overrides) if overrides else cfg


def _ok_fetch(_url: str, _timeout: float, _model: str) -> GpuPriceObservation:
    return GpuPriceObservation(
        model="RTX4090",
        currency="usd",
        unit="hour",
        price=0.34,
        sample_size=5,
        source="vast.ai/api/v0/bundles",
    )


def _make_dry_oracle(*, fetch_fn=_ok_fetch, cfg=None, registry=None):
    cfg = cfg or _cfg()
    reg = registry or build_compute_oracle_registry(time.monotonic())
    chain = Chain(cfg.endpoint, dry_run=True, dry_run_nonce=cfg.dry_run_nonce)
    funder = Keypair.generate()
    entity = Keypair.generate()
    entity_id = chain.entity_id_for(funder.address)
    return oracle_mod.Oracle(
        cfg,
        chain,
        funder,
        entity,
        entity_id,
        reg,
        dry_run=True,
        fetch_fn=fetch_fn,
        sleep_fn=lambda _s: None,
        time_fn=lambda: 1_718_000_000,
        monotonic_fn=lambda: 0.0,
    )


# -- Dry-run cycle -----------------------------------------------------------


def test_dry_run_tick_constructs_anchor_and_increments_metrics():
    reg = build_compute_oracle_registry(time.monotonic())
    o = _make_dry_oracle(registry=reg)
    o._tick()
    text = reg.render()
    assert "novai_compute_oracle_price_fetch_success_total 1" in text
    assert 'novai_compute_oracle_dry_run_constructed_total{kind="oracle_anchor"} 1' in text
    assert "novai_compute_oracle_last_price_usd_per_hour 0.34" in text
    assert "novai_compute_oracle_last_sample_size 5" in text
    # dry-run never increments the live submission counter
    assert "novai_compute_oracle_submission_success_total 0" in text


def test_run_once_runs_single_cycle():
    reg = build_compute_oracle_registry(time.monotonic())
    o = _make_dry_oracle(registry=reg)
    assert o.run_once() == 0
    assert 'novai_compute_oracle_dry_run_constructed_total{kind="oracle_anchor"} 1' in reg.render()


def test_dry_run_tick_skips_construction_when_no_data():
    reg = build_compute_oracle_registry(time.monotonic())

    def no_data(_u, _t, _m):
        raise NoDataError("no offers for model")

    o = _make_dry_oracle(fetch_fn=no_data, registry=reg)
    o._tick()
    text = reg.render()
    assert 'novai_compute_oracle_price_fetch_failure_total{reason="no_data"} 1' in text
    assert 'novai_compute_oracle_dry_run_constructed_total{kind="oracle_anchor"} 0' in text


@pytest.mark.parametrize(
    "exc,reason",
    [
        (RateLimitError(60.0), "rate_limit"),
        (ServerError(503), "server_error"),
        (NetworkError("conn refused"), "network_error"),
        (ParseError("bad json"), "parse_error"),
    ],
)
def test_dry_run_fetch_errors_increment_reason_and_do_not_construct(exc, reason):
    reg = build_compute_oracle_registry(time.monotonic())

    def boom(_u, _t, _m):
        raise exc

    o = _make_dry_oracle(fetch_fn=boom, registry=reg)
    o._tick()
    text = reg.render()
    assert f'novai_compute_oracle_price_fetch_failure_total{{reason="{reason}"}} 1' in text
    assert 'novai_compute_oracle_dry_run_constructed_total{kind="oracle_anchor"} 0' in text


def test_dry_run_rate_limit_bumps_backoff():
    def rl(_u, _t, _m):
        raise RateLimitError(60.0)

    o = _make_dry_oracle(fetch_fn=rl)
    o._tick()
    assert o.backoff.index == 1


def test_dry_run_reputation_constructed_when_enabled():
    cfg = _cfg(
        reputation_enabled=True,
        reputation_target_hex="0a" * 32,
        reputation_event_type=3,
        reputation_points_delta=5,
    )
    reg = build_compute_oracle_registry(time.monotonic())
    o = _make_dry_oracle(cfg=cfg, registry=reg)
    o._tick()
    text = reg.render()
    assert 'novai_compute_oracle_dry_run_constructed_total{kind="oracle_anchor"} 1' in text
    assert 'novai_compute_oracle_dry_run_constructed_total{kind="reputation_update"} 1' in text


# -- Live path (FakeChain, no network) ---------------------------------------


class _FakeSubmission:
    def __init__(self, txid: str = "txid", signal_hash: str = "sig") -> None:
        self.txid = txid
        self.signal_hash = signal_hash


class _FakeFaucet:
    def __init__(self) -> None:
        self.txid = "faucetxid"
        self.amount = "1000000"


class FakeChain:
    def __init__(self) -> None:
        self.posts: list[tuple] = []
        self.post_signer: list[bytes] = []
        self.credit_calls: list[tuple] = []
        self.credit_signer: list[bytes] = []
        self.account_nonce_addrs: list[bytes] = []
        self.faucet_calls: list[bytes] = []
        self.entity_balance = 10_000_000
        self.account_balance = 10_000_000
        self.account_nonce = 0
        self.height: Optional[int] = 999

    def entity_id_for(self, address: bytes) -> bytes:
        return b"E" * 32

    def get_entity_economic_balance(self, entity_id: bytes) -> int:
        return self.entity_balance

    def get_account_nonce(self, address: bytes) -> int:
        self.account_nonce_addrs.append(address)
        return self.account_nonce

    def credit_entity(self, kp, entity_id, amount, *, nonce, fee=100):
        self.credit_calls.append((entity_id, amount, nonce))
        self.credit_signer.append(kp.address)
        return _FakeSubmission("creditxid")

    def get_balance(self, address: bytes) -> int:
        return self.account_balance

    def faucet(self, address: bytes):
        self.faucet_calls.append(address)
        return _FakeFaucet()

    def post_anchor(self, kp, entity_id, data_hash, ts, tag, *, source_hash, expiry_height=0, fee=1000):
        self.posts.append((entity_id, data_hash, ts, tag, source_hash, expiry_height, fee))
        self.post_signer.append(kp.address)
        return _FakeSubmission()

    def latest_block_height(self) -> Optional[int]:
        return self.height


def _make_live_oracle(*, chain: FakeChain, registry=None, fetch_fn=_ok_fetch):
    cfg = _cfg(dry_run=False)
    reg = registry or build_compute_oracle_registry(time.monotonic())
    funder = Keypair.generate()
    entity = Keypair.generate()
    return oracle_mod.Oracle(
        cfg,
        chain,
        funder,
        entity,
        chain.entity_id_for(funder.address),
        reg,
        dry_run=False,
        fetch_fn=fetch_fn,
        sleep_fn=lambda _s: None,
        time_fn=lambda: 1_718_000_000,
        monotonic_fn=lambda: 0.0,
    )


def test_live_tick_submits_and_records_height():
    chain = FakeChain()
    reg = build_compute_oracle_registry(time.monotonic())
    o = _make_live_oracle(chain=chain, registry=reg)
    o._tick()
    assert len(chain.posts) == 1
    text = reg.render()
    assert "novai_compute_oracle_submission_success_total 1" in text
    assert "novai_compute_oracle_last_submission_height 999" in text
    # OracleAnchor is signed by the entity key, not the funder.
    assert chain.post_signer == [o.entity_kp.address]


def test_live_tier1_credits_with_funder_when_entity_balance_low():
    chain = FakeChain()
    chain.entity_balance = 100
    chain.account_nonce = 1
    o = _make_live_oracle(chain=chain)
    o._tick()
    assert len(chain.credit_calls) == 1
    _eid, amount, nonce = chain.credit_calls[0]
    assert amount == 100_000
    assert nonce == 1  # account nonce, passed through
    assert chain.credit_signer == [o.funder_kp.address]
    assert chain.account_nonce_addrs == [o.funder_kp.address]


def test_live_tier2_faucets_funder_when_account_low():
    chain = FakeChain()
    chain.account_balance = 100
    o = _make_live_oracle(chain=chain)
    o._tick()
    assert chain.faucet_calls == [o.funder_kp.address]


def test_live_submit_runs_after_both_topups():
    chain = FakeChain()
    chain.entity_balance = 0
    chain.account_balance = 0
    chain.account_nonce = 1
    o = _make_live_oracle(chain=chain)
    o._tick()
    assert len(chain.credit_calls) == 1
    assert len(chain.faucet_calls) == 1
    assert len(chain.posts) == 1
