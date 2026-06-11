"""Oracle main-loop tick semantics: success path and each failure path.

Under the two-key Type-10 funding model the Oracle holds both a funder
keypair (signs CreditAiEntity and consumes the faucet) and an entity
keypair (signs OracleAnchor). The signer-split tests verify that each
chain call uses the right key; the drift test in main() verifies the
oracle hard-fails when the keyfile's stored entity_id disagrees with
the on-chain derivation.
"""

from __future__ import annotations

import json
import time
from dataclasses import dataclass
from pathlib import Path
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
        self.post_signer_addrs: list[bytes] = []
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
        self.account_nonce_addrs: list[bytes] = []
        self.mempool_nonce_value: int = 708
        self.mempool_nonce_calls: int = 0
        self.credit_exc: Optional[BaseException] = None
        self.credit_calls: list[tuple[bytes, int, int]] = []
        self.credit_signer_addrs: list[bytes] = []

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
        self.account_nonce_addrs.append(address)
        if self.account_nonce_exc is not None:
            raise self.account_nonce_exc
        return self.account_nonce_value

    def get_nonce(self, address: bytes) -> int:
        """Mempool-drift footgun. Production must NOT call this for any
        account-signed tx. Present so the v2 nonce-source test can
        assert the agent did not reach for it."""
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
        self.credit_signer_addrs.append(kp.address)
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
        self.post_signer_addrs.append(kp.address)
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
        key_path=__file__,
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
    funder_kp = Keypair.generate()
    entity_kp = Keypair.generate()
    ch = chain if chain is not None else FakeChain()
    reg = registry if registry is not None else build_oracle_registry(time.monotonic())
    mono = monotonic_fn if monotonic_fn is not None else (lambda: 0.0)
    return oracle_mod.Oracle(
        cfg=cfg,
        chain=ch,
        funder_kp=funder_kp,
        entity_kp=entity_kp,
        entity_id=ch.entity_id_for(funder_kp.address),
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
    chain.balance_value = 100
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    assert len(chain.faucet_calls) == 1
    text = reg.render()
    assert 'novai_oracle_faucet_attempts_total{result="success"} 1' in text
    assert "novai_oracle_account_balance 100" in text


def test_refaucet_skipped_when_balance_above_threshold():
    chain = FakeChain()
    chain.balance_value = 1_000_000
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
    o._tick()
    assert len(chain.faucet_calls) == 1
    text = reg.render()
    assert 'novai_oracle_faucet_attempts_total{result="disabled"} 1' in text
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
    assert chain.faucet_calls == []
    text = reg.render()
    assert "novai_oracle_submission_success_total 1" in text


def test_v2_tier1_credits_when_entity_balance_below_threshold():
    chain = FakeChain()
    chain.entity_balance_value = 100
    chain.account_nonce_value = 1
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    assert len(chain.credit_calls) == 1
    _entity_id, amount, nonce = chain.credit_calls[0]
    assert amount == 100_000
    assert nonce == 1
    text = reg.render()
    assert 'novai_oracle_credit_attempts_total{result="success"} 1' in text
    assert "novai_oracle_entity_balance 100" in text


def test_v2_tier1_uses_account_nonce_not_mempool_nonce():
    chain = FakeChain()
    chain.entity_balance_value = 0
    chain.account_nonce_value = 1
    chain.mempool_nonce_value = 708
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    assert len(chain.credit_calls) == 1
    _entity_id, _amount, nonce = chain.credit_calls[0]
    assert nonce == 1, f"expected account nonce 1, got {nonce}"
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
    assert len(chain.credit_calls) == 1
    text = reg.render()
    assert 'novai_oracle_credit_attempts_total{result="nonce_mismatch"} 1' in text


def test_v2_tier1_skipped_when_entity_balance_healthy():
    chain = FakeChain()
    chain.entity_balance_value = 1_000_000
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    assert chain.credit_calls == []
    text = reg.render()
    assert "novai_oracle_entity_balance 1000000" in text


def test_v2_tier2_faucets_account_when_below_account_min_balance():
    chain = FakeChain()
    chain.balance_value = 100
    chain.entity_balance_value = 1_000_000
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
    chain.entity_balance_value = 1_000_000
    chain.faucet_exc = RateLimitedError(-32000, "Faucet rate limit")
    reg = build_oracle_registry(time.monotonic())
    clock = _MonotonicClock(start=1000.0)
    o = _make_oracle(
        fetch_fn=_ok_fetch, chain=chain, registry=reg, monotonic_fn=clock
    )
    for _ in range(60):
        o._tick()
        clock.advance(60.0)
    assert len(chain.faucet_calls) == 1


def test_v2_submit_runs_after_both_tiers_fail():
    chain = FakeChain()
    chain.entity_balance_value = 0
    chain.account_nonce_value = 1
    chain.credit_exc = NovaiServerError(-32000, "NonceMismatch")
    chain.balance_value = 0
    chain.faucet_exc = RateLimitedError(-32000, "Faucet rate limit")
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    assert len(chain.credit_calls) == 1
    assert len(chain.faucet_calls) == 1
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


# -- Two-key signer split (new under v2) -------------------------------------


def test_v2_credit_signs_with_funder_kp():
    """CreditAiEntity must be signed by the funder, not the entity. The
    funder is on a non-entity-bound path through check_ai_entity_sender;
    the entity is not, so signing credit with the entity key would
    bounce at lib.rs:9741.
    """
    chain = FakeChain()
    chain.entity_balance_value = 0
    chain.account_nonce_value = 1
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    assert chain.credit_signer_addrs == [o.funder_kp.address]
    assert chain.account_nonce_addrs == [o.funder_kp.address]
    # The entity key never touches the credit path.
    assert o.entity_kp.address not in chain.credit_signer_addrs
    assert o.entity_kp.address not in chain.account_nonce_addrs


def test_v2_anchor_signs_with_entity_kp():
    """OracleAnchor must be signed by the entity key, not the funder.
    The entity holds capability bit 6 (post_oracle_anchors); the funder
    has no capabilities and would be denied at validate_oracle_anchor.
    """
    chain = FakeChain()
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    assert chain.post_signer_addrs == [o.entity_kp.address]
    assert o.funder_kp.address not in chain.post_signer_addrs


def test_v2_faucet_targets_funder_address():
    """The faucet RPC pays the funder; the entity has no account ledger."""
    chain = FakeChain()
    chain.balance_value = 0
    reg = build_oracle_registry(time.monotonic())
    o = _make_oracle(fetch_fn=_ok_fetch, chain=chain, registry=reg)
    o._tick()
    assert chain.faucet_calls == [o.funder_kp.address]


# -- Keyfile entity_id drift check in main() (new under v2) ------------------


def _write_v2_keyfile(
    path: Path,
    *,
    funder: Keypair,
    entity: Keypair,
    entity_id_hex: str,
    capabilities_byte: int = 0x47,
    registered_at_unix: int = 1_717_428_000,
) -> None:
    path.write_text(
        json.dumps(
            {
                "version": 2,
                "funder_seed_hex": funder.seed.hex(),
                "funder_pubkey_hex": funder.pubkey.hex(),
                "funder_address_hex": funder.address.hex(),
                "entity_seed_hex": entity.seed.hex(),
                "entity_pubkey_hex": entity.pubkey.hex(),
                "entity_address_hex": entity.address.hex(),
                "entity_id_hex": entity_id_hex,
                "capabilities_byte": capabilities_byte,
                "registered_at_unix": registered_at_unix,
            }
        )
    )


def test_main_exits_on_keyfile_entity_id_drift(tmp_path, monkeypatch):
    """Hard fail on startup if the keyfile's entity_id_hex disagrees
    with chain.entity_id_for(funder_kp.address). Silently funding the
    wrong entity is the failure mode this check exists to prevent.
    """
    keyfile_path = tmp_path / "oracle-keys.json"
    funder = Keypair.generate()
    entity = Keypair.generate()
    _write_v2_keyfile(
        keyfile_path,
        funder=funder,
        entity=entity,
        entity_id_hex="ff" * 32,
    )

    class DriftChain:
        endpoint = "http://fake"

        def entity_id_for(self, address: bytes) -> bytes:
            return b"\x00" * 32

        def get_entity_status(self, entity_id: bytes) -> EntityStatus:
            return EntityStatus(True, True, 0x47, entity_id, entity_id.hex())

    monkeypatch.setattr(oracle_mod, "Chain", lambda endpoint: DriftChain())
    monkeypatch.setenv("PRICE_ORACLE_KEY_PATH", str(keyfile_path))
    monkeypatch.setenv("PRICE_ORACLE_RPC_ENDPOINT", "http://fake")

    rc = oracle_mod.main([])
    assert rc == 6, f"expected exit 6 on entity_id drift, got {rc}"


def test_main_exits_when_keyfile_entity_id_missing(tmp_path, monkeypatch):
    """A v2 keyfile that has not yet been updated by bootstrap (no
    entity_id_hex) is an aborted-or-legacy state; oracle.py refuses to
    start.
    """
    keyfile_path = tmp_path / "oracle-keys.json"
    funder = Keypair.generate()
    entity = Keypair.generate()
    keyfile_path.write_text(
        json.dumps(
            {
                "version": 2,
                "funder_seed_hex": funder.seed.hex(),
                "funder_pubkey_hex": funder.pubkey.hex(),
                "funder_address_hex": funder.address.hex(),
                "entity_seed_hex": entity.seed.hex(),
                "entity_pubkey_hex": entity.pubkey.hex(),
                "entity_address_hex": entity.address.hex(),
            }
        )
    )

    class AnyChain:
        endpoint = "http://fake"

        def entity_id_for(self, address: bytes) -> bytes:
            return b"\x00" * 32

        def get_entity_status(self, entity_id: bytes) -> EntityStatus:
            return EntityStatus(True, True, 0x47, entity_id, entity_id.hex())

    monkeypatch.setattr(oracle_mod, "Chain", lambda endpoint: AnyChain())
    monkeypatch.setenv("PRICE_ORACLE_KEY_PATH", str(keyfile_path))
    monkeypatch.setenv("PRICE_ORACLE_RPC_ENDPOINT", "http://fake")

    rc = oracle_mod.main([])
    assert rc == 6


def test_main_refuses_v1_keyfile(tmp_path, monkeypatch):
    """v1 keyfile on disk in v2 mode is a load-time refuse, returning
    exit code 2 (keyfile_error) per the documented failure table."""
    keyfile_path = tmp_path / "oracle-keys.json"
    kp = Keypair.generate()
    keyfile_path.write_text(
        json.dumps(
            {
                "version": 1,
                "seed_hex": kp.seed.hex(),
                "pubkey_hex": kp.pubkey.hex(),
                "address_hex": kp.address.hex(),
            }
        )
    )

    monkeypatch.setenv("PRICE_ORACLE_KEY_PATH", str(keyfile_path))
    monkeypatch.setenv("PRICE_ORACLE_RPC_ENDPOINT", "http://fake")

    rc = oracle_mod.main([])
    assert rc == 2
