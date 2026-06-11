"""Oracle main-loop tick semantics: success path and each failure path."""

from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Optional

from novai_sdk import (
    FeeTooLowError,
    Keypair,
    NonceTooLowError,
    RateLimitedError,
    ServerError as NovaiServerError,
)

import oracle as oracle_mod
from lib.chain import EntityStatus, map_credit_error, map_submit_error
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


@dataclass
class _FaucetResult:
    txid: str = "deadbeef"
    amount: str = "10000000"


class _MonotonicClock:
    """Injectable monotonic time source for backoff tests."""

    def __init__(self, start: float = 1000.0) -> None:
        self.t = start

    def __call__(self) -> float:
        return self.t

    def advance(self, secs: float) -> None:
        self.t += secs


class FakeChain:
    def __init__(self) -> None:
        self.posts: list[tuple[bytes, bytes, int, str]] = []
        self.post_exc: Optional[BaseException] = None
        self.height_value: Optional[int] = 12_345
        self.endpoint = "http://fake"
        self.balance_value: int = 10_000_000
        self.balance_exc: Optional[BaseException] = None
        self.faucet_exc: Optional[BaseException] = None
        self.faucet_calls: list[bytes] = []
        self.get_balance_calls: int = 0
        # v2: entity ledger + credit + nonce stubs.
        self.entity_balance_value: int = 10_000_000
        self.entity_balance_exc: Optional[BaseException] = None
        self.entity_balance_calls: int = 0
        self.account_nonce_value: int = 0
        self.account_nonce_exc: Optional[BaseException] = None
        self.account_nonce_calls: int = 0
        self.mempool_nonce_value: int = 708
        self.mempool_nonce_calls: int = 0
        self.credit_exc: Optional[BaseException] = None
        self.credit_calls: list[tuple[bytes, int, int]] = []

    def entity_id_for(self, address: bytes) -> bytes:
        return b"E" * 32

    def get_entity_status(self, entity_id: bytes) -> EntityStatus:
        return EntityStatus(True, True, 0x47, entity_id, entity_id.hex())

    def get_balance(self, address: bytes) -> int:
        self.get_balance_calls += 1
        if self.balance_exc is not None:
            raise self.balance_exc
        return self.balance_value

    def get_entity_economic_balance(self, entity_id: bytes) -> int:
        self.entity_balance_calls += 1
        if self.entity_balance_exc is not None:
            raise self.entity_balance_exc
        return self.entity_balance_value

    def get_account_nonce(self, address: bytes) -> int:
        self.account_nonce_calls += 1
        if self.account_nonce_exc is not None:
            raise self.account_nonce_exc
        return self.account_nonce_value

    def get_nonce(self, address: bytes) -> int:
        """Mempool-drift footgun. Production must NOT call this for any
        account-signed tx. Present so the v2 nonce-source test can assert
        the agent did not reach for it."""
        self.mempool_nonce_calls += 1
        return self.mempool_nonce_value

    def credit_entity(
        self,
        kp: Keypair,
        entity_id: bytes,
        amount: int,
        *,
        nonce: int,
        fee: int = 100,
    ) -> _SubmissionResult:
        self.credit_calls.append((entity_id, amount, nonce))
        if self.credit_exc is not None:
            raise self.credit_exc
        return _SubmissionResult(txid="creditxid")

    def faucet(self, address: bytes) -> _FaucetResult:
        self.faucet_calls.append(address)
        if self.faucet_exc is not None:
            raise self.faucet_exc
        return _FaucetResult()

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
    monotonic_fn=None,
    entity_min_balance: int = 5_000,
    account_min_balance: int = 200_000,
    credit_amount: int = 100_000,
    credit_retry_after_secs: float = 300.0,
    faucet_retry_after_secs: float = 3600.0,
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
        entity_min_balance=entity_min_balance,
        account_min_balance=account_min_balance,
        credit_amount=credit_amount,
        credit_retry_after_secs=credit_retry_after_secs,
        faucet_retry_after_secs=faucet_retry_after_secs,
    )
    kp = Keypair.generate()
    ch = chain if chain is not None else FakeChain()
    reg = registry if registry is not None else build_oracle_registry(time.monotonic())
    mono = monotonic_fn if monotonic_fn is not None else (lambda: 0.0)
    return oracle_mod.Oracle(
        cfg=cfg,
        chain=ch,
        kp=kp,
        entity_id=ch.entity_id_for(kp.address),
        registry=reg,
        fetch_fn=fetch_fn,
        sleep_fn=lambda _s: None,
        time_fn=time_fn,
        monotonic_fn=mono,
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


def test_refaucet_triggers_when_balance_below_threshold():
    chain = FakeChain()
    chain.balance_value = 100  # well below the 200_000 account threshold
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    assert len(chain.faucet_calls) == 1
    text = reg.render()
    assert 'novai_oracle_faucet_attempts_total{result="success"} 1' in text
    assert "novai_oracle_account_balance 100" in text


def test_refaucet_skipped_when_balance_above_threshold():
    chain = FakeChain()
    chain.balance_value = 1_000_000  # well above the 200_000 account threshold
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    assert chain.faucet_calls == []
    text = reg.render()
    assert "novai_oracle_account_balance 1000000" in text


def test_cooldown_blocked_oracle_does_not_spin_faucet():
    chain = FakeChain()
    chain.balance_value = 0
    chain.faucet_exc = RateLimitedError(
        -32000, "Faucet rate limit: try again in 3600 seconds"
    )
    reg = build_oracle_registry(time.monotonic())
    clock = _MonotonicClock(start=1000.0)
    o = _make_oracle(
        fetch_fn=_ok_fetch, chain=chain, registry=reg, monotonic_fn=clock
    )
    for _ in range(60):
        o._tick()
        clock.advance(60.0)
    # Across 60 ticks the oracle made exactly one faucet RPC call.
    assert len(chain.faucet_calls) == 1
    text = reg.render()
    assert 'novai_oracle_faucet_attempts_total{result="rate_limited"} 1' in text


def test_cooldown_boundary_allows_one_more_faucet_attempt():
    chain = FakeChain()
    chain.balance_value = 0
    chain.faucet_exc = RateLimitedError(-32000, "Faucet rate limit")
    reg = build_oracle_registry(time.monotonic())
    clock = _MonotonicClock(start=1000.0)
    o = _make_oracle(
        fetch_fn=_ok_fetch, chain=chain, registry=reg, monotonic_fn=clock
    )
    o._tick()
    assert len(chain.faucet_calls) == 1
    # Advance past the 3600s backoff window.
    clock.advance(3601.0)
    o._tick()
    assert len(chain.faucet_calls) == 2


def test_faucet_disabled_does_not_sys_exit_and_sets_backoff():
    chain = FakeChain()
    chain.balance_value = 0
    chain.faucet_exc = NovaiServerError(
        -32000, "Faucet disabled. Use --faucet-key <path> or --dev-keys to enable."
    )
    reg = build_oracle_registry(time.monotonic())
    clock = _MonotonicClock(start=1000.0)
    o = _make_oracle(
        fetch_fn=_ok_fetch, chain=chain, registry=reg, monotonic_fn=clock
    )
    o._tick()  # must not raise SystemExit
    assert len(chain.faucet_calls) == 1
    text = reg.render()
    assert 'novai_oracle_faucet_attempts_total{result="disabled"} 1' in text
    # Backoff was stamped: a second tick at the same monotonic time skips the faucet.
    o._tick()
    assert len(chain.faucet_calls) == 1


def test_submit_still_runs_when_below_threshold_and_cooldown_blocked():
    chain = FakeChain()
    chain.balance_value = 0
    chain.faucet_exc = RateLimitedError(-32000, "Faucet rate limit")
    chain.post_exc = NovaiServerError(
        -32000, "InsufficientFunds { balance: 0, needed: 1000 }"
    )
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    text = reg.render()
    assert (
        'novai_oracle_submission_failure_total{reason="insufficient_funds"} 1' in text
    )
    assert len(chain.faucet_calls) == 1


def test_map_submit_error_maps_insufficient_funds():
    exc = NovaiServerError(-32000, "InsufficientFunds { balance: 0, needed: 1000 }")
    assert map_submit_error(exc) == "insufficient_funds"


def test_get_balance_failure_skips_topup_but_does_not_block_submit():
    chain = FakeChain()
    chain.balance_exc = OSError("network down")
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    # No faucet call because the balance read failed first.
    assert chain.faucet_calls == []
    # Submit still ran (FakeChain.post_anchor has no post_exc).
    text = reg.render()
    assert "novai_oracle_submission_success_total 1" in text


def test_v2_tier1_credits_when_entity_balance_below_threshold():
    chain = FakeChain()
    chain.entity_balance_value = 100  # well below default 5_000
    chain.account_nonce_value = 1
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    assert len(chain.credit_calls) == 1
    _entity_id, amount, nonce = chain.credit_calls[0]
    assert amount == 100_000  # default credit_amount
    assert nonce == 1
    text = reg.render()
    assert 'novai_oracle_credit_attempts_total{result="success"} 1' in text
    assert "novai_oracle_entity_balance 100" in text


def test_v2_tier1_uses_account_nonce_not_mempool_nonce():
    chain = FakeChain()
    chain.entity_balance_value = 0  # tier 1 fires
    chain.account_nonce_value = 1  # on-chain account.nonce
    chain.mempool_nonce_value = 708  # drifted mempool expected
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    assert len(chain.credit_calls) == 1
    _entity_id, _amount, nonce = chain.credit_calls[0]
    # The credit MUST use account.nonce (1), not the mempool's drifted value (708).
    assert nonce == 1, f"expected account nonce 1, got {nonce}"
    # And the agent must not have asked for the mempool nonce at all.
    assert chain.mempool_nonce_calls == 0


def test_v2_tier1_does_not_spin_on_repeated_failure():
    chain = FakeChain()
    chain.entity_balance_value = 0
    chain.account_nonce_value = 1
    chain.credit_exc = NovaiServerError(
        -32000, "NonceMismatch { expected: 1, got: 708 }"
    )
    reg = build_oracle_registry(time.monotonic())
    clock = _MonotonicClock(start=1000.0)
    o = _make_oracle(
        fetch_fn=_ok_fetch, chain=chain, registry=reg, monotonic_fn=clock
    )
    for _ in range(5):
        o._tick()
        clock.advance(60.0)
    # Default credit_retry_after_secs is 300; after 5 ticks (240s simulated) backoff still holds.
    assert len(chain.credit_calls) == 1
    text = reg.render()
    assert 'novai_oracle_credit_attempts_total{result="nonce_mismatch"} 1' in text


def test_v2_tier1_skipped_when_entity_balance_healthy():
    chain = FakeChain()
    chain.entity_balance_value = 1_000_000  # well above default 5_000
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    assert chain.credit_calls == []
    text = reg.render()
    assert "novai_oracle_entity_balance 1000000" in text


def test_v2_tier2_faucets_account_when_below_account_min_balance():
    chain = FakeChain()
    chain.balance_value = 100  # below default 200_000
    chain.entity_balance_value = 1_000_000  # above 5_000, tier 1 skipped
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    assert len(chain.faucet_calls) == 1
    text = reg.render()
    assert 'novai_oracle_faucet_attempts_total{result="success"} 1' in text
    assert "novai_oracle_account_balance 100" in text


def test_v2_tier2_does_not_spin():
    chain = FakeChain()
    chain.balance_value = 0
    chain.entity_balance_value = 1_000_000  # tier 1 skipped
    chain.faucet_exc = RateLimitedError(-32000, "Faucet rate limit")
    reg = build_oracle_registry(time.monotonic())
    clock = _MonotonicClock(start=1000.0)
    o = _make_oracle(
        fetch_fn=_ok_fetch, chain=chain, registry=reg, monotonic_fn=clock
    )
    for _ in range(60):
        o._tick()
        clock.advance(60.0)
    # 3600s default backoff; 60 ticks * 60s = 3540s, still in window.
    assert len(chain.faucet_calls) == 1


def test_v2_submit_runs_after_both_tiers_fail():
    chain = FakeChain()
    chain.entity_balance_value = 0  # tier 1 fires
    chain.account_nonce_value = 1
    chain.credit_exc = NovaiServerError(-32000, "NonceMismatch")
    chain.balance_value = 0  # tier 2 fires
    chain.faucet_exc = RateLimitedError(-32000, "Faucet rate limit")
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    # Both tiers attempted.
    assert len(chain.credit_calls) == 1
    assert len(chain.faucet_calls) == 1
    # And _submit still ran (no post_exc, so it succeeds).
    assert len(chain.posts) == 1
    text = reg.render()
    assert "novai_oracle_submission_success_total 1" in text


def test_v2_map_credit_error_maps_nonce_mismatch_and_insufficient_funds():
    nm = NovaiServerError(-32000, "NonceMismatch { expected: 1, got: 708 }")
    insuff = NovaiServerError(
        -32000, "InsufficientFunds { balance: 0, needed: 100100 }"
    )
    assert map_credit_error(nm) == "nonce_mismatch"
    assert map_credit_error(insuff) == "insufficient_funds"
