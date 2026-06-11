#!/usr/bin/env python3
"""PURPOSE: Long-running NOVAI price-oracle main loop.

Two-key Type-10 funding model: the funder keypair signs account-level
operations (CreditAiEntity top-ups, faucet target). The entity keypair
signs entity-level signals (OracleAnchor commitments). See
docs/AGENT_FUNDING_PLAYBOOK.md for the reusable lifecycle and
docs/gate-oracle-funding-model-diagnosis.md for why the funder must be
on a non-entity-bound path.

Every PRICE_ORACLE_LOOP_INTERVAL_SECS seconds:
  1. Fetch BTC/USD spot from CoinGecko (urllib).
  2. Build a deterministic data_hash from (price, timestamp).
  3. (Tier 1) If entity.economic_balance is low, CreditAiEntity from
     the funder.
  4. (Tier 2) If funder.balance is low, faucet the funder.
  5. Submit an OracleAnchor signal signed by the entity key.
  6. Update Prometheus metrics on localhost:9201.
  7. Sleep, interruptibly.

INVARIANTS:
- The loop never crashes on a recoverable error; only on signature /
  encoding bugs (which would not be recoverable by re-running).
- SIGTERM / SIGINT cause a clean exit within ~1 second via a sliced
  sleep loop.
- Metrics increments correspond 1:1 to the events they describe; no
  silent failures.
- The keyfile's entity_id_hex is the authority for self.entity_id; the
  derivation from funder address is a sanity check that must match.
  Disagreement is a hard fail at startup (exit 6), not a warning.

FAILURE MODES:
- Missing keyfile / endpoint -> fatal at startup (exit non-zero, systemd
  restarts after RestartSec).
- v1 keyfile or seed-derived address mismatch -> exit 2.
- Entity not registered with bit 6 -> exit 3.
- Metrics port bind failed -> exit 4.
- Keyfile entity_id missing or disagrees with derived -> exit 6.
- All other tick-level failures increment a metric and continue.
"""

from __future__ import annotations

import json
import logging
import os
import signal
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

from novai_sdk import Keypair

from lib.chain import Chain, map_credit_error, map_faucet_error, map_submit_error
from lib.coingecko import (
    BackoffState,
    CoinGeckoError,
    NetworkError,
    ParseError,
    PriceObservation,
    RateLimitError,
    ServerError,
    fetch_btc_usd,
)
from lib.log import configure_logging
from lib.metrics import MetricsRegistry, build_oracle_registry, start_metrics_server
from lib.signal import DATA_TAG, build_data_hash

LOG = logging.getLogger("price_oracle.oracle")

DEFAULTS = {
    "PRICE_ORACLE_RPC_ENDPOINT": "http://localhost:3030",
    "PRICE_ORACLE_KEY_PATH": "/etc/novai/oracle-keys.json",
    "PRICE_ORACLE_COINGECKO_URL": (
        "https://api.coingecko.com/api/v3/simple/price?ids=bitcoin&vs_currencies=usd"
    ),
    "PRICE_ORACLE_METRICS_HOST": "127.0.0.1",
    "PRICE_ORACLE_METRICS_PORT": "9201",
    "PRICE_ORACLE_LOOP_INTERVAL_SECS": "60",
    "PRICE_ORACLE_HTTP_TIMEOUT_SECS": "10",
    "PRICE_ORACLE_DATA_TAG": DATA_TAG,
    "PRICE_ORACLE_LOG_LEVEL": "INFO",
    "PRICE_ORACLE_ENTITY_MIN_BALANCE": "5000",
    "PRICE_ORACLE_ACCOUNT_MIN_BALANCE": "200000",
    "PRICE_ORACLE_MIN_BALANCE": "200000",  # legacy alias of PRICE_ORACLE_ACCOUNT_MIN_BALANCE
    "PRICE_ORACLE_CREDIT_AMOUNT": "100000",
    "PRICE_ORACLE_CREDIT_RETRY_AFTER_SECS": "300",
    "PRICE_ORACLE_FAUCET_RETRY_AFTER_SECS": "3600",
}


@dataclass(frozen=True)
class OracleConfig:
    endpoint: str
    key_path: Path
    coingecko_url: str
    metrics_host: str
    metrics_port: int
    loop_interval_secs: float
    http_timeout_secs: float
    data_tag: str
    log_level: str
    entity_min_balance: int = 5_000
    account_min_balance: int = 200_000
    credit_amount: int = 100_000
    credit_retry_after_secs: float = 300.0
    faucet_retry_after_secs: float = 3600.0

    @classmethod
    def from_env(cls, env: dict[str, str] | None = None) -> "OracleConfig":
        env = env if env is not None else dict(os.environ)

        def get(key: str) -> str:
            return env.get(key, DEFAULTS[key])

        # Backward-compat alias: prefer PRICE_ORACLE_ACCOUNT_MIN_BALANCE (v2),
        # fall back to the v1 name PRICE_ORACLE_MIN_BALANCE with a one-line
        # deprecation warning so existing systemd env files keep working.
        if "PRICE_ORACLE_ACCOUNT_MIN_BALANCE" in env:
            account_min_balance = _env_int("PRICE_ORACLE_ACCOUNT_MIN_BALANCE", env)
        elif "PRICE_ORACLE_MIN_BALANCE" in env:
            account_min_balance = _env_int("PRICE_ORACLE_MIN_BALANCE", env)
            LOG.warning(
                "config event=deprecated_alias key=PRICE_ORACLE_MIN_BALANCE "
                "use=PRICE_ORACLE_ACCOUNT_MIN_BALANCE value=%d",
                account_min_balance,
            )
        else:
            account_min_balance = int(DEFAULTS["PRICE_ORACLE_ACCOUNT_MIN_BALANCE"])

        return cls(
            endpoint=get("PRICE_ORACLE_RPC_ENDPOINT"),
            key_path=Path(get("PRICE_ORACLE_KEY_PATH")),
            coingecko_url=get("PRICE_ORACLE_COINGECKO_URL"),
            metrics_host=get("PRICE_ORACLE_METRICS_HOST"),
            metrics_port=_env_int("PRICE_ORACLE_METRICS_PORT", env),
            loop_interval_secs=_env_float("PRICE_ORACLE_LOOP_INTERVAL_SECS", env),
            http_timeout_secs=_env_float("PRICE_ORACLE_HTTP_TIMEOUT_SECS", env),
            data_tag=get("PRICE_ORACLE_DATA_TAG"),
            log_level=get("PRICE_ORACLE_LOG_LEVEL"),
            entity_min_balance=_env_int("PRICE_ORACLE_ENTITY_MIN_BALANCE", env),
            account_min_balance=account_min_balance,
            credit_amount=_env_int("PRICE_ORACLE_CREDIT_AMOUNT", env),
            credit_retry_after_secs=_env_float(
                "PRICE_ORACLE_CREDIT_RETRY_AFTER_SECS", env
            ),
            faucet_retry_after_secs=_env_float(
                "PRICE_ORACLE_FAUCET_RETRY_AFTER_SECS", env
            ),
        )


def _env_int(key: str, env: dict[str, str]) -> int:
    raw = env.get(key, DEFAULTS[key])
    try:
        return int(raw)
    except (TypeError, ValueError):
        LOG.warning("config event=bad_int key=%s value=%s falling_back=%s", key, raw, DEFAULTS[key])
        return int(DEFAULTS[key])


def _env_float(key: str, env: dict[str, str]) -> float:
    raw = env.get(key, DEFAULTS[key])
    try:
        return float(raw)
    except (TypeError, ValueError):
        LOG.warning("config event=bad_float key=%s value=%s falling_back=%s", key, raw, DEFAULTS[key])
        return float(DEFAULTS[key])


KEYFILE_VERSION_V2 = 2


def load_keypair_from_file(
    path: Path,
) -> tuple[Keypair, Keypair, dict[str, Any]]:
    """Return (funder_kp, entity_kp, keyfile_dict).

    Refuses any keyfile whose ``version`` is not 2. The v1 file is
    bound to the dead Type-8 entity; auto-migration would silently
    inherit a poisoned funder. Operator must archive and re-bootstrap.
    """
    data = json.loads(path.read_text(encoding="utf-8"))
    version = data.get("version")
    if version != KEYFILE_VERSION_V2:
        raise ValueError(
            f"keyfile {path} has version={version!r}, oracle requires "
            f"version={KEYFILE_VERSION_V2}; archive and re-run bootstrap"
        )
    funder_seed_hex = str(data["funder_seed_hex"])
    entity_seed_hex = str(data["entity_seed_hex"])
    for label, seed_hex in (("funder", funder_seed_hex), ("entity", entity_seed_hex)):
        if len(seed_hex) != 64:
            raise ValueError(
                f"{label}_seed_hex must be 64 hex chars, got {len(seed_hex)}"
            )
    funder_kp = Keypair.from_seed(bytes.fromhex(funder_seed_hex))
    entity_kp = Keypair.from_seed(bytes.fromhex(entity_seed_hex))
    if str(data.get("funder_address_hex", funder_kp.address.hex())) != funder_kp.address.hex():
        raise ValueError(
            "funder_address_hex in keyfile does not match seed; refusing to load"
        )
    if str(data.get("entity_address_hex", entity_kp.address.hex())) != entity_kp.address.hex():
        raise ValueError(
            "entity_address_hex in keyfile does not match seed; refusing to load"
        )
    return funder_kp, entity_kp, data


class Oracle:
    """SIGTERM-aware main loop for the price oracle."""

    def __init__(
        self,
        cfg: OracleConfig,
        chain: Chain,
        funder_kp: Keypair,
        entity_kp: Keypair,
        entity_id: bytes,
        registry: MetricsRegistry,
        *,
        fetch_fn: Callable[[str, float], PriceObservation] = fetch_btc_usd,
        sleep_fn: Callable[[float], None] = time.sleep,
        time_fn: Callable[[], float] = time.time,
        monotonic_fn: Callable[[], float] = time.monotonic,
    ) -> None:
        self.cfg = cfg
        self.chain = chain
        # funder_kp: signs account-level ops (CreditAiEntity, faucet
        # target). Never signs entity-bound signals; that would route
        # through check_ai_entity_sender's deny arm at lib.rs:9741.
        # entity_kp: signs SignalCommitment carrying OracleAnchor.
        # Holds capability bit 6 (post_oracle_anchors); the funder does
        # not.
        self.funder_kp = funder_kp
        self.entity_kp = entity_kp
        self.entity_id = entity_id
        self.registry = registry
        self.fetch_fn = fetch_fn
        self.sleep_fn = sleep_fn
        self.time_fn = time_fn
        self.monotonic_fn = monotonic_fn
        self.backoff = BackoffState()
        self._stopping = False
        self._next_credit_attempt_at: float | None = None
        self._next_faucet_attempt_at: float | None = None
        signal.signal(signal.SIGTERM, self._on_signal)
        signal.signal(signal.SIGINT, self._on_signal)

    def _on_signal(self, signum: int, _frame: object) -> None:
        LOG.info("oracle event=signal signum=%d stopping=true", signum)
        self._stopping = True

    def run_forever(self) -> int:
        LOG.info(
            "oracle_start event=running endpoint=%s entity_id=%s tag=%s interval=%.1f",
            self.cfg.endpoint,
            self.entity_id.hex(),
            self.cfg.data_tag,
            self.cfg.loop_interval_secs,
        )
        while not self._stopping:
            try:
                self._tick()
            except Exception:  # noqa: BLE001 last-ditch
                LOG.exception("tick event=exception")
            self.registry.set_gauge(
                "novai_oracle_last_loop_completed_timestamp", self.time_fn()
            )
            sleep_secs = self._next_sleep_secs()
            self._sliced_sleep(sleep_secs)
        LOG.info("oracle_stop event=clean")
        return 0

    def _next_sleep_secs(self) -> float:
        if self.backoff.index > 0:
            return float(self.backoff.LADDER[min(self.backoff.index, len(self.backoff.LADDER) - 1)])
        return float(self.cfg.loop_interval_secs)

    def _sliced_sleep(self, total_secs: float) -> None:
        slept = 0.0
        slice_secs = 1.0
        while slept < total_secs and not self._stopping:
            remaining = total_secs - slept
            chunk = remaining if remaining < slice_secs else slice_secs
            self.sleep_fn(chunk)
            slept += chunk

    def _tick(self) -> None:
        obs = self._fetch_price()
        if obs is None:
            return
        self.registry.set_gauge("novai_oracle_last_price_usd", obs.price)
        self._maybe_top_up()
        self._submit(obs)

    def _maybe_top_up(self) -> None:
        """Two-tier best-effort balance management.

        Tier 1 (_maybe_credit_entity) reads entity.economic_balance and
        submits a CreditAiEntity tx from the account when the entity is
        low. Tier 2 (_maybe_faucet_account) reads account.balance and
        calls novai_faucet when the account is low. Both tiers are
        best-effort and never raise. _submit runs after both regardless.
        See docs/gate-oracle-balance-diagnosis-v2.md for why the two
        ledgers are separate and why this order matters.
        """
        self._maybe_credit_entity()
        self._maybe_faucet_account()

    def _maybe_credit_entity(self) -> None:
        """Tier 1: read entity.economic_balance and CreditAiEntity if low.

        Never raises. Never calls sys.exit. Every credit-attempt exit
        path (success, nonce mismatch, insufficient funds, RPC error,
        nonce-read failure) stamps self._next_credit_attempt_at to
        monotonic_now + cfg.credit_retry_after_secs so a stuck oracle
        cannot spin the credit RPC.

        Critical nonce-source detail: account.nonce comes from
        novai_getBalance (chain.get_account_nonce), NOT from
        novai_getNonce (mempool's expected_nonce). The chain's
        apply_credit_ai_entity_tx at crates/execution/src/lib.rs:9226
        uses exact equality against the on-chain account.nonce. The
        mempool cache advances on every committed tx (success or fail)
        per crates/node/src/main.rs:183-201, so its expected_nonce
        drifts away from account.nonce whenever entity-signed signals
        are interleaved (the oracle's case).
        """
        try:
            entity_balance = self.chain.get_entity_economic_balance(self.entity_id)
        except Exception as exc:  # noqa: BLE001 best-effort
            LOG.warning("entity_balance_read event=failed error=%s", exc)
            return

        self.registry.set_gauge("novai_oracle_entity_balance", float(entity_balance))

        if entity_balance >= self.cfg.entity_min_balance:
            return

        now_mono = self.monotonic_fn()
        if (
            self._next_credit_attempt_at is not None
            and now_mono < self._next_credit_attempt_at
        ):
            retry_in = self._next_credit_attempt_at - now_mono
            LOG.info(
                "credit event=skipped reason=backoff retry_in_secs=%.0f "
                "entity_balance=%d threshold=%d",
                retry_in,
                entity_balance,
                self.cfg.entity_min_balance,
            )
            return

        try:
            account_nonce = self.chain.get_account_nonce(self.funder_kp.address)
        except Exception as exc:  # noqa: BLE001 mapped by map_credit_error
            reason = map_credit_error(exc)
            self._next_credit_attempt_at = (
                now_mono + self.cfg.credit_retry_after_secs
            )
            self.registry.inc_counter("novai_oracle_credit_attempts_total", reason)
            LOG.warning(
                "credit event=failed phase=nonce_read reason=%s exc=%s error=%s",
                reason,
                type(exc).__name__,
                exc,
            )
            return

        try:
            result = self.chain.credit_entity(
                self.funder_kp,
                self.entity_id,
                self.cfg.credit_amount,
                nonce=account_nonce,
            )
        except Exception as exc:  # noqa: BLE001 mapped by map_credit_error
            reason = map_credit_error(exc)
            self._next_credit_attempt_at = (
                now_mono + self.cfg.credit_retry_after_secs
            )
            self.registry.inc_counter("novai_oracle_credit_attempts_total", reason)
            LOG.warning(
                "credit event=failed phase=submit reason=%s exc=%s error=%s "
                "entity_balance=%d account_nonce=%d",
                reason,
                type(exc).__name__,
                exc,
                entity_balance,
                account_nonce,
            )
            return

        self._next_credit_attempt_at = now_mono + self.cfg.credit_retry_after_secs
        self.registry.inc_counter("novai_oracle_credit_attempts_total", "success")
        LOG.info(
            "credit event=requested txid=%s amount=%d entity_balance_before=%d "
            "account_nonce=%d threshold=%d",
            result.txid,
            self.cfg.credit_amount,
            entity_balance,
            account_nonce,
            self.cfg.entity_min_balance,
        )

    def _maybe_faucet_account(self) -> None:
        """Tier 2: read account.balance and faucet when low.

        Never raises. Never calls sys.exit. Every faucet-attempt exit
        path stamps self._next_faucet_attempt_at to
        monotonic_now + cfg.faucet_retry_after_secs. The 3600s default
        matches FAUCET_PER_ADDRESS_COOLDOWN_SECS at
        crates/node/src/rpc.rs:3125.
        """
        try:
            balance = self.chain.get_balance(self.funder_kp.address)
        except Exception as exc:  # noqa: BLE001 best-effort
            LOG.warning("account_balance_read event=failed error=%s", exc)
            return

        self.registry.set_gauge("novai_oracle_account_balance", float(balance))

        if balance >= self.cfg.account_min_balance:
            return

        now_mono = self.monotonic_fn()
        if (
            self._next_faucet_attempt_at is not None
            and now_mono < self._next_faucet_attempt_at
        ):
            retry_in = self._next_faucet_attempt_at - now_mono
            LOG.info(
                "faucet event=skipped reason=backoff retry_in_secs=%.0f "
                "balance=%d threshold=%d",
                retry_in,
                balance,
                self.cfg.account_min_balance,
            )
            return

        try:
            result = self.chain.faucet(self.funder_kp.address)
        except Exception as exc:  # noqa: BLE001 mapped by map_faucet_error
            reason = map_faucet_error(exc)
            self._next_faucet_attempt_at = (
                now_mono + self.cfg.faucet_retry_after_secs
            )
            self.registry.inc_counter("novai_oracle_faucet_attempts_total", reason)
            LOG.warning(
                "faucet event=failed reason=%s exc=%s error=%s balance=%d",
                reason,
                type(exc).__name__,
                exc,
                balance,
            )
            return

        self._next_faucet_attempt_at = now_mono + self.cfg.faucet_retry_after_secs
        self.registry.inc_counter("novai_oracle_faucet_attempts_total", "success")
        LOG.info(
            "faucet event=requested txid=%s amount=%s balance_before=%d threshold=%d",
            result.txid,
            result.amount,
            balance,
            self.cfg.account_min_balance,
        )

    def _fetch_price(self) -> PriceObservation | None:
        try:
            obs = self.fetch_fn(self.cfg.coingecko_url, self.cfg.http_timeout_secs)
        except RateLimitError as exc:
            self.registry.inc_counter("novai_oracle_price_fetch_failure_total", "rate_limit")
            delay = self.backoff.on_rate_limit()
            LOG.warning(
                "fetch event=rate_limited next_backoff_secs=%.0f retry_after_hint=%.0f",
                delay,
                exc.retry_after_secs,
            )
            return None
        except ServerError as exc:
            self.registry.inc_counter("novai_oracle_price_fetch_failure_total", "server_error")
            LOG.warning("fetch event=server_error status=%d", exc.status)
            return None
        except NetworkError as exc:
            self.registry.inc_counter("novai_oracle_price_fetch_failure_total", "network_error")
            LOG.warning("fetch event=network_error error=%s", exc)
            return None
        except ParseError as exc:
            self.registry.inc_counter("novai_oracle_price_fetch_failure_total", "parse_error")
            LOG.warning("fetch event=parse_error error=%s", exc)
            return None
        except CoinGeckoError as exc:
            self.registry.inc_counter("novai_oracle_price_fetch_failure_total", "network_error")
            LOG.warning("fetch event=unknown_error error=%s", exc)
            return None
        self.backoff.reset()
        self.registry.inc_counter("novai_oracle_price_fetch_success_total")
        LOG.info("fetch event=ok price_usd=%.2f", obs.price)
        return obs

    def _submit(self, obs: PriceObservation) -> None:
        ts = int(self.time_fn())
        try:
            data_hash = build_data_hash(obs.price, ts)
        except ValueError as exc:
            self.registry.inc_counter(
                "novai_oracle_submission_failure_total", "encoding_error"
            )
            LOG.error("submit event=encoding_error error=%s", exc)
            return
        try:
            result = self.chain.post_anchor(
                self.entity_kp,
                self.entity_id,
                data_hash,
                ts,
                self.cfg.data_tag,
            )
        except Exception as exc:  # noqa: BLE001 mapped by map_submit_error
            reason = map_submit_error(exc)
            self.registry.inc_counter("novai_oracle_submission_failure_total", reason)
            LOG.warning(
                "submit event=failed reason=%s exc=%s error=%s",
                reason,
                type(exc).__name__,
                exc,
            )
            return
        height = self._latest_height_safe()
        if height is not None:
            self.registry.set_gauge("novai_oracle_last_submission_height", height)
        self.registry.inc_counter("novai_oracle_submission_success_total")
        LOG.info(
            "submit event=ok txid=%s signal_hash=%s price=%.2f tag=%s ts=%d height=%s",
            result.txid,
            result.signal_hash,
            obs.price,
            self.cfg.data_tag,
            ts,
            "n/a" if height is None else height,
        )

    def _latest_height_safe(self) -> int | None:
        try:
            return self.chain.latest_block_height()
        except Exception as exc:  # noqa: BLE001 best-effort gauge
            LOG.debug("latest_block event=unavailable error=%s", exc)
            return None


def main(argv: list[str] | None = None) -> int:
    cfg = OracleConfig.from_env()
    configure_logging(cfg.log_level)
    LOG.info(
        "oracle_init event=loading endpoint=%s key_path=%s metrics=%s:%d",
        cfg.endpoint,
        cfg.key_path,
        cfg.metrics_host,
        cfg.metrics_port,
    )

    try:
        funder_kp, entity_kp, keyfile_data = load_keypair_from_file(cfg.key_path)
    except (OSError, json.JSONDecodeError, ValueError) as exc:
        LOG.error("oracle_init event=keyfile_error error=%s path=%s", exc, cfg.key_path)
        return 2

    # The keyfile's entity_id_hex is the authority. bootstrap.py
    # persists it at registration time, after the chain confirms the
    # entity exists with the right capabilities. The derivation below
    # is a sanity check that must match: disagreement means the keyfile
    # and funder have drifted (different registrations, swapped files,
    # or a manually-edited keyfile). Silently funding the wrong entity
    # is the bug class this check exists to prevent.
    expected_entity_id_hex = keyfile_data.get("entity_id_hex")
    if not expected_entity_id_hex:
        LOG.error(
            "oracle_init event=keyfile_missing_entity_id path=%s "
            "advice=re-run-bootstrap-to-register",
            cfg.key_path,
        )
        return 6
    try:
        expected_entity_id = bytes.fromhex(str(expected_entity_id_hex))
    except ValueError as exc:
        LOG.error(
            "oracle_init event=keyfile_entity_id_malformed value=%s error=%s",
            expected_entity_id_hex,
            exc,
        )
        return 6

    chain = Chain(cfg.endpoint)
    derived_entity_id = chain.entity_id_for(funder_kp.address)
    if expected_entity_id != derived_entity_id:
        LOG.error(
            "oracle_init event=entity_id_drift_fatal keyfile=%s derived=%s "
            "funder=%s advice=archive-keyfile-and-re-bootstrap",
            expected_entity_id_hex,
            derived_entity_id.hex(),
            funder_kp.address.hex(),
        )
        return 6

    entity_id = expected_entity_id

    status = chain.get_entity_status(entity_id)
    if not status.exists or not status.has_post_oracle_anchors:
        LOG.error(
            "oracle_init event=entity_not_ready exists=%s bit_6=%s caps=0x%02x",
            status.exists,
            status.has_post_oracle_anchors,
            status.capabilities,
        )
        return 3

    registry = build_oracle_registry(time.monotonic())
    try:
        start_metrics_server(cfg.metrics_host, cfg.metrics_port, registry)
    except OSError as exc:
        LOG.error(
            "metrics_server event=bind_failed host=%s port=%d error=%s",
            cfg.metrics_host,
            cfg.metrics_port,
            exc,
        )
        return 4

    oracle = Oracle(cfg, chain, funder_kp, entity_kp, entity_id, registry)
    return oracle.run_forever()


if __name__ == "__main__":
    sys.exit(main())
