#!/usr/bin/env python3
"""PURPOSE: Long-running NOVAI compute-oracle main loop.

The compute oracle observes GPU rental pricing from a public marketplace and
commits a reproducible hash of that observation to the chain as an OracleAnchor
signal (type 22). It can optionally emit a ReputationUpdate (type 7). It mirrors
the price-oracle structure (two-key Type-10 funding model, Prometheus metrics,
SIGTERM-aware sliced sleep) and adds a DRY_RUN mode that constructs and logs the
exact signal it would submit without touching the chain.

Every COMPUTE_ORACLE_LOOP_INTERVAL_SECS seconds:
  1. Fetch GPU pricing for the configured model (urllib).
  2. Build a deterministic data_hash from (model, price, timestamp).
  3. (live only) Two-tier balance top-up from the funder.
  4. Construct an OracleAnchor signal signed by the entity key.
     - DRY_RUN: log the constructed signal bytes; do not submit.
     - live: submit via the SDK client.
  5. Optionally construct a ReputationUpdate the same way.
  6. Update Prometheus metrics.
  7. Sleep, interruptibly.

INVARIANTS:
- DRY_RUN is the default. The loop never reaches the RPC client in dry_run.
- The loop never crashes on a recoverable error.
- SIGTERM / SIGINT cause a clean exit within ~1 second.
- The agent never posts a price it did not compute; missing data skips a cycle.

FAILURE MODES:
- live mode, missing keyfile / endpoint -> fatal at startup (exit non-zero).
- v1 keyfile or seed mismatch -> exit 2.
- live mode, entity not registered with bit 6 -> exit 3.
- metrics port bind failed -> exit 4.
- keyfile entity_id missing or disagrees with derived -> exit 6.
- All other tick-level failures increment a metric and continue.
"""

from __future__ import annotations

import json
import logging
import signal
import sys
import time
from pathlib import Path
from typing import Any, Callable

from novai_sdk import Keypair

from lib.chain import (
    Chain,
    DryRunResult,
    map_credit_error,
    map_faucet_error,
    map_submit_error,
)
from lib.config import ComputeOracleConfig
from lib.gpu_source import (
    BackoffState,
    GpuPriceObservation,
    GpuSourceError,
    NetworkError,
    NoDataError,
    ParseError,
    RateLimitError,
    ServerError,
    fetch_gpu_price,
)
from lib.log import configure_logging
from lib.metrics import (
    MetricsRegistry,
    build_compute_oracle_registry,
    start_metrics_server,
)
from lib.signal import build_data_hash, source_hash_for

LOG = logging.getLogger("compute_oracle.oracle")

KEYFILE_VERSION_V2 = 2


def load_keypair_from_file(path: Path) -> tuple[Keypair, Keypair, dict[str, Any]]:
    """Return (funder_kp, entity_kp, keyfile_dict). Refuses non-v2 files."""
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
            raise ValueError(f"{label}_seed_hex must be 64 hex chars, got {len(seed_hex)}")
    funder_kp = Keypair.from_seed(bytes.fromhex(funder_seed_hex))
    entity_kp = Keypair.from_seed(bytes.fromhex(entity_seed_hex))
    if str(data.get("funder_address_hex", funder_kp.address.hex())) != funder_kp.address.hex():
        raise ValueError("funder_address_hex in keyfile does not match seed; refusing to load")
    if str(data.get("entity_address_hex", entity_kp.address.hex())) != entity_kp.address.hex():
        raise ValueError("entity_address_hex in keyfile does not match seed; refusing to load")
    return funder_kp, entity_kp, data


def _default_fetch(url: str, timeout: float, model: str) -> GpuPriceObservation:
    return fetch_gpu_price(url=url, timeout=timeout, model=model)


class Oracle:
    """SIGTERM-aware main loop for the compute oracle."""

    def __init__(
        self,
        cfg: ComputeOracleConfig,
        chain: Chain,
        funder_kp: Keypair,
        entity_kp: Keypair,
        entity_id: bytes,
        registry: MetricsRegistry,
        *,
        dry_run: bool | None = None,
        fetch_fn: Callable[[str, float, str], GpuPriceObservation] = _default_fetch,
        sleep_fn: Callable[[float], None] = time.sleep,
        time_fn: Callable[[], float] = time.time,
        monotonic_fn: Callable[[], float] = time.monotonic,
    ) -> None:
        self.cfg = cfg
        self.chain = chain
        # funder_kp signs account-level ops (CreditAiEntity, faucet target).
        # entity_kp signs entity-bound signals (OracleAnchor, ReputationUpdate)
        # and holds the capability bits.
        self.funder_kp = funder_kp
        self.entity_kp = entity_kp
        self.entity_id = entity_id
        self.registry = registry
        self.dry_run = cfg.dry_run if dry_run is None else dry_run
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
            "oracle_start event=running mode=%s endpoint=%s entity_id=%s tag=%s "
            "model=%s interval=%.1f",
            "dry_run" if self.dry_run else "live",
            self.cfg.endpoint,
            self.entity_id.hex(),
            self.cfg.data_tag,
            self.cfg.gpu_model,
            self.cfg.loop_interval_secs,
        )
        while not self._stopping:
            try:
                self._tick()
            except Exception:  # noqa: BLE001 last-ditch
                LOG.exception("tick event=exception")
            self.registry.set_gauge(
                "novai_compute_oracle_last_loop_completed_timestamp", self.time_fn()
            )
            self._sliced_sleep(self._next_sleep_secs())
        LOG.info("oracle_stop event=clean")
        return 0

    def run_once(self) -> int:
        """Run a single cycle and return. Used for the DRY_RUN demonstration."""
        LOG.info(
            "oracle_start event=run_once mode=%s entity_id=%s tag=%s model=%s",
            "dry_run" if self.dry_run else "live",
            self.entity_id.hex(),
            self.cfg.data_tag,
            self.cfg.gpu_model,
        )
        try:
            self._tick()
        except Exception:  # noqa: BLE001 last-ditch
            LOG.exception("tick event=exception")
        self.registry.set_gauge(
            "novai_compute_oracle_last_loop_completed_timestamp", self.time_fn()
        )
        LOG.info("oracle_stop event=run_once_complete")
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
        self.registry.set_gauge("novai_compute_oracle_last_price_usd_per_hour", obs.price)
        self.registry.set_gauge("novai_compute_oracle_last_sample_size", float(obs.sample_size))
        # Funding top-ups touch the RPC; they are a live-only concern. In
        # dry_run the chain has no client and these would raise DryRunError.
        if not self.dry_run:
            self._maybe_top_up()
        self._submit(obs)

    # -- Funding (live only, mirrors price-oracle) ---------------------------

    def _maybe_top_up(self) -> None:
        self._maybe_credit_entity()
        self._maybe_faucet_account()

    def _maybe_credit_entity(self) -> None:
        """Tier 1: read entity.economic_balance and CreditAiEntity if low."""
        try:
            entity_balance = self.chain.get_entity_economic_balance(self.entity_id)
        except Exception as exc:  # noqa: BLE001 best-effort
            LOG.warning("entity_balance_read event=failed error=%s", exc)
            return
        self.registry.set_gauge("novai_compute_oracle_entity_balance", float(entity_balance))
        if entity_balance >= self.cfg.entity_min_balance:
            return
        now_mono = self.monotonic_fn()
        if self._next_credit_attempt_at is not None and now_mono < self._next_credit_attempt_at:
            LOG.info(
                "credit event=skipped reason=backoff retry_in_secs=%.0f entity_balance=%d",
                self._next_credit_attempt_at - now_mono,
                entity_balance,
            )
            return
        try:
            account_nonce = self.chain.get_account_nonce(self.funder_kp.address)
        except Exception as exc:  # noqa: BLE001
            reason = map_credit_error(exc)
            self._next_credit_attempt_at = now_mono + self.cfg.credit_retry_after_secs
            self.registry.inc_counter("novai_compute_oracle_credit_attempts_total", reason)
            LOG.warning("credit event=failed phase=nonce_read reason=%s error=%s", reason, exc)
            return
        try:
            result = self.chain.credit_entity(
                self.funder_kp, self.entity_id, self.cfg.credit_amount, nonce=account_nonce
            )
        except Exception as exc:  # noqa: BLE001
            reason = map_credit_error(exc)
            self._next_credit_attempt_at = now_mono + self.cfg.credit_retry_after_secs
            self.registry.inc_counter("novai_compute_oracle_credit_attempts_total", reason)
            LOG.warning(
                "credit event=failed phase=submit reason=%s error=%s entity_balance=%d nonce=%d",
                reason,
                exc,
                entity_balance,
                account_nonce,
            )
            return
        self._next_credit_attempt_at = now_mono + self.cfg.credit_retry_after_secs
        self.registry.inc_counter("novai_compute_oracle_credit_attempts_total", "success")
        LOG.info(
            "credit event=requested txid=%s amount=%d entity_balance_before=%d nonce=%d",
            result.txid,
            self.cfg.credit_amount,
            entity_balance,
            account_nonce,
        )

    def _maybe_faucet_account(self) -> None:
        """Tier 2: read account.balance and faucet when low."""
        try:
            balance = self.chain.get_balance(self.funder_kp.address)
        except Exception as exc:  # noqa: BLE001 best-effort
            LOG.warning("account_balance_read event=failed error=%s", exc)
            return
        self.registry.set_gauge("novai_compute_oracle_account_balance", float(balance))
        if balance >= self.cfg.account_min_balance:
            return
        now_mono = self.monotonic_fn()
        if self._next_faucet_attempt_at is not None and now_mono < self._next_faucet_attempt_at:
            LOG.info(
                "faucet event=skipped reason=backoff retry_in_secs=%.0f balance=%d",
                self._next_faucet_attempt_at - now_mono,
                balance,
            )
            return
        try:
            result = self.chain.faucet(self.funder_kp.address)
        except Exception as exc:  # noqa: BLE001
            reason = map_faucet_error(exc)
            self._next_faucet_attempt_at = now_mono + self.cfg.faucet_retry_after_secs
            self.registry.inc_counter("novai_compute_oracle_faucet_attempts_total", reason)
            LOG.warning("faucet event=failed reason=%s error=%s balance=%d", reason, exc, balance)
            return
        self._next_faucet_attempt_at = now_mono + self.cfg.faucet_retry_after_secs
        self.registry.inc_counter("novai_compute_oracle_faucet_attempts_total", "success")
        LOG.info(
            "faucet event=requested txid=%s amount=%s balance_before=%d",
            result.txid,
            result.amount,
            balance,
        )

    # -- Fetch + submit ------------------------------------------------------

    def _fetch_price(self) -> GpuPriceObservation | None:
        try:
            obs = self.fetch_fn(self.cfg.source_url, self.cfg.http_timeout_secs, self.cfg.gpu_model)
        except RateLimitError as exc:
            self.registry.inc_counter("novai_compute_oracle_price_fetch_failure_total", "rate_limit")
            delay = self.backoff.on_rate_limit()
            LOG.warning(
                "fetch event=rate_limited next_backoff_secs=%.0f retry_after_hint=%.0f",
                delay,
                exc.retry_after_secs,
            )
            return None
        except ServerError as exc:
            self.registry.inc_counter("novai_compute_oracle_price_fetch_failure_total", "server_error")
            LOG.warning("fetch event=server_error status=%d", exc.status)
            return None
        except NetworkError as exc:
            self.registry.inc_counter("novai_compute_oracle_price_fetch_failure_total", "network_error")
            LOG.warning("fetch event=network_error error=%s", exc)
            return None
        except ParseError as exc:
            self.registry.inc_counter("novai_compute_oracle_price_fetch_failure_total", "parse_error")
            LOG.warning("fetch event=parse_error error=%s", exc)
            return None
        except NoDataError as exc:
            self.registry.inc_counter("novai_compute_oracle_price_fetch_failure_total", "no_data")
            LOG.warning("fetch event=no_data error=%s", exc)
            return None
        except GpuSourceError as exc:
            self.registry.inc_counter("novai_compute_oracle_price_fetch_failure_total", "network_error")
            LOG.warning("fetch event=unknown_error error=%s", exc)
            return None
        self.backoff.reset()
        self.registry.inc_counter("novai_compute_oracle_price_fetch_success_total")
        LOG.info(
            "fetch event=ok model=%s price_usd_hr=%.4f sample=%d source=%s",
            obs.model,
            obs.price,
            obs.sample_size,
            obs.source,
        )
        return obs

    def _submit(self, obs: GpuPriceObservation) -> None:
        ts = int(self.time_fn())
        try:
            data_hash = build_data_hash(obs.model, obs.price, ts)
        except ValueError as exc:
            self.registry.inc_counter("novai_compute_oracle_submission_failure_total", "encoding_error")
            LOG.error("submit event=encoding_error error=%s", exc)
            return
        source_hash = source_hash_for(self.cfg.source_id)
        try:
            result = self.chain.post_anchor(
                self.entity_kp,
                self.entity_id,
                data_hash,
                ts,
                self.cfg.data_tag,
                source_hash=source_hash,
                expiry_height=self.cfg.expiry_height,
                fee=self.cfg.anchor_fee,
            )
        except Exception as exc:  # noqa: BLE001 mapped by map_submit_error
            reason = map_submit_error(exc)
            self.registry.inc_counter("novai_compute_oracle_submission_failure_total", reason)
            LOG.warning("submit event=failed reason=%s exc=%s error=%s", reason, type(exc).__name__, exc)
            return
        self._record_anchor_result(obs, ts, data_hash, source_hash, result)
        if self.cfg.reputation_enabled:
            self._submit_reputation()

    def _record_anchor_result(self, obs, ts, data_hash, source_hash, result) -> None:
        if isinstance(result, DryRunResult):
            self.registry.inc_counter(
                "novai_compute_oracle_dry_run_constructed_total", "oracle_anchor"
            )
            LOG.info(
                "dry_run event=constructed kind=oracle_anchor submitted=false model=%s "
                "price_usd_hr=%.4f ts=%d tag=%s data_hash=%s source_hash=%s signal_hash=%s "
                "payload_len=%d txid=%s",
                obs.model,
                obs.price,
                ts,
                self.cfg.data_tag,
                data_hash.hex(),
                source_hash.hex(),
                result.signal_hash_hex,
                result.payload_len,
                result.txid_hex,
            )
            for line in result.summary_lines():
                LOG.info("dry_run_detail %s", line)
            return
        height = self._latest_height_safe()
        if height is not None:
            self.registry.set_gauge("novai_compute_oracle_last_submission_height", height)
        self.registry.inc_counter("novai_compute_oracle_submission_success_total")
        LOG.info(
            "submit event=ok txid=%s signal_hash=%s model=%s price_usd_hr=%.4f tag=%s ts=%d height=%s",
            result.txid,
            result.signal_hash,
            obs.model,
            obs.price,
            self.cfg.data_tag,
            ts,
            "n/a" if height is None else height,
        )

    def _submit_reputation(self) -> None:
        """Optional ReputationUpdate demonstration (type 7)."""
        try:
            target = bytes.fromhex(self.cfg.reputation_target_hex)
        except ValueError:
            LOG.warning("reputation event=skipped reason=bad_target value=%s", self.cfg.reputation_target_hex)
            return
        if len(target) != 32:
            LOG.warning("reputation event=skipped reason=bad_target_len len=%d", len(target))
            return
        try:
            result = self.chain.submit_reputation_update(
                self.entity_kp,
                self.entity_id,
                target,
                self.cfg.reputation_event_type,
                self.cfg.reputation_points_delta,
                fee=self.cfg.reputation_fee,
            )
        except Exception as exc:  # noqa: BLE001
            reason = map_submit_error(exc)
            self.registry.inc_counter("novai_compute_oracle_submission_failure_total", reason)
            LOG.warning("reputation event=failed reason=%s error=%s", reason, exc)
            return
        if isinstance(result, DryRunResult):
            self.registry.inc_counter(
                "novai_compute_oracle_dry_run_constructed_total", "reputation_update"
            )
            LOG.info(
                "dry_run event=constructed kind=reputation_update submitted=false target=%s "
                "event_type=%d points_delta=%d signal_hash=%s payload_len=%d txid=%s",
                target.hex(),
                self.cfg.reputation_event_type,
                self.cfg.reputation_points_delta,
                result.signal_hash_hex,
                result.payload_len,
                result.txid_hex,
            )
            for line in result.summary_lines():
                LOG.info("dry_run_detail %s", line)
            return
        self.registry.inc_counter("novai_compute_oracle_submission_success_total")
        LOG.info("reputation event=ok txid=%s signal_hash=%s", result.txid, result.signal_hash)

    def _latest_height_safe(self) -> int | None:
        try:
            return self.chain.latest_block_height()
        except Exception as exc:  # noqa: BLE001 best-effort gauge
            LOG.debug("latest_block event=unavailable error=%s", exc)
            return None


def _load_or_generate_keys(
    cfg: ComputeOracleConfig, chain: Chain
) -> tuple[Keypair, Keypair, bytes]:
    """Load keys from the keyfile, or generate ephemeral keys in dry_run.

    Returns (funder_kp, entity_kp, entity_id). In live mode a keyfile is
    mandatory and its stored entity_id is authoritative. In dry_run with no
    keyfile, ephemeral keys are generated and the entity_id is derived locally
    (pure hashing, no RPC).
    """
    if cfg.key_path.exists():
        funder_kp, entity_kp, data = load_keypair_from_file(cfg.key_path)
        expected_hex = data.get("entity_id_hex")
        derived = chain.entity_id_for(funder_kp.address)
        if not expected_hex:
            if cfg.dry_run:
                LOG.warning("keys event=keyfile_missing_entity_id using=derived dry_run=true")
                return funder_kp, entity_kp, derived
            raise ValueError("keyfile missing entity_id_hex; re-run bootstrap")
        expected = bytes.fromhex(str(expected_hex))
        if expected != derived:
            raise ValueError(
                f"entity_id drift: keyfile={expected_hex} derived={derived.hex()}"
            )
        return funder_kp, entity_kp, expected
    if cfg.dry_run:
        funder_kp = Keypair.generate()
        entity_kp = Keypair.generate()
        entity_id = chain.entity_id_for(funder_kp.address)
        LOG.warning(
            "keys event=ephemeral_generated reason=dry_run_no_keyfile funder=%s entity=%s entity_id=%s",
            funder_kp.address.hex(),
            entity_kp.address.hex(),
            entity_id.hex(),
        )
        return funder_kp, entity_kp, entity_id
    raise FileNotFoundError(f"keyfile not found: {cfg.key_path}; run bootstrap first")


def main(argv: list[str] | None = None) -> int:
    cfg = ComputeOracleConfig.from_env()
    configure_logging(cfg.log_level)
    LOG.info(
        "oracle_init event=loading mode=%s endpoint=%s key_path=%s source=%s",
        "dry_run" if cfg.dry_run else "live",
        cfg.endpoint,
        cfg.key_path,
        cfg.source_id,
    )

    chain = Chain(cfg.endpoint, dry_run=cfg.dry_run, dry_run_nonce=cfg.dry_run_nonce)

    try:
        funder_kp, entity_kp, entity_id = _load_or_generate_keys(cfg, chain)
    except (OSError, json.JSONDecodeError, ValueError) as exc:
        LOG.error("oracle_init event=keyfile_error error=%s path=%s", exc, cfg.key_path)
        return 2

    # Live mode requires the entity to exist with the anchor capability. In
    # dry_run we cannot and must not query the chain, so we skip the check.
    if not cfg.dry_run:
        status = chain.get_entity_status(entity_id)
        if not status.exists or not status.has_post_oracle_anchors:
            LOG.error(
                "oracle_init event=entity_not_ready exists=%s bit_6=%s caps=0x%02x",
                status.exists,
                status.has_post_oracle_anchors,
                status.capabilities,
            )
            return 3

    registry = build_compute_oracle_registry(time.monotonic())
    if not cfg.run_once:
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
    if cfg.run_once:
        return oracle.run_once()
    return oracle.run_forever()


if __name__ == "__main__":
    sys.exit(main())
